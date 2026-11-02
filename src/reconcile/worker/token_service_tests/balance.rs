//! 余额集成契约：真实worker/Store/传输，期望由合成账号余额与输入独立给出。
use super::*;
use crate::catalog::{BalancePolicy, BalancePolicyDraft, BALANCE_CAP};
use crate::reconcile::worker::balance::{
    BalanceAccountView, BalanceRefillOutcome, BalanceState, RefillAmount, RefillReport,
    RefillSource,
};
use tokio::time::Instant;

fn at_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn account(worker: &ReconcileWorker, scope: TokenOwnerScope) -> BalanceAccountView {
    worker
        .balance_view(at_ms())
        .unwrap()
        .accounts
        .into_iter()
        .find(|entry| entry.owner_scope == scope)
        .unwrap()
}

fn quota(f: &TokenFixture, id: i64) -> i64 {
    f.state
        .balance_quotas
        .lock()
        .unwrap()
        .get(&id)
        .copied()
        .unwrap_or_else(|| f.state.quota.load(Ordering::SeqCst))
}

fn policy(worker: &ReconcileWorker, enabled: bool, warn: i64, target: i64) -> BalancePolicy {
    let revision = worker.balance_view(at_ms()).unwrap().policy.revision;
    worker
        .set_balance_policy(
            revision,
            BalancePolicyDraft {
                auto_refill: enabled,
                warn_below: warn,
                refill_to: target,
            },
        )
        .unwrap()
}

async fn manual(
    worker: &ReconcileWorker,
    scope: TokenOwnerScope,
    amount: i64,
) -> ControlResult<Option<RefillReport>> {
    worker
        .refill(
            scope,
            RefillAmount::Exact(amount),
            RefillSource::Manual,
            Some(Instant::now() + Duration::from_secs(60)),
        )
        .await
}

fn recorded(
    account: &BalanceAccountView,
    source: RefillSource,
    outcome: BalanceRefillOutcome,
    amount: Option<i64>,
    code: Option<ControlErrorCode>,
) {
    let attempt = account.last_refill.as_ref().unwrap();
    assert_eq!(attempt.source, source);
    assert_eq!(attempt.outcome, outcome);
    assert_eq!(attempt.amount, amount);
    assert_eq!(attempt.error_code, code);
}

#[tokio::test(start_paused = true)]
async fn refill_steps_and_outcomes() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        let before = quota(&f, 41);
        let report = manual(&f.worker, TokenOwnerScope::Issuer, 123)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(report.owner_scope, TokenOwnerScope::Issuer);
        assert_eq!(report.user_id, 41);
        assert_eq!(
            (report.before, report.amount, report.after),
            (before, 123, Some(before + 123))
        );
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), 2);
        assert_eq!(quota(&f, 41), before + 123);
        let settled = account(&f.worker, TokenOwnerScope::Issuer);
        recorded(
            &settled,
            RefillSource::Manual,
            BalanceRefillOutcome::Refilled,
            Some(123),
            None,
        );
        assert!(settled.locked_until_ms.is_none() && settled.backoff_until_ms.is_none());

        // 入口没有身份或截止，均不允许覆盖已有尝试。
        assert_eq!(
            manual(&f.worker, TokenOwnerScope::LegacyAdmin, 1)
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(
            f.worker
                .refill(
                    TokenOwnerScope::Issuer,
                    RefillAmount::Exact(1),
                    RefillSource::Manual,
                    None
                )
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::InvalidInput
        );
        assert_eq!(
            account(&f.worker, TokenOwnerScope::Issuer).last_refill,
            settled.last_refill
        );
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);

        for amount in [0, -1, BALANCE_CAP, BALANCE_CAP + 1] {
            let reads = f.state.balance_reads.load(Ordering::SeqCst);
            assert_eq!(
                manual(&f.worker, TokenOwnerScope::Issuer, amount)
                    .await
                    .unwrap_err()
                    .code,
                ControlErrorCode::InvalidInput
            );
            assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), reads + 1);
            recorded(
                &account(&f.worker, TokenOwnerScope::Issuer),
                RefillSource::Manual,
                BalanceRefillOutcome::NotSent,
                None,
                Some(ControlErrorCode::InvalidInput),
            );
            assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
            assert_eq!(quota(&f, 41), before + 123);
        }

        let reads = f.state.balance_reads.load(Ordering::SeqCst);
        assert_eq!(
            f.worker
                .refill(
                    TokenOwnerScope::Issuer,
                    RefillAmount::Exact(1),
                    RefillSource::Manual,
                    Some(Instant::now())
                )
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        recorded(
            &account(&f.worker, TokenOwnerScope::Issuer),
            RefillSource::Manual,
            BalanceRefillOutcome::NotSent,
            None,
            Some(ControlErrorCode::NotReady),
        );
        assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), reads);

        f.worker
            .publish_backend(BackendReadiness::from_preflight(
                "",
                NativeRecoveryCheck::Unknown,
                false,
                0,
                0,
            ))
            .unwrap();
        assert_eq!(
            manual(&f.worker, TokenOwnerScope::Issuer, 1)
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), reads);
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);

        let mut dry = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        let mut cfg = (*policy_config()).clone();
        cfg.dry_run = true;
        Arc::get_mut(&mut dry.worker).unwrap().policy_config = Arc::new(cfg);
        assert_eq!(
            manual(&dry.worker, TokenOwnerScope::Issuer, 1)
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert!(account(&dry.worker, TokenOwnerScope::Issuer)
            .last_refill
            .is_none());
        assert_eq!(dry.state.balance_reads.load(Ordering::SeqCst), 0);
        assert_eq!(dry.state.balance_adds.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn refill_explicit_rejection_only_backs_off_automatic_attempts() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        f.state.balance_mode.store(1, Ordering::SeqCst);
        assert_eq!(
            manual(&f.worker, TokenOwnerScope::Issuer, 77)
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::RemoteRejected
        );
        let rejected = account(&f.worker, TokenOwnerScope::Issuer);
        recorded(
            &rejected,
            RefillSource::Manual,
            BalanceRefillOutcome::Rejected,
            Some(77),
            Some(ControlErrorCode::RemoteRejected),
        );
        assert!(rejected.locked_until_ms.is_none());
        assert_eq!(
            rejected.backoff_until_ms.unwrap() - rejected.last_refill.as_ref().unwrap().at_ms,
            600_000
        );
        assert_eq!(quota(&f, 41), 10_000);
        f.state.balance_mode.store(0, Ordering::SeqCst);
        assert_eq!(
            f.worker
                .refill(
                    TokenOwnerScope::Issuer,
                    RefillAmount::ToTarget,
                    RefillSource::Auto,
                    None
                )
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(
            account(&f.worker, TokenOwnerScope::Issuer).last_refill,
            rejected.last_refill
        );
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        assert_eq!(
            manual(&f.worker, TokenOwnerScope::Issuer, 77)
                .await
                .unwrap()
                .unwrap()
                .after,
            Some(10_077)
        );
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 2);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn refill_uncertain_delivery_locks_second_attempt_without_another_add() {
    drive_case(async {
        for mode in [2, 3, 4] {
            let f = tokens_fixture(SourceKind::Glm, &[], true, mode == 2).await;
            f.state.balance_mode.store(mode, Ordering::SeqCst);
            assert_eq!(
                manual(&f.worker, TokenOwnerScope::Issuer, 83)
                    .await
                    .unwrap_err()
                    .code,
                ControlErrorCode::TransportUnknown,
                "delivery mode {mode}"
            );
            let uncertain = account(&f.worker, TokenOwnerScope::Issuer);
            recorded(
                &uncertain,
                RefillSource::Manual,
                BalanceRefillOutcome::Uncertain,
                Some(83),
                Some(ControlErrorCode::TransportUnknown),
            );
            let recorded_at = uncertain.last_refill.as_ref().unwrap().at_ms;
            assert_eq!(uncertain.locked_until_ms.unwrap() - recorded_at, 600_000);
            assert_eq!(uncertain.backoff_until_ms.unwrap() - recorded_at, 600_000);
            assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
            // timeout分支的远端实际已写；500/429分支未写也必须保守封锁。
            assert_eq!(quota(&f, 41), if mode == 4 { 10_083 } else { 10_000 });
            f.state.balance_mode.store(0, Ordering::SeqCst);
            let reads = f.state.balance_reads.load(Ordering::SeqCst);
            assert_eq!(
                manual(&f.worker, TokenOwnerScope::Issuer, 83)
                    .await
                    .unwrap_err()
                    .code,
                ControlErrorCode::NotReady
            );
            assert_eq!(
                f.worker
                    .refill(
                        TokenOwnerScope::Issuer,
                        RefillAmount::ToTarget,
                        RefillSource::Auto,
                        None
                    )
                    .await
                    .unwrap_err()
                    .code,
                ControlErrorCode::NotReady
            );
            assert_eq!(
                account(&f.worker, TokenOwnerScope::Issuer).last_refill,
                uncertain.last_refill
            );
            assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), reads);
            assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
            if mode == 2 {
                assert_eq!(
                    manual(&f.worker, TokenOwnerScope::LegacyAdmin, 7)
                        .await
                        .unwrap()
                        .unwrap()
                        .after,
                    Some(10_007)
                );
                assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 2);
                assert_eq!(
                    account(&f.worker, TokenOwnerScope::Issuer).last_refill,
                    uncertain.last_refill
                );
                assert!(account(&f.worker, TokenOwnerScope::LegacyAdmin)
                    .locked_until_ms
                    .is_none());
            }
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn refill_success_with_failed_readback_keeps_refilled_and_unknown_balance() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        f.state
            .balance_read_fail_after_add
            .store(true, Ordering::SeqCst);
        let report = manual(&f.worker, TokenOwnerScope::Issuer, 91)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (report.before, report.amount, report.after),
            (10_000, 91, None)
        );
        assert_eq!(quota(&f, 41), 10_091);
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        let view = account(&f.worker, TokenOwnerScope::Issuer);
        recorded(
            &view,
            RefillSource::Manual,
            BalanceRefillOutcome::Refilled,
            Some(91),
            None,
        );
        assert_eq!(view.state, BalanceState::Unknown);
        assert_eq!(view.quota, Some(10_000));
        assert!(view.unknown_reason.is_some());
        assert!(view.locked_until_ms.is_none() && view.backoff_until_ms.is_none());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn auto_refill_amount_and_skips() {
    drive_case(async {
        for (before, enabled, warn, target, expected) in [
            (99, true, 100, 1000, Some(901)),
            (100, true, 100, 1000, None),
            (101, true, 100, 1000, None),
            (99, false, 100, 1000, None),
            (-100, true, 100, BALANCE_CAP, Some(BALANCE_CAP)),
        ] {
            let f = tokens_fixture(SourceKind::Glm, &[], true, false).await;
            f.state.quota.store(before, Ordering::SeqCst);
            policy(&f.worker, enabled, warn, target);
            let report = f
                .worker
                .refill(
                    TokenOwnerScope::Issuer,
                    RefillAmount::ToTarget,
                    RefillSource::Auto,
                    None,
                )
                .await
                .unwrap();
            assert_eq!(report.as_ref().map(|v| v.amount), expected);
            assert_eq!(
                f.state.balance_adds.load(Ordering::SeqCst),
                usize::from(expected.is_some())
            );
            assert_eq!(quota(&f, 41), before + expected.unwrap_or(0));
            let view = account(&f.worker, TokenOwnerScope::Issuer);
            if let Some(amount) = expected {
                let report = report.unwrap();
                assert_eq!(report.before, before);
                assert_eq!(report.after, Some(before + amount));
                assert!(amount <= BALANCE_CAP && report.before + amount <= BALANCE_CAP);
                recorded(
                    &view,
                    RefillSource::Auto,
                    BalanceRefillOutcome::Refilled,
                    Some(amount),
                    None,
                );
            } else {
                assert!(view.last_refill.is_none());
            }
        }
        let mut dry = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        let mut cfg = (*policy_config()).clone();
        cfg.dry_run = true;
        Arc::get_mut(&mut dry.worker).unwrap().policy_config = Arc::new(cfg);
        assert_eq!(
            dry.worker
                .refill(
                    TokenOwnerScope::Issuer,
                    RefillAmount::ToTarget,
                    RefillSource::Auto,
                    None
                )
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(dry.state.balance_reads.load(Ordering::SeqCst), 0);
        assert_eq!(dry.state.balance_adds.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn auto_refill_rechecks_current_policy_after_profile() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        policy(&f.worker, true, 20_000, 30_000);
        let worker = Arc::downgrade(&f.worker);
        let state = Arc::downgrade(&f.state);
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |request| {
            if request.path == "/api/user/self" {
                assert_eq!(
                    state.upgrade().unwrap().balance_adds.load(Ordering::SeqCst),
                    0
                );
                policy(&worker.upgrade().unwrap(), false, 20_000, 30_000);
            }
        }));
        assert!(f
            .worker
            .refill(
                TokenOwnerScope::Issuer,
                RefillAmount::ToTarget,
                RefillSource::Auto,
                None
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), 1);
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 0);
        assert_eq!(quota(&f, 41), 10_000);
        assert!(!f.worker.balance_view(at_ms()).unwrap().policy.auto_refill);
        assert!(account(&f.worker, TokenOwnerScope::Issuer)
            .last_refill
            .is_none());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn balance_owners_have_independent_locks_and_attempts() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, true).await;
        let held = f
            .worker
            .balance_test_owner_lock(TokenOwnerScope::Issuer)
            .await;
        assert_eq!(
            f.worker
                .refill(
                    TokenOwnerScope::Issuer,
                    RefillAmount::ToTarget,
                    RefillSource::Auto,
                    None
                )
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::QueueFull
        );
        assert!(account(&f.worker, TokenOwnerScope::Issuer)
            .last_refill
            .is_none());
        let legacy = manual(&f.worker, TokenOwnerScope::LegacyAdmin, 7)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((legacy.user_id, legacy.after), (1, Some(10_007)));
        let reads = f.state.balance_reads.load(Ordering::SeqCst);
        assert_eq!(
            f.worker
                .refill(
                    TokenOwnerScope::Issuer,
                    RefillAmount::Exact(9),
                    RefillSource::Manual,
                    Some(Instant::now() + Duration::from_millis(50))
                )
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::QueueFull
        );
        assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), reads);
        assert!(account(&f.worker, TokenOwnerScope::Issuer)
            .last_refill
            .is_none());
        assert_eq!(quota(&f, 41), 10_000);
        drop(held);
        let (issuer, legacy) = tokio::join!(
            manual(&f.worker, TokenOwnerScope::Issuer, 11),
            manual(&f.worker, TokenOwnerScope::LegacyAdmin, 13),
        );
        assert_eq!(issuer.unwrap().unwrap().after, Some(10_011));
        assert_eq!(legacy.unwrap().unwrap().after, Some(10_020));
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 3);
        assert_eq!(quota(&f, 41), 10_011);
        assert_eq!(quota(&f, 1), 10_020);
        let ids: Vec<_> = f
            .worker
            .balance_view(at_ms())
            .unwrap()
            .accounts
            .into_iter()
            .map(|a| a.user_id)
            .collect();
        assert_eq!(ids, ["41", "1"]);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn balance_round_unknown_never_zero() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        policy(&f.worker, false, 20_000, 30_000);
        f.worker.balance_round().await.unwrap();
        let known = account(&f.worker, TokenOwnerScope::Issuer);
        assert_eq!(known.quota, Some(10_000));
        assert_eq!(known.state, BalanceState::Low);
        policy(&f.worker, true, 20_000, 30_000);
        f.state.balance_profile_fail.store(true, Ordering::SeqCst);
        f.worker.balance_round().await.unwrap();
        let unknown = account(&f.worker, TokenOwnerScope::Issuer);
        assert_eq!(unknown.quota, known.quota);
        assert_eq!(unknown.observed_at_ms, known.observed_at_ms);
        assert_eq!(unknown.state, BalanceState::Unknown);
        assert!(unknown.unknown_reason.is_some());
        assert!(unknown.last_refill.is_none());
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 0);
        f.state.balance_profile_fail.store(false, Ordering::SeqCst);
        f.worker.balance_round().await.unwrap();
        let recovered = account(&f.worker, TokenOwnerScope::Issuer);
        assert_eq!(recovered.quota, Some(30_000));
        assert_eq!(recovered.state, BalanceState::Sufficient);
        assert!(recovered.unknown_reason.is_none());
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        recorded(
            &recovered,
            RefillSource::Auto,
            BalanceRefillOutcome::Refilled,
            Some(20_000),
            None,
        );
        let stale = f
            .worker
            .balance_view(recovered.observed_at_ms.unwrap() + 180_001)
            .unwrap();
        assert_eq!(stale.accounts[0].state, BalanceState::Unknown);
        assert_eq!(stale.accounts[0].quota, Some(30_000));

        let missing = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        missing
            .state
            .balance_profile_fail
            .store(true, Ordering::SeqCst);
        missing.worker.balance_round().await.unwrap();
        let unknown = account(&missing.worker, TokenOwnerScope::Issuer);
        assert_eq!(unknown.state, BalanceState::Unknown);
        assert!(unknown.quota.is_none() && unknown.observed_at_ms.is_none());
        assert_eq!(missing.state.balance_adds.load(Ordering::SeqCst), 0);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn balance_policy_write_does_not_bump_publication() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        let before = f.worker.publication.load(Ordering::SeqCst);
        let epoch = f.worker.core.lock().unwrap().input_epoch;
        assert!(f.worker.gate(before, now() - 1, now() + 60, &AtomicBool::new(false)));
        let saved = policy(&f.worker, false, 20_000, 30_000);
        let draft = BalancePolicyDraft { auto_refill: false, warn_below: 20_000, refill_to: 30_000 };
        assert_eq!(f.worker.set_balance_policy(saved.revision, draft).unwrap(), saved);
        assert_eq!(f.worker.set_balance_policy(saved.revision - 1, draft).unwrap_err().code,
                   ControlErrorCode::RevisionConflict);
        assert_eq!(f.worker.set_balance_policy(saved.revision, BalancePolicyDraft { warn_below: 0, ..draft })
                   .unwrap_err().code, ControlErrorCode::InvalidInput);
        let conn = rusqlite::Connection::open(f.dir.path().join("catalog.sqlite3")).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert_eq!(f.worker.set_balance_policy(saved.revision, BalancePolicyDraft { auto_refill: true, ..draft })
                   .unwrap_err().code, ControlErrorCode::QueueFull);
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(f.worker.publication.load(Ordering::SeqCst), before);
        assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
        assert_eq!(f.worker.core.lock().unwrap().store.balance_policy().unwrap(), saved);
        assert_eq!(f.worker.balance_view(at_ms()).unwrap().policy, saved);
        assert!(f.worker.gate(before, now() - 1, now() + 60, &AtomicBool::new(false)));
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 0);

        conn.execute_batch("CREATE TABLE balance_test_parent(id INTEGER PRIMARY KEY);
            CREATE TABLE balance_test_child(id INTEGER REFERENCES balance_test_parent(id) DEFERRABLE INITIALLY DEFERRED);
            CREATE TRIGGER balance_deferred_fault AFTER UPDATE ON balance_policy
            BEGIN INSERT INTO balance_test_child VALUES(1); END;").unwrap();
        assert_eq!(f.worker.set_balance_policy(saved.revision, BalancePolicyDraft { auto_refill: true, ..draft })
                   .unwrap_err().code, ControlErrorCode::Storage);
        assert!(f.worker.core.lock().unwrap().store.usable().is_err());
        assert_eq!(f.worker.publication.load(Ordering::SeqCst), u64::MAX);
        assert_eq!(f.worker.core.lock().unwrap().input_epoch, epoch);
        assert_eq!(f.worker.balance_view(at_ms()).unwrap().policy, saved);
        assert!(!f.worker.gate(before, now() - 1, now() + 60, &AtomicBool::new(false)));
        let stored: (i64, bool, i64, i64) = conn.query_row(
            "SELECT revision,auto_refill,warn_below,refill_to FROM balance_policy", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).unwrap();
        assert_eq!(stored, (saved.revision, saved.auto_refill, saved.warn_below, saved.refill_to));
        assert_eq!(manual(&f.worker, TokenOwnerScope::Issuer, 1).await.unwrap_err().code, ControlErrorCode::Storage);
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 0);
    }).await;
}

#[tokio::test(start_paused = true)]
async fn refund_does_not_change_pre_refill_bounds_contract() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        f.state.quota.store(BALANCE_CAP - 100, Ordering::SeqCst);
        let weak = Arc::downgrade(&f.state);
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |request| {
            if request.path == "/api/user/manage" {
                let state = weak.upgrade().unwrap();
                assert_eq!(state.balance_adds.load(Ordering::SeqCst), 1);
                assert_eq!(request.body["value"], 100);
                state
                    .balance_quotas
                    .lock()
                    .unwrap()
                    .insert(41, BALANCE_CAP - 50);
            }
        }));
        let report = manual(&f.worker, TokenOwnerScope::Issuer, 100)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(report.before, BALANCE_CAP - 100);
        assert_eq!(report.amount, 100);
        assert_eq!(report.before + report.amount, BALANCE_CAP);
        assert_eq!(report.after, Some(BALANCE_CAP + 50));
        assert_eq!(quota(&f, 41), BALANCE_CAP + 50);
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        recorded(
            &account(&f.worker, TokenOwnerScope::Issuer),
            RefillSource::Manual,
            BalanceRefillOutcome::Refilled,
            Some(100),
            None,
        );

        let over = tokens_fixture(SourceKind::Glm, &[], true, false).await;
        over.state.quota.store(JSON_SAFE + 1, Ordering::SeqCst);
        over.worker.balance_round().await.unwrap();
        let unknown = account(&over.worker, TokenOwnerScope::Issuer);
        assert_eq!(unknown.state, BalanceState::Unknown);
        assert!(unknown.quota.is_none());
        assert_eq!(
            unknown.unknown_reason,
            Some(ControlErrorCode::InvalidResponse)
        );
        assert_eq!(over.state.balance_adds.load(Ordering::SeqCst), 0);
        assert_eq!(
            manual(&over.worker, TokenOwnerScope::Issuer, 1)
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(over.state.balance_adds.load(Ordering::SeqCst), 0);
        assert_eq!(quota(&over, 41), JSON_SAFE + 1);
    })
    .await;
}
