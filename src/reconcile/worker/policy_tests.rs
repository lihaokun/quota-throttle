use super::*;
use crate::quota::{QuotaStatus, WindowStatus};
use crate::source_policy::probe::{ProbeCycle, QuotaObservation, QuotaReading};
use rusqlite::{types::Value as SqlValue, Connection};
use std::collections::BTreeMap;

fn db(f: &Fixture) -> Connection {
    Connection::open(f._dir.path().join("catalog.sqlite3")).unwrap()
}
fn image(f: &Fixture) -> BTreeMap<String, Vec<Vec<SqlValue>>> {
    let conn = db(f);
    [
        "sources",
        "source_secrets",
        "source_groups",
        "memberships",
        "source_control",
        "member_control",
        "member_execution",
        "execution_attempts",
        "control_operations",
        "operation_targets",
        "group_policy_state",
        "volcano_runtime",
    ]
    .into_iter()
    .map(|table| {
        let mut q = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
            .unwrap();
        let width = q.column_count();
        let rows = q
            .query_map([], |r| {
                (0..width)
                    .map(|i| r.get(i))
                    .collect::<rusqlite::Result<Vec<SqlValue>>>()
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        (table.to_string(), rows)
    })
    .collect()
}
fn cycle(f: &Fixture, seq: u64, at: i64, pct: Option<f64>) -> ProbeCycle {
    let mut sources = BTreeMap::new();
    for id in &f.ids {
        let w = f.worker.work(*id).unwrap();
        if w.source.kind == SourceKind::Glm {
            sources.insert(w.source.source_id, w.source.config_revision);
        }
    }
    ProbeCycle::test_input(
        seq,
        at,
        sources
            .into_iter()
            .map(|(source_id, config_revision)| QuotaObservation {
                source_id,
                config_revision,
                cycle_id: seq,
                observed_at_ms: at,
                reading: pct
                    .map(|percentage| {
                        QuotaReading::Available(QuotaStatus {
                            five_hour: Some(WindowStatus {
                                percentage,
                                next_reset_time: at + 3_600_000,
                            }),
                            weekly: None,
                            level: None,
                        })
                    })
                    .unwrap_or(QuotaReading::Unavailable),
            })
            .collect(),
    )
}
fn leases(f: &Fixture, at: i64) -> BTreeMap<i64, (i64, bool)> {
    let mut core = f.worker.core.lock().unwrap();
    let permits = core.permits.clone();
    permits
        .into_iter()
        .map(|(id, p)| {
            let w = core.store.load_work(id, false).unwrap();
            (id.get(), (p.expires_at(), p.valid_for(&w, at)))
        })
        .collect()
}
fn reject<T>(result: ControlResult<T>, code: ControlErrorCode) {
    assert_eq!(result.err().expect("expected rejection").code, code);
}

#[tokio::test]
async fn policy_batch_replaces_all_scopes_noop_keeps_observations_and_replay_cannot_renew() {
    let f = fixture(
        SourceKind::Glm,
        &[(0, 0, 100, Some(1)), (0, 1, 100, Some(1))],
    )
    .await;
    // 使用真实读路径填缓存；后续策略 no-op 不应清掉它。
    f.worker.reconcile_once().await.unwrap();
    let at = now() * 1000;
    let b = cycle(&f, 1, at, Some(1.0));
    let before = image(&f);
    let epoch = f.worker.core.lock().unwrap().input_epoch;
    let observed = f.worker.core.lock().unwrap().observations.len();
    assert!(observed > 0);
    let report = f.worker.publish_glm_cycle(&b, at).unwrap();
    assert_eq!(report.permitted, 2);
    assert_eq!(report.changed_members, 0);
    assert_eq!(f.worker.core.lock().unwrap().observations.len(), observed);
    assert!(f.worker.core.lock().unwrap().input_epoch > epoch);
    let after = image(&f);
    assert_eq!(before["memberships"], after["memberships"]);
    let old = leases(&f, at / 1000);
    let epoch = f.worker.core.lock().unwrap().input_epoch;
    reject(
        f.worker.publish_glm_cycle(&b, at + 1000),
        ControlErrorCode::RevisionConflict,
    );
    assert_eq!(leases(&f, at / 1000), old);
    assert_eq!(image(&f), after);
    assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
    // 新轮次全部未知必须撤掉整批旧许可，不能按新增项 extend。
    let unknown = cycle(&f, 2, at + 1000, None);
    let result = f.worker.publish_glm_cycle(&unknown, at + 1000).unwrap();
    assert_eq!(result.permitted, 0);
    assert_eq!(result.waiting, 2);
    assert!(f.worker.core.lock().unwrap().permits.is_empty());
    let snapshot = f.worker.snapshot().unwrap();
    assert_eq!(snapshot.memberships.len(), 2);
}

#[tokio::test]
async fn policy_sql_failure_keeps_entire_old_cache_and_unknown_commit_closes_gate() {
    for uncertain in [false, true] {
        let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1)), (0, 1, 10, Some(1))]).await;
        let at = now() * 1000;
        let b = cycle(&f, 1, at, Some(1.0));
        if uncertain {
            db(&f).execute_batch("CREATE TABLE commit_fault (source_id INTEGER REFERENCES sources(source_id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_commit AFTER UPDATE ON group_policy_state BEGIN INSERT INTO commit_fault VALUES(999999); END;").unwrap();
        } else {
            let second = f.worker.work(f.ids[1]).unwrap().group.group_id.get();
            db(&f).execute_batch(&format!("CREATE TRIGGER fail_second BEFORE UPDATE ON group_policy_state WHEN NEW.group_id={second} BEGIN SELECT RAISE(ABORT,'fake failure'); END;")).unwrap();
        }
        let before = image(&f);
        let old = leases(&f, at / 1000);
        let epoch = f.worker.core.lock().unwrap().input_epoch;
        reject(
            f.worker.publish_glm_cycle(&b, at),
            ControlErrorCode::Storage,
        );
        assert_eq!(image(&f), before);
        let c = f.worker.core.lock().unwrap();
        assert_eq!(c.last_glm_cycle, None);
        assert_eq!(c.input_epoch, epoch);
        assert_eq!(c.permits.len(), 2);
        assert_eq!(c.store.usable().is_err(), uncertain);
        drop(c);
        assert_eq!(
            f.worker.publication.load(Ordering::SeqCst) % 2,
            if uncertain { 1 } else { 0 }
        );
        if !uncertain {
            assert_eq!(leases(&f, at / 1000), old);
        }
        assert!(f.fake.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn policy_commit_fences_old_sends_and_readers_never_see_partial_map() {
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1)), (0, 1, 10, Some(1))]).await;
    let at = now() * 1000;
    let b = cycle(&f, 1, at, Some(1.0));
    let old = f.worker.publication.load(Ordering::SeqCst);
    let blocker = db(&f);
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let worker = f.worker.clone();
    let task = tokio::task::spawn_blocking(move || worker.publish_glm_cycle(&b, at));
    until(|| {
        !f.worker
            .publication
            .load(Ordering::SeqCst)
            .is_multiple_of(2)
    })
    .await;
    assert!(!f
        .worker
        .gate(old, at / 1000, at / 1000 + 30, &AtomicBool::new(false)));
    assert!(f.worker.core.try_lock().is_err());
    blocker.execute_batch("ROLLBACK").unwrap();
    let report = task.await.unwrap().unwrap();
    assert_eq!(report.changed_members, 2);
    let c = f.worker.core.lock().unwrap();
    assert_eq!(c.permits.len(), 2);
    assert_eq!(c.last_glm_cycle, Some(1));
    assert!(f
        .worker
        .publication
        .load(Ordering::SeqCst)
        .is_multiple_of(2));
}

#[tokio::test]
async fn policy_recovery_write_failure_rolls_back_earlier_targets_and_cache() {
    let f = fixture(
        SourceKind::Glm,
        &[(0, 0, 10, Some(1)), (0, 1, 100, Some(1))],
    )
    .await;
    let at = now() * 1000;
    let b = cycle(&f, 1, at, Some(1.0));
    let recovery = f.ids[1];
    f.worker.core.lock().unwrap().permits.remove(&recovery);
    db(&f).execute("UPDATE member_control SET last_error_code='not_ready',next_attempt_at=?1 WHERE membership_id=?2",rusqlite::params![at/1000+500,recovery.get()]).unwrap();
    db(&f).execute_batch(&format!("CREATE TRIGGER fail_wake BEFORE UPDATE OF next_attempt_at ON member_control WHEN NEW.membership_id={} AND NEW.next_attempt_at<OLD.next_attempt_at BEGIN SELECT RAISE(ABORT,'wake failed'); END;",recovery.get())).unwrap();
    let before = image(&f);
    let permits = leases(&f, at / 1000);
    let epoch = f.worker.core.lock().unwrap().input_epoch;
    reject(
        f.worker.publish_glm_cycle(&b, at),
        ControlErrorCode::Storage,
    );
    assert_eq!(image(&f), before);
    assert_eq!(leases(&f, at / 1000), permits);
    assert_eq!(f.worker.core.lock().unwrap().last_glm_cycle, None);
    assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
    db(&f).execute_batch("DROP TRIGGER fail_wake").unwrap();
    let report = f.worker.publish_glm_cycle(&b, at).unwrap();
    assert_eq!(report.changed_members, 1);
    assert_eq!(report.permitted, 2);
    assert_eq!(
        f.worker.work(recovery).unwrap().target.next_attempt_at,
        at / 1000
    );
}

#[tokio::test]
async fn policy_dry_run_preview_and_every_remote_write_path_are_blocked() {
    for scenario in 0..5 {
        let mut cfg = toml::from_str::<Config>(
            "poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'",
        )
        .unwrap();
        cfg.dry_run = true;
        let f = fixture_with_options(
            SourceKind::Glm,
            &[(
                0,
                0,
                100,
                if scenario == 0 {
                    None
                } else {
                    Some(if scenario == 1 { 2 } else { 1 })
                },
            )],
            true,
            Arc::new(cfg),
        )
        .await;
        let id = f.ids[0];
        if scenario == 2 {
            let w = f.worker.work(id).unwrap();
            f.worker
                .submit(
                    operation(400),
                    ControlCommand::SetSourcePaused {
                        source_id: w.source.source_id,
                        expected_revision: w.source.config_revision,
                        paused: true,
                    },
                )
                .unwrap();
        }
        if scenario == 3 {
            f.state.channels.lock().unwrap().get_mut(&1).unwrap()["priority"] = json!(10);
        }
        if scenario == 4 {
            f.state.channels.lock().unwrap().get_mut(&1).unwrap()["models"] = json!("old-model");
        }
        let at = now() * 1000;
        let b = cycle(&f, 1, at, Some(1.0));
        let before = image(&f);
        let old = leases(&f, at / 1000);
        let epoch = f.worker.publication.load(Ordering::SeqCst);
        let preview = f.worker.publish_glm_cycle(&b, at).unwrap();
        assert!(preview.preview_only);
        assert_eq!(preview.permitted, 0);
        assert_eq!(image(&f), before);
        assert_eq!(leases(&f, at / 1000), old);
        assert_eq!(f.worker.core.lock().unwrap().last_glm_cycle, None);
        assert_eq!(f.worker.publication.load(Ordering::SeqCst), epoch);
        for _ in 0..3 {
            f.worker.reconcile_once().await.unwrap();
        }
        let after = image(&f);
        assert!(after["execution_attempts"].is_empty());
        assert_eq!(after["member_execution"], before["member_execution"]);
        assert!(f
            .fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.method == "GET"));
    }
}

#[tokio::test]
async fn policy_restart_empty_config_and_queue_limits() {
    let f = fixture(SourceKind::Glm, &[(0, 0, 10, Some(1))]).await;
    let at = now() * 1000;
    let b = cycle(&f, 1, at, Some(1.0));
    let slots: Vec<_> = (0..64)
        .map(|_| f.worker.commands.try_acquire().unwrap())
        .collect();
    reject(
        f.worker.publish_glm_cycle(&b, at),
        ControlErrorCode::QueueFull,
    );
    drop(slots);
    f.worker.publish_glm_cycle(&b, at).unwrap();
    let Fixture {
        _dir,
        fake,
        state,
        worker,
        ids,
    } = f;
    let client = worker.client.clone();
    drop(worker);
    let store = CatalogStore::open(&_dir.path().join("catalog.sqlite3")).unwrap();
    let worker = ReconcileWorker::new(store, client, policy_config()).unwrap();
    assert!(worker.core.lock().unwrap().permits.is_empty());
    assert_eq!(worker.core.lock().unwrap().last_glm_cycle, None);
    let f = Fixture {
        _dir,
        fake,
        state,
        worker,
        ids,
    };
    let report = f.worker.publish_glm_cycle(&b, at).unwrap();
    assert_eq!(report.permitted, 0); // 预检未恢复。
    let empty = fixture(SourceKind::Glm, &[]).await;
    let at = now() * 1000;
    assert_eq!(
        empty
            .worker
            .publish_glm_cycle(&ProbeCycle::test_input(1, at, vec![]), at)
            .unwrap()
            .waiting,
        0
    );
    let path = empty._dir.path().join("other").join("catalog.sqlite3");
    let store = CatalogStore::open(&path).unwrap();
    let mut bad: Config = toml::from_str(
        "poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'",
    )
    .unwrap();
    bad.priority_active = bad.priority_standby;
    reject(
        ReconcileWorker::new(store, empty.worker.client.clone(), Arc::new(bad)),
        ControlErrorCode::InvalidInput,
    );
}

#[tokio::test]
async fn policy_dry_run_dispatch_guard_blocks_preexisting_ticket() {
    let mut cfg: Config = toml::from_str(
        "poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'",
    )
    .unwrap();
    cfg.dry_run = true;
    let f =
        fixture_with_options(SourceKind::Glm, &[(0, 0, 10, Some(1))], true, Arc::new(cfg)).await;
    let id = f.ids[0];
    let w = f.worker.work(id).unwrap();
    f.worker
        .submit(
            operation(401),
            ControlCommand::SetSourcePaused {
                source_id: w.source.source_id,
                expected_revision: w.source.config_revision,
                paused: true,
            },
        )
        .unwrap();
    let stop = AtomicBool::new(false);
    f.worker
        .observe(&f.worker.assemble(id).unwrap(), &stop)
        .await
        .unwrap();
    let plan = f.worker.assemble(id).unwrap();
    assert_eq!(plan.step(), PlanStep::Disable);
    // 单独模拟已准备的票据，绕过第一层拦截，证明发送层自己也拒绝。
    let ticket = f
        .worker
        .core
        .lock()
        .unwrap()
        .store
        .prepare_execution(
            id,
            plan.work.revision(),
            plan.work.policy_disabled,
            plan.observation.as_ref(),
            &plan.context(),
        )
        .unwrap();
    let error = f
        .worker
        .dispatch(&ticket, &plan, plan.epoch, &stop, None)
        .await
        .expect_err("dry-run send must fail");
    assert_eq!(error.delivery, DeliveryEvidence::NotSent);
    assert!(f
        .fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.method == "GET"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn policy_pbt_failed_rounds_never_publish_and_replays_do_not_renew(groups in 1usize..4,failed in 0usize..3,fail in any::<bool>(),seq in 2u64..1000) {
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let entries:Vec<_>=(0..groups).map(|g|(0,g,10,Some(1))).collect();
            let f=fixture(SourceKind::Glm,&entries).await;let at=now()*1000;
            let first=cycle(&f,1,at,Some(1.0));f.worker.publish_glm_cycle(&first,at).unwrap();
            let before=image(&f);let old=leases(&f,at/1000);let epoch=f.worker.core.lock().unwrap().input_epoch;
            let second=cycle(&f,seq,at,Some(100.0));
            if fail {
                let target=f.worker.work(f.ids[failed%groups]).unwrap().group.group_id.get();
                db(&f).execute_batch(&format!("CREATE TRIGGER fail_round BEFORE UPDATE ON group_policy_state WHEN NEW.group_id={target} BEGIN SELECT RAISE(ABORT,'fake'); END;")).unwrap();
                reject(f.worker.publish_glm_cycle(&second,at),ControlErrorCode::Storage);
                assert_eq!(image(&f),before);assert_eq!(leases(&f,at/1000),old);
                assert_eq!(f.worker.core.lock().unwrap().last_glm_cycle,Some(1));assert_eq!(f.worker.core.lock().unwrap().input_epoch,epoch);
            } else {
                let report=f.worker.publish_glm_cycle(&second,at).unwrap();assert_eq!(report.permitted,0);
                assert!(f.worker.core.lock().unwrap().permits.is_empty());assert_eq!(f.worker.core.lock().unwrap().last_glm_cycle,Some(seq));
                let after=image(&f);reject(f.worker.publish_glm_cycle(&first,at+1000),ControlErrorCode::RevisionConflict);
                assert_eq!(image(&f),after);assert!(f.worker.core.lock().unwrap().permits.is_empty());
            }
        });
    }
}
