use super::*;

fn history(count: i64) -> (TempDir, CatalogStore) {
    let (dir, mut store, _, _) = mapped(2);
    let second = store
        .create_group(group("ledger-g2", SourceKind::Glm))
        .unwrap()
        .group_id;
    store
        .conn
        .execute(
            "UPDATE memberships SET group_id=?1 WHERE channel_id=1002",
            [second.get()],
        )
        .unwrap();
    let end = MAX_QUERY_SECONDS + 1;
    store.initialize_ledger(0, end).unwrap();
    store
        .begin_scan(ScanLane::Recent, 0, ScanWindow { from: 0, to: end }, end)
        .unwrap();
    let mut batch = Vec::with_capacity(1000);
    for i in 0..count {
        let channel = if i % 7 == 0 { 0 } else { 1001 + i % 2 };
        let group = if channel == 1001 {
            "ledger-g"
        } else if channel == 1002 {
            "ledger-g2"
        } else {
            "unmanaged"
        };
        batch.push(json!({"id":i+1,"type":2,"created_at":1+i*MAX_QUERY_SECONDS/count,"channel":channel,"token_id":if i%17==0{0}else{i%100+1},"model_name":if i%19==0{None}else{Some(format!("m{}",i%20))},"group":group,"prompt_tokens":1,"completion_tokens":2,"is_stream":false,"other":""}));
        if batch.len() == 1000 || i + 1 == count {
            let t = store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
            store
                .commit_fact_page(&t, &batch, PageEnd::More, end)
                .unwrap();
            batch.clear();
        }
    }
    (dir, store)
}
fn chart(token: DimensionFilter<i64>, grain: UsageGrain) -> UsageQuery {
    UsageQuery::new(
        ScanWindow {
            from: 1,
            to: MAX_QUERY_SECONDS + 1,
        },
        480,
        grain,
        UsageFilter {
            token,
            ..Default::default()
        },
    )
    .unwrap()
}
#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_analytics_sql_100k() {
    let (_dir, store) = history(100000);
    let start = Instant::now();
    let result = read(&store, chart(DimensionFilter::Any, UsageGrain::Day))
        .usage
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(result.total.input, 100000);
    assert_eq!(result.total.output, 200000);
    assert_eq!(result.total.consumption_rows, 100000);
    eprintln!("ANALYTICS_SQL_OVERALL_100K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
    let expected = (0..100000).filter(|i| i % 17 != 0 && i % 100 == 0).count() as i64;
    let start = Instant::now();
    let result = read(&store, chart(DimensionFilter::Exact(1), UsageGrain::Hour))
        .usage
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(result.total.input, expected);
    assert_eq!(result.total.consumption_rows, expected);
    eprintln!("ANALYTICS_SQL_TOKEN_100K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}
#[test]
#[ignore = "显式隔离容量趋势，不是默认全量或硬实时门禁"]
fn analytics_sql_large_history_trends() {
    // 显式测量时允许一进程只做一个规模，外部 time 的 RSS 不含另一个夹具。
    let counts: &[i64] = match std::env::var("QUOTA_THROTTLE_ANALYTICS_TREND_ROWS")
        .ok()
        .as_deref()
    {
        None => &[200000, 1000000],
        Some("200000") => &[200000],
        Some("1000000") => &[1000000],
        Some(_) => panic!("trend rows must be 200000 or 1000000"),
    };
    for &count in counts {
        let (dir, mut store) = history(count);
        let checkpoint: (i64, i64, i64) = store
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(checkpoint.0, 0);
        let target = store.usage_read_target().unwrap();
        let mut callbacks = 0;
        let mut scanned = 0;
        let mut pinned_bytes = 0;
        let mut held_checkpoint = (0, 0, 0);
        let start = Instant::now();
        let result = target
            .test_query_observed(
                chart(DimensionFilter::Any, UsageGrain::Day),
                &control(),
                |observed| {
                    callbacks += 1;
                    scanned = observed;
                    if callbacks == 1 {
                        let ticket = store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
                        store
                            .commit_fact_page(
                                &ticket,
                                &[row(count + 1)],
                                PageEnd::More,
                                MAX_QUERY_SECONDS + 1,
                            )
                            .unwrap();
                        held_checkpoint = store
                            .conn
                            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| {
                                Ok((
                                    r.get::<_, i64>(0)?,
                                    r.get::<_, i64>(1)?,
                                    r.get::<_, i64>(2)?,
                                ))
                            })
                            .unwrap();
                        pinned_bytes = fs::metadata(dir.path().join("catalog.sqlite3-wal"))
                            .unwrap()
                            .len();
                    }
                },
            )
            .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(result.usage.unwrap().total.consumption_rows, count);
        assert_eq!(scanned, count as usize);
        assert!(held_checkpoint.1 > held_checkpoint.2);
        assert!(pinned_bytes > 0);
        let released: (i64, i64, i64) = store
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(released, (0, 0, 0));
        let after = fs::metadata(dir.path().join("catalog.sqlite3-wal"))
            .unwrap()
            .len();
        assert_eq!(after, 0);
        let rss = fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|s| s.starts_with("VmHWM:"))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "VmHWM unavailable".into());
        eprintln!("ANALYTICS_TREND rows={count} query_with_write_ms={} scanned_rows={scanned} callbacks={callbacks} wal_held_bytes={pinned_bytes} wal_frames={} wal_backfilled={} wal_after_release={after} reported_proc_vmhwm_including_fixture={rss}",elapsed.as_millis(),held_checkpoint.1,held_checkpoint.2);
    }
}
