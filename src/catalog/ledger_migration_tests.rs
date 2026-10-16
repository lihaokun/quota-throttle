use super::*;
use crate::catalog::ledger::ScanPhase;

fn migrate_case(version: i64, n: usize, nv: usize, salt: i64, offset: i64, prepared: bool) {
    let mut h = history(n, nv, salt, offset, prepared); // 保留旧B5完整材料/收据/未决执行，不另建简化历史。
    let mut tables = [CORE, CONTROL, EXECUTION].concat();
    if version == 4 {
        h.store.migrate_v3().unwrap();
        for &id in &h.volcano {
            h.store.initialize_source_policy(id, 1, NOW).unwrap();
        }
        tables.extend(["volcano_runtime", "group_policy_state"]);
    } else if version < 3 {
        let tx = h.store.conn.transaction().unwrap();
        let id = instance(&tx);
        tx.execute_batch(
            "DROP TABLE execution_attempts; DROP TABLE member_execution; DROP TABLE catalog_meta;",
        )
        .unwrap();
        if version == 1 {
            tx.execute_batch("DROP TABLE operation_targets; DROP TABLE control_operations; DROP TABLE member_control; DROP TABLE source_control;").unwrap();
            tx.execute_batch(SCHEMA.split(';').next().unwrap()).unwrap();
            tx.execute("INSERT INTO catalog_meta VALUES(1,1)", [])
                .unwrap();
            tables = CORE.to_vec();
        } else {
            tx.execute_batch(SCHEMA_V2.split(';').next().unwrap())
                .unwrap();
            tx.execute("INSERT INTO catalog_meta VALUES(1,2,?1)", [id])
                .unwrap();
            tables = [CORE, CONTROL].concat();
        }
        tx.pragma_update(None, "user_version", version).unwrap();
        tx.commit().unwrap();
    }
    let before = image_of(&h.store.conn, &tables);
    let original_id = if version > 1 {
        Some(instance(&h.store.conn))
    } else {
        None
    };
    drop(h.store);
    let low = epoch();
    let mut store = CatalogStore::open(&h.dir.path().join("catalog.sqlite3")).unwrap();
    let high = epoch();
    let after = image_of(&store.conn, &tables);
    if version >= 3 {
        assert_recovery(&before, &after, low, high);
    } else {
        assert_images(&after, &before);
    }
    if let Some(id) = original_id {
        assert_eq!(instance(&store.conn), id);
    }
    assert_eq!(
        store
            .conn
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(store.snapshot().unwrap().schema_version, 1);
    let state = store.load_ledger_state().unwrap();
    assert_eq!(state.revision, 1);
    assert_eq!(state.initial_requested_from, None);
    assert!(state.scans.iter().all(|s| s.phase == ScanPhase::Idle));
    let files = backup_files(&h.dir, version);
    assert_eq!(files.len(), 1);
    let backup = Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    verify_schema(&backup, version).unwrap();
    assert_images(&image_of(&backup, &tables), &before);
    let stable = history_image(&store.conn);
    drop(store);
    let store = CatalogStore::open(&h.dir.path().join("catalog.sqlite3")).unwrap();
    assert_images(&history_image(&store.conn), &stable);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb6_v1_to_v4_complex_migration_is_lossless(version in 1i64..=4,n in 4usize..7,nv in 2usize..5,salt in 0i64..10000,offset in -720i64..=840,prepared in any::<bool>()) {
        migrate_case(version,n,nv,salt,offset,prepared);
    }
}

#[test]
fn v5_migration_failure_keeps_v4_and_original_backup() {
    for collision in [
        "ledger_state",
        "ledger_checkpoints",
        "ledger_facts",
        "ledger_minute_usage",
    ] {
        let mut h = history(4, 2, 27, 480, true);
        h.store.migrate_v3().unwrap();
        let before = image_of(
            &h.store.conn,
            &[
                CORE,
                CONTROL,
                EXECUTION,
                &["volcano_runtime", "group_policy_state"],
            ]
            .concat(),
        );
        let identity = instance(&h.store.conn);
        h.store.conn.execute_batch(&format!("CREATE TABLE {collision}(marker TEXT); INSERT INTO {collision} VALUES('owned-fixture');")).unwrap();
        drop(h.store);
        assert!(CatalogStore::open(&h.dir.path().join("catalog.sqlite3")).is_err());
        let conn = Connection::open(h.dir.path().join("catalog.sqlite3")).unwrap();
        verify_schema(&conn, 4).unwrap();
        assert_eq!(instance(&conn), identity);
        assert_images(
            &image_of(
                &conn,
                &[
                    CORE,
                    CONTROL,
                    EXECUTION,
                    &["volcano_runtime", "group_policy_state"],
                ]
                .concat(),
            ),
            &before,
        );
        assert_eq!(
            conn.query_row(&format!("SELECT marker FROM {collision}"), [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "owned-fixture"
        );
        let files = backup_files(&h.dir, 4);
        assert_eq!(files.len(), 1);
        let backup =
            Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        verify_schema(&backup, 4).unwrap();
        assert_images(
            &image_of(
                &backup,
                &[
                    CORE,
                    CONTROL,
                    EXECUTION,
                    &["volcano_runtime", "group_policy_state"],
                ]
                .concat(),
            ),
            &before,
        );
        conn.execute(&format!("DROP TABLE {collision}"), [])
            .unwrap();
        drop(conn);
        let low = epoch();
        let store = CatalogStore::open(&h.dir.path().join("catalog.sqlite3")).unwrap();
        let high = epoch();
        assert_recovery(
            &before,
            &image_of(
                &store.conn,
                &[
                    CORE,
                    CONTROL,
                    EXECUTION,
                    &["volcano_runtime", "group_policy_state"],
                ]
                .concat(),
            ),
            low,
            high,
        );
    }
}

#[test]
fn v4_corruption_is_rejected_before_v5_backup() {
    let mut h = history(4, 2, 99, 480, false);
    h.store.migrate_v3().unwrap();
    h.store.conn.execute("DELETE FROM group_policy_state WHERE group_id=(SELECT min(group_id) FROM source_groups)",[]).unwrap();
    let before = history_image(&h.store.conn);
    drop(h.store);
    assert!(CatalogStore::open(&h.dir.path().join("catalog.sqlite3")).is_err());
    assert!(backup_files(&h.dir, 4).is_empty());
    let conn = Connection::open(h.dir.path().join("catalog.sqlite3")).unwrap();
    assert_images(&history_image(&conn), &before);
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_ledger_v4_migration() {
    let mut h = history(20, 10, 77, 480, true);
    h.store.migrate_v3().unwrap();
    let before = history_image(&h.store.conn);
    drop(h.store);
    let low = epoch();
    let start = Instant::now();
    let store = CatalogStore::open(&h.dir.path().join("catalog.sqlite3")).unwrap();
    let elapsed = start.elapsed();
    let high = epoch();
    assert_recovery(&before, &history_image(&store.conn), low, high);
    assert_eq!(backup_files(&h.dir, 4).len(), 1);
    println!("LEDGER_V4_MIGRATION_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}
