//! 现有本地托管的只读预检；没有实例登记、数据库指纹或远程管理协议。
use super::managed_channels::NativeRecoveryCheck;
use super::transport::{
    endpoint_is_loopback, ManagementErrorKind, ManagementRequest, RequestClass, RequestSemantics,
    LIST_BODY_LIMIT, SINGLE_BODY_LIMIT,
};
use super::NewApiClient;
use crate::boot::NewApiProcess;
use crate::catalog::JSON_SAFE;
use crate::reconcile::planner::{BackendReadiness, SUPPORTED_BACKEND_VERSION};
use reqwest::{Method, Url};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime};

const LEASE_SECONDS: i64 = 60;
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const KNOWN_LEGACY_SHA256: &str =
    "90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendIssue {
    LocalBinding,
    UnsupportedVersion,
    RecoveryEnabled,
    RecoveryUnknown,
    Management(ManagementErrorKind),
    Expired,
}
pub struct BackendCheck {
    readiness: BackendReadiness,
    issue: Option<BackendIssue>,
}
impl BackendCheck {
    pub fn readiness(&self) -> BackendReadiness {
        self.readiness
    }
    pub fn issue(&self) -> Option<BackendIssue> {
        self.issue
    }
    fn failed(issue: BackendIssue) -> Self {
        Self {
            readiness: unavailable(),
            issue: Some(issue),
        }
    }
}
pub(crate) fn unavailable() -> BackendReadiness {
    BackendReadiness::from_preflight("", NativeRecoveryCheck::Unknown, false, 0, 0)
}
pub(super) fn utc_now() -> Result<i64, BackendIssue> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .filter(|n| *n <= JSON_SAFE)
        .ok_or(BackendIssue::Expired)
}
fn ready(start: i64, now: i64) -> Result<BackendCheck, BackendIssue> {
    let end = start
        .checked_add(LEASE_SECONDS)
        .filter(|end| *end <= JSON_SAFE)
        .ok_or(BackendIssue::Expired)?;
    if start < 0 || now < start || now >= end {
        return Err(BackendIssue::Expired);
    }
    Ok(BackendCheck {
        readiness: BackendReadiness::from_preflight(
            SUPPORTED_BACKEND_VERSION,
            NativeRecoveryCheck::Disabled,
            true,
            start,
            end,
        ),
        issue: None,
    })
}
fn read_small(path: &Path, limit: u64) -> Result<String, BackendIssue> {
    if !std::fs::metadata(path)
        .map_err(|_| BackendIssue::LocalBinding)?
        .is_file()
    {
        return Err(BackendIssue::LocalBinding);
    }
    let file = std::fs::File::open(path).map_err(|_| BackendIssue::LocalBinding)?;
    if !file
        .metadata()
        .map_err(|_| BackendIssue::LocalBinding)?
        .is_file()
    {
        return Err(BackendIssue::LocalBinding);
    }
    let mut value = String::new();
    file.take(limit + 1)
        .read_to_string(&mut value)
        .map_err(|_| BackendIssue::LocalBinding)?;
    if value.len() as u64 > limit {
        return Err(BackendIssue::LocalBinding);
    }
    Ok(value)
}
fn parse_pid(value: &str) -> Result<u32, BackendIssue> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(BackendIssue::LocalBinding);
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or(BackendIssue::LocalBinding)
}
pub(super) async fn read_pid(process: &NewApiProcess) -> Result<u32, BackendIssue> {
    let path = process.data_dir.join("new-api.pid");
    tokio::task::spawn_blocking(move || parse_pid(&read_small(&path, 32)?))
        .await
        .map_err(|_| BackendIssue::LocalBinding)?
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ProcessSnapshot {
    pub(super) pid: u32,
    pub(super) start: u64,
}
fn process_start(stat: &str, pid: u32) -> Result<u64, BackendIssue> {
    let (prefix, tail) = stat.rsplit_once(") ").ok_or(BackendIssue::LocalBinding)?;
    if !prefix.starts_with(&format!("{pid} (")) {
        return Err(BackendIssue::LocalBinding);
    }
    let fields: Vec<_> = tail.split_whitespace().collect();
    if !matches!(fields.first().copied(), Some("R" | "S" | "D" | "I")) {
        return Err(BackendIssue::LocalBinding);
    }
    fields
        .get(19)
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or(BackendIssue::LocalBinding)
}
fn owns_listener(text: &str, pid: u32) -> bool {
    let lines: Vec<_> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines.len() == 1
        && lines[0].split_whitespace().next() == Some("LISTEN")
        && lines[0].matches("pid=").count() == 1
        && lines[0].contains(&format!("pid={pid},"))
}
pub(super) async fn process_snapshot(
    process: &NewApiProcess,
    pid: u32,
) -> Result<ProcessSnapshot, BackendIssue> {
    if !cfg!(target_os = "linux") {
        return Err(BackendIssue::LocalBinding);
    }
    let binary = process.binary.clone();
    let directory = process.data_dir.clone();
    let start = tokio::task::spawn_blocking(move || {
        let proc_dir = std::path::PathBuf::from(format!("/proc/{pid}"));
        let canonical = |p: &Path| std::fs::canonicalize(p).map_err(|_| BackendIssue::LocalBinding);
        if canonical(&proc_dir.join("exe"))? != canonical(&binary)?
            || canonical(&proc_dir.join("cwd"))? != canonical(&directory)?
        {
            return Err(BackendIssue::LocalBinding);
        }
        process_start(&read_small(&proc_dir.join("stat"), 8192)?, pid)
    })
    .await
    .map_err(|_| BackendIssue::LocalBinding)??;
    let mut child = tokio::process::Command::new("ss")
        .args(["-H", "-ltnp", &format!("sport = :{}", process.port)])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| BackendIssue::LocalBinding)?;
    let reading = async {
        use tokio::io::AsyncReadExt;
        let stdout = child.stdout.take().ok_or(BackendIssue::LocalBinding)?;
        let mut output = Vec::new();
        stdout
            .take((SINGLE_BODY_LIMIT + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|_| BackendIssue::LocalBinding)?;
        if output.len() > SINGLE_BODY_LIMIT
            || !child
                .wait()
                .await
                .map_err(|_| BackendIssue::LocalBinding)?
                .success()
            || !std::str::from_utf8(&output).is_ok_and(|s| owns_listener(s, pid))
        {
            return Err(BackendIssue::LocalBinding);
        }
        Ok::<_, BackendIssue>(())
    };
    tokio::time::timeout(Duration::from_secs(3), reading)
        .await
        .map_err(|_| BackendIssue::LocalBinding)??;
    Ok(ProcessSnapshot { pid, start })
}
pub(super) async fn known_legacy_build(pid: u32) -> bool {
    tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
        let mut file = std::fs::File::open(format!("/proc/{pid}/exe"))?;
        let limit = 256 * 1024 * 1024;
        if !file.metadata()?.is_file() || file.metadata()?.len() > limit {
            return Ok(false);
        }
        let mut hash = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        let mut size = 0;
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > limit {
                return Ok(false);
            }
            hash.update(&buf[..n]);
        }
        Ok(format!("{:x}", hash.finalize()) == KNOWN_LEGACY_SHA256)
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or(false)
}
fn page_capability(body: &Value) -> bool {
    let data = &body["data"];
    let Some(total) = data["total"]
        .as_i64()
        .filter(|n| (0..=JSON_SAFE).contains(n))
    else {
        return false;
    };
    let Some(items) = data["items"].as_array() else {
        return false;
    };
    data["page"] == 1
        && data["page_size"] == 1
        && items.len() == usize::from(total > 0)
        && items.iter().all(|item| {
            item["id"]
                .as_i64()
                .is_some_and(|id| (1..=JSON_SAFE).contains(&id))
        })
}
impl NewApiClient {
    pub(super) fn local_target(&self, process: &NewApiProcess) -> Result<(), BackendIssue> {
        let url = Url::parse(&process.base_url).map_err(|_| BackendIssue::LocalBinding)?;
        if process.configured_version() != SUPPORTED_BACKEND_VERSION {
            return Err(BackendIssue::UnsupportedVersion);
        }
        if url.as_str().trim_end_matches('/') != self.transport.base_url()
            || url.scheme() != "http"
            || !endpoint_is_loopback(&url)
            || url.path() != "/"
            || process.port == 0
            || url.port_or_known_default() != Some(process.port)
        {
            return Err(BackendIssue::LocalBinding);
        }
        Ok(())
    }
    async fn preflight_read(&self, path: &str, limit: usize) -> Result<Value, BackendIssue> {
        let request =
            ManagementRequest::new(Method::GET, path, None, RequestSemantics::Read, limit)
                .map_err(|e| BackendIssue::Management(e.kind))?;
        self.transport
            .send(request, RequestClass::Control)
            .await
            .and_then(|r| r.envelope())
            .map_err(|e| BackendIssue::Management(e.kind))
    }
    async fn probe_pid(
        &self,
        process: &NewApiProcess,
        pid: u32,
        start: i64,
    ) -> Result<BackendCheck, BackendIssue> {
        self.local_target(process)?;
        let before = process_snapshot(process, pid).await?;
        let status = self
            .preflight_read("/api/status", SINGLE_BODY_LIMIT)
            .await?;
        let version = status["data"]["version"]
            .as_str()
            .ok_or(BackendIssue::Management(
                ManagementErrorKind::InvalidResponse,
            ))?;
        if version != SUPPORTED_BACKEND_VERSION
            && !(version == "v0.0.0" && known_legacy_build(pid).await)
        {
            return Err(BackendIssue::UnsupportedVersion);
        }
        match self
            .managed_channels()
            .check_native_recovery()
            .await
            .map_err(|e| BackendIssue::Management(e.kind))?
        {
            NativeRecoveryCheck::Disabled => {}
            NativeRecoveryCheck::Enabled => return Err(BackendIssue::RecoveryEnabled),
            NativeRecoveryCheck::Unknown => return Err(BackendIssue::RecoveryUnknown),
        }
        let page = self
            .preflight_read(
                "/api/channel/?p=1&page_size=1&sort_by=id&sort_order=asc",
                LIST_BODY_LIMIT,
            )
            .await?;
        if !page_capability(&page) {
            return Err(BackendIssue::Management(
                ManagementErrorKind::InvalidResponse,
            ));
        }
        if process_snapshot(process, pid).await? != before {
            return Err(BackendIssue::LocalBinding);
        }
        ready(start, utc_now()?)
    }
    pub async fn check_local_backend(&self, process: &NewApiProcess) -> BackendCheck {
        let check = async {
            self.local_target(process)?;
            let start = utc_now()?;
            let pid = read_pid(process).await?;
            let result = self.probe_pid(process, pid, start).await?;
            if read_pid(process).await? != pid {
                return Err(BackendIssue::LocalBinding);
            }
            Ok(result)
        };
        match tokio::time::timeout(CHECK_TIMEOUT, check).await {
            Ok(Ok(result)) => result,
            Ok(Err(issue)) => BackendCheck::failed(issue),
            Err(_) => BackendCheck::failed(BackendIssue::Expired),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
