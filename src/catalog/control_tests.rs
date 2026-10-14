use super::*;
use crate::catalog::control::*;

fn op(n: usize) -> OperationId {
    OperationId::new(format!("operation-{n:016}")).unwrap()
}
fn pause(source_id: SourceId, revision: i64, paused: bool) -> ControlCommand {
    ControlCommand::SetSourcePaused {
        source_id,
        expected_revision: revision,
        paused,
    }
}
fn counts(store: &CatalogStore) -> (i64, i64, i64) {
    store.conn.query_row("SELECT (SELECT count(*) FROM control_operations),(SELECT count(*) FROM operation_targets),(SELECT count(*) FROM member_control WHERE dirty=1)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap()
}
fn targets(store: &CatalogStore) -> Vec<MemberTarget> {
    let ids: Vec<i64> = store
        .conn
        .prepare("SELECT membership_id FROM memberships ORDER BY membership_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    ids.into_iter()
        .map(|id| store.member_target(MembershipId::new(id).unwrap()).unwrap())
        .collect()
}
fn turn_fixture_into_v1(store: &mut CatalogStore) {
    // 只用于测试自己的临时库，保留冻结的 v1 核心表及数据。
    let tx = store.conn.transaction().unwrap();
    tx.execute_batch("DROP TABLE operation_targets; DROP TABLE control_operations; DROP TABLE member_control; DROP TABLE source_control; DROP TABLE catalog_meta;").unwrap();
    tx.execute_batch(SCHEMA.split(';').next().unwrap()).unwrap();
    tx.execute("INSERT INTO catalog_meta VALUES(1,1)", [])
        .unwrap();
    tx.pragma_update(None, "user_version", 1).unwrap();
    tx.commit().unwrap();
}
fn backups(dir: &TempDir) -> Vec<PathBuf> {
    fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension()
                .is_some_and(|extension| extension == "sqlite3")
                && {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("catalog-v1-before-v2-")
                }
        })
        .collect()
}
fn two_groups() -> (
    TempDir,
    CatalogStore,
    SourceId,
    GroupId,
    GroupId,
    MembershipId,
    MembershipId,
) {
    let (dir, mut store) = setup();
    let s = store
        .create_source(source("fake-shared-key", SourceKind::Volcano))
        .unwrap();
    let a = store
        .create_group(group("group-a", SourceKind::Volcano))
        .unwrap();
    let b = store
        .create_group(group("group-b", SourceKind::Volcano))
        .unwrap();
    let ma = store.plan_membership(s.source_id, a.group_id, 10).unwrap();
    let mb = store.plan_membership(s.source_id, b.group_id, 20).unwrap();
    (
        dir,
        store,
        s.source_id,
        a.group_id,
        b.group_id,
        ma.membership_id,
        mb.membership_id,
    )
}

#[test]
fn v2_is_private_v1_wire_and_unmanaged_records_are_not_dirty() {
    let (dir, mut store, s, _, _, ma, mb) = two_groups();
    assert_eq!(
        store
            .conn
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(store.snapshot().unwrap().schema_version, 1);
    let instance: String = store
        .conn
        .query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(instance.len(), 32);
    for id in [ma, mb] {
        let target = store.member_target(id).unwrap();
        assert_eq!(target.create_name, format!("qt-{instance}-m{}", id.get()));
        assert_eq!(
            (
                target.revision,
                target.status_directive,
                target.create_state,
                target.dirty
            ),
            (1, StatusDirective::Preserve, CreateState::Never, false)
        );
    }
    let before = targets(&store);
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(targets(&store), before);
    assert_eq!(counts(&store), (0, 0, 0));
    assert_eq!(
        store.read_source_material(s).unwrap().credential.expose(),
        "fake-shared-key"
    );
    assert!(backups(&dir).is_empty());
}

#[test]
fn migration_backups_preserve_v1_schema_and_are_never_replaced() {
    let (dir, mut store) = setup();
    let (cfg, channels) = legacy(2);
    let plan = build_legacy_import(&cfg, &channels).unwrap();
    store.import_legacy(&plan).unwrap();
    let expected = store.snapshot().unwrap();
    turn_fixture_into_v1(&mut store);
    drop(store);
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(store.snapshot().unwrap(), expected);
    let files = backups(&dir);
    assert_eq!(files.len(), 1);
    assert_eq!(
        fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let backup = Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    verify_schema(&backup, 1).unwrap();
    verify_core_data(&backup).unwrap();
    drop(backup);
    let bytes = fs::read(&files[0]).unwrap();
    let ids = targets(&store);
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(targets(&store), ids);
    assert_eq!(backups(&dir), files);
    assert_eq!(fs::read(&files[0]).unwrap(), bytes);
}

#[test]
fn migration_ddl_failure_rolls_back_metadata_and_preserves_backup() {
    for collision in [
        "source_control",
        "member_control",
        "control_operations",
        "operation_targets",
    ] {
        let (dir, mut store) = setup();
        store
            .create_source(source("fake-rollback", SourceKind::Glm))
            .unwrap();
        let expected = store.snapshot().unwrap();
        turn_fixture_into_v1(&mut store);
        store.conn.execute_batch(&format!("CREATE TABLE {collision}(marker TEXT); INSERT INTO {collision} VALUES('owned-fixture');")).unwrap();
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
        let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        verify_schema(&conn, 1).unwrap();
        assert_eq!(read_snapshot(&conn).unwrap(), expected);
        assert_eq!(
            conn.query_row(&format!("SELECT marker FROM {collision}"), [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "owned-fixture"
        );
        let first = backups(&dir);
        assert_eq!(first.len(), 1);
        let bytes = fs::read(&first[0]).unwrap();
        conn.execute(&format!("DROP TABLE {collision}"), [])
            .unwrap();
        drop(conn);
        let mut reopened = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(reopened.snapshot().unwrap(), expected);
        assert_eq!(backups(&dir).len(), 2);
        assert_eq!(fs::read(&first[0]).unwrap(), bytes);
    }
}

#[test]
fn corrupted_v1_is_rejected_before_backup_or_migration() {
    let (dir, mut store) = setup();
    store
        .create_source(source("fake-corrupt", SourceKind::Glm))
        .unwrap();
    turn_fixture_into_v1(&mut store);
    store
        .conn
        .execute("UPDATE source_secrets SET raw_key='different-fake-key'", [])
        .unwrap();
    drop(store);
    let path = dir.path().join("catalog.sqlite3");
    let before = fs::read(&path).unwrap();
    expect(CatalogStore::open(&path), ErrorKind::CorruptData);
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(backups(&dir).is_empty());
}

#[test]
fn v2_side_table_corruption_is_not_silently_repaired() {
    for sql in [
        "DELETE FROM source_control",
        "DELETE FROM member_control",
        "UPDATE member_control SET create_name='wrong-name-'||membership_id",
        "UPDATE member_control SET create_state='resolved'",
        "DROP INDEX control_dirty_due",
    ] {
        let (dir, store, _, _, _, _, _) = two_groups();
        store.conn.execute(sql, []).unwrap();
        drop(store);
        expect(
            CatalogStore::open(&dir.path().join("catalog.sqlite3")),
            ErrorKind::CorruptData,
        );
    }
}

#[test]
fn pause_fans_out_atomically_and_replays_before_revision_validation() {
    let (dir, mut store, s, _, _, ma, mb) = two_groups();
    let command = pause(s, 1, true);
    let receipt = store.submit_operation(op(1), command.clone(), 100).unwrap();
    assert_eq!(
        (
            receipt.phase,
            receipt.target_revision,
            receipt.total_targets,
            receipt.confirmed_targets
        ),
        (OperationPhase::Queued, 2, 2, 0)
    );
    for id in [ma, mb] {
        let t = store.member_target(id).unwrap();
        assert_eq!(
            (t.revision, t.status_directive, t.dirty),
            (2, StatusDirective::Disabled, true)
        );
    }
    assert!(store.snapshot().unwrap().sources[0].manual_paused);
    assert_eq!(
        store.submit_operation(op(1), command.clone(), 999).unwrap(),
        receipt
    );
    assert_eq!(counts(&store), (1, 2, 2));
    assert_eq!(
        store
            .submit_operation(op(1), pause(s, 1, false), 100)
            .unwrap_err()
            .code,
        ControlErrorCode::IdempotencyConflict
    );
    assert_eq!(
        store
            .submit_operation(op(2), pause(s, 1, true), 100)
            .unwrap_err()
            .code,
        ControlErrorCode::RevisionConflict
    );
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(store.operation(&op(1)).unwrap(), receipt);
}

#[test]
fn operation_injected_failures_roll_back_intent_revisions_and_receipts() {
    for (table, timing, condition) in [
        ("control_operations", "INSERT", "1"),
        ("operation_targets", "INSERT", "NEW.membership_id=2"),
        ("member_control", "UPDATE", "NEW.membership_id=2"),
    ] {
        let (_dir, mut store, s, _, _, _, _) = two_groups();
        let before = store.snapshot().unwrap();
        let targets_before = targets(&store);
        store.conn.execute_batch(&format!("CREATE TRIGGER injected_control BEFORE {timing} ON {table} WHEN {condition} BEGIN SELECT RAISE(ABORT,'fake-failure'); END;")).unwrap();
        assert_eq!(
            store
                .submit_operation(op(1), pause(s, 1, true), 100)
                .unwrap_err()
                .code,
            ControlErrorCode::Storage
        );
        assert_eq!(store.snapshot().unwrap(), before);
        assert_eq!(targets(&store), targets_before);
        assert_eq!(counts(&store), (0, 0, 0));
    }
}

#[test]
fn retirement_blocks_members_and_resume_does_not_cancel_member_retirement() {
    let (_dir, mut store, s, _, b, ma, mb) = two_groups();
    store
        .submit_operation(
            op(1),
            ControlCommand::RetireMember {
                membership_id: ma,
                expected_revision: 1,
            },
            10,
        )
        .unwrap();
    store
        .submit_operation(op(2), pause(s, 1, true), 11)
        .unwrap();
    store
        .submit_operation(op(3), pause(s, 2, false), 12)
        .unwrap();
    assert!(store.member_target(ma).unwrap().retire_requested);
    assert_eq!(
        store.member_target(ma).unwrap().status_directive,
        StatusDirective::Disabled
    );
    assert_eq!(
        store.member_target(mb).unwrap().status_directive,
        StatusDirective::Enabled
    );
    assert_eq!(
        store
            .submit_operation(
                op(4),
                ControlCommand::EnsureMember {
                    membership_id: ma,
                    expected_revision: 4,
                    status: StatusDirective::Enabled
                },
                13
            )
            .unwrap_err()
            .code,
        ControlErrorCode::NotReady
    );
    store
        .submit_operation(
            op(5),
            ControlCommand::RetireSource {
                source_id: s,
                expected_revision: 3,
            },
            14,
        )
        .unwrap();
    assert!(store.snapshot().unwrap().sources[0].manual_paused);
    assert!(store.snapshot().unwrap().sources[0].retired_at.is_none());
    expect(store.plan_membership(s, b, 10), ErrorKind::Retired);
    assert_eq!(
        store
            .submit_operation(op(6), pause(s, 4, false), 15)
            .unwrap_err()
            .code,
        ControlErrorCode::NotReady
    );
}

#[test]
fn source_models_update_preserves_key_and_quota_material() {
    let (_dir, mut store) = setup();
    let (cfg, channels) = legacy(2);
    let plan = build_legacy_import(&cfg, &channels).unwrap();
    let ImportOutcome::Applied(receipt) = store.import_legacy(&plan).unwrap() else {
        panic!()
    };
    let id = receipt.source_ids[0];
    let before = store.read_source_material(id).unwrap();
    let other_before = store.read_source_material(receipt.source_ids[1]).unwrap();
    let result = store
        .submit_operation(
            op(1),
            ControlCommand::SyncSourceModels {
                source_id: id,
                expected_revision: 1,
                models: vec![" b ".into(), "a".into(), "b".into()],
            },
            10,
        )
        .unwrap();
    assert_eq!(result.total_targets, 1);
    let after = store.read_source_material(id).unwrap();
    assert_eq!(after.credential.expose(), before.credential.expose());
    let mut expected = before.settings;
    let SourceSettings::Glm { upstream, .. } = &mut expected else {
        panic!()
    };
    upstream.models = vec!["a".into(), "b".into()];
    assert_eq!(
        encode_settings(&after.settings).unwrap(),
        encode_settings(&expected).unwrap()
    );
    assert_eq!(
        encode_settings(
            &store
                .read_source_material(receipt.source_ids[1])
                .unwrap()
                .settings
        )
        .unwrap(),
        encode_settings(&other_before.settings).unwrap()
    );
    let replay = store
        .submit_operation(
            op(1),
            ControlCommand::SyncSourceModels {
                source_id: id,
                expected_revision: 1,
                models: vec!["b".into(), "a".into()],
            },
            20,
        )
        .unwrap();
    assert_eq!(replay, result);
}

#[test]
fn invalid_and_incomplete_group_commands_leave_everything_unchanged() {
    let (_dir, mut store, _, a, _, ma, mb) = two_groups();
    let before = targets(&store);
    for priorities in [
        vec![],
        vec![(mb, 1)],
        vec![(ma, 1), (ma, 2)],
        vec![(ma, JSON_SAFE + 1)],
    ] {
        let command = ControlCommand::SetGroupPriorities {
            group_id: a,
            expected_revision: 1,
            priorities,
        };
        assert_eq!(
            store.submit_operation(op(1), command, 0).unwrap_err().code,
            ControlErrorCode::InvalidInput
        );
        assert_eq!(targets(&store), before);
        assert_eq!(counts(&store), (0, 0, 0));
    }
    assert_eq!(
        store
            .submit_operation(
                op(2),
                ControlCommand::EnsureMember {
                    membership_id: MembershipId::new(999).unwrap(),
                    expected_revision: 1,
                    status: StatusDirective::Enabled
                },
                0
            )
            .unwrap_err()
            .code,
        ControlErrorCode::NotFound
    );
    for id in [
        "",
        "short",
        " invalid-xxxxxxxxxxxx",
        "abcdefghijklmnop!",
        "_abcdefghijklmnop",
    ] {
        assert!(OperationId::new(id.into()).is_err());
    }
    assert!(OperationId::new("a".repeat(64)).is_ok());
    assert!(OperationId::new("a".repeat(65)).is_err());
}

#[test]
fn history_success_receipts_survive_new_commands_and_zero_targets_are_local() {
    let (_dir, mut store, s, _, _, _, _) = two_groups();
    let command = pause(s, 1, true);
    store.submit_operation(op(1), command.clone(), 1).unwrap();
    // 模拟第三批核对完毕的历史 fixture，不把本批 API 称作已能接收远端确认。
    store
        .conn
        .execute(
            "UPDATE operation_targets SET confirmed_at=2,receipt_json='{}'",
            [],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE control_operations SET phase='succeeded',updated_at=2",
            [],
        )
        .unwrap();
    let success = store.operation(&op(1)).unwrap();
    assert_eq!(success.confirmed_targets, 2);
    store
        .submit_operation(op(2), pause(s, 2, false), 3)
        .unwrap();
    assert_eq!(store.operation(&op(1)).unwrap(), success);
    assert_eq!(store.submit_operation(op(1), command, 4).unwrap(), success);
    let empty = store
        .create_source(source("fake-empty", SourceKind::Glm))
        .unwrap();
    let result = store
        .submit_operation(
            op(3),
            ControlCommand::RetireSource {
                source_id: empty.source_id,
                expected_revision: 1,
            },
            5,
        )
        .unwrap();
    assert_eq!(
        (result.phase, result.total_targets, result.confirmed_targets),
        (OperationPhase::Succeeded, 0, 0)
    );
    assert_eq!(store.snapshot().unwrap().sources[1].retired_at, Some(5));
}

#[test]
fn revision_overflow_and_incomplete_success_are_rejected() {
    let (dir, mut store, s, _, _, _, mb) = two_groups();
    store
        .conn
        .execute(
            "UPDATE member_control SET revision=?1 WHERE membership_id=?2",
            params![JSON_SAFE, mb.get()],
        )
        .unwrap();
    let before = store.snapshot().unwrap();
    let target_before = targets(&store);
    assert_eq!(
        store
            .submit_operation(op(1), pause(s, 1, true), 1)
            .unwrap_err()
            .code,
        ControlErrorCode::InvalidInput
    );
    assert_eq!(store.snapshot().unwrap(), before);
    assert_eq!(targets(&store), target_before);
    assert_eq!(counts(&store), (0, 0, 0));
    store
        .conn
        .execute("UPDATE member_control SET revision=1", [])
        .unwrap();
    store.submit_operation(op(2), pause(s, 1, true), 1).unwrap();
    store
        .conn
        .execute("UPDATE control_operations SET phase='succeeded'", [])
        .unwrap();
    assert_eq!(
        store.operation(&op(2)).unwrap_err().code,
        ControlErrorCode::Storage
    );
    drop(store);
    expect(
        CatalogStore::open(&dir.path().join("catalog.sqlite3")),
        ErrorKind::CorruptData,
    );
}

#[test]
fn control_operation_schema_output() {
    let (_dir, mut store, s, _, _, _, _) = two_groups();
    let queued = store
        .submit_operation(op(1), pause(s, 1, true), 100)
        .unwrap();
    let empty = store
        .create_source(source("schema-fake", SourceKind::Glm))
        .unwrap();
    let success = store
        .submit_operation(op(2), pause(empty.source_id, 1, true), 200)
        .unwrap();
    println!(
        "CONTROL_SCHEMA_JSON={}",
        serde_json::to_string(&vec![queued, success]).unwrap()
    );
    println!(
        "CONTROL_ENUM_JSON={}",
        serde_json::json!({"kind":OperationKind::VALUES,"phase":OperationPhase::VALUES,"target_type":TargetType::VALUES,"error_code":ControlErrorCode::VALUES,"status_directive":StatusDirective::VALUES,"create_state":CreateState::VALUES})
    );
}

#[test]
fn lost_target_rows_cannot_turn_nonzero_operation_into_zero_target_success() {
    for kind in 0..3 {
        let (dir, mut store, s, a, _, ma, _) = two_groups();
        let command = match kind {
            0 => ControlCommand::EnsureMember {
                membership_id: ma,
                expected_revision: 1,
                status: StatusDirective::Disabled,
            },
            1 => pause(s, 1, true),
            _ => ControlCommand::SetGroupPriorities {
                group_id: a,
                expected_revision: 1,
                priorities: vec![(ma, 100)],
            },
        };
        store.submit_operation(op(1), command, 1).unwrap();
        store
            .conn
            .execute("DELETE FROM operation_targets", [])
            .unwrap();
        assert_eq!(
            store.operation(&op(1)).unwrap_err().code,
            ControlErrorCode::Storage
        );
        store
            .conn
            .execute("UPDATE control_operations SET phase='succeeded'", [])
            .unwrap();
        assert_eq!(
            store.operation(&op(1)).unwrap_err().code,
            ControlErrorCode::Storage
        );
        drop(store);
        expect(
            CatalogStore::open(&dir.path().join("catalog.sqlite3")),
            ErrorKind::CorruptData,
        );
    }
}

#[test]
fn successful_receipt_losing_one_or_all_targets_is_corrupt() {
    for all in [false, true] {
        let (dir, mut store, s, _, _, ma, _) = two_groups();
        store.submit_operation(op(1), pause(s, 1, true), 1).unwrap();
        store
            .conn
            .execute(
                "UPDATE operation_targets SET confirmed_at=2,receipt_json='{}'",
                [],
            )
            .unwrap();
        store
            .conn
            .execute("UPDATE control_operations SET phase='succeeded'", [])
            .unwrap();
        assert_eq!(store.operation(&op(1)).unwrap().confirmed_targets, 2);
        if all {
            store
                .conn
                .execute("DELETE FROM operation_targets", [])
                .unwrap();
        } else {
            store
                .conn
                .execute(
                    "DELETE FROM operation_targets WHERE membership_id=?1",
                    [ma.get()],
                )
                .unwrap();
        }
        assert_eq!(
            store.operation(&op(1)).unwrap_err().code,
            ControlErrorCode::Storage
        );
        drop(store);
        expect(
            CatalogStore::open(&dir.path().join("catalog.sqlite3")),
            ErrorKind::CorruptData,
        );
    }
}

#[test]
fn mixed_provider_multi_group_v1_migration_preserves_bound_and_unbound_members() {
    let (dir, mut store) = setup();
    let mut expected_material = Vec::new();
    for (prefix, kind) in [("glm", SourceKind::Glm), ("volcano", SourceKind::Volcano)] {
        let source = store
            .create_source(source(&format!("fake-{prefix}"), kind))
            .unwrap();
        let material = store.read_source_material(source.source_id).unwrap();
        expected_material.push((
            source.source_id,
            encode_settings(&material.settings).unwrap(),
            material.credential.expose().to_string(),
        ));
        let a = store
            .create_group(group(&format!("{prefix}-a"), kind))
            .unwrap();
        let b = store
            .create_group(group(&format!("{prefix}-b"), kind))
            .unwrap();
        let ma = store
            .plan_membership(source.source_id, a.group_id, 21)
            .unwrap();
        let mb = store
            .plan_membership(source.source_id, b.group_id, 43)
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE memberships SET channel_id=?1,lifecycle=?2 WHERE membership_id=?3",
                params![
                    ma.membership_id.get() + 100,
                    if kind == SourceKind::Glm {
                        "ready"
                    } else {
                        "created_disabled"
                    },
                    ma.membership_id.get()
                ],
            )
            .unwrap();
        if kind == SourceKind::Volcano {
            store.conn.execute("UPDATE memberships SET lifecycle='retired',retired_at=5 WHERE membership_id=?1",[mb.membership_id.get()]).unwrap();
        }
        store
            .conn
            .execute(
                "UPDATE sources SET manual_paused=1,config_revision=7 WHERE source_id=?1",
                [source.source_id.get()],
            )
            .unwrap();
    }
    let expected = store.snapshot().unwrap();
    turn_fixture_into_v1(&mut store);
    drop(store);
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(store.snapshot().unwrap(), expected);
    for (id, settings, key) in expected_material {
        let material = store.read_source_material(id).unwrap();
        assert_eq!(encode_settings(&material.settings).unwrap(), settings);
        assert_eq!(material.credential.expose(), key);
    }
    for member in expected.memberships {
        let target = store.member_target(member.membership_id).unwrap();
        assert!(!target.dirty);
        assert_eq!(target.status_directive, StatusDirective::Preserve);
        assert_eq!(
            target.create_state,
            if member.channel_id.is_some() {
                CreateState::Resolved
            } else {
                CreateState::Never
            }
        );
    }
}

#[test]
fn full_group_operation_is_bounded_and_preserves_each_priority() {
    let (_dir, mut store) = setup();
    let (cfg, channels) = legacy(MAX_SOURCES);
    let plan = build_legacy_import(&cfg, &channels).unwrap();
    let ImportOutcome::Applied(receipt) = store.import_legacy(&plan).unwrap() else {
        panic!()
    };
    let priorities: Vec<_> = receipt
        .membership_ids
        .iter()
        .map(|id| (*id, id.get() % 97 - 48))
        .collect();
    let started = Instant::now();
    let result = store
        .submit_operation(
            op(1),
            ControlCommand::SetGroupPriorities {
                group_id: receipt.group_id,
                expected_revision: 1,
                priorities: priorities.clone(),
            },
            1,
        )
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(result.total_targets, MAX_SOURCES);
    assert_eq!(result.confirmed_targets, 0);
    for (id, priority) in priorities {
        let target = store.member_target(id).unwrap();
        assert_eq!(target.desired_priority, priority);
        assert_eq!(target.revision, 2);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn r1_reference_operation_replay_and_reopen(actions in prop::collection::vec(any::<bool>(),1..24)) {
        let (dir,mut store,s,_,_,ma,mb)=two_groups();let mut commands=Vec::new();
        for (index,paused) in actions.iter().enumerate() {
            let command=pause(s,index as i64+1,*paused);let result=store.submit_operation(op(index),command.clone(),index as i64).unwrap();
            prop_assert_eq!(result.phase,OperationPhase::Queued);prop_assert_eq!(result.confirmed_targets,0);
            prop_assert_eq!(store.snapshot().unwrap().sources[0].manual_paused,*paused);
            for member in [ma,mb] {let t=store.member_target(member).unwrap();prop_assert_eq!(t.revision,index as i64+2);prop_assert_eq!(t.status_directive,if *paused {StatusDirective::Disabled}else{StatusDirective::Enabled});}
            commands.push(command);
            for (old,command) in commands.iter().enumerate() {
                let receipt=store.submit_operation(op(old),command.clone(),1000).unwrap();
                prop_assert_eq!(receipt.phase,if old==index {OperationPhase::Queued}else{OperationPhase::Superseded});
                prop_assert_eq!(receipt.total_targets,2);prop_assert_eq!(receipt.confirmed_targets,0);
            }
            prop_assert_eq!(counts(&store),((index+1) as i64,((index+1)*2) as i64,2));
        }
        let expected=store.snapshot().unwrap();let expected_targets=targets(&store);drop(store);
        let mut store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        prop_assert_eq!(store.snapshot().unwrap(),expected);prop_assert_eq!(targets(&store),expected_targets);
    }

    #[test]
    fn r2_group_priorities_do_not_touch_other_group(priority in -10000i64..10000,reverse in any::<bool>()) {
        let (_dir,mut store,s,a,b,ma,mb)=two_groups();
        let other=store.create_source(source("fake-second-key",SourceKind::Volcano)).unwrap();
        let oa=store.plan_membership(other.source_id,a,31).unwrap();let ob=store.plan_membership(other.source_id,b,71).unwrap();
        let untouched=[store.member_target(mb).unwrap(),store.member_target(ob.membership_id).unwrap()];
        let mut priorities=vec![(ma,priority),(oa.membership_id,priority+1)];if reverse {priorities.reverse();}
        let result=store.submit_operation(op(1),ControlCommand::SetGroupPriorities {group_id:a,expected_revision:1,priorities:priorities.clone()},1).unwrap();
        prop_assert_eq!(result.total_targets,2);prop_assert_eq!(store.member_target(ma).unwrap().desired_priority,priority);
        prop_assert_eq!(store.member_target(oa.membership_id).unwrap().desired_priority,priority+1);
        prop_assert_eq!([store.member_target(mb).unwrap(),store.member_target(ob.membership_id).unwrap()],untouched);
        priorities.reverse();prop_assert_eq!(store.submit_operation(op(1),ControlCommand::SetGroupPriorities {group_id:a,expected_revision:1,priorities},2).unwrap(),result);
        let material=store.read_source_material(s).unwrap();prop_assert_eq!(material.credential.expose(),"fake-shared-key");
    }

    #[test]
    fn r11_v1_migration_preserves_complete_mapping_material_and_import_receipt(n in 1usize..7,paused in any::<bool>(),retire in any::<bool>()) {
        let (dir,mut store)=setup();let (cfg,mut channels)=legacy(n);
        for c in &mut channels {c.models_csv=format!("source-{},shared",c.id);c.priority=c.id*3-302;}
        let plan=build_legacy_import(&cfg,&channels).unwrap();let outcome=store.import_legacy(&plan).unwrap();
        let ImportOutcome::Applied(ref receipt)=outcome else {panic!()};
        if paused {store.conn.execute("UPDATE sources SET manual_paused=1 WHERE source_id%2=1",[]).unwrap();}
        if retire {store.conn.execute("UPDATE memberships SET lifecycle='retired',retired_at=1 WHERE membership_id%2=0",[]).unwrap();}
        let expected=store.snapshot().unwrap();
        let material:Vec<_>=receipt.source_ids.iter().map(|id|{let m=store.read_source_material(*id).unwrap();(*id,encode_settings(&m.settings).unwrap(),m.credential.expose().to_string())}).collect();
        turn_fixture_into_v1(&mut store);drop(store);let mut store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        prop_assert_eq!(store.snapshot().unwrap(),expected);
        for (id,settings,key) in material {let m=store.read_source_material(id).unwrap();prop_assert_eq!(encode_settings(&m.settings).unwrap(),settings);prop_assert_eq!(m.credential.expose(),key);}
        prop_assert_eq!(store.import_legacy(&plan).unwrap(),ImportOutcome::AlreadyApplied(receipt.clone()));
        for target in targets(&store) {prop_assert!(!target.dirty);prop_assert_eq!(target.status_directive,StatusDirective::Preserve);prop_assert_eq!(target.create_state,CreateState::Resolved);}
        prop_assert_eq!(backups(&dir).len(),1);
    }

    #[test]
    fn operation_views_and_direct_command_debug_do_not_leak_material(suffix in "[a-zA-Z0-9]{16,40}") {
        let (_dir,mut store)=setup();let secret=format!("PRIVATE-{suffix}");let s=store.create_source(source(&secret,SourceKind::Glm)).unwrap();
        let command=ControlCommand::SyncSourceModels {source_id:s.source_id,expected_revision:1,models:vec![secret.clone()]};
        let diagnostic=format!("{command:?}");prop_assert!(!diagnostic.contains(&secret));
        let view=store.submit_operation(op(1),command,1).unwrap();let output=format!("{view:?} {}",serde_json::to_string(&view).unwrap());
        prop_assert!(!output.contains(&secret));prop_assert!(!output.contains("payload_digest"));
    }
}
