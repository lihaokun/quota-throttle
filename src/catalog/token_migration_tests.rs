use super::*;

fn as_v7(store: &mut CatalogStore) {
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM managed_tokens", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let tx = store.conn.transaction().unwrap();
    tx.execute_batch(DROP_TOKENS_V8).unwrap();
    replace_control_operations(&tx, SCHEMA_V7).unwrap();
    let id: String = tx
        .query_row(
            "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    tx.execute("DROP TABLE catalog_meta", []).unwrap();
    tx.execute_batch(SCHEMA_V7.split(';').next().unwrap())
        .unwrap();
    tx.execute("INSERT INTO catalog_meta VALUES(1,7,?1)", [id])
        .unwrap();
    tx.pragma_update(None, "user_version", 7).unwrap();
    tx.commit().unwrap();
    verify_schema(&store.conn, 7).unwrap();
}
fn expected_v8(
    mut before: BTreeMap<String, Vec<Vec<rusqlite::types::Value>>>,
) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    before.get_mut("catalog_meta").unwrap()[0][1] = rusqlite::types::Value::Integer(8);
    for name in [
        "managed_tokens",
        "token_operation_targets",
        "token_execution_attempts",
    ] {
        assert!(before.insert(name.into(), vec![]).is_none());
    }
    before
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn t5b_pb5_v7_migration_keeps_complex_history_and_backup(pauses in prop::collection::vec(any::<bool>(),0..5)) {
        let (dir,mut store)=super::super::policy_commands::token_history_fixture();let source=store.snapshot().unwrap().sources[0].source_id;
        for (i,paused) in pauses.into_iter().enumerate() {store.submit_operation(op(1000+i),ControlCommand::SetSourcePaused{source_id:source,expected_revision:i as i64+1,paused},NOW).unwrap();}
        assert!(store.conn.query_row("SELECT count(*) FROM operation_targets WHERE confirmed_at IS NOT NULL",[],|r|r.get::<_,i64>(0)).unwrap()>0);
        for table in ["ledger_facts","ledger_minute_usage","volcano_period_history","execution_attempts"] {prop_assert!(store.conn.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get::<_,i64>(0)).unwrap()>0,"complex history must be nonempty");}
        let original=image(&store.conn);as_v7(&mut store);let before=image(&store.conn);store=reopen(&dir,store);prop_assert_eq!(image(&store.conn),original);
        let backups=fs::read_dir(dir.path()).unwrap().map(|v|v.unwrap().path()).filter(|p|p.file_name().unwrap().to_string_lossy().starts_with("catalog-v7-before-v8-")).collect::<Vec<_>>();prop_assert_eq!(backups.len(),1);
        let saved=Connection::open_with_flags(&backups[0],OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();verify_schema(&saved,7).unwrap();prop_assert_eq!(image(&saved),before);
        let stable=image(&store.conn);store=reopen(&dir,store);prop_assert_eq!(image(&store.conn),stable);
    }
}

#[test]
fn t5b_migration_failures_keep_v7_and_reject_foreign_constraints() {
    for obstacle in [
        "CREATE TABLE _qt_c2_operations(marker TEXT);INSERT INTO _qt_c2_operations VALUES('keep')",
        "CREATE TABLE managed_tokens(marker TEXT);INSERT INTO managed_tokens VALUES('keep')",
        "CREATE INDEX external_operations ON control_operations(target_id)",
        "CREATE TABLE external_reference(op TEXT REFERENCES control_operations(operation_id))",
    ] {
        let (dir, mut store) = super::super::policy_commands::token_history_fixture();
        as_v7(&mut store);
        store.conn.execute_batch(obstacle).unwrap();
        let before = image(&store.conn);
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
        let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(image(&conn), before);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        verify_schema(&conn, 7).unwrap();
    }
    let (_dir, mut store) = super::super::policy_commands::token_history_fixture();
    as_v7(&mut store);
    store
        .conn
        .execute("DROP INDEX ledger_cell_source_time", [])
        .unwrap();
    let before = image(&store.conn);
    assert!(store.migrate_v7().is_err());
    assert_eq!(image(&store.conn), before);
    assert!(!store.poisoned);
}

#[test]
fn t5b_reopen_rejects_corrupt_identity_receipt_and_future_schema() {
    for damage in 0..7 {
        let (dir, mut store, g) = fixture();
        let id = submit(&mut store, g, 0, None);
        let ticket = prepare(&mut store, 0, NOW);
        confirm_create(&mut store, 0, &ticket, 91, NOW);
        match damage {
            0 => {
                store
                    .conn
                    .execute("DELETE FROM token_execution_attempts", [])
                    .unwrap();
                store
                    .conn
                    .execute("DELETE FROM token_operation_targets", [])
                    .unwrap();
            }
            1 => {
                store.conn.execute("UPDATE token_operation_targets SET receipt_json=json_set(receipt_json,'$.owner_user_id','42')",[]).unwrap();
            }
            2 => {
                store.conn.execute("UPDATE token_operation_targets SET receipt_json=json_set(receipt_json,'$.key','forbidden')",[]).unwrap();
            }
            3 => {
                store
                    .conn
                    .execute("UPDATE managed_tokens SET evidence_revision=0", [])
                    .unwrap();
            }
            4 => {
                store
                    .conn
                    .execute("DROP INDEX token_one_prepared", [])
                    .unwrap();
            }
            5 => {
                store.conn.pragma_update(None, "user_version", 9).unwrap();
            }
            _ => {
                store
                    .conn
                    .execute(
                        "UPDATE managed_tokens SET display_name='bad\nlabel' WHERE token_id=?1",
                        [id.get()],
                    )
                    .unwrap();
            }
        }
        let before = image(&store.conn);
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
        let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(image(&conn), before);
    }
}

#[test]
fn t5b_reopen_requires_reverse_lifecycle_evidence() {
    let mut accepted = Vec::new();
    for damage in 0..3 {
        let (dir, mut store, group) = fixture();
        if damage == 1 {
            submit(&mut store, group, 0, None);
            let ticket = prepare(&mut store, 0, NOW);
            confirm_create(&mut store, 0, &ticket, 91, NOW);
            store
                .conn
                .execute("DELETE FROM token_execution_attempts", [])
                .unwrap();
        } else {
            imported(&mut store, 91, "");
            store
                .conn
                .execute(
                    if damage == 0 {
                        "UPDATE managed_tokens SET deleted_at=created_at"
                    } else {
                        "UPDATE managed_tokens SET revision=2"
                    },
                    [],
                )
                .unwrap();
        }
        let before = image(&store.conn);
        drop(store);
        if CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_ok() {
            accepted.push(damage);
        }
        let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(image(&conn), before);
    }
    assert!(
        accepted.is_empty(),
        "accepted unreachable lifecycle states: {accepted:?}"
    );
}

#[test]
fn t5b_capacity_pagination_and_overflow_do_not_truncate_or_wrap() {
    let (_dir, mut store, _) = fixture();
    let tx = store.conn.transaction().unwrap();
    for id in 1..=10_000 {
        tx.execute("INSERT INTO managed_tokens(owner_user_id,owner_scope,native_token_id,explicit_group,display_name,revision,evidence_revision,created_at) VALUES(41,'issuer',?1,'','fixture',1,0,?2)",params![id,NOW]).unwrap();
    }
    tx.commit().unwrap();
    let before = image(&store.conn);
    assert_eq!(
        code(store.register_token(owner(), &native(10001, ""), "extra".into(), NOW)),
        Code::QueueFull
    );
    assert_eq!(image(&store.conn), before);
    let mut cursor = None;
    let mut actual = Vec::new();
    loop {
        let page = store.list_token_identities(cursor).unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 100);
        cursor = page.last().map(|v| v.token_id);
        actual.extend(
            page.into_iter()
                .map(|v| v.native_token_id.unwrap().parse::<i64>().unwrap()),
        );
    }
    assert_eq!(actual, (1..=10_000).collect::<Vec<_>>());
    for n in 0..65 {
        store
            .submit_token_operation(
                op(n),
                TokenCommand::DeleteToken {
                    token_id: ManagedTokenId::new(n as i64 + 1).unwrap(),
                    expected_revision: 1,
                },
                NOW,
            )
            .unwrap();
    }
    assert_eq!(
        store.pending_token_operations(NOW).unwrap(),
        (0..64).map(op).collect::<Vec<_>>()
    );
    let id = ManagedTokenId::new(10_000).unwrap();
    store
        .conn
        .execute(
            "UPDATE managed_tokens SET revision=?1 WHERE token_id=10000",
            [JSON_SAFE],
        )
        .unwrap();
    let before = image(&store.conn);
    assert_eq!(
        code(store.submit_token_operation(
            op(999),
            TokenCommand::DeleteToken {
                token_id: id,
                expected_revision: JSON_SAFE
            },
            NOW
        )),
        Code::InvalidInput
    );
    assert_eq!(image(&store.conn), before);
}

#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_token_identity_10k() {
    let (_dir, mut store, _) = fixture();
    let tx = store.conn.transaction().unwrap();
    for id in 1..=10_000 {
        tx.execute("INSERT INTO managed_tokens(owner_user_id,owner_scope,native_token_id,explicit_group,display_name,revision,evidence_revision,created_at) VALUES(41,'issuer',?1,'',?2,1,0,?3)",params![id,"标".repeat(85),NOW]).unwrap();
    }
    tx.commit().unwrap();
    let start = Instant::now();
    crate::catalog::tokens::verify_tokens(&store.conn).unwrap();
    let mut cursor = None;
    let mut count = 0;
    loop {
        let rows = store.list_token_identities(cursor).unwrap();
        if rows.is_empty() {
            break;
        }
        cursor = rows.last().map(|v| v.token_id);
        count += rows.len();
    }
    let elapsed = start.elapsed();
    assert_eq!(count, 10000);
    eprintln!("TOKEN_IDENTITIES_10K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_token_v7_migration_10k_operations() {
    let (dir, mut store) = super::super::policy_commands::token_history_fixture();
    let source = store.snapshot().unwrap().sources[0].source_id;
    let tx = store.conn.transaction().unwrap();
    for i in 0..10000 {
        tx.execute("INSERT INTO control_operations(operation_id,kind,target_type,target_id,payload_digest,target_revision,expected_targets,phase,created_at,updated_at) VALUES(?1,'sync_source_models','source',?2,?3,1,0,'succeeded',0,0)",params![format!("token-old-operation-{i:016}"),source.get(),"a".repeat(64)]).unwrap();
    }
    tx.commit().unwrap();
    as_v7(&mut store);
    let before = image(&store.conn);
    let start = Instant::now();
    store = reopen(&dir, store);
    let elapsed = start.elapsed();
    assert_eq!(image(&store.conn), expected_v8(before));
    eprintln!("TOKEN_V7_MIGRATION_10K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}
