use super::*;
use crate::ledger::analytics::*;
#[path = "ledger_query_scale.rs"]
mod scale;

fn control() -> QueryControl {
    QueryControl::new(Instant::now() + Duration::from_secs(3600))
}
fn query(from: i64, to: i64, token: DimensionFilter<i64>) -> UsageQuery {
    UsageQuery::new(
        ScanWindow { from, to },
        480,
        UsageGrain::Hour,
        UsageFilter {
            token,
            ..Default::default()
        },
    )
    .unwrap()
}
fn read(store: &CatalogStore, q: UsageQuery) -> UsageSnapshot {
    store
        .usage_read_target()
        .unwrap()
        .query(q, &control())
        .unwrap()
}
fn add(store: &mut CatalogStore, rs: &[Value]) {
    let t = store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
    store.commit_fact_page(&t, rs, PageEnd::More, 300).unwrap();
}

#[test]
fn query_snapshot_mixes_exact_edges_full_minutes_and_all_token_identities() {
    let (_dir, mut store) = setup();
    begin(&mut store);
    let mut rows = Vec::new();
    for (i, at) in [0, 59, 60, 61, 119, 120, 121, 179, 180]
        .into_iter()
        .enumerate()
    {
        let mut r = row(i as i64 + 1);
        r["created_at"] = json!(at);
        r["token_id"] = json!(if i % 2 == 0 { i64::MAX } else { 0 });
        r["model_name"] = json!(if i % 3 == 0 { Some("未知") } else { None });
        rows.push(r);
    }
    let mut e = row(99);
    e["type"] = json!(5);
    e["prompt_tokens"] = json!(0);
    e["completion_tokens"] = json!(0);
    e["created_at"] = json!(120);
    rows.push(e);
    add(&mut store, &rows);
    for (from, to) in [(0, 1), (59, 61), (1, 180), (60, 120), (120, 121), (0, 300)] {
        for token in [
            DimensionFilter::Any,
            DimensionFilter::Unknown,
            DimensionFilter::Exact(i64::MAX),
            DimensionFilter::Exact(123),
        ] {
            let n = rows
                .iter()
                .filter(|r| {
                    r["type"] == 2
                        && r["created_at"]
                            .as_i64()
                            .is_some_and(|t| from <= t && t < to)
                        && match token {
                            DimensionFilter::Any => true,
                            DimensionFilter::Unknown => r["token_id"] == 0,
                            DimensionFilter::Exact(id) => r["token_id"] == id,
                        }
                })
                .count() as i64;
            let snap = read(&store, query(from, to, token));
            assert_eq!(snap.state, store.load_ledger_state().unwrap());
            let r = snap.usage.unwrap();
            assert_eq!(
                r.total,
                crate::ledger::UsageTotals {
                    input: n * 5,
                    output: n * 3,
                    consumption_rows: n
                }
            );
            assert_eq!(r.models.iter().map(|(_, v)| v.input).sum::<i64>(), n * 5);
            assert_eq!(r.tokens.iter().map(|(_, v)| v.input).sum::<i64>(), n * 5);
        }
    }
}
#[test]
fn query_read_transaction_does_not_block_writer_or_mix_revisions() {
    let (_dir, mut store) = setup();
    begin(&mut store);
    add(&mut store, &[row(1)]);
    let old = store.load_ledger_state().unwrap();
    let target = store.usage_read_target().unwrap();
    let mut called = false;
    let snap = target
        .test_query(
            query(0, 300, DimensionFilter::Any),
            &control(),
            MAX_QUERY_ROWS,
            || {
                if !called {
                    called = true;
                    add(&mut store, &[row(2)]);
                    assert!(store.load_ledger_state().unwrap().revision > old.revision);
                }
            },
        )
        .unwrap();
    assert!(called);
    assert_eq!(snap.state, old);
    assert_eq!(snap.usage.unwrap().total.consumption_rows, 1);
    assert_eq!(
        read(&store, query(0, 300, DimensionFilter::Any))
            .usage
            .unwrap()
            .total
            .consumption_rows,
        2
    );
}
#[test]
fn query_guard_outlives_store_without_reviving_duplicate_file_ownership() {
    let (dir, mut store) = setup();
    begin(&mut store);
    add(&mut store, &[row(1)]);
    let target = store.usage_read_target().unwrap();
    let duplicate = store._lock.0.try_clone().unwrap();
    drop(store);
    assert_eq!(
        CatalogStore::open(&dir.path().join("catalog.sqlite3"))
            .err()
            .unwrap()
            .kind,
        ErrorKind::AlreadyOpen
    );
    assert_eq!(
        target
            .query(query(0, 300, DimensionFilter::Any), &control())
            .unwrap()
            .usage
            .unwrap()
            .total
            .consumption_rows,
        1
    );
    let reopened = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    drop(duplicate);
    assert_eq!(
        CatalogStore::open(&dir.path().join("catalog.sqlite3"))
            .err()
            .unwrap()
            .kind,
        ErrorKind::AlreadyOpen
    );
    drop(reopened);
}
#[test]
fn query_budget_is_pre_filter_exact_and_cancelled_pages_never_publish() {
    let (_dir, mut store) = setup();
    begin(&mut store);
    let rs: Vec<_> = (1..=600)
        .map(|i| {
            let mut r = row(i);
            r["token_id"] = json!(i);
            r
        })
        .collect();
    add(&mut store, &rs);
    let c = control();
    let q = query(0, 300, DimensionFilter::Any);
    assert_eq!(
        store
            .usage_read_target()
            .unwrap()
            .test_query(q.clone(), &c, 600, || {})
            .unwrap()
            .usage
            .unwrap()
            .total
            .consumption_rows,
        600
    );
    assert_eq!(
        store
            .usage_read_target()
            .unwrap()
            .test_query(q, &c, 599, || {})
            .err(),
        Some(AnalyticsError::LimitExceeded)
    );
    let q = UsageQuery::new(
        ScanWindow { from: 0, to: 300 },
        480,
        UsageGrain::Hour,
        UsageFilter {
            model: DimensionFilter::Exact("nonexistent".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        store
            .usage_read_target()
            .unwrap()
            .test_query(q, &c, 599, || {})
            .err(),
        Some(AnalyticsError::LimitExceeded)
    );
    assert_eq!(
        store
            .usage_read_target()
            .unwrap()
            .test_query(query(0, 300, DimensionFilter::Exact(123)), &c, 1, || {})
            .unwrap()
            .usage
            .unwrap()
            .total
            .consumption_rows,
        1
    );
    let c = control();
    let before = image(&store.conn);
    let mut steps = 0;
    let e = store
        .usage_read_target()
        .unwrap()
        .test_query(query(0, 300, DimensionFilter::Any), &c, 600, || {
            steps += 1;
            if steps == 2 {
                c.cancel();
            }
        })
        .err();
    assert_eq!(e, Some(AnalyticsError::Cancelled));
    assert_eq!(image(&store.conn), before);
}
#[test]
fn query_uninitialized_corrupt_and_changed_identity_are_explicit() {
    let (_dir, mut store) = setup();
    assert!(read(&store, query(0, 1, DimensionFilter::Any))
        .usage
        .is_none());
    begin(&mut store);
    add(&mut store, &[row(1)]);
    let target = store.usage_read_target().unwrap();
    store
        .conn
        .execute(
            "UPDATE catalog_meta SET catalog_instance_id='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
            [],
        )
        .unwrap();
    assert_eq!(
        target
            .query(query(0, 300, DimensionFilter::Any), &control())
            .err(),
        Some(AnalyticsError::Ledger(LedgerError::Catalog(error(
            ErrorKind::CorruptData
        ))))
    );
    let target = store.usage_read_target().unwrap();
    store.conn.pragma_update(None, "user_version", 99).unwrap();
    assert_eq!(
        target
            .query(query(0, 300, DimensionFilter::Any), &control())
            .err(),
        Some(AnalyticsError::Ledger(LedgerError::Catalog(error(
            ErrorKind::UnsupportedSchema
        ))))
    );
    store.conn.pragma_update(None, "user_version", 5).unwrap();
    store
        .conn
        .execute("DELETE FROM ledger_checkpoints WHERE lane='history'", [])
        .unwrap();
    assert_eq!(
        store
            .usage_read_target()
            .unwrap()
            .query(query(0, 300, DimensionFilter::Any), &control())
            .err(),
        Some(AnalyticsError::Ledger(LedgerError::Catalog(error(
            ErrorKind::CorruptData
        ))))
    );
    store.poisoned = true;
    assert_eq!(
        store.usage_read_target().err(),
        Some(AnalyticsError::Ledger(LedgerError::Catalog(error(
            ErrorKind::CommitUnknown
        ))))
    );
}
#[test]
fn query_private_target_and_errors_do_not_expose_paths_or_labels() {
    use crate::catalog::ledger::query::UsageReadTarget;
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(UsageReadTarget, std::fmt::Debug);
    assert_not_impl!(UsageReadTarget, serde::Serialize);
    assert_not_impl!(UsageReadTarget, serde::Deserialize<'static>);
    let (dir, mut store) = setup();
    begin(&mut store);
    let target = store.usage_read_target().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let e = target
        .query(query(0, 300, DimensionFilter::Any), &control())
        .err()
        .unwrap();
    assert_eq!(
        e,
        AnalyticsError::Ledger(LedgerError::Catalog(error(ErrorKind::UnsafePath)))
    );
    assert!(!format!("{e:?}").contains(dir.path().to_str().unwrap()));
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
}
#[test]
fn query_keyset_uses_indexes_without_rescanning_equal_time_prefixes() {
    let (_dir, mut store) = setup();
    begin(&mut store);
    for chunk in (1..=20000).collect::<Vec<_>>().chunks(1000) {
        let rs: Vec<_> = chunk
            .iter()
            .map(|i| {
                let mut r = row(*i);
                r["channel"] = json!(i);
                r["token_id"] = json!(if *i <= 10000 { 1 } else { 0 });
                r
            })
            .collect();
        add(&mut store, &rs);
    }
    for cells in [true, false] {
        for token in [None, Some(None), Some(Some(1i64))] {
            for same_time in [true, false] {
                let sql =
                    crate::catalog::ledger::query::test_page_sql(cells, token.is_some(), same_time);
                let token_value = token.flatten();
                let time = if cells { 120 } else { 125 };
                let cursor = if token == Some(Some(1)) { 9900 } else { 19900 };
                let plan: Vec<String> = store
                    .conn
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .unwrap()
                    .query_map(params![0, 300, time, cursor, token_value, 256], |r| {
                        r.get(3)
                    })
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect();
                assert!(plan.iter().any(|s| s.contains("USING INDEX")));
                assert!(!plan.iter().any(|s| s.contains("TEMP B-TREE")));
                let mut stmt = store.conn.prepare(&sql).unwrap();
                let rs = stmt
                    .query_map(params![0, 300, time, cursor, token_value, 256], |_| Ok(()))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                assert_eq!(rs.len(), if same_time { 100 } else { 0 });
                assert!(
                    stmt.get_status(rusqlite::StatementStatus::VmStep) < 10000,
                    "keyset rescanned prefix: {}",
                    stmt.get_status(rusqlite::StatementStatus::VmStep)
                );
            }
        }
    }
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn query_pbt_sql_edges_filters_and_snapshot_match_raw_facts(values in prop::collection::vec((0i64..300,0i64..5,0i64..100,any::<bool>()),0..100),from in 0i64..299,span in 1i64..300,token in prop::option::of(0i64..5)) {
        let (_dir,mut store)=setup();begin(&mut store);let to=(from+span).min(300);
        let rs:Vec<_>=values.iter().enumerate().map(|(i,(at,k,input,consume))|{let mut r=row(i as i64+1);r["created_at"]=json!(at);r["token_id"]=json!(k);r["prompt_tokens"]=json!(if *consume{*input}else{0});r["completion_tokens"]=json!(0);r["type"]=json!(if *consume{2}else{5});r}).collect();add(&mut store,&rs);
        let f=match token{None=>DimensionFilter::Any,Some(0)=>DimensionFilter::Unknown,Some(k)=>DimensionFilter::Exact(k)};
        let expected:Vec<_>=values.iter().filter(|(at,k,_,consume)|*consume&&from<=*at&&*at<to&&token.is_none_or(|want|want==*k)).collect();let actual=read(&store,query(from,to,f)).usage.unwrap().total;
        prop_assert_eq!(actual.input,expected.iter().map(|v|v.2).sum::<i64>());prop_assert_eq!(actual.output,0);prop_assert_eq!(actual.consumption_rows,expected.len() as i64);
    }
    #[test]
    fn query_pb6_concurrent_writes_never_mix_snapshot_revisions(first in 1usize..30,second in 1usize..30) {
        let (_dir,mut store)=setup();begin(&mut store);add(&mut store,&(1..=first as i64).map(row).collect::<Vec<_>>());let old=store.load_ledger_state().unwrap();let target=store.usage_read_target().unwrap();let mut once=false;
        let snap=target.test_query(query(0,300,DimensionFilter::Any),&control(),MAX_QUERY_ROWS,||{if !once{once=true;add(&mut store,&(first as i64+1..=(first+second) as i64).map(row).collect::<Vec<_>>());}}).unwrap();
        prop_assert_eq!(snap.state,old);prop_assert_eq!(snap.usage.unwrap().total.consumption_rows,first as i64);prop_assert_eq!(read(&store,query(0,300,DimensionFilter::Any)).usage.unwrap().total.consumption_rows,(first+second) as i64);
    }
}
