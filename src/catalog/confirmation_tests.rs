use super::*;
use crate::catalog::confirmation::ObservedFields;

#[test]
fn confirmation_record_observed_validates_batch_before_writing() {
    let (_dir, mut store, id) = fixture(SourceKind::Volcano, true);
    let w = store.load_work(id, false).unwrap();
    let before = serde_json::to_value(store.snapshot().unwrap()).unwrap();
    for (status, priority) in [
        (Some(1), None),
        (None, Some(0)),
        (Some(0), Some(0)),
        (Some(4), Some(0)),
        (Some(1), Some(JSON_SAFE + 1)),
    ] {
        assert_eq!(
            store
                .record_observed(&[ObservedFields {
                    member: id,
                    revision: w.revision(),
                    status,
                    priority
                }])
                .unwrap_err()
                .code,
            ControlErrorCode::InvalidInput
        );
        assert_eq!(
            serde_json::to_value(store.snapshot().unwrap()).unwrap(),
            before
        );
    }
    let row = || ObservedFields {
        member: id,
        revision: w.revision(),
        status: Some(2),
        priority: Some(0),
    };
    assert_eq!(
        store.record_observed(&[row(), row()]).unwrap_err().code,
        ControlErrorCode::InvalidInput
    );
    assert_eq!(
        serde_json::to_value(store.snapshot().unwrap()).unwrap(),
        before
    );
}

#[test]
fn confirmation_record_observed_database_failure_rolls_back_whole_batch() {
    let (_dir, mut store, ids) = members(2, SourceKind::Volcano);
    let updates: Vec<_> = ids
        .iter()
        .map(|id| ObservedFields {
            member: *id,
            revision: store.load_work(*id, false).unwrap().revision(),
            status: Some(2),
            priority: Some(99),
        })
        .collect();
    let before = serde_json::to_value(store.snapshot().unwrap()).unwrap();
    let history = receipts(&store);
    store.conn.execute_batch(&format!("CREATE TEMP TRIGGER reject_observed BEFORE UPDATE OF observed_status ON memberships WHEN OLD.membership_id={} BEGIN SELECT RAISE(ABORT,'fixture'); END;", ids[1].get())).unwrap();
    assert_eq!(
        store.record_observed(&updates).unwrap_err().code,
        ControlErrorCode::Storage
    );
    assert_eq!(
        serde_json::to_value(store.snapshot().unwrap()).unwrap(),
        before
    );
    assert_eq!(receipts(&store), history);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn confirmation_pbt_observation_is_atomic_diagnostic_only(n in 2usize..6, stale in 0usize..5, field in 0usize..5, status in 1i64..4, priority in -1000i64..1000) {
        let (_dir,mut store,ids)=members(n,SourceKind::Volcano);
        stop_all(&mut store,ids[0],false);
        let before=serde_json::to_value(store.snapshot().unwrap()).unwrap();
        let history=receipts(&store);let op_before=store.operation(&op(1)).unwrap();
        let revisions:Vec<_>=ids.iter().map(|id|store.load_work(*id,false).unwrap().revision()).collect();
        let mut updates:Vec<_>=ids.iter().zip(&revisions).map(|(id,r)|ObservedFields {member:*id,revision:*r,status:Some(status),priority:Some(priority)}).collect();
        let changed=&mut updates[stale%n].revision;
        match field {0=>changed.member+=1,1=>changed.source+=1,2=>changed.group+=1,3=>changed.execution+=1,_=>{}}
        let result=store.record_observed(&updates);
        if field<4 {
            prop_assert_eq!(result.unwrap_err().code,ControlErrorCode::RevisionConflict);
            prop_assert_eq!(serde_json::to_value(store.snapshot().unwrap()).unwrap(),before);
        } else {
            prop_assert!(result.is_ok());
            for (id,revision) in ids.iter().zip(revisions) {
                let w=store.load_work(*id,false).unwrap();
                prop_assert_eq!(w.revision(),revision);
                prop_assert_eq!((w.member.observed_status,w.member.observed_priority),(Some(status),Some(priority)));
                prop_assert_eq!(w.member.applied_revision,None);prop_assert!(w.target.dirty);
            }
            for row in &mut updates {row.status=None;row.priority=None;}
            store.record_observed(&updates).unwrap();
            prop_assert_eq!(serde_json::to_value(store.snapshot().unwrap()).unwrap(),before);
        }
        prop_assert_eq!(receipts(&store),history);prop_assert_eq!(store.operation(&op(1)).unwrap(),op_before);
    }
}

fn confirm(store: &mut CatalogStore, id: MembershipId, status: i64) -> ControlResult<()> {
    let work = store.load_work(id, false).unwrap();
    let obs = observation(&work, status);
    let members = [id];
    let ctx = context(&work, &members, &[]);
    store.confirm_member(id, work.revision(), false, Some(&obs), &ctx)
}
fn bind(
    store: &mut CatalogStore,
    ticket: &WorkTicket,
    channels: &[ChannelObservation],
    complete: bool,
) -> ControlResult<ChannelId> {
    let work = store.load_work(ticket.membership_id(), false).unwrap();
    let members = [ticket.membership_id()];
    let ctx = context(&work, &members, &[]);
    store.bind_created(ticket, complete, channels, ctx.backend, 100)
}
fn members(n: usize, kind: SourceKind) -> (TempDir, CatalogStore, Vec<MembershipId>) {
    let (dir, mut store, a) = fixture(kind, true);
    let source = store.load_work(a, false).unwrap().source.source_id;
    let mut result = vec![a];
    for i in 1..n {
        let g = store
            .create_group(group(&format!("group-{i}"), kind))
            .unwrap();
        let id = store
            .plan_membership(source, g.group_id, 10 + i as i64)
            .unwrap()
            .membership_id;
        store
            .conn
            .execute(
                "UPDATE memberships SET channel_id=?1,lifecycle='ready' WHERE membership_id=?2",
                params![101 + i as i64, id.get()],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
                [id.get()],
            )
            .unwrap();
        result.push(id);
    }
    store
        .conn
        .execute("UPDATE source_groups SET ready=1", [])
        .unwrap();
    (dir, store, result)
}
fn stop_all(store: &mut CatalogStore, id: MembershipId, retire: bool) {
    let source = store.load_work(id, false).unwrap().source;
    let command = if retire {
        ControlCommand::RetireSource {
            source_id: source.source_id,
            expected_revision: source.config_revision,
        }
    } else {
        ControlCommand::SetSourcePaused {
            source_id: source.source_id,
            expected_revision: source.config_revision,
            paused: true,
        }
    };
    store.submit_operation(op(1), command, 100).unwrap();
}
fn receipts(store: &CatalogStore) -> Vec<(String, i64, Option<i64>, Option<String>)> {
    store.conn.prepare("SELECT operation_id,membership_id,confirmed_at,receipt_json FROM operation_targets ORDER BY operation_id,membership_id").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().map(|r|r.unwrap()).collect()
}

#[test]
fn confirmation_create_bind_then_fresh_enable_and_readback_complete() {
    let (_dir, mut store, id) = fixture(SourceKind::Volcano, false);
    let creation = prepare(&mut store, id, 2).unwrap();
    store
        .settle_execution(&creation, ExecutionOutcome::Responded(None), 100)
        .unwrap();
    let w = store.load_work(id, false).unwrap();
    let channel = observation(&w, 2).channel;
    assert_eq!(
        bind(&mut store, &creation, std::slice::from_ref(&channel), true)
            .unwrap()
            .get(),
        101
    );
    let bound = store.load_work(id, false).unwrap();
    assert!(bound.execution_revision > w.execution_revision);
    assert_eq!(bound.member.lifecycle, MembershipLifecycle::CreatedDisabled);
    assert_eq!(counts(&store), (1, 0, 0));
    assert_eq!(
        bind(&mut store, &creation, &[channel], true).unwrap().get(),
        101
    );
    assert_eq!(
        store.load_work(id, false).unwrap().revision(),
        bound.revision()
    );
    let enable = prepare(&mut store, id, 2).unwrap();
    assert!(matches!(enable.effect(), ExecutionEffect::Enable));
    store
        .settle_execution(&enable, ExecutionOutcome::Responded(None), 100)
        .unwrap();
    assert_eq!(store.operation(&op(0)).unwrap().confirmed_targets, 0);
    confirm(&mut store, id, 1).unwrap();
    assert_eq!(
        store.operation(&op(0)).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert!(store.due_members(100).unwrap().is_empty());
}

#[test]
fn confirmation_unknown_creation_materializes_without_fabricating_a_response() {
    for settle in [false, true] {
        let (dir, mut store, id) = fixture(SourceKind::Glm, false);
        let ticket = prepare(&mut store, id, 2).unwrap();
        if settle {
            store
                .settle_execution(&ticket, ExecutionOutcome::Uncertain, 100)
                .unwrap();
        }
        stop_all(&mut store, id, true);
        let w = store.load_work(id, false).unwrap();
        let channel = observation(&w, 2).channel;
        bind(&mut store, &ticket, &[channel], true).unwrap();
        let state: String = store
            .conn
            .query_row("SELECT state FROM execution_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "uncertain");
        assert_eq!(
            store.load_work(id, false).unwrap().pending_write,
            PendingWrite::None
        );
        drop(store);
        let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(
            store.load_work(id, false).unwrap().pending_write,
            PendingWrite::None
        );
        assert_ne!(
            store.operation(&op(1)).unwrap().phase,
            OperationPhase::Unknown
        );
        confirm(&mut store, id, 2).unwrap();
        let w = store.load_work(id, false).unwrap();
        assert_eq!(w.member.channel_id.unwrap().get(), 101);
        assert_eq!(w.member.lifecycle, MembershipLifecycle::Retired);
        assert_eq!(w.source.retired_at, Some(100));
        assert_eq!(
            store.operation(&op(1)).unwrap().phase,
            OperationPhase::Succeeded
        );
    }
}

#[test]
fn confirmation_binding_rejects_missing_duplicate_drift_stale_and_foreign_ticket() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, false);
    let ticket = prepare(&mut store, id, 2).unwrap();
    let w = store.load_work(id, false).unwrap();
    let channel = observation(&w, 2).channel;
    assert_eq!(
        bind(&mut store, &ticket, &[], true).unwrap_err().code,
        ControlErrorCode::TransportUnknown
    );
    assert_eq!(
        bind(&mut store, &ticket, std::slice::from_ref(&channel), false)
            .unwrap_err()
            .code,
        ControlErrorCode::InvalidResponse
    );
    let mut duplicate = channel.clone();
    duplicate.channel_id += 1;
    assert_eq!(
        bind(&mut store, &ticket, &[channel.clone(), duplicate], true)
            .unwrap_err()
            .code,
        ControlErrorCode::AmbiguousCreate
    );
    for index in 0..6 {
        let mut bad = channel.clone();
        match index {
            0 => bad.group = "foreign".into(),
            1 => bad.channel_type = 1,
            2 => bad.base_url = "https://foreign.invalid".into(),
            3 => bad.is_multi_key = true,
            4 => bad.observed_at = SystemTime::UNIX_EPOCH,
            _ => bad.observed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(101),
        };
        assert!(bind(&mut store, &ticket, &[bad], true).is_err());
    }
    let (_other, mut other, other_id) = fixture(SourceKind::Glm, false);
    prepare(&mut other, other_id, 2).unwrap();
    assert_eq!(
        bind(&mut other, &ticket, std::slice::from_ref(&channel), true)
            .unwrap_err()
            .code,
        ControlErrorCode::RevisionConflict
    );
    assert!(store
        .load_work(id, false)
        .unwrap()
        .member
        .channel_id
        .is_none());
    bind(&mut store, &ticket, std::slice::from_ref(&channel), true).unwrap();
    let mut moved = channel;
    moved.channel_id += 100;
    assert_eq!(
        bind(&mut store, &ticket, &[moved], true).unwrap_err().code,
        ControlErrorCode::BindingDrift
    );
}

#[test]
fn confirmation_prepared_create_bind_immediately_enables_without_reopening() {
    let (dir, mut store, id) = fixture(SourceKind::Volcano, false);
    let create = prepare(&mut store, id, 2).unwrap();
    let work = store.load_work(id, false).unwrap();
    let channel = observation(&work, 2).channel;
    bind(&mut store, &create, &[channel], true).unwrap();
    let bound = store.load_work(id, false).unwrap().revision();
    assert_eq!(
        store
            .settle_execution(
                &create,
                ExecutionOutcome::NotSent(ControlErrorCode::RevisionConflict),
                100
            )
            .unwrap_err()
            .code,
        ControlErrorCode::RevisionConflict
    );
    assert_eq!(store.load_work(id, false).unwrap().revision(), bound);
    let enable = prepare(&mut store, id, 2).unwrap();
    assert!(matches!(enable.effect(), ExecutionEffect::Enable));
    store
        .settle_execution(&enable, ExecutionOutcome::Responded(None), 100)
        .unwrap();
    confirm(&mut store, id, 1).unwrap();
    let success = store.operation(&op(0)).unwrap();
    store
        .settle_execution(&create, ExecutionOutcome::Responded(None), 101)
        .unwrap();
    assert_eq!(store.operation(&op(0)).unwrap(), success);
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(store.operation(&op(0)).unwrap(), success);
}

#[test]
fn confirmation_binding_respects_reserved_channel_and_rolls_back_every_write() {
    for table in [
        "memberships",
        "member_control",
        "member_execution",
        "execution_attempts",
        "control_operations",
    ] {
        let (_dir, mut store, id) = fixture(SourceKind::Glm, false);
        let ticket = prepare(&mut store, id, 2).unwrap();
        let work = store.load_work(id, false).unwrap();
        let channel = observation(&work, 2).channel;
        store.conn.execute_batch(&format!("CREATE TRIGGER fail_bind BEFORE UPDATE ON {table} BEGIN SELECT RAISE(ABORT,'fixture'); END;")).unwrap();
        assert!(bind(&mut store, &ticket, &[channel], true).is_err());
        let after = store.load_work(id, false).unwrap();
        assert_eq!(after.revision(), work.revision());
        assert!(after.member.channel_id.is_none());
        assert_eq!(after.target.create_state, CreateState::MayHaveBeenSent);
        assert_eq!(
            store
                .conn
                .query_row("SELECT state FROM execution_attempts", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "prepared"
        );
    }
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let own = store.load_work(id, false).unwrap();
    let source = store
        .create_source(source("fake-conflict-other", SourceKind::Glm))
        .unwrap();
    let other = store
        .plan_membership(source.source_id, own.group.group_id, 10)
        .unwrap()
        .membership_id;
    store
        .submit_operation(
            op(2),
            ControlCommand::EnsureMember {
                membership_id: other,
                expected_revision: 1,
                status: StatusDirective::Enabled,
            },
            100,
        )
        .unwrap();
    let ticket = prepare(&mut store, other, 2).unwrap();
    let work = store.load_work(other, false).unwrap();
    let channel = observation(&work, 2).channel;
    assert_eq!(channel.channel_id, 101);
    assert_eq!(
        bind(&mut store, &ticket, &[channel], true)
            .unwrap_err()
            .code,
        ControlErrorCode::BindingDrift
    );
    assert_eq!(store.load_work(id, false).unwrap().member, own.member);
}

#[test]
fn confirmation_retry_preserves_consumed_glm_epoch_and_preserve_ownership() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    confirm(&mut store, id, 1).unwrap();
    update(&mut store, id, 1, false);
    let epoch = |store: &CatalogStore| {
        store.conn.query_row("SELECT enable_epoch,enable_requested_revision,enable_consumed FROM member_execution",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,bool>(2)?))).unwrap()
    };
    let before = epoch(&store);
    assert!(before.2);
    let work = store.load_work(id, false).unwrap();
    store
        .defer_member(
            id,
            work.revision(),
            ControlErrorCode::RemoteRejected,
            100,
            None,
        )
        .unwrap();
    assert_eq!(
        store.retry_operation(&op(1), 100).unwrap().phase,
        OperationPhase::Queued
    );
    assert_eq!(epoch(&store), before);
    assert_eq!(
        store.member_target(id).unwrap().status_directive,
        StatusDirective::Preserve
    );
    confirm(&mut store, id, 3).unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Succeeded
    );
}

#[test]
fn confirmation_not_sent_create_cannot_claim_an_object() {
    let (_dir, mut store, id) = fixture(SourceKind::Volcano, false);
    let ticket = prepare(&mut store, id, 2).unwrap();
    store
        .settle_execution(
            &ticket,
            ExecutionOutcome::NotSent(ControlErrorCode::AuthFailed),
            100,
        )
        .unwrap();
    let w = store.load_work(id, false).unwrap();
    let channel = observation(&w, 2).channel;
    assert_eq!(
        store.pending_creation(id).unwrap_err().code,
        ControlErrorCode::NotFound
    );
    assert!(bind(&mut store, &ticket, &[channel], true).is_err());
    assert!(w.member.channel_id.is_none());
}

#[test]
fn confirmation_requires_actual_fields_freshness_identity_and_every_revision() {
    for variant in 0..12 {
        let (_dir, mut store, id) = fixture(SourceKind::Volcano, true);
        let w = store.load_work(id, false).unwrap();
        let mut expected = w.revision();
        let mut o = observation(&w, 1);
        match variant {
            0 => expected.member += 1,
            1 => expected.source += 1,
            2 => expected.group += 1,
            3 => expected.execution += 1,
            4 => o.channel.models = "wrong".into(),
            5 => o.channel.priority += 1,
            6 => o.channel.status = 2,
            7 => o.channel.auto_ban = 1,
            8 => o.channel.group = "other".into(),
            9 => o.channel.channel_id += 1,
            10 => o.channel.observed_at = SystemTime::UNIX_EPOCH,
            _ => o.revision.execution += 1,
        }
        let members = [id];
        let ctx = context(&w, &members, &[]);
        assert!(
            store
                .confirm_member(id, expected, false, Some(&o), &ctx)
                .is_err(),
            "variant {variant}"
        );
        assert_eq!(counts(&store), (0, 0, 0));
        assert_eq!(
            store.operation(&op(0)).unwrap().phase,
            OperationPhase::Queued
        );
    }
}

#[test]
fn confirmation_unknown_status_write_is_not_resolved_by_matching_get() {
    let (_dir, mut store, id) = fixture(SourceKind::Volcano, true);
    stop_all(&mut store, id, false);
    let disable = prepare(&mut store, id, 1).unwrap();
    store
        .settle_execution(&disable, ExecutionOutcome::Uncertain, 100)
        .unwrap();
    assert_eq!(
        confirm(&mut store, id, 2).unwrap_err().code,
        ControlErrorCode::TransportUnknown
    );
    assert_eq!(store.operation(&op(1)).unwrap().confirmed_targets, 0);
    store
        .settle_execution(&disable, ExecutionOutcome::Responded(None), 100)
        .unwrap();
    confirm(&mut store, id, 2).unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Succeeded
    );
}

#[test]
fn confirmation_partial_retire_archives_only_after_last_member() {
    let (_dir, mut store, ids) = members(2, SourceKind::Volcano);
    stop_all(&mut store, ids[0], true);
    confirm(&mut store, ids[0], 2).unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Partial
    );
    assert!(store
        .load_work(ids[0], false)
        .unwrap()
        .source
        .retired_at
        .is_none());
    confirm(&mut store, ids[0], 2).unwrap();
    assert_eq!(store.operation(&op(1)).unwrap().confirmed_targets, 1);
    let w = store.load_work(ids[1], false).unwrap();
    store
        .defer_member(
            ids[1],
            w.revision(),
            ControlErrorCode::AuthFailed,
            100,
            None,
        )
        .unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Failed
    );
    assert!(store.due_members(200).unwrap().is_empty());
    assert_eq!(
        store.retry_operation(&op(1), 100).unwrap().phase,
        OperationPhase::Partial
    );
    assert_eq!(store.due_members(100).unwrap(), vec![ids[1]]);
    confirm(&mut store, ids[1], 2).unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Succeeded
    );
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.sources[0].retired_at, Some(100));
    assert!(snapshot
        .memberships
        .iter()
        .all(|m| m.lifecycle == MembershipLifecycle::Retired && m.channel_id.is_some()));
    let history = receipts(&store);
    let old = store.operation(&op(1)).unwrap();
    assert_eq!(store.retry_operation(&op(1), 150).unwrap(), old);
    assert_eq!(receipts(&store), history);
}

#[test]
fn confirmation_never_sent_retirement_needs_no_remote_object() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, false);
    stop_all(&mut store, id, true);
    let w = store.load_work(id, false).unwrap();
    let members = [id];
    let ctx = context(&w, &members, &[]);
    store
        .confirm_member(id, w.revision(), false, None, &ctx)
        .unwrap();
    let w = store.load_work(id, false).unwrap();
    assert!(w.member.channel_id.is_none());
    assert!(w.source.retired_at.is_some());
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Succeeded
    );
}

#[test]
fn confirmation_rewrite_invalidates_only_unfinished_receipt_and_ack_cannot_finish_it() {
    let (_dir, mut store, ids) = members(2, SourceKind::Volcano);
    let source = store.load_work(ids[0], false).unwrap().source;
    store
        .submit_operation(
            op(1),
            ControlCommand::SetSourcePaused {
                source_id: source.source_id,
                expected_revision: source.config_revision,
                paused: false,
            },
            100,
        )
        .unwrap();
    confirm(&mut store, ids[0], 1).unwrap();
    assert_eq!(store.operation(&op(1)).unwrap().confirmed_targets, 1);
    let ticket = prepare(&mut store, ids[0], 2).unwrap();
    assert_eq!(store.operation(&op(1)).unwrap().confirmed_targets, 0);
    confirm(&mut store, ids[1], 1).unwrap();
    store
        .settle_execution(&ticket, ExecutionOutcome::Responded(None), 100)
        .unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Partial
    );
    confirm(&mut store, ids[0], 1).unwrap();
    let completed = store.operation(&op(1)).unwrap();
    let history = receipts(&store);
    let _again = prepare(&mut store, ids[0], 2).unwrap();
    assert_eq!(store.operation(&op(1)).unwrap(), completed);
    assert_eq!(receipts(&store), history);
}

#[test]
fn confirmation_glm_already_enabled_readback_consumes_authorization() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    confirm(&mut store, id, 1).unwrap();
    assert!(store.load_work(id, false).unwrap().enable_consumed);
    update(&mut store, id, 1, false);
    assert_eq!(
        store.member_target(id).unwrap().status_directive,
        StatusDirective::Preserve
    );
    confirm(&mut store, id, 3).unwrap();
    assert_eq!(
        store.operation(&op(1)).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert_eq!(counts(&store).0, 0);
}

#[test]
fn confirmation_failure_at_each_write_rolls_back_receipts_lifecycle_and_archive() {
    for table in [
        "memberships",
        "member_control",
        "operation_targets",
        "sources",
        "control_operations",
    ] {
        let (_dir, mut store, id) = fixture(SourceKind::Volcano, true);
        stop_all(&mut store, id, true);
        let before = store.snapshot().unwrap();
        let targets = receipts(&store);
        store.conn.execute_batch(&format!("CREATE TRIGGER fail_confirmation BEFORE UPDATE ON {table} BEGIN SELECT RAISE(ABORT,'fixture'); END;")).unwrap();
        assert!(confirm(&mut store, id, 2).is_err());
        assert_eq!(store.snapshot().unwrap(), before);
        assert_eq!(receipts(&store), targets);
    }
}

#[test]
fn confirmation_missing_receipt_target_cannot_become_success() {
    let (_dir, mut store, ids) = members(2, SourceKind::Glm);
    stop_all(&mut store, ids[0], false);
    store
        .conn
        .execute(
            "DELETE FROM operation_targets WHERE operation_id=?1 AND membership_id=?2",
            params![op(1).as_str(), ids[1].get()],
        )
        .unwrap();
    assert!(confirm(&mut store, ids[0], 2).is_err());
    assert!(store
        .load_work(ids[0], false)
        .unwrap()
        .member
        .applied_revision
        .is_none());
}

#[test]
fn confirmation_glm_consumption_failure_rolls_back_readback_and_receipt() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let before = store.snapshot().unwrap();
    let history = receipts(&store);
    store.conn.execute_batch("CREATE TRIGGER fail_consume BEFORE UPDATE OF enable_consumed ON member_execution BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(confirm(&mut store, id, 1).is_err());
    assert_eq!(store.snapshot().unwrap(), before);
    assert_eq!(receipts(&store), history);
    assert!(!store.load_work(id, false).unwrap().enable_consumed);
    assert_eq!(
        store.operation(&op(0)).unwrap().phase,
        OperationPhase::Queued
    );
}

#[test]
fn confirmation_retry_is_same_operation_and_does_not_clear_unknown_or_authorization() {
    let (_dir, mut store, id) = fixture(SourceKind::Glm, true);
    let ticket = prepare(&mut store, id, 2).unwrap();
    store
        .settle_execution(&ticket, ExecutionOutcome::Uncertain, 100)
        .unwrap();
    let before = store.load_work(id, false).unwrap();
    let op_count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM control_operations", [], |r| r.get(0))
        .unwrap();
    store
        .defer_member(
            id,
            before.revision(),
            ControlErrorCode::RateLimited,
            100,
            Some(1000),
        )
        .unwrap();
    assert!(store.due_members(999).unwrap().is_empty());
    assert_eq!(store.due_members(1000).unwrap(), vec![id]);
    assert_eq!(
        store.retry_operation(&op(0), 100).unwrap().phase,
        OperationPhase::Unknown
    );
    assert_eq!(
        store.load_work(id, false).unwrap().pending_write,
        PendingWrite::Unknown
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM control_operations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        op_count
    );
    update(&mut store, id, 1, true);
    assert_eq!(
        store.retry_operation(&op(0), 101).unwrap_err().code,
        ControlErrorCode::RevisionConflict
    );
    assert_eq!(
        store
            .defer_member(
                id,
                before.revision(),
                ControlErrorCode::AuthFailed,
                101,
                None
            )
            .unwrap_err()
            .code,
        ControlErrorCode::RevisionConflict
    );
}

#[test]
fn confirmation_due_queue_is_bounded_stop_first_and_backoff_is_at_least_30s() {
    let (_dir, mut store, ids) = members(70, SourceKind::Glm);
    let source = store.load_work(ids[0], false).unwrap().source;
    store
        .submit_operation(
            op(1),
            ControlCommand::SetSourcePaused {
                source_id: source.source_id,
                expected_revision: source.config_revision,
                paused: false,
            },
            100,
        )
        .unwrap();
    let last = *ids.last().unwrap();
    let work = store.load_work(last, false).unwrap();
    store
        .submit_operation(
            op(2),
            ControlCommand::EnsureMember {
                membership_id: last,
                expected_revision: work.target.revision,
                status: StatusDirective::Disabled,
            },
            100,
        )
        .unwrap();
    let due = store.due_members(100).unwrap();
    assert_eq!(due.len(), 64);
    assert_eq!(due[0], last);
    let w = store.load_work(last, false).unwrap();
    store
        .defer_member(
            last,
            w.revision(),
            ControlErrorCode::NotReady,
            100,
            Some(101),
        )
        .unwrap();
    assert_eq!(store.member_target(last).unwrap().next_attempt_at, 130);
    assert!(!store.due_members(129).unwrap().contains(&last));
    assert_eq!(store.due_members(130).unwrap()[0], last);
}

#[test]
fn confirmation_operation_schema_output() {
    let (_dir, mut store, ids) = members(2, SourceKind::Volcano);
    stop_all(&mut store, ids[0], true);
    let mut views = vec![store.operation(&op(1)).unwrap()];
    confirm(&mut store, ids[0], 2).unwrap();
    views.push(store.operation(&op(1)).unwrap());
    let w = store.load_work(ids[1], false).unwrap();
    store
        .defer_member(
            ids[1],
            w.revision(),
            ControlErrorCode::AuthFailed,
            100,
            None,
        )
        .unwrap();
    views.push(store.operation(&op(1)).unwrap());
    store.retry_operation(&op(1), 100).unwrap();
    confirm(&mut store, ids[1], 2).unwrap();
    views.push(store.operation(&op(1)).unwrap());
    assert_eq!(
        views.iter().map(|v| v.phase).collect::<Vec<_>>(),
        vec![
            OperationPhase::Queued,
            OperationPhase::Partial,
            OperationPhase::Failed,
            OperationPhase::Succeeded
        ]
    );
    println!(
        "CONFIRMATION_SCHEMA_JSON={}",
        serde_json::to_string(&views).unwrap()
    );
    for (_, _, _, receipt) in receipts(&store) {
        if let Some(receipt) = receipt {
            assert!(!receipt.contains("fake-execution-private-key"));
            assert!(!receipt.contains("base_url"));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn confirmation_pbt_generation_write_confirmation_retry_interleavings(events in prop::collection::vec(0usize..4,1..10)) {
        let (_dir,mut store,id)=fixture(SourceKind::Volcano,true);let mut latest=op(0);
        for (n,event) in events.into_iter().enumerate() {
            let old=store.load_work(id,false).unwrap();let old_observation=observation(&old,1);
            match event {
                0=>{latest=op(n+1);store.submit_operation(latest.clone(),ControlCommand::EnsureMember {membership_id:id,expected_revision:old.target.revision,status:StatusDirective::Enabled},100).unwrap();}
                1=>{latest=op(n+1);store.submit_operation(latest.clone(),ControlCommand::SyncSourceModels {source_id:old.source.source_id,expected_revision:old.source.config_revision,models:vec![format!("interleave-{n}")]},100).unwrap();}
                2=>{latest=op(n+1);store.submit_operation(latest.clone(),ControlCommand::SetGroupPriorities {group_id:old.group.group_id,expected_revision:old.group.revision,priorities:vec![(id,10+n as i64)]},100).unwrap();}
                _=>{let mut mismatch=observation(&old,1);mismatch.channel.models="remote-old-model".into();let members=[id];let ctx=context(&old,&members,&[]);let ticket=store.prepare_execution(id,old.revision(),false,Some(&mismatch),&ctx).unwrap();store.settle_execution(&ticket,ExecutionOutcome::Responded(None),100).unwrap();}
            }
            let receipt=store.operation(&latest).unwrap();let rows=receipts(&store);
            let members=[id];let ctx=context(&old,&members,&[]);
            prop_assert_eq!(store.confirm_member(id,old.revision(),false,Some(&old_observation),&ctx).unwrap_err().code,ControlErrorCode::RevisionConflict);
            prop_assert_eq!(store.operation(&latest).unwrap(),receipt);prop_assert_eq!(receipts(&store),rows);
            store.retry_operation(&latest,100).unwrap();confirm(&mut store,id,1).unwrap();prop_assert_eq!(store.operation(&latest).unwrap().phase,OperationPhase::Succeeded);
        }
    }
    #[test]
    fn confirmation_pbt_partial_and_duplicate_readback_matches_independent_set(n in 1usize..7,retire in any::<bool>(),order in prop::collection::vec((0usize..40,any::<bool>()),1..24)) {
        let (_dir,mut store,ids)=members(n,SourceKind::Volcano);stop_all(&mut store,ids[0],retire);
        let mut confirmed=std::collections::HashSet::new();
        for (index,matching) in order.into_iter().chain((0..n).map(|i|(i,true))) {
            let i=index%n;
            let result=confirm(&mut store,ids[i],if matching {2}else {1});
            if matching {prop_assert!(result.is_ok());confirmed.insert(i);}else {prop_assert!(result.is_err());}
            let receipt=store.operation(&op(1)).unwrap();
            prop_assert_eq!(receipt.confirmed_targets,confirmed.len());
            prop_assert_eq!(receipt.phase==OperationPhase::Succeeded,confirmed.len()==n);
            let source=store.load_work(ids[0],false).unwrap().source;
            prop_assert_eq!(source.retired_at.is_some(),retire && confirmed.len()==n);
            for (i,id) in ids.iter().enumerate() {let w=store.load_work(*id,false).unwrap();prop_assert_eq!(w.member.channel_id.unwrap().get(),101+i as i64);prop_assert_eq!(w.member.source_id,source.source_id);}
        }
    }
    #[test]
    fn confirmation_pbt_creation_lookup_after_intent_change_and_restart(matches in 0usize..3,retire in any::<bool>(),paused in any::<bool>(),uncertain in any::<bool>()) {
        let (dir,mut store,id)=fixture(SourceKind::Glm,false);let ticket=prepare(&mut store,id,2).unwrap();
        if uncertain {store.settle_execution(&ticket,ExecutionOutcome::Uncertain,100).unwrap();}
        if retire || paused {stop_all(&mut store,id,retire);}
        drop(store);let mut store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        let resumed=store.pending_creation(id).unwrap();prop_assert_eq!(&resumed,&ticket);
        let work=store.load_work(id,false).unwrap();let channels:Vec<_>=(0..matches).map(|i| {let mut c=observation(&work,2).channel;c.channel_id=101+i as i64;c}).collect();
        let result=bind(&mut store,&resumed,&channels,true);
        let w=store.load_work(id,false).unwrap();
        if matches==1 {
            prop_assert!(result.is_ok());prop_assert_eq!(w.pending_write,PendingWrite::None);
            if retire || paused {confirm(&mut store,id,2).unwrap();prop_assert_eq!(store.operation(&op(1)).unwrap().phase,OperationPhase::Succeeded);}
            else {prop_assert!(matches!(prepare(&mut store,id,2).unwrap().effect(),ExecutionEffect::Enable));}
        }else {prop_assert!(result.is_err());prop_assert!(w.member.channel_id.is_none());prop_assert_eq!(w.target.create_state,CreateState::MayHaveBeenSent);prop_assert!(prepare(&mut store,id,2).is_err());}
        prop_assert_eq!(store.conn.query_row("SELECT count(*) FROM execution_attempts WHERE action='create'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
    #[test]
    fn confirmation_pbt_partial_history_survives_reopen_and_explicit_retry(n in 2usize..6,first in 0usize..6,retire in any::<bool>()) {
        let (dir,mut store,ids)=members(n,SourceKind::Glm);stop_all(&mut store,ids[0],retire);
        let first=first%n;confirm(&mut store,ids[first],2).unwrap();let history=receipts(&store);
        drop(store);let mut store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        prop_assert_eq!(receipts(&store),history);prop_assert_eq!(store.retry_operation(&op(1),100).unwrap().confirmed_targets,1);
        for id in ids {confirm(&mut store,id,2).unwrap();}
        let completed=store.operation(&op(1)).unwrap();let history=receipts(&store);drop(store);
        let mut store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();prop_assert_eq!(store.retry_operation(&op(1),150).unwrap(),completed);prop_assert_eq!(receipts(&store),history);
    }
    #[test]
    fn confirmation_pbt_stale_receipt_never_completes_new_target(updates in 1usize..10,status in 1i64..4) {
        let (_dir,mut store,id)=fixture(SourceKind::Glm,true);let old=store.load_work(id,false).unwrap();let observed=observation(&old,status);
        for n in 1..=updates {update(&mut store,id,n,false);}
        let members=[id];let ctx=context(&old,&members,&[]);
        prop_assert_eq!(store.confirm_member(id,old.revision(),false,Some(&observed),&ctx).unwrap_err().code,ControlErrorCode::RevisionConflict);
        prop_assert_eq!(counts(&store).1,0);prop_assert_eq!(store.operation(&op(updates)).unwrap().phase,OperationPhase::Queued);
    }
}
