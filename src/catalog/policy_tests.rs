use super::*;
use crate::catalog::control::{ControlErrorCode as Code, ControlResult};
use crate::catalog::policy::*;
use crate::orchestrator::Regime;
use crate::source_policy::glm::GlmGroupState;
use crate::source_policy::volcano::*;

const NOW: i64 = 20 * DAY_SECONDS + 18 * 3600;

fn err<T>(result: ControlResult<T>, code: Code) {
    match result {
        Err(e) => assert_eq!(e.code, code),
        Ok(_) => panic!("expected {code:?}"),
    }
}
fn fixture() -> (
    TempDir,
    CatalogStore,
    SourceId,
    Vec<GroupId>,
    Vec<MembershipId>,
) {
    let (dir, mut store) = setup();
    let mut draft = source("fake-policy-private", SourceKind::Volcano);
    if let SourceSettings::Volcano { policy, .. } = &mut draft.settings {
        policy.daily_token_limit = Some(100);
    }
    let source = store.create_source(draft).unwrap();
    let mut groups = Vec::new();
    let mut members = Vec::new();
    for i in 0..2 {
        let g = store
            .create_group(group(&format!("policy-{i}"), SourceKind::Volcano))
            .unwrap();
        members.push(
            store
                .plan_membership(source.source_id, g.group_id, 10)
                .unwrap()
                .membership_id,
        );
        groups.push(g.group_id);
    }
    (dir, store, source.source_id, groups, members)
}
fn batch(
    r: &VolcanoRuntime,
    cursor: i64,
    input: i64,
    output: i64,
    failures: usize,
    now: i64,
) -> VolcanoEvidence {
    VolcanoEvidence {
        source_id: r.source_id,
        period_id: r.period_id,
        error_epoch: r.error_epoch,
        from_cursor: r.evidence_cursor,
        to_cursor: cursor,
        used_input: input,
        used_output: output,
        coverage_end: now + 1,
        outcomes: vec![
            FinalOutcome {
                first_event_at: now,
                completed_at: now,
                kind: OutcomeKind::Unavailable
            };
            failures
        ],
    }
}
fn selected(id: MembershipId, order: i64) -> GroupPolicyState {
    GroupPolicyState::Volcano(VolcanoGroupState {
        preferred: Some(id),
        cursor: Some((order, id)),
    })
}
fn materials(conn: &Connection) -> Vec<(i64, String, String, String)> {
    conn.prepare("SELECT source_id,settings_json,raw_key,credential_fingerprint FROM sources JOIN source_secrets USING(secret_id) ORDER BY source_id")
        .unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().map(|r|r.unwrap()).collect()
}
fn as_v3(store: &mut CatalogStore) {
    let tx = store.conn.transaction().unwrap();
    let instance: String = tx
        .query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    tx.execute_batch(
        "DROP TABLE group_policy_state; DROP TABLE volcano_runtime; DROP TABLE catalog_meta;",
    )
    .unwrap();
    tx.execute_batch(SCHEMA_V3.split(';').next().unwrap())
        .unwrap();
    tx.execute("INSERT INTO catalog_meta VALUES(1,3,?1)", [instance])
        .unwrap();
    tx.pragma_update(None, "user_version", 3).unwrap();
    tx.commit().unwrap();
}
fn backups_v3(dir: &TempDir) -> Vec<PathBuf> {
    fs::read_dir(dir.path())
        .unwrap()
        .map(|r| r.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("catalog-v3-before-v4-")
        })
        .collect()
}

#[test]
fn policy_defaults_are_private_uninitialized_and_shared() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(SourcePolicyUpdate, serde::Deserialize<'static>);
    assert_not_impl!(SourcePolicyWrite, serde::Serialize);
    let (_dir, mut store, s, groups, _) = fixture();
    assert_eq!(store.load_source_policy(s).unwrap(), None);
    for g in groups {
        assert_eq!(
            store.load_group_policy(g).unwrap(),
            StoredGroupPolicy {
                group_id: g,
                policy_revision: 1,
                state: GroupPolicyState::Volcano(VolcanoGroupState::default())
            }
        );
    }
    let glm = store
        .create_source(source("fake-policy-glm", SourceKind::Glm))
        .unwrap();
    let gg = store
        .create_group(group("glm-policy", SourceKind::Glm))
        .unwrap();
    assert_eq!(
        store.load_group_policy(gg.group_id).unwrap().state,
        GroupPolicyState::Glm(GlmGroupState::default())
    );
    err(store.load_source_policy(glm.source_id), Code::InvalidInput);
    err(
        store.load_source_policy(SourceId::new(999).unwrap()),
        Code::NotFound,
    );
    err(
        store.load_group_policy(GroupId::new(999).unwrap()),
        Code::NotFound,
    );
    err(
        store.apply_source_policy(s, 1, 1, SourcePolicyUpdate::Tick, NOW),
        Code::NotReady,
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM volcano_runtime", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(store.snapshot().unwrap().schema_version, 1);
    assert_eq!(
        store
            .conn
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn policy_two_revisions_noops_and_reopen_preserve_both_reasons_and_pause() {
    let (dir, mut store, s, _, _) = fixture();
    let mut r = store.initialize_source_policy(s, 1, NOW).unwrap();
    assert_eq!(
        store
            .initialize_source_policy(s, 1, r.next_reset_at + DAY_SECONDS)
            .unwrap(),
        r
    );
    let write = store
        .apply_source_policy(
            s,
            1,
            1,
            SourcePolicyUpdate::Evidence(batch(&r, 1, 80, 30, 3, NOW + 1)),
            NOW + 1,
        )
        .unwrap();
    r = write.runtime;
    assert_eq!(r.revision, 2);
    assert!(r.error_exhausted && r.limit_exhausted);
    err(
        store.apply_source_policy(s, 2, 2, SourcePolicyUpdate::Tick, NOW + 2),
        Code::RevisionConflict,
    );
    err(
        store.apply_source_policy(s, 1, 1, SourcePolicyUpdate::Tick, NOW + 2),
        Code::RevisionConflict,
    );
    assert_eq!(
        store
            .apply_source_policy(s, 1, 2, SourcePolicyUpdate::Tick, NOW + 2)
            .unwrap()
            .runtime,
        r
    );
    store
        .conn
        .execute(
            "UPDATE sources SET manual_paused=1,config_revision=2 WHERE source_id=?1",
            [s.get()],
        )
        .unwrap();
    let clear = store
        .apply_source_policy(s, 2, 2, SourcePolicyUpdate::ClearError, NOW + 3)
        .unwrap();
    assert!(clear.source.manual_paused);
    assert!(!clear.runtime.error_exhausted && clear.runtime.limit_exhausted);
    assert_eq!(
        (clear.runtime.used_input, clear.runtime.used_output),
        (80, 30)
    );
    r = clear.runtime;
    drop(store);
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(store.load_source_policy(s).unwrap(), Some(r.clone()));
    let new = VolcanoPolicy {
        daily_token_limit: Some(200),
        reset_minute: 90,
        ..VolcanoPolicy::default()
    };
    let edited = store
        .apply_source_policy(
            s,
            2,
            r.revision,
            SourcePolicyUpdate::ReplacePolicy(new.clone()),
            NOW + 4,
        )
        .unwrap();
    assert_eq!(edited.source.config_revision, 3);
    assert_eq!(edited.runtime.revision, r.revision + 1);
    assert_eq!(
        (edited.runtime.used_input, edited.runtime.used_output),
        (80, 30)
    );
    assert!(!edited.runtime.limit_exhausted);
    let noop = store
        .apply_source_policy(
            s,
            3,
            edited.runtime.revision,
            SourcePolicyUpdate::ReplacePolicy(new),
            NOW + 4,
        )
        .unwrap();
    assert_eq!(noop.source, edited.source);
    assert_eq!(noop.runtime, edited.runtime);
    let reset = store
        .apply_source_policy(
            s,
            3,
            noop.runtime.revision,
            SourcePolicyUpdate::Tick,
            noop.runtime.next_reset_at + 2 * DAY_SECONDS,
        )
        .unwrap();
    assert_eq!(reset.runtime.period_id, 2);
    assert!(reset.source.manual_paused);
    assert_eq!(reset.runtime.state(true), VolcanoState::Paused);
    assert_eq!(reset.runtime.used_input, 0);
    assert_eq!(
        reset.transition.unwrap().old_next,
        noop.runtime.next_reset_at
    );
}

#[test]
fn policy_stale_due_evidence_commits_reset_but_invalid_and_gap_roll_back() {
    let (_dir, mut store, s, _, _) = fixture();
    let r = store.initialize_source_policy(s, 1, NOW).unwrap();
    let due = r.next_reset_at;
    let mut gap = batch(&r, 2, 1, 0, 0, due);
    gap.period_id += 1;
    gap.error_epoch += 1;
    gap.from_cursor = Some(1);
    err(
        store.apply_source_policy(s, 1, 1, SourcePolicyUpdate::Evidence(gap.clone()), due),
        Code::NotReady,
    );
    assert_eq!(store.load_source_policy(s).unwrap(), Some(r.clone()));
    gap.used_input = -1;
    err(
        store.apply_source_policy(s, 1, 1, SourcePolicyUpdate::Evidence(gap), due),
        Code::InvalidInput,
    );
    assert_eq!(store.load_source_policy(s).unwrap(), Some(r.clone()));
    let old = batch(&r, 1, 20, 3, 3, NOW + 1);
    let reset = store
        .apply_source_policy(s, 1, 1, SourcePolicyUpdate::Evidence(old), due)
        .unwrap();
    assert_eq!(reset.disposition, Some(EvidenceDisposition::Stale));
    assert!(reset.transition.is_some());
    assert_eq!(
        (
            reset.runtime.revision,
            reset.runtime.period_id,
            reset.runtime.used_input
        ),
        (2, 2, 0)
    );
    let b = batch(&reset.runtime, 2, 7, 5, 1, due + 1);
    let accepted = store
        .apply_source_policy(s, 1, 2, SourcePolicyUpdate::Evidence(b.clone()), due + 1)
        .unwrap();
    let duplicate = store
        .apply_source_policy(s, 1, 3, SourcePolicyUpdate::Evidence(b), due + 2)
        .unwrap();
    assert_eq!(duplicate.runtime, accepted.runtime);
    assert_eq!(duplicate.disposition, Some(EvidenceDisposition::Duplicate));
}

#[test]
fn policy_group_types_revisions_and_history_are_not_routing_receipts() {
    let (dir, mut store, s, groups, members) = fixture();
    let before = store.snapshot().unwrap();
    err(
        store.save_group_policy(groups[0], 1, 1, selected(members[1], 0)),
        Code::InvalidInput,
    );
    err(
        store.save_group_policy(
            groups[0],
            1,
            1,
            GroupPolicyState::Glm(GlmGroupState::default()),
        ),
        Code::InvalidInput,
    );
    let state = selected(members[0], 0);
    let saved = store
        .save_group_policy(groups[0], 1, 1, state.clone())
        .unwrap();
    assert_eq!(saved.policy_revision, 2);
    assert_eq!(
        store
            .save_group_policy(groups[0], 1, 2, state.clone())
            .unwrap(),
        saved
    );
    err(
        store.save_group_policy(groups[0], 2, 2, state.clone()),
        Code::RevisionConflict,
    );
    err(
        store.save_group_policy(groups[0], 1, 1, state),
        Code::RevisionConflict,
    );
    assert_eq!(store.snapshot().unwrap(), before);
    assert_eq!(store.load_source_policy(s).unwrap(), None);
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM control_operations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .conn
        .execute(
            "UPDATE memberships SET lifecycle='retired',retired_at=?1 WHERE membership_id=?2",
            params![NOW, members[0].get()],
        )
        .unwrap();
    let cursor = GroupPolicyState::Volcano(VolcanoGroupState {
        preferred: None,
        cursor: Some((0, members[0])),
    });
    let history = store.save_group_policy(groups[0], 1, 2, cursor).unwrap();
    let gs = store
        .create_source(source("fake-group-glm", SourceKind::Glm))
        .unwrap();
    let gg = store
        .create_group(group("group-glm", SourceKind::Glm))
        .unwrap();
    let gm = store
        .plan_membership(gs.source_id, gg.group_id, 10)
        .unwrap();
    let glm = store
        .save_group_policy(
            gg.group_id,
            1,
            1,
            GroupPolicyState::Glm(GlmGroupState {
                active: Some(gm.membership_id),
                pinned: Some(gm.membership_id),
                regime: Regime::Degraded,
            }),
        )
        .unwrap();
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(store.load_group_policy(groups[0]).unwrap(), history);
    assert_eq!(store.load_group_policy(gg.group_id).unwrap(), glm);
}

#[test]
fn policy_sql_failures_and_overflows_roll_back_complete_write() {
    let (_dir, mut store, s, groups, members) = fixture();
    store.conn.execute_batch("CREATE TRIGGER fail_init BEFORE UPDATE ON volcano_runtime BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    err(store.initialize_source_policy(s, 1, NOW), Code::Storage);
    assert_eq!(store.load_source_policy(s).unwrap(), None);
    store.conn.execute_batch("DROP TRIGGER fail_init").unwrap();
    let r = store.initialize_source_policy(s, 1, NOW).unwrap();
    let original = materials(&store.conn);
    for limit in [120, 130] {
        if limit == 120 {
            store.conn.execute_batch("CREATE TRIGGER fail_runtime BEFORE UPDATE ON volcano_runtime BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        } else {
            store
                .conn
                .execute("UPDATE volcano_runtime SET revision=?1", [JSON_SAFE])
                .unwrap();
        }
        let expected = store.load_source_policy(s).unwrap().unwrap();
        let p = VolcanoPolicy {
            daily_token_limit: Some(limit),
            reset_minute: 70,
            ..VolcanoPolicy::default()
        };
        err(
            store.apply_source_policy(
                s,
                1,
                expected.revision,
                SourcePolicyUpdate::ReplacePolicy(p),
                NOW,
            ),
            Code::Storage,
        );
        assert_eq!(materials(&store.conn), original);
        assert_eq!(store.load_source_policy(s).unwrap(), Some(expected));
        if limit == 120 {
            store
                .conn
                .execute_batch("DROP TRIGGER fail_runtime")
                .unwrap();
        }
    }
    store.conn.execute_batch("CREATE TRIGGER fail_group BEFORE UPDATE ON group_policy_state BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    err(
        store.save_group_policy(groups[0], 1, 1, selected(members[0], 0)),
        Code::Storage,
    );
    assert_eq!(
        store.load_group_policy(groups[0]).unwrap().policy_revision,
        1
    );
    store.conn.execute_batch("DROP TRIGGER fail_group").unwrap();
    store
        .conn
        .execute(
            "UPDATE group_policy_state SET revision=?1 WHERE group_id=?2",
            params![JSON_SAFE, groups[0].get()],
        )
        .unwrap();
    err(
        store.save_group_policy(groups[0], 1, JSON_SAFE, selected(members[0], 0)),
        Code::Storage,
    );
    assert_eq!(
        store.load_group_policy(groups[0]).unwrap().state,
        GroupPolicyState::Volcano(VolcanoGroupState::default())
    );
    assert_eq!(r.revision, 1);
}

#[test]
fn policy_default_row_failure_rolls_back_source_and_group_creation() {
    let (_dir, mut store) = setup();
    store.conn.execute_batch("CREATE TRIGGER fail_source BEFORE INSERT ON volcano_runtime BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(store
        .create_source(source("fake-default-row", SourceKind::Volcano))
        .is_err());
    assert!(store.snapshot().unwrap().sources.is_empty());
    assert_eq!(materials(&store.conn).len(), 0);
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM source_secrets", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store.conn.execute_batch("CREATE TRIGGER fail_group BEFORE INSERT ON group_policy_state BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(store
        .create_group(group("no-partial-group", SourceKind::Glm))
        .is_err());
    assert!(store.snapshot().unwrap().groups.is_empty());
}

#[test]
fn policy_missing_rows_and_corrupt_state_are_not_repaired() {
    for corrupt in [
        "DELETE FROM volcano_runtime", "DELETE FROM group_policy_state",
        "UPDATE volcano_runtime SET error_exhausted=2", "UPDATE volcano_runtime SET used_input=100,limit_exhausted=0",
        "UPDATE volcano_runtime SET consecutive_failures=3,error_exhausted=0",
        "UPDATE volcano_runtime SET next_reset_at=next_reset_at+1",
        "UPDATE volcano_runtime SET period_id=NULL", "UPDATE volcano_runtime SET evidence_cursor=1,coverage_end=NULL",
        "UPDATE group_policy_state SET active_member_id=2,cursor_member_id=2,cursor_order=0 WHERE group_id=1",
        "UPDATE group_policy_state SET regime='normal'", "UPDATE group_policy_state SET kind='glm',regime='normal'",
        "UPDATE group_policy_state SET cursor_order=0,cursor_member_id=NULL",
    ] {
        let (dir,mut store,s,_,_)=fixture();store.initialize_source_policy(s,1,NOW).unwrap();
        store.conn.pragma_update(None,"ignore_check_constraints",true).unwrap();
        store.conn.pragma_update(None,"foreign_keys",false).unwrap();
        store.conn.execute_batch(corrupt).unwrap();
        assert!(crate::catalog::policy::verify_policy(&store.conn).is_err(),"{corrupt}");
        if corrupt=="DELETE FROM volcano_runtime" {
            err(store.load_source_policy(s),Code::Storage);err(store.initialize_source_policy(s,1,NOW),Code::Storage);
        }
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err(),"{corrupt}");
        assert!(backups_v3(&dir).is_empty());
    }
    let (dir, store, _, _, _) = fixture();
    store
        .conn
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .conn
        .execute("UPDATE volcano_runtime SET used_input=1", [])
        .unwrap();
    drop(store);
    assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
}

#[test]
fn policy_ddl_rejects_null_loopholes_and_wrong_kinds() {
    let (_dir, store, _, _, _) = fixture();
    for sql in ["UPDATE volcano_runtime SET revision=1,error_epoch=1",
        "UPDATE volcano_runtime SET revision=1,period_id=1,period_start=1,next_reset_at=2,error_epoch=0",
        "UPDATE volcano_runtime SET used_input=1", "UPDATE group_policy_state SET active_member_id=1",
        "UPDATE group_policy_state SET kind='glm',regime=NULL",
        "UPDATE group_policy_state SET cursor_order=0",
        "UPDATE group_policy_state SET revision=0"] {
        assert!(store.conn.execute_batch(sql).is_err(),"{sql}");
    }
}

#[test]
fn policy_config_only_change_keeps_runtime_revision_and_source_overflow_rolls_back() {
    let (_dir, mut store, s, _, _) = fixture();
    let runtime = store.initialize_source_policy(s, 1, NOW).unwrap();
    let new = VolcanoPolicy {
        daily_token_limit: Some(200),
        ..VolcanoPolicy::default()
    };
    let saved = store
        .apply_source_policy(s, 1, 1, SourcePolicyUpdate::ReplacePolicy(new), NOW)
        .unwrap();
    assert_eq!(saved.source.config_revision, 2);
    assert_eq!(saved.runtime, runtime);
    assert_eq!(store.load_source_policy(s).unwrap(), Some(runtime.clone()));
    store
        .conn
        .execute(
            "UPDATE sources SET config_revision=?1 WHERE source_id=?2",
            params![JSON_SAFE, s.get()],
        )
        .unwrap();
    let before = materials(&store.conn);
    let new = VolcanoPolicy {
        daily_token_limit: Some(300),
        ..VolcanoPolicy::default()
    };
    err(
        store.apply_source_policy(s, JSON_SAFE, 1, SourcePolicyUpdate::ReplacePolicy(new), NOW),
        Code::Storage,
    );
    assert_eq!(materials(&store.conn), before);
    assert_eq!(store.load_source_policy(s).unwrap(), Some(runtime));
}

#[test]
fn policy_retired_sources_reject_writes_and_poisoned_store_is_closed() {
    let (dir, mut store, s, g, m) = fixture();
    let r = store.initialize_source_policy(s, 1, NOW).unwrap();
    store
        .conn
        .execute("UPDATE source_control SET retire_requested=1", [])
        .unwrap();
    err(store.initialize_source_policy(s, 1, NOW), Code::NotReady);
    err(
        store.apply_source_policy(s, 1, 1, SourcePolicyUpdate::ClearError, NOW),
        Code::NotReady,
    );
    assert_eq!(store.load_source_policy(s).unwrap(), Some(r.clone()));
    store
        .conn
        .execute(
            "UPDATE memberships SET lifecycle='retired',retired_at=?1 WHERE source_id=?2",
            params![NOW, s.get()],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE sources SET retired_at=?1,manual_paused=1 WHERE source_id=?2",
            params![NOW, s.get()],
        )
        .unwrap();
    drop(store);
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    err(store.initialize_source_policy(s, 1, NOW), Code::NotReady);
    err(
        store.apply_source_policy(s, 1, 1, SourcePolicyUpdate::ClearError, NOW),
        Code::NotReady,
    );
    assert_eq!(store.load_source_policy(s).unwrap(), Some(r));
    store.poisoned = true;
    err(store.load_source_policy(s), Code::Storage);
    err(store.load_group_policy(g[0]), Code::Storage);
    err(store.initialize_source_policy(s, 1, NOW), Code::Storage);
    err(
        store.apply_source_policy(s, 1, 1, SourcePolicyUpdate::Tick, NOW),
        Code::Storage,
    );
    err(
        store.save_group_policy(g[0], 1, 1, selected(m[0], 0)),
        Code::Storage,
    );
}

#[test]
fn policy_tx_helpers_compose_and_unknown_commit_does_not_publish() {
    let (_dir, mut store, s, g, m) = fixture();
    let r = store.initialize_source_policy(s, 1, NOW).unwrap();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    apply_source_policy_tx(&tx, s, 1, 1, SourcePolicyUpdate::ClearError, NOW).unwrap();
    save_group_policy_tx(&tx, g[0], 1, 1, selected(m[0], 0)).unwrap();
    tx.rollback().unwrap();
    assert_eq!(store.load_source_policy(s).unwrap(), Some(r.clone()));
    assert_eq!(store.load_group_policy(g[0]).unwrap().policy_revision, 1);
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.pragma_update(None, "defer_foreign_keys", true).unwrap();
    apply_source_policy_tx(&tx, s, 1, 1, SourcePolicyUpdate::ClearError, NOW).unwrap();
    tx.execute("INSERT INTO volcano_runtime(source_id) VALUES(999)", [])
        .unwrap();
    assert_eq!(
        commit(tx, &mut store.poisoned).unwrap_err().kind,
        ErrorKind::CommitUnknown
    );
    err(store.load_source_policy(s), Code::Storage);
    assert_eq!(
        store
            .conn
            .query_row(
                "SELECT error_epoch FROM volcano_runtime WHERE source_id=?1",
                [s.get()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        r.error_epoch
    );
}

#[test]
fn policy_v3_migration_preserves_material_and_private_backup() {
    let (dir, mut store, s, g, _) = fixture();
    store
        .create_source(source("fake-migration-glm", SourceKind::Glm))
        .unwrap();
    store
        .create_group(group("migration-glm", SourceKind::Glm))
        .unwrap();
    let old = store.snapshot().unwrap();
    let private = materials(&store.conn);
    let instance: String = store
        .conn
        .query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    as_v3(&mut store);
    drop(store);
    let started = std::time::Instant::now();
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(store.snapshot().unwrap(), old);
    assert_eq!(materials(&store.conn), private);
    assert_eq!(
        store
            .conn
            .query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        instance
    );
    assert_eq!(store.load_source_policy(s).unwrap(), None);
    assert_eq!(store.load_group_policy(g[0]).unwrap().policy_revision, 1);
    let files = backups_v3(&dir);
    assert_eq!(files.len(), 1);
    assert_eq!(
        fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let backup = Connection::open(&files[0]).unwrap();
    verify_schema(&backup, 3).unwrap();
    assert_eq!(read_snapshot(&backup).unwrap(), old);
    assert_eq!(materials(&backup), private);
    let bytes = fs::read(&files[0]).unwrap();
    drop(backup);
    drop(store);
    let _store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(backups_v3(&dir), files);
    assert_eq!(fs::read(&files[0]).unwrap(), bytes);
}

#[test]
fn policy_migration_failure_keeps_v3_and_corruption_is_rejected_before_backup() {
    for collision in ["volcano_runtime", "group_policy_state"] {
        let (dir, mut store, _, _, _) = fixture();
        let before = materials(&store.conn);
        as_v3(&mut store);
        store
            .conn
            .execute_batch(&format!(
                "CREATE TABLE {collision}(marker TEXT); INSERT INTO {collision} VALUES('fixture');"
            ))
            .unwrap();
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
        let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
        verify_schema(&conn, 3).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(materials(&conn), before);
        assert_eq!(backups_v3(&dir).len(), 1);
        assert_eq!(
            conn.query_row(&format!("SELECT marker FROM {collision}"), [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "fixture"
        );
    }
    for sql in [
        "DELETE FROM member_execution",
        "DROP INDEX execution_one_prepared",
    ] {
        let (dir, mut store, _, _, _) = fixture();
        as_v3(&mut store);
        store.conn.execute_batch(sql).unwrap();
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
        assert!(backups_v3(&dir).is_empty());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn storage_pbt_shared_runtime_and_group_reopen(actions in prop::collection::vec((0u8..5,0u16..80,any::<bool>()),1..15)) {
        let (dir,mut store,s,g,m)=fixture();let mut actual=store.initialize_source_policy(s,1,NOW).unwrap();
        let mut expected=VolcanoRuntime{source_id:s,revision:1,period_id:1,period_start:20*DAY_SECONDS+17*3600,next_reset_at:21*DAY_SECONDS+17*3600,
            used_input:0,used_output:0,consecutive_failures:0,error_exhausted:false,limit_exhausted:false,error_epoch:1,error_cutoff_at:None,evidence_cursor:None,coverage_end:None};
        prop_assert_eq!(&actual,&expected);
        let mut group_revisions=[1,1];let mut chosen=[false,false];
        for (i,(failures,input,clear)) in actions.iter().enumerate() {
            let now=NOW+i as i64+1;
            let evidence=VolcanoEvidence{source_id:s,period_id:1,error_epoch:expected.error_epoch,from_cursor:expected.evidence_cursor,to_cursor:i as i64+1,
                used_input:expected.used_input+i64::from(*input),used_output:expected.used_output+2,coverage_end:now+1,
                outcomes:vec![FinalOutcome{first_event_at:now,completed_at:now,kind:OutcomeKind::Unavailable};*failures as usize]};
            actual=store.apply_source_policy(s,1,expected.revision,SourcePolicyUpdate::Evidence(evidence),now).unwrap().runtime;
            expected.revision+=1;expected.used_input+=i64::from(*input);expected.used_output+=2;
            expected.consecutive_failures+=i64::from(*failures);expected.error_exhausted|=expected.consecutive_failures>=3;
            expected.limit_exhausted=expected.used_input+expected.used_output>=100;expected.evidence_cursor=Some(i as i64+1);expected.coverage_end=Some(now+1);
            prop_assert_eq!(&actual,&expected);
            if *clear {
                actual=store.apply_source_policy(s,1,expected.revision,SourcePolicyUpdate::ClearError,now).unwrap().runtime;
                expected.revision+=1;expected.error_epoch+=1;expected.error_cutoff_at=Some(now);expected.evidence_cursor=None;expected.consecutive_failures=0;expected.error_exhausted=false;
                prop_assert_eq!(&actual,&expected);
            }
            let gi=i%2;let state=if chosen[gi]{GroupPolicyState::Volcano(VolcanoGroupState{preferred:None,cursor:Some((0,m[gi]))})}else{selected(m[gi],0)};
            store.save_group_policy(g[gi],1,group_revisions[gi],state).unwrap();group_revisions[gi]+=1;chosen[gi]= !chosen[gi];
            drop(store);store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
            prop_assert_eq!(store.load_source_policy(s).unwrap(),Some(expected.clone()));
            for j in 0..2 {
                let saved=store.load_group_policy(g[j]).unwrap();prop_assert_eq!(saved.policy_revision,group_revisions[j]);
                let expected_state=if chosen[j]{selected(m[j],0)}else if group_revisions[j]>1{GroupPolicyState::Volcano(VolcanoGroupState{preferred:None,cursor:Some((0,m[j]))})}else{GroupPolicyState::Volcano(VolcanoGroupState::default())};
                prop_assert_eq!(saved.state,expected_state);
            }
        }
    }
    #[test]
    fn storage_pbt_conflicts_and_bad_batches_preserve_all_state(mode in 0u8..6,input in 0i64..10000,now_delta in 1i64..1000) {
        let (_dir,mut store,s,g,m)=fixture();let r=store.initialize_source_policy(s,1,NOW).unwrap();
        let before=store.snapshot().unwrap();let private=materials(&store.conn);
        let result=match mode {
            0=>store.apply_source_policy(s,2,1,SourcePolicyUpdate::Tick,NOW).map(|_|()),
            1=>store.apply_source_policy(s,1,2,SourcePolicyUpdate::ClearError,NOW).map(|_|()),
            2=>{let mut b=batch(&r,1,input,0,0, NOW+now_delta);b.used_output= -1;store.apply_source_policy(s,1,1,SourcePolicyUpdate::Evidence(b),NOW+now_delta).map(|_|())},
            3=>{let mut b=batch(&r,2,input,0,0,NOW+now_delta);b.from_cursor=Some(1);store.apply_source_policy(s,1,1,SourcePolicyUpdate::Evidence(b),NOW+now_delta).map(|_|())},
            4=>store.save_group_policy(g[0],1,1,selected(m[1],0)).map(|_|()),
            _=>store.save_group_policy(g[0],1,2,selected(m[0],0)).map(|_|()),
        };
        prop_assert!(result.is_err());prop_assert_eq!(store.snapshot().unwrap(),before);prop_assert_eq!(materials(&store.conn),private);
        prop_assert_eq!(store.load_source_policy(s).unwrap(),Some(r));
        for id in g {prop_assert_eq!(store.load_group_policy(id).unwrap().policy_revision,1);}
    }
}
