use super::*;
use crate::catalog::{GroupId, SourceId};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

mod properties;

fn row(id: i64) -> Value {
    json!({"id":id,"type":2,"created_at":125,"channel":11,"token_id":31,
        "model_name":"model-a","group":"g-a","prompt_tokens":5,"completion_tokens":3,
        "is_stream":false,"request_id":"same_request","other":""})
}
fn fact(id: i64) -> NativeFact {
    decode_native_row(&row(id)).unwrap().unwrap()
}
fn history(channel: i64, source: i64, group: i64, name: &str) -> ChannelAttribution {
    ChannelAttribution {
        channel_id: ChannelId::new(channel).unwrap(),
        source_id: SourceId::new(source).unwrap(),
        group_id: GroupId::new(group).unwrap(),
        native_group: name.into(),
    }
}
fn expect<T>(value: Result<T>, expected: FactError) {
    match value {
        Err(e) => assert_eq!(e, expected),
        Ok(_) => panic!("expected {expected}"),
    }
}
fn usage(rows: &[Value], map: &[ChannelAttribution]) -> UsageDelta {
    usage_delta(&attribute_batch(&decode_native_batch(rows).unwrap(), map).unwrap()).unwrap()
}
fn set_other(row: &mut Value, value: Value) {
    row["other"] = Value::String(value.to_string());
}
fn totals(input: i64, output: i64, consumption_rows: i64) -> UsageTotals {
    UsageTotals {
        input,
        output,
        consumption_rows,
    }
}

#[test]
fn private_metadata_has_no_debug_display_or_wire_traits() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    macro_rules! private { ($($ty:ty),+) => { $(
        assert_not_impl!($ty, std::fmt::Debug);
        assert_not_impl!($ty, std::fmt::Display);
        assert_not_impl!($ty, serde::Serialize);
        assert_not_impl!($ty, serde::Deserialize<'static>);
    )+ }; }
    private!(
        NativeFact,
        RelayMetadata,
        RelayMetadataState,
        ChannelAttribution,
        AttributedFact,
        UsageCellKey,
        UsageDelta
    );
}

#[test]
fn core_fields_are_required_strict_and_bounded() {
    expect(decode_native_row(&json!([])), FactError::InvalidShape);
    for key in [
        "id",
        "type",
        "created_at",
        "channel",
        "token_id",
        "prompt_tokens",
        "completion_tokens",
        "is_stream",
    ] {
        let mut r = row(1);
        r.as_object_mut().unwrap().remove(key);
        expect(decode_native_row(&r), FactError::InvalidShape);
    }
    for key in [
        "id",
        "type",
        "created_at",
        "channel",
        "token_id",
        "prompt_tokens",
        "completion_tokens",
    ] {
        for invalid in [
            json!(null),
            json!(true),
            json!("5"),
            json!(5.0),
            json!(-1),
            json!(u64::MAX),
        ] {
            let mut r = row(1);
            r[key] = invalid;
            expect(decode_native_row(&r), FactError::InvalidNumber);
        }
    }
    let mut r = row(1);
    r["id"] = json!(0);
    expect(decode_native_row(&r), FactError::InvalidNumber);
    r = row(i64::MAX);
    r["channel"] = json!(i64::MAX);
    r["token_id"] = json!(i64::MAX);
    assert!(decode_native_row(&r).is_ok());
    for key in ["created_at", "prompt_tokens", "completion_tokens"] {
        let mut r = row(1);
        r[key] = json!(JSON_SAFE + 1);
        expect(decode_native_row(&r), FactError::InvalidNumber);
    }
    r = row(1);
    r["prompt_tokens"] = json!(JSON_SAFE);
    expect(decode_native_row(&r), FactError::InvalidNumber);
    r["completion_tokens"] = json!(0);
    assert!(decode_native_row(&r).is_ok());
    r["is_stream"] = json!(1);
    expect(decode_native_row(&r), FactError::InvalidShape);
}

#[test]
fn error_and_unrelated_rows_cannot_become_consumption() {
    let mut error = row(2);
    error["type"] = json!(5);
    expect(decode_native_row(&error), FactError::InvalidNumber);
    error["prompt_tokens"] = json!(0);
    error["completion_tokens"] = json!(0);
    assert_eq!(usage(&[error.clone()], &[]).total, totals(0, 0, 0));
    assert!(usage(&[error], &[]).cells.is_empty());
    for kind in [0, 1, 3, 4, 6, 7, 999] {
        assert!(decode_native_row(&json!({"type":kind})).unwrap().is_none());
    }
    assert!(decode_native_batch(&[]).unwrap().is_empty());
    assert!(usage(&[], &[]).cells.is_empty());
    let mut zero = row(1);
    zero["prompt_tokens"] = json!(0);
    zero["completion_tokens"] = json!(0);
    let delta = usage(&[zero], &[]);
    assert_eq!(delta.total, totals(0, 0, 1));
    assert_eq!(delta.cells.len(), 1);
}

#[test]
fn label_degradation_keeps_usage_and_other_valid_dimensions() {
    for key in ["model_name", "group"] {
        for value in [
            json!(true),
            json!(["x"]),
            json!("x".repeat(257)),
            json!("坏\n标签"),
            json!("é".repeat(129)),
        ] {
            let mut r = row(1);
            r[key] = value;
            let f = decode_native_row(&r).unwrap().unwrap();
            assert!(matches!(f.metadata, RelayMetadataState::Unusable));
            if key == "model_name" {
                assert!(f.model.is_none());
                assert_eq!(f.native_group.as_deref(), Some("g-a"));
            } else {
                assert!(f.native_group.is_none());
                assert_eq!(f.model.as_deref(), Some("model-a"));
            }
            assert_eq!(usage(&[r], &[]).total, totals(5, 3, 1));
        }
        for value in [Value::Null, json!("")] {
            let mut r = row(1);
            r[key] = value;
            assert!(matches!(
                decode_native_row(&r).unwrap().unwrap().metadata,
                RelayMetadataState::Absent
            ));
        }
        let mut r = row(1);
        r[key] = json!("é".repeat(128));
        assert!(matches!(
            decode_native_row(&r).unwrap().unwrap().metadata,
            RelayMetadataState::Absent
        ));
        r.as_object_mut().unwrap().remove(key);
        assert_eq!(usage(&[r], &[]).total, totals(5, 3, 1));
    }
    let mut r = row(1);
    r["group"] = json!("auto");
    r["model_name"] = json!("  GLM/模型  ");
    let f = decode_native_row(&r).unwrap().unwrap();
    assert_eq!(f.model.as_deref(), Some("  GLM/模型  "));
    assert_eq!(f.native_group.as_deref(), Some("auto"));
}

#[test]
fn request_ids_are_optional_bounded_and_not_synthesized() {
    for id in [
        "202610160620223895527848268d9d63Z4YUsH2".to_owned(),
        "20261016062022389552784AbCdEfG1A1b2C3d4".to_owned(),
        "x_-".repeat(21) + "x",
    ] {
        let mut r = row(1);
        r["request_id"] = json!(id);
        assert_eq!(decode_native_row(&r).unwrap().unwrap().request_id, Some(id));
    }
    for value in [
        json!("x".repeat(65)),
        json!("请求"),
        json!("bad id"),
        json!(1),
    ] {
        let mut r = row(1);
        r["request_id"] = value;
        let f = decode_native_row(&r).unwrap().unwrap();
        assert!(f.request_id.is_none() && matches!(f.metadata, RelayMetadataState::Unusable));
        assert_eq!(usage(&[r], &[]).total, totals(5, 3, 1));
    }
    let mut r = row(1);
    r.as_object_mut().unwrap().remove("request_id");
    assert!(decode_native_row(&r).unwrap().unwrap().request_id.is_none());
}

#[test]
fn metadata_boundaries_are_degraded_not_silently_counted() {
    let mut r = row(1);
    r["other"] = json!(format!("{{}}{}", " ".repeat(MAX_OTHER_BYTES - 2)));
    assert!(matches!(
        decode_native_row(&r).unwrap().unwrap().metadata,
        RelayMetadataState::Parsed(_)
    ));
    for other in [
        json!(" ".repeat(MAX_OTHER_BYTES + 1)),
        json!("{"),
        json!("[]"),
        json!({}),
        json!(7),
    ] {
        r["other"] = other;
        assert!(matches!(
            decode_native_row(&r).unwrap().unwrap().metadata,
            RelayMetadataState::Unusable
        ));
        assert_eq!(usage(&[r.clone()], &[]).total, totals(5, 3, 1));
    }
    set_other(
        &mut r,
        json!({"admin_info":{"use_channel":vec!["11";128]},"error_code":"x".repeat(128),"status_code":599}),
    );
    let RelayMetadataState::Parsed(m) = decode_native_row(&r).unwrap().unwrap().metadata else {
        panic!()
    };
    assert_eq!(m.attempted_channels.unwrap().len(), 128);
    assert_eq!(m.status_code, Some(599));
    for malformed in [
        json!({"admin_info":{"use_channel":vec!["11";129]}}),
        json!({"admin_info":{"use_channel":[11]}}),
        json!({"admin_info":{"use_channel":["0"]}}),
        json!({"admin_info":{"use_channel":["-1"]}}),
        json!({"admin_info":{"use_channel":["+1"]}}),
        json!({"admin_info":{"use_channel":["9223372036854775808"]}}),
        json!({"admin_info":false}),
        json!({"status_code":99}),
        json!({"status_code":600}),
        json!({"status_code":"503"}),
        json!({"error_code":"x".repeat(129)}),
        json!({"error_type":"bad\n"}),
        json!({"error_code":7}),
        json!({"stream_status":[]}),
        json!({"stream_status":{"status":true}}),
    ] {
        set_other(&mut r, malformed);
        assert!(matches!(
            decode_native_row(&r).unwrap().unwrap().metadata,
            RelayMetadataState::Unusable
        ));
        assert_eq!(usage(&[r.clone()], &[]).total, totals(5, 3, 1));
    }
}

#[test]
fn stream_and_retry_metadata_never_reclassify_usage() {
    for (stream, expected) in [
        (
            json!({"status":"ok","end_reason":"done"}),
            StreamObservation::Done,
        ),
        (
            json!({"status":"ok","end_reason":"eof"}),
            StreamObservation::Eof,
        ),
        (
            json!({"status":"error","end_reason":"client_gone","end_error":"PRIVATE_CONTEXT"}),
            StreamObservation::ClientGone,
        ),
        (
            json!({"status":"ok","end_reason":"new_reason"}),
            StreamObservation::Other,
        ),
        (json!({}), StreamObservation::Other),
        (json!(null), StreamObservation::Missing),
    ] {
        let mut r = row(1);
        r["is_stream"] = json!(true);
        set_other(
            &mut r,
            json!({"stream_status":stream,"admin_info":{"use_channel":["11","11"]},"cache_tokens":500,"quota":999}),
        );
        let f = decode_native_row(&r).unwrap().unwrap();
        let RelayMetadataState::Parsed(m) = f.metadata else {
            panic!()
        };
        assert_eq!(m.stream, expected);
        assert_eq!(
            m.attempted_channels.unwrap(),
            vec![ChannelId::new(11).unwrap(); 2]
        );
        assert_eq!(usage(&[r], &[]).total, totals(5, 3, 1));
    }
    let mut error = row(1);
    error["type"] = json!(5);
    error["prompt_tokens"] = json!(0);
    error["completion_tokens"] = json!(0);
    assert_eq!(usage(&[error, row(2)], &[]).total, totals(5, 3, 1));
    assert_eq!(usage(&[row(1), row(2)], &[]).total, totals(10, 6, 2)); // 同request也不是同消费记录。
}

#[test]
fn duplicate_conflicts_check_every_canonical_field() {
    let original = row(1);
    assert_eq!(
        decode_native_batch(&[original.clone(), original.clone()])
            .unwrap()
            .len(),
        1
    );
    for (key, value) in [
        ("created_at", json!(126)),
        ("channel", json!(12)),
        ("token_id", json!(32)),
        ("model_name", json!("other")),
        ("group", json!("other")),
        ("prompt_tokens", json!(6)),
        ("completion_tokens", json!(4)),
        ("is_stream", json!(true)),
        ("request_id", json!("other")),
        ("other", json!("{}")),
    ] {
        let mut changed = original.clone();
        changed[key] = value;
        expect(
            decode_native_batch(&[original.clone(), changed]),
            FactError::ConflictingIdentity,
        );
    }
    let mut zero = row(1);
    zero["prompt_tokens"] = json!(0);
    zero["completion_tokens"] = json!(0);
    let mut error = zero.clone();
    error["type"] = json!(5);
    expect(
        decode_native_batch(&[zero, error]),
        FactError::ConflictingIdentity,
    );
    let mut extra = original.clone();
    extra["content"] = json!("PRIVATE_TEXT");
    extra["ip"] = json!("PRIVATE_IP");
    assert_eq!(decode_native_batch(&[original, extra]).unwrap().len(), 1);
}

#[test]
fn history_validation_precedes_filtering_and_preserves_unknown_groups() {
    let map = history(11, 701, 901, "g-a");
    expect(
        attribute_batch(&[], &[map.clone(), map.clone()]),
        FactError::InvalidAttribution,
    );
    let mut bad = history(999, 702, 902, "");
    expect(
        attribute_batch(&[fact(1)], &[map.clone(), bad.clone()]),
        FactError::InvalidAttribution,
    );
    bad.native_group = "x".repeat(257);
    expect(attribute_batch(&[], &[bad]), FactError::InvalidAttribution);
    let rows = [
        fact(1),
        {
            let mut f = fact(2);
            f.native_group = None;
            f
        },
        {
            let mut f = fact(3);
            f.native_group = Some("external".into());
            f
        },
        {
            let mut f = fact(4);
            f.channel_id = None;
            f
        },
    ];
    let attributed = attribute_batch(&rows, &[map]).unwrap();
    assert_eq!(attributed[0].group_id, Some(GroupId::new(901).unwrap()));
    for f in &attributed[1..3] {
        assert_eq!(f.source_id, Some(SourceId::new(701).unwrap()));
        assert_eq!(f.group_id, None);
        assert_eq!(f.issue, AttributionIssue::GroupUnresolved);
    }
    assert_eq!(attributed[3].issue, AttributionIssue::UnmappedChannel);
    assert_eq!(usage_delta(&attributed).unwrap().total, totals(20, 12, 4));
}

#[test]
fn handmade_facts_and_attributions_cannot_bypass_checks() {
    let mut bads = Vec::new();
    let mut f = fact(1);
    f.log_id = 0;
    bads.push(f);
    let mut f = fact(1);
    f.occurred_at = -1;
    bads.push(f);
    let mut f = fact(1);
    f.token_id = Some(0);
    bads.push(f);
    let mut f = fact(1);
    f.model = Some("".into());
    bads.push(f);
    let mut f = fact(1);
    f.native_group = Some("bad\n".into());
    bads.push(f);
    let mut f = fact(1);
    f.request_id = Some("bad id".into());
    bads.push(f);
    let mut f = fact(1);
    f.metadata = RelayMetadataState::Parsed(RelayMetadata {
        status_code: Some(600),
        error_type: None,
        error_code: None,
        attempted_channels: None,
        stream: StreamObservation::Missing,
    });
    bads.push(f);
    for bad in bads {
        assert!(attribute_batch(std::slice::from_ref(&bad), &[]).is_err());
        assert!(usage_delta(&[AttributedFact {
            native: bad,
            source_id: None,
            group_id: None,
            issue: AttributionIssue::UnmappedChannel
        }])
        .is_err());
    }
    expect(
        attribute_batch(&[fact(1), fact(1)], &[]),
        FactError::ConflictingIdentity,
    );
    let attrs = attribute_batch(&[fact(1)], &[]).unwrap();
    expect(
        usage_delta(&[attrs[0].clone(), attrs[0].clone()]),
        FactError::ConflictingIdentity,
    );
    for (source, group, issue, channel) in [
        (None, Some(901), AttributionIssue::UnmappedChannel, true),
        (Some(701), None, AttributionIssue::None, true),
        (None, None, AttributionIssue::GroupUnresolved, true),
        (Some(701), Some(901), AttributionIssue::None, false),
        (
            Some(701),
            Some(901),
            AttributionIssue::GroupUnresolved,
            true,
        ),
    ] {
        let mut f = fact(1);
        if !channel {
            f.channel_id = None;
        }
        expect(
            usage_delta(&[AttributedFact {
                native: f,
                source_id: source.map(|n| SourceId::new(n).unwrap()),
                group_id: group.map(|n| GroupId::new(n).unwrap()),
                issue,
            }]),
            FactError::InvalidAttribution,
        );
    }
}

#[test]
fn overflow_in_late_cell_returns_no_partial_delta() {
    expect(
        add_usage(totals(JSON_SAFE, 0, 0), totals(0, 1, 0)),
        FactError::ArithmeticOverflow,
    );
    expect(
        add_usage(totals(0, 0, JSON_SAFE), totals(0, 0, 1)),
        FactError::ArithmeticOverflow,
    );
    expect(
        add_usage(totals(-1, 0, 0), totals(0, 0, 0)),
        FactError::ArithmeticOverflow,
    );
    let mut first = row(1);
    first["prompt_tokens"] = json!(JSON_SAFE);
    first["completion_tokens"] = json!(0);
    let mut second = row(2);
    second["model_name"] = json!("different-cell");
    let rows = attribute_batch(&decode_native_batch(&[first, second]).unwrap(), &[]).unwrap();
    expect(usage_delta(&rows), FactError::ArithmeticOverflow);
    assert_eq!(rows[0].native.input, JSON_SAFE);
}

fn maximum_case() -> Duration {
    let mut metadata = json!({"status_code":503,"error_type":"e".repeat(MAX_TAG_BYTES),
        "error_code":"c".repeat(MAX_TAG_BYTES),"admin_info":{"use_channel":vec![i64::MAX.to_string();MAX_ATTEMPTS]},
        "stream_status":{"status":"ok","end_reason":"done"},"padding":""});
    let padding = MAX_OTHER_BYTES - metadata.to_string().len();
    metadata["padding"] = json!("x".repeat(padding));
    let encoded = metadata.to_string();
    assert_eq!(encoded.len(), MAX_OTHER_BYTES);
    let group_name = |i| format!("g-{i:05}-{}", "g".repeat(248));
    let rows = (1..=MAX_FACTS as i64)
        .map(|i| {
            let mut r = row(i);
            r["channel"] = json!(i);
            r["token_id"] = json!(i + 30);
            r["created_at"] = json!(i * 60);
            r["group"] = json!(group_name(i));
            r["model_name"] = json!(format!("model-{i:04}-{}", "m".repeat(245)));
            r["other"] = json!(encoded);
            r
        })
        .collect::<Vec<_>>();
    let mapping = (1..=MAX_HISTORY as i64)
        .map(|i| history(i, 50_000 + i, 80_000 + i, &group_name(i)))
        .collect::<Vec<_>>();
    let start = Instant::now();
    let facts = decode_native_batch(&rows).unwrap();
    let attrs = attribute_batch(&facts, &mapping).unwrap();
    let delta = usage_delta(&attrs).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(
        delta.total,
        totals(5 * MAX_FACTS as i64, 3 * MAX_FACTS as i64, MAX_FACTS as i64)
    );
    assert_eq!(delta.cells.len(), MAX_FACTS);
    assert!(facts
        .iter()
        .all(|f| matches!(&f.metadata,RelayMetadataState::Parsed(m)
        if m.attempted_channels.as_ref().is_some_and(|v|v.len()==MAX_ATTEMPTS))));
    let mut too_many = rows;
    too_many.push(row(99_999));
    expect(decode_native_batch(&too_many), FactError::LimitExceeded);
    let mut too_many = mapping;
    too_many.push(history(99_999, 199_999, 299_999, "g-a"));
    expect(attribute_batch(&[], &too_many), FactError::LimitExceeded);
    let too_many = vec![fact(1); MAX_FACTS + 1];
    expect(attribute_batch(&too_many, &[]), FactError::LimitExceeded);
    expect(
        usage_delta(&vec![attrs[0].clone(); MAX_FACTS + 1]),
        FactError::LimitExceeded,
    );
    elapsed
}
#[test]
fn maximum_batch_and_history_are_bounded_without_truncation() {
    maximum_case();
}
#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_ledger_maximum_batch() {
    let elapsed = maximum_case();
    println!("LEDGER_MAXIMUM_BATCH_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}
