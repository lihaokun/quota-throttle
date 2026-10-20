use super::*;
use crate::catalog::control::{ControlErrorCode, CreateState, StatusDirective};
use crate::catalog::panel::{PanelGroup, PanelMembership, PanelSource, VolcanoPanel};
use crate::catalog::*;
use crate::reconcile::worker::panel::{
    GlmGroupAssessment, GlmQuotaView, ObservationState, WindowView,
};

// 大目录使用实际可成立的关联关系与长数值；独立字段宽度上界另证，不冒充所有路由状态的基准。
const AT: i64 = JSON_SAFE - 86_400_000;

fn maximum(mode: u8) -> PanelSnapshot {
    let mut panel = empty(AT);
    let base = i64::MAX - 20_000;
    let kinds: Vec<_> = (0..MAX_SOURCES)
        .map(|i| {
            if mode == 0 || mode == 2 && i < MAX_SOURCES / 2 {
                SourceKind::Glm
            } else {
                SourceKind::Volcano
            }
        })
        .collect();
    for (i, kind) in kinds.iter().copied().enumerate() {
        let source_id = SourceId::new(base + i as i64).unwrap();
        let volcano = if kind == SourceKind::Volcano {
            let policy = VolcanoPolicy {
                daily_token_limit: Some(JSON_SAFE),
                failure_threshold: 100,
                reset_minute: 1439,
                utc_offset_minutes: 840,
            };
            let mut runtime =
                crate::source_policy::volcano::initialize_runtime(source_id, &policy, AT / 1000)
                    .unwrap();
            runtime.revision = JSON_SAFE;
            runtime.period_id = JSON_SAFE;
            runtime.used_input = JSON_SAFE - 1;
            runtime.used_output = 1;
            runtime.consecutive_failures = JSON_SAFE;
            runtime.error_exhausted = true;
            runtime.limit_exhausted = true;
            runtime.coverage_end = Some(AT / 1000);
            runtime.validate(&policy).unwrap();
            Some(VolcanoPanel {
                policy,
                runtime: Some(runtime.into()),
            })
        } else {
            None
        };
        panel.catalog.sources.push(PanelSource {
            source: SourceView {
                source_id,
                kind,
                name: "\u{1}".repeat(128),
                note: "\u{1}".repeat(2048),
                manual_paused: false,
                retired_at: None,
                config_revision: JSON_SAFE,
            },
            retire_requested: false,
            model_count: 1000,
            volcano,
        });
        if kind == SourceKind::Glm {
            panel.glm.sources.push(GlmQuotaView {
                source_id,
                source_revision: JSON_SAFE,
                observed_at_ms: Some(AT),
                state: ObservationState::Observed,
                five_hour: Some(WindowView {
                    percentage: f64::MIN_POSITIVE,
                    reset_at_ms: Some(AT + 600_000),
                }),
                weekly: Some(WindowView {
                    percentage: f64::MIN_POSITIVE,
                    reset_at_ms: Some(AT + 600_000),
                }),
                level: Some("\u{1}".repeat(128)),
            });
        }
    }
    for group_index in 0..MAX_GROUPS {
        let kind = if mode == 0 || mode == 2 && group_index % 2 == 0 {
            SourceKind::Glm
        } else {
            SourceKind::Volcano
        };
        let group_id = GroupId::new(base + group_index as i64).unwrap();
        let available: Vec<_> = panel
            .catalog
            .sources
            .iter()
            .filter(|s| s.source.kind == kind)
            .map(|s| s.source.source_id)
            .collect();
        let mut ids = Vec::new();
        for k in 0..MAX_MEMBERSHIPS / MAX_GROUPS {
            let index = group_index * (MAX_MEMBERSHIPS / MAX_GROUPS) + k;
            let member_id = MembershipId::new(base + index as i64).unwrap();
            ids.push(member_id);
            let priority = if k == 0 { JSON_SAFE } else { -JSON_SAFE };
            panel.catalog.memberships.push(PanelMembership {
                membership: MembershipView {
                    membership_id: member_id,
                    source_id: available[index % available.len()],
                    group_id,
                    channel_id: Some(ChannelId::new(base + index as i64).unwrap()),
                    order: JSON_SAFE - 100 + k as i64,
                    lifecycle: MembershipLifecycle::Ready,
                    desired_priority: priority,
                    observed_status: Some(if kind == SourceKind::Glm { 1 } else { 2 }),
                    observed_priority: Some(priority),
                    applied_revision: Some(JSON_SAFE),
                    retired_at: None,
                },
                control_revision: JSON_SAFE,
                status_directive: if kind == SourceKind::Glm {
                    StatusDirective::Preserve
                } else {
                    StatusDirective::Disabled
                },
                retire_requested: false,
                create_state: CreateState::Resolved,
                dirty: false,
                next_attempt_at: JSON_SAFE,
                error_code: Some(ControlErrorCode::TransportUnknown),
            });
        }
        panel.catalog.groups.push(PanelGroup {
            group: GroupView {
                group_id,
                newapi_group: format!("g{group_index:03}{}", "a".repeat(60)),
                name: "\u{1}".repeat(128),
                kind,
                revision: JSON_SAFE,
                ready: true,
            },
            policy_revision: JSON_SAFE,
            selected_member_id: if kind == SourceKind::Glm {
                Some(ids[0])
            } else {
                None
            },
            pinned_member_id: if kind == SourceKind::Glm {
                Some(ids[0])
            } else {
                None
            },
            regime: if kind == SourceKind::Glm {
                Some("normal")
            } else {
                None
            },
        });
        if kind == SourceKind::Glm {
            panel.glm.groups.push(GlmGroupAssessment {
                group_id,
                cycle_id: Some(u64::MAX.to_string()),
                evaluated_at_ms: Some(AT),
                current: true,
                imminent_members: Some(ids),
                pin_release: None,
            });
        }
    }
    panel.glm.cycle_id = Some(u64::MAX.to_string());
    panel.glm.completed_at_ms = Some(AT);
    panel
}

fn check_maximum(mode: u8) -> Duration {
    let panel = maximum(mode);
    assert_eq!(panel.catalog.sources.len(), MAX_SOURCES);
    assert_eq!(panel.catalog.groups.len(), MAX_GROUPS);
    assert_eq!(panel.catalog.memberships.len(), MAX_MEMBERSHIPS);
    let mut pairs = std::collections::BTreeSet::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut owners = std::collections::BTreeMap::new();
    for member in &panel.catalog.memberships {
        assert!(pairs.insert((member.membership.source_id, member.membership.group_id)));
        assert!(ids.insert(member.membership.membership_id));
        owners.insert(member.membership.membership_id, member.membership.group_id);
        assert!(serde_json::to_vec(member).unwrap().len() <= 1024);
    }
    for source in &panel.catalog.sources {
        let quota = panel
            .glm
            .sources
            .iter()
            .find(|q| q.source_id == source.source.source_id);
        let size = serde_json::to_vec(source).unwrap().len()
            + quota
                .map(|q| serde_json::to_vec(q).unwrap().len())
                .unwrap_or(0);
        assert!(size <= 16 * 1024);
    }
    let mut imminent = std::collections::BTreeSet::new();
    for group in &panel.catalog.groups {
        let mut size = serde_json::to_vec(group).unwrap().len();
        if let Some(g) = panel
            .glm
            .groups
            .iter()
            .find(|g| g.group_id == group.group.group_id)
        {
            for id in g.imminent_members.as_ref().unwrap() {
                assert!(imminent.insert(*id));
                assert_eq!(owners.get(id), Some(&g.group_id));
            }
            let mut without = g.clone();
            without.imminent_members = Some(vec![]);
            size += serde_json::to_vec(&without).unwrap().len();
        }
        assert!(size <= 2 * 1024);
    }
    assert!(imminent.len() <= MAX_MEMBERSHIPS);
    let start = std::time::Instant::now();
    let encoded = encode_panel(&panel).unwrap();
    let elapsed = start.elapsed();
    assert!(encoded.len() < 27_114_336);
    assert!(encoded.len() < MAX_RESPONSE);
    assert_eq!(encoded.as_ref(), serde_json::to_vec(&panel).unwrap());
    println!(
        "HTTP_MAXIMUM_JSON_BYTES={} HTTP_MAXIMUM_JSON_MS={}",
        encoded.len(),
        elapsed.as_millis()
    );
    elapsed
}

#[test]
fn h1_maximum_valid_domain_fits_each_published_capacity_budget() {
    for mode in 0..3 {
        check_maximum(mode);
    }
}

// 仅计算字段宽度上界：互斥状态的最大值可叠加以保守估算，不作为合法快照/schema正例。
fn field_width_bound(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    match value {
        Value::Null => 4,
        Value::Bool(_) => 5,
        Value::Number(_) => 32, // i64/u64及最短有限f64十进制编码的保守界限。
        Value::String(s) => serde_json::to_vec(s).unwrap().len(),
        Value::Array(v) => {
            2 + v.len().saturating_sub(1) + v.iter().map(field_width_bound).sum::<usize>()
        }
        Value::Object(v) => {
            2 + v.len().saturating_sub(1)
                + v.iter()
                    .map(|(k, v)| serde_json::to_vec(k).unwrap().len() + 1 + field_width_bound(v))
                    .sum::<usize>()
        }
    }
}
#[test]
fn h1_field_width_bounds_cover_optional_states_numbers_and_all_shared_enums() {
    use serde_json::json;
    let glm = maximum(0);
    let volcano = maximum(1);
    let mut source = serde_json::to_value(&volcano.catalog.sources[0]).unwrap();
    source["source"]["retired_at"] = json!(JSON_SAFE);
    let mut quota = serde_json::to_value(&glm.glm.sources[0]).unwrap();
    quota["observed_at_ms"] = json!(JSON_SAFE);
    quota["state"] = serde_json::to_value(ObservationState::Unavailable).unwrap();
    for w in ["five_hour", "weekly"] {
        quota[w]["percentage"] = json!(f64::MIN_POSITIVE);
        quota[w]["reset_at_ms"] = json!(JSON_SAFE);
    }
    let source_width = field_width_bound(&source) + field_width_bound(&quota);
    assert!(source_width <= 16 * 1024);
    let mut member = serde_json::to_value(&glm.catalog.memberships[0]).unwrap();
    member["membership"]["lifecycle"] = json!(MembershipLifecycle::VALUES
        .iter()
        .max_by_key(|s| s.len())
        .unwrap());
    member["membership"]["desired_priority"] = json!(-JSON_SAFE);
    member["membership"]["observed_priority"] = json!(-JSON_SAFE);
    member["membership"]["retired_at"] = json!(JSON_SAFE);
    member["create_state"] = json!(CreateState::VALUES.iter().max_by_key(|s| s.len()).unwrap());
    member["error_code"] = json!(ControlErrorCode::VALUES
        .iter()
        .max_by_key(|s| s.len())
        .unwrap());
    member["status_directive"] = json!(StatusDirective::VALUES
        .iter()
        .max_by_key(|s| s.len())
        .unwrap());
    let member_width = field_width_bound(&member);
    assert!(member_width <= 1024);
    let mut group = serde_json::to_value(&glm.catalog.groups[0]).unwrap();
    group["pinned_member_id"] = json!(i64::MAX.to_string());
    group["regime"] = json!(crate::orchestrator::Regime::Degraded.as_str());
    let mut assessment = serde_json::to_value(&glm.glm.groups[0]).unwrap();
    assessment["imminent_members"] = serde_json::Value::Null; // 此固定字段保守计4字节，ID按全局成员另加。
    assessment["pin_release"] = json!({"membership_id":i64::MAX.to_string(),"pct":f64::MIN_POSITIVE,"limit":f64::MIN_POSITIVE});
    let group_width = field_width_bound(&group) + field_width_bound(&assessment);
    assert!(group_width <= 2 * 1024);
    let mut root = empty(JSON_SAFE);
    root.glm.cycle_id = Some(u64::MAX.to_string());
    root.glm.completed_at_ms = Some(JSON_SAFE);
    let root_width = field_width_bound(&serde_json::to_value(root).unwrap());
    assert!(root_width <= 64 * 1024);
    assert_eq!(
        serde_json::to_vec(&i64::MAX.to_string()).unwrap().len() + 1,
        22
    );
    let total = 16 * 1024 * MAX_SOURCES
        + 1024 * MAX_MEMBERSHIPS
        + 2 * 1024 * MAX_GROUPS
        + 22 * MAX_MEMBERSHIPS
        + 64 * 1024;
    assert_eq!(total, 27_114_336);
    assert!(total < MAX_RESPONSE);
    println!("HTTP_FIELD_BOUNDS source={source_width} member={member_width} group={group_width} root={root_width} total={total}");
}

#[test]
fn h1_oversized_encoding_and_error_wire_never_publish_private_shapes() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(PublishedPanel, serde::Serialize);
    assert_not_impl!(ActivePublication, serde::Serialize);
    assert_not_impl!(crate::panel_http::request::RequestHead, serde::Serialize);
    assert_not_impl!(crate::panel_http::request::RequestHead, std::fmt::Debug);
    let mut panel = maximum(1);
    panel.catalog.sources[0].source.note = "x".repeat(MAX_RESPONSE);
    assert!(encode_panel(&panel).is_err());
    let all = crate::panel_http::HttpErrorCode::VALUES
        .iter()
        .map(|code| {
            let response = crate::panel_http::Response::error(*code);
            let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(value.as_object().unwrap().len(), 2);
            assert_eq!(value["schema_version"], 1);
            serde_json::json!({"body":value,"status":response.status})
        })
        .collect::<Vec<_>>();
    println!("HTTP_ERRORS_JSON={}", serde_json::to_string(&all).unwrap());
}

async fn maximum_body() -> Duration {
    let bytes = vec![b'x'; crate::panel_http::request::MAX_BODY];
    let mut input = bytes.as_slice();
    let start = std::time::Instant::now();
    let body = crate::panel_http::request::read_body(
        &mut input,
        vec![],
        bytes.len(),
        Instant::now() + Duration::from_secs(10),
    )
    .await
    .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(body, bytes);
    assert!(input.is_empty());
    println!("HTTP_MAXIMUM_BODY_MS={}", elapsed.as_millis());
    elapsed
}
#[tokio::test]
async fn h1_maximum_body_keeps_all_bytes_without_timing_gate() {
    maximum_body().await;
}

#[test]
#[ignore = "独立串行release性能门禁"]
fn performance_gate_panel_http_encoding() {
    for mode in 0..3 {
        assert!(check_maximum(mode) < Duration::from_secs(2));
    }
}
#[tokio::test]
#[ignore = "独立串行release性能门禁"]
async fn performance_gate_panel_http_body() {
    assert!(maximum_body().await < Duration::from_secs(1));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn h1_pb4_safe_encoding_preserves_input_unicode_and_complete_arrays(notes in prop::collection::vec(prop::collection::vec(any::<char>(),0..128),0..6), number_bits in any::<u64>()) {
        let mut panel=empty(AT);let mut expected=Vec::new();
        for (i,chars) in notes.into_iter().enumerate() {
            let note:String=chars.into_iter().filter(|c|*c!='\0').collect();expected.push(note.clone());let id=SourceId::new(i as i64+1).unwrap();
            panel.catalog.sources.push(PanelSource{source:SourceView{source_id:id,kind:SourceKind::Glm,name:format!("source-{i}"),note,manual_paused:false,retired_at:None,config_revision:1},retire_requested:false,model_count:1,volcano:None});
            panel.glm.sources.push(GlmQuotaView{source_id:id,source_revision:1,observed_at_ms:None,state:ObservationState::Unknown,five_hour:None,weekly:None,level:None});
        }
        let encoded=encode_panel(&panel).unwrap();let value:serde_json::Value=serde_json::from_slice(&encoded).unwrap();
        prop_assert_eq!(value["catalog"]["sources"].as_array().unwrap().len(),expected.len());
        prop_assert_eq!(value["glm"]["sources"].as_array().unwrap().len(),expected.len());
        for (i,note) in expected.iter().enumerate() {prop_assert_eq!(value["catalog"]["sources"][i]["source"]["note"].as_str(),Some(note.as_str()));prop_assert_eq!(value["glm"]["sources"][i]["state"].as_str(),Some("unknown"));}
        prop_assert!(encoded.len()<MAX_RESPONSE);
        let value=f64::from_bits(number_bits);
        if value.is_finite() {prop_assert!(serde_json::to_vec(&value).unwrap().len()<=32);}
    }
}
