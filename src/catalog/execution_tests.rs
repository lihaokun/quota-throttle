use super::*;
use crate::catalog::control::*;
use crate::catalog::execution::*;
use crate::newapi::managed_channels::{ChannelObservation, NativeRecoveryCheck};
use crate::reconcile::planner::*;
use std::time::{Duration, SystemTime};

#[path = "confirmation_tests.rs"]
mod confirmation;

fn op(n: usize) -> OperationId {
    OperationId::new(format!("execution-test-{n:016}")).unwrap()
}
fn fixture(kind: SourceKind, bound: bool) -> (TempDir, CatalogStore, MembershipId) {
    let (dir, mut store) = setup();
    let source = store
        .create_source(source("fake-execution-private-key", kind))
        .unwrap();
    let group = store.create_group(group("execution-a", kind)).unwrap();
    let id = store
        .plan_membership(source.source_id, group.group_id, 10)
        .unwrap()
        .membership_id;
    store
        .conn
        .execute("UPDATE source_groups SET ready=1", [])
        .unwrap();
    if bound {
        store
            .conn
            .execute(
                "UPDATE memberships SET channel_id=101,lifecycle='ready'",
                [],
            )
            .unwrap();
        store
            .conn
            .execute("UPDATE member_control SET create_state='resolved'", [])
            .unwrap();
    }
    store
        .submit_operation(
            op(0),
            ControlCommand::EnsureMember {
                membership_id: id,
                expected_revision: 1,
                status: StatusDirective::Enabled,
            },
            100,
        )
        .unwrap();
    (dir, store, id)
}
fn observation(work: &WorkSnapshot, status: i64) -> StampedObservation {
    StampedObservation {
        revision: work.revision(),
        channel: ChannelObservation {
            observed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
            channel_id: work.member.channel_id.map(ChannelId::get).unwrap_or(101),
            channel_type: 8,
            name: work.target.create_name.clone(),
            group: work.group.newapi_group.clone(),
            base_url: work.identity.base_url.clone(),
            models: work.models.join(","),
            priority: work.target.desired_priority,
            status,
            auto_ban: if work.source.kind == SourceKind::Volcano {
                0
            } else {
                1
            },
            is_multi_key: false,
        },
    }
}
fn context<'a>(
    work: &WorkSnapshot,
    members: &'a [MembershipId],
    peers: &'a [Peer],
) -> PlanContext<'a> {
    PlanContext {
        backend: BackendReadiness::from_preflight(
            SUPPORTED_BACKEND_VERSION,
            NativeRecoveryCheck::Disabled,
            true,
            90,
            200,
        ),
        permit: Some(RoutingPermit::from_policy(
            work,
            work.target.desired_priority,
            10,
            true,
            90,
            200,
        )),
        expected_members: members,
        peers,
        now: 100,
    }
}
fn prepare(store: &mut CatalogStore, id: MembershipId, status: i64) -> ControlResult<WorkTicket> {
    let work = store.load_work(id, false).unwrap();
    let observed = observation(&work, status);
    let members = [id];
    let ctx = context(&work, &members, &[]);
    store.prepare_execution(id, work.revision(), false, Some(&observed), &ctx)
}
fn counts(store: &CatalogStore) -> (i64, i64, i64) {
    store.conn.query_row("SELECT (SELECT count(*) FROM execution_attempts),(SELECT count(*) FROM operation_targets WHERE confirmed_at IS NOT NULL),(SELECT count(*) FROM memberships WHERE applied_revision IS NOT NULL)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap()
}
fn update(store: &mut CatalogStore, id: MembershipId, n: usize, explicit: bool) {
    let work = store.load_work(id, false).unwrap();
    let command = if explicit {
        ControlCommand::EnsureMember {
            membership_id: id,
            expected_revision: work.target.revision,
            status: StatusDirective::Enabled,
        }
    } else {
        ControlCommand::SyncSourceModels {
            source_id: work.source.source_id,
            expected_revision: work.source.config_revision,
            models: vec![format!("model-{n}")],
        }
    };
    store.submit_operation(op(n), command, 100).unwrap();
}
fn as_v2(store: &mut CatalogStore) {
    let tx = store.conn.transaction().unwrap();
    let instance: String = tx
        .query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    tx.execute_batch(
        "DROP TABLE group_policy_state; DROP TABLE volcano_runtime; DROP TABLE execution_attempts; DROP TABLE member_execution; DROP TABLE catalog_meta;",
    )
    .unwrap();
    tx.execute_batch(SCHEMA_V2.split(';').next().unwrap())
        .unwrap();
    tx.execute("INSERT INTO catalog_meta VALUES(1,2,?1)", [instance])
        .unwrap();
    tx.pragma_update(None, "user_version", 2).unwrap();
    tx.commit().unwrap();
}
fn v2_backups(dir: &TempDir) -> Vec<PathBuf> {
    fs::read_dir(dir.path())
        .unwrap()
        .map(|r| r.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("catalog-v2-before-v3-")
        })
        .collect()
}

#[test]
fn journal_prepare_and_ack_never_confirm_member_or_operation() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let before = store.load_work(id, false).unwrap();
    let ticket = prepare(&mut store, id, 2).unwrap();
    assert!(matches!(ticket.effect(), ExecutionEffect::Enable));
    assert_eq!(ticket.revision().execution, before.execution_revision + 1);
    assert_eq!(
        store.load_work(id, false).unwrap().pending_write,
        PendingWrite::InFlight
    );
    assert!(prepare(&mut store, id, 2).is_err());
    store
        .settle_execution(&ticket, ExecutionOutcome::Responded(None), 101)
        .unwrap();
    let after = store.load_work(id, false).unwrap();
    assert!(after.enable_consumed);
    assert_eq!(after.pending_write, PendingWrite::None);
    assert!(after.execution_revision > ticket.revision().execution);
    assert_eq!(counts(&store), (1, 0, 0));
    assert_eq!(
        store.operation(&op(0)).unwrap().phase,
        OperationPhase::Waiting
    );
    // 已终结写之前的 GET 不能被下一步复用。
    let old = observation(&before, 1);
    let members = [id];
    let ctx = context(&after, &members, &[]);
    assert_eq!(
        plan_next_action(&after, Some(&old), &ctx),
        PlanStep::Observe
    );
    assert_eq!(
        prepare(&mut store, id, 3).unwrap_err().code,
        ControlErrorCode::RemoteRejected
    );
}

#[test]
fn journal_duplicate_and_conflicting_receipts_are_not_new_effects() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let ticket = prepare(&mut store, id, 2).unwrap();
    store
        .settle_execution(&ticket, ExecutionOutcome::Responded(None), 101)
        .unwrap();
    let rev = store.load_work(id, false).unwrap().revision();
    for _ in 0..3 {
        store
            .settle_execution(&ticket, ExecutionOutcome::Responded(None), 105)
            .unwrap();
    }
    assert_eq!(store.load_work(id, false).unwrap().revision(), rev);
    for outcome in [
        ExecutionOutcome::Uncertain,
        ExecutionOutcome::NotSent(ControlErrorCode::AuthFailed),
        ExecutionOutcome::Responded(Some(ControlErrorCode::RemoteRejected)),
    ] {
        assert_eq!(
            store
                .settle_execution(&ticket, outcome, 105)
                .unwrap_err()
                .code,
            ControlErrorCode::RevisionConflict
        );
    }
    assert_eq!(counts(&store), (1, 0, 0));
}

#[test]
fn journal_cross_catalog_tickets_cannot_settle_identical_ids() {
    let (_a, mut a, ma) = fixture(SourceKind::Glm, true);
    let (_b, mut b, mb) = fixture(SourceKind::Glm, true);
    let ta = prepare(&mut a, ma, 2).unwrap();
    let _tb = prepare(&mut b, mb, 2).unwrap();
    assert_eq!(
        b.settle_execution(&ta, ExecutionOutcome::Responded(None), 101)
            .unwrap_err()
            .code,
        ControlErrorCode::RevisionConflict
    );
    assert!(!b.load_work(mb, false).unwrap().enable_consumed);
    assert_eq!(
        b.load_work(mb, false).unwrap().pending_write,
        PendingWrite::InFlight
    );
}

#[test]
fn journal_stale_each_generation_is_rejected_before_preparation() {
    for dimension in 0..4 {
        let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
        let work = store.load_work(id, false).unwrap();
        let obs = observation(&work, 2);
        let members = [id];
        let ctx = context(&work, &members, &[]);
        let mut stale = work.revision();
        match dimension {
            0 => stale.member += 1,
            1 => stale.source += 1,
            2 => stale.group += 1,
            _ => stale.execution += 1,
        }
        assert_eq!(
            store
                .prepare_execution(id, stale, false, Some(&obs), &ctx)
                .unwrap_err()
                .code,
            ControlErrorCode::RevisionConflict
        );
        assert_eq!(counts(&store), (0, 0, 0));
    }
}

#[test]
fn journal_unknown_write_only_allows_safety_disable_and_never_confirmation() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let enable = prepare(&mut store, id, 2).unwrap();
    store
        .settle_execution(&enable, ExecutionOutcome::Uncertain, 101)
        .unwrap();
    let w = store.load_work(id, false).unwrap();
    store
        .submit_operation(
            op(1),
            ControlCommand::SetSourcePaused {
                source_id: w.source.source_id,
                expected_revision: w.source.config_revision,
                paused: true,
            },
            102,
        )
        .unwrap();
    let disable = prepare(&mut store, id, 1).unwrap();
    assert!(matches!(disable.effect(), ExecutionEffect::Disable));
    store
        .settle_execution(&disable, ExecutionOutcome::Responded(None), 103)
        .unwrap();
    assert_eq!(
        store.load_work(id, false).unwrap().pending_write,
        PendingWrite::Unknown
    );
    assert_eq!(
        prepare(&mut store, id, 2).unwrap_err().code,
        ControlErrorCode::TransportUnknown
    );
    // 原票的确定终止证据迟到，只消除未决；不确认暂停，也不改新操作阶段。
    let receipt = store.operation(&op(1)).unwrap();
    store
        .settle_execution(&enable, ExecutionOutcome::Responded(None), 104)
        .unwrap();
    assert_eq!(store.operation(&op(1)).unwrap(), receipt);
    assert_eq!(
        store.load_work(id, false).unwrap().pending_write,
        PendingWrite::None
    );
    assert_eq!(counts(&store), (2, 0, 0));
}

#[test]
fn journal_late_ack_checks_source_and_group_even_without_new_member_target() {
    for table in ["sources", "source_groups"] {
        let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
        let ticket = prepare(&mut store, id, 2).unwrap();
        // 就绪/策略代次可变化而不重写操作目标；旧 ACK 不得更新其阶段。
        let column = if table == "sources" {
            "config_revision"
        } else {
            "revision"
        };
        store
            .conn
            .execute(&format!("UPDATE {table} SET {column}={column}+1"), [])
            .unwrap();
        let receipt = store.operation(&op(0)).unwrap();
        store
            .settle_execution(&ticket, ExecutionOutcome::Responded(None), 101)
            .unwrap();
        assert_eq!(store.operation(&op(0)).unwrap(), receipt);
        assert_eq!(counts(&store), (1, 0, 0));
    }
}

#[test]
fn journal_create_retries_only_after_proven_not_sent() {
    for outcome in [
        ExecutionOutcome::Responded(None),
        ExecutionOutcome::Responded(Some(ControlErrorCode::InvalidResponse)),
        ExecutionOutcome::Uncertain,
        ExecutionOutcome::NotSent(ControlErrorCode::AuthFailed),
    ] {
        let (_dir, mut store, id) = fixture(SourceKind::Volcano, false);
        let ticket = prepare(&mut store, id, 2).unwrap();
        assert!(matches!(ticket.effect(), ExecutionEffect::Create { .. }));
        store.settle_execution(&ticket, outcome, 101).unwrap();
        let work = store.load_work(id, false).unwrap();
        let members = [id];
        let ctx = context(&work, &members, &[]);
        if matches!(outcome, ExecutionOutcome::NotSent(_)) {
            assert_eq!(work.target.create_state, CreateState::Never);
            assert_eq!(
                plan_next_action(&work, None, &ctx),
                PlanStep::CreateDisabled
            );
            assert!(prepare(&mut store, id, 2).is_ok());
        } else {
            assert_eq!(work.target.create_state, CreateState::MayHaveBeenSent);
            assert_eq!(plan_next_action(&work, None, &ctx), PlanStep::LookupCreate);
            assert!(prepare(&mut store, id, 2).is_err());
            assert_eq!(counts(&store).0, 1);
        }
    }
}

#[test]
fn journal_prepare_and_settle_failures_roll_back_all_side_effects() {
    for table in [
        "member_execution",
        "execution_attempts",
        "member_control",
        "control_operations",
    ] {
        let (_dir, mut store, id) = fixture(SourceKind::Volcano, false);
        let before = store.member_target(id).unwrap();
        let verb = if table == "execution_attempts" {
            "INSERT"
        } else {
            "UPDATE"
        };
        store.conn.execute_batch(&format!("CREATE TRIGGER fail_journal BEFORE {verb} ON {table} BEGIN SELECT RAISE(ABORT,'fixture'); END;")).unwrap();
        assert!(prepare(&mut store, id, 2).is_err());
        assert_eq!(store.member_target(id).unwrap(), before);
        assert_eq!(counts(&store), (0, 0, 0));
        assert_eq!(store.load_work(id, false).unwrap().execution_revision, 0);
    }
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let ticket = prepare(&mut store, id, 2).unwrap();
    store.conn.execute_batch("CREATE TRIGGER fail_settle BEFORE UPDATE ON member_execution BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(store
        .settle_execution(&ticket, ExecutionOutcome::Responded(None), 101)
        .is_err());
    let w = store.load_work(id, false).unwrap();
    assert!(!w.enable_consumed);
    assert_eq!(w.pending_write, PendingWrite::InFlight);
}

#[test]
fn journal_auth_epoch_overflow_rolls_back_new_intent() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    store
        .conn
        .execute("UPDATE member_execution SET enable_epoch=?1", [JSON_SAFE])
        .unwrap();
    let before = store.member_target(id).unwrap();
    assert_eq!(
        store
            .submit_operation(
                op(1),
                ControlCommand::EnsureMember {
                    membership_id: id,
                    expected_revision: before.revision,
                    status: StatusDirective::Enabled
                },
                101
            )
            .unwrap_err()
            .code,
        ControlErrorCode::InvalidInput
    );
    assert_eq!(store.member_target(id).unwrap(), before);
    assert_eq!(
        store.operation(&op(0)).unwrap().phase,
        OperationPhase::Queued
    );
}

#[test]
fn journal_other_members_ack_cannot_hide_an_unknown_target() {
    let (_dir, mut store, a) = fixture(SourceKind::Glm, true);
    let source = store.load_work(a, false).unwrap().source;
    let group = store
        .create_group(group("other-group", SourceKind::Glm))
        .unwrap();
    let b = store
        .plan_membership(source.source_id, group.group_id, 10)
        .unwrap()
        .membership_id;
    store
        .conn
        .execute(
            "UPDATE memberships SET channel_id=102,lifecycle='ready' WHERE membership_id=?1",
            [b.get()],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
            [b.get()],
        )
        .unwrap();
    store
        .submit_operation(
            op(1),
            ControlCommand::SetSourcePaused {
                source_id: source.source_id,
                expected_revision: source.config_revision,
                paused: true,
            },
            100,
        )
        .unwrap();
    let ta = prepare(&mut store, a, 1).unwrap();
    store
        .settle_execution(&ta, ExecutionOutcome::Uncertain, 101)
        .unwrap();
    let tb = prepare(&mut store, b, 1).unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Unknown
    );
    store
        .settle_execution(&tb, ExecutionOutcome::Responded(None), 102)
        .unwrap();
    let receipt = store.operation(&op(1)).unwrap();
    assert_eq!(receipt.phase, OperationPhase::Unknown);
    assert_eq!(receipt.error_code, Some(ControlErrorCode::TransportUnknown));
    assert_eq!(receipt.confirmed_targets, 0);
}

#[test]
fn journal_late_response_does_not_hide_a_prepared_compensation() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let source = store.load_work(id, false).unwrap().source;
    store
        .submit_operation(
            op(1),
            ControlCommand::SetSourcePaused {
                source_id: source.source_id,
                expected_revision: source.config_revision,
                paused: true,
            },
            100,
        )
        .unwrap();
    let a = prepare(&mut store, id, 1).unwrap();
    store
        .settle_execution(&a, ExecutionOutcome::Uncertain, 100)
        .unwrap();
    let b = prepare(&mut store, id, 1).unwrap();
    store
        .settle_execution(&a, ExecutionOutcome::Responded(None), 101)
        .unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Applying
    );
    assert_eq!(
        store.load_work(id, false).unwrap().pending_write,
        PendingWrite::InFlight
    );
    store
        .settle_execution(&b, ExecutionOutcome::Responded(None), 102)
        .unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Waiting
    );
}

#[test]
fn journal_volcano_enable_is_not_subject_to_glm_one_shot() {
    let (_dir, mut store, id) = fixture(SourceKind::Volcano, true);
    let ticket = prepare(&mut store, id, 2).unwrap();
    store
        .settle_execution(&ticket, ExecutionOutcome::Responded(None), 101)
        .unwrap();
    assert!(store.load_work(id, false).unwrap().enable_consumed);
    assert!(matches!(
        prepare(&mut store, id, 2).unwrap().effect(),
        ExecutionEffect::Enable
    ));
}

#[test]
fn journal_group_snapshot_has_bounded_cost() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let own = store.load_work(id, false).unwrap();
    for n in 0..128 {
        let source = store
            .create_source(source(&format!("fake-perf-{n}"), SourceKind::Glm))
            .unwrap();
        store
            .plan_membership(source.source_id, own.group.group_id, n)
            .unwrap();
    }
    let started = std::time::Instant::now();
    assert_eq!(
        store.load_group_work(own.group.group_id).unwrap().len(),
        129
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn journal_group_snapshot_keeps_retiring_and_rejects_forged_barrier() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let own = store.load_work(id, false).unwrap();
    let other_source = store
        .create_source(source("fake-peer-key", SourceKind::Glm))
        .unwrap();
    let peer = store
        .plan_membership(other_source.source_id, own.group.group_id, 100)
        .unwrap();
    store
        .submit_operation(
            op(1),
            ControlCommand::RetireMember {
                membership_id: peer.membership_id,
                expected_revision: 1,
            },
            100,
        )
        .unwrap();
    let all = store.load_group_work(own.group.group_id).unwrap();
    assert_eq!(all.len(), 2);
    assert!(all[1].target.retire_requested);
    // 计划器可接受的“只有自己”的集合，与数据库实际集合不一致。
    assert_eq!(
        prepare(&mut store, id, 2).unwrap_err().code,
        ControlErrorCode::RevisionConflict
    );
    assert_eq!(counts(&store).0, 0);
}

#[test]
fn journal_v3_failure_preserves_v2_and_private_backup() {
    for collision in ["member_execution", "execution_attempts"] {
        let (dir, mut store, id) = fixture(SourceKind::Glm, true);
        let before = store.snapshot().unwrap();
        let receipt = store.operation(&op(0)).unwrap();
        as_v2(&mut store);
        store
            .conn
            .execute_batch(&format!(
                "CREATE TABLE {collision}(marker TEXT); INSERT INTO {collision} VALUES('fixture');"
            ))
            .unwrap();
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
        let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
        verify_schema(&conn, 2).unwrap();
        assert_eq!(read_snapshot(&conn).unwrap(), before);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        let files = v2_backups(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(
            fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let backup = Connection::open(&files[0]).unwrap();
        verify_schema(&backup, 2).unwrap();
        assert_eq!(read_snapshot(&backup).unwrap(), before);
        drop(backup);
        conn.execute(&format!("DROP TABLE {collision}"), [])
            .unwrap();
        drop(conn);
        let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(store.operation(&op(0)).unwrap(), receipt);
        assert_eq!(store.load_work(id, false).unwrap().execution_revision, 0);
        assert_eq!(v2_backups(&dir).len(), 2);
    }
}

#[test]
fn journal_v3_corruption_is_rejected_without_repair() {
    for corrupt in ["DELETE FROM member_execution", "UPDATE member_execution SET enable_requested_revision=100", "UPDATE execution_attempts SET effect_json='{\"action\":\"disable\",\"credential\":\"fake\"}'", "UPDATE execution_attempts SET action='disable'", "UPDATE execution_attempts SET sequence=100", "DROP INDEX execution_one_prepared"] {
        let (dir,mut store,id)=fixture(SourceKind::Glm,true);prepare(&mut store,id,2).unwrap();
        store.conn.pragma_update(None,"foreign_keys",false).unwrap();store.conn.execute(corrupt,[]).unwrap();drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err(),"{corrupt}");
    }
}

#[test]
fn journal_effects_and_debug_exclude_private_material() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, false);
    let ticket = prepare(&mut store, id, 2).unwrap();
    let debug = format!("{ticket:?} {:?}", ticket.effect());
    assert!(!debug.contains("fake-execution-private-key"));
    assert!(!debug.contains("example.test"));
    let json: String = store
        .conn
        .query_row("SELECT effect_json FROM execution_attempts", [], |r| {
            r.get(0)
        })
        .unwrap();
    for forbidden in [
        "fake-execution-private-key",
        "credential",
        "raw_key",
        "quota_url",
        "global_headers",
        "key_headers",
        "fingerprint",
    ] {
        assert!(!json.contains(forbidden));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn journal_pbt_out_of_order_duplicate_settlements_keep_unresolved_writes(n in 1usize..8,order in prop::collection::vec(0usize..40,1..30)) {
        let (_dir,mut store,id)=fixture(SourceKind::Glm,true);
        let source=store.load_work(id,false).unwrap().source;
        store.submit_operation(op(1),ControlCommand::SetSourcePaused {source_id:source.source_id,expected_revision:source.config_revision,paused:true},100).unwrap();
        let mut tickets=Vec::new();
        for _ in 0..n {
            let ticket=prepare(&mut store,id,1).unwrap();
            store.settle_execution(&ticket,ExecutionOutcome::Uncertain,100).unwrap();tickets.push(ticket);
        }
        let mut resolved=vec![false;n];
        for index in order.into_iter().chain(0..n) {
            let index=index%n;
            store.settle_execution(&tickets[index],ExecutionOutcome::Responded(None),101).unwrap();resolved[index]=true;
            let all=resolved.iter().all(|x|*x);
            prop_assert_eq!(store.load_work(id,false).unwrap().pending_write,if all {PendingWrite::None}else {PendingWrite::Unknown});
            prop_assert_eq!(store.operation(&op(1)).unwrap().phase,if all {OperationPhase::Waiting}else {OperationPhase::Unknown});
            prop_assert_eq!(counts(&store),(n as i64,0,0));
        }
    }
    #[test]
    fn journal_pbt_late_ack_consumes_only_its_authorization(updates in prop::collection::vec(any::<bool>(),1..12), ack_after in 0usize..12) {
        let (_dir,mut store,id)=fixture(SourceKind::Glm,true);
        let ticket=prepare(&mut store,id,2).unwrap();
        let mut epoch=1;let mut consumed=false;let mut acked=false;
        for (i,explicit) in updates.iter().enumerate() {
            if !acked && i>=ack_after {store.settle_execution(&ticket,ExecutionOutcome::Responded(None),100).unwrap();consumed=epoch==1;acked=true;}
            update(&mut store,id,i+1,*explicit);
            if *explicit {epoch+=1;consumed=false;}
            let w=store.load_work(id,false).unwrap();
            prop_assert_eq!(w.enable_consumed,consumed);
            prop_assert_eq!(counts(&store),(1,0,0));
        }
        let latest=store.operation(&op(updates.len())).unwrap();
        store.settle_execution(&ticket,ExecutionOutcome::Responded(None),101).unwrap();
        if !acked {consumed=epoch==1;}
        let w=store.load_work(id,false).unwrap();
        prop_assert_eq!(w.enable_consumed,consumed);
        prop_assert_eq!(store.operation(&op(updates.len())).unwrap(),latest);
        prop_assert_eq!(counts(&store),(1,0,0));
        if consumed {
            // 普通目标更新继承已消费授权：后续原生 3 保留，不重新启用。
            let o=observation(&w,3);let members=[id];let ctx=context(&w,&members,&[]);
            prop_assert_eq!(plan_next_action(&w,Some(&o),&ctx),PlanStep::Confirm);
        }
    }
    #[test]
    fn journal_pbt_crash_and_restart_never_rearms_possible_create(paused in any::<bool>(),retire in any::<bool>(),restarts in 1usize..4) {
        let (dir,mut store,id)=fixture(SourceKind::Volcano,false);let ticket=prepare(&mut store,id,2).unwrap();
        let source=store.load_work(id,false).unwrap().source;
        if paused {store.submit_operation(op(1),ControlCommand::SetSourcePaused {source_id:source.source_id,expected_revision:source.config_revision,paused:true},100).unwrap();}
        if retire {let revision=store.member_target(id).unwrap().revision;store.submit_operation(op(2),ControlCommand::RetireMember {membership_id:id,expected_revision:revision},100).unwrap();}
        let snapshot=store.snapshot().unwrap();drop(store);
        for _ in 0..restarts {
            store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
            prop_assert_eq!(store.snapshot().unwrap(),snapshot.clone());
            let work=store.load_work(id,false).unwrap();prop_assert_eq!(work.pending_write,PendingWrite::Unknown);
            prop_assert_eq!(work.source.manual_paused,paused);prop_assert_eq!(work.target.create_state,CreateState::MayHaveBeenSent);
            prop_assert!(work.execution_revision>ticket.revision().execution);
            prop_assert!(prepare(&mut store,id,2).is_err());prop_assert_eq!(counts(&store),(1,0,0));drop(store);
        }
    }
    #[test]
    fn journal_pbt_v2_migration_preserves_full_records(n in 1usize..6,consumed in any::<bool>()) {
        let (dir,mut store)=setup();let (mut cfg,mut channels)=legacy(n);
        for (i,c) in channels.iter_mut().enumerate() {c.models_csv=format!("glm-model-{i},shared");}
        for (i,k) in cfg.keys.iter_mut().enumerate() {k.quota_headers.push(HeaderKV {key:"X-Second".into(),value:format!("different-{i}")});}
        let receipt=match store.import_legacy(&build_legacy_import(&cfg,&channels).unwrap()).unwrap() {ImportOutcome::Applied(r)=>r,_=>panic!("fresh fixture")};
        for (i,id) in receipt.membership_ids.iter().enumerate() {store.submit_operation(op(i),ControlCommand::EnsureMember {membership_id:*id,expected_revision:1,status:StatusDirective::Enabled},100).unwrap();}
        if consumed {store.conn.execute("UPDATE memberships SET applied_revision=2",[]).unwrap();}
        let mut volcano=source("fake-migration-volcano",SourceKind::Volcano);
        if let SourceSettings::Volcano {upstream,policy}=&mut volcano.settings {
            upstream.models=vec![format!("volcano-model-{n}")];policy.reset_minute=n as i64*7;policy.daily_token_limit=Some(n as i64*1234);
        }
        let volcano=store.create_source(volcano).unwrap();let vg=store.create_group(group("volcano-migration",SourceKind::Volcano)).unwrap();
        store.plan_membership(volcano.source_id,vg.group_id,37).unwrap();
        let before=store.snapshot().unwrap();let instance:String=store.conn.query_row("SELECT catalog_instance_id FROM catalog_meta",[],|r|r.get(0)).unwrap();
        let materials:Vec<_>=before.sources.iter().map(|s| {let m=store.read_material(s.source_id,true).unwrap();(s.source_id,serde_json::to_value(&m.settings).unwrap(),m.credential.expose().to_owned())}).collect();
        let operations:Vec<_>=(0..n).map(|i|store.operation(&op(i)).unwrap()).collect();
        as_v2(&mut store);drop(store);
        store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        prop_assert_eq!(store.snapshot().unwrap(),before);
        prop_assert_eq!(store.conn.query_row("SELECT catalog_instance_id FROM catalog_meta",[],|r|r.get::<_,String>(0)).unwrap(),instance);
        for (id,settings,key) in materials {let actual=store.read_material(id,true).unwrap();prop_assert_eq!(serde_json::to_value(actual.settings).unwrap(),settings);prop_assert_eq!(actual.credential.expose(),key.as_str());}
        for (i,id) in receipt.membership_ids.iter().enumerate() {
            prop_assert_eq!(store.operation(&op(i)).unwrap(),operations[i].clone());
            let w=store.load_work(*id,false).unwrap();prop_assert_eq!(w.enable_consumed,consumed);prop_assert_eq!(w.enable_requested_revision,2);
            let material=store.read_material(w.source.source_id,true).unwrap();
            prop_assert_eq!(material.credential.expose(),cfg.keys[i].zhipu_api_key.as_str());
            if let SourceSettings::Glm {quota,..}=material.settings {prop_assert_eq!(quota.key_headers[0].value.as_str(),cfg.keys[i].quota_headers[0].value.as_str());}
        }
        prop_assert_eq!(v2_backups(&dir).len(),1);
    }
}
