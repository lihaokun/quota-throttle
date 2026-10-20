use super::*;

#[test]
fn restart_scan_preserves_facts_and_rejects_old_foreign_or_finished_tickets() {
    let (_dir, mut store) = setup();
    let first = begin(&mut store);
    store
        .commit_fact_page(&first, &[row(7)], PageEnd::More, 400)
        .unwrap();
    let history = store
        .begin_scan(ScanLane::History, 0, ScanWindow { from: 10, to: 300 }, 400)
        .unwrap();
    store
        .commit_fact_page(&history, &[], PageEnd::More, 400)
        .unwrap();
    let old_state = store.load_ledger_state().unwrap();
    let t = store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
    let before = image(&store.conn);
    error_is(store.restart_scan(&t, 399), LedgerError::InvalidCheckpoint);
    assert_eq!(image(&store.conn), before);
    let (_other, mut other) = setup();
    let foreign = begin(&mut other);
    error_is(
        store.restart_scan(&foreign, 400),
        LedgerError::RevisionConflict,
    );
    error_is(
        store.restart_scan(&first, 400),
        LedgerError::RevisionConflict,
    );
    let next = store.restart_scan(&t, 400).unwrap();
    let after = image(&store.conn);
    let new_state = store.load_ledger_state().unwrap();
    let mut expected = old_state.clone();
    expected.revision += 1;
    expected.scans[0].generation += 1;
    expected.scans[0].next_page = 1;
    expected.scans[0].last_observed_at = None;
    expected.scans[0].fault = None;
    assert_eq!(new_state, expected);
    assert_eq!(
        new_state.initial_requested_from,
        old_state.initial_requested_from
    );
    assert_eq!(new_state.revision, old_state.revision + 1);
    assert_eq!(new_state.scans[1], old_state.scans[1]);
    assert_eq!(&after[2..], &before[2..]);
    assert_eq!(next.progress().generation, t.progress().generation + 1);
    assert_eq!(next.progress().next_page, 1);
    assert_eq!(next.progress().window, t.progress().window);
    assert_eq!(next.progress().last_observed_at, None);
    error_is(
        store.commit_fact_page(&t, &[row(8)], PageEnd::More, 400),
        LedgerError::RevisionConflict,
    );
    let replay = store
        .commit_fact_page(&next, &[row(7)], PageEnd::ReachedEnd, 400)
        .unwrap();
    assert_eq!((replay.inserted, replay.duplicates), (0, 1));
    error_is(
        store.restart_scan(&next, 400),
        LedgerError::RevisionConflict,
    );
    assert_eq!(total(&store.conn), (5, 3, 1));
}
#[test]
fn restart_scan_failure_and_unknown_commit_are_atomic() {
    for mode in 0..4 {
        let (_dir, mut store) = setup();
        let t = begin(&mut store);
        let before = image(&store.conn);
        match mode {
            0=>store.conn.execute_batch("CREATE TRIGGER fault BEFORE UPDATE ON ledger_checkpoints BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap(),
            1=>store.conn.execute_batch("CREATE TRIGGER fault BEFORE UPDATE ON ledger_state BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap(),
            2=>store.conn.execute_batch("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fault AFTER UPDATE ON ledger_checkpoints BEGIN INSERT INTO child VALUES(1); END;").unwrap(),
            _=>{store.conn.execute("UPDATE ledger_state SET revision=?1",[JSON_SAFE]).unwrap();},
        }
        let snapshot = if mode == 3 {
            image(&store.conn)
        } else {
            before
        };
        assert!(store.restart_scan(&t, 300).is_err());
        assert_eq!(image(&store.conn), snapshot);
        assert_eq!(store.poisoned, mode == 2);
    }
    let (_dir, mut store) = setup();
    begin(&mut store);
    store
        .conn
        .execute(
            "UPDATE ledger_checkpoints SET generation=?1 WHERE lane='recent'",
            [JSON_SAFE],
        )
        .unwrap();
    let t = store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
    let before = image(&store.conn);
    error_is(store.restart_scan(&t, 300), LedgerError::InvalidCheckpoint);
    assert_eq!(image(&store.conn), before);
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb3_restart_reopen_keeps_atomic_deduplicated_facts(actions in prop::collection::vec((any::<bool>(),any::<bool>(),1i64..15),1..30)) {
        let (dir,mut store)=setup();begin(&mut store);let mut generation=1;let mut page=1;let mut seen=std::collections::BTreeSet::new();let mut revision=3;
        for (restart,reopen,id) in actions {
            if reopen {drop(store);store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();}
            let t=store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
            if restart {let old=image(&store.conn);store.restart_scan(&t,400).unwrap();generation+=1;page=1;revision+=1;prop_assert_eq!(&image(&store.conn)[2..],&old[2..]);prop_assert_eq!(store.restart_scan(&t,400).err(),Some(LedgerError::RevisionConflict));}
            else {store.commit_fact_page(&t,&[row(id)],PageEnd::More,400).unwrap();seen.insert(id);page+=1;revision+=1;}
            let state=store.load_ledger_state().unwrap();prop_assert_eq!(state.revision,revision);prop_assert_eq!((state.scans[0].generation,state.scans[0].next_page),(generation,page));
            let n=seen.len() as i64;prop_assert_eq!(total(&store.conn),(n*5,n*3,n));
        }
    }
}

fn maximum_collection_page() -> Duration {
    use crate::newapi::logs::{decode_log_page, LogPageQuery};
    let (_dir, mut store, _, _) = mapped(10000);
    let t = begin(&mut store);
    let rows: Vec<_> = (1..=100)
        .rev()
        .map(|id| {
            let mut r = row(id);
            r["channel"] = json!(1000 + id);
            r["group"] = json!("ledger-g");
            r["model_name"] = json!(format!("model-{id}"));
            r
        })
        .collect();
    let query = LogPageQuery::new(ScanWindow { from: 0, to: 300 }, 1).unwrap();
    let body = json!({"success":true,"data":{"page":1,"page_size":100,"total":100,"items":rows}});
    let start = Instant::now();
    let page = decode_log_page(body, &query).unwrap();
    let committed = store
        .commit_fact_page(&t, &page.rows, page.end, 300)
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(committed.inserted, 100);
    assert_eq!(committed.progress.next_page, 2);
    assert_eq!(total(&store.conn), (500, 300, 100));
    elapsed
}
#[test]
fn collection_maximum_page_capacity() {
    maximum_collection_page();
}
#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_collection_page() {
    let t = maximum_collection_page();
    println!("COLLECTION_PAGE_MS={}", t.as_millis());
    assert!(t < Duration::from_secs(2));
}
#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_collection_restart_100k() {
    let (_dir, mut store) = setup();
    let t = begin(&mut store);
    store.conn.execute_batch("BEGIN; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO ledger_facts(native_log_id,kind,occurred_at,input,output,is_stream,metadata_json,attribution_issue) SELECT x,'consume',x*60,5,3,0,'{\"version\":1,\"state\":\"absent\"}','unmapped_channel' FROM n; INSERT INTO ledger_minute_usage(minute,input,output,consumption_rows) SELECT occurred_at,5,3,1 FROM ledger_facts; COMMIT;").unwrap();
    let start = Instant::now();
    let next = store.restart_scan(&t, 300).unwrap();
    let state = store.load_ledger_state().unwrap();
    let elapsed = start.elapsed();
    assert_eq!(state.scans[0].generation, 2);
    assert_eq!(next.progress().next_page, 1);
    assert_eq!(total(&store.conn), (500000, 300000, 100000));
    println!("COLLECTION_RESTART_100K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(1));
}
