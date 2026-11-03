//! 查询不占管理命令槽或长 Core；阻塞任务实际退出后才归还查询槽。
#![allow(dead_code)]
use super::*;
use crate::catalog::ledger::query::UsageReadTarget;
use crate::ledger::analytics::*;
use std::sync::TryLockError;
use std::time::Instant;

struct CancelQuery(Arc<QueryControl>);
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttributionReport {
    pub(crate) preview_only: bool,
    pub(crate) checked: usize,
    pub(crate) changed: usize,
    pub(crate) ledger_revision: i64,
    pub(crate) pass_finished: bool,
}
impl Drop for CancelQuery {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl ReconcileWorker {
    pub(crate) async fn repair_attribution_once(&self) -> AnalyticsResult<AttributionReport> {
        // Step B3.7: 一个命令槽和一批；只有确认提交后才发布内存游标。
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| AnalyticsError::Control(ControlErrorCode::QueueFull))?;
        let _automatic = self
            .automatic_updates
            .try_read()
            .map_err(|_| AnalyticsError::Busy)?;
        let mut cursor = self
            .repair_cursor
            .try_lock()
            .map_err(|_| AnalyticsError::Busy)?;
        if self.policy_config.dry_run {
            let core = self
                .core
                .lock()
                .map_err(|_| AnalyticsError::Control(ControlErrorCode::Storage))?;
            let state = core.store.load_ledger_state()?;
            return Ok(AttributionReport {
                preview_only: true,
                checked: 0,
                changed: 0,
                ledger_revision: state.revision,
                pass_finished: false,
            });
        }
        let batch = self
            .mutate(None, |core| {
                Ok((|| -> crate::catalog::ledger::LedgerResult<_> {
                    let ticket = match cursor.as_ref() {
                        Some(c) => c.clone(),
                        None => core.store.begin_attribution_pass()?,
                    };
                    core.store.reattribute_batch(&ticket)
                })())
            })
            .map_err(|e| AnalyticsError::Control(e.code))??;
        let result = AttributionReport {
            preview_only: false,
            checked: batch.checked,
            changed: batch.changed,
            ledger_revision: batch.ledger_revision,
            pass_finished: batch.next.is_none(),
        };
        *cursor = batch.next;
        Ok(result)
    }
    pub(crate) async fn query_usage(
        self: &Arc<Self>,
        query: UsageQuery,
    ) -> AnalyticsResult<UsageSnapshot> {
        self.query_usage_with(query, QUERY_TIMEOUT, |target, query, control| {
            target.query(query, &control)
        })
        .await
    }
    async fn query_usage_with<F>(
        self: &Arc<Self>,
        query: UsageQuery,
        timeout: Duration,
        run: F,
    ) -> AnalyticsResult<UsageSnapshot>
    where
        F: FnOnce(UsageReadTarget, UsageQuery, Arc<QueryControl>) -> AnalyticsResult<UsageSnapshot>
            + Send
            + 'static,
    {
        // Step B3.4: 单槽随真实工作存活；超时/取消只中断本查询，不取消写连接。
        let permit = self
            .query_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| AnalyticsError::Busy)?;
        let control = Arc::new(QueryControl::new(Instant::now() + timeout));
        let _cancel = CancelQuery(control.clone());
        tokio::time::timeout(
            control.deadline().saturating_duration_since(Instant::now()),
            async {
                let target = self
                    .query_core(&control, |core| core.store.usage_read_target())
                    .await?;
                let task_control = control.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    // 早退也先销毁 target，再归还槽；不能留在 closure capture 中延后析构。
                    let read_target = target;
                    task_control.check()?;
                    run(read_target, query, task_control)
                })
                .await
                .map_err(|_| AnalyticsError::Control(ControlErrorCode::Storage))??;
                control.check()?;
                self.query_core(&control, |_| Ok(())).await?;
                control.check()?;
                Ok(result)
            },
        )
        .await
        .map_err(|_| AnalyticsError::TimedOut)?
    }
    async fn query_core<T>(
        &self,
        control: &QueryControl,
        read: impl Fn(&Core) -> AnalyticsResult<T>,
    ) -> AnalyticsResult<T> {
        loop {
            control.check()?;
            {
                match self.core.try_lock() {
                    Ok(core) => {
                        core.store.usable()?;
                        return read(&core);
                    }
                    Err(TryLockError::Poisoned(_)) => {
                        return Err(AnalyticsError::Control(ControlErrorCode::Storage))
                    }
                    Err(TryLockError::WouldBlock) => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

#[cfg(test)]
mod tests;
