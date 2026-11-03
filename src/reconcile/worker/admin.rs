//! 新HTTP需要的窄入口；沿原Core/命令槽/发现client，不引入业务状态。
use super::*;
use crate::catalog::admin::SourceModelsView;
use crate::config::ModelDiscoveryConfig;

impl ReconcileWorker {
    #[cfg(test)]
    pub(crate) fn admin_test_core_locked(&self, f: impl FnOnce()) {
        let _guard = self.core.lock().unwrap();
        f();
    }
    #[cfg(test)]
    pub(crate) fn admin_test_commands_available(&self) -> usize {
        self.commands.available_permits()
    }
    #[cfg(test)]
    pub(crate) async fn admin_test_query_slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        self.query_slots.clone().acquire_owned().await.unwrap()
    }
    pub(crate) fn is_dry_run(&self) -> bool {
        self.policy_config.dry_run
    }
    pub(crate) fn standby_priority(&self) -> i64 {
        self.policy_config.priority_standby
    }
    pub(crate) fn source_models(&self, id: SourceId) -> ControlResult<SourceModelsView> {
        self.core
            .lock()
            .map_err(|_| error(ControlErrorCode::Storage))?
            .store
            .source_models(id)
    }
    pub(crate) fn source_for_credential(
        &self,
        credential: &SecretString,
    ) -> crate::catalog::types::Result<Option<SourceId>> {
        self.core
            .lock()
            .map_err(|_| CatalogError {
                kind: ErrorKind::Storage,
                source_id: None,
                incomplete_backup: false,
            })?
            .store
            .source_for_credential(credential)
    }
    pub(crate) async fn discover_candidate_models(
        &self,
        config: &ModelDiscoveryConfig,
        credential: &SecretString,
    ) -> ControlResult<Vec<String>> {
        if self.is_dry_run() {
            return Err(error(ControlErrorCode::NotReady));
        }
        if config.url.len() > 8192 {
            return Err(error(ControlErrorCode::InvalidInput));
        }
        crate::catalog::types::validate_url(&config.url, false)
            .map_err(|_| error(ControlErrorCode::InvalidInput))?;
        let _slot = self
            .commands
            .try_acquire()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let client = self
            .model_catalog
            .try_lock()
            .map_err(|_| error(ControlErrorCode::QueueFull))?;
        let mut models = client
            .discover_bounded(config, credential.expose())
            .await
            .map_err(|_| error(ControlErrorCode::InvalidResponse))?;
        crate::catalog::control::canonical_models(&mut models)
            .map_err(|_| error(ControlErrorCode::InvalidResponse))?;
        Ok(models)
    }
    pub(crate) async fn wait_usage_idle(&self) -> ControlResult<()> {
        // 仅在所有调用方停止接纳、所有HTTP wrapper已join后用；不关闭原semaphore。
        let permit = self
            .query_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| error(ControlErrorCode::Storage))?;
        drop(permit);
        Ok(())
    }
}
