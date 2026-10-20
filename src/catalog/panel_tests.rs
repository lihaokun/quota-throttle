use super::*;
use crate::catalog::control::{ControlCommand, ControlErrorCode, OperationId, StatusDirective};
use crate::catalog::policy::SourcePolicyUpdate;
use crate::source_policy::volcano::{OutcomeKind, OutcomeObservation, VolcanoEvidence};
use std::sync::Arc;
use std::time::{Duration, Instant};

const AT: i64 = 1_800_000_000;
fn op(n: usize) -> OperationId {
    OperationId::new(format!("panel-operation-{n:016}")).unwrap()
}
fn worker(store: CatalogStore) -> Arc<crate::reconcile::worker::ReconcileWorker> {
    let cfg:Config=toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'\nadmin_token='fake-panel-admin'").unwrap();
    let api = Arc::new(crate::newapi::NewApiClient::new(&cfg.new_api).unwrap());
    crate::reconcile::worker::ReconcileWorker::new(store, api, Arc::new(cfg)).unwrap()
}

#[test]
fn s6a1_catalog_empty_and_private_types() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(crate::catalog::panel::GroupInputStamp, serde::Serialize);
    assert_not_impl!(
        crate::catalog::panel::CommittedGroupAssessment,
        serde::Serialize
    );
    let (_dir, mut store) = setup();
    let changes = store.conn.total_changes();
    let view = store.panel_catalog().unwrap();
    assert!(view.sources.is_empty() && view.groups.is_empty() && view.memberships.is_empty());
    assert_eq!(changes, store.conn.total_changes());
    assert!(store.conn.is_autocommit());
}

#[test]
fn s6a1_catalog_schema_is_whitelisted_and_preserves_runtime() {
    let (_dir, mut store) = setup();
    let mut draft = source("a1-credential-never-publish", SourceKind::Glm);
    if let SourceSettings::Glm { quota, .. } = &mut draft.settings {
        quota.key_headers.push(HeaderKV {
            key: "X-Private".into(),
            value: "a1-selector-never-publish".into(),
        });
    }
    let glm = store.create_source(draft).unwrap();
    let volcano = store
        .create_source(source("a1-volcano-secret", SourceKind::Volcano))
        .unwrap();
    for (index, s) in [&glm, &volcano].into_iter().enumerate() {
        let g = store
            .create_group(group(&format!("panel-{index}"), s.kind))
            .unwrap();
        store.plan_membership(s.source_id, g.group_id, 13).unwrap();
    }
    assert!(store.panel_catalog().unwrap().sources[1]
        .volcano
        .as_ref()
        .unwrap()
        .runtime
        .is_none());
    let runtime = store
        .initialize_source_policy(volcano.source_id, 1, AT)
        .unwrap();
    store
        .apply_source_policy(
            volcano.source_id,
            1,
            runtime.revision,
            SourcePolicyUpdate::Evidence(VolcanoEvidence {
                source_id: volcano.source_id,
                period_id: runtime.period_id,
                error_epoch: runtime.error_epoch,
                from_cursor: None,
                to_cursor: 1,
                used_input: 5,
                used_output: 7,
                coverage_end: AT + 1,
                outcomes: vec![OutcomeObservation {
                    first_event_at: AT,
                    observed_at: AT,
                    kind: OutcomeKind::Unavailable,
                }],
            }),
            AT,
        )
        .unwrap();
    let before = store.conn.total_changes();
    let view = store.panel_catalog().unwrap();
    assert_eq!(before, store.conn.total_changes());
    assert_eq!(view.sources[0].model_count, 1);
    assert!(view.sources[0].volcano.is_none());
    let runtime = view.sources[1]
        .volcano
        .as_ref()
        .unwrap()
        .runtime
        .as_ref()
        .unwrap();
    assert_eq!(
        (
            runtime.used_input,
            runtime.used_output,
            runtime.consecutive_failures
        ),
        (5, 7, 1)
    );
    assert_eq!(runtime.coverage_end, Some(AT + 1));
    assert_eq!(view.groups[0].regime, Some("normal"));
    assert_eq!(view.groups[1].regime, None);
    let full = worker(store).panel_snapshot().unwrap();
    let encoded = serde_json::to_string(&full).unwrap();
    for secret in [
        "a1-credential-never-publish",
        "a1-selector-never-publish",
        "a1-volcano-secret",
        "example.test",
        "secret_ref",
        "fingerprint",
        "create_name",
        "settings_json",
    ] {
        assert!(!encoded.contains(secret), "private value/field in panel");
    }
    println!("PANEL_SCHEMA_JSON={encoded}");
    println!(
        "PANEL_ENUM_JSON={}",
        serde_json::json!({
            "state":crate::reconcile::worker::panel::ObservationState::VALUES,
            "status_directive":StatusDirective::VALUES,"create_state":crate::catalog::control::CreateState::VALUES,
            "error_code":ControlErrorCode::VALUES,
        })
    );
}

#[test]
fn s6a1_catalog_missing_or_corrupt_rows_fail_without_writes() {
    for damage in 0..3 {
        let (_dir, mut store) = setup();
        let s = store
            .create_source(source("a1-damage", SourceKind::Glm))
            .unwrap();
        let g = store
            .create_group(group("panel-damage", SourceKind::Glm))
            .unwrap();
        store.plan_membership(s.source_id, g.group_id, 10).unwrap();
        match damage {
            0 => store
                .conn
                .execute("DELETE FROM group_policy_state", [])
                .unwrap(),
            1 => store
                .conn
                .execute("DELETE FROM member_control", [])
                .unwrap(),
            _ => store
                .conn
                .execute(
                    "UPDATE sources SET settings_json=json_set(settings_json,'$.version',99)",
                    [],
                )
                .unwrap(),
        };
        let changes = store.conn.total_changes();
        assert_eq!(
            store.panel_catalog().unwrap_err().code,
            ControlErrorCode::Storage
        );
        assert_eq!(store.conn.total_changes(), changes);
        assert!(store.conn.is_autocommit());
    }
}

#[test]
fn s6a1_catalog_retire_intent_and_archived_identity_are_not_success_labels() {
    let (_dir, mut store) = setup();
    let s = store
        .create_source(source("a1-retiring", SourceKind::Glm))
        .unwrap();
    let archived = store
        .create_source(source("a1-archived", SourceKind::Glm))
        .unwrap();
    let g = store
        .create_group(group("panel-retiring", SourceKind::Glm))
        .unwrap();
    let m = store.plan_membership(s.source_id, g.group_id, 10).unwrap();
    store
        .submit_operation(
            op(1),
            ControlCommand::RetireSource {
                source_id: s.source_id,
                expected_revision: 1,
            },
            AT,
        )
        .unwrap();
    store
        .submit_operation(
            op(2),
            ControlCommand::RetireSource {
                source_id: archived.source_id,
                expected_revision: 1,
            },
            AT,
        )
        .unwrap();
    let view = store.panel_catalog().unwrap();
    assert_eq!(view.sources.len(), 2);
    assert!(view.sources[0].retire_requested && view.sources[0].source.manual_paused);
    assert_eq!(view.sources[1].source.retired_at, Some(AT));
    assert_eq!(
        view.memberships[0].membership.membership_id,
        m.membership_id
    );
    assert_eq!(view.memberships[0].control_revision, 2);
    assert!(view.memberships[0].retire_requested);
    assert_eq!(
        view.memberships[0].status_directive,
        StatusDirective::Disabled
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn s6a1_pb1_catalog_projection_matches_input_model(input in prop::collection::vec((any::<bool>(),any::<bool>(),1usize..5),0..12)) {
        let (_dir,mut store)=setup();
        let gs=[store.create_group(group("panel-glm",SourceKind::Glm)).unwrap(),store.create_group(group("panel-volcano",SourceKind::Volcano)).unwrap()];
        let mut ids=Vec::new();
        for (i,(volcano,paused,models)) in input.iter().enumerate() {
            let kind=if *volcano {SourceKind::Volcano}else{SourceKind::Glm};
            let mut draft=source(&format!("private-a1-{i}"),kind);
            draft.name=format!("public-{i}");
            let upstream=match &mut draft.settings {SourceSettings::Glm{upstream,..}|SourceSettings::Volcano{upstream,..}=>upstream};
            upstream.models=(0..*models).map(|n| format!("model-{n}")).collect();
            let s=store.create_source(draft).unwrap();
            let m=store.plan_membership(s.source_id,gs[usize::from(*volcano)].group_id,i as i64).unwrap();
            store.submit_operation(op(i),ControlCommand::SetSourcePaused{source_id:s.source_id,expected_revision:1,paused:*paused},AT).unwrap();
            ids.push((s.source_id,m.membership_id));
        }
        let changes=store.conn.total_changes();let view=store.panel_catalog().unwrap();
        prop_assert_eq!(store.conn.total_changes(),changes);
        prop_assert_eq!(view.sources.len(),input.len());prop_assert_eq!(view.memberships.len(),input.len());
        for (i,(volcano,paused,models)) in input.iter().enumerate() {
            let s=&view.sources[i];let m=&view.memberships[i];
            prop_assert_eq!(s.source.source_id,ids[i].0);prop_assert_eq!(s.source.config_revision,2);
            prop_assert_eq!(s.source.manual_paused,*paused);prop_assert_eq!(s.model_count,*models as u32);
            prop_assert_eq!(s.volcano.is_some(),*volcano);
            if let Some(v)=&s.volcano {prop_assert!(v.runtime.is_none());prop_assert_eq!(v.policy.reset_minute,60);prop_assert_eq!(v.policy.utc_offset_minutes,480);}
            prop_assert_eq!(m.membership.membership_id,ids[i].1);prop_assert_eq!(m.control_revision,2);
            prop_assert_eq!(m.status_directive,if *paused {StatusDirective::Disabled}else{StatusDirective::Enabled});
        }
        prop_assert!(!serde_json::to_string(&view).unwrap().contains("private-a1-"));
    }
}

fn maximum_case() -> Duration {
    let (_dir, mut store) = setup();
    let tx = store.conn.transaction().unwrap();
    let sources: Vec<_> = (0..MAX_SOURCES)
        .map(|i| {
            insert_source(
                &tx,
                &validate_source(source(&format!("a1-capacity-{i}"), SourceKind::Glm)).unwrap(),
            )
            .unwrap()
            .source_id
        })
        .collect();
    for n in 0..MAX_GROUPS {
        let g = insert_group(
            &tx,
            &validate_group(group(&format!("panel-max-{n}"), SourceKind::Glm)).unwrap(),
        )
        .unwrap();
        for k in 0..MAX_MEMBERSHIPS / MAX_GROUPS {
            let index = n * (MAX_MEMBERSHIPS / MAX_GROUPS) + k;
            insert_membership(
                &tx,
                &NewMembership {
                    source: sources[index % MAX_SOURCES],
                    group: g.group_id,
                    kind: SourceKind::Glm,
                    channel: None,
                    position: k as i64,
                    lifecycle: MembershipLifecycle::Planned,
                    priority: 10,
                },
            )
            .unwrap();
        }
    }
    commit(tx, &mut store.poisoned).unwrap();
    let worker = worker(store);
    let start = Instant::now();
    let view = worker.panel_snapshot().unwrap();
    let elapsed = start.elapsed();
    assert_eq!(view.catalog.sources.len(), MAX_SOURCES);
    assert_eq!(view.catalog.groups.len(), MAX_GROUPS);
    assert_eq!(view.catalog.memberships.len(), MAX_MEMBERSHIPS);
    assert_eq!(view.glm.sources.len(), MAX_SOURCES);
    assert!(view
        .glm
        .groups
        .iter()
        .all(|g| !g.current && g.imminent_members.is_none()));
    assert!(view
        .catalog
        .memberships
        .iter()
        .all(|m| m.control_revision == 1));
    println!("PANEL_MAXIMUM_CATALOG_MS={}", elapsed.as_millis());
    elapsed
}
#[test]
fn s6a1_catalog_maximum_fixture_preserves_all_rows() {
    maximum_case();
}
#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_panel_maximum_catalog() {
    assert!(maximum_case() < Duration::from_secs(5));
}
