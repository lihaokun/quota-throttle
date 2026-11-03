use super::*;
use crate::catalog::ledger::{LedgerState, ScanLane, ScanPhase, ScanProgress, ScanWindow};
use crate::ledger::analytics::*;
use crate::catalog::GroupId;
use crate::ledger::{add_usage, UsageTotals};
use proptest::prelude::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

fn state(initialized: bool) -> LedgerState {
    LedgerState {
        revision: 1,
        initial_requested_from: initialized.then_some(0),
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
fn query(hours: i64) -> UsageQuery {
    UsageQuery::new(
        ScanWindow {
            from: 0,
            to: hours * 3600,
        },
        480,
        UsageGrain::Hour,
        UsageFilter::default(),
    )
    .unwrap()
}
fn value(snapshot: UsageSnapshot) -> Value {
    let response = encode(AdminReply::Usage(snapshot)).ok().unwrap();
    let wire = serde_json::from_slice(&response.body).unwrap();
    crate::panel_http::admin::tests::support::emit("response_UsageSnapshot", &wire);
    wire
}
fn wire_totals(input: i64, output: i64, rows: i64) -> Value {
    json!({"input":input,"output":output,"total":input+output,"consumption_rows":rows})
}

#[test]
fn h2_usage_uninitialized_and_unknown_filters_remain_distinct() {
    let query = UsageQuery::new(
        ScanWindow { from: 0, to: 3600 },
        480,
        UsageGrain::Hour,
        UsageFilter {
            source: DimensionFilter::Unknown,
            group: DimensionFilter::Any,
            token: DimensionFilter::Exact(i64::MAX),
            model: DimensionFilter::Exact(" model ".into()),
        },
    )
    .unwrap();
    let wire = value(UsageSnapshot {
        query,
        state: state(false),
        usage: None,
    });
    assert_eq!(wire["usage"], Value::Null);
    assert_eq!(
        wire["query"]["filter"],
        json!({"source_id":{"mode":"unknown"},"group_id":{"mode":"any"},"native_token_id":{"mode":"exact","value":i64::MAX.to_string()},"model":{"mode":"exact","value":" model "}})
    );
    assert_eq!(wire["ledger"]["scans"][0]["lane"], "recent");
    assert_eq!(wire["ledger"]["scans"][1]["lane"], "history");
    assert!(wire.get("complete").is_none());
}

#[test]
fn h2_usage_group_breakdown_serializes_ids_as_strings_and_keeps_unattributed_null() {
    let part = |n| UsageTotals {
        input: n,
        output: 0,
        consumption_rows: 1,
    };
    let wire = value(UsageSnapshot {
        query: query(1),
        state: state(true),
        usage: Some(UsageReport {
            total: UsageTotals {
                input: 6,
                output: 0,
                consumption_rows: 2,
            },
            models: vec![(None, add_usage(part(1), part(5)).unwrap())],
            tokens: vec![(None, add_usage(part(1), part(5)).unwrap())],
            groups: vec![(None, part(1)), (Some(GroupId::new(i64::MAX).unwrap()), part(5))],
            buckets: vec![UsageBucket {
                from: 0,
                to: 3600,
                total: add_usage(part(1), part(5)).unwrap(),
                models: vec![(None, add_usage(part(1), part(5)).unwrap())],
            }],
        }),
    });
    assert_eq!(
        wire["usage"]["groups"],
        json!([{"group_id":null,"usage":wire_totals(1, 0, 1)},{"group_id":i64::MAX.to_string(),"usage":wire_totals(5, 0, 1)}])
    );
}

#[test]
fn h2_identity_page_exact_hundred_only_promises_a_continuation_cursor() {
    use crate::catalog::control::CreateState;
    use crate::catalog::tokens::TokenOrigin;
    use crate::newapi::tokens::TokenOwnerScope;
    for count in [0, 99, 100] {
        let after = Some(ManagedTokenId::new(7).unwrap());
        let items = (0..count)
            .map(|i| TokenIdentityView {
                schema_version: 1,
                token_id: ManagedTokenId::new(i + 8).unwrap(),
                native_token_id: Some((i + 1000).to_string()),
                owner_user_id: "41".into(),
                owner_scope: TokenOwnerScope::Issuer,
                origin: TokenOrigin::Created,
                group_id: Some(crate::catalog::GroupId::new(1).unwrap()),
                explicit_group: "managed".into(),
                display_name: "key".into(),
                revision: 1,
                create_state: CreateState::Resolved,
                created_at: 1,
                deleted_at: if i % 2 == 0 { Some(2) } else { None },
            })
            .collect();
        let response = encode(AdminReply::Identities { items, after })
            .ok()
            .unwrap();
        let wire: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(wire["after"], "7");
        assert_eq!(wire["items"].as_array().unwrap().len(), count as usize);
        assert_eq!(
            wire["next_after"],
            if count == 100 {
                json!("107")
            } else {
                Value::Null
            }
        );
        assert!(wire.get("total").is_none());
        crate::panel_http::admin::tests::support::emit("response_IdentityPage", &wire);
    }
}

fn large_snapshot(label_bytes: usize) -> UsageSnapshot {
    let labels: Vec<_> = (0..1024)
        .map(|i| Some(format!("{i:04}{}", "\"\\".repeat((label_bytes - 4) / 2))))
        .collect();
    let cell = UsageTotals {
        input: 1,
        output: 1,
        consumption_rows: 1,
    };
    UsageSnapshot {
        query: query(64),
        state: state(true),
        usage: Some(UsageReport {
            total: UsageTotals {
                input: 65536,
                output: 65536,
                consumption_rows: 65536,
            },
            models: labels
                .iter()
                .map(|model| {
                    (
                        model.clone(),
                        UsageTotals {
                            input: 64,
                            output: 64,
                            consumption_rows: 64,
                        },
                    )
                })
                .collect(),
            tokens: vec![(
                None,
                UsageTotals {
                    input: 65536,
                    output: 65536,
                    consumption_rows: 65536,
                },
            )],
            groups: vec![(
                None,
                UsageTotals {
                    input: 65536,
                    output: 65536,
                    consumption_rows: 65536,
                },
            )],
            buckets: (0..64)
                .map(|b| UsageBucket {
                    from: b * 3600,
                    to: (b + 1) * 3600,
                    total: UsageTotals {
                        input: 1024,
                        output: 1024,
                        consumption_rows: 1024,
                    },
                    models: labels.iter().map(|model| (model.clone(), cell)).collect(),
                })
                .collect(),
        }),
    }
}
#[test]
fn h2_usage_legitimate_escaped_maximum_fails_whole_response_without_truncation() {
    let snapshot = large_snapshot(256);
    assert!(snapshot
        .usage
        .as_ref()
        .unwrap()
        .models
        .iter()
        .all(|(m, _)| crate::ledger::valid_label(m.as_deref().unwrap())));
    let Err(error) = encode(AdminReply::Usage(snapshot)) else {
        panic!("oversized usage accepted");
    };
    assert_eq!(
        serde_json::to_value(error).unwrap()["error"],
        "response_too_large"
    );
}
#[test]
fn h2_usage_json_safe_overflow_returns_error_not_wrapped_total() {
    let mut snapshot = large_snapshot(16);
    snapshot.usage.as_mut().unwrap().total = UsageTotals {
        input: crate::catalog::JSON_SAFE,
        output: 1,
        consumption_rows: 1,
    };
    let Err(error) = encode(AdminReply::Usage(snapshot)) else {
        panic!("overflow accepted");
    };
    assert_eq!(
        serde_json::to_value(error).unwrap()["error"],
        "arithmetic_overflow"
    );
}
#[test]
#[ignore = "independent release performance gate"]
fn performance_gate_admin_usage_encoding() {
    // runner以--release串行执行；普通suite忽略本独立墙钟门禁。
    let snapshot = large_snapshot(128);
    let started = Instant::now();
    let response = encode(AdminReply::Usage(snapshot)).ok().unwrap();
    let elapsed = started.elapsed();
    eprintln!(
        "ADMIN_USAGE_BYTES={} ADMIN_USAGE_MS={}",
        response.body.len(),
        elapsed.as_millis()
    );
    assert!(response.body.len() < 32 * 1024 * 1024);
    assert!(elapsed <= Duration::from_secs(2));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb5_admin_usage_preserves_joint_buckets_models_tokens_and_unknown(rows in prop::collection::vec((prop::option::of(0u8..4),prop::option::of(1u8..5),0usize..4,0i64..1000,0i64..1000),0..80)) {
        let control = QueryControl::new(Instant::now()+Duration::from_secs(60));
        let mut aggregate = UsageAccumulator::new(query(4),&control).unwrap();
        let mut total = (0i64,0i64,0i64);
        let mut models = BTreeMap::<Option<String>,(i64,i64,i64)>::new();
        let mut tokens = BTreeMap::<Option<i64>,(i64,i64,i64)>::new();
        let mut buckets = [0,1,2,3].map(|_| BTreeMap::<Option<String>,(i64,i64,i64)>::new());
        for (model, token, bucket, input, output) in rows {
            let label = model.map(|m| format!("model-{m}"));
            let token = token.map(i64::from);
            aggregate.push(UsageDimensions { source: None, group: None, token, model: label.as_deref() },bucket as i64*3600+1,UsageTotals { input, output, consumption_rows: 1 },&control).unwrap();
            for t in [&mut total,models.entry(label.clone()).or_default(),tokens.entry(token).or_default(),buckets[bucket].entry(label).or_default()] { t.0+=input; t.1+=output; t.2+=1; }
        }
        let wire = value(aggregate.finish(state(true),&control).unwrap());
        let report = &wire["usage"];
        prop_assert_eq!(&report["total"],&wire_totals(total.0,total.1,total.2));
        let expected_models: Vec<_> = models.iter().map(|(label,t)| json!({"model":label,"usage":wire_totals(t.0,t.1,t.2)})).collect();
        let expected_tokens: Vec<_> = tokens.iter().map(|(token,t)| json!({"native_token_id":token.map(|id|id.to_string()),"usage":wire_totals(t.0,t.1,t.2)})).collect();
        prop_assert_eq!(&report["models"],&json!(expected_models));
        prop_assert_eq!(&report["tokens"],&json!(expected_tokens));
        for (i, expected) in buckets.into_iter().enumerate() {
            let total = expected.values().fold((0,0,0),|a,t|(a.0+t.0,a.1+t.1,a.2+t.2));
            let expected: Vec<_> = expected.iter().map(|(label,t)|json!({"model":label,"usage":wire_totals(t.0,t.1,t.2)})).collect();
            prop_assert_eq!(&report["buckets"][i]["models"],&json!(expected));
            prop_assert_eq!(&report["buckets"][i]["total"],&wire_totals(total.0,total.1,total.2));
            prop_assert_eq!(report["buckets"][i]["from"].as_i64(),Some(i as i64*3600));
        }
    }
}
