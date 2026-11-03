//! 独立账号余额观察与补足；不参与普通路由 publication 代际。
use super::*;
use crate::newapi::tokens::{TokenOwner, TokenOwnerScope};
use crate::newapi::users::QuotaUnit;
use serde::Serialize;
use std::sync::MutexGuard;
use tokio::time::Instant;

#[cfg(test)]
mod tests;

pub(crate) const BALANCE_INTERVAL: Duration = Duration::from_secs(60);
const READING_FRESH_MS: i64 = 180_000;
const BLOCK_MS: i64 = 600_000;
const BALANCE_READ_BUDGET: Duration = Duration::from_secs(20);
const AUTO_REFILL_BUDGET: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefillSource {
    Auto,
    Manual,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefillAmount {
    ToTarget,
    Exact(i64),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Refilled,
    NotSent(ControlErrorCode),
    Rejected(ControlErrorCode),
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    at_ms: i64,
    source: RefillSource,
    amount: Option<i64>,
    outcome: Outcome,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefillReport {
    pub owner_scope: TokenOwnerScope,
    pub user_id: i64,
    pub before: i64,
    pub amount: i64,
    pub after: Option<i64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BalanceState {
    Sufficient,
    Low,
    Exhausted,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BalanceRefillOutcome {
    Refilled,
    NotSent,
    Rejected,
    Uncertain,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct BalanceView {
    pub policy: BalancePolicy,
    pub unit: Option<QuotaUnit>,
    pub accounts: Vec<BalanceAccountView>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct BalanceAccountView {
    pub owner_scope: TokenOwnerScope,
    pub user_id: String,
    pub state: BalanceState,
    pub quota: Option<i64>,
    pub observed_at_ms: Option<i64>,
    pub unknown_reason: Option<ControlErrorCode>,
    pub locked_until_ms: Option<i64>,
    pub backoff_until_ms: Option<i64>,
    pub last_refill: Option<BalanceRefillView>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct BalanceRefillView {
    pub at_ms: i64,
    pub source: RefillSource,
    pub outcome: BalanceRefillOutcome,
    pub amount: Option<i64>,
    pub error_code: Option<ControlErrorCode>,
}
struct Account {
    owner: TokenOwner,
    reading: Option<(i64, i64)>,
    failure: Option<(ControlErrorCode, i64)>,
    locked_until: i64,
    backoff_until: i64,
    last_refill: Option<Record>,
}
struct Slot {
    policy: BalancePolicy,
    unit: Option<QuotaUnit>,
    accounts: Vec<Account>,
}
pub(super) struct Balance {
    slot: Mutex<Slot>,
    issuer: AsyncMutex<()>,
    legacy: AsyncMutex<()>,
    failed: AtomicBool,
    #[cfg(test)]
    waiters: [std::sync::atomic::AtomicUsize; 2],
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .filter(|v| *v <= JSON_SAFE)
        .unwrap_or(JSON_SAFE)
}
fn fresh(reading: Option<(i64, i64)>, at: i64) -> bool {
    reading.is_some_and(|(_, observed)| {
        at.checked_sub(observed)
            .is_some_and(|age| (0..=READING_FRESH_MS).contains(&age))
    })
}
impl Account {
    fn view(&self, policy: BalancePolicy, at: i64) -> BalanceAccountView {
        let state = if self.failure.is_some() || !fresh(self.reading, at) {
            BalanceState::Unknown
        } else {
            match self.reading.unwrap().0 {
                n if n <= 0 => BalanceState::Exhausted,
                n if n < policy.warn_below => BalanceState::Low,
                _ => BalanceState::Sufficient,
            }
        };
        BalanceAccountView {
            owner_scope: self.owner.scope,
            user_id: self.owner.user_id.to_string(),
            state,
            quota: self.reading.map(|r| r.0),
            observed_at_ms: self.reading.map(|r| r.1),
            unknown_reason: self.failure.map(|r| r.0),
            locked_until_ms: (self.locked_until > at).then_some(self.locked_until),
            backoff_until_ms: (self.backoff_until > at).then_some(self.backoff_until),
            last_refill: self.last_refill.map(|r| {
                let (outcome, error_code) = match r.outcome {
                    Outcome::Refilled => (BalanceRefillOutcome::Refilled, None),
                    Outcome::NotSent(code) => (BalanceRefillOutcome::NotSent, Some(code)),
                    Outcome::Rejected(code) => (BalanceRefillOutcome::Rejected, Some(code)),
                    Outcome::Uncertain => (
                        BalanceRefillOutcome::Uncertain,
                        Some(ControlErrorCode::TransportUnknown),
                    ),
                };
                BalanceRefillView {
                    at_ms: r.at_ms,
                    source: r.source,
                    outcome,
                    amount: r.amount,
                    error_code,
                }
            }),
        }
    }
}
impl Balance {
    pub(super) fn new(policy: BalancePolicy) -> Self {
        Self {
            slot: Mutex::new(Slot {
                policy,
                unit: None,
                accounts: Vec::new(),
            }),
            issuer: AsyncMutex::new(()),
            legacy: AsyncMutex::new(()),
            failed: AtomicBool::new(false),
            #[cfg(test)]
            waiters: std::array::from_fn(|_| std::sync::atomic::AtomicUsize::new(0)),
        }
    }
    pub(super) fn configure(&mut self, owners: Vec<TokenOwner>) -> ControlResult<()> {
        let slot = self
            .slot
            .get_mut()
            .map_err(|_| error(ControlErrorCode::Storage))?;
        slot.accounts = owners
            .into_iter()
            .map(|owner| Account {
                owner,
                reading: None,
                failure: None,
                locked_until: 0,
                backoff_until: 0,
                last_refill: None,
            })
            .collect();
        Ok(())
    }
    fn slot(&self) -> ControlResult<MutexGuard<'_, Slot>> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(error(ControlErrorCode::Storage));
        }
        self.slot.lock().map_err(|_| {
            self.failed.store(true, Ordering::SeqCst);
            error(ControlErrorCode::Storage)
        })
    }
    fn owner_lock(&self, scope: TokenOwnerScope) -> &AsyncMutex<()> {
        match scope {
            TokenOwnerScope::Issuer => &self.issuer,
            TokenOwnerScope::LegacyAdmin => &self.legacy,
        }
    }
    fn account(slot: &mut Slot, scope: TokenOwnerScope) -> ControlResult<&mut Account> {
        slot.accounts
            .iter_mut()
            .find(|a| a.owner.scope == scope)
            .ok_or_else(|| error(ControlErrorCode::Storage))
    }
    fn reading(
        &self,
        scope: TokenOwnerScope,
        result: std::result::Result<i64, ControlErrorCode>,
    ) -> ControlResult<()> {
        let mut slot = self.slot()?;
        let account = Self::account(&mut slot, scope)?;
        match result {
            Ok(quota) => {
                account.reading = Some((quota, now_ms()));
                account.failure = None;
            }
            Err(code) => account.failure = Some((code, now_ms())),
        }
        Ok(())
    }
    fn record(
        &self,
        scope: TokenOwnerScope,
        source: RefillSource,
        amount: Option<i64>,
        outcome: Outcome,
    ) -> ControlResult<()> {
        let at = now_ms();
        let mut slot = self.slot()?;
        let account = Self::account(&mut slot, scope)?;
        account.last_refill = Some(Record {
            at_ms: at,
            source,
            amount,
            outcome,
        });
        let end = at.saturating_add(BLOCK_MS).min(JSON_SAFE);
        match outcome {
            Outcome::Rejected(_) => account.backoff_until = end,
            Outcome::Uncertain => {
                account.locked_until = end;
                account.backoff_until = end;
            }
            _ => {}
        }
        Ok(())
    }
}
struct RefillAttemptGuard<'a> {
    balance: &'a Balance,
    scope: TokenOwnerScope,
    source: RefillSource,
    amount: i64,
    settled: bool,
    // Drop的结算先执行，字段析构才解锁；不向后台分离补发/结算任务。
    _owner: tokio::sync::MutexGuard<'a, ()>,
}
#[cfg(test)]
struct Waiting<'a>(&'a std::sync::atomic::AtomicUsize);
#[cfg(test)]
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl RefillAttemptGuard<'_> {
    fn settle(&mut self, outcome: Outcome) -> ControlResult<()> {
        self.balance
            .record(self.scope, self.source, Some(self.amount), outcome)?;
        self.settled = true;
        Ok(())
    }
}
impl Drop for RefillAttemptGuard<'_> {
    fn drop(&mut self) {
        // Step B4.cancel: 先在所有者锁内封锁，后解锁；私有故障不写全局publication。
        if !self.settled
            && self
                .balance
                .record(
                    self.scope,
                    self.source,
                    Some(self.amount),
                    Outcome::Uncertain,
                )
                .is_err()
        {
            self.balance.failed.store(true, Ordering::SeqCst);
        }
    }
}
impl ReconcileWorker {
    pub(crate) async fn balance_round(&self) -> ControlResult<()> {
        // Step B3.observe: 失败保留历史读数，不以旧低余额触发自动补足。
        let owners: Vec<_> = self
            .balance
            .slot()?
            .accounts
            .iter()
            .map(|a| a.owner)
            .collect();
        if owners.is_empty() {
            return Ok(());
        }
        if let Ok(unit) = self
            .client
            .quota_unit_until(Instant::now() + BALANCE_READ_BUDGET)
            .await
        {
            self.balance.slot()?.unit = Some(unit);
        }
        for owner in owners {
            let result = match self.token_client(owner.scope) {
                Ok((client, _)) => client
                    .managed_tokens(owner)
                    .observe_profile_until(Instant::now() + BALANCE_READ_BUDGET)
                    .await
                    .map(|p| p.quota)
                    .map_err(|e| code(&e)),
                Err(e) => Err(e.code),
            };
            self.balance.reading(owner.scope, result)?;
            let eligible = {
                let slot = self.balance.slot()?;
                let at = now_ms();
                let account = slot
                    .accounts
                    .iter()
                    .find(|a| a.owner == owner)
                    .ok_or_else(|| error(ControlErrorCode::Storage))?;
                slot.policy.auto_refill
                    && account.failure.is_none()
                    && fresh(account.reading, at)
                    && account
                        .reading
                        .is_some_and(|(quota, _)| quota < slot.policy.warn_below)
                    && at >= account.locked_until
                    && at >= account.backoff_until
            };
            if eligible {
                if let Err(e) = self
                    .refill(
                        owner.scope,
                        RefillAmount::ToTarget,
                        RefillSource::Auto,
                        None,
                    )
                    .await
                {
                    if e.code == ControlErrorCode::Storage {
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn refill(
        &self,
        scope: TokenOwnerScope,
        amount: RefillAmount,
        source: RefillSource,
        route_deadline: Option<Instant>,
    ) -> ControlResult<Option<RefillReport>> {
        // Step B3.refill.1: 截止固定在入口，所有者互斥一直持有到结算/回读结束。
        drop(self.balance.slot()?);
        let (client, owner) = self.token_client(scope)?;
        let end = match source {
            RefillSource::Manual => {
                route_deadline.ok_or_else(|| error(ControlErrorCode::InvalidInput))?
            }
            RefillSource::Auto => Instant::now() + AUTO_REFILL_BUDGET,
        };
        #[cfg(test)]
        let waiting = (source == RefillSource::Manual).then(|| {
            let counter = &self.balance.waiters[usize::from(scope == TokenOwnerScope::LegacyAdmin)];
            counter.fetch_add(1, Ordering::SeqCst);
            Waiting(counter)
        });
        let lock = match source {
            RefillSource::Auto => self
                .balance
                .owner_lock(scope)
                .try_lock()
                .map_err(|_| error(ControlErrorCode::QueueFull))?,
            RefillSource::Manual => {
                tokio::time::timeout_at(end, self.balance.owner_lock(scope).lock())
                    .await
                    .map_err(|_| error(ControlErrorCode::QueueFull))?
            }
        };
        #[cfg(test)]
        drop(waiting);
        // Step B3.refill.1b: 先复核封锁，不覆盖原不确定记录；锁序不允许Slot→Core。
        {
            let mut slot = self.balance.slot()?;
            let at = now_ms();
            let account = Balance::account(&mut slot, scope)?;
            if at < account.locked_until
                || source == RefillSource::Auto && at < account.backoff_until
            {
                return Err(error(ControlErrorCode::NotReady));
            }
            if source == RefillSource::Manual && Instant::now() >= end {
                account.last_refill = Some(Record {
                    at_ms: at,
                    source,
                    amount: None,
                    outcome: Outcome::NotSent(ControlErrorCode::NotReady),
                });
                return Err(error(ControlErrorCode::NotReady));
            }
            if self.is_dry_run() {
                return Err(error(ControlErrorCode::NotReady));
            }
        }
        // Step B3.refill.2: Core只用于核存储和后端租约，不跨await，不使用正常代际。
        let frame = (|| {
            let core = self
                .core
                .lock()
                .map_err(|_| error(ControlErrorCode::Storage))?;
            core.store.usable()?;
            let start = now();
            if !core.backend.ready_at(start) {
                return Err(error(ControlErrorCode::NotReady));
            }
            Ok((start, core.backend.expires_at()))
        })();
        let (start, lease_end) = match frame {
            Ok(frame) => frame,
            Err(e) => {
                self.balance
                    .record(scope, source, None, Outcome::NotSent(e.code))?;
                return Err(e);
            }
        };
        // Step B3.refill.3: 本次身份核验后的新鲜读数才可参与计算。
        let before = match client
            .managed_tokens(owner)
            .profile_until(end.min(Instant::now() + BALANCE_READ_BUDGET))
            .await
        {
            Ok(profile) => {
                self.balance.reading(scope, Ok(profile.quota))?;
                profile.quota
            }
            Err(e) => {
                self.balance.reading(scope, Err(code(&e)))?;
                self.balance.record(
                    scope,
                    source,
                    None,
                    Outcome::NotSent(ControlErrorCode::NotReady),
                )?;
                return Err(error(ControlErrorCode::NotReady));
            }
        };
        // Step B3.refill.4: 自动开关在读完后重查；只承诺每笔及before+amount有界。
        let value = {
            let slot = self.balance.slot()?;
            match amount {
                RefillAmount::ToTarget => {
                    if !slot.policy.auto_refill || before >= slot.policy.warn_below {
                        return Ok(None);
                    }
                    slot.policy
                        .refill_to
                        .checked_sub(before)
                        .map(|a| a.min(BALANCE_CAP))
                }
                RefillAmount::Exact(a) => Some(a),
            }
        };
        let Some(value) = value.filter(|a| {
            (1..=BALANCE_CAP).contains(a)
                && before.checked_add(*a).is_some_and(|sum| sum <= BALANCE_CAP)
        }) else {
            self.balance.record(
                scope,
                source,
                None,
                Outcome::NotSent(ControlErrorCode::InvalidInput),
            )?;
            return Err(error(ControlErrorCode::InvalidInput));
        };
        // Step B3.refill.5: 潜在投递前无await转移锁；截止/回拨/私有故障均在最终准入复核。
        let mut attempt = RefillAttemptGuard {
            balance: &self.balance,
            scope,
            source,
            amount: value,
            settled: false,
            _owner: lock,
        };
        let guard = || {
            let at = now();
            start <= at
                && at < lease_end
                && Instant::now() < end
                && !self.balance.failed.load(Ordering::SeqCst)
        };
        let result = self
            .client
            .add_user_quota_until(owner.user_id, value, &guard, end)
            .await;
        // Step B3.refill.6: 先记录并封锁，再解除取消保护；500+合法JSON也属不确定。
        let outcome = match result {
            Ok(()) => Outcome::Refilled,
            Err(e) if e.delivery == DeliveryEvidence::NotSent => Outcome::NotSent(match code(&e) {
                ControlErrorCode::RevisionConflict | ControlErrorCode::TransportUnknown => {
                    ControlErrorCode::NotReady
                }
                c => c,
            }),
            Err(e)
                if e.delivery == DeliveryEvidence::ResponseReceived
                    && e.status.is_none_or(|s| s < 500) =>
            {
                Outcome::Rejected(code(&e))
            }
            Err(_) => Outcome::Uncertain,
        };
        attempt.settle(outcome)?;
        match outcome {
            Outcome::NotSent(c) | Outcome::Rejected(c) => return Err(error(c)),
            Outcome::Uncertain => return Err(error(ControlErrorCode::TransportUnknown)),
            Outcome::Refilled => {}
        }
        // Step B3.refill.7: 回读失败/被取消不会推翻已确认的Refilled。
        let after = if Instant::now() >= end {
            self.balance
                .reading(scope, Err(ControlErrorCode::NotReady))?;
            None
        } else {
            match client
                .managed_tokens(owner)
                .profile_until(end.min(Instant::now() + BALANCE_READ_BUDGET))
                .await
            {
                Ok(profile) => {
                    self.balance.reading(scope, Ok(profile.quota))?;
                    Some(profile.quota)
                }
                Err(e) => {
                    self.balance.reading(scope, Err(code(&e)))?;
                    None
                }
            }
        };
        Ok(Some(RefillReport {
            owner_scope: scope,
            user_id: owner.user_id,
            before,
            amount: value,
            after,
        }))
    }

    pub(crate) fn set_balance_policy(
        &self,
        expected_revision: i64,
        draft: BalancePolicyDraft,
    ) -> ControlResult<BalancePolicy> {
        // Step B5.policy: 成功/明确回滚不改代际；真正存储中毒只在Core锁内封发布。
        drop(self.balance.slot()?);
        if self.is_dry_run() {
            return Err(error(ControlErrorCode::NotReady));
        }
        let mut core = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?;
        let result = core.store.save_balance_policy(expected_revision, draft);
        if core.store.usable().is_err() {
            self.publication.store(u64::MAX, Ordering::SeqCst);
            return Err(error(ControlErrorCode::Storage));
        }
        let policy = result?;
        self.balance.slot()?.policy = policy;
        Ok(policy)
    }
    pub(crate) fn balance_view(&self, at_ms: i64) -> ControlResult<BalanceView> {
        // Step B6.balance_view: 失败/缺失/回拨/超龄均未知，负余额仍是有效读数。
        let slot = self.balance.slot()?;
        Ok(BalanceView {
            policy: slot.policy,
            unit: slot.unit.clone(),
            accounts: slot
                .accounts
                .iter()
                .map(|a| a.view(slot.policy, at_ms))
                .collect(),
        })
    }
    #[cfg(test)]
    pub(crate) fn balance_test_last_refills(&self) -> ControlResult<serde_json::Value> {
        let slot = self.balance.slot()?;
        Ok(serde_json::Value::Array(slot.accounts.iter().map(|a|serde_json::json!({"owner_scope":a.owner.scope,"last_refill":a.view(slot.policy,now_ms()).last_refill})).collect()))
    }
    #[cfg(test)]
    pub(crate) async fn balance_test_owner_lock(
        &self,
        scope: TokenOwnerScope,
    ) -> tokio::sync::MutexGuard<'_, ()> {
        self.balance.owner_lock(scope).lock().await
    }
    #[cfg(test)]
    pub(crate) fn balance_test_waiters(&self, scope: TokenOwnerScope) -> usize {
        self.balance.waiters[usize::from(scope == TokenOwnerScope::LegacyAdmin)]
            .load(Ordering::SeqCst)
    }
}
