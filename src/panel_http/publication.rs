use super::HttpErrorCode as Code;
use crate::catalog::JSON_SAFE;
use crate::reconcile::worker::panel::PanelSnapshot;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::{JoinError, JoinHandle};
use tokio::time::Instant;

pub(super) const MAX_RESPONSE: usize = 32 * 1024 * 1024;
pub(super) const PUBLICATION_TTL: Duration = Duration::from_secs(10);
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(super) struct PublishError;
pub(super) type SnapshotReader = Arc<dyn Fn() -> Result<PanelSnapshot, PublishError> + Send + Sync>;
pub(super) fn wall_ms() -> Option<i64> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    (value <= JSON_SAFE as u128).then_some(value as i64)
}
struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| io::Error::other("panel response limit"))?;
        if next > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(next)
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn encode_panel(snapshot: &PanelSnapshot) -> Result<Arc<[u8]>, PublishError> {
    // Step H1.4: 只编码A1 DTO；先限制每次写入，再冻结完整bytes供所有请求共享。
    encode_bounded(snapshot)
}
pub(super) fn encode_bounded<T: serde::Serialize>(value: &T) -> Result<Arc<[u8]>, PublishError> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit: MAX_RESPONSE,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| PublishError)?;
    Ok(writer.bytes.into())
}
pub(super) struct PublishedPanel {
    pub(super) bytes: Arc<[u8]>,
    pub(super) captured_at_ms: i64,
    pub(super) started_at: Instant,
}
impl PublishedPanel {
    fn age(&self, now: Instant, wall: Option<i64>) -> Option<u64> {
        let wall = wall.filter(|v| (0..=JSON_SAFE).contains(v))?;
        if !(0..=JSON_SAFE).contains(&self.captured_at_ms) || wall < self.captured_at_ms {
            return None;
        }
        let elapsed = now.checked_duration_since(self.started_at)?;
        let millis = elapsed.as_nanos().div_ceil(1_000_000);
        let age = millis.max((wall - self.captured_at_ms) as u128);
        (age < PUBLICATION_TTL.as_millis()).then_some(age as u64)
    }
}
#[derive(Default)]
pub(super) struct PanelCache {
    entry: Mutex<Option<PublishedPanel>>,
}
impl PanelCache {
    pub(super) fn replace(&self, next: Option<PublishedPanel>) -> Result<(), Code> {
        let old = {
            let mut guard = self.entry.lock().map_err(|_| Code::Internal)?;
            std::mem::replace(&mut *guard, next)
        };
        drop(old);
        Ok(())
    }
    pub(super) fn get(&self, now: Instant, wall: Option<i64>) -> Result<(Arc<[u8]>, u64), Code> {
        let mut guard = self.entry.try_lock().map_err(|e| match e {
            TryLockError::WouldBlock => Code::Overloaded,
            TryLockError::Poisoned(_) => Code::Internal,
        })?;
        let entry = guard.as_ref().ok_or(Code::SnapshotUnavailable)?;
        if let Some(age) = entry.age(now, wall) {
            return Ok((entry.bytes.clone(), age));
        }
        let old = guard.take();
        drop(guard);
        drop(old);
        Err(Code::SnapshotUnavailable)
    }
}
pub(super) struct ActivePublication {
    pub(super) task: JoinHandle<Result<PublishedPanel, PublishError>>,
    pub(super) started_at: Instant,
    pub(super) expired: bool,
}
pub(super) fn start_publication(reader: SnapshotReader, started_at: Instant) -> ActivePublication {
    // Step H1.5: 唯一supervisor保留handle；闭包只能返回数据，没有cache发布能力。
    let task = tokio::task::spawn_blocking(move || {
        let snapshot = reader()?;
        let bytes = encode_panel(&snapshot)?;
        Ok(PublishedPanel {
            bytes,
            captured_at_ms: snapshot.captured_at_ms,
            started_at,
        })
    });
    ActivePublication {
        task,
        started_at,
        expired: false,
    }
}
pub(super) fn expire_publication(
    active: &mut ActivePublication,
    cache: &PanelCache,
) -> Result<(), Code> {
    active.expired = true;
    cache.replace(None)
}
pub(super) fn finish_publication(
    active: ActivePublication,
    result: Result<Result<PublishedPanel, PublishError>, JoinError>,
    cache: &PanelCache,
    now: Instant,
    wall: Option<i64>,
    shutting_down: bool,
) -> Result<(), Code> {
    let entry = match result {
        Err(_) => {
            cache.replace(None)?;
            return Err(Code::Internal);
        }
        Ok(Err(_)) => None,
        Ok(Ok(entry)) if !active.expired && !shutting_down && entry.age(now, wall).is_some() => {
            Some(entry)
        }
        Ok(Ok(_)) => None,
    };
    cache.replace(entry)
}

#[cfg(test)]
#[path = "tests/publication.rs"]
mod tests;
