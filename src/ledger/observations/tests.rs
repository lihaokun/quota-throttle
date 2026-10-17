use super::*;
use crate::catalog::{GroupId, VolcanoPolicy};
use crate::source_policy::volcano::{self, VolcanoRuntime, VolcanoState};
use proptest::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const NOW: i64 = 180_000;
fn source(n: i64) -> SourceId {
    SourceId::new(n).unwrap()
}
fn metadata() -> RelayMetadata {
    RelayMetadata {
        status_code: None,
        error_type: None,
        error_code: None,
        attempted_channels: None,
        stream: StreamObservation::Missing,
    }
}
fn fact(id: i64, src: i64, request: &str, success: bool) -> AttributedFact {
    let other = if success {
        String::new()
    } else {
        json!({"status_code":503,"error_type":"openai_error","error_code":"unavailable"})
            .to_string()
    };
    let row = json!({"id":id,"type":if success{2}else{5},"created_at":NOW,"channel":100+src,"token_id":7,"model_name":"fake-model","group":"fake-group","prompt_tokens":if success{5}else{0},"completion_tokens":if success{3}else{0},"is_stream":false,"request_id":request,"other":other});
    AttributedFact {
        native: decode_native_row(&row).unwrap().unwrap(),
        source_id: Some(source(src)),
        group_id: Some(GroupId::new(40).unwrap()),
        issue: AttributionIssue::None,
    }
}
fn candidate(request: &str, kind: OutcomeKind, at: i64) -> ObservationCandidate {
    ObservationCandidate {
        source_id: source(5),
        request_id: request.into(),
        occurred_at: at,
        kind,
    }
}
fn runtime(src: i64) -> VolcanoRuntime {
    volcano::initialize_runtime(source(src), &VolcanoPolicy::default(), NOW).unwrap()
}
fn feed(
    requests: &mut BTreeMap<(SourceId, String), RequestObservationState>,
    runtimes: &mut BTreeMap<SourceId, VolcanoRuntime>,
    f: &AttributedFact,
) -> Option<OutcomeKind> {
    let c = classify_observation(f).unwrap()?;
    let key = (c.source_id, c.request_id.clone());
    let d = observe_request(requests.get(&key), &c).unwrap();
    if let Some(o) = &d.outcome {
        let old = runtimes.get(&c.source_id).unwrap();
        let new = volcano::apply_outcome(old, &VolcanoPolicy::default(), o).unwrap();
        runtimes.insert(c.source_id, new);
    }
    requests.insert(key, d.next);
    d.outcome.map(|o| o.kind)
}

#[test]
fn observed_breaker_success_tokens_include_nonstream_done_eof_and_client_gone() {
    let mut f = fact(1, 5, "observed", true);
    assert_eq!(
        classify_observation(&f).unwrap().unwrap().kind,
        OutcomeKind::Success
    );
    f.native.output = 0; // prompt-only positive usage also counts: not output>0 only.
    assert!(classify_observation(&f).unwrap().is_some());
    for stream in [
        StreamObservation::Done,
        StreamObservation::Eof,
        StreamObservation::ClientGone,
    ] {
        f.native.is_stream = true;
        f.native.metadata = RelayMetadataState::Parsed(RelayMetadata {
            stream,
            ..metadata()
        });
        assert_eq!(
            classify_observation(&f).unwrap().unwrap().kind,
            OutcomeKind::Success
        );
    }
    f.native.input = 0;
    assert!(classify_observation(&f).unwrap().is_none());
}

#[test]
fn observed_breaker_unknown_zero_and_explicit_failures_never_look_healthy() {
    let base = fact(1, 5, "success", true);
    for state in [
        RelayMetadataState::Unusable,
        RelayMetadataState::Parsed(RelayMetadata {
            status_code: Some(503),
            ..metadata()
        }),
        RelayMetadataState::Parsed(RelayMetadata {
            error_type: Some("openai_error".into()),
            ..metadata()
        }),
        RelayMetadataState::Parsed(RelayMetadata {
            error_code: Some("failed".into()),
            ..metadata()
        }),
    ] {
        let mut f = base.clone();
        f.native.metadata = state;
        assert!(classify_observation(&f).unwrap().is_none());
    }
    for state in [
        RelayMetadataState::Absent,
        RelayMetadataState::Parsed(metadata()),
        RelayMetadataState::Parsed(RelayMetadata {
            stream: StreamObservation::Other,
            ..metadata()
        }),
    ] {
        let mut f = base.clone();
        f.native.is_stream = true;
        f.native.metadata = state;
        assert!(classify_observation(&f).unwrap().is_none());
    }
    let mut f = base.clone();
    f.native.input = 0;
    f.native.output = 0;
    assert!(classify_observation(&f).unwrap().is_none());
    let mut f = base;
    f.native.metadata = RelayMetadataState::Parsed(RelayMetadata {
        stream: StreamObservation::Done,
        ..metadata()
    });
    assert!(classify_observation(&f).unwrap().is_none()); // contradictory non-stream metadata
}

#[test]
fn observed_breaker_errors_use_upstream_whitelist_and_exclusions_first() {
    for status in 100..=599 {
        let mut f = fact(1, 5, "error", false);
        let RelayMetadataState::Parsed(ref mut m) = f.native.metadata else {
            unreachable!()
        };
        m.status_code = Some(status);
        assert_eq!(
            classify_observation(&f).unwrap().is_some(),
            matches!(status, 401 | 429 | 500 | 502 | 503 | 504)
        );
    }
    for code in [
        "invalid_api_key",
        "authentication_error",
        "permission_denied",
    ] {
        let mut f = fact(1, 5, "error", false);
        let RelayMetadataState::Parsed(ref mut m) = f.native.metadata else {
            unreachable!()
        };
        m.status_code = Some(403);
        m.error_code = Some(code.into());
        assert!(classify_observation(&f).unwrap().is_some());
    }
    for code in [
        "invalid_request_error",
        "invalid_request",
        "invalid_parameter",
        "context_length_exceeded",
        "content_filter",
        "content_policy_violation",
        "context_canceled",
        "context_cancelled",
        "client_gone",
        "request_cancelled",
    ] {
        for status in [401, 403, 429, 500, 502, 503, 504] {
            let mut f = fact(1, 5, "error", false);
            let RelayMetadataState::Parsed(ref mut m) = f.native.metadata else {
                unreachable!()
            };
            m.status_code = Some(status);
            m.error_code = Some(code.into());
            assert!(classify_observation(&f).unwrap().is_none());
        }
    }
    for kind in [None, Some("new_api_error"), Some("unknown_provider")] {
        let mut f = fact(1, 5, "error", false);
        let RelayMetadataState::Parsed(ref mut m) = f.native.metadata else {
            unreachable!()
        };
        m.error_type = kind.map(str::to_owned);
        assert!(classify_observation(&f).unwrap().is_none());
    }
    let mut f = fact(1, 5, "error", false);
    let RelayMetadataState::Parsed(ref mut m) = f.native.metadata else {
        unreachable!()
    };
    m.stream = StreamObservation::ClientGone;
    assert!(classify_observation(&f).unwrap().is_none());
}

#[test]
fn observed_breaker_failure_failure_success_failure_resets_without_unlatching() {
    let mut requests = BTreeMap::new();
    let mut runtimes = BTreeMap::from([(source(5), runtime(5))]);
    for (id, ok, expected) in [(1, false, 1), (2, false, 2), (3, true, 0), (4, false, 1)] {
        feed(
            &mut requests,
            &mut runtimes,
            &fact(id, 5, &format!("r{id}"), ok),
        );
        let r = &runtimes[&source(5)];
        assert_eq!(r.consecutive_failures, expected);
        assert!(!r.error_exhausted);
    }
    for id in 5..=6 {
        feed(
            &mut requests,
            &mut runtimes,
            &fact(id, 5, &format!("r{id}"), false),
        );
    }
    assert!(runtimes[&source(5)].error_exhausted);
    feed(
        &mut requests,
        &mut runtimes,
        &fact(7, 5, "success-after-trip", true),
    );
    let r = &runtimes[&source(5)];
    assert_eq!(r.consecutive_failures, 0);
    assert!(r.error_exhausted);
    assert_eq!(r.state(false), VolcanoState::Exhausted);
    assert_eq!(r.state(true), VolcanoState::Paused);
    let p = VolcanoPolicy {
        daily_token_limit: Some(1),
        ..VolcanoPolicy::default()
    };
    let limited = volcano::apply_usage(r, &p, 1, 0).unwrap();
    let cleared = volcano::apply_outcome(
        &limited,
        &p,
        &OutcomeObservation {
            first_event_at: NOW,
            observed_at: NOW,
            kind: OutcomeKind::Success,
        },
    )
    .unwrap();
    assert!(cleared.error_exhausted && cleared.limit_exhausted);
    assert_eq!(cleared.used_input, 1);
}

#[test]
fn observed_breaker_shared_source_dedup_and_other_source_success_are_isolated() {
    let mut requests = BTreeMap::new();
    let mut runtimes = BTreeMap::from([(source(5), runtime(5)), (source(9), runtime(9))]);
    let mut f = fact(1, 5, "same-request", false);
    assert_eq!(
        feed(&mut requests, &mut runtimes, &f),
        Some(OutcomeKind::Unavailable)
    );
    f.native.log_id = 2;
    f.native.channel_id = Some(ChannelId::new(200).unwrap());
    f.group_id = Some(GroupId::new(80).unwrap());
    assert_eq!(feed(&mut requests, &mut runtimes, &f), None);
    feed(
        &mut requests,
        &mut runtimes,
        &fact(3, 9, "same-request", false),
    );
    feed(
        &mut requests,
        &mut runtimes,
        &fact(4, 9, "same-request", true),
    );
    assert_eq!(runtimes[&source(5)].consecutive_failures, 1);
    assert_eq!(runtimes[&source(9)].consecutive_failures, 0);
    feed(
        &mut requests,
        &mut runtimes,
        &fact(5, 5, "same-request", true),
    );
    assert_eq!(runtimes[&source(5)].consecutive_failures, 0);
    feed(
        &mut requests,
        &mut runtimes,
        &fact(6, 5, "new-request", false),
    );
    assert_eq!(
        feed(
            &mut requests,
            &mut runtimes,
            &fact(7, 5, "same-request", true)
        ),
        None
    );
    assert_eq!(
        feed(
            &mut requests,
            &mut runtimes,
            &fact(8, 5, "same-request", false)
        ),
        None
    );
    assert_eq!(runtimes[&source(5)].consecutive_failures, 1);
}

#[test]
fn observed_breaker_identity_validation_and_failed_inputs_are_atomic() {
    let c = candidate("request", OutcomeKind::Unavailable, NOW);
    let state = observe_request(None, &c).unwrap().next;
    for bad in [
        ObservationCandidate {
            source_id: source(9),
            ..c.clone()
        },
        ObservationCandidate {
            request_id: "other".into(),
            ..c.clone()
        },
    ] {
        let before = state.clone();
        assert_eq!(
            observe_request(Some(&state), &bad).err(),
            Some(FactError::InvalidAttribution)
        );
        assert!(state == before);
    }
    for bad in [
        ObservationCandidate {
            request_id: "bad id".into(),
            ..c.clone()
        },
        ObservationCandidate {
            request_id: "a".repeat(65),
            ..c.clone()
        },
        ObservationCandidate {
            occurred_at: -1,
            ..c.clone()
        },
        ObservationCandidate {
            occurred_at: JSON_SAFE + 1,
            ..c.clone()
        },
        ObservationCandidate {
            kind: OutcomeKind::Ignored,
            ..c.clone()
        },
    ] {
        assert_eq!(
            observe_request(Some(&state), &bad).err(),
            Some(FactError::InvalidShape)
        );
    }
    let mut bad_state = state.clone();
    bad_state.first_event_at = -1;
    assert_eq!(
        observe_request(Some(&bad_state), &c).err(),
        Some(FactError::InvalidShape)
    );
    let mut f = fact(1, 5, "request", true);
    f.native.request_id = None;
    assert!(classify_observation(&f).unwrap().is_none());
    f = fact(1, 5, "request", true);
    f.source_id = None;
    f.group_id = None;
    f.issue = AttributionIssue::UnmappedChannel;
    assert!(classify_observation(&f).unwrap().is_none());
    f = fact(1, 5, "request", false);
    f.native.input = 1;
    assert_eq!(
        classify_observation(&f).err(),
        Some(FactError::InvalidNumber)
    );
    let mut f = fact(1, 5, "request", true);
    f.native.output = JSON_SAFE;
    assert_eq!(
        classify_observation(&f).err(),
        Some(FactError::InvalidNumber)
    );
}

#[test]
fn observed_breaker_late_success_respects_existing_cutoff_and_period() {
    let c = candidate("old", OutcomeKind::Unavailable, NOW);
    let old = observe_request(None, &c).unwrap().next;
    let earlier = observe_request(
        Some(&old),
        &ObservationCandidate {
            occurred_at: NOW - 1,
            ..c
        },
    )
    .unwrap();
    assert_eq!(earlier.next.first_event_at, NOW - 1);
    assert!(earlier.outcome.is_none());
    let late = observe_request(
        Some(&earlier.next),
        &candidate("old", OutcomeKind::Success, NOW + 4),
    )
    .unwrap()
    .outcome
    .unwrap();
    let p = VolcanoPolicy::default();
    let mut r = volcano::clear_error(&runtime(5), NOW + 1).unwrap();
    r = volcano::apply_outcome(
        &r,
        &p,
        &OutcomeObservation {
            first_event_at: NOW + 2,
            observed_at: NOW + 2,
            kind: OutcomeKind::Unavailable,
        },
    )
    .unwrap();
    assert_eq!(volcano::apply_outcome(&r, &p, &late).unwrap(), r);
    let next = volcano::advance_period(&r, &p, r.next_reset_at).unwrap().0;
    assert_eq!(volcano::apply_outcome(&next, &p, &late).unwrap(), next);
}

#[test]
fn observed_breaker_private_request_material_has_no_logging_or_wire_traits() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(ObservationCandidate, std::fmt::Debug);
    assert_not_impl!(ObservationCandidate, std::fmt::Display);
    assert_not_impl!(ObservationCandidate, serde::Serialize);
    assert_not_impl!(ObservationCandidate, serde::Deserialize<'static>);
    assert_not_impl!(RequestObservationState, std::fmt::Debug);
    assert_not_impl!(RequestObservationState, std::fmt::Display);
    assert_not_impl!(RequestObservationState, serde::Serialize);
    assert_not_impl!(RequestObservationState, serde::Deserialize<'static>);
    assert_not_impl!(ObservationDecision, std::fmt::Debug);
    assert_not_impl!(ObservationDecision, std::fmt::Display);
    assert_not_impl!(ObservationDecision, serde::Serialize);
    assert_not_impl!(ObservationDecision, serde::Deserialize<'static>);
    assert_not_impl!(OutcomeObservation, serde::Deserialize<'static>);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn observed_pb1_classification_matches_independent_table(success in any::<bool>(),tokens in 0i64..10,is_stream in any::<bool>(),stream in 0u8..5,meta in 0u8..3,status in prop_oneof![4 => prop::sample::select(vec![200u16,401,403,429,500,502,503,504]), 1 => 100u16..600],error_type in 0u8..3,code in 0u8..4,known in any::<bool>()) {
        let mut f=fact(1,5,"random",success);f.native.is_stream=is_stream;
        if success{f.native.input=tokens;f.native.output=0;}
        let streams=[StreamObservation::Missing,StreamObservation::Done,StreamObservation::Eof,StreamObservation::ClientGone,StreamObservation::Other];
        f.native.metadata=match meta{0=>RelayMetadataState::Absent,1=>RelayMetadataState::Unusable,_=>RelayMetadataState::Parsed(RelayMetadata{status_code:Some(status),error_type:match error_type{0=>None,1=>Some("openai_error".into()),_=>Some("new_api_error".into())},error_code:match code{0=>None,1=>Some("unavailable".into()),2=>Some("context_length_exceeded".into()),_=>Some("permission_denied".into())},stream:streams[stream as usize],attempted_channels:None})};
        if !known{f.native.request_id=None;}
        let expected=if !known{None}else if success {
            if tokens>0&&((meta==0&&!is_stream)||(meta==2&&status<400&&error_type==0&&code==0&&((!is_stream&&stream==0)||(is_stream&&(1..=3).contains(&stream))))){Some(OutcomeKind::Success)}else{None}
        }else if meta==2&&error_type==1&&code!=2&&stream!=3&&(matches!(status,401|429|500|502|503|504)||(status==403&&code==3)){Some(OutcomeKind::Unavailable)}else{None};
        prop_assert_eq!(classify_observation(&f).unwrap().map(|c|c.kind),expected);
        // 每个随机 case 都另外进入正向主分支，不依赖多层布尔恰好碰中。
        for http in [401,429,500,502,503,504] {
            let mut upstream=fact(2,5,"positive-error",false);
            let RelayMetadataState::Parsed(ref mut m)=upstream.native.metadata else{unreachable!()};m.status_code=Some(http);
            prop_assert_eq!(classify_observation(&upstream).unwrap().unwrap().kind,OutcomeKind::Unavailable);
        }
        for flow in [None,Some(StreamObservation::Done),Some(StreamObservation::Eof),Some(StreamObservation::ClientGone)] {
            let mut active=fact(3,5,"positive-tokens",true);active.native.input=tokens+1;active.native.output=0;
            if let Some(stream)=flow{active.native.is_stream=true;active.native.metadata=RelayMetadataState::Parsed(RelayMetadata{stream,..metadata()});}
            prop_assert_eq!(classify_observation(&active).unwrap().unwrap().kind,OutcomeKind::Success);
        }
    }
    #[test]
    fn observed_pb2_request_source_replays_and_reordering_are_idempotent(events in prop::collection::vec((0i64..3,0u8..15,any::<bool>(),0i64..100),0..160)) {
        let mut actual=BTreeMap::new();let mut failed=BTreeSet::new();let mut succeeded=BTreeSet::new();let mut earliest=BTreeMap::new();
        for (src,req,success,at) in events {
            let key=(src,req);let expected=if succeeded.contains(&key){None}else if success{succeeded.insert(key);Some(OutcomeKind::Success)}else if failed.insert(key){Some(OutcomeKind::Unavailable)}else{None};
            let first=earliest.entry(key).or_insert(NOW+at);*first=(*first).min(NOW+at);
            let c=ObservationCandidate{source_id:source(src+1),request_id:format!("r{req}"),occurred_at:NOW+at,kind:if success{OutcomeKind::Success}else{OutcomeKind::Unavailable}};
            let d=observe_request(actual.get(&key),&c).unwrap();prop_assert_eq!(d.outcome.as_ref().map(|o|o.kind),expected);prop_assert_eq!(d.next.first_event_at,*first);
            prop_assert_eq!(d.next.phase,if succeeded.contains(&key){RequestObservationPhase::TokenFlow}else{RequestObservationPhase::Failed});
            if let Some(o)=&d.outcome{prop_assert_eq!(o.first_event_at,*first);prop_assert_eq!(o.observed_at,NOW+at);}
            actual.insert(key,d.next);
        }
    }
    #[test]
    fn observed_pb3_token_activity_and_source_counters_match_independent_model(events in prop::collection::vec((0i64..3,0u8..20,0u8..6),0..200)) {
        let mut requests=BTreeMap::new();let mut runtimes:BTreeMap<_,_>=(1..=3).map(|s|(source(s),runtime(s))).collect();
        let mut failed=BTreeSet::new();let mut success=BTreeSet::new();let mut counts=[0i64;3];let mut latched=[false;3];
        for (i,(s,req,event)) in events.into_iter().enumerate(){
            let key=(s,req);let active=event==1||event==4;let error=event==0||event==2;
            let expected=if success.contains(&key){None}else if active{success.insert(key);counts[s as usize]=0;Some(OutcomeKind::Success)}else if error&&failed.insert(key){counts[s as usize]+=1;latched[s as usize]|=counts[s as usize]>=3;Some(OutcomeKind::Unavailable)}else{None};
            let mut f=fact(i as i64+1,s+1,&format!("r{req}"),matches!(event,1|3|4));
            if event==3{f.native.input=0;f.native.output=0;}
            if event==4{f.native.is_stream=true;f.native.metadata=RelayMetadataState::Parsed(RelayMetadata{stream:StreamObservation::ClientGone,..metadata()});}
            if event==5{f.native.metadata=RelayMetadataState::Unusable;}
            prop_assert_eq!(feed(&mut requests,&mut runtimes,&f),expected);
            for n in 0..3{let r=&runtimes[&source(n+1)];prop_assert_eq!(r.consecutive_failures,counts[n as usize]);prop_assert_eq!(r.error_exhausted,latched[n as usize]);prop_assert_eq!(r.used_input+r.used_output,0);}
        }
    }
}

#[test]
#[ignore = "独立串行 release 性能门禁"]
fn performance_gate_observed_breaker_100k() {
    let mut f = fact(1, 5, &"r".repeat(MAX_REQUEST_ID_BYTES), true);
    f.native.metadata = RelayMetadataState::Parsed(RelayMetadata {
        attempted_channels: Some(vec![ChannelId::new(105).unwrap(); MAX_ATTEMPTS]),
        ..metadata()
    });
    let mut state = None;
    let start = Instant::now();
    for _ in 0..100000 {
        let c = classify_observation(std::hint::black_box(&f))
            .unwrap()
            .unwrap();
        let d = observe_request(state.as_ref(), &c).unwrap();
        state = Some(d.next);
    }
    let elapsed = start.elapsed();
    assert_eq!(state.unwrap().phase, RequestObservationPhase::TokenFlow);
    eprintln!("OBSERVED_BREAKER_100K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}
