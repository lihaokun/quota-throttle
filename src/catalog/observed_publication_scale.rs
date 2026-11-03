use super::*;
use rusqlite::TransactionBehavior;
use std::time::{Duration, Instant};

#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_observed_period_usage_100k() {
    let mut f = fixture();
    for batch in 0..100i64 {
        let rows = (1..=1000)
            .map(|i| {
                let id = batch * 1000 + i;
                let mut row = event(id, 1001, "historical", true, NOW - 1000);
                row["token_id"] = json!(id);
                row
            })
            .collect::<Vec<_>>();
        ingest(&mut f.store, &rows, NOW);
    }
    assert_eq!(queue(&f.store), 0);
    let start = Instant::now();
    let r = round(&mut f, NOW);
    let elapsed = start.elapsed();
    assert_eq!(
        (runtime(&f, 0).used_input, runtime(&f, 0).used_output),
        (500000, 300000)
    );
    assert_eq!(r.report.permitted, 4);
    eprintln!("OBSERVED_PERIOD_USAGE_100K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(10));
}

#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_observed_maximum_catalog() {
    let (_dir, mut store) = setup();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let mut sources = Vec::new();
    for i in 0..MAX_SOURCES {
        sources.push(
            insert_source(
                &tx,
                &validate_source(source(
                    &format!("fake-o4-capacity-{i}"),
                    SourceKind::Volcano,
                ))
                .unwrap(),
            )
            .unwrap()
            .source_id,
        );
    }
    let per_group = MAX_MEMBERSHIPS / MAX_GROUPS;
    for g in 0..MAX_GROUPS {
        let group = insert_group(
            &tx,
            &validate_group(group(&format!("capacity-{g}"), SourceKind::Volcano)).unwrap(),
        )
        .unwrap();
        let op = format!("capacity-op-{g:016}");
        tx.execute("INSERT INTO control_operations VALUES(?1,'set_group_priorities','group',?2,?3,1,?4,'queued',NULL,0,0)",params![op,group.group_id.get(),"a".repeat(64),per_group as i64]).unwrap();
        for i in 0..per_group {
            let index = g * per_group + i;
            let m = insert_membership(
                &tx,
                &NewMembership {
                    source: sources[index % MAX_SOURCES],
                    group: group.group_id,
                    kind: SourceKind::Volcano,
                    channel: Some(ChannelId::new(index as i64 + 1).unwrap()),
                    position: i as i64,
                    lifecycle: MembershipLifecycle::Imported,
                    priority: 7,
                },
            )
            .unwrap();
            tx.execute(
                "INSERT INTO operation_targets VALUES(?1,?2,1,NULL,NULL)",
                params![op, m.membership_id.get()],
            )
            .unwrap();
        }
    }
    tx.execute("UPDATE source_groups SET ready=1", []).unwrap();
    tx.commit().unwrap();
    store.initialize_ledger(0, NOW).unwrap();
    // 真正 B1 摄入：每源16次同请求重试，不能算成16次不可用。
    for attempt in 0..16 {
        let rows = (0..MAX_SOURCES)
            .map(|i| {
                let mut row = event(
                    (attempt * MAX_SOURCES + i + 1) as i64,
                    i as i64 + 1,
                    "same-retry",
                    false,
                    NOW - 1,
                );
                row["group"] = json!(format!("capacity-{}", i / per_group));
                row
            })
            .collect::<Vec<_>>();
        ingest(&mut store, &rows, NOW);
    }
    let cfg: Config =
        toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\n").unwrap();
    let start = Instant::now();
    let r = store
        .commit_volcano_round(&cfg, backend(NOW), &HashMap::new(), NOW)
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(r.report.sources, MAX_SOURCES);
    assert_eq!(r.report.observations, 16 * MAX_SOURCES);
    assert_eq!(r.report.pending_sources, 0);
    assert_eq!(r.report.permitted, MAX_MEMBERSHIPS);
    assert_eq!(r.report.changed_members, MAX_MEMBERSHIPS);
    assert_eq!(r.report.changed_groups, MAX_GROUPS);
    assert_eq!(store.conn.query_row("SELECT count(*) FROM volcano_runtime WHERE consecutive_failures=1 AND error_exhausted=0",[],|r|r.get::<_,i64>(0)).unwrap(),MAX_SOURCES as i64);
    assert_eq!(queue(&store), 0);
    let changes = store.conn.total_changes();
    let no_op = store
        .commit_volcano_round(&cfg, backend(NOW), &r.permits, NOW)
        .unwrap();
    assert_eq!(no_op.report.changed_members, 0);
    assert_eq!(changes, store.conn.total_changes());
    eprintln!("OBSERVED_MAXIMUM_CATALOG_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(10));
}
