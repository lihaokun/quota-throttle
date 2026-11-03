use super::*;

fn populate(worker: &ReconcileWorker, count: i64) {
    initialize(worker);
    let rows:Vec<_>=(1..=count).map(|id|json!({"id":id,"type":2,"created_at":125,"channel":1001,"token_id":1,"model_name":"fake-model","group":"repair-group","prompt_tokens":1,"completion_tokens":2,"is_stream":false,"other":""})).collect();
    for chunk in rows.chunks(1024) {
        worker
            .mutate(None, |core| {
                let t = core
                    .store
                    .load_scan_ticket(ScanLane::Recent)
                    .unwrap()
                    .unwrap();
                Ok(core.store.commit_fact_page(&t, chunk, PageEnd::More, 300))
            })
            .unwrap()
            .unwrap();
    }
}
fn mapping(dir: &tempfile::TempDir, worker: &ReconcileWorker) -> SourceId {
    let (source, member) = {
        let mut core = worker.core.lock().unwrap();
        let source = core
            .store
            .create_source(SourceDraft {
                name: "repair-source".into(),
                note: String::new(),
                settings: SourceSettings::Volcano {
                    upstream: UpstreamProfile {
                        channel_type: 8,
                        base_url: "https://fake.invalid/v1".into(),
                        models: vec!["fake-model".into()],
                        model_discovery: None,
                    },
                    policy: VolcanoPolicy::default(),
                },
                credential: SecretString::new("fake-repair-only".into()).unwrap(),
            })
            .unwrap()
            .source_id;
        let group = core
            .store
            .create_group(GroupDraft {
                name: "repair-group".into(),
                newapi_group: "repair-group".into(),
                kind: SourceKind::Volcano,
            })
            .unwrap()
            .group_id;
        let member = core
            .store
            .plan_membership(source, group, 10)
            .unwrap()
            .membership_id;
        (source, member)
    };
    let db = rusqlite::Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
    db.execute("UPDATE memberships SET channel_id=1001,lifecycle='retired',retired_at=10 WHERE membership_id=?1",[member.get()]).unwrap();
    db.execute(
        "UPDATE member_control SET create_state='resolved',dirty=0 WHERE membership_id=?1",
        [member.get()],
    )
    .unwrap();
    source
}
fn unknown(dir: &tempfile::TempDir) -> i64 {
    let db = rusqlite::Connection::open_with_flags(
        dir.path().join("catalog.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    db.query_row(
        "SELECT count(*) FROM ledger_facts WHERE source_id IS NULL",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn attribution_worker_batches_and_replays_preserve_routing_and_totals() {
    let (dir, worker) = setup();
    populate(&worker, 600);
    let source = mapping(&dir, &worker);
    let member = worker.snapshot().unwrap().memberships[0].membership_id;
    let work = worker.work(member).unwrap();
    let (source_state, group_state) = {
        let mut core = worker.core.lock().unwrap();
        core.source_disabled.insert(source, true);
        core.permits.insert(
            member,
            RoutingPermit::from_policy(&work, work.target.desired_priority, 0, true, 100, 200),
        );
        core.last_glm_cycle = Some(73);
        core.backend = BackendReadiness::from_preflight(
            SUPPORTED_BACKEND_VERSION,
            NativeRecoveryCheck::Disabled,
            true,
            100,
            200,
        );
        core.observations.insert(
            member,
            CachedObservation::Unavailable {
                revision: work.revision(),
                code: ControlErrorCode::RateLimited,
                checked_at: 125,
            },
        );
        let source_state = core
            .store
            .initialize_source_policy(source, work.source.config_revision, 86_400)
            .unwrap();
        let old = core.store.load_group_policy(work.group.group_id).unwrap();
        let group_state = core
            .store
            .save_group_policy(
                work.group.group_id,
                work.group.revision,
                old.policy_revision,
                crate::catalog::policy::GroupPolicyState::Volcano(
                    crate::source_policy::volcano::VolcanoGroupState {
                        preferred: Some(member),
                        cursor: Some((10, member)),
                    },
                ),
            )
            .unwrap();
        (source_state, group_state)
    };
    let before = serde_json::to_value(worker.snapshot().unwrap()).unwrap();
    let before_usage = worker.query_usage(query()).await.unwrap().usage;
    let epoch = worker.core.lock().unwrap().input_epoch;
    let first = worker.repair_attribution_once().await.unwrap();
    assert_eq!(
        (first.checked, first.changed, first.pass_finished),
        (256, 256, false)
    );
    assert_eq!(unknown(&dir), 344);
    let second = worker.repair_attribution_once().await.unwrap();
    assert_eq!((second.checked, second.changed), (256, 256));
    let third = worker.repair_attribution_once().await.unwrap();
    assert_eq!(
        (third.checked, third.changed, third.pass_finished),
        (88, 88, true)
    );
    assert_eq!(unknown(&dir), 0);
    let replay = worker.repair_attribution_once().await.unwrap();
    assert_eq!((replay.checked, replay.changed), (256, 0));
    assert_eq!(replay.ledger_revision, third.ledger_revision);
    let after_usage = worker.query_usage(query()).await.unwrap().usage;
    let (b, a) = (before_usage.as_ref().unwrap(), after_usage.as_ref().unwrap());
    assert!(a.total == b.total && a.models == b.models && a.tokens == b.tokens && a.buckets == b.buckets);
    // 归属修复只把「未归组」挪进真实组：按组分项随之变化，总量守恒。
    assert!(b.groups.iter().any(|(group, _)| group.is_none()));
    assert!(a.groups.iter().all(|(group, _)| group.is_some()));
    assert_eq!(
        serde_json::to_value(worker.snapshot().unwrap()).unwrap(),
        before
    );
    let core = worker.core.lock().unwrap();
    assert_eq!(core.input_epoch, epoch);
    assert_eq!(core.source_disabled, HashMap::from([(source, true)]));
    assert_eq!(core.permits.len(), 1);
    let permit = core.permits.get(&member).unwrap();
    assert!(permit.valid_for(&work, 125));
    assert_eq!(permit.expires_at(), 200);
    assert_eq!(core.last_glm_cycle, Some(73));
    assert!(core.backend.ready_at(125));
    assert_eq!(core.backend.expires_at(), 200);
    assert_eq!(core.observations.len(), 1);
    assert_eq!(
        core.observations
            .get(&member)
            .unwrap()
            .problem(work.revision(), 125),
        Some(ControlErrorCode::RateLimited)
    );
    assert_eq!(
        core.store.load_source_policy(source).unwrap(),
        Some(source_state)
    );
    assert_eq!(
        core.store.load_group_policy(work.group.group_id).unwrap(),
        group_state
    );
    assert_eq!(worker.commands.available_permits(), 64);
}

#[tokio::test]
async fn attribution_worker_dry_run_busy_and_uninitialized_do_not_advance() {
    let (_dir, worker) = setup();
    assert_eq!(
        worker.repair_attribution_once().await.err(),
        Some(AnalyticsError::Ledger(LedgerError::NotInitialized))
    );
    let slot = worker.commands.try_acquire_many(64).unwrap();
    assert_eq!(
        worker.repair_attribution_once().await.err(),
        Some(AnalyticsError::Control(ControlErrorCode::QueueFull))
    );
    drop(slot);
    let cursor = worker.repair_cursor.lock().await;
    assert_eq!(
        worker.repair_attribution_once().await.err(),
        Some(AnalyticsError::Busy)
    );
    drop(cursor);
    let (dir, worker) = setup_with(true);
    populate(&worker, 300);
    mapping(&dir, &worker);
    let ticket = worker
        .core
        .lock()
        .unwrap()
        .store
        .begin_attribution_pass()
        .unwrap();
    *worker.repair_cursor.lock().await = Some(ticket.clone());
    let before = worker
        .core
        .lock()
        .unwrap()
        .store
        .load_ledger_state()
        .unwrap();
    let publication = worker.publication.load(Ordering::SeqCst);
    let report = worker.repair_attribution_once().await.unwrap();
    assert!(report.preview_only && !report.pass_finished);
    assert_eq!((report.checked, report.changed), (0, 0));
    assert_eq!(unknown(&dir), 300);
    assert!(worker.repair_cursor.lock().await.as_ref() == Some(&ticket));
    assert_eq!(
        worker
            .core
            .lock()
            .unwrap()
            .store
            .load_ledger_state()
            .unwrap(),
        before
    );
    assert_eq!(worker.publication.load(Ordering::SeqCst), publication);
}

#[tokio::test]
async fn attribution_worker_restart_revisits_earlier_unknown_facts() {
    let (dir, worker) = setup();
    populate(&worker, 600);
    assert_eq!(worker.repair_attribution_once().await.unwrap().changed, 0);
    let client = worker.client.clone();
    let config = worker.policy_config.clone();
    drop(worker);
    let worker = ReconcileWorker::new(
        CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap(),
        client,
        config,
    )
    .unwrap();
    let source = mapping(&dir, &worker);
    assert_eq!(worker.repair_attribution_once().await.unwrap().changed, 256);
    let db = rusqlite::Connection::open_with_flags(
        dir.path().join("catalog.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        db.query_row(
            "SELECT source_id FROM ledger_facts WHERE native_log_id=1",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        source.get()
    );
    while !worker
        .repair_attribution_once()
        .await
        .unwrap()
        .pass_finished
    {}
    assert_eq!(unknown(&dir), 0);
}

#[tokio::test]
async fn attribution_worker_commit_unknown_does_not_publish_cursor_or_reopen_gate() {
    let (dir, worker) = setup();
    populate(&worker, 600);
    mapping(&dir, &worker);
    worker.repair_attribution_once().await.unwrap();
    let before = worker.repair_cursor.lock().await.clone();
    let db = rusqlite::Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fault AFTER UPDATE ON ledger_facts WHEN NEW.native_log_id=257 BEGIN INSERT INTO fault_child VALUES(1); END;").unwrap();
    assert!(
        matches!(worker.repair_attribution_once().await,Err(AnalyticsError::Ledger(LedgerError::Catalog(e))) if e.kind==ErrorKind::CommitUnknown)
    );
    assert!(*worker.repair_cursor.lock().await == before);
    assert_eq!(unknown(&dir), 344);
    assert_eq!(worker.commands.available_permits(), 64);
    assert_eq!(worker.publication.load(Ordering::SeqCst) % 2, 1);
    assert!(worker.query_usage(query()).await.is_err());
}

#[tokio::test]
async fn attribution_worker_query_and_repair_share_one_atomic_ledger() {
    let (dir, worker) = setup();
    populate(&worker, 20);
    mapping(&dir, &worker);
    let old = worker
        .core
        .lock()
        .unwrap()
        .store
        .load_ledger_state()
        .unwrap();
    let q = UsageQuery::new(
        ScanWindow { from: 0, to: 300 },
        480,
        UsageGrain::Hour,
        UsageFilter {
            source: DimensionFilter::Unknown,
            ..Default::default()
        },
    )
    .unwrap();
    let (ready_tx, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    let w = worker.clone();
    let task = tokio::spawn(async move {
        w.query_usage_with(q, Duration::from_secs(60), move |target, q, c| {
            let mut tx = Some(ready_tx);
            target.test_query(q, &c, MAX_QUERY_ROWS, || {
                if let Some(tx) = tx.take() {
                    tx.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(10)).unwrap();
                }
            })
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), ready)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(worker.repair_attribution_once().await.unwrap().changed, 20);
    release.send(()).unwrap();
    let result = task.await.unwrap().unwrap();
    assert_eq!(result.state, old);
    assert_eq!(result.usage.unwrap().total.consumption_rows, 20);
    assert_eq!(unknown(&dir), 0);
}
