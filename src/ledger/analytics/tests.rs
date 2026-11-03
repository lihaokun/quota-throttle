use super::*;
use crate::catalog::ledger::{ScanLane, ScanPhase, ScanProgress};
use proptest::prelude::*;

mod control_tests;
mod properties;

fn control() -> QueryControl {
    QueryControl::new(Instant::now() + Duration::from_secs(3600))
}
fn state() -> LedgerState {
    LedgerState {
        revision: 7,
        initial_requested_from: Some(0),
        scans: [ScanLane::Recent, ScanLane::History].map(|lane| ScanProgress {
            lane,
            generation: 0,
            window: None,
            next_page: 1,
            phase: ScanPhase::Idle,
            last_observed_at: None,
            fault: None,
        }),
    }
}
fn query(from: i64, to: i64, offset: i64, grain: UsageGrain) -> UsageQuery {
    UsageQuery::new(
        ScanWindow { from, to },
        offset,
        grain,
        UsageFilter::default(),
    )
    .unwrap()
}
fn dimensions<'a>(model: Option<&'a str>, token: Option<i64>) -> UsageDimensions<'a> {
    UsageDimensions {
        source: None,
        group: None,
        token,
        model,
    }
}
fn amount(input: i64, output: i64, consumption_rows: i64) -> UsageTotals {
    UsageTotals {
        input,
        output,
        consumption_rows,
    }
}
fn report(acc: UsageAccumulator, c: &QueryControl) -> UsageReport {
    let snapshot = acc.finish(state(), c).unwrap();
    assert_eq!(snapshot.state, state());
    snapshot.usage.unwrap()
}

#[test]
fn query_rejects_invalid_domains_without_normalizing_labels_or_ids() {
    for (from, to, offset) in [
        (-1, 1, 0),
        (1, 1, 0),
        (2, 1, 0),
        (i64::MIN, i64::MAX, 0),
        (0, JSON_SAFE + 1, 0),
        (0, MAX_QUERY_SECONDS + 1, 0),
        (0, 1, -721),
        (0, 1, 841),
    ] {
        assert_eq!(
            UsageQuery::new(
                ScanWindow { from, to },
                offset,
                UsageGrain::Hour,
                UsageFilter::default()
            )
            .err(),
            Some(AnalyticsError::InvalidQuery)
        );
    }
    for label in [
        String::new(),
        "x\n".into(),
        "密".repeat(86),
        "x".repeat(257),
    ] {
        let f = UsageFilter {
            model: DimensionFilter::Exact(label),
            ..Default::default()
        };
        assert_eq!(
            UsageQuery::new(ScanWindow { from: 0, to: 1 }, 0, UsageGrain::Day, f).err(),
            Some(AnalyticsError::InvalidQuery)
        );
    }
    for token in [0, -1, i64::MIN] {
        let f = UsageFilter {
            token: DimensionFilter::Exact(token),
            ..Default::default()
        };
        assert_eq!(
            UsageQuery::new(ScanWindow { from: 0, to: 1 }, 0, UsageGrain::Day, f).err(),
            Some(AnalyticsError::InvalidQuery)
        );
    }
    let f = UsageFilter {
        token: DimensionFilter::Exact(i64::MAX),
        model: DimensionFilter::Exact(" X ".into()),
        ..Default::default()
    };
    let q = UsageQuery::new(
        ScanWindow {
            from: JSON_SAFE - 1,
            to: JSON_SAFE,
        },
        840,
        UsageGrain::Hour,
        f.clone(),
    )
    .unwrap();
    assert!(q.filter() == &f);
    assert_eq!(
        q.window(),
        ScanWindow {
            from: JSON_SAFE - 1,
            to: JSON_SAFE
        }
    );
    assert_eq!(q.utc_offset_minutes(), 840);
    assert!(q.grain() == UsageGrain::Hour);
    assert_eq!(q.bucket_index(JSON_SAFE), Err(AnalyticsError::InvalidQuery));
    assert_eq!(q.bucket_index(-1), Err(AnalyticsError::InvalidQuery));
}

#[test]
fn query_short_edges_and_local_bucket_boundaries_are_exact() {
    for (from, to) in [
        (0, 1),
        (30, 90),
        (59, 61),
        (59, 60),
        (JSON_SAFE - 1, JSON_SAFE),
    ] {
        let q = query(from, to, -1, UsageGrain::Hour);
        let parts = q.partition();
        assert!(parts.whole.is_none());
        assert_eq!(parts.edges, vec![ScanWindow { from, to }]);
    }
    let p = query(1, 121, 0, UsageGrain::Hour).partition();
    assert_eq!(p.whole, Some(ScanWindow { from: 60, to: 120 }));
    assert_eq!(
        p.edges,
        vec![
            ScanWindow { from: 1, to: 60 },
            ScanWindow { from: 120, to: 121 }
        ]
    );
    let q = query(0, 86401, -1, UsageGrain::Day);
    assert_eq!(
        q.bucket_ranges(),
        vec![
            ScanWindow { from: 0, to: 60 },
            ScanWindow {
                from: 60,
                to: 86401
            }
        ]
    );
    assert_eq!(q.bucket_index(59), Ok(0));
    assert_eq!(q.bucket_index(60), Ok(1));
    assert_eq!(
        query(1, MAX_QUERY_SECONDS + 1, 0, UsageGrain::Hour)
            .bucket_ranges()
            .len(),
        745
    );
    assert_eq!(
        query(1, MAX_QUERY_SECONDS + 1, 0, UsageGrain::Day)
            .bucket_ranges()
            .len(),
        32
    );
    let p = query(0, 120, 0, UsageGrain::Hour).partition();
    assert!(p.edges.is_empty());
    assert_eq!(p.whole, Some(ScanWindow { from: 0, to: 120 }));
}

#[test]
fn analytics_unknown_models_zero_consumption_and_stable_identity_are_preserved() {
    let c = control();
    let q = query(0, 7200, 0, UsageGrain::Hour);
    let mut acc = UsageAccumulator::new(q.clone(), &c).unwrap();
    acc.push(dimensions(None, None), 0, amount(0, 0, 1), &c)
        .unwrap();
    acc.push(
        dimensions(Some("未知"), Some(i64::MAX)),
        3599,
        amount(4, 5, 1),
        &c,
    )
    .unwrap();
    acc.push(dimensions(Some("M"), Some(1)), 3600, amount(2, 0, 1), &c)
        .unwrap();
    acc.push(dimensions(Some("m"), Some(1)), 7199, amount(3, 0, 1), &c)
        .unwrap();
    let snapshot = acc.finish(state(), &c).unwrap();
    assert!(snapshot.query == q);
    let r = snapshot.usage.unwrap();
    assert_eq!(r.total, amount(9, 5, 4));
    assert_eq!(
        r.models,
        vec![
            (None, amount(0, 0, 1)),
            (Some("M".into()), amount(2, 0, 1)),
            (Some("m".into()), amount(3, 0, 1)),
            (Some("未知".into()), amount(4, 5, 1))
        ]
    );
    assert_eq!(
        r.tokens,
        vec![
            (None, amount(0, 0, 1)),
            (Some(1), amount(5, 0, 2)),
            (Some(i64::MAX), amount(4, 5, 1))
        ]
    );
    assert_eq!(r.buckets[0].total, amount(4, 5, 2));
    assert_eq!(r.buckets[1].total, amount(5, 0, 2));
}

#[test]
fn analytics_all_filters_are_anded_and_missing_ids_return_observed_empty() {
    let c = control();
    let s = SourceId::new(i64::MAX).unwrap();
    let g = GroupId::new(i64::MAX).unwrap();
    let f = UsageFilter {
        source: DimensionFilter::Exact(s),
        group: DimensionFilter::Exact(g),
        token: DimensionFilter::Exact(9),
        model: DimensionFilter::Exact(" X ".into()),
    };
    let mut acc = UsageAccumulator::new(
        UsageQuery::new(ScanWindow { from: 0, to: 10 }, 0, UsageGrain::Hour, f).unwrap(),
        &c,
    )
    .unwrap();
    let good = UsageDimensions {
        source: Some(s),
        group: Some(g),
        token: Some(9),
        model: Some(" X "),
    };
    for d in [
        UsageDimensions {
            source: Some(SourceId::new(1).unwrap()),
            ..good
        },
        UsageDimensions {
            group: None,
            ..good
        },
        UsageDimensions {
            token: Some(8),
            ..good
        },
        UsageDimensions {
            model: Some("X"),
            ..good
        },
        good,
    ] {
        acc.push(d, 0, amount(1, 2, 1), &c).unwrap();
    }
    assert_eq!(report(acc, &c).total, amount(1, 2, 1));
    let f = UsageFilter {
        source: DimensionFilter::Unknown,
        group: DimensionFilter::Unknown,
        token: DimensionFilter::Unknown,
        model: DimensionFilter::Unknown,
    };
    let mut acc = UsageAccumulator::new(
        UsageQuery::new(ScanWindow { from: 0, to: 10 }, 0, UsageGrain::Hour, f).unwrap(),
        &c,
    )
    .unwrap();
    acc.push(dimensions(Some("未知"), None), 0, amount(2, 3, 1), &c)
        .unwrap();
    acc.push(dimensions(None, None), 0, amount(0, 0, 1), &c)
        .unwrap();
    assert_eq!(report(acc, &c).total, amount(0, 0, 1));
    let f = UsageFilter {
        token: DimensionFilter::Exact(999),
        ..Default::default()
    };
    let mut acc = UsageAccumulator::new(
        UsageQuery::new(ScanWindow { from: 0, to: 10 }, 0, UsageGrain::Hour, f).unwrap(),
        &c,
    )
    .unwrap();
    acc.push(dimensions(None, None), 0, amount(5, 0, 1), &c)
        .unwrap();
    let r = report(acc, &c);
    assert_eq!(r.total, UsageTotals::default());
    assert!(r.tokens.is_empty());
}

#[test]
fn analytics_empty_and_uninitialized_are_not_conflated() {
    let c = control();
    let r = report(
        UsageAccumulator::new(query(30, 7230, 0, UsageGrain::Hour), &c).unwrap(),
        &c,
    );
    assert_eq!(r.total, UsageTotals::default());
    assert!(r.models.is_empty() && r.tokens.is_empty());
    assert_eq!(r.buckets.len(), 3);
    assert!(r
        .buckets
        .iter()
        .all(|b| b.models.is_empty() && b.total == UsageTotals::default()));
    let mut s = state();
    s.initial_requested_from = None;
    assert_eq!(
        UsageAccumulator::new(query(0, 1, 0, UsageGrain::Hour), &c)
            .unwrap()
            .finish(s, &c)
            .err(),
        Some(AnalyticsError::Ledger(LedgerError::NotInitialized))
    );
}

#[test]
fn analytics_errors_destroy_partial_work_and_cannot_be_ignored() {
    let c = control();
    for bad in [
        amount(-1, 0, 1),
        amount(0, i64::MAX, 1),
        amount(1, 0, 0),
        amount(0, 0, -1),
        amount(0, 0, JSON_SAFE + 1),
    ] {
        let mut acc = UsageAccumulator::new(query(0, 60, 0, UsageGrain::Hour), &c).unwrap();
        acc.push(dimensions(None, None), 0, amount(5, 0, 1), &c)
            .unwrap();
        let e = acc.push(dimensions(None, None), 0, bad, &c).unwrap_err();
        assert!(acc.work.is_err());
        assert_eq!(
            acc.push(dimensions(None, None), 0, amount(1, 0, 1), &c),
            Err(e.clone())
        );
        assert_eq!(acc.finish(state(), &c).err(), Some(e));
    }
    for at in [-1, 60, i64::MAX] {
        let mut acc = UsageAccumulator::new(query(0, 60, 0, UsageGrain::Hour), &c).unwrap();
        assert_eq!(
            acc.push(dimensions(None, None), at, amount(1, 0, 1), &c),
            Err(AnalyticsError::InvalidQuery)
        );
        assert!(acc.finish(state(), &c).is_err());
    }
    let mut acc = UsageAccumulator::new(query(0, 60, 0, UsageGrain::Hour), &c).unwrap();
    acc.push(dimensions(None, None), 0, amount(JSON_SAFE - 1, 0, 1), &c)
        .unwrap();
    assert_eq!(
        acc.push(dimensions(Some("late"), Some(2)), 1, amount(1, 1, 1), &c),
        Err(AnalyticsError::ArithmeticOverflow)
    );
    assert_eq!(
        acc.finish(state(), &c).err(),
        Some(AnalyticsError::ArithmeticOverflow)
    );
}

#[test]
fn analytics_control_cancel_and_deadline_are_checked_before_publish() {
    let now = Instant::now();
    let c = QueryControl::new(now + Duration::from_secs(1));
    assert_eq!(c.check_at(now), Ok(()));
    assert_eq!(
        c.check_at(now + Duration::from_secs(1)),
        Err(AnalyticsError::TimedOut)
    );
    c.cancel();
    assert_eq!(c.check_at(now), Err(AnalyticsError::Cancelled));
    assert!(matches!(
        UsageAccumulator::new(query(0, 1, 0, UsageGrain::Hour), &c),
        Err(AnalyticsError::Cancelled)
    ));
    let c = control();
    let mut acc = UsageAccumulator::new(query(0, 1, 0, UsageGrain::Hour), &c).unwrap();
    acc.push(dimensions(None, None), 0, amount(1, 0, 1), &c)
        .unwrap();
    c.cancel();
    assert_eq!(
        acc.finish(state(), &c).err(),
        Some(AnalyticsError::Cancelled)
    );
    let c = control();
    let acc = UsageAccumulator::new(query(0, 1, 0, UsageGrain::Hour), &c).unwrap();
    let expired = QueryControl::new(Instant::now());
    assert_eq!(
        acc.finish(state(), &expired).err(),
        Some(AnalyticsError::TimedOut)
    );
}

#[test]
fn analytics_model_and_token_capacity_include_unknown_and_reject_plus_one() {
    for models in [true, false] {
        let limit = if models { MAX_MODELS } else { MAX_TOKENS };
        let c = control();
        let mut acc = UsageAccumulator::new(query(0, 60, 0, UsageGrain::Hour), &c).unwrap();
        acc.push(dimensions(None, None), 0, amount(0, 0, 1), &c)
            .unwrap();
        for i in 1..limit {
            let label = format!("m{i}");
            acc.push(
                dimensions(
                    models.then_some(label.as_str()),
                    (!models).then_some(i as i64),
                ),
                0,
                amount(1, 0, 1),
                &c,
            )
            .unwrap();
        }
        let data = acc.work.as_ref().ok().unwrap();
        assert_eq!(
            if models {
                data.models.len()
            } else {
                data.tokens.len()
            },
            limit
        );
        // 已存在的分项仍可加，不能把“容量满”当成全拒绝。
        acc.push(dimensions(None, None), 0, amount(0, 0, 1), &c)
            .unwrap();
        assert_eq!(
            acc.push(
                dimensions(models.then_some("new"), (!models).then_some(i64::MAX)),
                0,
                amount(1, 0, 1),
                &c
            ),
            Err(AnalyticsError::LimitExceeded)
        );
        assert_eq!(
            acc.finish(state(), &c).err(),
            Some(AnalyticsError::LimitExceeded)
        );
    }
}

fn grouped<'a>(group: Option<i64>, model: Option<&'a str>) -> UsageDimensions<'a> {
    UsageDimensions {
        source: group.map(|_| SourceId::new(1).unwrap()),
        group: group.map(|g| GroupId::new(g).unwrap()),
        token: None,
        model,
    }
}
#[test]
fn analytics_group_breakdown_sums_each_group_and_keeps_unattributed_separate() {
    let c = control();
    let mut acc = UsageAccumulator::new(query(0, 7200, 0, UsageGrain::Hour), &c).unwrap();
    acc.push(grouped(Some(2), Some("a")), 0, amount(3, 1, 1), &c).unwrap();
    acc.push(grouped(Some(1), Some("a")), 10, amount(5, 0, 1), &c).unwrap();
    acc.push(grouped(Some(2), Some("b")), 3600, amount(2, 2, 2), &c).unwrap();
    acc.push(grouped(None, None), 3601, amount(0, 0, 1), &c).unwrap();
    let r = report(acc, &c);
    assert_eq!(r.total, amount(10, 3, 5));
    // 与 tokens 一样按键排序；未归组（None）单列，不并入任何组。
    assert_eq!(
        r.groups,
        vec![
            (None, amount(0, 0, 1)),
            (Some(GroupId::new(1).unwrap()), amount(5, 0, 1)),
            (Some(GroupId::new(2).unwrap()), amount(5, 3, 3)),
        ]
    );
}
#[test]
fn analytics_group_capacity_includes_unattributed_and_rejects_plus_one() {
    let c = control();
    let mut acc = UsageAccumulator::new(query(0, 60, 0, UsageGrain::Hour), &c).unwrap();
    acc.push(grouped(None, None), 0, amount(0, 0, 1), &c).unwrap();
    for g in 1..MAX_USAGE_GROUPS as i64 {
        acc.push(grouped(Some(g), None), 0, amount(1, 0, 1), &c).unwrap();
    }
    assert_eq!(acc.work.as_ref().ok().unwrap().groups.len(), MAX_USAGE_GROUPS);
    // 已存在的组仍可加；新组越界整份作废，不发布部分结果。
    acc.push(grouped(Some(1), None), 0, amount(1, 0, 1), &c).unwrap();
    assert_eq!(
        acc.push(grouped(Some(MAX_USAGE_GROUPS as i64), None), 0, amount(1, 0, 1), &c),
        Err(AnalyticsError::LimitExceeded)
    );
    assert_eq!(acc.finish(state(), &c).err(), Some(AnalyticsError::LimitExceeded));
}

fn maximum_output(c: &QueryControl) -> UsageAccumulator {
    let mut acc =
        UsageAccumulator::new(query(1, MAX_QUERY_SECONDS + 1, 0, UsageGrain::Hour), c).unwrap();
    for hour in 0..64 {
        for model in 0..1024 {
            acc.push(
                dimensions(
                    Some(&format!("m{model:0255}")),
                    Some((hour * 1024 + model) % 4096 + 1),
                ),
                1 + hour * 3600,
                amount(1, 2, 1),
                c,
            )
            .unwrap();
        }
    }
    acc
}
#[test]
fn analytics_bucket_model_capacity_and_complete_output() {
    let c = control();
    let acc = maximum_output(&c);
    let r = report(acc, &c);
    assert_eq!(r.total, amount(65536, 131072, 65536));
    assert_eq!(
        r.buckets.iter().map(|b| b.models.len()).sum::<usize>(),
        65536
    );
    assert_eq!(r.models.len(), 1024);
    assert_eq!(r.tokens.len(), 4096);
    assert!(r
        .models
        .iter()
        .all(|(m, _)| m.as_ref().unwrap().len() == 256));
    assert_eq!(r.buckets.len(), 745);
    assert!(r.buckets[64..].iter().all(|b| b.models.is_empty()));
    let mut acc = maximum_output(&c);
    let existing = format!("m{:0255}", 0);
    acc.push(dimensions(Some(&existing), Some(1)), 1, amount(0, 0, 1), &c)
        .unwrap();
    assert_eq!(
        acc.push(
            dimensions(Some(&existing), Some(1)),
            1 + 64 * 3600,
            amount(1, 0, 1),
            &c
        ),
        Err(AnalyticsError::LimitExceeded)
    );
    assert!(acc.finish(state(), &c).is_err());
}

#[test]
fn analytics_private_types_have_no_logging_or_wire_traits() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    macro_rules! private {
        ($t:ty) => {
            assert_not_impl!($t, std::fmt::Debug);
            assert_not_impl!($t, std::fmt::Display);
            assert_not_impl!($t, serde::Serialize);
            assert_not_impl!($t, serde::Deserialize<'static>);
        };
    }
    private!(UsageQuery);
    private!(UsageFilter);
    private!(DimensionFilter<String>);
    private!(UsageSnapshot);
    private!(UsageReport);
    private!(UsageBucket);
    private!(UsageDimensions<'static>);
    private!(UsageAccumulator);
    private!(QueryControl);
    let c = control();
    let f = UsageFilter {
        model: DimensionFilter::Exact("private-sentinel\n".into()),
        ..Default::default()
    };
    let error = UsageQuery::new(ScanWindow { from: 0, to: 1 }, 0, UsageGrain::Hour, f)
        .err()
        .unwrap();
    assert!(!format!("{error:?}").contains("sentinel"));
    let mut acc = UsageAccumulator::new(query(0, 1, 0, UsageGrain::Hour), &c).unwrap();
    let error = acc
        .push(
            dimensions(Some("private-sentinel\n"), None),
            0,
            amount(1, 0, 1),
            &c,
        )
        .unwrap_err();
    assert!(!format!("{error:?}").contains("sentinel"));
}

#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_analytics_maximum_output() {
    let c = control();
    let start = Instant::now();
    let r = report(maximum_output(&c), &c);
    assert_eq!(r.total, amount(65536, 131072, 65536));
    let elapsed = start.elapsed();
    eprintln!("ANALYTICS_MAXIMUM_OUTPUT_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}
