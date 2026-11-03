//! 日志读取的本地进程证据；与路由 RecoveryDisabled 许可严格分离。
use super::*;
use crate::boot::NewApiProcess;
use crate::ledger::collection::{LogCapabilityIssue, CHECK_TIMEOUT, LEASE_SECONDS};
use crate::newapi::preflight::{self, BackendIssue, ProcessSnapshot};
use crate::newapi::transport::SINGLE_BODY_LIMIT;
use crate::reconcile::planner::SUPPORTED_BACKEND_VERSION;
use std::io::Read;
use tokio::time::Instant;

const ENV_LIMIT: usize = 1024 * 1024;
pub(crate) struct LocalLogLease {
    process: ProcessSnapshot,
    started: i64,
    expires: i64,
    deadline: Instant,
}
impl LocalLogLease {
    pub(crate) fn check_time(&self) -> Result<(), LogCapabilityIssue> {
        let now = preflight::utc_now().map_err(issue)?;
        self.check_time_at(now, Instant::now())
    }
    fn check_time_at(&self, now: i64, instant: Instant) -> Result<(), LogCapabilityIssue> {
        if now < self.started || now >= self.expires || instant >= self.deadline {
            return Err(LogCapabilityIssue::Expired);
        }
        Ok(())
    }
    pub(crate) fn same_process(&self, other: &Self) -> bool {
        self.process == other.process
    }
    #[cfg(test)]
    pub(crate) fn test_fixture() -> Self {
        let started = preflight::utc_now().unwrap();
        Self {
            process: ProcessSnapshot {
                pid: std::process::id(),
                start: 1,
            },
            started,
            expires: started + LEASE_SECONDS,
            deadline: Instant::now() + std::time::Duration::from_secs(LEASE_SECONDS as u64),
        }
    }
}
fn issue(e: BackendIssue) -> LogCapabilityIssue {
    match e {
        BackendIssue::UnsupportedVersion => LogCapabilityIssue::UnsupportedVersion,
        BackendIssue::Management(kind) => LogCapabilityIssue::Management(kind),
        BackendIssue::Expired => LogCapabilityIssue::Expired,
        _ => LogCapabilityIssue::LocalBinding,
    }
}
fn validate_environment(raw: &[u8]) -> Result<(), LogCapabilityIssue> {
    if raw.len() > ENV_LIMIT || !raw.ends_with(&[0]) {
        return Err(LogCapabilityIssue::SqliteUnverified);
    }
    let mut seen = [false; 2];
    for entry in raw.split(|b| *b == 0) {
        for (i, key) in [b"SQL_DSN=".as_slice(), b"LOG_SQL_DSN=".as_slice()]
            .iter()
            .enumerate()
        {
            if let Some(value) = entry.strip_prefix(*key) {
                if seen[i] || value != b"local" {
                    return Err(LogCapabilityIssue::SqliteUnverified);
                }
                seen[i] = true;
            }
        }
    }
    if seen == [true, true] {
        Ok(())
    } else {
        Err(LogCapabilityIssue::SqliteUnverified)
    }
}
async fn sqlite_startup_evidence(pid: u32) -> Result<(), LogCapabilityIssue> {
    tokio::task::spawn_blocking(move || {
        let mut raw = Vec::new();
        std::fs::File::open(format!("/proc/{pid}/environ"))
            .and_then(|f| f.take((ENV_LIMIT + 1) as u64).read_to_end(&mut raw))
            .map_err(|_| LogCapabilityIssue::SqliteUnverified)?;
        validate_environment(&raw)
    })
    .await
    .map_err(|_| LogCapabilityIssue::SqliteUnverified)?
}
impl NewApiClient {
    pub(crate) async fn check_local_logs(
        &self,
        process: &NewApiProcess,
    ) -> Result<LocalLogLease, LogCapabilityIssue> {
        // Step B2.4: 真实进程/启动分支/版本前后核对；不依赖路由自动恢复开关。
        let check = async {
            self.local_target(process).map_err(issue)?;
            let start_instant = Instant::now();
            let started = preflight::utc_now().map_err(issue)?;
            let expires = started
                .checked_add(LEASE_SECONDS)
                .filter(|n| *n <= JSON_SAFE)
                .ok_or(LogCapabilityIssue::Expired)?;
            let pid = preflight::read_pid(process).await.map_err(issue)?;
            let before = preflight::process_snapshot(process, pid)
                .await
                .map_err(issue)?;
            sqlite_startup_evidence(pid).await?;
            let request = ManagementRequest::new(
                Method::GET,
                "/api/status",
                None,
                RequestSemantics::Read,
                SINGLE_BODY_LIMIT,
            )
            .map_err(|e| LogCapabilityIssue::Management(e.kind))?;
            let response = self
                .transport
                .send(request, RequestClass::Observe)
                .await
                .and_then(|r| r.envelope())
                .map_err(|e| LogCapabilityIssue::Management(e.kind))?;
            let version = response["data"]["version"]
                .as_str()
                .ok_or(LogCapabilityIssue::UnsupportedVersion)?;
            if version != SUPPORTED_BACKEND_VERSION
                && !(version == "v0.0.0" && preflight::known_legacy_build(pid).await)
            {
                return Err(LogCapabilityIssue::UnsupportedVersion);
            }
            if preflight::read_pid(process).await.map_err(issue)? != pid
                || preflight::process_snapshot(process, pid)
                    .await
                    .map_err(issue)?
                    != before
            {
                return Err(LogCapabilityIssue::LocalBinding);
            }
            let lease = LocalLogLease {
                process: before,
                started,
                expires,
                deadline: start_instant + std::time::Duration::from_secs(LEASE_SECONDS as u64),
            };
            lease.check_time()?;
            Ok(lease)
        };
        tokio::time::timeout(CHECK_TIMEOUT, check)
            .await
            .map_err(|_| LogCapabilityIssue::Expired)?
    }
    pub(crate) async fn recheck_local_logs(
        &self,
        process: &NewApiProcess,
        lease: &LocalLogLease,
    ) -> Result<(), LogCapabilityIssue> {
        lease.check_time()?;
        self.local_target(process).map_err(issue)?;
        let pid = preflight::read_pid(process).await.map_err(issue)?;
        if pid != lease.process.pid
            || preflight::process_snapshot(process, pid)
                .await
                .map_err(issue)?
                != lease.process
        {
            return Err(LogCapabilityIssue::LocalBinding);
        }
        if preflight::read_pid(process).await.map_err(issue)? != pid {
            return Err(LogCapabilityIssue::LocalBinding);
        }
        lease.check_time()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod producer_tests;
#[cfg(test)]
mod tests;
