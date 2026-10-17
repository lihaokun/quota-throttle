use super::*;
use crate::catalog::ledger::{PageEnd, ScanLane, ScanWindow};
use crate::catalog::policy::GroupPolicyState;
use crate::quota::{QuotaStatus, WindowStatus};
use crate::source_policy::probe::{ProbeCycle, QuotaObservation, QuotaReading};
use crate::source_policy::volcano::VolcanoGroupState;
use std::collections::BTreeMap;

fn initialize(f: &Fixture) {
    let mut core = f.worker.core.lock().unwrap();
    core.store.initialize_ledger(0, now()).unwrap();
    drop(core);
    ingest(f, &[]);
}
fn ingest(f: &Fixture, rows: &[Value]) {
    let at = now();
    f.worker
        .mutate(None, |c| {
            let old = c
                .store
                .load_ledger_state()
                .unwrap()
                .scans
                .into_iter()
                .find(|s| s.lane == ScanLane::Recent)
                .unwrap();
            let t = c
                .store
                .begin_scan(
                    ScanLane::Recent,
                    old.generation,
                    ScanWindow { from: 0, to: at },
                    at,
                )
                .unwrap();
            c.store
                .commit_fact_page(&t, rows, PageEnd::ReachedEnd, at)
                .unwrap();
            Ok(())
        })
        .unwrap();
}
fn row(id: i64, channel: i64, req: &str, success: bool) -> Value {
    json!({"id":id,"type":if success{2}else{5},"created_at":now()-1,"channel":channel,"token_id":1,"model_name":"model-0","group":if channel<3{"group-0"}else{"group-1"},"prompt_tokens":if success{5}else{0},"completion_tokens":if success{3}else{0},"is_stream":false,"request_id":req,"other":if success{String::new()}else{json!({"status_code":503,"error_type":"openai_error","error_code":"unavailable"}).to_string()}})
}
fn routing_targets(f: &Fixture) {
    let mut targets = f.state.routing_targets.lock().unwrap();
    for id in &f.ids {
        let w = f.worker.work(*id).unwrap();
        targets.insert(
            w.member.channel_id.unwrap().get(),
            w.target.desired_priority,
        );
    }
}
fn leases(worker: &ReconcileWorker) -> BTreeMap<MembershipId, i64> {
    worker
        .core
        .lock()
        .unwrap()
        .permits
        .iter()
        .map(|(id, p)| (*id, p.expires_at()))
        .collect()
}
fn db_image(f: &Fixture) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    let conn = rusqlite::Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap();
    [
        "volcano_observation_queue",
        "volcano_request_observations",
        "volcano_period_history",
        "volcano_runtime",
        "group_policy_state",
        "memberships",
        "member_control",
        "member_execution",
        "control_operations",
        "operation_targets",
        "ledger_state",
        "ledger_facts",
        "ledger_minute_usage",
    ]
    .into_iter()
    .map(|t| {
        let mut q = conn
            .prepare(&format!("SELECT * FROM {t} ORDER BY rowid"))
            .unwrap();
        let n = q.column_count();
        let rows = q
            .query_map([], |r| (0..n).map(|i| r.get(i)).collect())
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        (t.into(), rows)
    })
    .collect()
}

#[tokio::test(start_paused = true)]
async fn observed_worker_real_reconcile_disables_shared_source_before_promotion() {
    scenarios::drive(async {
        let f = fixture(
            SourceKind::Volcano,
            &[
                (0, 0, 100, Some(1)),
                (1, 0, 10, Some(1)),
                (0, 1, 10, Some(1)),
                (1, 1, 100, Some(1)),
            ],
        )
        .await;
        let w = f.worker.work(f.ids[3]).unwrap();
        f.worker
            .core
            .lock()
            .unwrap()
            .store
            .save_group_policy(
                w.group.group_id,
                w.group.revision,
                1,
                GroupPolicyState::Volcano(VolcanoGroupState {
                    preferred: Some(w.member.membership_id),
                    cursor: Some((w.member.order, w.member.membership_id)),
                }),
            )
            .unwrap();
        initialize(&f);
        assert_eq!(
            f.worker.refresh_volcano_policy().await.unwrap().permitted,
            4
        );
        routing_targets(&f);
        f.state.check_routing.store(true, Ordering::SeqCst);
        finish(&f).await;
        ingest(
            &f,
            &[
                row(1, 1, "f1", false),
                row(2, 3, "f2", false),
                row(3, 1, "f3", false),
            ],
        );
        let report = f.worker.refresh_volcano_policy().await.unwrap();
        assert_eq!(report.permitted, 2);
        routing_targets(&f);
        finish(&f).await;
        for ch in [1, 3] {
            assert_eq!(f.state.status(ch), 2);
        }
        {
            let channels = f.state.channels.lock().unwrap();
            assert_eq!(channels[&2]["priority"], 100);
            assert_eq!(channels[&4]["priority"], 100);
        }
        let source = f.worker.work(f.ids[0]).unwrap().source.source_id;
        {
            let c = f.worker.core.lock().unwrap();
            assert!(c.source_disabled[&source]);
            assert_eq!(
                c.store
                    .load_source_policy(source)
                    .unwrap()
                    .unwrap()
                    .consecutive_failures,
                3
            );
        }
        ingest(&f, &[row(4, 1, "f1", true)]);
        f.worker.refresh_volcano_policy().await.unwrap();
        finish(&f).await;
        {
            let c = f.worker.core.lock().unwrap();
            let r = c.store.load_source_policy(source).unwrap().unwrap();
            assert_eq!(r.consecutive_failures, 0);
            assert!(r.error_exhausted);
        }
        assert_eq!(f.state.status(1), 2);
        assert_eq!(f.state.status(3), 2);
        assert!(f.state.routing_violations.lock().unwrap().is_empty());
        assert!(f
            .fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.method == "POST" && r.path.ends_with("/status") && r.body["status"] == 2));
    })
    .await;
}

fn add_glm(f: &Fixture) -> MembershipId {
    let mut c = f.worker.core.lock().unwrap();
    let source = c
        .store
        .create_source(SourceDraft {
            name: "o4-glm".into(),
            note: String::new(),
            settings: SourceSettings::Glm {
                upstream: UpstreamProfile {
                    channel_type: 8,
                    base_url: "https://glm.invalid/v1".into(),
                    models: vec!["glm".into()],
                    model_discovery: None,
                },
                quota: GlmQuotaProfile {
                    quota_url: "https://quota.invalid".into(),
                    global_headers: vec![],
                    key_headers: vec![],
                },
            },
            credential: SecretString::new("fake-o4-glm".into()).unwrap(),
        })
        .unwrap()
        .source_id;
    let group = c
        .store
        .create_group(GroupDraft {
            name: "glm".into(),
            newapi_group: "glm".into(),
            kind: SourceKind::Glm,
        })
        .unwrap()
        .group_id;
    let id = c
        .store
        .plan_membership(source, group, 100)
        .unwrap()
        .membership_id;
    let conn = rusqlite::Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap();
    conn.execute(
        "UPDATE memberships SET channel_id=1001,lifecycle='imported' WHERE membership_id=?1",
        [id.get()],
    )
    .unwrap();
    conn.execute(
        "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
        [id.get()],
    )
    .unwrap();
    conn.execute(
        "UPDATE source_groups SET ready=1 WHERE group_id=?1",
        [group.get()],
    )
    .unwrap();
    c.store
        .submit_operation(
            operation(1001),
            ControlCommand::EnsureMember {
                membership_id: id,
                expected_revision: 1,
                status: StatusDirective::Preserve,
            },
            now(),
        )
        .unwrap();
    id
}
fn glm_cycle(f: &Fixture, id: MembershipId, sequence: u64) -> ProbeCycle {
    let w = f.worker.work(id).unwrap();
    let at = now() * 1000;
    ProbeCycle::test_input(
        sequence,
        at,
        vec![QuotaObservation {
            source_id: w.source.source_id,
            config_revision: w.source.config_revision,
            cycle_id: sequence,
            observed_at_ms: at,
            reading: QuotaReading::Available(QuotaStatus {
                five_hour: Some(WindowStatus {
                    percentage: 1.0,
                    next_reset_time: at + 3_600_000,
                }),
                weekly: None,
                level: None,
            }),
        }],
    )
}

#[tokio::test]
async fn observed_worker_retired_members_are_removed_only_by_their_provider() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
    initialize(&f);
    let glm = add_glm(&f);
    let cycle = glm_cycle(&f, glm, 1);
    f.worker.refresh_volcano_policy().await.unwrap();
    f.worker.publish_glm_cycle(&cycle, now() * 1000).unwrap();
    let before = leases(&f.worker);
    let conn = rusqlite::Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap();
    conn.execute(
        "UPDATE memberships SET lifecycle='retired',retired_at=?1",
        [now()],
    )
    .unwrap();
    f.worker.refresh_volcano_policy().await.unwrap();
    let next = leases(&f.worker);
    assert!(!next.contains_key(&f.ids[0]));
    assert_eq!(next[&glm], before[&glm]);
    f.worker
        .publish_glm_cycle(&glm_cycle(&f, glm, 2), now() * 1000)
        .unwrap();
    assert!(leases(&f.worker).is_empty());
    assert!(f.fake.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn observed_worker_glm_and_volcano_scopes_keep_other_provider_leases() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
    initialize(&f);
    let glm = add_glm(&f);
    f.worker.refresh_volcano_policy().await.unwrap();
    let old = leases(&f.worker);
    assert_eq!(old.len(), 1);
    f.worker
        .publish_glm_cycle(&glm_cycle(&f, glm, 1), now() * 1000)
        .unwrap();
    let both = leases(&f.worker);
    assert_eq!(both.len(), 2);
    assert_eq!(both[&f.ids[0]], old[&f.ids[0]]);
    f.worker.refresh_volcano_policy().await.unwrap();
    assert_eq!(leases(&f.worker)[&glm], both[&glm]);
    ingest(
        &f,
        &[
            row(1, 1, "a", false),
            row(2, 1, "b", false),
            row(3, 1, "c", false),
        ],
    );
    f.worker.refresh_volcano_policy().await.unwrap();
    assert_eq!(leases(&f.worker).len(), 1);
    assert_eq!(leases(&f.worker)[&glm], both[&glm]);
    f.worker
        .publish_glm_cycle(&glm_cycle(&f, glm, 2), now() * 1000)
        .unwrap();
    assert!(!leases(&f.worker).contains_key(&f.ids[0]));
    assert!(f.fake.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn observed_worker_failure_and_unknown_commit_do_not_publish_partial_state() {
    for unknown in [false, true] {
        let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
        initialize(&f);
        f.worker.refresh_volcano_policy().await.unwrap();
        ingest(&f, &[row(1, 1, "a", false)]);
        let before = db_image(&f);
        let old = leases(&f.worker);
        let disabled = f.worker.core.lock().unwrap().source_disabled.clone();
        let epoch = f.worker.core.lock().unwrap().input_epoch;
        let conn = rusqlite::Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap();
        conn.execute_batch(if unknown{"CREATE TABLE bad_parent(id INTEGER PRIMARY KEY);CREATE TABLE bad_child(id INTEGER REFERENCES bad_parent(id) DEFERRABLE INITIALLY DEFERRED);CREATE TRIGGER fail AFTER UPDATE ON volcano_runtime BEGIN INSERT INTO bad_child VALUES(1); END;"}else{"CREATE TRIGGER fail BEFORE UPDATE ON volcano_runtime BEGIN SELECT RAISE(ABORT,'synthetic'); END;"}).unwrap();
        assert!(f.worker.refresh_volcano_policy().await.is_err());
        assert_eq!(db_image(&f), before);
        assert_eq!(leases(&f.worker), old);
        let c = f.worker.core.lock().unwrap();
        assert_eq!(c.source_disabled, disabled);
        assert_eq!(c.input_epoch, epoch);
        assert_eq!(c.store.usable().is_err(), unknown);
        assert_eq!(
            f.worker.publication.load(Ordering::SeqCst) % 2,
            u64::from(unknown)
        );
        assert!(f.fake.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn observed_worker_dry_run_queue_full_and_uninitialized_do_not_authorize() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
    let report = f.worker.refresh_volcano_policy().await.unwrap();
    assert_eq!(report.permitted, 0);
    assert!(leases(&f.worker).is_empty());
    let slots = f.worker.commands.try_acquire_many(64).unwrap();
    assert_eq!(
        f.worker.refresh_volcano_policy().await.err().unwrap().code,
        ControlErrorCode::QueueFull
    );
    drop(slots);
    let mut config = (*policy_config()).clone();
    config.dry_run = true;
    let f = fixture_with_options(
        SourceKind::Volcano,
        &[(0, 0, 100, Some(1))],
        true,
        Arc::new(config),
    )
    .await;
    initialize(&f);
    ingest(&f, &[row(1, 1, "a", false)]);
    let before = db_image(&f);
    let old = leases(&f.worker);
    let epoch = f.worker.core.lock().unwrap().input_epoch;
    let publication = f.worker.publication.load(Ordering::SeqCst);
    assert!(
        f.worker
            .refresh_volcano_policy()
            .await
            .unwrap()
            .preview_only
    );
    assert_eq!(db_image(&f), before);
    assert_eq!(leases(&f.worker), old);
    assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
    assert_eq!(f.worker.publication.load(Ordering::SeqCst), publication);
    assert!(f.fake.calls.lock().unwrap().is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn observed_worker_pb4_provider_publications_preserve_other_scope(events in prop::collection::vec(any::<bool>(),1..15)){
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
            let f=fixture(SourceKind::Volcano,&[(0,0,100,Some(1))]).await;initialize(&f);let glm=add_glm(&f);let mut sequence=0;
            for use_glm in events{let before=leases(&f.worker);if use_glm{sequence+=1;f.worker.publish_glm_cycle(&glm_cycle(&f,glm,sequence),now()*1000).unwrap();if let Some(end)=before.get(&f.ids[0]){assert_eq!(leases(&f.worker)[&f.ids[0]],*end);}}else{f.worker.refresh_volcano_policy().await.unwrap();if let Some(end)=before.get(&glm){assert_eq!(leases(&f.worker)[&glm],*end);}}}
            assert!(f.fake.calls.lock().unwrap().is_empty());
        });
    }
}
