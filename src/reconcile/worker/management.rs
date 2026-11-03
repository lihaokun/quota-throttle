//! C2来源发现；使用原同步控制提交和发布屏障，不新增后台任务。
#![cfg_attr(not(test), allow(dead_code))]
use super::*;
use crate::catalog::policy_commands::ModelDiscoveryInput;

impl ReconcileWorker {
    pub(crate) async fn discover_source_models(
        &self,
        id: OperationId,
        source_id: SourceId,
        expected_revision: i64,
    ) -> ControlResult<OperationView> {
        // Step C2.4: GET前重放短路；网络不持Core，提交重新核版本并整体发布。
        if self.policy_config.dry_run {
            return Err(error(ControlErrorCode::NotReady));
        }
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let client = self
            .model_catalog
            .try_lock()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let input = self
            .core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?
            .store
            .model_discovery_input(&id, source_id, expected_revision)?;
        let models = match input {
            ModelDiscoveryInput::Replayed(view) => return Ok(view),
            ModelDiscoveryInput::Fetch { config, credential } => client
                .discover_bounded(&config, credential.expose())
                .await
                .map_err(|_| error(ControlErrorCode::InvalidResponse))?,
        };
        self.publish(|c| {
            c.store
                .submit_discovered_models(id, source_id, expected_revision, models, now())
        })
    }
}
