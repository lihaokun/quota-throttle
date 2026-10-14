use super::*;
use crate::newapi::transport::tests::{Fake, Reply};
use proptest::prelude::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::AtomicUsize;

#[test]
fn r10_worker_private_snapshots_have_no_debug_or_serialize() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(Core, std::fmt::Debug);
    assert_not_impl!(Core, serde::Serialize);
    assert_not_impl!(CachedObservation, std::fmt::Debug);
    assert_not_impl!(CachedObservation, serde::Serialize);
    assert_not_impl!(Plan, std::fmt::Debug);
    assert_not_impl!(Plan, serde::Serialize);
    assert_not_impl!(ReconcileWorker, std::fmt::Debug);
    assert_not_impl!(ReconcileWorker, serde::Serialize);
}

#[path = "scenarios.rs"]
mod scenarios;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteFault {
    Reject,
    Uncertain,
    PartialLower(i64),
}

#[derive(Default)]
struct NativeState {
    channels: Mutex<HashMap<i64, Value>>,
    bad_create: AtomicBool,
    slow_enable: AtomicBool,
    enables: AtomicUsize,
    read_failures: AtomicUsize,
    read_reject: AtomicBool,
    rejections: AtomicUsize,
    write_fault: Mutex<Option<(i64, WriteFault)>>,
    check_routing: AtomicBool,
    routing_violations: Mutex<Vec<&'static str>>,
    routing_targets: Mutex<HashMap<i64, i64>>,
    observed_remote: Mutex<HashMap<i64, (i64, i64)>>,
}
impl NativeState {
    fn reply(&self, r: &crate::newapi::transport::tests::Recorded) -> Reply {
        if r.method == "GET"
            && self
                .read_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    if n > 0 {
                        Some(n - 1)
                    } else {
                        None
                    }
                })
                .is_ok()
        {
            return Reply {
                status: 503,
                ..Reply::json(json!({"success":false}))
            };
        }
        if r.method == "GET" && self.read_reject.load(Ordering::SeqCst) {
            self.rejections.fetch_add(1, Ordering::SeqCst);
            return Reply {
                delay: Duration::from_millis(300),
                ..Reply::json(json!({"success":false}))
            };
        }
        let target = if r.method == "PUT" {
            r.body["id"].as_i64()
        } else if r.method == "POST" && r.path.ends_with("/status") {
            r.path.split('/').nth(3).and_then(|s| s.parse().ok())
        } else {
            None
        };
        let fault = {
            let mut fault = self.write_fault.lock().unwrap();
            if fault.as_ref().is_some_and(|(id, _)| Some(*id) == target) {
                fault.take().map(|(_, f)| f)
            } else {
                None
            }
        };
        if fault == Some(WriteFault::Reject) {
            return Reply {
                status: 400,
                ..Reply::json(json!({"success":false}))
            };
        }
        let mut channels = self.channels.lock().unwrap();
        if let Some(WriteFault::PartialLower(priority)) = fault {
            assert_eq!(r.method, "PUT");
            channels.get_mut(&target.unwrap()).unwrap()["priority"] = json!(priority);
            return Reply {
                status: 400,
                ..Reply::json(json!({"success":false}))
            };
        }
        if self.check_routing.load(Ordering::SeqCst) {
            if let Some(id) = target {
                let before = &channels[&id];
                if r.method == "PUT"
                    && before["status"] == 1
                    && r.body["priority"]
                        .as_i64()
                        .is_some_and(|p| p > 10 && p > before["priority"].as_i64().unwrap())
                    && channels.iter().any(|(other, c)| {
                        *other != id
                            && c["group"] == before["group"]
                            && c["status"] == 1
                            && c["priority"].as_i64() >= r.body["priority"].as_i64()
                    })
                {
                    self.routing_violations
                        .lock()
                        .unwrap()
                        .push("promotion before lowering peer");
                }
                if r.method == "POST"
                    && r.body["status"] == 1
                    && before["auto_ban"] == 0
                    && before["priority"].as_i64().unwrap() > 10
                {
                    self.routing_violations
                        .lock()
                        .unwrap()
                        .push("Volcano enabled before standby");
                }
                let routing = r.method == "POST" && r.body["status"] == 1
                    || r.method == "PUT"
                        && before["status"] == 1
                        && r.body["priority"]
                            .as_i64()
                            .is_some_and(|p| p > before["priority"].as_i64().unwrap());
                if routing {
                    let seen = self.observed_remote.lock().unwrap();
                    let targets = self.routing_targets.lock().unwrap();
                    for (peer, c) in channels
                        .iter()
                        .filter(|(_, c)| c["group"] == before["group"])
                    {
                        let current = (
                            c["status"].as_i64().unwrap(),
                            c["priority"].as_i64().unwrap(),
                        );
                        if seen.get(peer) != Some(&current) {
                            self.routing_violations
                                .lock()
                                .unwrap()
                                .push("routing before actual readback");
                        }
                        if *peer != id
                            && current.0 == 1
                            && targets.get(peer).is_some_and(|p| current.1 > *p)
                        {
                            self.routing_violations
                                .lock()
                                .unwrap()
                                .push("peer has not reached its target priority");
                        }
                    }
                }
            }
        }
        if r.method == "GET" && r.path.starts_with("/api/channel/?") {
            let url = reqwest::Url::parse(&format!("http://fixture{}", r.path)).unwrap();
            let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
            let page = query["p"].parse::<usize>().unwrap();
            let group = query.get("group");
            let mut items: Vec<_> = channels
                .values()
                .filter(|v| group.is_none_or(|g| v["group"].as_str() == Some(g)))
                .cloned()
                .collect();
            items.sort_by_key(|v| v["id"].as_i64().unwrap());
            let total = items.len();
            let items: Vec<_> = items.into_iter().skip((page - 1) * 100).take(100).collect();
            for c in &items {
                self.observed_remote.lock().unwrap().insert(
                    c["id"].as_i64().unwrap(),
                    (
                        c["status"].as_i64().unwrap(),
                        c["priority"].as_i64().unwrap(),
                    ),
                );
            }
            return Reply::json(
                json!({"success":true,"data":{"total":total,"page":page,"page_size":100,"items":items}}),
            );
        }
        if r.method == "GET" {
            let id = r.path.rsplit('/').next().unwrap().parse::<i64>().unwrap();
            let c = &channels[&id];
            self.observed_remote.lock().unwrap().insert(
                id,
                (
                    c["status"].as_i64().unwrap(),
                    c["priority"].as_i64().unwrap(),
                ),
            );
            return Reply::json(json!({"success":true,"data":channels[&id]}));
        }
        if r.method == "POST" && r.path == "/api/channel/" {
            let id = channels.keys().max().copied().unwrap_or(0) + 1;
            let mut c = r.body["channel"].clone();
            c["id"] = json!(id);
            c["channel_info"] = json!({"is_multi_key":false});
            if c.get("auto_ban").is_none() {
                c["auto_ban"] = json!(1);
            }
            assert_eq!(c["status"], 2);
            channels.insert(id, c);
            if self.bad_create.swap(false, Ordering::SeqCst) {
                return Reply {
                    body: "fake-broken-receipt".into(),
                    ..Reply::json(json!({}))
                };
            }
        } else if r.method == "POST" && r.path.ends_with("/status") {
            let id = r.path.split('/').nth(3).unwrap().parse::<i64>().unwrap();
            channels.get_mut(&id).unwrap()["status"] = r.body["status"].clone();
            if r.body["status"] == 1 {
                self.enables.fetch_add(1, Ordering::SeqCst);
                if self.slow_enable.load(Ordering::SeqCst) {
                    return Reply {
                        delay: Duration::from_millis(300),
                        ..Reply::json(json!({"success":true}))
                    };
                }
            }
        } else if r.method == "PUT" {
            let id = r.body["id"].as_i64().unwrap();
            let existing = channels.get_mut(&id).unwrap().as_object_mut().unwrap();
            for (k, v) in r.body.as_object().unwrap() {
                existing.insert(k.clone(), v.clone());
            }
        } else if r.path == "/api/user/login" {
            return Reply::json(json!({"success":true,"data":{"id":1}}));
        } else {
            panic!("unexpected fake management request");
        }
        if fault == Some(WriteFault::Uncertain) {
            return Reply {
                body: "fake-lost-write-receipt".into(),
                ..Reply::json(json!({}))
            };
        }
        Reply::json(json!({"success":true}))
    }
    fn status(&self, id: i64) -> i64 {
        self.channels.lock().unwrap()[&id]["status"]
            .as_i64()
            .unwrap()
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    fake: Fake,
    state: Arc<NativeState>,
    worker: Arc<ReconcileWorker>,
    ids: Vec<MembershipId>,
}
fn operation(n: usize) -> OperationId {
    OperationId::new(format!("worker-operation-{n:016}")).unwrap()
}
async fn fixture(kind: SourceKind, entries: &[(usize, usize, i64, Option<i64>)]) -> Fixture {
    let state = Arc::new(NativeState::default());
    let server = state.clone();
    let fake = Fake::start(move |r| server.reply(r)).await;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("catalog.sqlite3");
    let mut store = CatalogStore::open(&path).unwrap();
    let mut sources = HashMap::new();
    let mut groups = HashMap::new();
    let mut ids = Vec::new();
    for (n, (s, g, priority, status)) in entries.iter().enumerate() {
        if !sources.contains_key(s) {
            let upstream = UpstreamProfile {
                channel_type: 8,
                base_url: format!("https://source-{s}.invalid/v1"),
                models: vec![format!("model-{s}")],
                model_discovery: None,
            };
            let settings = if kind == SourceKind::Glm {
                SourceSettings::Glm {
                    upstream,
                    quota: GlmQuotaProfile {
                        quota_url: "https://quota.invalid".into(),
                        global_headers: vec![],
                        key_headers: vec![],
                    },
                }
            } else {
                SourceSettings::Volcano {
                    upstream,
                    policy: VolcanoPolicy::default(),
                }
            };
            sources.insert(
                *s,
                store
                    .create_source(SourceDraft {
                        name: format!("source-{s}"),
                        note: String::new(),
                        settings,
                        credential: SecretString::new(format!("fake-upstream-key-{s}")).unwrap(),
                    })
                    .unwrap()
                    .source_id,
            );
        }
        if !groups.contains_key(g) {
            groups.insert(
                *g,
                store
                    .create_group(GroupDraft {
                        name: format!("group-{g}"),
                        newapi_group: format!("group-{g}"),
                        kind,
                    })
                    .unwrap()
                    .group_id,
            );
        }
        let id = store
            .plan_membership(sources[s], groups[g], *priority)
            .unwrap()
            .membership_id;
        ids.push(id);
        if let Some(status) = status {
            // 本测试私有目录中构造已导入渠道；不是生产组注册或实例核验。
            let conn = rusqlite::Connection::open(&path).unwrap();
            let channel = (n + 1) as i64;
            conn.execute(
                "UPDATE memberships SET channel_id=?1,lifecycle='imported' WHERE membership_id=?2",
                rusqlite::params![channel, id.get()],
            )
            .unwrap();
            conn.execute(
                "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
                [id.get()],
            )
            .unwrap();
            state.channels.lock().unwrap().insert(channel,json!({"id":channel,"name":format!("imported-{n}"),"type":8,"group":format!("group-{g}"),"base_url":format!("https://source-{s}.invalid/v1"),"models":format!("model-{s}"),"priority":priority,"status":status,"auto_ban":if kind==SourceKind::Glm {1}else {0},"channel_info":{"is_multi_key":false}}));
        }
        store
            .submit_operation(
                operation(n),
                ControlCommand::EnsureMember {
                    membership_id: id,
                    expected_revision: 1,
                    status: StatusDirective::Enabled,
                },
                now(),
            )
            .unwrap();
    }
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE source_groups SET ready=1", [])
        .unwrap();
    let worker = ReconcileWorker::new(
        store,
        Arc::new(NewApiClient::new(&fake.config(true)).unwrap()),
    );
    worker
        .publish_backend(BackendReadiness::from_preflight(
            SUPPORTED_BACKEND_VERSION,
            NativeRecoveryCheck::Disabled,
            true,
            now() - 1,
            now() + 3600,
        ))
        .unwrap();
    for id in &ids {
        publish(&worker, *id, false);
    }
    Fixture {
        _dir: dir,
        fake,
        state,
        worker,
        ids,
    }
}
fn publish(worker: &ReconcileWorker, id: MembershipId, disabled: bool) {
    let w = worker.work(id).unwrap();
    let permit = (!disabled).then(|| {
        RoutingPermit::from_policy(
            &w.source,
            &w.group,
            w.target.desired_priority,
            10,
            true,
            now() - 1,
            now() + 3600,
        )
    });
    worker
        .publish_route(
            id,
            w.source.config_revision,
            w.group.revision,
            disabled,
            permit,
        )
        .unwrap();
}
async fn finish(f: &Fixture) {
    for _ in 0..12 {
        f.worker.reconcile_once().await.unwrap();
        if f.ids
            .iter()
            .all(|id| !f.worker.work(*id).unwrap().target.dirty)
        {
            return;
        }
    }
    panic!("worker did not converge within bounded rounds");
}
async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(4), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn worker_creation_runs_disabled_bind_enable_then_real_readback() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, None)]).await;
    finish(&f).await;
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert_eq!(f.state.status(1), 1);
    let calls = f.fake.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/api/channel/")
            .count(),
        1
    );
    assert!(calls
        .iter()
        .filter(|r| r.path.contains("?p="))
        .all(|r| r.path.contains("group=group-0")));
}

#[tokio::test]
async fn worker_unknown_create_retry_finds_object_without_second_post() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, None)]).await;
    f.state.bad_create.store(true, Ordering::SeqCst);
    f.worker.reconcile_once().await.unwrap();
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Unknown
    );
    assert_eq!(
        f.worker.work(f.ids[0]).unwrap().pending_write,
        PendingWrite::Unknown
    );
    f.worker.retry(&operation(0)).await.unwrap();
    finish(&f).await;
    assert_eq!(
        f.fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/api/channel/" && r.method == "POST")
            .count(),
        1
    );
}

#[tokio::test]
async fn worker_pause_during_budget_wait_prevents_stale_enable() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(2))]).await;
    f.worker.reconcile_once().await.unwrap();
    for _ in 0..2 {
        f.worker
            .client
            .managed_channels()
            .get_channel(1)
            .await
            .unwrap();
    }
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.reconcile_once().await });
    until(|| f.worker.work(f.ids[0]).unwrap().pending_write == PendingWrite::InFlight).await;
    let source = f.worker.work(f.ids[0]).unwrap().source;
    f.worker
        .submit(
            operation(10),
            ControlCommand::SetSourcePaused {
                source_id: source.source_id,
                expected_revision: source.config_revision,
                paused: true,
            },
        )
        .unwrap();
    task.await.unwrap().unwrap();
    finish(&f).await;
    assert_eq!(f.state.enables.load(Ordering::SeqCst), 0);
    assert_eq!(f.state.status(1), 2);
    assert_eq!(
        f.worker.operation(&operation(10)).unwrap().phase,
        OperationPhase::Succeeded
    );
}

#[tokio::test]
async fn worker_late_enable_reply_cannot_finish_new_pause() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(2))]).await;
    f.worker.reconcile_once().await.unwrap();
    f.state.slow_enable.store(true, Ordering::SeqCst);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.reconcile_once().await });
    until(|| f.state.enables.load(Ordering::SeqCst) == 1).await;
    let source = f.worker.work(f.ids[0]).unwrap().source;
    f.worker
        .submit(
            operation(10),
            ControlCommand::SetSourcePaused {
                source_id: source.source_id,
                expected_revision: source.config_revision,
                paused: true,
            },
        )
        .unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(
        f.worker
            .operation(&operation(10))
            .unwrap()
            .confirmed_targets,
        0
    );
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Superseded
    );
    finish(&f).await;
    assert_eq!(f.state.status(1), 2);
    assert_eq!(f.state.enables.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn worker_caller_cancellation_does_not_cancel_dispatched_write() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(2))]).await;
    f.worker.reconcile_once().await.unwrap();
    f.state.slow_enable.store(true, Ordering::SeqCst);
    let worker = f.worker.clone();
    let caller = tokio::spawn(async move { worker.reconcile_once().await });
    until(|| f.state.enables.load(Ordering::SeqCst) == 1).await;
    caller.abort();
    let _ = caller.await;
    until(|| f.worker.work(f.ids[0]).unwrap().pending_write == PendingWrite::None).await;
    finish(&f).await;
    assert_eq!(f.state.status(1), 1);
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Succeeded
    );
}

#[tokio::test]
async fn worker_group_lowering_is_observed_before_other_source_promotion() {
    let f = fixture(
        SourceKind::Volcano,
        &[(0, 0, 100, Some(1)), (1, 0, 10, Some(1))],
    )
    .await;
    finish(&f).await;
    let group = f.worker.work(f.ids[0]).unwrap().group;
    f.worker
        .submit(
            operation(10),
            ControlCommand::SetGroupPriorities {
                group_id: group.group_id,
                expected_revision: group.revision,
                priorities: vec![(f.ids[0], 10), (f.ids[1], 100)],
            },
        )
        .unwrap();
    for id in &f.ids {
        publish(&f.worker, *id, false);
    }
    finish(&f).await;
    let calls = f.fake.calls.lock().unwrap();
    let puts: Vec<_> = calls
        .iter()
        .filter(|r| r.method == "PUT")
        .map(|r| {
            (
                r.body["id"].as_i64().unwrap(),
                r.body["priority"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(puts, vec![(1, 10), (2, 100)]);
    assert_eq!(
        f.worker.operation(&operation(10)).unwrap().phase,
        OperationPhase::Succeeded
    );
}

#[tokio::test]
async fn worker_source_gate_is_shared_across_groups_and_can_wake_deferred_work() {
    let f = fixture(
        SourceKind::Volcano,
        &[(0, 0, 10, Some(1)), (0, 1, 10, Some(1))],
    )
    .await;
    finish(&f).await;
    publish(&f.worker, f.ids[0], true);
    for _ in 0..3 {
        f.worker.reconcile_once().await.unwrap();
    }
    assert_eq!((f.state.status(1), f.state.status(2)), (2, 2));
    assert!(f
        .ids
        .iter()
        .all(|id| f.worker.work(*id).unwrap().policy_disabled));
    publish(&f.worker, f.ids[0], false);
    finish(&f).await;
    assert_eq!((f.state.status(1), f.state.status(2)), (1, 1));
}

#[tokio::test]
async fn worker_glm_preserves_native_disable_and_rejects_supplier_stop_gate() {
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1))]).await;
    finish(&f).await;
    f.state.channels.lock().unwrap().get_mut(&1).unwrap()["status"] = json!(3);
    let w = f.worker.work(f.ids[0]).unwrap();
    f.worker
        .submit(
            operation(10),
            ControlCommand::SyncSourceModels {
                source_id: w.source.source_id,
                expected_revision: w.source.config_revision,
                models: vec!["new-glm-model".into()],
            },
        )
        .unwrap();
    publish(&f.worker, f.ids[0], false);
    finish(&f).await;
    assert_eq!(f.state.status(1), 3);
    assert_eq!(f.state.enables.load(Ordering::SeqCst), 0);
    let w = f.worker.work(f.ids[0]).unwrap();
    assert_eq!(
        f.worker
            .publish_route(
                f.ids[0],
                w.source.config_revision,
                w.group.revision,
                true,
                None
            )
            .unwrap_err()
            .code,
        ControlErrorCode::InvalidInput
    );
}

#[tokio::test]
async fn worker_failed_publication_is_atomic_and_slots_are_bounded() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(1))]).await;
    let before = f.worker.publication.load(Ordering::SeqCst);
    let w = f.worker.work(f.ids[0]).unwrap();
    rusqlite::Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap().execute_batch("CREATE TRIGGER fail_wake BEFORE UPDATE ON member_control BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(f
        .worker
        .publish_route(
            f.ids[0],
            w.source.config_revision,
            w.group.revision,
            true,
            None
        )
        .is_err());
    assert!(!f.worker.work(f.ids[0]).unwrap().policy_disabled);
    assert!(!f
        .worker
        .gate(before, now() - 1, now() + 100, &AtomicBool::new(false)));
    let permits: Vec<_> = (0..64)
        .map(|_| f.worker.commands.try_acquire().unwrap())
        .collect();
    assert_eq!(
        f.worker
            .submit(
                operation(10),
                ControlCommand::EnsureMember {
                    membership_id: f.ids[0],
                    expected_revision: w.target.revision,
                    status: StatusDirective::Disabled
                }
            )
            .unwrap_err()
            .code,
        ControlErrorCode::QueueFull
    );
    drop(permits);
}

#[tokio::test]
async fn worker_background_run_exits_on_shutdown_without_new_write() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(1))]).await;
    finish(&f).await;
    let (sender, receiver) = watch::channel(false);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.run(receiver).await });
    sender.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(f.state.enables.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn worker_read_5xx_retries_then_remains_recoverable() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(1))]).await;
    f.state.read_failures.store(10, Ordering::SeqCst);
    f.worker.reconcile_once().await.unwrap();
    let w = f.worker.work(f.ids[0]).unwrap();
    assert!(w.target.dirty);
    assert!(w.target.next_attempt_at >= now() + 29);
    assert_eq!(w.pending_write, PendingWrite::None);
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Waiting
    );
    assert_eq!(f.fake.calls.lock().unwrap().len(), 3);
    f.state.read_failures.store(0, Ordering::SeqCst);
    publish(&f.worker, f.ids[0], false);
    finish(&f).await;
}

#[tokio::test]
async fn worker_old_read_failure_does_not_override_new_ready_publication() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(1))]).await;
    f.state.read_reject.store(true, Ordering::SeqCst);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.reconcile_once().await });
    until(|| f.state.rejections.load(Ordering::SeqCst) > 0).await;
    f.state.read_reject.store(false, Ordering::SeqCst);
    publish(&f.worker, f.ids[0], false);
    task.await.unwrap().unwrap();
    let w = f.worker.work(f.ids[0]).unwrap();
    assert!(w.target.dirty);
    assert!(w.target.next_attempt_at <= now());
    assert_eq!(w.target.last_error_code, None);
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Queued
    );
    finish(&f).await;
}

#[tokio::test]
async fn worker_missing_permits_do_not_wait_on_each_other_forever() {
    let f = fixture(
        SourceKind::Volcano,
        &[(0, 0, 10, Some(2)), (1, 0, 10, Some(2))],
    )
    .await;
    for id in &f.ids {
        let w = f.worker.work(*id).unwrap();
        f.worker
            .publish_route(*id, w.source.config_revision, w.group.revision, false, None)
            .unwrap();
    }
    for _ in 0..3 {
        f.worker.reconcile_once().await.unwrap();
    }
    assert!(f
        .worker
        .core
        .lock()
        .unwrap()
        .store
        .due_members(now())
        .unwrap()
        .is_empty());
    assert!(f
        .ids
        .iter()
        .all(|id| f.worker.work(*id).unwrap().target.next_attempt_at > now()));
    assert_eq!(f.state.enables.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn worker_supplier_stop_is_prioritized_before_the_64_member_window() {
    let mut entries: Vec<_> = (0..64).map(|s| (s, 0, 10, Some(2))).collect();
    entries.push((64, 1, 10, Some(1)));
    let f = fixture(SourceKind::Volcano, &entries).await;
    for id in &f.ids[..64] {
        let w = f.worker.work(*id).unwrap();
        f.worker
            .publish_route(*id, w.source.config_revision, w.group.revision, false, None)
            .unwrap();
    }
    let stop = AtomicBool::new(false);
    for id in [f.ids[0], f.ids[64]] {
        let p = f.worker.assemble(id).unwrap();
        f.worker.observe(&p, &stop).await.unwrap();
    }
    publish(&f.worker, f.ids[64], true);
    let report = f.worker.reconcile_once().await.unwrap();
    assert_eq!(report.visited, 64);
    assert_eq!(f.state.status(65), 2);
    assert!(f.worker.work(f.ids[63]).unwrap().target.dirty);
}

#[tokio::test]
async fn worker_missing_id_invalidates_positive_cache_and_does_not_hot_poll() {
    let f = fixture(
        SourceKind::Volcano,
        &[(0, 0, 10, Some(1)), (1, 0, 10, Some(1))],
    )
    .await;
    let stop = AtomicBool::new(false);
    let p = f.worker.assemble(f.ids[0]).unwrap();
    f.worker.observe(&p, &stop).await.unwrap();
    assert!(f.worker.assemble(f.ids[1]).unwrap().observation.is_some());
    f.state.channels.lock().unwrap().remove(&2);
    let p = f.worker.assemble(f.ids[0]).unwrap();
    f.worker.observe(&p, &stop).await.unwrap();
    let p = f.worker.assemble(f.ids[1]).unwrap();
    assert!(p.observation.is_none());
    assert_eq!(p.step(), PlanStep::Reject(ControlErrorCode::BindingDrift));
    assert_eq!(
        f.worker.work(f.ids[1]).unwrap().member.observed_status,
        None
    );
    let count = f.fake.calls.lock().unwrap().len();
    f.worker.reconcile_once().await.unwrap();
    f.worker.reconcile_once().await.unwrap();
    assert_eq!(f.fake.calls.lock().unwrap().len(), count);
    assert_eq!(
        f.worker.operation(&operation(1)).unwrap().phase,
        OperationPhase::Failed
    );
    assert_eq!(
        f.worker.operation(&operation(1)).unwrap().confirmed_targets,
        0
    );
}

#[tokio::test]
async fn worker_explicit_retry_observes_repaired_id_without_waiting_for_cache_ttl() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(1))]).await;
    let channel = f.state.channels.lock().unwrap().remove(&1).unwrap();
    f.worker.reconcile_once().await.unwrap();
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Failed
    );
    assert!(matches!(
        f.worker.assemble(f.ids[0]).unwrap().step(),
        PlanStep::Reject(ControlErrorCode::BindingDrift)
    ));
    f.state.channels.lock().unwrap().insert(1, channel);
    let count = f.fake.calls.lock().unwrap().len();
    f.worker.retry(&operation(0)).await.unwrap();
    finish(&f).await;
    assert!(f.fake.calls.lock().unwrap().len() > count);
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Succeeded
    );
    let cached = f.worker.core.lock().unwrap().observations.len();
    let count = f.fake.calls.lock().unwrap().len();
    f.worker.retry(&operation(0)).await.unwrap();
    assert_eq!(f.worker.core.lock().unwrap().observations.len(), cached);
    assert_eq!(f.fake.calls.lock().unwrap().len(), count);
}

#[tokio::test]
async fn worker_background_lane_busy_is_not_a_terminal_failure() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(1))]).await;
    finish(&f).await;
    let lane = f.worker.lane.lock().await;
    let (sender, receiver) = watch::channel(false);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.run(receiver).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!task.is_finished());
    drop(lane);
    sender.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn worker_stale_creation_refresh_failure_defers_and_preserves_storage_errors() {
    for fail_storage in [false, true] {
        let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(1))]).await;
        let p = f.worker.assemble(f.ids[0]).unwrap();
        // 通用 fixture 是 imported 名称；此 helper 用例需模拟稳定创建名匹配。
        f.state.channels.lock().unwrap().get_mut(&1).unwrap()["name"] =
            json!(p.work.target.create_name);
        let mut channel = f
            .worker
            .client
            .managed_channels()
            .get_channel(1)
            .await
            .unwrap()
            .observation()
            .unwrap();
        channel.observed_at = SystemTime::UNIX_EPOCH;
        assert_eq!(channel.name, p.work.target.create_name);
        if fail_storage {
            let conn = rusqlite::Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap();
            conn.execute_batch("CREATE TRIGGER reject_defer BEFORE UPDATE ON member_control BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        }
        f.state.read_failures.store(1, Ordering::SeqCst);
        let result = f
            .worker
            .refresh_creation_observations(&p, &AtomicBool::new(false), &mut [channel])
            .await;
        assert_eq!(
            result.unwrap_err().code,
            if fail_storage {
                ControlErrorCode::Storage
            } else {
                ControlErrorCode::TransportUnknown
            }
        );
        let w = f.worker.work(f.ids[0]).unwrap();
        assert!(w.target.dirty);
        assert_eq!(w.target.next_attempt_at > now(), !fail_storage);
    }
}

#[tokio::test]
async fn worker_catalog_wrapper_preserves_duplicate_identity_and_counter_never_wraps() {
    let f = fixture(SourceKind::Volcano, &[(0, 0, 10, Some(1))]).await;
    let result = f.worker.create_source(SourceDraft {
        name: "duplicate".into(),
        note: String::new(),
        credential: SecretString::new("fake-upstream-key-0".into()).unwrap(),
        settings: SourceSettings::Volcano {
            upstream: UpstreamProfile {
                channel_type: 8,
                base_url: "https://source-0.invalid/v1".into(),
                models: vec!["model-0".into()],
                model_discovery: None,
            },
            policy: VolcanoPolicy::default(),
        },
    });
    let error = result.unwrap_err();
    assert_eq!(error.kind, ErrorKind::DuplicateSource);
    assert_eq!(
        error.source_id,
        Some(f.worker.work(f.ids[0]).unwrap().source.source_id)
    );
    f.worker.publication.store(u64::MAX - 3, Ordering::SeqCst);
    assert_eq!(
        f.worker
            .publish_backend(BackendReadiness::from_preflight(
                "",
                NativeRecoveryCheck::Unknown,
                false,
                0,
                0
            ))
            .unwrap_err()
            .code,
        ControlErrorCode::Storage
    );
    assert_eq!(f.worker.publication.load(Ordering::SeqCst), u64::MAX);
    assert_eq!(
        f.worker.reconcile_once().await.unwrap_err().code,
        ControlErrorCode::Storage
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn worker_pbt_publication_never_revalidates_old_epoch(changes in prop::collection::vec(any::<bool>(),1..25)) {
        let runtime=tokio::runtime::Runtime::new().unwrap();
        let f=runtime.block_on(fixture(SourceKind::Volcano,&[(0,0,10,Some(1))]));
        let mut old=Vec::new();
        for change in changes {let epoch=f.worker.publication.load(Ordering::SeqCst);old.push(epoch);let w=f.worker.work(f.ids[0]).unwrap();
            if change {f.worker.submit(operation(old.len()+10),ControlCommand::EnsureMember {membership_id:f.ids[0],expected_revision:w.target.revision,status:StatusDirective::Disabled}).unwrap();}
            else {f.worker.publish_backend(BackendReadiness::from_preflight(SUPPORTED_BACKEND_VERSION,NativeRecoveryCheck::Disabled,true,now()-1,now()+3600)).unwrap();}
            let current=f.worker.publication.load(Ordering::SeqCst);prop_assert!(current>epoch && current.is_multiple_of(2));
            for previous in &old {prop_assert!(!f.worker.gate(*previous,now()-1,now()+3600,&AtomicBool::new(false)));}
        }
        prop_assert!(f.fake.calls.lock().unwrap().is_empty());
    }
}
