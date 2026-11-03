use super::*;
use crate::panel_http::tests::empty;
use proptest::prelude::*;

const AT: i64 = 1_800_000_000_000;
fn frame(start: Instant, at: i64) -> PublishedPanel {
    PublishedPanel {
        bytes: encode_panel(&empty(at)).unwrap(),
        captured_at_ms: at,
        started_at: start,
    }
}

#[test]
fn h1_cache_age_bounds_round_up_and_never_revive_after_clock_reversal() {
    let start = Instant::now();
    for (nanos, wall_delta, expected) in [
        (0, 0, Some(0)),
        (1, 0, Some(1)),
        (1_000_001, 0, Some(2)),
        (9_999_000_000, 0, Some(9999)),
        (9_999_000_001, 0, None),
        (1, 9999, Some(9999)),
        (1, 10000, None),
        (1, -1, None),
    ] {
        let cache = PanelCache::default();
        cache.replace(Some(frame(start, AT))).unwrap();
        let result = cache.get(start + Duration::from_nanos(nanos), Some(AT + wall_delta));
        assert_eq!(result.as_ref().ok().map(|(_, age)| *age), expected);
        if expected.is_none() {
            assert_eq!(
                cache.get(start, Some(AT)).err(),
                Some(Code::SnapshotUnavailable)
            );
        }
    }
    for wall in [None, Some(-1), Some(JSON_SAFE + 1)] {
        let cache = PanelCache::default();
        cache.replace(Some(frame(start, AT))).unwrap();
        assert_eq!(
            cache.get(start, wall).err(),
            Some(Code::SnapshotUnavailable)
        );
        assert_eq!(
            cache.get(start, Some(AT)).err(),
            Some(Code::SnapshotUnavailable)
        );
    }
    let cache = PanelCache::default();
    cache.replace(Some(frame(start, AT))).unwrap();
    assert_eq!(
        cache.get(start - Duration::from_nanos(1), Some(AT)).err(),
        Some(Code::SnapshotUnavailable)
    );
}

#[test]
fn h1_cache_empty_busy_poison_and_repeated_reads_do_not_mutate_evidence() {
    let start = Instant::now();
    let cache = Arc::new(PanelCache::default());
    assert_eq!(
        cache.get(start, Some(AT)).err(),
        Some(Code::SnapshotUnavailable)
    );
    cache.replace(Some(frame(start, AT))).unwrap();
    let first = cache.get(start, Some(AT)).unwrap().0;
    for ms in [0, 10, 1000, 9999] {
        let (bytes, age) = cache
            .get(start + Duration::from_millis(ms), Some(AT))
            .unwrap();
        assert!(Arc::ptr_eq(&first, &bytes));
        assert_eq!(age, ms);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["captured_at_ms"],
            AT
        );
    }
    let guard = cache.entry.lock().unwrap();
    assert_eq!(cache.get(start, Some(AT)).err(), Some(Code::Overloaded));
    drop(guard);
    let c = cache.clone();
    assert!(std::thread::spawn(move || {
        let _guard = c.entry.lock().unwrap();
        panic!("test-only cache poison")
    })
    .join()
    .is_err());
    assert_eq!(cache.get(start, Some(AT)).err(), Some(Code::Internal));
    assert_eq!(cache.replace(None), Err(Code::Internal));
}

#[test]
fn h1_bounded_writer_rejects_before_append_and_limits_capacity_growth() {
    assert_eq!(MAX_RESPONSE, 32 * 1024 * 1024);
    assert_eq!(PUBLICATION_TTL, Duration::from_secs(10));
    let mut writer = BoundedWriter {
        bytes: vec![],
        limit: 10,
    };
    writer.write_all(&[1; 6]).unwrap();
    writer.write_all(&[2; 4]).unwrap();
    let before = writer.bytes.clone();
    assert!(writer.write_all(&[3]).is_err());
    assert_eq!(writer.bytes, before);
    assert!(writer.bytes.capacity() <= 10);
    let mut writer = BoundedWriter {
        bytes: vec![],
        limit: 8,
    };
    assert!(writer.write_all(&[0; 9]).is_err());
    assert!(writer.bytes.is_empty());
    let mut writer = BoundedWriter {
        bytes: vec![],
        limit: 0,
    };
    writer.write_all(&[]).unwrap();
    assert!(writer.write_all(&[0]).is_err());
}

#[tokio::test]
async fn h1_publication_completion_never_renews_start_or_accepts_late_results() {
    for mode in 0..5 {
        let start = Instant::now();
        let cache = PanelCache::default();
        cache.replace(Some(frame(start, AT))).unwrap();
        let mut active = start_publication(
            Arc::new(move || {
                if mode == 4 {
                    Err(PublishError)
                } else {
                    Ok(empty(AT))
                }
            }),
            start,
        );
        let result = (&mut active.task).await;
        if mode == 1 {
            expire_publication(&mut active, &cache).unwrap();
        }
        let now = if mode == 2 {
            start + PUBLICATION_TTL
        } else {
            start + Duration::from_secs(3)
        };
        finish_publication(active, result, &cache, now, Some(AT + 3000), mode == 3).unwrap();
        if mode == 0 {
            let (_, age) = cache.get(now, Some(AT + 3000)).unwrap();
            assert_eq!(age, 3000);
            assert_eq!(
                cache.get(start + PUBLICATION_TTL, Some(AT + 3000)).err(),
                Some(Code::SnapshotUnavailable)
            );
        } else {
            assert_eq!(
                cache.get(start, Some(AT)).err(),
                Some(Code::SnapshotUnavailable)
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn h1_pb3_cache_reference_trace_never_renews_or_revives(events in prop::collection::vec((0u8..3,0u64..13_000,-2i64..13_000),0..40)) {
        let cache=PanelCache::default();let start=Instant::now();let mut model:Option<(u64,i64)>=None;let mut mono=0;
        for (kind,advance,wall_delta) in events {
            mono+=advance;let now=start+Duration::from_millis(mono);let wall=AT+wall_delta;
            match kind {
                0=>{cache.replace(Some(frame(now,wall))).unwrap();model=Some((mono,wall));},
                1=>{cache.replace(None).unwrap();model=None;},
                _=>{
                    let expected=model.and_then(|(began,captured)| {if wall<captured {None}else{let age=(mono-began).max((wall-captured) as u64);(age<10_000).then_some((age,captured))}});
                    let actual=cache.get(now,Some(wall));
                    if let Some((age,captured))=expected {let (bytes,got_age)=actual.unwrap();prop_assert_eq!(got_age,age);prop_assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["captured_at_ms"].as_i64(),Some(captured));}
                    else {prop_assert_eq!(actual.err(),Some(Code::SnapshotUnavailable));model=None;}
                }
            }
        }
    }
}

#[path = "capacity.rs"]
mod capacity;
