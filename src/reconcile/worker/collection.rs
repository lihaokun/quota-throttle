//! 唯一日志采集入口；每页原子提交，尚未接 CLI/现行面板。
#![allow(dead_code)]
use super::*;
use crate::boot::NewApiProcess;
use crate::catalog::ledger::*;
use crate::ledger::collection::*;
use crate::ledger::FactError;
use crate::newapi::logs::{
    observe_page_change, CheckedLogPage, LocalLogLease, LogPageQuery, PageObservation,
};
use crate::newapi::transport::RequestClass;
use std::future::Future;
use std::result::Result;
use tokio::time::Instant;

#[derive(Debug)]
pub(crate) struct CollectionReport {
    pub(crate) preview_only: bool,
    pub(crate) attempted: usize,
    pub(crate) committed: usize,
    pub(crate) inserted: usize,
    pub(crate) duplicates: usize,
    pub(crate) state: LedgerState,
    pub(crate) issue: Option<CollectionError>,
    pub(crate) pagination_changed: bool,
}
#[derive(Default)]
struct LaneSession {
    accepted: Option<i64>,
    previous: Option<PageObservation>,
}
pub(super) struct CollectorSession {
    lanes: [LaneSession; 2],
    next_lane: usize,
    last_wall: Option<i64>,
    lease: Option<Arc<LocalLogLease>>,
    force_probe: bool,
    next_due: Instant,
    issue: Option<CollectionError>,
    pagination_changed: bool,
}
impl Default for CollectorSession {
    fn default() -> Self {
        Self {
            lanes: Default::default(),
            next_lane: 0,
            last_wall: None,
            lease: None,
            force_probe: true,
            next_due: Instant::now(),
            issue: None,
            pagination_changed: false,
        }
    }
}
struct RoundGuard<'a> {
    session: tokio::sync::MutexGuard<'a, CollectorSession>,
    inflight: Option<usize>,
}
impl Drop for RoundGuard<'_> {
    fn drop(&mut self) {
        self.session.next_due = Instant::now() + ROUND_INTERVAL;
        if let Some(index) = self.inflight {
            self.session.lanes[index] = LaneSession::default();
            self.session.force_probe = true;
        }
    }
}
struct RunningGuard<'a>(&'a AtomicBool);
impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
// 小型 IO seam 仅用于可控故障测试；生产唯一实现始终调用真实本地 producer。
trait LogIo: Sync {
    fn check(&self) -> impl Future<Output = Result<LocalLogLease, LogCapabilityIssue>> + Send;
    fn recheck(
        &self,
        lease: &LocalLogLease,
    ) -> impl Future<Output = Result<(), LogCapabilityIssue>> + Send;
    fn read(
        &self,
        query: LogPageQuery,
        lease: &LocalLogLease,
        class: RequestClass,
    ) -> impl Future<Output = CollectionResult<CheckedLogPage>> + Send;
    fn now(&self) -> CollectionResult<i64>;
}
struct ManagedLogIo<'a> {
    client: &'a NewApiClient,
    process: &'a NewApiProcess,
}
impl LogIo for ManagedLogIo<'_> {
    async fn check(&self) -> Result<LocalLogLease, LogCapabilityIssue> {
        self.client.check_local_logs(self.process).await
    }
    async fn recheck(&self, lease: &LocalLogLease) -> Result<(), LogCapabilityIssue> {
        self.client.recheck_local_logs(self.process, lease).await
    }
    async fn read(
        &self,
        query: LogPageQuery,
        lease: &LocalLogLease,
        class: RequestClass,
    ) -> CollectionResult<CheckedLogPage> {
        self.client
            .read_log_page(query, lease, class, &|| true)
            .await
    }
    fn now(&self) -> CollectionResult<i64> {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .and_then(|t| i64::try_from(t.as_secs()).ok())
            .filter(|n| (2..=JSON_SAFE).contains(n))
            .ok_or(CollectionError::Clock)
    }
}
fn timeout_error() -> CollectionError {
    CollectionError::Management(ManagementError {
        kind: ManagementErrorKind::TransportUnknown,
        status: None,
        retry_after: None,
        delivery: DeliveryEvidence::Uncertain,
    })
}
fn storage_failure(error: &CollectionError) -> bool {
    matches!(
        error,
        CollectionError::Ledger(LedgerError::Catalog(_))
            | CollectionError::Control(ControlErrorCode::Storage)
    )
}
fn scan_fault(error: &CollectionError) -> Option<ScanFault> {
    match error {
        CollectionError::InvalidPage => Some(ScanFault::InvalidPage),
        CollectionError::Ledger(LedgerError::Fact(f)) => Some(match f {
            FactError::ConflictingIdentity => ScanFault::ConflictingIdentity,
            FactError::LimitExceeded | FactError::ArithmeticOverflow => ScanFault::ResourceLimit,
            _ => ScanFault::InvalidFact,
        }),
        CollectionError::Ledger(LedgerError::InvalidCheckpoint) => Some(ScanFault::ResourceLimit),
        CollectionError::Management(e) if e.status == Some(429) => Some(ScanFault::Transport),
        CollectionError::Management(e) => match e.kind {
            ManagementErrorKind::QueueFull
            | ManagementErrorKind::RevisionConflict
            | ManagementErrorKind::AuthFailed => None,
            ManagementErrorKind::InvalidInput | ManagementErrorKind::InvalidResponse => {
                Some(ScanFault::InvalidPage)
            }
            _ => Some(ScanFault::Transport),
        },
        _ => None,
    }
}
impl ReconcileWorker {
    fn ledger_state(&self) -> CollectionResult<LedgerState> {
        self.core
            .lock()
            .map_err(|_| CollectionError::Control(ControlErrorCode::Storage))?
            .store
            .load_ledger_state()
            .map_err(Into::into)
    }
    fn ledger_mutation<T>(
        &self,
        f: impl FnOnce(&mut CatalogStore) -> LedgerResult<T>,
    ) -> CollectionResult<T> {
        // Step B2.6: 保留内层精确错误；mutate 根据 Store usable 决定是否重新开 gate。
        self.mutate(None, |core| Ok(f(&mut core.store)))
            .map_err(|e| CollectionError::Control(e.code))?
            .map_err(Into::into)
    }
    fn scan_ticket(&self, lane: ScanLane) -> CollectionResult<ScanTicket> {
        self.core
            .lock()
            .map_err(|_| CollectionError::Control(ControlErrorCode::Storage))?
            .store
            .load_scan_ticket(lane)?
            .ok_or(LedgerError::RevisionConflict.into())
    }
    fn report(
        &self,
        preview_only: bool,
        session: Option<&CollectorSession>,
    ) -> CollectionResult<CollectionReport> {
        Ok(CollectionReport {
            preview_only,
            attempted: 0,
            committed: 0,
            inserted: 0,
            duplicates: 0,
            state: self.ledger_state()?,
            issue: session.and_then(|s| s.issue.clone()),
            pagination_changed: session.is_some_and(|s| s.pagination_changed),
        })
    }
    async fn log_lease(
        &self,
        io: &impl LogIo,
        session: &mut CollectorSession,
    ) -> CollectionResult<Arc<LocalLogLease>> {
        if !session.force_probe
            && session
                .lease
                .as_ref()
                .is_some_and(|l| l.check_time().is_ok())
        {
            return Ok(session.lease.as_ref().unwrap().clone());
        }
        let lease = Arc::new(io.check().await?);
        if session
            .lease
            .as_ref()
            .is_some_and(|old| !old.same_process(&lease))
        {
            session.lanes = Default::default();
        }
        session.lease = Some(lease.clone());
        session.force_probe = false;
        Ok(lease)
    }
    fn failed_page(
        &self,
        guard: &mut RoundGuard<'_>,
        ticket: &ScanTicket,
        error: CollectionError,
        now: i64,
    ) -> CollectionResult<()> {
        if storage_failure(&error) {
            return Err(error);
        }
        if let Some(fault) = scan_fault(&error) {
            // 写诊断前再验证时序；不为旧/未知提交补写一个假健康状态。
            let state = self.ledger_state()?;
            check_clock(&state, guard.session.last_wall, now)?;
            match self.ledger_mutation(|s| s.mark_scan_fault(ticket, fault, now)) {
                Err(CollectionError::Ledger(LedgerError::RevisionConflict)) => {}
                Err(e) => return Err(e),
                Ok(_) => {}
            }
        }
        guard.session.issue = Some(error);
        guard.session.lanes[lane_index(ticket.progress().lane)] = LaneSession::default();
        guard.inflight = None;
        Ok(())
    }
    async fn collection_round(
        &self,
        io: &impl LogIo,
        guard: &mut RoundGuard<'_>,
    ) -> CollectionResult<CollectionReport> {
        let mut state = self.ledger_state()?;
        let now = io.now()?;
        check_clock(&state, guard.session.last_wall, now)?;
        guard.session.last_wall = Some(now);
        if state.scans.iter().filter_map(|p| p.fault).any(fatal_fault) {
            return self.report(false, Some(&guard.session));
        }
        let lease = self.log_lease(io, &mut guard.session).await?;
        let now = io.now()?;
        check_clock(&state, guard.session.last_wall, now)?;
        guard.session.last_wall = Some(now);
        if state.initial_requested_from.is_none() {
            state =
                self.ledger_mutation(|s| s.initialize_ledger((now - INITIAL_SECONDS).max(0), now))?;
        }
        guard.session.issue = None;
        let mut report = self.report(false, Some(&guard.session))?;
        let mut inactive = [false; 2];
        while report.attempted < ROUND_PAGES && !inactive.iter().all(|v| *v) {
            let mut index = guard.session.next_lane;
            if inactive[index] {
                index = 1 - index;
            }
            guard.session.next_lane = 1 - index;
            let lane = if index == 0 {
                ScanLane::Recent
            } else {
                ScanLane::History
            };
            let now = io.now()?;
            let action = plan_scan(
                &state,
                lane,
                guard.session.lanes[index].accepted,
                guard.session.last_wall,
                now,
            )?;
            guard.session.last_wall = Some(now);
            let ticket = match action {
                ScanAction::Skip | ScanAction::Blocked(_) => {
                    inactive[index] = true;
                    continue;
                }
                ScanAction::Continue => self.scan_ticket(lane)?,
                ScanAction::RestartSameWindow => {
                    let old = self.scan_ticket(lane)?;
                    self.ledger_mutation(|s| s.restart_scan(&old, now))?
                }
                ScanAction::Begin(window) => self.ledger_mutation(|s| {
                    s.begin_scan(lane, state.scans[index].generation, window, now)
                })?,
                ScanAction::RetrySameWindow => self.ledger_mutation(|s| {
                    s.begin_scan(
                        lane,
                        state.scans[index].generation,
                        state.scans[index]
                            .window
                            .ok_or(LedgerError::InvalidCheckpoint)?,
                        now,
                    )
                })?,
            };
            if guard.session.lanes[index].accepted != Some(ticket.progress().generation) {
                guard.session.lanes[index].previous = None;
            }
            guard.session.lanes[index].accepted = Some(ticket.progress().generation);
            guard.inflight = Some(index);
            if let Err(e) = io.recheck(&lease).await {
                guard.session.force_probe = true;
                guard.session.lanes = Default::default();
                return Err(e.into());
            }
            let query = match LogPageQuery::new(
                ticket
                    .progress()
                    .window
                    .ok_or(LedgerError::InvalidCheckpoint)?,
                ticket.progress().next_page,
            ) {
                Ok(query) => query,
                Err(error) => {
                    let observed_at = io.now()?;
                    self.failed_page(guard, &ticket, error, observed_at)?;
                    guard.session.last_wall = Some(observed_at);
                    inactive[index] = true;
                    state = self.ledger_state()?;
                    continue;
                }
            };
            report.attempted += 1;
            let class = if index == 0 {
                RequestClass::Observe
            } else {
                RequestClass::Backfill
            };
            let fetched = tokio::time::timeout(PAGE_TIMEOUT, io.read(query, &lease, class))
                .await
                .unwrap_or_else(|_| Err(timeout_error()));
            // 原始页收到后仍须后验；不能只在发送前证明绑定。
            if let Err(e) = io.recheck(&lease).await {
                guard.session.force_probe = true;
                guard.session.lanes = Default::default();
                return Err(e.into());
            }
            let now = io.now()?;
            check_clock(&state, guard.session.last_wall, now)?;
            guard.session.last_wall = Some(now);
            let page = match fetched {
                Ok(page) => page,
                Err(e) => {
                    let stop = matches!(&e,CollectionError::Management(m) if m.status==Some(429)||m.kind==ManagementErrorKind::AuthFailed)
                        || matches!(e, CollectionError::Capability(_));
                    self.failed_page(guard, &ticket, e, now)?;
                    inactive[index] = true;
                    state = self.ledger_state()?;
                    if stop {
                        break;
                    } else {
                        continue;
                    }
                }
            };
            let changed = observe_page_change(guard.session.lanes[index].previous, &page);
            match self.ledger_mutation(|s| s.commit_fact_page(&ticket, &page.rows, page.end, now)) {
                Ok(committed) => {
                    report.committed += 1;
                    report.inserted += committed.inserted;
                    report.duplicates += committed.duplicates;
                    guard.session.lanes[index].previous = Some(page.observation());
                    guard.session.pagination_changed |= changed;
                    guard.inflight = None;
                    inactive[index] = page.end == PageEnd::ReachedEnd;
                }
                Err(e) => {
                    self.failed_page(guard, &ticket, e, now)?;
                    inactive[index] = true;
                }
            }
            state = self.ledger_state()?;
            if state.scans.iter().filter_map(|p| p.fault).any(fatal_fault) {
                break;
            }
        }
        report.state = self.ledger_state()?;
        report.issue = guard.session.issue.clone();
        report.pagination_changed = guard.session.pagination_changed;
        Ok(report)
    }
    async fn collect_with(&self, io: &impl LogIo) -> CollectionResult<CollectionReport> {
        if self.policy_config.dry_run {
            return self.report(true, None);
        }
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| CollectionError::Busy)?;
        let session = self
            .collector
            .try_lock()
            .map_err(|_| CollectionError::Busy)?;
        if Instant::now() < session.next_due {
            return self.report(false, Some(&session));
        }
        let mut guard = RoundGuard {
            session,
            inflight: None,
        };
        let result = tokio::time::timeout(ROUND_TIMEOUT, self.collection_round(io, &mut guard))
            .await
            .unwrap_or_else(|_| Err(timeout_error()));
        if let Err(e) = &result {
            guard.session.issue = Some(e.clone());
        }
        result
    }
    /// process 沿用 worker 生命周期的固定托管配置；不支持热切换日志数据集。
    pub(crate) async fn collect_facts_once(
        &self,
        process: &NewApiProcess,
    ) -> CollectionResult<CollectionReport> {
        self.collect_with(&ManagedLogIo {
            client: &self.client,
            process,
        })
        .await
    }
    pub(crate) async fn retry_fact_lane(
        &self,
        lane: ScanLane,
        generation: i64,
        process: &NewApiProcess,
    ) -> CollectionResult<ScanProgress> {
        self.retry_fact_with(
            lane,
            generation,
            &ManagedLogIo {
                client: &self.client,
                process,
            },
        )
        .await
    }
    async fn retry_fact_with(
        &self,
        lane: ScanLane,
        generation: i64,
        io: &impl LogIo,
    ) -> CollectionResult<ScanProgress> {
        if self.policy_config.dry_run {
            return Err(CollectionError::Control(ControlErrorCode::NotReady));
        }
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| CollectionError::Busy)?;
        let session = self
            .collector
            .try_lock()
            .map_err(|_| CollectionError::Busy)?;
        let mut guard = RoundGuard {
            session,
            inflight: None,
        };
        let state = self.ledger_state()?;
        let p = &state.scans[lane_index(lane)];
        if state.scans.iter().filter_map(|p| p.fault).any(fatal_fault)
            || p.phase != ScanPhase::Stalled
            || p.generation != generation
            || !matches!(
                p.fault,
                Some(
                    ScanFault::InvalidPage
                        | ScanFault::InvalidFact
                        | ScanFault::ResourceLimit
                        | ScanFault::HistoryUnavailable
                )
            )
        {
            return Err(LedgerError::RevisionConflict.into());
        }
        guard.session.force_probe = true;
        self.log_lease(io, &mut guard.session).await?;
        let now = io.now()?;
        check_clock(&state, guard.session.last_wall, now)?;
        let ticket = self.ledger_mutation(|s| {
            s.begin_scan(
                lane,
                generation,
                p.window.ok_or(LedgerError::InvalidCheckpoint)?,
                now,
            )
        })?;
        guard.session.lanes[lane_index(lane)] = LaneSession {
            accepted: Some(ticket.progress().generation),
            previous: None,
        };
        guard.session.last_wall = Some(now);
        Ok(ticket.progress().clone())
    }
    pub(crate) async fn run_fact_collection(
        &self,
        process: &NewApiProcess,
        shutdown: watch::Receiver<bool>,
    ) -> CollectionResult<()> {
        self.run_collection_with(
            &ManagedLogIo {
                client: &self.client,
                process,
            },
            shutdown,
        )
        .await
    }
    async fn run_collection_with(
        &self,
        io: &impl LogIo,
        mut shutdown: watch::Receiver<bool>,
    ) -> CollectionResult<()> {
        // Step B2.7: 显式启动、可取消、无脱离生命周期任务；生产 CLI 尚未调用。
        if *shutdown.borrow() {
            return Ok(());
        }
        self.collection_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| CollectionError::Busy)?;
        let _running = RunningGuard(&self.collection_running);
        {
            let mut session = self
                .collector
                .try_lock()
                .map_err(|_| CollectionError::Busy)?;
            session.lanes = Default::default();
            session.force_probe = true;
        }
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            tokio::select! {
                biased;
                changed=shutdown.changed()=>{if changed.is_err()||*shutdown.borrow(){return Ok(());}continue;},
                result=self.collect_with(io)=>{if let Err(e)=result{if storage_failure(&e){return Err(e);}}},
            }
            let next = self
                .collector
                .try_lock()
                .map(|s| s.next_due)
                .unwrap_or_else(|_| Instant::now() + ROUND_INTERVAL)
                .max(Instant::now() + ROUND_INTERVAL);
            tokio::select! {changed=shutdown.changed()=>{if changed.is_err()||*shutdown.borrow(){return Ok(());}},_=tokio::time::sleep_until(next)=>{}}
        }
    }
}

#[cfg(test)]
mod tests;
