//! 来源单飞采集与 GLM 批次发布；等待网络不持 Core，取消不发布半轮。
#![cfg_attr(not(test), allow(dead_code))]
use super::*;
use crate::catalog::glm_publication::PolicyRefreshReport;
use crate::source_policy::probe::ProbeCycle;
use crate::source_policy::PolicyError;

fn current_ms() -> ControlResult<i64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .filter(|v| (0..=JSON_SAFE).contains(v))
        .ok_or_else(|| error(ControlErrorCode::InvalidInput))
}

impl ReconcileWorker {
    pub(crate) async fn refresh_glm_policy(&self) -> ControlResult<PolicyRefreshReport> {
        // Step C2: 单槽/单飞采集整轮；取消释放锁和槽，采集过程中不持 Core。
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let mut probe = self
            .probe
            .try_lock()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let inputs = {
            let mut core = self
                .core
                .lock()
                .map_err(|_| error(ControlErrorCode::Storage))?;
            core.store.glm_probe_inputs()?
        };
        let cycle = probe.collect_cycle(inputs).await.map_err(|e| {
            error(match e {
                PolicyError::InvalidInput | PolicyError::Capacity => ControlErrorCode::InvalidInput,
                PolicyError::SequenceExhausted => ControlErrorCode::Storage,
            })
        })?;
        self.apply_glm_cycle(&cycle, current_ms()?)
    }

    #[cfg(test)]
    pub(super) fn publish_glm_cycle(
        &self,
        cycle: &ProbeCycle,
        now_ms: i64,
    ) -> ControlResult<PolicyRefreshReport> {
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        self.apply_glm_cycle(cycle, now_ms)
    }

    fn apply_glm_cycle(
        &self,
        cycle: &ProbeCycle,
        now_ms: i64,
    ) -> ControlResult<PolicyRefreshReport> {
        if self.policy_config.dry_run {
            return self
                .core
                .lock()
                .map_err(|_| error(ControlErrorCode::Storage))?
                .store
                .preview_glm_round(&self.policy_config, cycle, now_ms);
        }
        // 重放不续期，完整提交后才替换整个许可集合；caller 已持命令槽。
        self.publish(|c| {
            if c.last_glm_cycle
                .is_some_and(|last| cycle.cycle_id() <= last)
            {
                return Err(error(ControlErrorCode::RevisionConflict));
            }
            let committed = c.store.commit_glm_round(
                &self.policy_config,
                cycle,
                c.backend,
                &c.permits,
                now_ms,
            )?;
            c.permits = committed.permits;
            c.last_glm_cycle = Some(committed.report.cycle_id);
            Ok(committed.report)
        })
    }
}
