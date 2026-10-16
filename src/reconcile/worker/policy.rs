//! 无网络的 GLM 批次发布；完整采集入口由下一小步接入。
#![cfg_attr(not(test), allow(dead_code))]
use super::*;
use crate::catalog::glm_publication::PolicyRefreshReport;
use crate::source_policy::probe::ProbeCycle;

impl ReconcileWorker {
    pub(super) fn publish_glm_cycle(
        &self,
        cycle: &ProbeCycle,
        now_ms: i64,
    ) -> ControlResult<PolicyRefreshReport> {
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        if self.policy_config.dry_run {
            return self
                .core
                .lock()
                .map_err(|_| error(ControlErrorCode::Storage))?
                .store
                .preview_glm_round(&self.policy_config, cycle, now_ms);
        }
        // Step C2: 无网络的提交半段；重放不续期，完整提交后才替换整个许可集合。
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
