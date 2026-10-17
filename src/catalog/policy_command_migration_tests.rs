use super::*;
use crate::catalog::ledger::{PageEnd, ScanLane, ScanWindow};
use crate::newapi::managed_channels::{ChannelObservation, NativeRecoveryCheck};
use crate::reconcile::planner::{
    BackendReadiness, PlanContext, StampedObservation, SUPPORTED_BACKEND_VERSION,
};
use std::time::{Duration, Instant, SystemTime};

fn ingest(f: &mut Fixture, rows: &[serde_json::Value], at: i64) {
    let old = f
        .store
        .load_ledger_state()
        .unwrap()
        .scans
        .into_iter()
        .find(|p| p.lane == ScanLane::Recent)
        .unwrap();
    let t = f
        .store
        .begin_scan(
            ScanLane::Recent,
            old.generation,
            ScanWindow { from: 0, to: at },
            at,
        )
        .unwrap();
    f.store
        .commit_fact_page(&t, rows, PageEnd::ReachedEnd, at)
        .unwrap();
}
fn history_fixture() -> Fixture {
    let mut f = fixture();
    f.store.initialize_ledger(0, NOW).unwrap();
    let rows=(1..=4).map(|id|json!({"id":id,"type":if id==1{2}else{5},"created_at":NOW-1,"channel":1003,"token_id":77,"model_name":"fake-c2-model","group":"c2-2","prompt_tokens":if id==1{4}else{0},"completion_tokens":if id==1{2}else{0},"is_stream":false,"request_id":format!("r{id}"),"other":if id==1 {String::new()}else{json!({"error_type":"openai_error","status_code":503,"error_code":"unavailable"}).to_string()}})).collect::<Vec<_>>();
    ingest(&mut f, &rows, NOW);
    let cfg: Config =
        toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\n").unwrap();
    let backend = BackendReadiness::from_preflight(
        SUPPORTED_BACKEND_VERSION,
        NativeRecoveryCheck::Disabled,
        true,
        NOW,
        NOW + 100,
    );
    f.store
        .commit_volcano_round(&cfg, backend, &HashMap::new(), NOW)
        .unwrap();
    let r = runtime(&f);
    let due = r.next_reset_at;
    f.store
        .apply_source_policy(f.sources[1], 1, r.revision, SourcePolicyUpdate::Tick, due)
        .unwrap();
    let mut late = rows[1].clone();
    late["id"] = json!(5);
    late["request_id"] = json!("next-day");
    late["created_at"] = json!(due + 1);
    ingest(&mut f, &[late], due + 2);
    let id = f.members[0].get();
    f.store
        .conn
        .execute(
            "UPDATE member_execution SET sequence=2 WHERE membership_id=?1",
            [id],
        )
        .unwrap();
    for (seq, state, error) in [
        (1, "responded", None),
        (2, "uncertain", Some("transport_unknown")),
    ] {
        f.store.conn.execute("INSERT INTO execution_attempts VALUES(?1,?2,2,1,1,0,'fields',?3,?4,?5,?6,?7)",params![id,seq,json!({"action":"fields","priority":100,"models":null,"disable_auto_ban":false}).to_string(),state,error,NOW-2,NOW-1]).unwrap();
    }
    f.store.conn.execute("UPDATE control_operations SET phase='unknown',error_code='transport_unknown' WHERE operation_id=?1",[op(0).as_str()]).unwrap();
    // 通过原确认路径生成真实成功证据，不能只迁移queued/unknown空回执。
    let member = f.members[1];
    let work = f.store.load_work(member, false).unwrap();
    let observation = StampedObservation {
        revision: work.revision(),
        channel: ChannelObservation {
            observed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(NOW as u64),
            channel_id: 1002,
            channel_type: 8,
            name: work.target.create_name.clone(),
            group: work.group.newapi_group.clone(),
            base_url: work.identity.base_url.clone(),
            models: work.models.join(","),
            priority: 100,
            status: 1,
            auto_ban: 1,
            is_multi_key: false,
        },
    };
    let members = [member];
    let context = PlanContext {
        backend,
        permit: None,
        expected_members: &members,
        peers: &[],
        now: NOW,
    };
    f.store
        .confirm_member(member, work.revision(), false, Some(&observation), &context)
        .unwrap();
    assert_eq!(
        f.store.operation(&op(1)).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert_eq!(f.store.conn.query_row("SELECT count(*) FROM operation_targets WHERE confirmed_at IS NOT NULL AND receipt_json IS NOT NULL", [], |r|r.get::<_, i64>(0)).unwrap(), 1);
    f
}
fn as_v6(store: &mut CatalogStore) {
    // 期望镜像取自调用生产重建helper之前，避免夹具与被测迁移同源丢行。
    let mut expected = image(&store.conn);
    expected.get_mut("catalog_meta").unwrap()[0][1] = rusqlite::types::Value::Integer(6);
    let tx = store.conn.transaction().unwrap();
    replace_control_operations(&tx, SCHEMA_V2).unwrap();
    let id: String = tx
        .query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    tx.execute("DROP TABLE catalog_meta", []).unwrap();
    tx.execute_batch(SCHEMA_V6.split(';').next().unwrap())
        .unwrap();
    tx.execute("INSERT INTO catalog_meta VALUES(1,6,?1)", [id])
        .unwrap();
    tx.pragma_update(None, "user_version", 6).unwrap();
    tx.commit().unwrap();
    assert_eq!(image(&store.conn), expected);
}
fn expected_v7(
    mut before: BTreeMap<String, Vec<Vec<rusqlite::types::Value>>>,
) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    before.get_mut("catalog_meta").unwrap()[0][1] = rusqlite::types::Value::Integer(7);
    before
}
fn open_again(f: Fixture) -> Fixture {
    let Fixture {
        dir,
        store,
        sources,
        groups,
        members,
    } = f;
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    Fixture {
        dir,
        store,
        sources,
        groups,
        members,
    }
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn c2_pb2_v6_migration_preserves_all_records_backups_and_reopen(actions in prop::collection::vec(any::<bool>(),0..8)) {
        let mut f=history_fixture();
        for (i,paused) in actions.into_iter().enumerate(){f.store.submit_operation(op(300+i),ControlCommand::SetSourcePaused {source_id:f.sources[0],expected_revision:i as i64+1,paused},NOW).unwrap();}
        let original=image(&f.store.conn);as_v6(&mut f.store);let before=image(&f.store.conn);f=open_again(f);
        prop_assert_eq!(image(&f.store.conn),original);
        let paths=fs::read_dir(f.dir.path()).unwrap().map(|r|r.unwrap().path()).filter(|p|p.extension().is_some_and(|e|e=="sqlite3")&&p.file_name().unwrap().to_string_lossy().starts_with("catalog-v6-before-v7-")).collect::<Vec<_>>();prop_assert_eq!(paths.len(),1);
        let backup=Connection::open_with_flags(&paths[0],OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();verify_schema(&backup,6).unwrap();prop_assert_eq!(image(&backup),before);
        let stable=image(&f.store.conn);f=open_again(f);prop_assert_eq!(image(&f.store.conn),stable);
        prop_assert_eq!(f.store.conn.pragma_query_value(None,"foreign_keys",|r|r.get::<_,i64>(0)).unwrap(),1);
    }
}

#[test]
fn c2_v7_migration_faults_preserve_v6_and_foreign_keys() {
    for extra in [
        "CREATE TABLE _qt_c2_operations(marker TEXT);INSERT INTO _qt_c2_operations VALUES('keep')",
        "CREATE TABLE _qt_c2_targets(marker TEXT);INSERT INTO _qt_c2_targets VALUES('keep')",
        "CREATE INDEX custom_ops ON control_operations(target_id)",
        "CREATE TRIGGER custom_ops AFTER INSERT ON control_operations BEGIN SELECT 1;END",
        "CREATE TABLE custom_fk(id TEXT REFERENCES control_operations(operation_id))",
    ] {
        let mut f = history_fixture();
        as_v6(&mut f.store);
        let before = image(&f.store.conn);
        f.store.conn.execute_batch(extra).unwrap();
        let path = f.dir.path().join("catalog.sqlite3");
        drop(f.store);
        assert!(CatalogStore::open(&path).is_err(), "{extra}");
        let conn = Connection::open(&path).unwrap();
        verify_schema(&conn, 6).unwrap();
        assert_eq!(image(&conn), before);
        if extra.contains("marker") {
            let table = if extra.contains("_qt_c2_targets") {
                "_qt_c2_targets"
            } else {
                "_qt_c2_operations"
            };
            assert_eq!(
                conn.query_row(&format!("SELECT marker FROM {table}"), [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "keep"
            );
        }
    }
    // 直接注入最终schema核验失败，覆盖重建和复制已完成后的整事务回滚。
    let mut f = history_fixture();
    as_v6(&mut f.store);
    f.store
        .conn
        .execute("DROP INDEX ledger_cell_source_time", [])
        .unwrap();
    let before = image(&f.store.conn);
    assert!(f.store.migrate_v6().is_err());
    assert_eq!(image(&f.store.conn), before);
    assert_eq!(
        f.store
            .conn
            .pragma_query_value(None, "foreign_keys", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(!f.store.poisoned);
    f.store
        .conn
        .execute(
            "CREATE INDEX ledger_cell_source_time ON ledger_minute_usage(source_id,minute)",
            [],
        )
        .unwrap();
    verify_schema(&f.store.conn, 6).unwrap();
}

#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_c2_v6_migration_10k_operations() {
    let mut f = history_fixture();
    let tx = f.store.conn.transaction().unwrap();
    for i in 0..10000 {
        tx.execute("INSERT INTO control_operations(operation_id,kind,target_type,target_id,payload_digest,target_revision,expected_targets,phase,created_at,updated_at) VALUES(?1,'sync_source_models','source',?2,?3,1,0,'succeeded',0,0)",params![format!("capacity-operation-{i:016}"),f.sources[0].get(),"a".repeat(64)]).unwrap();
    }
    tx.commit().unwrap();
    as_v6(&mut f.store);
    let before = image(&f.store.conn);
    let start = Instant::now();
    let f = open_again(f);
    let elapsed = start.elapsed();
    assert_eq!(image(&f.store.conn), expected_v7(before));
    eprintln!("C2_V6_MIGRATION_10K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}
