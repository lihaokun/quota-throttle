use super::*;
use crate::ledger::analytics::*;

fn base() -> (TempDir, CatalogStore, SourceId, GroupId) {
    let (dir, mut s) = setup();
    let source = s
        .create_source(source("fake-late-mapping", SourceKind::Glm))
        .unwrap()
        .source_id;
    let group = s
        .create_group(group("ledger-g", SourceKind::Glm))
        .unwrap()
        .group_id;
    begin(&mut s);
    (dir, s, source, group)
}
fn fact(id: i64, channel: i64) -> Value {
    let mut r = row(id);
    r["channel"] = json!(channel);
    r["group"] = json!("ledger-g");
    r
}
fn ingest(s: &mut CatalogStore, rows: &[Value]) {
    for batch in rows.chunks(MAX_FACTS) {
        let t = s.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
        s.commit_fact_page(&t, batch, PageEnd::More, 300).unwrap();
    }
}
fn history(s: &mut CatalogStore, source: SourceId, group: GroupId, start: i64, count: usize) {
    let tx = s.conn.transaction().unwrap();
    for channel in start..start + count as i64 {
        tx.execute("INSERT INTO memberships(source_id,group_id,kind,channel_id,position,lifecycle,desired_priority,retired_at) VALUES(?1,?2,'glm',?3,0,'retired',10,10)",params![source.get(),group.get(),channel]).unwrap();
    }
    tx.execute("INSERT INTO member_control(membership_id,revision,status_directive,retire_requested,create_state,create_name,dirty,next_attempt_at) SELECT m.membership_id,1,'preserve',0,'resolved','qt-'||(SELECT catalog_instance_id FROM catalog_meta)||'-m'||m.membership_id,0,0 FROM memberships m LEFT JOIN member_control c USING(membership_id) WHERE c.membership_id IS NULL",[]).unwrap();
    tx.execute("INSERT INTO member_execution SELECT m.membership_id,0,0,0,0 FROM memberships m LEFT JOIN member_execution e USING(membership_id) WHERE e.membership_id IS NULL",[]).unwrap();
    tx.commit().unwrap();
}
fn native(s: &CatalogStore) -> Vec<Value> {
    full_facts(&s.conn)
        .into_iter()
        .map(|mut r| {
            for k in ["source", "group_id", "issue"] {
                r.as_object_mut().unwrap().remove(k);
            }
            r
        })
        .collect()
}
fn chart(s: &CatalogStore, source: DimensionFilter<SourceId>) -> UsageSnapshot {
    let q = UsageQuery::new(
        ScanWindow { from: 0, to: 300 },
        480,
        UsageGrain::Hour,
        UsageFilter {
            source,
            ..Default::default()
        },
    )
    .unwrap();
    s.usage_read_target()
        .unwrap()
        .query(
            q,
            &QueryControl::new(Instant::now() + Duration::from_secs(60)),
        )
        .unwrap()
}
fn pass(s: &mut CatalogStore) -> (usize, usize) {
    let mut cursor = Some(s.begin_attribution_pass().unwrap());
    let (mut checked, mut changed) = (0, 0);
    while let Some(c) = cursor {
        let r = s.reattribute_batch(&c).unwrap();
        checked += r.checked;
        changed += r.changed;
        cursor = r.next;
    }
    (checked, changed)
}

#[test]
fn attribution_moves_unknown_partitions_without_changing_native_facts_or_totals() {
    let (_dir, mut s, source, g) = base();
    let mut rs = vec![fact(1, 1001), fact(2, 1001), fact(3, 1001), fact(4, 0)];
    rs[1]["prompt_tokens"] = json!(0);
    rs[1]["completion_tokens"] = json!(0);
    rs[2]["type"] = json!(5);
    rs[2]["prompt_tokens"] = json!(0);
    rs[2]["completion_tokens"] = json!(0);
    rs.push(fact(5, 1001));
    rs[4]["group"] = json!("other");
    ingest(&mut s, &rs);
    let raw = native(&s);
    let totals = total(&s.conn);
    let before = s.load_ledger_state().unwrap();
    let before_chart = chart(&s, DimensionFilter::Any).usage;
    history(&mut s, source, g, 1001, 1);
    s.conn
        .execute(
            "UPDATE sources SET name='renamed' WHERE source_id=?1",
            [source.get()],
        )
        .unwrap();
    let cursor = s.begin_attribution_pass().unwrap();
    let result = s.reattribute_batch(&cursor).unwrap();
    assert_eq!((result.checked, result.changed), (5, 4));
    assert!(result.next.is_none());
    assert_eq!(result.ledger_revision, before.revision + 1);
    assert_eq!(native(&s), raw);
    assert_eq!(total(&s.conn), totals);
    let after_chart = chart(&s, DimensionFilter::Any).usage;
    let (b, a) = (before_chart.as_ref().unwrap(), after_chart.as_ref().unwrap());
    assert!(a.total == b.total && a.models == b.models && a.tokens == b.tokens && a.buckets == b.buckets);
    // 重新归属只把「未归组」挪进真实组：按组分项随之变化，总量守恒（finish_parts 校验边际和）。
    assert_eq!(b.groups.iter().map(|(group, _)| *group).collect::<Vec<_>>(), vec![None]);
    assert!(a.groups.iter().any(|(group, _)| *group == Some(g)));
    assert_eq!(s.load_ledger_state().unwrap().scans, before.scans);
    let facts = full_facts(&s.conn);
    assert_eq!(facts[0]["source"], source.get());
    assert_eq!(facts[0]["group_id"], g.get());
    assert_eq!(facts[2]["source"], source.get());
    assert_eq!(facts[3]["source"], Value::Null);
    assert_eq!(facts[4]["issue"], "group_unresolved");
    assert_eq!(
        chart(&s, DimensionFilter::Exact(source))
            .usage
            .unwrap()
            .total
            .consumption_rows,
        3
    );
    let after = image(&s.conn);
    assert_eq!(s.reattribute_batch(&cursor).unwrap().changed, 0);
    assert_eq!(image(&s.conn), after);
    ingest(&mut s, &rs);
    assert_eq!(&image(&s.conn)[2..], &after[2..]);
}

#[test]
fn attribution_fixed_pass_replay_reopen_and_late_mapping_do_not_skip_unknown_history() {
    let (dir, mut s, source, g) = base();
    let rs: Vec<_> = (1..=601).map(|id| fact(id * 2, 1001)).collect();
    ingest(&mut s, &rs);
    let cursor = s.begin_attribution_pass().unwrap();
    let first = s.reattribute_batch(&cursor).unwrap();
    assert_eq!((first.checked, first.changed), (256, 0));
    let mut next = first.next;
    ingest(&mut s, &[fact(1, 1001), fact(i64::MAX, 1001)]);
    history(&mut s, source, g, 1001, 1);
    let mut checked = 256;
    while let Some(c) = next {
        let r = s.reattribute_batch(&c).unwrap();
        checked += r.checked;
        next = r.next;
    }
    assert_eq!(checked, 601);
    assert_eq!(
        chart(&s, DimensionFilter::Unknown)
            .usage
            .unwrap()
            .total
            .consumption_rows,
        258
    );
    drop(s);
    let mut s = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(pass(&mut s), (603, 258));
    assert_eq!(pass(&mut s), (603, 0));
    assert_eq!(
        chart(&s, DimensionFilter::Exact(source))
            .usage
            .unwrap()
            .total
            .consumption_rows,
        603
    );
}

#[test]
fn attribution_empty_foreign_and_uninitialized_cursors_are_explicit() {
    let (_dir, mut s) = setup();
    assert_eq!(
        s.begin_attribution_pass().err(),
        Some(LedgerError::NotInitialized)
    );
    begin(&mut s);
    let before = image(&s.conn);
    let c = s.begin_attribution_pass().unwrap();
    let r = s.reattribute_batch(&c).unwrap();
    assert_eq!((r.checked, r.changed), (0, 0));
    assert!(r.next.is_none());
    assert_eq!(image(&s.conn), before);
    let (_other, mut other) = setup();
    begin(&mut other);
    let foreign = other.begin_attribution_pass().unwrap();
    assert_eq!(
        s.reattribute_batch(&foreign).err(),
        Some(LedgerError::RevisionConflict)
    );
    assert_eq!(image(&s.conn), before);
}

#[test]
fn attribution_sql_failure_and_late_overflow_roll_back_the_entire_batch() {
    for fault in ["CREATE TRIGGER fault BEFORE UPDATE ON ledger_facts WHEN NEW.native_log_id=2 BEGIN SELECT RAISE(FAIL,'private-sentinel'); END;","CREATE TRIGGER fault BEFORE INSERT ON ledger_minute_usage WHEN NEW.source_id IS NOT NULL AND NEW.model='b' BEGIN SELECT RAISE(FAIL,'private-sentinel'); END;"] {
        let (_dir,mut s,source,g)=base();let a=fact(1,1001);let mut b=fact(2,1001);b["model_name"]=json!("b");ingest(&mut s,&[a,b]);history(&mut s,source,g,1001,1);let c=s.begin_attribution_pass().unwrap();let before=image(&s.conn);s.conn.execute_batch(fault).unwrap();
        let e=s.reattribute_batch(&c).err().unwrap();assert!(!format!("{e}").contains("sentinel"));assert_eq!(image(&s.conn),before);assert!(s.usable().is_ok());s.conn.execute_batch("DROP TRIGGER fault").unwrap();assert_eq!(s.reattribute_batch(&c).unwrap().changed,2);
    }
    let (_dir, mut s, source, g) = base();
    let a = fact(1, 1001);
    let mut b = fact(2, 1001);
    b["model_name"] = json!("b");
    b["prompt_tokens"] = json!(2);
    b["completion_tokens"] = json!(0);
    ingest(&mut s, &[a, b]);
    history(&mut s, source, g, 1001, 1);
    let mut big = fact(3, 1001);
    big["model_name"] = json!("b");
    big["prompt_tokens"] = json!(JSON_SAFE - 1);
    big["completion_tokens"] = json!(0);
    ingest(&mut s, &[big]);
    let c = s.begin_attribution_pass().unwrap();
    let before = image(&s.conn);
    assert_eq!(
        s.reattribute_batch(&c).err(),
        Some(LedgerError::Fact(FactError::ArithmeticOverflow))
    );
    assert_eq!(image(&s.conn), before);
}

#[test]
fn attribution_unknown_commit_poisoning_requires_reopen_before_retry() {
    let (dir, mut s, source, g) = base();
    ingest(&mut s, &[fact(1, 1001)]);
    history(&mut s, source, g, 1001, 1);
    let c = s.begin_attribution_pass().unwrap();
    let before = image(&s.conn);
    s.conn.execute_batch("CREATE TABLE test_parent(id INTEGER PRIMARY KEY); CREATE TABLE test_child(id INTEGER REFERENCES test_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fault AFTER UPDATE ON ledger_facts BEGIN INSERT INTO test_child VALUES(1); END;").unwrap();
    assert!(
        matches!(s.reattribute_batch(&c),Err(LedgerError::Catalog(e)) if e.kind==ErrorKind::CommitUnknown)
    );
    assert!(s.poisoned);
    assert_eq!(image(&s.conn), before);
    assert!(s.begin_attribution_pass().is_err());
    s.conn
        .execute_batch("DROP TRIGGER fault; DROP TABLE test_child; DROP TABLE test_parent;")
        .unwrap();
    drop(s);
    let mut s = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(pass(&mut s), (1, 1));
    assert_eq!(pass(&mut s), (1, 0));
}

#[test]
fn attribution_corrupt_balances_and_rebinding_known_identity_are_rejected() {
    let (_dir, mut s, source, g) = base();
    ingest(&mut s, &[fact(1, 1001)]);
    history(&mut s, source, g, 1001, 1);
    s.conn
        .execute("UPDATE ledger_minute_usage SET input=4", [])
        .unwrap();
    let before = image(&s.conn);
    let c = s.begin_attribution_pass().unwrap();
    assert!(
        matches!(s.reattribute_batch(&c),Err(LedgerError::Catalog(e)) if e.kind==ErrorKind::CorruptData)
    );
    assert_eq!(image(&s.conn), before);
    s.conn
        .execute("UPDATE ledger_minute_usage SET input=5", [])
        .unwrap();
    assert_eq!(s.reattribute_batch(&c).unwrap().changed, 1);
    let second = s
        .create_source(super::super::source("fake-second-source", SourceKind::Glm))
        .unwrap()
        .source_id;
    s.conn
        .execute(
            "UPDATE memberships SET source_id=?1 WHERE channel_id=1001",
            [second.get()],
        )
        .unwrap();
    let before = image(&s.conn);
    assert!(
        matches!(s.reattribute_batch(&c),Err(LedgerError::Catalog(e)) if e.kind==ErrorKind::CorruptData)
    );
    assert_eq!(image(&s.conn), before);
}

#[test]
fn attribution_readers_observe_complete_before_or_after_partitions() {
    let (_dir, mut s, source, g) = base();
    ingest(&mut s, &[fact(1, 1001), fact(2, 1001)]);
    history(&mut s, source, g, 1001, 1);
    let c = s.begin_attribution_pass().unwrap();
    let target = s.usage_read_target().unwrap();
    let old = s.load_ledger_state().unwrap();
    let mut once = false;
    let q = UsageQuery::new(
        ScanWindow { from: 0, to: 300 },
        480,
        UsageGrain::Hour,
        UsageFilter {
            source: DimensionFilter::Unknown,
            ..Default::default()
        },
    )
    .unwrap();
    let result = target
        .test_query(
            q,
            &QueryControl::new(Instant::now() + Duration::from_secs(60)),
            MAX_QUERY_ROWS,
            || {
                if !once {
                    once = true;
                    assert_eq!(s.reattribute_batch(&c).unwrap().changed, 2);
                }
            },
        )
        .unwrap();
    assert_eq!(result.state, old);
    assert_eq!(result.usage.unwrap().total.consumption_rows, 2);
    assert_eq!(
        chart(&s, DimensionFilter::Unknown)
            .usage
            .unwrap()
            .total
            .consumption_rows,
        0
    );
}

#[test]
fn attribution_private_cursor_and_batch_have_no_wire_or_logging_traits() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(AttributionCursor, std::fmt::Debug);
    assert_not_impl!(AttributionCursor, serde::Serialize);
    assert_not_impl!(AttributionCursor, serde::Deserialize<'static>);
    assert_not_impl!(AttributionBatch, std::fmt::Debug);
    assert_not_impl!(AttributionBatch, serde::Serialize);
}

#[test]
fn attribution_max_id_and_deep_pages_seek_the_primary_key() {
    let (_dir, mut s, _, _) = base();
    ingest(
        &mut s,
        &(1..=20000).map(|i| fact(i, 1001)).collect::<Vec<_>>(),
    );
    let (max_sql, page_sql) = attribution_test_sql();
    let mut max = s.conn.prepare(max_sql).unwrap();
    assert_eq!(max.query_row([], |r| r.get::<_, i64>(0)).unwrap(), 20000);
    assert!(max.get_status(rusqlite::StatementStatus::VmStep) < 50);
    for after in [0, 19000] {
        let plan: Vec<String> = s
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {page_sql}"))
            .unwrap()
            .query_map(params![after, 20000, 256], |r| r.get(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(plan
            .iter()
            .any(|s| s.contains("SEARCH ledger_facts USING INTEGER PRIMARY KEY")));
        assert!(!plan.iter().any(|s| s.contains("TEMP B-TREE")));
        let mut page = s.conn.prepare(&page_sql).unwrap();
        let ids: Vec<i64> = page
            .query_map(params![after, 20000, 256], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ids, (after + 1..=after + 256).collect::<Vec<_>>());
        assert!(page.get_status(rusqlite::StatementStatus::VmStep) < 10000);
    }
}

#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_attribution_batch_10k_history() {
    let (_dir, mut s, source, g) = base();
    ingest(
        &mut s,
        &(0..ATTRIBUTION_BATCH_ROWS as i64)
            .map(|i| fact(i + 1, 1001 + i))
            .collect::<Vec<_>>(),
    );
    history(&mut s, source, g, 1001, 10000);
    let c = s.begin_attribution_pass().unwrap();
    let start = Instant::now();
    let batch = s.reattribute_batch(&c).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(batch.checked, 256);
    assert_eq!(batch.changed, 256);
    eprintln!("ATTRIBUTION_BATCH_10K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(1));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn attribution_pb3_history_identity_and_counts_follow_independent_reference(values in prop::collection::vec((0u8..4,0u8..3,0i64..30,any::<bool>()),1..100)) {
        let (_dir,mut s,source,g)=base();let g2=s.create_group(group("ledger-g2",SourceKind::Glm)).unwrap().group_id;
        let rows:Vec<_>=values.iter().enumerate().map(|(i,(ch,gr,input,consume))|{let mut r=fact(i as i64+1,if *ch==0{0}else{1000+i64::from(*ch)});r["group"]=json!(match gr {0=>None,1=>Some("ledger-g"),_=>Some("ledger-g2")});r["type"]=json!(if *consume{2}else{5});r["prompt_tokens"]=json!(if *consume{*input}else{0});r["completion_tokens"]=json!(0);r}).collect();ingest(&mut s,&rows);let original=native(&s);
        history(&mut s,source,g,1001,1);history(&mut s,source,g2,1002,1);let result=pass(&mut s);prop_assert_eq!(result.0,values.len());prop_assert_eq!(result.1,values.iter().filter(|v|v.0==1||v.0==2).count());
        for (r,(ch,gr,_,_)) in full_facts(&s.conn).iter().zip(values.iter()) {
            let known=*ch==1||*ch==2;prop_assert_eq!(&r["source"],&if known {json!(source.get())}else{Value::Null});let group=if *ch==1&&*gr==1{Some(g)}else if *ch==2&&*gr==2{Some(g2)}else{None};prop_assert_eq!(&r["group_id"],&json!(group.map(|v|v.get())));
        }
        prop_assert_eq!(native(&s),original);prop_assert_eq!(total(&s.conn),(values.iter().filter(|v|v.3).map(|v|v.2).sum::<i64>(),0,values.iter().filter(|v|v.3).count() as i64));prop_assert_eq!(pass(&mut s).1,0);
    }
    #[test]
    fn attribution_pb4_failure_replay_and_reopen_are_atomic(count in 2usize..50,fail_at in 1usize..50,inject in any::<bool>()) {
        let (dir,mut s,source,g)=base();ingest(&mut s,&(1..=count as i64).map(|i|fact(i,1001)).collect::<Vec<_>>());history(&mut s,source,g,1001,1);let c=s.begin_attribution_pass().unwrap();let before=image(&s.conn);let old=s.load_ledger_state().unwrap();
        if inject {s.conn.execute_batch(&format!("CREATE TRIGGER fault BEFORE UPDATE ON ledger_facts WHEN NEW.native_log_id={} BEGIN SELECT RAISE(FAIL,'synthetic'); END;",1+fail_at%count)).unwrap();prop_assert!(s.reattribute_batch(&c).is_err());prop_assert_eq!(image(&s.conn),before);s.conn.execute_batch("DROP TRIGGER fault").unwrap();}
        prop_assert_eq!(s.reattribute_batch(&c).unwrap().changed,count);prop_assert_eq!(s.load_ledger_state().unwrap().revision,old.revision+1);let after=image(&s.conn);prop_assert_eq!(s.reattribute_batch(&c).unwrap().changed,0);prop_assert_eq!(image(&s.conn),after.clone());drop(s);
        let mut s=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();prop_assert_eq!(image(&s.conn),after);prop_assert_eq!(pass(&mut s).1,0);
    }
    #[test]
    fn attribution_pb5_late_mapping_restarts_and_frozen_high_water_progress(count in 257usize..650,events in prop::collection::vec((0u8..4, any::<bool>(), any::<bool>()),1..12)) {
        let (dir,mut s,source,g)=base();
        let channel=|id:i64|1001+id%3;
        ingest(&mut s,&(1..=count as i64).map(|i|fact(i*2,channel(i*2))).collect::<Vec<_>>());
        let mut next=Some(s.begin_attribution_pass().unwrap());
        let mut mapped=[false;3];let mut added=0usize;
        for (step,(arrive,reopen,late)) in events.iter().enumerate() {
            if *arrive<3&&!mapped[*arrive as usize] {history(&mut s,source,g,1001+i64::from(*arrive),1);mapped[*arrive as usize]=true;}
            if *late {let low=1+step as i64*2;let high=i64::MAX-step as i64;ingest(&mut s,&[fact(low,channel(low)),fact(high,channel(high))]);added+=2;}
            if *reopen {drop(s);s=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();next=Some(s.begin_attribution_pass().unwrap());}
            let cursor=next.take().unwrap_or_else(||s.begin_attribution_pass().unwrap());
            let batch=s.reattribute_batch(&cursor).unwrap();prop_assert!(batch.checked<=256);next=batch.next;
        }
        for (i,known) in mapped.iter().enumerate(){if !known{history(&mut s,source,g,1001+i as i64,1);}}
        let native_before=native(&s);let totals_before=total(&s.conn);
        // 冻结旧遍最多结束三页；稳定映射后新遍必须覆盖所有迟到的低/高 ID。
        let mut rounds=0;while let Some(c)=next {let r=s.reattribute_batch(&c).unwrap();next=r.next;rounds+=1;prop_assert!(rounds<=4);}
        let (checked,_)=pass(&mut s);prop_assert_eq!(checked,count+added);
        for row in full_facts(&s.conn){prop_assert_eq!(&row["source"],&json!(source.get()));prop_assert_eq!(&row["group_id"],&json!(g.get()));}
        prop_assert_eq!(native(&s),native_before);prop_assert_eq!(total(&s.conn),totals_before);
        prop_assert_eq!(pass(&mut s).1,0);prop_assert_eq!(chart(&s,DimensionFilter::Exact(source)).usage.unwrap().total.consumption_rows,(count+added) as i64);
    }
}
