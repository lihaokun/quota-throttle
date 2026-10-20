use super::*;
use crate::quota::{QuotaStatus, WindowStatus};
use crate::reconcile::worker::panel::{project_glm, ObservationState};
use crate::source_policy::probe::{ProbeCycle, QuotaObservation, QuotaReading};
use rusqlite::{types::Value as SqlValue, Connection};
use std::collections::BTreeMap;

fn db(f: &Fixture) -> Connection {
    Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap()
}
fn image(f: &Fixture) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    let conn = db(f);
    let names:Vec<String>=conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut q = conn
                .prepare(&format!("SELECT * FROM {name} ORDER BY 1,2"))
                .unwrap();
            let width = q.column_count();
            let rows = q
                .query_map([], |r| (0..width).map(|n| r.get(n)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<Vec<SqlValue>>>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}
fn available(pct: f64, at: i64) -> QuotaReading {
    QuotaReading::Available(QuotaStatus {
        five_hour: Some(WindowStatus {
            percentage: pct,
            next_reset_time: at + 3_600_000,
        }),
        weekly: Some(WindowStatus {
            percentage: pct,
            next_reset_time: at + 7 * 86_400_000,
        }),
        level: Some("max".into()),
    })
}
fn batch(f: &Fixture, seq: u64, at: i64, readings: Vec<QuotaReading>) -> ProbeCycle {
    let sources: Vec<_> = f
        .worker
        .snapshot()
        .unwrap()
        .sources
        .into_iter()
        .filter(|s| s.kind == SourceKind::Glm)
        .collect();
    assert_eq!(sources.len(), readings.len());
    ProbeCycle::test_input(
        seq,
        at,
        sources
            .into_iter()
            .zip(readings)
            .map(|(s, reading)| QuotaObservation {
                source_id: s.source_id,
                config_revision: s.config_revision,
                cycle_id: seq,
                observed_at_ms: at,
                reading,
            })
            .collect(),
    )
}

#[tokio::test]
async fn s6a1_snapshot_is_readonly_unknown_and_provider_scoped() {
    for kind in [SourceKind::Glm, SourceKind::Volcano] {
        let f = fixture(kind, &[(0, 0, 10, Some(1))]).await;
        let before = image(&f);
        let epoch = f.worker.publication.load(Ordering::SeqCst);
        for _ in 0..3 {
            let view = f.worker.panel_snapshot().unwrap();
            assert_eq!(view.catalog.sources.len(), 1);
            assert_eq!(view.glm.sources.len(), usize::from(kind == SourceKind::Glm));
            assert!(view
                .glm
                .sources
                .iter()
                .all(|s| s.state == ObservationState::Unknown));
            assert!(view
                .glm
                .groups
                .iter()
                .all(|g| !g.current && g.imminent_members.is_none()));
            assert_eq!(view.glm.cycle_id, None);
        }
        assert_eq!(image(&f), before);
        assert_eq!(f.worker.publication.load(Ordering::SeqCst), epoch);
        assert!(f.fake.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn s6a1_clock_busy_and_poison_have_distinct_safe_errors() {
    let f = fixture(SourceKind::Glm, &[]).await;
    assert_eq!(
        f.worker.panel_snapshot_at(-1).unwrap_err().code,
        ControlErrorCode::InvalidInput
    );
    assert_eq!(
        f.worker.panel_snapshot_at(JSON_SAFE + 1).unwrap_err().code,
        ControlErrorCode::InvalidInput
    );
    let guard = f.worker.core.lock().unwrap();
    assert_eq!(
        f.worker.panel_snapshot().unwrap_err().code,
        ControlErrorCode::QueueFull
    );
    drop(guard);
    let worker = f.worker.clone();
    assert!(std::thread::spawn(move || {
        let _guard = worker.core.lock().unwrap();
        panic!("test-only poison");
    })
    .join()
    .is_err());
    assert_eq!(
        f.worker.panel_snapshot().unwrap_err().code,
        ControlErrorCode::Storage
    );
}

#[tokio::test]
async fn s6a1_observed_time_boundaries_do_not_renew_from_completion() {
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1))]).await;
    let at = now() * 1000;
    let original = batch(&f, 1, at, vec![available(20.0, at)]);
    let delayed = ProbeCycle::test_input(1, at + 20_000, original.observations().to_vec());
    f.worker.publish_glm_cycle(&delayed, at + 20_000).unwrap();
    for (age, state) in [
        (-1, ObservationState::Stale),
        (0, ObservationState::Observed),
        (30_000, ObservationState::Observed),
        (30_001, ObservationState::Stale),
    ] {
        let view = f.worker.panel_snapshot_at(at + age).unwrap();
        assert_eq!(view.glm.sources[0].state, state);
        assert_eq!(view.glm.sources[0].observed_at_ms, Some(at));
        if state == ObservationState::Stale {
            assert!(view.glm.sources[0].five_hour.is_none());
            assert!(view.glm.sources[0].level.is_none());
        }
    }
    assert!(!f.worker.panel_snapshot_at(at + 30_001).unwrap().glm.groups[0].current);
    assert!(f.fake.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn s6a1_empty_failed_and_changed_during_probe_have_bounded_assessments() {
    let empty = fixture(SourceKind::Glm, &[]).await;
    let at = now() * 1000;
    empty
        .worker
        .create_group(GroupDraft {
            name: "empty".into(),
            newapi_group: "empty".into(),
            kind: SourceKind::Glm,
        })
        .unwrap();
    empty
        .worker
        .publish_glm_cycle(&ProbeCycle::test_input(1, at, vec![]), at)
        .unwrap();
    assert!(
        empty
            .worker
            .panel_snapshot_at(at + 30_000)
            .unwrap()
            .glm
            .groups[0]
            .current
    );
    assert!(
        !empty
            .worker
            .panel_snapshot_at(at + 30_001)
            .unwrap()
            .glm
            .groups[0]
            .current
    );
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1))]).await;
    f.worker
        .publish_glm_cycle(&batch(&f, 1, at, vec![QuotaReading::Unavailable]), at)
        .unwrap();
    let failed = f.worker.panel_snapshot_at(at).unwrap();
    assert!(failed.glm.groups[0].current);
    assert_eq!(failed.glm.sources[0].state, ObservationState::Unavailable);
    assert_eq!(failed.glm.groups[0].imminent_members, Some(vec![]));
    let old = batch(&f, 2, at + 1, vec![available(1.0, at)]);
    let source = f.worker.snapshot().unwrap().sources[0].clone();
    f.worker
        .submit(
            operation(99),
            ControlCommand::SetSourcePaused {
                source_id: source.source_id,
                expected_revision: source.config_revision,
                paused: false,
            },
        )
        .unwrap();
    f.worker.publish_glm_cycle(&old, at + 1).unwrap();
    let stale = f.worker.panel_snapshot_at(at + 1).unwrap();
    assert!(!stale.glm.groups[0].current);
    assert_eq!(stale.glm.sources[0].state, ObservationState::Stale);
    assert_eq!(stale.glm.sources[0].source_revision, 2);
    f.worker
        .submit(
            operation(100),
            ControlCommand::SetSourcePaused {
                source_id: source.source_id,
                expected_revision: 2,
                paused: true,
            },
        )
        .unwrap();
    f.worker
        .publish_glm_cycle(&ProbeCycle::test_input(3, at + 2, vec![]), at + 2)
        .unwrap();
    assert!(f.worker.panel_snapshot_at(at + 2).unwrap().glm.groups[0].current);
    assert!(!f.worker.panel_snapshot_at(at + 30_003).unwrap().glm.groups[0].current);
}

#[tokio::test]
async fn s6a1_invalid_display_fields_and_large_cycles_remain_safe() {
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1))]).await;
    let at = now() * 1000;
    for (seq, label, expected) in [
        (1, "模".repeat(128), true),
        (2, "x".repeat(129), false),
        (3, "a\0b".into(), false),
    ] {
        let reading = QuotaReading::Available(QuotaStatus {
            five_hour: Some(WindowStatus {
                percentage: 20.0,
                next_reset_time: i64::MAX,
            }),
            weekly: None,
            level: Some(label.clone()),
        });
        f.worker
            .publish_glm_cycle(&batch(&f, seq, at, vec![reading]), at)
            .unwrap();
        let view = f.worker.panel_snapshot_at(at).unwrap();
        assert_eq!(view.glm.sources[0].state, ObservationState::Observed);
        assert_eq!(view.glm.sources[0].level, expected.then_some(label));
        assert_eq!(
            view.glm.sources[0].five_hour.as_ref().unwrap().reset_at_ms,
            None
        );
        assert!(serde_json::to_string(&view).is_ok());
    }
    for (seq, bad) in [(4, f64::INFINITY), (5, -1.0), (6, f64::NEG_INFINITY)] {
        let reading = QuotaReading::Available(QuotaStatus {
            five_hour: Some(WindowStatus {
                percentage: bad,
                next_reset_time: 0,
            }),
            weekly: Some(WindowStatus {
                percentage: 20.0,
                next_reset_time: at + 3_600_000,
            }),
            level: Some("   ".into()),
        });
        f.worker
            .publish_glm_cycle(&batch(&f, seq, at, vec![reading]), at)
            .unwrap();
        let view = f.worker.panel_snapshot_at(at).unwrap();
        assert_eq!(view.glm.sources[0].state, ObservationState::Observed);
        assert!(view.glm.sources[0].five_hour.is_none());
        assert!(view.glm.sources[0].level.is_none());
        assert_eq!(
            view.glm.sources[0].weekly.as_ref().unwrap().percentage,
            20.0
        );
    }
    let original = batch(&f, 7, at, vec![available(10.0, at)]);
    let mut observations = original.observations().to_vec();
    observations[0].cycle_id = 6;
    f.worker
        .publish_glm_cycle(&ProbeCycle::test_input(7, at, observations), at)
        .unwrap();
    let view = f.worker.panel_snapshot_at(at).unwrap();
    assert_eq!(view.glm.sources[0].state, ObservationState::Stale);
    assert!(!view.glm.groups[0].current);
    f.worker
        .publish_glm_cycle(&batch(&f, u64::MAX, at, vec![available(10.0, at)]), at)
        .unwrap();
    let view = f.worker.panel_snapshot_at(at).unwrap();
    assert_eq!(view.glm.cycle_id.as_deref(), Some("18446744073709551615"));
    assert!(view.glm.groups[0].current);
    println!(
        "PANEL_SCHEMA_JSON={}",
        serde_json::to_string(&view).unwrap()
    );
}

#[tokio::test(start_paused = true)]
async fn s6a1_retirement_missing_members_and_new_binding_invalidate_display() {
    scenarios::drive(async {
        let f = fixture(
            SourceKind::Glm,
            &[(0, 0, 100, Some(1)), (1, 0, 10, Some(1))],
        )
        .await;
        finish(&f).await;
        let w = f.worker.work(f.ids[1]).unwrap();
        f.worker
            .submit(
                operation(901),
                ControlCommand::RetireMember {
                    membership_id: w.member.membership_id,
                    expected_revision: w.target.revision,
                },
            )
            .unwrap();
        let pending_at = now() * 1000;
        f.worker
            .publish_glm_cycle(
                &batch(
                    &f,
                    1,
                    pending_at,
                    vec![available(20.0, pending_at), available(30.0, pending_at)],
                ),
                pending_at,
            )
            .unwrap();
        assert!(f.worker.panel_snapshot_at(pending_at).unwrap().glm.groups[0].current);
        finish(&f).await;
        assert!(!f.worker.panel_snapshot_at(pending_at).unwrap().glm.groups[0].current);
        f.worker
            .submit(
                operation(902),
                ControlCommand::RetireSource {
                    source_id: w.source.source_id,
                    expected_revision: w.source.config_revision,
                },
            )
            .unwrap();
        let at = now() * 1000;
        let all = batch(
            &f,
            2,
            at,
            vec![available(20.0, at), QuotaReading::Unavailable],
        );
        let source = f.worker.work(f.ids[0]).unwrap().source.source_id;
        let observations = all
            .observations()
            .iter()
            .filter(|o| o.source_id == source)
            .cloned()
            .collect();
        f.worker
            .publish_glm_cycle(&ProbeCycle::test_input(2, at, observations), at)
            .unwrap();
        let original = f.worker.panel_snapshot_at(at).unwrap();
        assert!(original.glm.groups[0].current);
        assert_eq!(
            original.catalog.memberships[1].membership.lifecycle,
            MembershipLifecycle::Retired
        );
        assert!(original.catalog.sources[1].source.retired_at.is_some());
        for change in 0..7 {
            let mut catalog = original.catalog.clone();
            match change {
                0 => {
                    catalog.sources[1].source.retired_at =
                        catalog.sources[1].source.retired_at.map(|t| t - 1)
                }
                1 => {
                    catalog.memberships[1].membership.retired_at =
                        catalog.memberships[1].membership.retired_at.map(|t| t - 1)
                }
                2 => catalog.memberships[0].retire_requested = true,
                3 => {
                    catalog.memberships.remove(1);
                }
                4 => {
                    catalog.memberships.remove(0);
                }
                5 => {
                    catalog.memberships[0].membership.lifecycle = MembershipLifecycle::Retired;
                    catalog.memberships[0].membership.retired_at = Some(at / 1000);
                    catalog.memberships[0].retire_requested = true;
                }
                _ => catalog.sources[0].source.retired_at = Some(at / 1000),
            }
            let core = f.worker.core.lock().unwrap();
            assert_eq!(
                project_glm(&catalog, core.panel_glm.as_ref(), at)
                    .unwrap()
                    .groups[0]
                    .current,
                matches!(change, 0 | 1 | 3)
            );
        }
        let g = fixture(SourceKind::Glm, &[(0, 0, 10, None), (1, 0, 100, Some(1))]).await;
        g.worker
            .publish_glm_cycle(
                &batch(&g, 1, at, vec![available(10.0, at), available(20.0, at)]),
                at,
            )
            .unwrap();
        let mut catalog = g.worker.panel_snapshot_at(at).unwrap().catalog;
        catalog.memberships[0].membership.channel_id = Some(ChannelId::new(100).unwrap());
        catalog.memberships[0].membership.lifecycle = MembershipLifecycle::CreatedDisabled;
        catalog.memberships[0].create_state = crate::catalog::control::CreateState::Resolved;
        let core = g.worker.core.lock().unwrap();
        assert!(
            !project_glm(&catalog, core.panel_glm.as_ref(), at)
                .unwrap()
                .groups[0]
                .current
        );
    })
    .await;
}

#[tokio::test]
async fn s6a1_failed_commit_replay_and_preview_keep_prior_display() {
    let f = fixture(
        SourceKind::Glm,
        &[(0, 0, 100, Some(1)), (1, 0, 10, Some(1))],
    )
    .await;
    let at = now() * 1000;
    let first = batch(&f, 1, at, vec![available(20.0, at), available(30.0, at)]);
    f.worker.publish_glm_cycle(&first, at).unwrap();
    let before = f.worker.panel_snapshot_at(at).unwrap();
    assert_eq!(
        f.worker.publish_glm_cycle(&first, at).unwrap_err().code,
        ControlErrorCode::RevisionConflict
    );
    assert_eq!(f.worker.panel_snapshot_at(at).unwrap(), before);
    db(&f).execute_batch("CREATE TRIGGER panel_fail BEFORE UPDATE ON group_policy_state BEGIN SELECT RAISE(ABORT,'panel fault'); END;").unwrap();
    let data_before = image(&f);
    assert!(f
        .worker
        .publish_glm_cycle(
            &batch(
                &f,
                2,
                at + 1,
                vec![available(100.0, at), available(100.0, at)]
            ),
            at + 1
        )
        .is_err());
    assert_eq!(f.worker.panel_snapshot_at(at).unwrap(), before);
    assert_eq!(image(&f), data_before);
    let mut cfg = (*policy_config()).clone();
    cfg.dry_run = true;
    let preview =
        fixture_with_options(SourceKind::Glm, &[(0, 0, 10, Some(1))], true, Arc::new(cfg)).await;
    let data_before = image(&preview);
    assert!(
        preview
            .worker
            .publish_glm_cycle(&batch(&preview, 1, at, vec![available(0.0, at)]), at)
            .unwrap()
            .preview_only
    );
    assert_eq!(
        preview.worker.panel_snapshot_at(at).unwrap().glm.cycle_id,
        None
    );
    assert_eq!(image(&preview), data_before);
}

#[tokio::test]
async fn s6a1_imminent_reset_and_pin_release_are_last_committed_events() {
    let f = fixture(
        SourceKind::Glm,
        &[(0, 0, 100, Some(1)), (1, 0, 10, Some(1))],
    )
    .await;
    let at = now() * 1000;
    let view = f.worker.panel_snapshot_at(at).unwrap();
    let group = &view.catalog.groups[0];
    f.worker
        .submit(
            operation(77),
            ControlCommand::SetGroupPin {
                group_id: group.group.group_id,
                expected_revision: group.group.revision,
                expected_policy_revision: group.policy_revision,
                membership_id: Some(f.ids[0]),
            },
        )
        .unwrap();
    f.worker
        .publish_glm_cycle(
            &batch(&f, 1, at, vec![available(100.0, at), available(10.0, at)]),
            at,
        )
        .unwrap();
    let released = f.worker.panel_snapshot_at(at).unwrap();
    assert!(released.glm.groups[0].current);
    assert_eq!(
        released.glm.groups[0]
            .pin_release
            .as_ref()
            .unwrap()
            .membership_id,
        f.ids[0]
    );
    assert_eq!(
        released.glm.groups[0].pin_release.as_ref().unwrap().pct,
        100.0
    );
    println!(
        "PANEL_SCHEMA_JSON={}",
        serde_json::to_string(&released).unwrap()
    );
    let mut imminent = available(1.0, at);
    if let QuotaReading::Available(s) = &mut imminent {
        s.weekly.as_mut().unwrap().next_reset_time = at + 1000;
    }
    f.worker
        .publish_glm_cycle(
            &batch(&f, 2, at + 1, vec![available(100.0, at), imminent]),
            at + 1,
        )
        .unwrap();
    let next = f.worker.panel_snapshot_at(at + 1).unwrap();
    assert!(next.glm.groups[0].pin_release.is_none());
    assert_eq!(next.glm.groups[0].imminent_members, Some(vec![f.ids[1]]));
    assert!(!f.worker.panel_snapshot_at(at + 1000).unwrap().glm.groups[0].current);
}

#[tokio::test]
async fn s6a1_reopen_preserves_catalog_but_not_observation_cache() {
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1))]).await;
    let at = now() * 1000;
    f.worker
        .publish_glm_cycle(&batch(&f, 1, at, vec![available(20.0, at)]), at)
        .unwrap();
    let before = f.worker.panel_snapshot_at(at).unwrap().catalog;
    drop(f.worker);
    let store = CatalogStore::open(&f._dir.path().join("catalog.sqlite3")).unwrap();
    let reopened = ReconcileWorker::new(
        store,
        Arc::new(NewApiClient::new(&f.fake.config(true)).unwrap()),
        policy_config(),
    )
    .unwrap();
    let view = reopened.panel_snapshot_at(at).unwrap();
    assert_eq!(view.catalog, before);
    assert_eq!(view.glm.cycle_id, None);
    assert!(view
        .glm
        .sources
        .iter()
        .all(|s| s.state == ObservationState::Unknown));
}

#[tokio::test]
async fn s6a1_cancelled_real_probe_does_not_replace_committed_display() {
    let slow = Arc::new(AtomicBool::new(false));
    let flag = slow.clone();
    let at = now() * 1000;
    let quota=Fake::start(move |_|Reply { delay:if flag.load(Ordering::SeqCst) {Duration::from_secs(20)}else{Duration::ZERO},..Reply::json(json!({"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":10,"nextResetTime":at+3_600_000}]}})) }).await;
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1))]).await;
    db(&f).execute("UPDATE sources SET settings_json=json_set(settings_json,'$.settings.quota.quota_url',?1)",[quota.url.clone()]).unwrap();
    f.worker.refresh_glm_policy().await.unwrap();
    let before = f.worker.panel_snapshot().unwrap().glm;
    assert_eq!(before.sources[0].state, ObservationState::Observed);
    let data_before = image(&f);
    slow.store(true, Ordering::SeqCst);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.refresh_glm_policy().await });
    until(|| quota.calls.lock().unwrap().len() == 2).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(f.worker.panel_snapshot().unwrap().glm, before);
    assert_eq!(image(&f), data_before);
    assert!(f.fake.calls.lock().unwrap().is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn s6a1_pb2_quota_state_matches_independent_input_model(mode in 0u8..4,age in -2i64..30_003,changed in any::<bool>(),pct in 0u16..151) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let f=fixture(SourceKind::Glm,&[(0,0,10,Some(1))]).await;let at=now()*1000;
            let reading=match mode {0=>available(pct as f64,at),1=>QuotaReading::Unavailable,2=>QuotaReading::Available(QuotaStatus{five_hour:Some(WindowStatus{percentage:f64::NAN,next_reset_time:0}),weekly:None,level:Some("x".repeat(129))}),_=>QuotaReading::Available(QuotaStatus::default())};
            f.worker.publish_glm_cycle(&batch(&f,1,at,vec![reading]),at).unwrap();
            if changed {let s=f.worker.snapshot().unwrap().sources[0].clone();f.worker.submit(operation(888),ControlCommand::SetSourcePaused{source_id:s.source_id,expected_revision:1,paused:true}).unwrap();}
            let out=f.worker.panel_snapshot_at(at+age).unwrap();let q=&out.glm.sources[0];
            let expected=if changed||age<0||age>30_000 {ObservationState::Stale}else if mode==0 {ObservationState::Observed}else{ObservationState::Unavailable};
            prop_assert_eq!(q.state,expected);prop_assert_eq!(q.observed_at_ms,Some(at));
            prop_assert_eq!(q.source_revision,if changed {2}else{1});
            if expected==ObservationState::Observed {prop_assert_eq!(q.five_hour.as_ref().unwrap().percentage,pct as f64);}else{prop_assert!(q.five_hour.is_none()&&q.weekly.is_none()&&q.level.is_none());}
            prop_assert!(serde_json::to_string(&out).is_ok());
            Ok(())
        })?;
    }
    #[test]
    fn s6a1_pb3_group_stamp_covers_all_inputs_not_observation_fields(change in 0u8..13,delta in 1i64..20) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let f=fixture(SourceKind::Glm,&[(0,0,100,Some(1)),(1,0,10,Some(1)),(2,1,10,Some(1))]).await;let at=now()*1000;
            f.worker.publish_glm_cycle(&batch(&f,1,at,vec![available(10.0,at),available(20.0,at),available(30.0,at)]),at).unwrap();
            let mut catalog=f.worker.panel_snapshot_at(at).unwrap().catalog;
            match change {
                0=>{catalog.memberships[0].membership.observed_status=Some(2);catalog.memberships[0].membership.observed_priority=Some(10);catalog.memberships[0].membership.applied_revision=Some(1);},
                1=>catalog.memberships[0].membership.applied_revision=Some(1),
                2=>catalog.memberships[0].next_attempt_at+=delta,
                3=>catalog.sources[1].source.config_revision+=delta,
                4=>catalog.sources[1].source.manual_paused=true,
                5=>catalog.sources[1].retire_requested=true,
                6=>catalog.memberships[0].control_revision+=delta,
                7=>catalog.memberships[0].membership.channel_id=Some(ChannelId::new(100+delta).unwrap()),
                8=>catalog.memberships[0].membership.lifecycle=MembershipLifecycle::CreatedDisabled,
                9=>catalog.memberships[0].membership.order+=delta,
                10=>catalog.groups[0].policy_revision+=delta,
                11=>catalog.groups[0].group.revision+=delta,
                _=>{let mut added=catalog.memberships[0].clone();added.membership.membership_id=MembershipId::new(100+delta).unwrap();added.membership.source_id=catalog.sources[2].source.source_id;added.membership.order=2;added.membership.channel_id=None;added.membership.lifecycle=MembershipLifecycle::Planned;catalog.memberships.push(added);},
            }
            let core=f.worker.core.lock().unwrap();let projected=project_glm(&catalog,core.panel_glm.as_ref(),at).unwrap();
            prop_assert_eq!(projected.groups[0].current,change<=2);
            prop_assert!(projected.groups[1].current);
            if change>2 {prop_assert!(projected.groups[0].imminent_members.is_none());}
            Ok(())
        })?;
    }
}
