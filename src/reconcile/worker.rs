//! 单写通道执行器。状态发布先失效 fence，网络等待不持目录锁；策略整批提交后发布。
use super::planner::*;
use crate::catalog::control::{
    ControlCommand, ControlError, ControlErrorCode, ControlResult, OperationId, OperationPhase,
    OperationView, StatusDirective,
};
use crate::catalog::execution::{ExecutionEffect, ExecutionOutcome, WorkTicket};
use crate::catalog::*;
use crate::config::Config;
use crate::newapi::managed_channels::{ChannelPatch, DisabledChannelSpec, NativeRecoveryCheck};
use crate::newapi::transport::{DeliveryEvidence, ManagementError, ManagementErrorKind};
use crate::newapi::NewApiClient;
use crate::source_policy::probe::SourceProbe;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::{watch, Mutex as AsyncMutex, Notify, Semaphore};

mod admin;
mod analytics;
mod collection;
mod management;
pub(crate) mod panel;
mod policy;
pub(crate) mod token_service;

struct Core {
    input_epoch: u64,
    store: CatalogStore,
    backend: BackendReadiness,
    source_disabled: HashMap<SourceId, bool>,
    permits: HashMap<MembershipId, RoutingPermit>,
    last_glm_cycle: Option<u64>,
    panel_glm: Option<panel::GlmDisplayCache>,
    observations: HashMap<MembershipId, CachedObservation>,
}
enum CachedObservation {
    Found(StampedObservation),
    Unavailable {
        revision: Revision,
        code: ControlErrorCode,
        checked_at: i64,
    },
}
impl CachedObservation {
    fn found(&self) -> Option<&StampedObservation> {
        match self {
            Self::Found(o) => Some(o),
            _ => None,
        }
    }
    fn problem(&self, revision: Revision, at: i64) -> Option<ControlErrorCode> {
        match self {
            Self::Unavailable {
                revision: r,
                code,
                checked_at,
            } if *r == revision
                && at
                    .checked_sub(*checked_at)
                    .is_some_and(|age| (0..=OBSERVATION_MAX_AGE_SECS).contains(&age)) =>
            {
                Some(*code)
            }
            _ => None,
        }
    }
}
pub struct ReconcileWorker {
    core: Mutex<Core>,
    publication: AtomicU64,
    lane: AsyncMutex<()>,
    commands: Semaphore,
    changed: Notify,
    client: Arc<NewApiClient>,
    policy_config: Arc<Config>,
    probe: AsyncMutex<SourceProbe>,
    model_catalog: AsyncMutex<crate::model_catalog::ModelCatalogClient>,
    collector: AsyncMutex<collection::CollectorSession>,
    collection_running: AtomicBool,
    query_slots: Arc<Semaphore>,
    repair_cursor: AsyncMutex<Option<crate::catalog::ledger::AttributionCursor>>,
    token_owners: Option<token_service::TokenOwners>,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct RoundReport {
    pub visited: usize,
    pub progressed: usize,
    pub waiting: usize,
    pub failed: usize,
}
struct Plan {
    input_epoch: u64,
    epoch: u64,
    work: WorkSnapshot,
    group: Vec<WorkSnapshot>,
    observation: Option<StampedObservation>,
    observation_problem: Option<ControlErrorCode>,
    backend: BackendReadiness,
    permit: Option<RoutingPermit>,
    members: Vec<MembershipId>,
    peers: Vec<Peer>,
    now: i64,
}
impl Plan {
    fn context(&self) -> PlanContext<'_> {
        PlanContext {
            backend: self.backend,
            permit: self.permit,
            expected_members: &self.members,
            peers: &self.peers,
            now: self.now,
        }
    }
    fn step(&self) -> PlanStep {
        if let Some(code) = self.observation_problem {
            return if crate::catalog::confirmation::terminal(code) {
                PlanStep::Reject(code)
            } else {
                PlanStep::Wait(code)
            };
        }
        plan_next_action(&self.work, self.observation.as_ref(), &self.context())
    }
    fn deadline(&self, write: bool) -> i64 {
        if !write {
            return JSON_SAFE;
        }
        let mut end = self.backend.expires_at();
        if let Some(o) = &self.observation {
            end = end.min(observation_deadline(o));
        }
        let routes = matches!(self.step(), PlanStep::Enable)
            || matches!(self.step(),PlanStep::Patch(p) if p.priority.is_some_and(|p|self.observation.as_ref().is_some_and(|o|o.channel.status==1 && p>o.channel.priority)));
        if routes {
            end = end.min(self.permit.map(|p| p.expires_at()).unwrap_or(self.now));
            for peer in &self.peers {
                if let Some(o) = &peer.observation {
                    end = end.min(observation_deadline(o));
                }
            }
        }
        end
    }
}
fn error(code: ControlErrorCode) -> ControlError {
    ControlError { code }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .filter(|v| *v <= JSON_SAFE)
        .unwrap_or(JSON_SAFE)
}
fn observation_deadline(o: &StampedObservation) -> i64 {
    o.channel
        .observed_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .and_then(|t| t.checked_add(OBSERVATION_MAX_AGE_SECS + 1))
        .unwrap_or(0)
}
fn code(e: &ManagementError) -> ControlErrorCode {
    if e.status.is_some_and(|status| status >= 500) {
        return ControlErrorCode::TransportUnknown;
    }
    match e.kind {
        ManagementErrorKind::InvalidInput => ControlErrorCode::InvalidInput,
        ManagementErrorKind::RevisionConflict => ControlErrorCode::RevisionConflict,
        ManagementErrorKind::QueueFull => ControlErrorCode::QueueFull,
        ManagementErrorKind::AuthFailed => ControlErrorCode::AuthFailed,
        ManagementErrorKind::RemoteRejected => ControlErrorCode::RemoteRejected,
        ManagementErrorKind::InvalidResponse => ControlErrorCode::InvalidResponse,
        ManagementErrorKind::RateLimited => ControlErrorCode::RateLimited,
        ManagementErrorKind::TransportUnknown => ControlErrorCode::TransportUnknown,
    }
}
fn same_target(a: Revision, b: Revision) -> bool {
    (a.member, a.source, a.group) == (b.member, b.source, b.group)
}
fn can_route(plan: &Plan) -> bool {
    plan.backend.ready_at(plan.now)
        && plan.work.group.ready
        && !plan.work.source.manual_paused
        && !plan.work.source_retire_requested
        && !plan.work.target.retire_requested
        && !plan.work.policy_disabled
        && plan.work.pending_write == PendingWrite::None
        && plan
            .permit
            .is_some_and(|p| p.valid_for(&plan.work, plan.now))
}

impl ReconcileWorker {
    pub fn new(
        store: CatalogStore,
        client: Arc<NewApiClient>,
        policy_config: Arc<Config>,
    ) -> ControlResult<Arc<Self>> {
        crate::catalog::glm_round::validate_config(&policy_config)?;
        Ok(Arc::new(Self {
            core: Mutex::new(Core {
                input_epoch: 0,
                store,
                backend: BackendReadiness::from_preflight(
                    "",
                    NativeRecoveryCheck::Unknown,
                    false,
                    0,
                    0,
                ),
                source_disabled: HashMap::new(),
                permits: HashMap::new(),
                last_glm_cycle: None,
                panel_glm: None,
                observations: HashMap::new(),
            }),
            publication: AtomicU64::new(0),
            lane: AsyncMutex::new(()),
            commands: Semaphore::new(64),
            changed: Notify::new(),
            client,
            policy_config,
            probe: AsyncMutex::new(SourceProbe::default()),
            model_catalog: AsyncMutex::new(crate::model_catalog::ModelCatalogClient::new(
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(3))
                    .build()
                    .map_err(|_| error(ControlErrorCode::Storage))?,
            )),
            collector: AsyncMutex::new(collection::CollectorSession::default()),
            collection_running: AtomicBool::new(false),
            query_slots: Arc::new(Semaphore::new(1)),
            repair_cursor: AsyncMutex::new(None),
            token_owners: None,
        }))
    }
    fn mutate<T>(
        &self,
        expected: Option<u64>,
        f: impl FnOnce(&mut Core) -> ControlResult<T>,
    ) -> ControlResult<T> {
        let mut core = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?;
        core.store.usable()?;
        let epoch = self.publication.load(Ordering::SeqCst);
        if !epoch.is_multiple_of(2) || epoch >= u64::MAX - 3 {
            self.publication.store(u64::MAX, Ordering::SeqCst);
            return Err(error(ControlErrorCode::Storage));
        }
        if expected.is_some_and(|e| e != epoch) {
            return Err(error(ControlErrorCode::RevisionConflict));
        }
        // Step P13: 提交之前先封旧 gate；失败回滚可重新发布，未知提交保持关闭。
        self.publication.store(epoch + 1, Ordering::SeqCst);
        let result = f(&mut core);
        if core.store.usable().is_ok() {
            self.publication.store(epoch + 2, Ordering::SeqCst);
        }
        self.changed.notify_one();
        result
    }
    fn publish<T>(&self, f: impl FnOnce(&mut Core) -> ControlResult<T>) -> ControlResult<T> {
        self.mutate(None, |c| {
            let value = f(c)?;
            c.input_epoch = self.publication.load(Ordering::SeqCst) + 1;
            Ok(value)
        })
    }
    pub fn work(&self, id: MembershipId) -> ControlResult<WorkSnapshot> {
        let mut core = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?;
        let mut work = core.store.load_work(id, false)?;
        work.policy_disabled = core
            .source_disabled
            .get(&work.source.source_id)
            .copied()
            .unwrap_or(false);
        Ok(work)
    }
    pub fn snapshot(&self) -> ControlResult<CatalogSnapshot> {
        Ok(self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?
            .store
            .snapshot()?)
    }
    pub fn operation(&self, id: &OperationId) -> ControlResult<OperationView> {
        self.core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?
            .store
            .operation(id)
    }
    pub fn submit(&self, id: OperationId, command: ControlCommand) -> ControlResult<OperationView> {
        if self.policy_config.dry_run && command.is_c2() {
            return Err(error(ControlErrorCode::NotReady));
        }
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        self.publish(|c| c.store.submit_operation(id, command, now()))
    }
    fn catalog_change<T>(
        &self,
        f: impl FnOnce(&mut CatalogStore) -> crate::catalog::types::Result<T>,
    ) -> crate::catalog::types::Result<T> {
        let _slot = self.commands.try_acquire().map_err(|_| CatalogError {
            kind: ErrorKind::Busy,
            source_id: None,
            incomplete_backup: false,
        })?;
        // 保留目录错误及 DuplicateSource 的原 ID，不把用户输入问题改写为 Storage。
        self.mutate(None, |c| {
            let result = f(&mut c.store);
            if result.is_ok() {
                c.input_epoch = self.publication.load(Ordering::SeqCst) + 1;
            }
            Ok(result)
        })
        .map_err(|_| CatalogError {
            kind: ErrorKind::Storage,
            source_id: None,
            incomplete_backup: false,
        })?
    }
    pub fn create_source(&self, source: SourceDraft) -> crate::catalog::types::Result<SourceView> {
        self.catalog_change(|c| c.create_source(source))
    }
    pub fn create_group(&self, group: GroupDraft) -> crate::catalog::types::Result<GroupView> {
        self.catalog_change(|c| c.create_group(group))
    }
    pub fn plan_membership(
        &self,
        source: SourceId,
        group: GroupId,
        priority: i64,
    ) -> crate::catalog::types::Result<MembershipView> {
        self.catalog_change(|c| c.plan_membership(source, group, priority))
    }
    pub async fn retry(&self, id: &OperationId) -> ControlResult<OperationView> {
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let view = self.publish(|c| {
            let view = c.store.retry_operation(id, now())?;
            if view.phase != OperationPhase::Succeeded {
                // 人工修复远端后无需等负缓存 TTL；不改变持久未知写或历史收据。
                c.observations.clear();
            }
            Ok(view)
        })?;
        if view.phase == OperationPhase::Succeeded {
            return Ok(view);
        }
        if view.target_type == crate::catalog::control::TargetType::Token {
            self.retry_token_auth(id).await?;
            return Ok(view);
        }
        self.client
            .retry_management_authentication()
            .await
            .map_err(|e| error(code(&e)))?;
        Ok(view)
    }
    /// 只供可信预检组件发布；不是用户 HTTP 输入，也不自行证明实例身份。
    pub fn publish_backend(&self, backend: BackendReadiness) -> ControlResult<()> {
        self.publish(|c| {
            c.store.wake_pending(None, false, now())?;
            c.backend = backend;
            c.observations.clear();
            Ok(())
        })
    }
    /// 复用本地托管预检；先撤销旧许可，网络返回后拒绝覆盖较新的发布。
    pub async fn refresh_backend(
        &self,
        process: &crate::boot::NewApiProcess,
    ) -> ControlResult<crate::newapi::preflight::BackendCheck> {
        self.refresh_from(self.client.check_local_backend(process))
            .await
    }
    async fn refresh_from(
        &self,
        check: impl std::future::Future<Output = crate::newapi::preflight::BackendCheck>,
    ) -> ControlResult<crate::newapi::preflight::BackendCheck> {
        let revoked = self.publish(|c| {
            c.backend = crate::newapi::preflight::unavailable();
            c.observations.clear();
            Ok(self.publication.load(Ordering::SeqCst) + 1)
        })?;
        let check = check.await;
        self.mutate(None, |c| {
            if c.input_epoch != revoked {
                return Err(error(ControlErrorCode::RevisionConflict));
            }
            c.store.wake_pending(None, false, now())?;
            c.backend = check.readiness();
            c.observations.clear();
            c.input_epoch = self.publication.load(Ordering::SeqCst) + 1;
            Ok(())
        })?;
        Ok(check)
    }
    /// 策略与组注册层提供；来源停用 gate 按 SourceId 共享，不按下游协议拆分。
    #[cfg(test)]
    pub fn publish_route(
        &self,
        id: MembershipId,
        source_revision: i64,
        group_revision: i64,
        disabled: bool,
        permit: Option<RoutingPermit>,
    ) -> ControlResult<()> {
        self.publish(|c| {
            let w = c.store.load_work(id, false)?;
            if w.source.config_revision != source_revision || w.group.revision != group_revision {
                return Err(error(ControlErrorCode::RevisionConflict));
            }
            if disabled && w.source.kind == SourceKind::Glm {
                return Err(error(ControlErrorCode::InvalidInput));
            }
            if permit.is_some_and(|p| !p.valid_for(&w, now())) {
                return Err(error(ControlErrorCode::NotReady));
            }
            let changed = c.source_disabled.get(&w.source.source_id).copied() != Some(disabled);
            c.store
                .wake_pending(Some(w.source.source_id), changed, now())?;
            c.source_disabled.insert(w.source.source_id, disabled);
            if let Some(p) = permit {
                c.permits.insert(id, p);
            } else {
                c.permits.remove(&id);
            }
            Ok(())
        })
    }
    fn assemble(&self, id: MembershipId) -> ControlResult<Plan> {
        let mut c = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?;
        if !self.publication.load(Ordering::SeqCst).is_multiple_of(2) {
            return Err(error(ControlErrorCode::Storage));
        }
        let mut work = c.store.load_work(id, false)?;
        work.policy_disabled = c
            .source_disabled
            .get(&work.source.source_id)
            .copied()
            .unwrap_or(false);
        let group = c.store.load_group_work(work.group.group_id)?;
        let members = group.iter().map(|w| w.member.membership_id).collect();
        let peers = group
            .iter()
            .filter(|w| w.member.membership_id != id)
            .map(|w| Peer {
                member_id: w.member.membership_id,
                group_id: w.group.group_id,
                revision: w.revision(),
                channel_id: w.member.channel_id,
                create_state: w.target.create_state,
                identity: w.identity.clone(),
                desired_priority: w.target.desired_priority,
                desired_disabled: w.source.manual_paused
                    || w.source_retire_requested
                    || w.target.retire_requested
                    || w.target.status_directive == StatusDirective::Disabled
                    || c.source_disabled
                        .get(&w.source.source_id)
                        .copied()
                        .unwrap_or(false),
                pending_write: w.pending_write,
                observation: c
                    .observations
                    .get(&w.member.membership_id)
                    .and_then(CachedObservation::found)
                    .cloned(),
            })
            .collect();
        Ok(Plan {
            input_epoch: c.input_epoch,
            epoch: self.publication.load(Ordering::SeqCst),
            observation: c
                .observations
                .get(&id)
                .and_then(CachedObservation::found)
                .cloned(),
            observation_problem: c
                .observations
                .get(&id)
                .and_then(|o| o.problem(work.revision(), now())),
            backend: c.backend,
            permit: c.permits.get(&id).copied(),
            work,
            group,
            members,
            peers,
            now: now(),
        })
    }
    fn gate(&self, epoch: u64, start: i64, end: i64, stop: &AtomicBool) -> bool {
        let at = now();
        !stop.load(Ordering::SeqCst)
            && epoch.is_multiple_of(2)
            && self.publication.load(Ordering::SeqCst) == epoch
            && start <= at
            && at < end
    }
    fn defer(
        &self,
        plan: &Plan,
        code: ControlErrorCode,
        after: Option<Duration>,
    ) -> ControlResult<()> {
        if code == ControlErrorCode::RevisionConflict {
            return Ok(());
        }
        self.mutate(None, |c| {
            if c.input_epoch != plan.input_epoch {
                return Err(error(ControlErrorCode::RevisionConflict));
            }
            let w = c.store.load_work(plan.work.member.membership_id, false)?;
            if !same_target(w.revision(), plan.work.revision()) {
                return Ok(());
            }
            let at = now();
            let retry_at = after.map(|d| {
                let seconds = d
                    .as_secs()
                    .saturating_add(u64::from(d.subsec_nanos() != 0))
                    .saturating_add(1);
                at.saturating_add(i64::try_from(seconds).unwrap_or(JSON_SAFE))
                    .min(JSON_SAFE)
            });
            c.store
                .defer_member(w.member.membership_id, w.revision(), code, at, retry_at)
        })
    }
    async fn observe(&self, plan: &Plan, stop: &AtomicBool) -> ControlResult<()> {
        let required: HashSet<_> = plan
            .group
            .iter()
            .filter_map(|w| w.member.channel_id.map(ChannelId::get))
            .collect();
        if required.is_empty() {
            return Ok(());
        }
        let guard = || self.gate(plan.epoch, plan.now, JSON_SAFE, stop);
        let api = self.client.managed_channels();
        let api = api.fenced(&guard);
        let mut result = None;
        for attempt in 0..3 {
            match api
                .scan_group(&plan.work.group.newapi_group, Some(&required))
                .await
            {
                Ok(scan) => {
                    result = Some(scan);
                    break;
                }
                Err(e) => {
                    if attempt == 2 || !retryable(&e) {
                        self.defer(plan, code(&e), e.retry_after)?;
                        return Err(error(code(&e)));
                    }
                    tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                }
            }
        }
        let channels = result
            .ok_or_else(|| error(ControlErrorCode::InvalidResponse))?
            .channels
            .into_iter()
            .map(|d| d.observation().map_err(|e| error(code(&e))))
            .collect::<ControlResult<Vec<_>>>()?;
        let mut updates = Vec::new();
        let mut valid = Vec::new();
        let mut unavailable = Vec::new();
        let mut own_error = None;
        for w in &plan.group {
            if let Some(id) = w.member.channel_id {
                let observation = channels.iter().find(|o| o.channel_id == id.get());
                let problem = match observation {
                    None => Some(ControlErrorCode::BindingDrift),
                    Some(o) if !identity_matches(&w.identity, id, o) => {
                        Some(ControlErrorCode::BindingDrift)
                    }
                    Some(o) if !fresh(o, now()) => Some(ControlErrorCode::NotReady),
                    Some(o) if !(-JSON_SAFE..=JSON_SAFE).contains(&o.priority) => {
                        Some(ControlErrorCode::InvalidResponse)
                    }
                    Some(_) => None,
                };
                let found = observation.filter(|_| problem.is_none());
                updates.push(crate::catalog::confirmation::ObservedFields {
                    member: w.member.membership_id,
                    revision: w.revision(),
                    status: found.map(|o| o.status),
                    priority: found.map(|o| o.priority),
                });
                if let Some(o) = found {
                    valid.push((
                        w.member.membership_id,
                        StampedObservation {
                            revision: w.revision(),
                            channel: o.clone(),
                        },
                    ));
                }
                if let Some(code) = problem {
                    unavailable.push((w.member.membership_id, w.revision(), code));
                }
                if w.member.membership_id == plan.work.member.membership_id {
                    own_error = problem;
                }
            }
        }
        self.mutate(Some(plan.epoch), |c| {
            // 完整扫描的缺失也是真实证据；不得留下被本次扫描否定的旧正缓存。
            c.store.record_observed(&updates)?;
            for u in &updates {
                c.observations.remove(&u.member);
            }
            for (id, o) in valid {
                c.observations.insert(id, CachedObservation::Found(o));
            }
            for (id, revision, code) in unavailable {
                c.observations.insert(
                    id,
                    CachedObservation::Unavailable {
                        revision,
                        code,
                        checked_at: now(),
                    },
                );
            }
            Ok(())
        })?;
        if let Some(code) = own_error {
            self.defer(plan, code, None)?;
            return Err(error(code));
        }
        Ok(())
    }
    async fn lookup(&self, plan: &Plan, stop: &AtomicBool) -> ControlResult<()> {
        let ticket = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?
            .store
            .pending_creation(plan.work.member.membership_id)?;
        let guard = || self.gate(plan.epoch, plan.now, JSON_SAFE, stop);
        let api = self.client.managed_channels();
        let api = api.fenced(&guard);
        let scan = match api.scan_group(&plan.work.group.newapi_group, None).await {
            Ok(scan) => scan,
            Err(e) => {
                self.defer(plan, code(&e), e.retry_after)?;
                return Err(error(code(&e)));
            }
        };
        let mut channels = scan
            .channels
            .into_iter()
            .map(|d| d.observation().map_err(|e| error(code(&e))))
            .collect::<ControlResult<Vec<_>>>()?;
        self.refresh_creation_observations(plan, stop, &mut channels)
            .await?;
        let result = self.mutate(Some(plan.epoch), |c| {
            c.store
                .bind_created(&ticket, scan.complete, &channels, c.backend, now())?;
            c.observations.remove(&plan.work.member.membership_id);
            Ok(())
        });
        if let Err(e) = &result {
            self.defer(plan, e.code, None)?;
        }
        result
    }
    async fn refresh_creation_observations(
        &self,
        plan: &Plan,
        stop: &AtomicBool,
        channels: &mut [crate::newapi::managed_channels::ChannelObservation],
    ) -> ControlResult<()> {
        let guard = || self.gate(plan.epoch, plan.now, JSON_SAFE, stop);
        let api = self.client.managed_channels();
        let api = api.fenced(&guard);
        for channel in channels
            .iter_mut()
            .filter(|o| o.name == plan.work.target.create_name)
        {
            if !fresh(channel, now()) {
                *channel = match api
                    .get_channel(channel.channel_id)
                    .await
                    .and_then(|d| d.observation())
                {
                    Ok(observation) => observation,
                    Err(e) => {
                        self.defer(plan, code(&e), e.retry_after)?;
                        return Err(error(code(&e)));
                    }
                };
            }
        }
        Ok(())
    }
    async fn dispatch(
        &self,
        ticket: &WorkTicket,
        plan: &Plan,
        epoch: u64,
        stop: &AtomicBool,
        credential: Option<&SecretString>,
    ) -> std::result::Result<bool, ManagementError> {
        let deadline = plan.deadline(true);
        let guard = || !self.policy_config.dry_run && self.gate(epoch, plan.now, deadline, stop);
        let api = self.client.managed_channels();
        let api = api.fenced(&guard);
        match ticket.effect() {
            ExecutionEffect::Create {
                name,
                channel_type,
                group,
                base_url,
                models,
                priority,
                kind,
            } => {
                api.create_disabled_channel(DisabledChannelSpec {
                    name,
                    source_kind: *kind,
                    channel_type: *channel_type,
                    key: credential.ok_or_else(management_storage)?,
                    base_url,
                    models: &models.join(","),
                    group,
                    standby_priority: *priority,
                })
                .await?;
                Ok(true)
            }
            ExecutionEffect::Fields {
                priority,
                models,
                disable_auto_ban,
            } => {
                api.write_channel_fields(
                    plan.work
                        .member
                        .channel_id
                        .ok_or_else(management_storage)?
                        .get(),
                    ChannelPatch {
                        priority: *priority,
                        models: models.clone(),
                        disable_auto_ban: *disable_auto_ban,
                    },
                )
                .await
            }
            ExecutionEffect::Enable | ExecutionEffect::Disable => {
                api.write_status(
                    &[plan
                        .work
                        .member
                        .channel_id
                        .ok_or_else(management_storage)?
                        .get()],
                    matches!(ticket.effect(), ExecutionEffect::Enable),
                )
                .await?;
                Ok(true)
            }
        }
    }
    async fn write(&self, plan: &Plan, stop: &AtomicBool) -> ControlResult<()> {
        if self.policy_config.dry_run {
            return Err(error(ControlErrorCode::NotReady));
        }
        let (ticket, credential) = self.mutate(Some(plan.epoch), |c| {
            let credential = if plan.step() == PlanStep::CreateDisabled {
                Some(
                    c.store
                        .read_source_material(plan.work.source.source_id)?
                        .credential,
                )
            } else {
                None
            };
            let ticket = c.store.prepare_execution(
                plan.work.member.membership_id,
                plan.work.revision(),
                plan.work.policy_disabled,
                plan.observation.as_ref(),
                &plan.context(),
            )?;
            Ok((ticket, credential))
        })?;
        // 与这次准备事务同代发布，不能认领解锁后另一条用户命令的新 epoch。
        let epoch = plan.epoch + 2;
        let mut result = self
            .dispatch(&ticket, plan, epoch, stop, credential.as_ref())
            .await;
        for attempt in 0..2 {
            if !result
                .as_ref()
                .err()
                .is_some_and(|e| e.delivery == DeliveryEvidence::NotSent && retryable(e))
            {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            result = self
                .dispatch(&ticket, plan, epoch, stop, credential.as_ref())
                .await;
        }
        let outcome = match &result {
            Ok(true) => ExecutionOutcome::Responded(None),
            Ok(false) => ExecutionOutcome::NotSent(ControlErrorCode::NotReady),
            Err(e) => match e.delivery {
                DeliveryEvidence::NotSent => ExecutionOutcome::NotSent(code(e)),
                DeliveryEvidence::ResponseReceived => ExecutionOutcome::Responded(Some(code(e))),
                DeliveryEvidence::Uncertain => ExecutionOutcome::Uncertain,
            },
        };
        // Step P14: 旧票只结算自身；HTTP success 不是 applied，观察仍须按当前代次重取。
        self.mutate(None, |c| {
            c.store.settle_execution(&ticket, outcome, now())?;
            c.observations.remove(&plan.work.member.membership_id);
            Ok(())
        })?;
        let current = self.assemble(plan.work.member.membership_id)?;
        if current.input_epoch != plan.input_epoch
            || !same_target(current.work.revision(), plan.work.revision())
        {
            return Ok(());
        }
        if let Err(e) = result {
            // 应用已答复但拒绝时，实际状态仍可能满足目标；先回读再决定失败。
            if e.delivery == DeliveryEvidence::ResponseReceived
                && current.work.member.channel_id.is_some()
            {
                self.observe(&current, stop).await?;
                let readback = self.assemble(plan.work.member.membership_id)?;
                if readback.step() == PlanStep::Confirm {
                    return self.confirm(&readback);
                }
            }
            self.defer(&current, code(&e), e.retry_after)?;
            return Err(error(code(&e)));
        }
        Ok(())
    }
    fn confirm(&self, plan: &Plan) -> ControlResult<()> {
        self.mutate(Some(plan.epoch), |c| {
            c.store.confirm_member(
                plan.work.member.membership_id,
                plan.work.revision(),
                plan.work.policy_disabled,
                plan.observation.as_ref(),
                &plan.context(),
            )
        })
    }
    async fn step(&self, id: MembershipId, stop: &AtomicBool) -> ControlResult<bool> {
        let plan = self.assemble(id)?;
        match plan.step() {
            PlanStep::Observe => self.observe(&plan, stop).await?,
            PlanStep::LookupCreate => self.lookup(&plan, stop).await?,
            PlanStep::CreateDisabled
            | PlanStep::Disable
            | PlanStep::Enable
            | PlanStep::Patch(_) => self.write(&plan, stop).await?,
            PlanStep::Confirm | PlanStep::RetireAbsent => self.confirm(&plan)?,
            PlanStep::Wait(ControlErrorCode::NotReady) => {
                if !can_route(&plan) {
                    self.defer(&plan, ControlErrorCode::NotReady, None)?;
                    return Ok(false);
                }
                if self.peers_need_observation(&plan)? {
                    self.observe(&plan, stop).await?;
                } else if self.actionable_blocker(&plan)? {
                    return Ok(false);
                } else {
                    self.defer(&plan, ControlErrorCode::NotReady, None)?;
                    return Ok(false);
                }
            }
            PlanStep::Wait(e) | PlanStep::Reject(e) => {
                self.defer(&plan, e, None)?;
                return Err(error(e));
            }
        }
        Ok(true)
    }
    fn actionable_blocker(&self, plan: &Plan) -> ControlResult<bool> {
        let c = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?;
        if self.publication.load(Ordering::SeqCst) != plan.epoch {
            return Err(error(ControlErrorCode::RevisionConflict));
        }
        for w in &plan.group {
            if w.member.membership_id != plan.work.member.membership_id
                && w.target.dirty
                && w.target.next_attempt_at <= plan.now
            {
                let cached = c.observations.get(&w.member.membership_id);
                if cached
                    .and_then(|o| o.problem(w.revision(), plan.now))
                    .is_some()
                {
                    continue;
                }
                let mut work = w.clone();
                work.policy_disabled = c
                    .source_disabled
                    .get(&w.source.source_id)
                    .copied()
                    .unwrap_or(false);
                let observed = cached.and_then(CachedObservation::found);
                // 这里只找可执行的停止/降低，不签发许可。空屏障使提升/启用失败，
                // 避免每个 peer 重新加载并构造整个组，保持线性遍历。
                let context = PlanContext {
                    backend: plan.backend,
                    permit: c.permits.get(&w.member.membership_id).copied(),
                    expected_members: &[],
                    peers: &[],
                    now: plan.now,
                };
                let step = plan_next_action(&work, observed, &context);
                if matches!(step, PlanStep::Disable)
                    || matches!(step,PlanStep::Patch(p) if p.priority.is_some_and(|p|observed.is_some_and(|o|p<o.channel.priority)))
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn peers_need_observation(&self, plan: &Plan) -> ControlResult<bool> {
        let c = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?;
        if self.publication.load(Ordering::SeqCst) != plan.epoch {
            return Err(error(ControlErrorCode::RevisionConflict));
        }
        Ok(plan.peers.iter().any(|p| {
            p.channel_id.is_some()
                && p.observation
                    .as_ref()
                    .is_none_or(|o| o.revision != p.revision || !fresh(&o.channel, plan.now))
                && c.observations
                    .get(&p.member_id)
                    .and_then(|o| o.problem(p.revision, plan.now))
                    .is_none()
        }))
    }
    async fn round(&self, stop: &AtomicBool) -> ControlResult<RoundReport> {
        let _lane = self
            .lane
            .try_lock()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let ids = {
            let core = self
                .core
                .lock()
                .map_err(|_| error(ControlErrorCode::Storage))?;
            let stopped: Vec<_> = core
                .source_disabled
                .iter()
                .filter_map(|(s, disabled)| disabled.then_some(*s))
                .collect();
            core.store.due_members_with_stops(now(), &stopped)?
        };
        let mut report = RoundReport::default();
        // Step P16: 有限到期集，每成员一项动作；单 lane 串行，不同时发相反管理写。
        for id in ids {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            report.visited += 1;
            match self.step(id, stop).await {
                Ok(true) => report.progressed += 1,
                Ok(false) => report.waiting += 1,
                Err(e) => {
                    if e.code == ControlErrorCode::Storage {
                        return Err(e);
                    }
                    if crate::catalog::confirmation::terminal(e.code) {
                        report.failed += 1;
                    } else {
                        report.waiting += 1;
                    }
                }
            }
        }
        self.token_round(stop, &mut report).await?;
        Ok(report)
    }
    pub async fn reconcile_once(self: &Arc<Self>) -> ControlResult<RoundReport> {
        let worker = self.clone();
        tokio::spawn(async move { worker.round(&AtomicBool::new(false)).await })
            .await
            .map_err(|_| error(ControlErrorCode::Storage))?
    }
    pub async fn run(self: &Arc<Self>, mut shutdown: watch::Receiver<bool>) -> ControlResult<()> {
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let stop = Arc::new(AtomicBool::new(false));
            let signal = stop.clone();
            let worker = self.clone();
            let mut task = tokio::spawn(async move { worker.round(&signal).await });
            tokio::select! {
                result=&mut task=>{accept_background_round(result.map_err(|_|error(ControlErrorCode::Storage))?)?;}
                _=shutdown.changed()=>{stop.store(true,Ordering::SeqCst);accept_background_round(task.await.map_err(|_|error(ControlErrorCode::Storage))?)?;if *shutdown.borrow() || shutdown.has_changed().is_err() {return Ok(());}}
            }
            let next = {
                let core = self.core.lock().map_err(|_| error(ControlErrorCode::Storage))?;
                let members = core.store.next_attempt_at()?;
                let tokens = if self.token_owners.is_some() && !self.policy_config.dry_run {
                    core.store.next_token_attempt_at()?
                } else { None };
                match (members, tokens) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            };
            let delay = next
                .map(|at| Duration::from_secs(at.saturating_sub(now()).max(1) as u64))
                .unwrap_or(Duration::from_secs(3600));
            tokio::select! {_=tokio::time::sleep(delay)=>{},_=self.changed.notified()=>{tokio::time::sleep(Duration::from_secs(1)).await;},_=shutdown.changed()=>{if *shutdown.borrow() || shutdown.has_changed().is_err() {return Ok(());}}}
        }
    }
}
fn accept_background_round(result: ControlResult<RoundReport>) -> ControlResult<()> {
    match result {
        Ok(_) => Ok(()),
        Err(e) if e.code == ControlErrorCode::QueueFull => Ok(()),
        Err(e) => Err(e),
    }
}
fn retryable(e: &ManagementError) -> bool {
    !crate::catalog::confirmation::terminal(code(e))
        && e.kind != ManagementErrorKind::RevisionConflict
        && e.retry_after.is_none_or(|d| d <= Duration::from_secs(60))
}
fn management_storage() -> ManagementError {
    ManagementError {
        kind: ManagementErrorKind::InvalidInput,
        status: None,
        retry_after: None,
        delivery: DeliveryEvidence::NotSent,
    }
}

#[cfg(test)]
mod tests;
