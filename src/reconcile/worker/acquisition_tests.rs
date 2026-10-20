use super::*;
use crate::catalog::policy::GroupPolicyState;
use crate::newapi::transport::tests::Recorded;
use crate::orchestrator::Regime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct QuotaGate {
    url: String,
    received: Arc<AtomicUsize>,
    release: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for QuotaGate {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl QuotaGate {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(![3000, 3001].contains(&port));
        let received = Arc::new(AtomicUsize::new(0));
        let calls = received.clone();
        let release = Arc::new(Semaphore::new(0));
        let gate = release.clone();
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let (mut socket,_)=accepted.unwrap();let gate=gate.clone();let calls=calls.clone();
                        children.spawn(async move {
                            let mut buffer=Vec::new();let mut part=[0;4096];
                            while !buffer.windows(4).any(|w|w==b"\r\n\r\n") {
                                let n=socket.read(&mut part).await.unwrap();if n==0{return;}
                                buffer.extend_from_slice(&part[..n]);assert!(buffer.len()<16384);
                            }
                            calls.fetch_add(1,Ordering::SeqCst);
                            let permit=gate.acquire().await.unwrap();permit.forget();
                            let body=quota_reply(Some(1)).body;
                            let response=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                            let _=socket.write_all(response.as_bytes()).await;
                        });
                    },
                    Some(_)=children.join_next()=>{},
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            received,
            release,
            task,
        }
    }
}

fn quota_reply(pct: Option<u8>) -> Reply {
    Reply::json(pct.map(|pct|json!({"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","percentage":pct,"unit":3,"number":5,"nextResetTime":0}]}}))
        .unwrap_or_else(||json!({"success":false,"msg":"fake-secret-error"})))
}
fn request_source(r: &Recorded) -> usize {
    r.path
        .split('?')
        .next()
        .unwrap()
        .trim_start_matches("/quota/")
        .parse()
        .unwrap()
}
fn install_quota(f: &Fixture, quota_url: &str) -> Vec<SourceId> {
    let sources: Vec<_> = f
        .worker
        .snapshot()
        .unwrap()
        .sources
        .into_iter()
        .filter(|s| s.kind == SourceKind::Glm)
        .map(|s| s.source_id)
        .collect();
    let conn = db(f);
    for id in &sources {
        let raw: String = conn
            .query_row(
                "SELECT settings_json FROM sources WHERE source_id=?1",
                [id.get()],
                |r| r.get(0),
            )
            .unwrap();
        let mut value: Value = serde_json::from_str(&raw).unwrap();
        value["settings"]["quota"]["quota_url"] =
            json!(format!("{}/quota/{}?type=2", quota_url, id.get()));
        value["settings"]["quota"]["global_headers"] =
            json!([{"key":"X-Global","value":"fake-global"}]);
        value["settings"]["quota"]["key_headers"] = json!([{"key":"Bigmodel-Organization","value":format!("fake-org-{}",id.get())},{"key":"Bigmodel-Project","value":format!("fake-project-{}",id.get())}]);
        conn.execute(
            "UPDATE sources SET settings_json=?1 WHERE source_id=?2",
            rusqlite::params![value.to_string(), id.get()],
        )
        .unwrap();
    }
    f.worker.core.lock().unwrap().permits.clear();
    sources
}

#[tokio::test]
async fn acquisition_shared_sources_use_real_quota_and_keep_native_api_idle() {
    let f = fixture(
        SourceKind::Glm,
        &[
            (0, 0, 10, Some(1)),
            (1, 0, 10, Some(1)),
            (0, 1, 10, Some(1)),
            (1, 1, 10, Some(1)),
        ],
    )
    .await;
    let quota =
        Fake::start(|r| quota_reply(Some(if request_source(r) == 1 { 96 } else { 20 }))).await;
    install_quota(&f, &quota.url);
    let out = f.worker.refresh_glm_policy().await.unwrap();
    assert_eq!(out.cycle_id, 1);
    assert_eq!(out.permitted, 2);
    assert_eq!(out.changed_members, 4);
    assert_eq!(quota.calls.lock().unwrap().len(), 2);
    for r in quota.calls.lock().unwrap().iter() {
        let id = request_source(r);
        let raw = r.raw.to_ascii_lowercase();
        assert!(raw.contains(&format!(
            "authorization: bearer fake-upstream-key-{}",
            id - 1
        )));
        assert!(raw.contains(&format!("bigmodel-organization: fake-org-{id}")));
        assert!(raw.contains(&format!("bigmodel-project: fake-project-{id}")));
        assert!(raw.contains("x-global: fake-global"));
        assert!(r.path.ends_with("?type=2"));
        assert_eq!(r.method, "GET");
    }
    for (i, id) in f.ids.iter().enumerate() {
        assert_eq!(
            f.worker.work(*id).unwrap().target.desired_priority,
            if i % 2 == 0 { 0 } else { 100 }
        );
    }
    let before = image(&f);
    let second = f.worker.refresh_glm_policy().await.unwrap();
    assert_eq!(second.cycle_id, 2);
    assert_eq!(second.changed_members, 0);
    assert_eq!(second.changed_groups, 0);
    assert_eq!(image(&f), before);
    assert_eq!(quota.calls.lock().unwrap().len(), 4);
    assert!(f.fake.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn acquisition_dry_run_empty_failures_and_bad_material_are_safe() {
    let empty = fixture(SourceKind::Glm, &[]).await;
    assert_eq!(empty.worker.refresh_glm_policy().await.unwrap().waiting, 0);
    let cfg = policy_config();
    let mut dry = (*cfg).clone();
    dry.dry_run = true;
    let f =
        fixture_with_options(SourceKind::Glm, &[(0, 0, 10, Some(1))], true, Arc::new(dry)).await;
    let quota = Fake::start(|_| quota_reply(Some(1))).await;
    install_quota(&f, &quota.url);
    let before = image(&f);
    let epoch = f.worker.publication.load(Ordering::SeqCst);
    let out = f.worker.refresh_glm_policy().await.unwrap();
    assert!(out.preview_only);
    assert_eq!(out.changed_members, 1);
    assert_eq!(out.permitted, 0);
    assert_eq!(image(&f), before);
    assert_eq!(f.worker.publication.load(Ordering::SeqCst), epoch);
    assert_eq!(f.worker.core.lock().unwrap().last_glm_cycle, None);
    assert_eq!(quota.calls.lock().unwrap().len(), 1);
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1)), (1, 0, 10, Some(1))]).await;
    let quota = Fake::start(|_| quota_reply(None)).await;
    install_quota(&f, &quota.url);
    let out = f.worker.refresh_glm_policy().await.unwrap();
    assert_eq!(out.permitted, 0);
    assert_eq!(out.changed_members, 0);
    assert_eq!(quota.calls.lock().unwrap().len(), 2);
    db(&f)
        .execute(
            "UPDATE sources SET settings_json='{}' WHERE source_id=2",
            [],
        )
        .unwrap();
    let before = image(&f);
    reject(
        f.worker.refresh_glm_policy().await,
        ControlErrorCode::Storage,
    );
    assert_eq!(image(&f), before);
    assert_eq!(quota.calls.lock().unwrap().len(), 2);
    assert_eq!(f.worker.core.lock().unwrap().last_glm_cycle, Some(1));
}

#[tokio::test]
async fn acquisition_singleflight_single_slot_and_cancellation_never_publish_half_round() {
    let f = fixture(
        SourceKind::Glm,
        &[
            (0, 0, 10, Some(1)),
            (1, 0, 10, Some(1)),
            (0, 1, 10, Some(1)),
            (1, 1, 10, Some(1)),
        ],
    )
    .await;
    let delay = Arc::new(AtomicBool::new(false));
    let flag = delay.clone();
    let quota = Fake::start(move |request| {
        let mut r = quota_reply(Some(1));
        if flag.load(Ordering::SeqCst) && request_source(request) == 2 {
            r.delay = Duration::from_secs(30);
        }
        r
    })
    .await;
    install_quota(&f, &quota.url);
    // 留一个槽时也能采集并发布：不能在发布半段重复占槽。
    let slots: Vec<_> = (0..63)
        .map(|_| f.worker.commands.try_acquire().unwrap())
        .collect();
    let first = f.worker.refresh_glm_policy().await.unwrap();
    assert_eq!(first.cycle_id, 1);
    assert_eq!(first.permitted, 4);
    drop(slots);
    let before = image(&f);
    let old = leases(&f, now());
    let epoch = f.worker.core.lock().unwrap().input_epoch;
    delay.store(true, Ordering::SeqCst);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.refresh_glm_policy().await });
    until(|| quota.calls.lock().unwrap().len() == 4).await;
    reject(
        f.worker.refresh_glm_policy().await,
        ControlErrorCode::QueueFull,
    );
    assert_eq!(f.worker.commands.available_permits(), 63);
    assert!(f.worker.core.try_lock().is_ok());
    assert_eq!(image(&f), before);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(f.worker.probe.try_lock().is_ok());
    assert_eq!(f.worker.commands.available_permits(), 64);
    assert_eq!(image(&f), before);
    assert_eq!(leases(&f, now()), old);
    assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
    assert_eq!(f.worker.core.lock().unwrap().last_glm_cycle, Some(1));
    delay.store(false, Ordering::SeqCst);
    assert_eq!(f.worker.refresh_glm_policy().await.unwrap().cycle_id, 3);
    assert_eq!(quota.calls.lock().unwrap().len(), 6);
    let slots: Vec<_> = (0..64)
        .map(|_| f.worker.commands.try_acquire().unwrap())
        .collect();
    reject(
        f.worker.refresh_glm_policy().await,
        ControlErrorCode::QueueFull,
    );
    drop(slots);
    assert_eq!(quota.calls.lock().unwrap().len(), 6);
}

async fn pending_change(groups: usize, action: u8) {
    let entries: Vec<_> = (0..groups).map(|g| (0, g, 10, Some(1))).collect();
    let f = fixture(SourceKind::Glm, &entries).await;
    let quota = QuotaGate::start().await;
    let sources = install_quota(&f, &quota.url);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.refresh_glm_policy().await });
    until(|| quota.received.load(Ordering::SeqCst) == 1).await;
    let command = match action {
        0 => ControlCommand::SetSourcePaused {
            source_id: sources[0],
            expected_revision: 1,
            paused: true,
        },
        1 => ControlCommand::SyncSourceModels {
            source_id: sources[0],
            expected_revision: 1,
            models: vec!["new-model".into()],
        },
        _ => ControlCommand::RetireSource {
            source_id: sources[0],
            expected_revision: 1,
        },
    };
    // 在 quota 响应前提交控制操作，不被网络等待持有的 Core 阻塞。
    f.worker.submit(operation(900), command).unwrap();
    quota.release.add_permits(1);
    let report = task.await.unwrap().unwrap();
    assert_eq!(report.permitted, 0);
    assert_eq!(report.cycle_id, 1);
    for id in &f.ids {
        let w = f.worker.work(*id).unwrap();
        assert_eq!(w.source.config_revision, 2);
        if action == 0 {
            assert!(w.source.manual_paused);
            assert_eq!(w.target.status_directive, StatusDirective::Disabled);
        }
        if action == 1 {
            assert_eq!(w.models, vec!["new-model"]);
        }
        if action == 2 {
            assert!(w.source_retire_requested);
        }
    }
    assert_eq!(quota.received.load(Ordering::SeqCst), 1);
    quota.release.add_permits(1);
    let next = f.worker.refresh_glm_policy().await.unwrap();
    assert_eq!(next.permitted, if action == 1 { groups } else { 0 });
    assert_eq!(
        quota.received.load(Ordering::SeqCst),
        if action == 1 { 2 } else { 1 }
    );
}

#[tokio::test]
async fn acquisition_pending_pause_models_and_retirement_use_current_directory() {
    for action in 0..3 {
        pending_change(2, action).await;
    }
}

#[tokio::test]
async fn acquisition_member_added_while_waiting_uses_current_binding_and_authority() {
    for managed in [false, true] {
        let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1))]).await;
        let quota = QuotaGate::start().await;
        let sources = install_quota(&f, &quota.url);
        let worker = f.worker.clone();
        let task = tokio::spawn(async move { worker.refresh_glm_policy().await });
        until(|| quota.received.load(Ordering::SeqCst) == 1).await;
        let group = f
            .worker
            .create_group(GroupDraft {
                name: "late-group".into(),
                newapi_group: "late-group".into(),
                kind: SourceKind::Glm,
            })
            .unwrap();
        let member = f
            .worker
            .plan_membership(sources[0], group.group_id, 10)
            .unwrap();
        db(&f)
            .execute(
                "UPDATE source_groups SET ready=1 WHERE group_id=?1",
                [group.group_id.get()],
            )
            .unwrap();
        db(&f)
            .execute(
                "UPDATE memberships SET channel_id=999,lifecycle='imported' WHERE membership_id=?1",
                [member.membership_id.get()],
            )
            .unwrap();
        db(&f)
            .execute(
                "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
                [member.membership_id.get()],
            )
            .unwrap();
        if managed {
            f.worker
                .submit(
                    operation(901),
                    ControlCommand::EnsureMember {
                        membership_id: member.membership_id,
                        expected_revision: 1,
                        status: StatusDirective::Preserve,
                    },
                )
                .unwrap();
        }
        quota.release.add_permits(1);
        let report = task.await.unwrap().unwrap();
        assert_eq!(report.permitted, if managed { 2 } else { 1 });
        let w = f.worker.work(member.membership_id).unwrap();
        assert_eq!(w.target.desired_priority, if managed { 100 } else { 10 });
        assert_eq!(w.managed, managed);
        assert_eq!(
            f.worker
                .core
                .lock()
                .unwrap()
                .permits
                .contains_key(&member.membership_id),
            managed
        );
        assert_eq!(quota.received.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn acquisition_to_reconcile_rotates_groups_without_reenabling_native_disabled_glm() {
    let f = fixture(
        SourceKind::Glm,
        &[
            (0, 0, 100, Some(1)),
            (1, 0, 10, Some(1)),
            (0, 1, 100, Some(1)),
            (1, 1, 10, Some(1)),
        ],
    )
    .await;
    finish(&f).await;
    f.state.check_routing.store(true, Ordering::SeqCst);
    let reversed = Arc::new(AtomicBool::new(false));
    let phase = reversed.clone();
    let quota = Fake::start(move |r| {
        let first = request_source(r) == 1;
        quota_reply(Some(if first ^ phase.load(Ordering::SeqCst) {
            96
        } else {
            20
        }))
    })
    .await;
    install_quota(&f, &quota.url);
    assert_eq!(f.worker.refresh_glm_policy().await.unwrap().permitted, 2);
    *f.state.routing_targets.lock().unwrap() =
        [(1, 0), (2, 100), (3, 0), (4, 100)].into_iter().collect();
    finish(&f).await;
    for (i, id) in f.ids.iter().enumerate() {
        let w = f.worker.work(*id).unwrap();
        let channels = f.state.channels.lock().unwrap();
        let native = &channels[&w.member.channel_id.unwrap().get()];
        assert_eq!(native["priority"], json!(if i % 2 == 0 { 0 } else { 100 }));
        assert_eq!(native["status"], json!(1));
        assert!(!w.target.dirty);
    }
    // 原生自动禁用仍保留：下一轮重新选中也只能调 priority，不能重开 GLM。
    for i in [0, 2] {
        let channel = f
            .worker
            .work(f.ids[i])
            .unwrap()
            .member
            .channel_id
            .unwrap()
            .get();
        f.state.channels.lock().unwrap().get_mut(&channel).unwrap()["status"] = json!(3);
    }
    reversed.store(true, Ordering::SeqCst);
    assert_eq!(f.worker.refresh_glm_policy().await.unwrap().permitted, 2);
    *f.state.routing_targets.lock().unwrap() =
        [(1, 100), (2, 0), (3, 100), (4, 0)].into_iter().collect();
    finish(&f).await;
    for (i, id) in f.ids.iter().enumerate() {
        let w = f.worker.work(*id).unwrap();
        let channels = f.state.channels.lock().unwrap();
        let native = &channels[&w.member.channel_id.unwrap().get()];
        assert_eq!(native["priority"], json!(if i % 2 == 0 { 100 } else { 0 }));
        assert_eq!(native["status"], json!(if i % 2 == 0 { 3 } else { 1 }));
        assert_eq!(w.target.status_directive, StatusDirective::Preserve);
    }
    assert_eq!(f.state.enables.load(Ordering::SeqCst), 0);
    assert!(f.state.routing_violations.lock().unwrap().is_empty());
    assert_eq!(quota.calls.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn acquisition_each_source_lease_uses_its_own_response_time() {
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1)), (1, 0, 10, Some(1))]).await;
    let quota = Fake::start(|r| {
        let mut reply = quota_reply(Some(1));
        if request_source(r) == 2 {
            reply.delay = Duration::from_millis(1200);
        }
        reply
    })
    .await;
    install_quota(&f, &quota.url);
    let report = f.worker.refresh_glm_policy().await.unwrap();
    assert_eq!(report.permitted, 2);
    let core = f.worker.core.lock().unwrap();
    assert!(core.permits[&f.ids[0]].expires_at() < core.permits[&f.ids[1]].expires_at());
    assert_eq!(quota.calls.lock().unwrap().len(), 2);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn acquisition_pbt_shared_network_matches_simple_glm_model(values in prop::collection::vec(prop::option::of(0u8..=100),1..5),groups in 1usize..4) {
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let n=values.len();let entries:Vec<_>=(0..groups).flat_map(|g|(0..n).map(move|s|(s,g,10,Some(1)))).collect();
            let f=fixture(SourceKind::Glm,&entries).await;let input=values.clone();
            let quota=Fake::start(move|r|quota_reply(input[request_source(r)-1])).await;install_quota(&f,&quota.url);
            let normal:Vec<_>=(0..n).filter(|i|values[*i].is_some_and(|p|p<95)).collect();
            let degraded=normal.is_empty()&&values.iter().any(Option::is_some);
            let eligible:Vec<_>=if degraded {(0..n).filter(|i|values[*i].is_some_and(|p|p<100)).collect()}else{normal};
            let active=eligible.iter().copied().min_by_key(|i|values[*i].unwrap());
            let report=f.worker.refresh_glm_policy().await.unwrap();
            assert_eq!(report.permitted,eligible.len()*groups);assert_eq!(quota.calls.lock().unwrap().len(),n);
            for g in 0..groups {
                for (i,value) in values.iter().enumerate() {
                    let w=f.worker.work(f.ids[g*n+i]).unwrap();
                    assert_eq!(w.target.desired_priority,if active==Some(i){100}else if eligible.contains(&i){10}else if value.is_some(){0}else{10});
                    let state=f.worker.core.lock().unwrap().store.load_group_policy(w.group.group_id).unwrap();
                    let GroupPolicyState::Glm(state)=state.state else{panic!("wrong provider")};
                    assert_eq!(state.active,active.map(|i|f.ids[g*n+i]));assert_eq!(state.regime,if degraded {Regime::Degraded}else{Regime::Normal});
                }
            }
            assert!(f.fake.calls.lock().unwrap().is_empty());
        });
    }
    #[test]
    fn acquisition_pbt_pending_controls_never_authorize_old_source(groups in 1usize..4,action in 0u8..3) {
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(pending_change(groups,action));
    }
}
