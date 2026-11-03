use super::*;
use crate::newapi::transport::{
    tests::{fixture_cooldown, queued_requests, Fake, Recorded, Reply},
    ManagementBudget,
};
use crate::reconcile::worker::token_service::TokenOwners;
use proptest::prelude::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicI64, AtomicUsize};

struct Native {
    quota: AtomicI64,
    adds: AtomicUsize,
    applied: AtomicUsize,
    reads: AtomicUsize,
    refund: AtomicI64,
    hold_add: AtomicBool,
    hold_readback: AtomicBool,
    cool_legacy: AtomicBool,
    write_ready: Notify,
    read_ready: Notify,
    budget: Arc<ManagementBudget>,
}
impl Native {
    async fn reply(&self, record: Recorded) -> Reply {
        let user = record
            .raw
            .lines()
            .filter_map(|s| s.split_once(':'))
            .find(|(k, _)| k.eq_ignore_ascii_case("new-api-user"))
            .map(|(_, v)| v.trim().parse::<i64>().unwrap())
            .unwrap_or(1);
        match record.path.as_str() {
            "/api/user/self" => {
                self.reads.fetch_add(1, Ordering::SeqCst);
                if self.hold_readback.load(Ordering::SeqCst) && self.adds.load(Ordering::SeqCst) > 0
                {
                    self.read_ready.notified().await;
                }
                if user == 1 && self.cool_legacy.load(Ordering::SeqCst) {
                    fixture_cooldown(&self.budget, Duration::from_secs(30));
                }
                Reply::json(
                    json!({"success":true,"data":{"id":user,"role":if user==41 {1}else{100},"status":1,"group":"default","quota":self.quota.load(Ordering::SeqCst)}}),
                )
            }
            "/api/user/manage" => {
                assert_eq!(record.body["action"], "add_quota");
                assert_eq!(record.body["mode"], "add");
                let amount = record.body["value"].as_i64().unwrap();
                assert!(amount > 0);
                self.adds.fetch_add(1, Ordering::SeqCst);
                if self.hold_add.load(Ordering::SeqCst) {
                    self.write_ready.notified().await;
                }
                self.quota.fetch_add(
                    amount + self.refund.swap(0, Ordering::SeqCst),
                    Ordering::SeqCst,
                );
                self.applied.fetch_add(1, Ordering::SeqCst);
                Reply::json(json!({"success":true}))
            }
            _ => panic!("unknown isolated balance path"),
        }
    }
}
struct Fixture {
    _dir: tempfile::TempDir,
    _fake: Fake,
    state: Arc<Native>,
    worker: Arc<ReconcileWorker>,
}
async fn fixture() -> Fixture {
    let budget = ManagementBudget::new();
    let state = Arc::new(Native {
        quota: AtomicI64::new(100),
        adds: AtomicUsize::new(0),
        applied: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        refund: AtomicI64::new(0),
        hold_add: AtomicBool::new(false),
        hold_readback: AtomicBool::new(false),
        cool_legacy: AtomicBool::new(false),
        write_ready: Notify::new(),
        read_ready: Notify::new(),
        budget: budget.clone(),
    });
    let s = state.clone();
    let fake = Fake::start_async(move |r| {
        let s = s.clone();
        async move { s.reply(r).await }
    })
    .await;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let cfg = fake.config(true);
    let admin = Arc::new(NewApiClient::with_budget(&cfg, budget.clone()).unwrap());
    let issuer = Arc::new(NewApiClient::with_budget(&cfg, budget).unwrap());
    let policy: Config =
        toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\n").unwrap();
    let worker = ReconcileWorker::new_with_token_owners(
        store,
        admin,
        Arc::new(policy),
        TokenOwners::new(Some((issuer, 41)), Some(1)).unwrap(),
    )
    .unwrap();
    worker
        .publish_backend(BackendReadiness::from_preflight(
            SUPPORTED_BACKEND_VERSION,
            NativeRecoveryCheck::Disabled,
            true,
            now() - 1,
            now() + 3600,
        ))
        .unwrap();
    worker
        .set_balance_policy(
            1,
            BalancePolicyDraft {
                auto_refill: true,
                warn_below: 200,
                refill_to: 1000,
            },
        )
        .unwrap();
    Fixture {
        _dir: dir,
        _fake: fake,
        state,
        worker,
    }
}
async fn until(predicate: impl Fn() -> bool) {
    for _ in 0..100_000 {
        if predicate() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("balance fixture condition not reached");
}
async fn finish<T>(task: tokio::task::JoinHandle<T>) -> T {
    for _ in 0..10_000 {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        if task.is_finished() {
            return task.await.unwrap();
        }
        tokio::time::advance(Duration::from_millis(10)).await;
    }
    task.abort();
    panic!("bounded balance scenario did not finish");
}
fn manual(
    worker: Arc<ReconcileWorker>,
    scope: TokenOwnerScope,
) -> tokio::task::JoinHandle<ControlResult<Option<RefillReport>>> {
    tokio::spawn(async move {
        worker
            .refill(
                scope,
                RefillAmount::Exact(10),
                RefillSource::Manual,
                Some(Instant::now() + Duration::from_secs(60)),
            )
            .await
    })
}
fn account(worker: &ReconcileWorker, scope: TokenOwnerScope) -> BalanceAccountView {
    worker
        .balance_view(now_ms())
        .unwrap()
        .accounts
        .into_iter()
        .find(|a| a.owner_scope == scope)
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn cancelled_refill_settles_before_unlocking_waiting_manual() {
    let f = fixture().await;
    let server = Arc::new(crate::panel_http::AdminTestServer::start(f.worker.clone()).await);
    f.state.hold_add.store(true, Ordering::SeqCst);
    let w = f.worker.clone();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let auto = tokio::spawn(async move {
        crate::runtime::balance_test_owned_job(shutdown, async move {
            w.refill(
                TokenOwnerScope::Issuer,
                RefillAmount::ToTarget,
                RefillSource::Auto,
                None,
            )
            .await
        })
        .await
    });
    until(|| f.state.adds.load(Ordering::SeqCst) == 1).await;
    assert_eq!(f.state.applied.load(Ordering::SeqCst), 0);
    let caller = server.clone();
    let waiting = tokio::spawn(async move {
        caller
            .call(
                "POST",
                "/balance/refill",
                Some(&json!({"schema_version":1,"owner_scope":"issuer","amount":10})),
            )
            .await
    });
    until(|| f.worker.balance_test_waiters(TokenOwnerScope::Issuer) == 1).await;
    let before = now_ms();
    stop.send(true).unwrap();
    assert!(
        finish(auto).await.unwrap().is_none(),
        "real runtime stop must drop and settle the owned refill before joining"
    );
    let (status, response) = finish(waiting).await;
    assert_eq!(status, 503);
    assert_eq!(response["error"], "not_ready");
    let view = account(&f.worker, TokenOwnerScope::Issuer);
    let record = view.last_refill.unwrap();
    assert_eq!(record.source, RefillSource::Auto);
    assert_eq!(record.outcome, BalanceRefillOutcome::Uncertain);
    assert_eq!(record.amount, Some(900));
    assert_eq!(record.error_code, Some(ControlErrorCode::TransportUnknown));
    assert!(record.at_ms >= before);
    assert_eq!(view.locked_until_ms, Some(record.at_ms + 600000));
    assert_eq!(view.backoff_until_ms, view.locked_until_ms);
    assert_eq!(f.state.adds.load(Ordering::SeqCst), 1);
    f.state.write_ready.notify_one();
    until(|| f.state.applied.load(Ordering::SeqCst) == 1).await;
    assert_eq!(
        f.state.quota.load(Ordering::SeqCst),
        1000,
        "native write happens only after local cancellation sealed the owner"
    );
    assert_eq!(
        finish(manual(f.worker.clone(), TokenOwnerScope::Issuer))
            .await
            .unwrap_err()
            .code,
        ControlErrorCode::NotReady
    );
    assert_eq!(f.state.adds.load(Ordering::SeqCst), 1);
    Arc::try_unwrap(server).ok().unwrap().finish().await;
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_readback_keeps_confirmed_refill() {
    let f = fixture().await;
    f.state.hold_readback.store(true, Ordering::SeqCst);
    let task = manual(f.worker.clone(), TokenOwnerScope::Issuer);
    until(|| f.state.adds.load(Ordering::SeqCst) == 1).await;
    tokio::time::advance(Duration::from_secs(1)).await;
    until(|| f.state.reads.load(Ordering::SeqCst) == 2).await;
    let before = account(&f.worker, TokenOwnerScope::Issuer).last_refill;
    assert_eq!(
        before.as_ref().unwrap().outcome,
        BalanceRefillOutcome::Refilled
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let after = account(&f.worker, TokenOwnerScope::Issuer);
    assert_eq!(after.last_refill, before);
    assert!(after.locked_until_ms.is_none());
    assert!(after.backoff_until_ms.is_none());
    assert!(f.worker.balance.issuer.try_lock().is_ok());
    assert_eq!(f.state.applied.load(Ordering::SeqCst), 1);
    f.state.read_ready.notify_one();
}

#[tokio::test(start_paused = true)]
async fn private_balance_failure_does_not_overwrite_concurrent_publication() {
    let f = fixture().await;
    f.state.hold_add.store(true, Ordering::SeqCst);
    let first = manual(f.worker.clone(), TokenOwnerScope::Issuer);
    until(|| f.state.adds.load(Ordering::SeqCst) == 1).await;
    f.state.cool_legacy.store(true, Ordering::SeqCst);
    let other = manual(f.worker.clone(), TokenOwnerScope::LegacyAdmin);
    tokio::time::advance(Duration::from_secs(1)).await;
    until(|| {
        f.worker.balance.slot().unwrap().accounts[1]
            .reading
            .is_some()
            && queued_requests(&f.state.budget) > 0
    })
    .await;
    let original = f.worker.publication.load(Ordering::SeqCst);
    let ready = Arc::new(AtomicBool::new(false));
    let released = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let w = f.worker.clone();
    let r = ready.clone();
    let release = released.clone();
    let publish = std::thread::spawn(move || {
        w.publish(|_| {
            r.store(true, Ordering::SeqCst);
            let (mut open, cv) = (release.0.lock().unwrap(), &release.1);
            while !*open {
                open = cv.wait(open).unwrap();
            }
            Ok(())
        })
    });
    until(|| ready.load(Ordering::SeqCst)).await;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _slot = f.worker.balance.slot.lock().unwrap();
        panic!("synthetic balance slot poison");
    }));
    first.abort();
    let cancelled = first.await;
    let during = f.worker.publication.load(Ordering::SeqCst);
    let failed = f.worker.balance.failed.load(Ordering::SeqCst);
    // 必须先解开测试线程，即使回退变异失败也不能把测试进程卡在join。
    *released.0.lock().unwrap() = true;
    released.1.notify_one();
    let published = publish.join();
    assert!(cancelled.unwrap_err().is_cancelled());
    assert!(failed);
    assert_eq!(during, original + 1);
    published.unwrap().unwrap();
    assert_eq!(f.worker.publication.load(Ordering::SeqCst), original + 2);
    assert_eq!(f.worker.core.lock().unwrap().input_epoch, original + 2);
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(
        finish(other).await.unwrap_err().code,
        ControlErrorCode::Storage
    );
    assert_eq!(
        f.state.adds.load(Ordering::SeqCst),
        1,
        "already queued other-owner add must also reject private failed"
    );
    assert!(
        f.worker.snapshot().is_ok(),
        "ordinary catalog remains usable"
    );
    assert_eq!(
        f.worker.balance_view(now_ms()).unwrap_err().code,
        ControlErrorCode::Storage
    );
    f.state.write_ready.notify_one();
    until(|| f.state.applied.load(Ordering::SeqCst) == 1).await;
}

#[tokio::test(start_paused = true)]
async fn cancellation_before_admission_is_conservative_and_before_attempt_is_not() {
    let f = fixture().await;
    f.state.cool_legacy.store(true, Ordering::SeqCst);
    let task = manual(f.worker.clone(), TokenOwnerScope::LegacyAdmin);
    until(|| {
        f.worker.balance.slot().unwrap().accounts[1]
            .reading
            .is_some()
            && queued_requests(&f.state.budget) > 0
    })
    .await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        account(&f.worker, TokenOwnerScope::LegacyAdmin)
            .last_refill
            .unwrap()
            .outcome,
        BalanceRefillOutcome::Uncertain
    );
    assert_eq!(f.state.adds.load(Ordering::SeqCst), 0);
    let before = account(&f.worker, TokenOwnerScope::Issuer).last_refill;
    let early = manual(f.worker.clone(), TokenOwnerScope::Issuer);
    until(|| queued_requests(&f.state.budget) > 0).await;
    early.abort();
    assert!(early.await.unwrap_err().is_cancelled());
    assert_eq!(
        account(&f.worker, TokenOwnerScope::Issuer).last_refill,
        before
    );
    assert!(f.worker.balance.issuer.try_lock().is_ok());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn balance_projection_pbt_age_failure_and_signed_values(quota in -9_007_199_254_740_991i64..=9_007_199_254_740_991, age in -10i64..=180_010, failed in any::<bool>(), missing in any::<bool>()) {
        let policy=BalancePolicy{revision:1,auto_refill:true,warn_below:100,refill_to:1000};let at=200_000;
        let reading=(!missing).then_some((quota,at-age));
        let a=Account{owner:TokenOwner{scope:TokenOwnerScope::Issuer,user_id:i64::MAX},reading,failure:failed.then_some((ControlErrorCode::AuthFailed,at)),locked_until:at,backoff_until:at+1,last_refill:None};
        let v=a.view(policy,at);
        let expected=if missing||failed||age<0||age>180000 {BalanceState::Unknown} else if quota<=0 {BalanceState::Exhausted} else if quota<100 {BalanceState::Low}else{BalanceState::Sufficient};
        prop_assert_eq!(v.state,expected);prop_assert_eq!(v.quota,(!missing).then_some(quota));prop_assert_eq!(v.observed_at_ms,reading.map(|r|r.1));
        prop_assert_eq!(v.user_id,i64::MAX.to_string());prop_assert_eq!(v.locked_until_ms,None);prop_assert_eq!(v.backoff_until_ms,Some(at+1));
    }
    #[test]
    fn cancelled_attempt_pbt_seals_one_owner_before_release(manual in any::<bool>(), amount in 1i64..=BALANCE_CAP, legacy in any::<bool>(), terminal in 0u8..5, next_auto in any::<bool>()) {
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
        rt.block_on(async {
            let f=fixture().await; let balance=&f.worker.balance;
            let source=if manual {RefillSource::Manual}else{RefillSource::Auto};
            let scope=if legacy{TokenOwnerScope::LegacyAdmin}else{TokenOwnerScope::Issuer};let index=usize::from(legacy);
            let lock=balance.owner_lock(scope).lock().await;
            let mut attempt=RefillAttemptGuard{balance:&balance,scope,source,amount,settled:false,_owner:lock};
            assert!(balance.owner_lock(scope).try_lock().is_err());
            let expected=match terminal {1=>Outcome::Refilled,2=>Outcome::NotSent(ControlErrorCode::QueueFull),3=>Outcome::Rejected(ControlErrorCode::RemoteRejected),_=>Outcome::Uncertain};
            if terminal!=0 {attempt.settle(expected).unwrap();}
            // 0=潜在投递后取消，1..4=已结算后取消/退出；不让Drop改写确定结论。
            drop(attempt);let released=balance.owner_lock(scope).try_lock().unwrap();let slot=balance.slot().unwrap();
            let a=&slot.accounts[index];let record=a.last_refill.unwrap();assert_eq!(record.source,source);assert_eq!(record.amount,Some(amount));assert_eq!(record.outcome,expected);
            let unknown=terminal==0||terminal==4;let rejected=terminal==3;
            assert_eq!(a.locked_until,if unknown{record.at_ms+600000}else{0});
            assert_eq!(a.backoff_until,if unknown||rejected{record.at_ms+600000}else{0});
            assert!(slot.accounts[1-index].last_refill.is_none());assert_eq!(slot.accounts[1-index].locked_until,0);assert_eq!(slot.accounts[1-index].backoff_until,0);
            drop(slot);drop(released);
            let w=f.worker.clone();
            let followup=finish(tokio::spawn(async move {
                w.refill(scope,if next_auto{RefillAmount::ToTarget}else{RefillAmount::Exact(10)},if next_auto{RefillSource::Auto}else{RefillSource::Manual},(!next_auto).then(||Instant::now()+Duration::from_secs(60))).await
            })).await;
            let blocked=unknown||(rejected&&next_auto);
            if blocked {
                assert_eq!(followup.unwrap_err().code,ControlErrorCode::NotReady);
                assert_eq!(balance.slot().unwrap().accounts[index].last_refill,Some(record));
                assert_eq!(f.state.reads.load(Ordering::SeqCst),0);
            }else {assert!(followup.unwrap().is_some());}
            assert_eq!(f.state.adds.load(Ordering::SeqCst),usize::from(!blocked));
            // 对另一账号的准入独立，不能把本账号封锁扩成全局锁。
            let other=if legacy{TokenOwnerScope::Issuer}else{TokenOwnerScope::LegacyAdmin};
            assert!(finish(super::tests::manual(f.worker.clone(),other)).await.unwrap().is_some());
            assert_eq!(f.state.adds.load(Ordering::SeqCst),usize::from(!blocked)+1);
        });
    }
    #[test]
    fn refill_bounds_pbt_actual_http_matches_wide_integer_reference(
        before in -9_007_199_254_740_991i64..=9_007_199_254_740_991,
        exact in -1i64..=4_000_000_000_000_001,
        target in 2i64..=4_000_000_000_000_000,
        refund in 0i64..1000,
        auto in any::<bool>(),
    ) {
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
        rt.block_on(async {
            let f=fixture().await;
            f.state.quota.store(before,Ordering::SeqCst);f.state.refund.store(refund,Ordering::SeqCst);
            f.worker.set_balance_policy(2,BalancePolicyDraft{auto_refill:true,warn_below:target-1,refill_to:target}).unwrap();
            // 独立i128模型；不调用生产金额计算或校验函数作oracle。
            let skip=auto && i128::from(before)>=i128::from(target)-1;
            let amount=if auto {(i128::from(target)-i128::from(before)).min(4_000_000_000_000_000)} else {i128::from(exact)};
            let admitted=!skip && amount>=1 && amount<=4_000_000_000_000_000 && i128::from(before)+amount<=4_000_000_000_000_000;
            let w=f.worker.clone();
            let result=finish(tokio::spawn(async move {
                w.refill(TokenOwnerScope::Issuer,if auto {RefillAmount::ToTarget}else{RefillAmount::Exact(exact)},if auto{RefillSource::Auto}else{RefillSource::Manual},(!auto).then(||Instant::now()+Duration::from_secs(60))).await
            })).await;
            if skip {assert!(result.unwrap().is_none());}
            else if admitted {
                let report=result.unwrap().unwrap();assert_eq!(i128::from(report.amount),amount);assert_eq!(report.before,before);
                assert_eq!(report.after,Some((i128::from(before)+amount+i128::from(refund)) as i64));
                assert_eq!(f.state.quota.load(Ordering::SeqCst),report.after.unwrap());
                assert_eq!(account(&f.worker,TokenOwnerScope::Issuer).last_refill.unwrap().outcome,BalanceRefillOutcome::Refilled);
            } else {assert_eq!(result.unwrap_err().code,ControlErrorCode::InvalidInput);}
            assert_eq!(f.state.adds.load(Ordering::SeqCst),usize::from(admitted));
            assert_eq!(f.state.applied.load(Ordering::SeqCst),usize::from(admitted));
            if !admitted {assert_eq!(f.state.quota.load(Ordering::SeqCst),before);}
        });
    }
}
