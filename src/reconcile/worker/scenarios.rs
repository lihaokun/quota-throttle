//! 组合轨迹只使用真实 worker/Store/HTTP；期望由输入命令推进，不调用计划器作 oracle。
use super::*;

async fn drive(future: impl std::future::Future<Output = ()> + Send + 'static) {
    let task = tokio::spawn(future);
    // 始终 runnable，防止 OS socket 尚未返回时 paused runtime 跳到超时。
    for _ in 0..200_000 {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        if task.is_finished() {
            task.await.unwrap();
            return;
        }
        tokio::time::advance(Duration::from_millis(10)).await;
    }
    task.abort();
    panic!("scenario exceeded bounded virtual time");
}

fn backend(worker: &ReconcileWorker) {
    worker
        .publish_backend(BackendReadiness::from_preflight(
            SUPPORTED_BACKEND_VERSION,
            NativeRecoveryCheck::Disabled,
            true,
            now() - 1,
            now() + 3600,
        ))
        .unwrap();
}
fn routes(f: &Fixture) {
    for id in &f.ids {
        let w = f.worker.work(*id).unwrap();
        if w.member.lifecycle != MembershipLifecycle::Retired && w.source.retired_at.is_none() {
            publish(&f.worker, *id, false);
        }
    }
}
fn identities(f: &Fixture) -> Vec<(MembershipId, SourceId, GroupId, Option<ChannelId>)> {
    f.ids
        .iter()
        .map(|id| {
            let w = f.worker.work(*id).unwrap();
            (
                *id,
                w.source.source_id,
                w.group.group_id,
                w.member.channel_id,
            )
        })
        .collect()
}
fn reopen(f: Fixture) -> Fixture {
    let before = identities(&f);
    let Fixture {
        _dir,
        fake,
        state,
        worker,
        ids,
    } = f;
    assert_eq!(
        Arc::strong_count(&worker),
        1,
        "must settle owned rounds before reopening"
    );
    let client = worker.client.clone();
    let config = worker.policy_config.clone();
    drop(worker);
    let store = CatalogStore::open(&_dir.path().join("catalog.sqlite3")).unwrap();
    let worker = ReconcileWorker::new(store, client, config).unwrap();
    backend(&worker);
    let f = Fixture {
        _dir,
        fake,
        state,
        worker,
        ids,
    };
    assert_eq!(identities(&f), before);
    routes(&f);
    f
}
async fn rounds(f: &Fixture, count: usize) {
    for _ in 0..count {
        f.worker.reconcile_once().await.unwrap();
    }
}
fn replay(f: &Fixture, id: &OperationId, command: &ControlCommand, view: &OperationView) {
    let before = f.fake.calls.lock().unwrap().len();
    assert_eq!(&f.worker.submit(id.clone(), command.clone()).unwrap(), view);
    assert_eq!(f.fake.calls.lock().unwrap().len(), before);
}

fn receipts(f: &Fixture, id: &OperationId) -> Vec<(i64, i64, String)> {
    let conn = rusqlite::Connection::open_with_flags(
        f._dir.path().join("catalog.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let rows=conn.prepare("SELECT membership_id,confirmed_at,receipt_json FROM operation_targets WHERE operation_id=?1 ORDER BY membership_id").unwrap()
        .query_map([id.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().map(|r|r.unwrap()).collect();
    rows
}

fn only_writes_to(f: &Fixture, start: usize, allowed: &[i64]) {
    for r in &f.fake.calls.lock().unwrap()[start..] {
        if r.method == "GET" {
            continue;
        }
        let id = if r.method == "PUT" && r.path == "/api/channel/" {
            r.body["id"].as_i64().unwrap()
        } else if r.method == "POST" && r.path.ends_with("/status") {
            r.path.split('/').nth(3).unwrap().parse().unwrap()
        } else {
            panic!("unexpected write in imported-member scenario");
        };
        assert!(
            allowed.contains(&id),
            "write escaped the command target set"
        );
    }
}

fn add_preserved_glm(f: &mut Fixture) {
    let source = f
        .worker
        .create_source(SourceDraft {
            name: "glm-independent".into(),
            note: String::new(),
            credential: SecretString::new("fake-independent-glm-key".into()).unwrap(),
            settings: SourceSettings::Glm {
                upstream: UpstreamProfile {
                    channel_type: 8,
                    base_url: "https://glm.invalid/v1".into(),
                    models: vec!["model-0".into()],
                    model_discovery: None,
                },
                quota: GlmQuotaProfile {
                    quota_url: "https://glm.invalid/quota".into(),
                    global_headers: vec![],
                    key_headers: vec![],
                },
            },
        })
        .unwrap();
    let group = f
        .worker
        .create_group(GroupDraft {
            name: "glm".into(),
            newapi_group: "glm".into(),
            kind: SourceKind::Glm,
        })
        .unwrap();
    let member = f
        .worker
        .plan_membership(source.source_id, group.group_id, 20)
        .unwrap();
    // 仅启动前构造 imported 夹具，不在运行中改意图/延期/未决日志。
    let conn = rusqlite::Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap();
    conn.execute(
        "UPDATE source_groups SET ready=1 WHERE group_id=?1",
        [group.group_id.get()],
    )
    .unwrap();
    conn.execute(
        "UPDATE memberships SET channel_id=5,lifecycle='imported' WHERE membership_id=?1",
        [member.membership_id.get()],
    )
    .unwrap();
    conn.execute(
        "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
        [member.membership_id.get()],
    )
    .unwrap();
    f.state.channels.lock().unwrap().insert(5,json!({"id":5,"name":"imported-glm","type":8,"group":"glm","base_url":"https://glm.invalid/v1","models":"model-0","priority":20,"status":3,"auto_ban":1,"channel_info":{"is_multi_key":false}}));
    f.worker
        .submit(
            operation(4),
            ControlCommand::EnsureMember {
                membership_id: member.membership_id,
                expected_revision: 1,
                status: StatusDirective::Preserve,
            },
        )
        .unwrap();
    f.ids.push(member.membership_id);
}

struct Expected {
    status: [i64; 5],
    priority: [i64; 5],
    models: [String; 5],
}
impl Expected {
    fn publish_targets(&self, f: &Fixture) {
        *f.state.routing_targets.lock().unwrap() = self
            .priority
            .iter()
            .enumerate()
            .map(|(i, p)| (i as i64 + 1, *p))
            .collect();
    }
    fn check(&self, f: &Fixture) {
        let actual = f.state.channels.lock().unwrap();
        assert_eq!(actual.len(), 5);
        for (index, id) in f.ids.iter().enumerate() {
            let w = f.worker.work(*id).unwrap();
            assert_eq!(w.member.channel_id.unwrap().get(), index as i64 + 1);
            let c = &actual[&(index as i64 + 1)];
            assert_eq!(c["status"], self.status[index]);
            assert_eq!(c["priority"], self.priority[index]);
            assert_eq!(c["models"], self.models[index]);
            assert_eq!(
                c["group"],
                if index == 4 {
                    "glm".into()
                } else {
                    format!("group-{}", index / 2)
                }
            );
            assert_eq!(
                c["base_url"],
                if index == 4 {
                    "https://glm.invalid/v1".into()
                } else {
                    format!("https://source-{}.invalid/v1", index % 2)
                }
            );
            assert_eq!(c["auto_ban"], if index == 4 { 1 } else { 0 });
            assert_eq!(w.source.manual_paused, self.status[index] == 2);
            assert_eq!(
                w.source.kind,
                if index == 4 {
                    SourceKind::Glm
                } else {
                    SourceKind::Volcano
                }
            );
            assert_eq!(w.member.observed_status, Some(self.status[index]));
            assert_eq!(w.member.observed_priority, Some(self.priority[index]));
            assert_eq!(w.member.applied_revision, Some(w.target.revision));
        }
        assert!(f.state.routing_violations.lock().unwrap().is_empty());
    }
}

async fn mixed_trace(events: Vec<(u8, bool, bool)>) {
    let mut f = fixture(
        SourceKind::Volcano,
        &[
            (0, 0, 100, Some(1)),
            (1, 0, 10, Some(1)),
            (0, 1, 10, Some(1)),
            (1, 1, 100, Some(1)),
        ],
    )
    .await;
    add_preserved_glm(&mut f);
    f.state.check_routing.store(true, Ordering::SeqCst);
    let mut expected = Expected {
        status: [1, 1, 1, 1, 3],
        priority: [100, 10, 10, 100, 20],
        models: [
            "model-0".into(),
            "model-1".into(),
            "model-0".into(),
            "model-1".into(),
            "model-0".into(),
        ],
    };
    expected.publish_targets(&f);
    finish(&f).await;
    expected.check(&f);
    let mapping = identities(&f);
    let mut history = Vec::new();
    // 每条轨迹强制遇到部分失败、跨组模型和首选交换，随机尾部继续组合。
    let all = vec![
        (1, true, true),
        (1, false, false),
        (2, false, false),
        (0, false, true),
    ]
    .into_iter()
    .chain(events);
    for (step, (event, restart, reject)) in all.enumerate() {
        let id = operation(step + 20);
        let allowed;
        let mut failed_lowering = None;
        let command = match event {
            0 | 3 => {
                let group = usize::from(event == 3);
                allowed = vec![group as i64 * 2 + 1, group as i64 * 2 + 2];
                let a = group * 2;
                let b = a + 1;
                if reject {
                    let (old, new) = if expected.priority[a] > expected.priority[b] {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    failed_lowering = Some((
                        old,
                        new,
                        expected.status[old] == 1 && expected.status[new] == 1,
                    ));
                    *f.state.write_fault.lock().unwrap() =
                        Some((old as i64 + 1, WriteFault::PartialLower(50)));
                }
                expected.priority.swap(a, b);
                let g = f.worker.work(f.ids[a]).unwrap().group;
                ControlCommand::SetGroupPriorities {
                    group_id: g.group_id,
                    expected_revision: g.revision,
                    priorities: vec![
                        (f.ids[a], expected.priority[a]),
                        (f.ids[b], expected.priority[b]),
                    ],
                }
            }
            1 => {
                allowed = vec![1, 3];
                let source = f.worker.work(f.ids[0]).unwrap().source;
                let paused = expected.status[0] != 2;
                for i in [0, 2] {
                    expected.status[i] = if paused { 2 } else { 1 };
                }
                if reject {
                    *f.state.write_fault.lock().unwrap() =
                        Some((if restart { 3 } else { 1 }, WriteFault::Reject));
                }
                ControlCommand::SetSourcePaused {
                    source_id: source.source_id,
                    expected_revision: source.config_revision,
                    paused,
                }
            }
            _ => {
                allowed = vec![2, 4];
                let source = f.worker.work(f.ids[1]).unwrap().source;
                for i in [1, 3] {
                    expected.models[i] = format!("source-b-model-{step}");
                }
                ControlCommand::SyncSourceModels {
                    source_id: source.source_id,
                    expected_revision: source.config_revision,
                    models: vec![expected.models[1].clone()],
                }
            }
        };
        let calls_before = f.fake.calls.lock().unwrap().len();
        let accepted = f.worker.submit(id.clone(), command.clone()).unwrap();
        expected.publish_targets(&f);
        replay(&f, &id, &command, &accepted);
        routes(&f);
        if event == 1 && reject || failed_lowering.is_some() {
            rounds(&f, 10).await;
            assert!(f.state.write_fault.lock().unwrap().is_none());
            let partial = f.worker.operation(&id).unwrap();
            assert_ne!(partial.phase, OperationPhase::Succeeded);
            if event == 1 {
                assert_eq!(partial.confirmed_targets, 1);
            }
            if let Some((old, new, both_routing)) = failed_lowering {
                if both_routing {
                    let channels = f.state.channels.lock().unwrap();
                    assert_eq!(channels[&(old as i64 + 1)]["priority"], 50);
                    assert_eq!(channels[&(new as i64 + 1)]["priority"], 10);
                    assert_eq!(partial.confirmed_targets, 0);
                }
            }
            assert!(f.worker.work(f.ids[0]).unwrap().source.retired_at.is_none());
            if restart {
                f = reopen(f);
                assert_eq!(f.worker.operation(&id).unwrap(), partial);
            }
            f.worker.retry(&id).await.unwrap();
        } else if restart {
            rounds(&f, 1).await;
            f = reopen(f);
        }
        finish(&f).await;
        let complete = f.worker.operation(&id).unwrap();
        assert_eq!(complete.phase, OperationPhase::Succeeded);
        expected.check(&f);
        assert_eq!(identities(&f), mapping);
        only_writes_to(&f, calls_before, &allowed);
        let receipt = receipts(&f, &id);
        assert_eq!(receipt.len(), complete.total_targets);
        history.push((id, command, complete, receipt));
        for (id, command, view, receipt) in &history {
            replay(&f, id, command, view);
            assert_eq!(&receipts(&f, id), receipt);
        }
    }
    let source = f.worker.work(f.ids[1]).unwrap().source;
    let id = operation(200);
    let calls_before = f.fake.calls.lock().unwrap().len();
    f.worker
        .submit(
            id.clone(),
            ControlCommand::RetireSource {
                source_id: source.source_id,
                expected_revision: source.config_revision,
            },
        )
        .unwrap();
    *f.state.write_fault.lock().unwrap() = Some((4, WriteFault::Reject));
    rounds(&f, 10).await;
    assert_eq!(f.worker.operation(&id).unwrap().confirmed_targets, 1);
    assert!(f.worker.work(f.ids[1]).unwrap().source.retired_at.is_none());
    f = reopen(f);
    f.worker.retry(&id).await.unwrap();
    finish(&f).await;
    for i in [1, 3] {
        expected.status[i] = 2;
    }
    expected.check(&f);
    assert_eq!(identities(&f), mapping);
    assert!(f.worker.work(f.ids[1]).unwrap().source.retired_at.is_some());
    assert_eq!(
        f.worker.operation(&id).unwrap().phase,
        OperationPhase::Succeeded
    );
    only_writes_to(&f, calls_before, &[2, 4]);
    for (id, command, view, receipt) in &history {
        replay(&f, id, command, view);
        assert_eq!(&receipts(&f, id), receipt);
    }
}

async fn unknown_creation(retire: bool, reverse: bool, damage: u8) {
    let entries = if reverse {
        vec![(0, 1, 23, None), (0, 0, 10, None)]
    } else {
        vec![(0, 0, 10, None), (0, 1, 23, None)]
    };
    let mut f = fixture(SourceKind::Volcano, &entries).await;
    f.state.bad_create.store(true, Ordering::SeqCst);
    rounds(&f, 1).await;
    assert_eq!(
        f.worker.work(f.ids[0]).unwrap().pending_write,
        PendingWrite::Unknown
    );
    let source = f.worker.work(f.ids[0]).unwrap().source;
    let id = operation(20);
    let command = if retire {
        ControlCommand::RetireSource {
            source_id: source.source_id,
            expected_revision: source.config_revision,
        }
    } else {
        ControlCommand::SetSourcePaused {
            source_id: source.source_id,
            expected_revision: source.config_revision,
            paused: true,
        }
    };
    f.worker.submit(id.clone(), command).unwrap();
    f = reopen(f);
    let original = f.state.channels.lock().unwrap().clone();
    {
        let mut channels = f.state.channels.lock().unwrap();
        match damage {
            0 => {
                channels.remove(&1);
            }
            1 => {
                let mut duplicate = channels[&1].clone();
                duplicate["id"] = json!(99);
                channels.insert(99, duplicate);
            }
            _ => {
                channels.get_mut(&1).unwrap()["base_url"] = json!("https://wrong.invalid/v1");
            }
        }
    }
    rounds(&f, 7).await;
    assert!(f.worker.work(f.ids[0]).unwrap().member.channel_id.is_none());
    assert!(f.worker.work(f.ids[0]).unwrap().source.retired_at.is_none());
    assert_ne!(
        f.worker.operation(&id).unwrap().phase,
        OperationPhase::Succeeded
    );
    *f.state.channels.lock().unwrap() = original;
    f.worker.retry(&id).await.unwrap();
    finish(&f).await;
    assert_eq!(
        f.worker.operation(&id).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert_eq!(
        f.worker.work(f.ids[0]).unwrap().source.retired_at.is_some(),
        retire
    );
    let calls = f.fake.calls.lock().unwrap();
    let creates: Vec<_> = calls
        .iter()
        .filter(|r| r.method == "POST" && r.path == "/api/channel/")
        .collect();
    assert_eq!(creates.len(), 2);
    assert_ne!(
        creates[0].body["channel"]["name"],
        creates[1].body["channel"]["name"]
    );
    assert_eq!(f.state.enables.load(Ordering::SeqCst), 0);
    for index in 0..2 {
        assert_eq!(
            f.worker
                .work(f.ids[index])
                .unwrap()
                .member
                .channel_id
                .unwrap()
                .get(),
            index as i64 + 1
        );
        assert_eq!(f.state.status(index as i64 + 1), 2);
    }
}

async fn late_reply(kind: SourceKind, changes: Vec<bool>, retire: bool) {
    let mut f = fixture(kind, &[(0, 0, 10, Some(2)), (0, 1, 10, Some(2))]).await;
    rounds(&f, 1).await;
    f.state.slow_enable.store(true, Ordering::SeqCst);
    let worker = f.worker.clone();
    let task = tokio::spawn(async move { worker.reconcile_once().await });
    until(|| f.state.enables.load(Ordering::SeqCst) > 0).await;
    for (n, paused) in changes.into_iter().chain([true]).enumerate() {
        let source = f.worker.work(f.ids[0]).unwrap().source;
        f.worker
            .submit(
                operation(n + 20),
                ControlCommand::SetSourcePaused {
                    source_id: source.source_id,
                    expected_revision: source.config_revision,
                    paused,
                },
            )
            .unwrap();
    }
    let source = f.worker.work(f.ids[0]).unwrap().source;
    let last = operation(100);
    f.worker
        .submit(
            last.clone(),
            if retire {
                ControlCommand::RetireSource {
                    source_id: source.source_id,
                    expected_revision: source.config_revision,
                }
            } else {
                ControlCommand::SetSourcePaused {
                    source_id: source.source_id,
                    expected_revision: source.config_revision,
                    paused: true,
                }
            },
        )
        .unwrap();
    task.await.unwrap().unwrap();
    let w = f.worker.work(f.ids[0]).unwrap();
    assert_ne!(w.member.applied_revision, Some(w.target.revision));
    assert_ne!(
        f.worker.operation(&last).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert_eq!(
        f.worker.operation(&operation(0)).unwrap().phase,
        OperationPhase::Superseded
    );
    f.state.slow_enable.store(false, Ordering::SeqCst);
    finish(&f).await;
    f = reopen(f);
    assert_eq!(
        f.worker.operation(&last).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert_eq!(f.state.enables.load(Ordering::SeqCst), 1);
    assert_eq!((f.state.status(1), f.state.status(2)), (2, 2));
    assert_eq!(
        f.worker.work(f.ids[0]).unwrap().source.retired_at.is_some(),
        retire
    );
}

async fn unknown_general_write(groups: usize, failed: usize, retries: usize, fields: bool) {
    let entries: Vec<_> = (0..groups)
        .map(|g| (0, g, 10 + g as i64 * 7, Some(1)))
        .collect();
    let mut f = fixture(SourceKind::Volcano, &entries).await;
    finish(&f).await;
    let mapping = identities(&f);
    let failed = failed % groups;
    *f.state.write_fault.lock().unwrap() = Some((failed as i64 + 1, WriteFault::Uncertain));
    let source = f.worker.work(f.ids[0]).unwrap().source;
    f.worker
        .submit(
            operation(20),
            if fields {
                ControlCommand::SyncSourceModels {
                    source_id: source.source_id,
                    expected_revision: source.config_revision,
                    models: vec!["unknown-new-model".into()],
                }
            } else {
                ControlCommand::SetSourcePaused {
                    source_id: source.source_id,
                    expected_revision: source.config_revision,
                    paused: true,
                }
            },
        )
        .unwrap();
    // 模型更新推进 source revision；字段协调仍须当前策略许可，不能复用旧许可。
    routes(&f);
    rounds(&f, 10).await;
    assert!(
        f.state.write_fault.lock().unwrap().is_none(),
        "unknown-write fault must actually be dispatched"
    );
    assert_eq!(
        f.worker.operation(&operation(20)).unwrap().phase,
        OperationPhase::Unknown
    );
    let source = f.worker.work(f.ids[0]).unwrap().source;
    let retire = operation(21);
    f.worker
        .submit(
            retire.clone(),
            ControlCommand::RetireSource {
                source_id: source.source_id,
                expected_revision: source.config_revision,
            },
        )
        .unwrap();
    for _ in 0..retries {
        f = reopen(f);
        f.worker.retry(&retire).await.unwrap();
        rounds(&f, 8).await;
        let view = f.worker.operation(&retire).unwrap();
        assert_eq!(view.phase, OperationPhase::Unknown);
        assert_eq!(view.confirmed_targets, groups - 1);
        let w = f.worker.work(f.ids[failed]).unwrap();
        assert_eq!(w.pending_write, PendingWrite::Unknown);
        assert_ne!(w.member.applied_revision, Some(w.target.revision));
        assert!(w.source.retired_at.is_none());
        assert_eq!(identities(&f), mapping);
        for id in 1..=groups {
            assert_eq!(f.state.status(id as i64), 2);
            assert_eq!(
                f.state.channels.lock().unwrap()[&(id as i64)]["models"],
                if fields {
                    "unknown-new-model"
                } else {
                    "model-0"
                }
            );
        }
        assert_eq!(f.state.enables.load(Ordering::SeqCst), 0);
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn scenario_pbt_shared_groups_partial_retry_reopen_and_glm_isolation(events in prop::collection::vec((0u8..5,any::<bool>(),any::<bool>()),0..7)) {
        runtime().block_on(drive(mixed_trace(events)));
    }
    #[test]
    fn scenario_pbt_unknown_create_reopen_lookup_and_latest_stop(retire in any::<bool>(),reverse in any::<bool>(),damage in 0u8..3) {
        runtime().block_on(drive(unknown_creation(retire,reverse,damage)));
    }
    #[test]
    fn scenario_pbt_late_enable_ack_never_completes_new_stop(glm in any::<bool>(),changes in prop::collection::vec(any::<bool>(),0..6),retire in any::<bool>()) {
        runtime().block_on(drive(late_reply(if glm {SourceKind::Glm}else{SourceKind::Volcano},changes,retire)));
    }
    #[test]
    fn scenario_pbt_general_unknown_survives_matching_get_reopen_and_retry(groups in 2usize..5,failed in 0usize..4,retries in 1usize..4,fields in any::<bool>()) {
        runtime().block_on(drive(unknown_general_write(groups,failed,retries,fields)));
    }
}
