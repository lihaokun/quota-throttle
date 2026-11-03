use super::*;
use crate::catalog::ledger::{PageEnd, ScanLane, ScanWindow};
use crate::catalog::policy::GroupPolicyState;
use crate::quota::{QuotaStatus, WindowStatus};
use crate::source_policy::probe::{ProbeCycle, QuotaObservation, QuotaReading};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn db(f: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap()
}
fn configure(f: &Fixture, url: &str) -> SourceId {
    let id = f.worker.work(f.ids[0]).unwrap().source.source_id;
    let conn = db(f);
    let raw: String = conn
        .query_row(
            "SELECT settings_json FROM sources WHERE source_id=?1",
            [id.get()],
            |r| r.get(0),
        )
        .unwrap();
    let mut v: Value = serde_json::from_str(&raw).unwrap();
    v["settings"]["upstream"]["model_discovery"] =
        json!({"url":format!("{url}/models"),"auth":"bearer"});
    conn.execute(
        "UPDATE sources SET settings_json=?1 WHERE source_id=?2",
        rusqlite::params![v.to_string(), id.get()],
    )
    .unwrap();
    id
}
fn image(f: &Fixture) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let conn = db(f);
    [
        "sources",
        "source_groups",
        "memberships",
        "source_control",
        "member_control",
        "member_execution",
        "control_operations",
        "operation_targets",
        "group_policy_state",
        "volcano_runtime",
        "volcano_period_history",
    ]
    .into_iter()
    .map(|name| {
        let mut q = conn
            .prepare(&format!("SELECT * FROM {name} ORDER BY 1,2"))
            .unwrap();
        let n = q.column_count();
        q.query_map([], |r| (0..n).map(|i| r.get(i)).collect())
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    })
    .collect()
}
fn cycle(f: &Fixture, id: u64, pct: [f64; 2]) -> ProbeCycle {
    let at = now() * 1000;
    let observations = f
        .worker
        .snapshot()
        .unwrap()
        .sources
        .into_iter()
        .filter(|s| s.kind == SourceKind::Glm)
        .enumerate()
        .map(|(i, s)| QuotaObservation {
            source_id: s.source_id,
            config_revision: s.config_revision,
            cycle_id: id,
            observed_at_ms: at,
            reading: QuotaReading::Available(QuotaStatus {
                five_hour: Some(WindowStatus {
                    percentage: pct[i],
                    next_reset_time: at + 3600000,
                }),
                weekly: None,
                level: None,
            }),
        })
        .collect();
    ProbeCycle::test_input(id, at, observations)
}

#[tokio::test(start_paused = true)]
async fn c2_worker_pin_waits_for_policy_and_keeps_original_release_rule() {
    scenarios::drive(async {
        let f=fixture(SourceKind::Glm,&[(0,0,100,Some(1)),(1,0,10,Some(1)),(0,1,100,Some(1)),(1,1,10,Some(1))]).await;
        f.worker.publish_glm_cycle(&cycle(&f,1,[10.0,20.0]),now()*1000).unwrap();finish(&f).await;
        let w=f.worker.work(f.ids[1]).unwrap();let policy=f.worker.core.lock().unwrap().store.load_group_policy(w.group.group_id).unwrap();
        let other=f.worker.work(f.ids[2]).unwrap();let other_policy=f.worker.core.lock().unwrap().store.load_group_policy(other.group.group_id).unwrap();
        let previous=f.worker.core.lock().unwrap().permits[&f.ids[1]];
        let cmd=ControlCommand::SetGroupPin {group_id:w.group.group_id,expected_revision:w.group.revision,expected_policy_revision:policy.policy_revision,membership_id:Some(f.ids[1])};
        let calls=f.fake.calls.lock().unwrap().len();let saved=f.worker.submit(operation(500),cmd.clone()).unwrap();assert_eq!((saved.phase,saved.total_targets),(OperationPhase::Succeeded,0));assert_eq!(calls,f.fake.calls.lock().unwrap().len());assert!(!previous.valid_for(&f.worker.work(f.ids[1]).unwrap(),now()));
        assert_eq!(f.worker.core.lock().unwrap().store.load_group_policy(other.group.group_id).unwrap(),other_policy);
        f.worker.publish_glm_cycle(&cycle(&f,2,[10.0,20.0]),now()*1000).unwrap();finish(&f).await;
        assert_eq!(f.state.channels.lock().unwrap()[&2]["priority"],100);assert_eq!(f.state.channels.lock().unwrap()[&1]["priority"],10);
        f.worker.publish_glm_cycle(&cycle(&f,3,[10.0,96.0]),now()*1000).unwrap();finish(&f).await;
        assert!(matches!(f.worker.core.lock().unwrap().store.load_group_policy(w.group.group_id).unwrap().state,GroupPolicyState::Glm(s) if s.pinned.is_none()));
        assert_eq!(f.worker.submit(operation(500),cmd).unwrap(),saved);assert_eq!(f.state.channels.lock().unwrap()[&1]["priority"],100);
    }).await;
}

fn ledger(f: &Fixture, rows: &[Value]) {
    let at = now();
    let mut c = f.worker.core.lock().unwrap();
    if c.store
        .load_ledger_state()
        .unwrap()
        .initial_requested_from
        .is_none()
    {
        c.store.initialize_ledger(0, at).unwrap();
    }
    let old = c
        .store
        .load_ledger_state()
        .unwrap()
        .scans
        .into_iter()
        .find(|p| p.lane == ScanLane::Recent)
        .unwrap();
    let ticket = c
        .store
        .begin_scan(
            ScanLane::Recent,
            old.generation,
            ScanWindow { from: 0, to: at },
            at,
        )
        .unwrap();
    c.store
        .commit_fact_page(&ticket, rows, PageEnd::ReachedEnd, at)
        .unwrap();
}
#[tokio::test(start_paused = true)]
async fn c2_worker_clear_reenables_only_after_observed_publication_and_readback() {
    scenarios::drive(async {
        let f=fixture(SourceKind::Volcano,&[(0,0,100,Some(1)),(1,0,10,Some(1)),(0,1,100,Some(1)),(1,1,10,Some(1))]).await;
        ledger(&f,&[]);f.worker.refresh_volcano_policy().await.unwrap();finish(&f).await;
        let rows=(1..=3).map(|i|json!({"id":i,"type":5,"created_at":now()-1,"channel":1,"token_id":1,"model_name":"model-0","group":"group-0","prompt_tokens":0,"completion_tokens":0,"is_stream":false,"request_id":format!("failure-{i}"),"other":json!({"error_type":"openai_error","status_code":503,"error_code":"unavailable"}).to_string()})).collect::<Vec<_>>();
        ledger(&f,&rows);f.worker.refresh_volcano_policy().await.unwrap();finish(&f).await;assert_eq!(f.state.status(1),2);assert_eq!(f.state.status(3),2);
        let w=f.worker.work(f.ids[0]).unwrap();let old=f.worker.core.lock().unwrap().store.load_source_policy(w.source.source_id).unwrap().unwrap();let cmd=ControlCommand::ClearVolcanoError{source_id:w.source.source_id,expected_revision:w.source.config_revision,expected_runtime_revision:old.revision};
        let saved=f.worker.submit(operation(600),cmd.clone()).unwrap();assert_eq!(f.state.status(1),2);assert!(f.worker.work(f.ids[0]).unwrap().policy_disabled);
        assert_eq!(f.worker.submit(operation(600),cmd).unwrap(),saved);assert_eq!(f.worker.core.lock().unwrap().store.load_source_policy(w.source.source_id).unwrap().unwrap().error_epoch,old.error_epoch+1);
        f.worker.refresh_volcano_policy().await.unwrap();finish(&f).await;assert_eq!(f.state.status(1),1);assert_eq!(f.state.status(3),1);assert_eq!(f.state.channels.lock().unwrap()[&1]["priority"],10);
    }).await;
}

#[tokio::test(start_paused = true)]
async fn c2_worker_models_use_one_get_and_original_multigroup_execution() {
    scenarios::drive(async {
        let f = fixture(
            SourceKind::Glm,
            &[
                (0, 0, 100, Some(1)),
                (0, 1, 100, Some(1)),
                (1, 0, 10, Some(1)),
            ],
        )
        .await;
        let upstream =
            Fake::start(|_| Reply::json(json!({"data":[{"id":" z "},{"id":"a"},{"id":"z"}]})))
                .await;
        let source = configure(&f, &upstream.url);
        let before_other = f.worker.work(f.ids[2]).unwrap();
        let old_permit = f.worker.core.lock().unwrap().permits[&f.ids[0]];
        let saved = f
            .worker
            .discover_source_models(operation(700), source, 1)
            .await
            .unwrap();
        assert_eq!(saved.total_targets, 2);
        assert_eq!(upstream.calls.lock().unwrap().len(), 1);
        let raw = upstream.calls.lock().unwrap()[0].raw.to_lowercase();
        assert!(raw.contains("authorization: bearer fake-upstream-key-0"));
        let epoch = f.worker.core.lock().unwrap().input_epoch;
        let fence = f.worker.publication.load(Ordering::SeqCst);
        assert_eq!(
            f.worker
                .discover_source_models(operation(700), source, 1)
                .await
                .unwrap(),
            saved
        );
        assert_eq!(upstream.calls.lock().unwrap().len(), 1);
        assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
        assert_eq!(f.worker.publication.load(Ordering::SeqCst), fence);
        assert_eq!(
            f.worker.work(f.ids[2]).unwrap().revision(),
            before_other.revision()
        );
        assert!(!old_permit.valid_for(&f.worker.work(f.ids[0]).unwrap(), now()));
        assert_eq!(
            f.worker
                .discover_source_models(operation(700), source, 2)
                .await
                .err()
                .unwrap()
                .code,
            ControlErrorCode::IdempotencyConflict
        );
        assert_eq!(upstream.calls.lock().unwrap().len(), 1);
        f.worker.reconcile_once().await.unwrap();
        assert_ne!(
            f.worker.operation(&operation(700)).unwrap().phase,
            OperationPhase::Succeeded
        );
        // 新来源版本必须重新经过真实策略发布，不能拿旧许可执行Enabled目标。
        f.worker
            .publish_glm_cycle(&cycle(&f, 1, [10.0, 20.0]), now() * 1000)
            .unwrap();
        finish(&f).await;
        assert_eq!(
            f.worker.operation(&operation(700)).unwrap().phase,
            OperationPhase::Succeeded
        );
        assert_eq!(f.state.channels.lock().unwrap()[&1]["models"], "a,z");
        assert_eq!(f.state.channels.lock().unwrap()[&2]["models"], "a,z");
        assert_eq!(f.state.channels.lock().unwrap()[&3]["models"], "model-1");
    })
    .await;
}

#[tokio::test]
async fn c2_worker_bad_model_responses_leave_catalog_unchanged_and_redact_errors() {
    for kind in 0..7 {
        let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
        let upstream = Fake::start(move |_| match kind {
            0 => Reply {
                status: 503,
                ..Reply::json(json!({"message":"PRIVATE_RESPONSE"}))
            },
            1 => Reply::json(json!({"data":[]})),
            2 => Reply::json(json!({"data":[{"id":"bad,model"}]})),
            3 => Reply::json(json!({"data":[{"id":"bad\0model"}]})),
            4 => Reply::json(
                json!({"data":(0..1001).map(|i|json!({"id":format!("m{i}")})).collect::<Vec<_>>()}),
            ),
            5 => Reply {
                body: "x".repeat(1024 * 1024 + 1),
                ..Reply::json(json!(null))
            },
            _ => Reply {
                body: "PRIVATE_RESPONSE".into(),
                ..Reply::json(json!(null))
            },
        })
        .await;
        let source = configure(&f, &upstream.url);
        let before = image(&f);
        let epoch = f.worker.core.lock().unwrap().input_epoch;
        let error = f
            .worker
            .discover_source_models(operation(800), source, 1)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, ControlErrorCode::InvalidResponse);
        assert!(!format!("{error:?} {error}").contains("PRIVATE_RESPONSE"));
        assert_eq!(image(&f), before);
        assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
        assert_eq!(upstream.calls.lock().unwrap().len(), 1);
        assert!(f.fake.calls.lock().unwrap().is_empty());
    }
    // 不声明Content-Length；有效JSON的累计chunk正文也必须在1MiB停止。
    let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
    let upstream = ModelGate::start(ModelResponse::ChunkedOversized).await;
    let source = configure(&f, &upstream.url);
    let before = image(&f);
    let epoch = f.worker.core.lock().unwrap().input_epoch;
    assert_eq!(
        f.worker
            .discover_source_models(operation(801), source, 1)
            .await
            .err()
            .unwrap()
            .code,
        ControlErrorCode::InvalidResponse
    );
    assert_eq!(image(&f), before);
    assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
    assert_eq!(upstream.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.worker.commands.available_permits(), 64);
    assert!(f.worker.model_catalog.try_lock().is_ok());
    assert!(f.fake.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn c2_worker_models_no_discovery_config_redirect_or_dry_run_requests() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
    let source = f.worker.work(f.ids[0]).unwrap().source.source_id;
    assert_eq!(
        f.worker
            .discover_source_models(operation(900), source, 1)
            .await
            .err()
            .unwrap()
            .code,
        ControlErrorCode::NotReady
    );
    let target = Fake::start(|_| Reply::json(json!({"data":[{"id":"not-followed"}]}))).await;
    let target_url = target.url.clone();
    let upstream = Fake::start(move |_| Reply {
        status: 302,
        headers: format!("Location: {target_url}/models\r\n"),
        ..Reply::json(json!({}))
    })
    .await;
    configure(&f, &upstream.url);
    assert_eq!(
        f.worker
            .discover_source_models(operation(900), source, 1)
            .await
            .err()
            .unwrap()
            .code,
        ControlErrorCode::InvalidResponse
    );
    assert!(target.calls.lock().unwrap().is_empty());
    let mut cfg = (*policy_config()).clone();
    cfg.dry_run = true;
    let f = fixture_with_options(
        SourceKind::Volcano,
        &[(0, 0, 100, Some(1))],
        true,
        Arc::new(cfg),
    )
    .await;
    let source = configure(&f, &upstream.url);
    let before = image(&f);
    let calls = upstream.calls.lock().unwrap().len();
    assert_eq!(
        f.worker
            .discover_source_models(operation(901), source, 1)
            .await
            .err()
            .unwrap()
            .code,
        ControlErrorCode::NotReady
    );
    assert_eq!(
        f.worker
            .submit(
                operation(902),
                ControlCommand::UpdateVolcanoPolicy {
                    source_id: source,
                    expected_revision: 1,
                    expected_runtime_revision: 0,
                    policy: VolcanoPolicy::default()
                }
            )
            .err()
            .unwrap()
            .code,
        ControlErrorCode::NotReady
    );
    assert_eq!(image(&f), before);
    assert_eq!(upstream.calls.lock().unwrap().len(), calls);
}

#[derive(Clone, Copy)]
enum ModelResponse {
    Wait,
    ChunkedOversized,
    StalledBody,
}
struct ModelGate {
    url: String,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
    calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for ModelGate {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl ModelGate {
    async fn new() -> Self {
        Self::start(ModelResponse::Wait).await
    }
    async fn start(mode: ModelResponse) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(![3000, 3001].contains(&port));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let (e, r, c) = (entered.clone(), release.clone(), calls.clone());
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{let(mut socket,_)=accepted.unwrap();let(e,r,c)=(e.clone(),r.clone(),c.clone());children.spawn(async move {
                        let mut buffer=Vec::new();let mut part=[0u8;4096];while !buffer.windows(4).any(|w|w==b"\r\n\r\n"){let n=socket.read(&mut part).await.unwrap();if n==0{return;}buffer.extend_from_slice(&part[..n]);assert!(buffer.len()<16384);}
                        c.fetch_add(1,Ordering::SeqCst);
                        if !matches!(mode, ModelResponse::Wait)
                            && socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.is_err(){return;}
                        e.notify_one();
                        if !matches!(mode, ModelResponse::ChunkedOversized) {let permit=r.acquire().await.unwrap();permit.forget();}
                        let body=json!({"data":[{"id":"fresh"}]}).to_string();
                        if matches!(mode, ModelResponse::Wait) {
                            let text=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);let _=socket.write_all(text.as_bytes()).await;
                        } else {
                            let body=if matches!(mode, ModelResponse::ChunkedOversized){format!("{}{body}"," ".repeat(1024*1024))}else{body};
                            for chunk in body.as_bytes().chunks(65536) {
                                if socket.write_all(format!("{:x}\r\n",chunk.len()).as_bytes()).await.is_err()||socket.write_all(chunk).await.is_err()||socket.write_all(b"\r\n").await.is_err(){return;}
                            }
                            let _=socket.write_all(b"0\r\n\r\n").await;
                        }
                    });},
                    Some(_)=children.join_next()=>{},
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            entered,
            release,
            calls,
            task,
        }
    }
}

#[tokio::test]
async fn c2_worker_model_response_cannot_overwrite_pause_retire_or_new_models() {
    for mode in 0..3 {
        let f = fixture(
            SourceKind::Volcano,
            &[(0, 0, 100, Some(1)), (0, 1, 100, Some(1))],
        )
        .await;
        let gate = ModelGate::new().await;
        let source = configure(&f, &gate.url);
        let worker = f.worker.clone();
        let pending = tokio::spawn(async move {
            worker
                .discover_source_models(operation(1000), source, 1)
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        let cmd = match mode {
            0 => ControlCommand::SetSourcePaused {
                source_id: source,
                expected_revision: 1,
                paused: true,
            },
            1 => ControlCommand::RetireSource {
                source_id: source,
                expected_revision: 1,
            },
            _ => ControlCommand::SyncSourceModels {
                source_id: source,
                expected_revision: 1,
                models: vec!["manual".into()],
            },
        };
        f.worker.submit(operation(1001), cmd).unwrap();
        let before = image(&f);
        gate.release.add_permits(1);
        assert_eq!(
            pending.await.unwrap().err().unwrap().code,
            ControlErrorCode::RevisionConflict
        );
        assert_eq!(image(&f), before);
        assert_eq!(
            f.worker.operation(&operation(1000)).err().unwrap().code,
            ControlErrorCode::NotFound
        );
    }
}

#[tokio::test]
async fn c2_worker_model_cancellation_and_singleflight_leave_no_half_operation() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
    let gate = ModelGate::new().await;
    let source = configure(&f, &gate.url);
    let worker = f.worker.clone();
    let before = image(&f);
    let task = tokio::spawn(async move {
        worker
            .discover_source_models(operation(1100), source, 1)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    assert_eq!(
        f.worker
            .discover_source_models(operation(1101), source, 1)
            .await
            .err()
            .unwrap()
            .code,
        ControlErrorCode::QueueFull
    );
    assert_eq!(f.worker.commands.available_permits(), 63);
    assert_eq!(image(&f), before);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(f.worker.commands.available_permits(), 64);
    assert!(f.worker.model_catalog.try_lock().is_ok());
    assert_eq!(image(&f), before);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move {
        worker
            .discover_source_models(operation(1100), source, 1)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    gate.release.add_permits(2);
    let view = task.await.unwrap().unwrap();
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        f.worker
            .discover_source_models(operation(1100), source, 1)
            .await
            .unwrap(),
        view
    );
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn c2_worker_model_timeout_is_bounded_and_does_not_publish() {
    scenarios::drive(async {
        for mode in [ModelResponse::Wait, ModelResponse::StalledBody] {
            let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
            let gate = ModelGate::start(mode).await;
            let source = configure(&f, &gate.url);
            let before = image(&f);
            let start = tokio::time::Instant::now();
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_secs(12),
                    f.worker.discover_source_models(operation(1200), source, 1)
                )
                .await
                .expect("whole response, including body, must time out before outer guard")
                .err()
                .unwrap()
                .code,
                ControlErrorCode::InvalidResponse
            );
            assert!(start.elapsed() >= Duration::from_secs(10));
            assert!(start.elapsed() < Duration::from_secs(11));
            assert_eq!(image(&f), before);
            assert_eq!(f.worker.commands.available_permits(), 64);
            assert!(f.worker.model_catalog.try_lock().is_ok());
            assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
        }
    })
    .await;
}

#[tokio::test]
async fn c2_worker_local_commit_failure_keeps_cache_and_unknown_commit_closes_gate() {
    for unknown in [false, true] {
        let f = fixture(SourceKind::Volcano, &[(0, 0, 100, Some(1))]).await;
        let source = f.worker.work(f.ids[0]).unwrap().source.source_id;
        let before = image(&f);
        let epoch = f.worker.core.lock().unwrap().input_epoch;
        let leases = f
            .worker
            .core
            .lock()
            .unwrap()
            .permits
            .iter()
            .map(|(id, p)| (*id, p.expires_at()))
            .collect::<HashMap<_, _>>();
        db(&f).execute_batch(if unknown {"CREATE TABLE c2_bad_parent(id INTEGER PRIMARY KEY);CREATE TABLE c2_bad_child(id INTEGER REFERENCES c2_bad_parent(id) DEFERRABLE INITIALLY DEFERRED);CREATE TRIGGER c2_fail AFTER INSERT ON control_operations BEGIN INSERT INTO c2_bad_child VALUES(1);END;"} else {"CREATE TRIGGER c2_fail BEFORE INSERT ON control_operations BEGIN SELECT RAISE(ABORT,'synthetic'); END;"}).unwrap();
        assert_eq!(
            f.worker
                .submit(
                    operation(1250),
                    ControlCommand::UpdateVolcanoPolicy {
                        source_id: source,
                        expected_revision: 1,
                        expected_runtime_revision: 0,
                        policy: VolcanoPolicy {
                            daily_token_limit: Some(12),
                            ..Default::default()
                        }
                    }
                )
                .err()
                .unwrap()
                .code,
            ControlErrorCode::Storage
        );
        assert_eq!(image(&f), before);
        let c = f.worker.core.lock().unwrap();
        assert_eq!(c.input_epoch, epoch);
        assert_eq!(
            c.permits
                .iter()
                .map(|(id, p)| (*id, p.expires_at()))
                .collect::<HashMap<_, _>>(),
            leases
        );
        assert_eq!(c.store.usable().is_err(), unknown);
        assert_eq!(
            f.worker.publication.load(Ordering::SeqCst) % 2,
            u64::from(unknown)
        );
        assert!(f.fake.calls.lock().unwrap().is_empty());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn c2_pb4_worker_policy_edits_invalidate_only_affected_source_versions(events in prop::collection::vec(0usize..2,1..12)) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
            let f=fixture(SourceKind::Volcano,&[(0,0,100,Some(1)),(1,0,10,Some(1)),(0,1,100,Some(1)),(1,1,10,Some(1))]).await;ledger(&f,&[]);f.worker.refresh_volcano_policy().await.unwrap();let glm=volcano_tests::add_glm(&f);f.worker.publish_glm_cycle(&cycle(&f,1,[10.0,20.0]),now()*1000).unwrap();
            let original=f.worker.core.lock().unwrap().permits.clone();let mut revisions=[1i64;2];
            for (i,which) in events.into_iter().enumerate(){let source=f.worker.work(f.ids[which]).unwrap().source.source_id;let runtime_revision=f.worker.core.lock().unwrap().store.load_source_policy(source).unwrap().unwrap().revision;
                f.worker.submit(operation(1300+i),ControlCommand::UpdateVolcanoPolicy {source_id:source,expected_revision:revisions[which],expected_runtime_revision:runtime_revision,policy:VolcanoPolicy {daily_token_limit:Some(i as i64+1),..Default::default()}}).unwrap();revisions[which]+=1;
                for j in 0..4 {let w=f.worker.work(f.ids[j]).unwrap();assert_eq!(w.source.config_revision,revisions[j%2]);assert_eq!(original[&f.ids[j]].valid_for(&w,now()),revisions[j%2]==1);}
                assert!(original[&glm].valid_for(&f.worker.work(glm).unwrap(),now()));assert_eq!(f.worker.core.lock().unwrap().permits[&glm].expires_at(),original[&glm].expires_at());
            }
            assert!(f.fake.calls.lock().unwrap().is_empty());
        });
    }
}
