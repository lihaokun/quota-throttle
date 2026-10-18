use super::*;

fn legacy_image(conn: &Connection) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    let mut q=conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT IN ('catalog_meta','sqlite_sequence') AND name NOT LIKE 'volcano_observation%' AND name NOT IN ('volcano_request_observations','volcano_period_history','managed_tokens','token_operation_targets','token_execution_attempts') ORDER BY name").unwrap();
    let tables = q
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    tables
        .into_iter()
        .map(|t| {
            let mut q = conn
                .prepare(&format!("SELECT * FROM {t} ORDER BY rowid"))
                .unwrap();
            let n = q.column_count();
            let rows = q
                .query_map([], |r| (0..n).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            (t, rows)
        })
        .collect()
}
fn downgrade(f: &mut Fixture) {
    let tx = f.store.conn.transaction().unwrap();
    tx.execute_batch(DROP_TOKENS_V8).unwrap();
    replace_control_operations(&tx, SCHEMA_V2).unwrap();
    let instance: String = tx
        .query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    tx.execute_batch(DROP_OBSERVED_V6).unwrap();
    tx.execute_batch("DROP TABLE catalog_meta").unwrap();
    tx.execute_batch(SCHEMA_V5.split(';').next().unwrap())
        .unwrap();
    tx.execute("INSERT INTO catalog_meta VALUES(1,5,?1)", [instance])
        .unwrap();
    tx.pragma_update(None, "user_version", 5).unwrap();
    tx.commit().unwrap();
}
fn reopen(f: Fixture) -> Fixture {
    let Fixture {
        dir,
        store,
        sources,
        groups,
        members,
        cfg,
    } = f;
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    Fixture {
        dir,
        store,
        sources,
        groups,
        members,
        cfg,
    }
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn observed_publication_pb1_v5_migration_preserves_state_and_does_not_replay(n in 1usize..12, paused in any::<bool>()) {
        let mut f=fixture();
        let rows=(1..=n).map(|id|event(id as i64,1001,&format!("old-{id}"),id%2==0,NOW-1)).collect::<Vec<_>>();
        ingest(&mut f.store,&rows,NOW);round(&mut f,NOW);
        ingest(&mut f.store,&[event(100,1001,"not-yet-folded",false,NOW-1)],NOW);
        if paused{f.store.conn.execute("UPDATE sources SET manual_paused=1 WHERE source_id=?1",[f.sources[0].get()]).unwrap();}
        let original=runtime(&f,0);
        downgrade(&mut f);
        let identity:String=f.store.conn.query_row("SELECT catalog_instance_id FROM catalog_meta",[],|r|r.get(0)).unwrap();
        let before=legacy_image(&f.store.conn);
        let mut f=reopen(f);
        prop_assert_eq!(legacy_image(&f.store.conn),before.clone());
        prop_assert_eq!(runtime(&f,0),original.clone());
        prop_assert_eq!(queue(&f.store),0);
        prop_assert_eq!(f.store.snapshot().unwrap().schema_version,1);
        prop_assert_eq!(f.store.conn.query_row("SELECT catalog_instance_id FROM catalog_meta",[],|r|r.get::<_,String>(0)).unwrap(),identity);
        let files=fs::read_dir(f.dir.path()).unwrap().map(|p|p.unwrap().path()).filter(|p|p.file_name().unwrap().to_string_lossy().contains("catalog-v5-before-v6-")).collect::<Vec<_>>();
        prop_assert_eq!(files.len(),1);
        let backup=Connection::open_with_flags(&files[0],OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        verify_schema(&backup,5).unwrap();prop_assert_eq!(legacy_image(&backup),before);
        round(&mut f,NOW);
        prop_assert_eq!(runtime(&f,0),original);
        let stable=image(&f.store.conn);let f=reopen(f);prop_assert_eq!(image(&f.store.conn),stable);
    }
    #[test]
    fn observed_publication_pb2_events_retries_reopens_match_independent_reference(events in prop::collection::vec((0usize..2,0usize..6,0u8..4,any::<bool>()),1..25)) {
        let mut f=fixture();let mut phases=HashMap::<(usize,usize),bool>::new();let mut counts=[0i64;2];let mut latched=[false;2];let mut used=[0i64;2];
        for (index,(src,req,kind,restart)) in events.into_iter().enumerate(){
            let success=kind>=2;let valid=kind!=3;
            let mut row=event(index as i64+1,1001+src as i64,&format!("r{req}"),success,NOW-1);
            if !valid{row["prompt_tokens"]=json!(0);row["completion_tokens"]=json!(0);}
            ingest(&mut f.store,&[row.clone(),row.clone()],NOW);
            if success&&valid{used[src]+=8;}
            if valid{match phases.get(&(src,req)){
                None=>{if success{counts[src]=0;}else{counts[src]+=1;latched[src]|=counts[src]>=3;}phases.insert((src,req),success);},
                Some(false) if success=>{counts[src]=0;phases.insert((src,req),true);},
                _=>{}
            }}
            if restart{f=reopen(f);}
            round(&mut f,NOW);
            for i in 0..2{let r=runtime(&f,i);prop_assert_eq!(r.consecutive_failures,counts[i]);prop_assert_eq!(r.error_exhausted,latched[i]);prop_assert_eq!(r.used_input+r.used_output,used[i]);}
            let stable=image(&f.store.conn);round(&mut f,NOW);prop_assert_eq!(image(&f.store.conn),stable);
        }
    }
    #[test]
    fn observed_publication_pb3_fault_rollback_then_replay_is_once(fault in 0usize..5,n in 3i64..9,success in any::<bool>()) {
        let mut f=fixture();
        ingest(&mut f.store,&(1..=n).map(|i|event(i,1001,&format!("f{i}"),false,NOW-1)).collect::<Vec<_>>(),NOW);
        let tables=[("volcano_request_observations","INSERT"),("volcano_observation_queue","DELETE"),("volcano_runtime","UPDATE"),("group_policy_state","UPDATE"),("member_control","UPDATE")];
        let (table,verb)=tables[fault];let before=image(&f.store.conn);
        f.store.conn.execute_batch(&format!("CREATE TRIGGER fault BEFORE {verb} ON {table} BEGIN SELECT RAISE(ABORT,'synthetic'); END;")).unwrap();
        prop_assert!(f.store.commit_volcano_round(&f.cfg,backend(NOW),&HashMap::new(),NOW).is_err());
        prop_assert_eq!(image(&f.store.conn),before);
        f.store.conn.execute_batch("DROP TRIGGER fault").unwrap();f=reopen(f);round(&mut f,NOW);
        prop_assert_eq!(runtime(&f,0).consecutive_failures,n);
        if success{ingest(&mut f.store,&[event(100,1001,"f1",true,NOW)],NOW+1);round(&mut f,NOW+1);prop_assert_eq!(runtime(&f,0).consecutive_failures,0);}
        prop_assert!(runtime(&f,0).error_exhausted);prop_assert_eq!(queue(&f.store),0);
    }
}

#[test]
fn observed_publication_v6_migration_collision_rolls_back_and_preserves_v5() {
    for table in [
        "volcano_observation_queue",
        "volcano_request_observations",
        "volcano_period_history",
    ] {
        let mut f = fixture();
        downgrade(&mut f);
        let before = legacy_image(&f.store.conn);
        f.store
            .conn
            .execute_batch(&format!(
                "CREATE TABLE {table}(marker TEXT);INSERT INTO {table} VALUES('fixture');"
            ))
            .unwrap();
        let path = f.dir.path().join("catalog.sqlite3");
        drop(f.store);
        assert!(CatalogStore::open(&path).is_err());
        let conn = Connection::open(&path).unwrap();
        verify_schema(&conn, 5).unwrap();
        assert_eq!(legacy_image(&conn), before);
        assert_eq!(
            conn.query_row(&format!("SELECT marker FROM {table}"), [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "fixture"
        );
    }
}

#[test]
fn observed_publication_v6_corruption_is_rejected_without_repair() {
    for fault in ["UPDATE volcano_observation_queue SET source_id=(SELECT max(source_id) FROM sources)","PRAGMA ignore_check_constraints=ON;UPDATE volcano_request_observations SET phase='invented'","DROP INDEX ledger_fact_source_request","UPDATE sqlite_sequence SET seq=9007199254740992 WHERE name='volcano_observation_queue'","INSERT INTO volcano_period_history(source_id,period_id,old_start,old_next,new_start,new_next,reset_minute,utc_offset_minutes) SELECT source_id,period_id,0,1,1,2,60,480 FROM volcano_runtime LIMIT 1"] {
        let mut f=fixture();ingest(&mut f.store,&[event(1,1001,"first",false,NOW-1)],NOW);round(&mut f,NOW);
        ingest(&mut f.store,&[event(2,1001,"second",false,NOW-1)],NOW);
        f.store.conn.execute_batch(fault).unwrap();let before=image(&f.store.conn);let path=f.dir.path().join("catalog.sqlite3");drop(f.store);
        assert!(CatalogStore::open(&path).is_err(),"{fault}");let conn=Connection::open(&path).unwrap();assert_eq!(image(&conn),before);
    }
}
