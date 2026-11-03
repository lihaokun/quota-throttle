//! 采集计划和安全结果类型；计划器没有网络或数据库副作用。
use crate::catalog::control::ControlErrorCode;
use crate::catalog::ledger::{
    LedgerError, LedgerState, ScanFault, ScanLane, ScanPhase, ScanWindow,
};
use crate::catalog::JSON_SAFE;
use crate::newapi::transport::{ManagementError, ManagementErrorKind};
use std::time::Duration;

pub(crate) const INITIAL_SECONDS: i64 = 31 * 86400;
pub(crate) const RECENT_SECONDS: i64 = 300;
pub(crate) const PAGE_SIZE: usize = 100;
pub(crate) const ROUND_PAGES: usize = 4;
pub(crate) const ROUND_INTERVAL: Duration = Duration::from_secs(10);
pub(crate) const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const PAGE_TIMEOUT: Duration = Duration::from_secs(25);
pub(crate) const ROUND_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const LEASE_SECONDS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogCapabilityIssue {
    LocalBinding,
    UnsupportedVersion,
    SqliteUnverified,
    Management(ManagementErrorKind),
    Expired,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CollectionError {
    Ledger(LedgerError),
    Control(ControlErrorCode),
    Capability(LogCapabilityIssue),
    Management(ManagementError),
    Clock,
    Busy,
    InvalidPage,
}
pub(crate) type CollectionResult<T> = Result<T, CollectionError>;
impl From<LedgerError> for CollectionError {
    fn from(e: LedgerError) -> Self {
        Self::Ledger(e)
    }
}
impl From<LogCapabilityIssue> for CollectionError {
    fn from(e: LogCapabilityIssue) -> Self {
        Self::Capability(e)
    }
}
impl From<ManagementError> for CollectionError {
    fn from(e: ManagementError) -> Self {
        Self::Management(e)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanAction {
    Skip,
    Begin(ScanWindow),
    Continue,
    RestartSameWindow,
    RetrySameWindow,
    Blocked(ScanFault),
}
pub(crate) fn lane_index(lane: ScanLane) -> usize {
    match lane {
        ScanLane::Recent => 0,
        ScanLane::History => 1,
    }
}
pub(crate) fn fatal_fault(fault: ScanFault) -> bool {
    matches!(
        fault,
        ScanFault::ConflictingIdentity | ScanFault::BindingChanged
    )
}
pub(crate) fn check_clock(
    state: &LedgerState,
    last: Option<i64>,
    now: i64,
) -> CollectionResult<()> {
    if !(2..=JSON_SAFE).contains(&now)
        || last.is_some_and(|t| now < t)
        || state.initial_requested_from.is_some_and(|from| from >= now)
        || state.scans.iter().any(|p| {
            p.window.is_some_and(|w| now < w.to) || p.last_observed_at.is_some_and(|t| now < t)
        })
    {
        return Err(CollectionError::Clock);
    }
    Ok(())
}
/// state 来自 CatalogStore 的严格读取；accepted 仅为当前会话接纳的持久代次。
pub(crate) fn plan_scan(
    state: &LedgerState,
    lane: ScanLane,
    accepted: Option<i64>,
    last: Option<i64>,
    now: i64,
) -> CollectionResult<ScanAction> {
    // Step B2.2: 先校时间/全局身份故障，再规划单 lane；不移动持久历史起点。
    check_clock(state, last, now)?;
    let from = state
        .initial_requested_from
        .ok_or(LedgerError::NotInitialized)?;
    if let Some(fault) = state
        .scans
        .iter()
        .filter_map(|p| p.fault)
        .find(|f| fatal_fault(*f))
    {
        return Ok(ScanAction::Blocked(fault));
    }
    let p = &state.scans[lane_index(lane)];
    Ok(match p.phase {
        ScanPhase::Scanning if accepted == Some(p.generation) => ScanAction::Continue,
        ScanPhase::Scanning => ScanAction::RestartSameWindow,
        ScanPhase::Stalled => match p.fault.ok_or(LedgerError::InvalidCheckpoint)? {
            ScanFault::Transport | ScanFault::Clock => ScanAction::RetrySameWindow,
            fault => ScanAction::Blocked(fault),
        },
        ScanPhase::Idle | ScanPhase::Scanned => {
            if p.window.is_some_and(|w| w.to >= now) {
                ScanAction::Skip
            } else {
                ScanAction::Begin(ScanWindow {
                    from: match lane {
                        ScanLane::Recent => from.max(now.saturating_sub(RECENT_SECONDS)),
                        ScanLane::History => from,
                    },
                    to: now,
                })
            }
        }
    })
}

#[cfg(test)]
mod tests;
