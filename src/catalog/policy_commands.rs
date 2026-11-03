//! 管理配置与回执共用原控制事务；不签发渠道许可或隐式运行策略。
use super::control::{
    ensure, failure, replayed_operation, ControlCommand, ControlErrorCode as Code, ControlResult,
    OperationId, OperationView,
};
use super::policy::{
    apply_source_policy_tx, group_record, read_group_policy, read_runtime, save_group_policy_tx,
    source_record, GroupPolicyState, SourcePolicyUpdate,
};
use super::*;
use rusqlite::{params, Transaction};

pub(crate) enum ModelDiscoveryInput {
    Replayed(OperationView),
    Fetch {
        config: crate::config::ModelDiscoveryConfig,
        credential: SecretString,
    },
}

pub(super) fn apply_policy_command_tx(
    tx: &Transaction<'_>,
    command: &ControlCommand,
    next: i64,
    now: i64,
) -> ControlResult<()> {
    // Step C2.2: 局部设置/周期历史/实体版本与调用方回执在同一事务。
    match command {
        ControlCommand::SetGroupPin {
            group_id,
            expected_revision,
            expected_policy_revision,
            membership_id,
        } => {
            let group = group_record(tx, *group_id)?;
            ensure(group.kind == SourceKind::Glm, Code::InvalidInput)?;
            let old = read_group_policy(tx, &group)?;
            let GroupPolicyState::Glm(mut state) = old.state else {
                return Err(failure(Code::Storage));
            };
            if let Some(id) = membership_id {
                let work = super::execution::load_work(tx, *id, false)?;
                ensure(work.group.group_id == *group_id, Code::InvalidInput)?;
                ensure(super::glm_round::editable(&work), Code::NotReady)?;
            }
            state.pinned = *membership_id;
            save_group_policy_tx(
                tx,
                *group_id,
                *expected_revision,
                *expected_policy_revision,
                GroupPolicyState::Glm(state),
            )?;
            ensure(
                tx.execute(
                    "UPDATE source_groups SET revision=?1 WHERE group_id=?2 AND revision=?3",
                    params![next, group_id.get(), expected_revision],
                )? == 1,
                Code::RevisionConflict,
            )?;
        }
        ControlCommand::ClearVolcanoError {
            source_id,
            expected_revision,
            expected_runtime_revision,
        }
        | ControlCommand::UpdateVolcanoPolicy {
            source_id,
            expected_revision,
            expected_runtime_revision,
            ..
        } => {
            let (source, mut settings, retiring) = source_record(tx, *source_id)?;
            ensure(!retiring && source.retired_at.is_none(), Code::NotReady)?;
            let SourceSettings::Volcano { policy, .. } = &settings else {
                return Err(failure(Code::InvalidInput));
            };
            let old = read_runtime(tx, *source_id, policy)?;
            if matches!(command, ControlCommand::ClearVolcanoError { .. }) {
                ensure(old.is_some(), Code::NotReady)?;
            }
            ensure(
                old.as_ref().map_or(0, |r| r.revision) == *expected_runtime_revision,
                Code::RevisionConflict,
            )?;
            if old.is_some() {
                let update = match command {
                    ControlCommand::ClearVolcanoError { .. } => SourcePolicyUpdate::ClearError,
                    ControlCommand::UpdateVolcanoPolicy { policy, .. } => {
                        SourcePolicyUpdate::ReplacePolicy(policy.clone())
                    }
                    _ => return Err(failure(Code::InvalidInput)),
                };
                apply_source_policy_tx(
                    tx,
                    *source_id,
                    *expected_revision,
                    *expected_runtime_revision,
                    update,
                    now,
                )?;
            } else if let ControlCommand::UpdateVolcanoPolicy { policy: new, .. } = command {
                if let SourceSettings::Volcano { policy, .. } = &mut settings {
                    *policy = new.clone();
                }
                tx.execute(
                    "UPDATE sources SET settings_json=?1 WHERE source_id=?2",
                    params![super::types::encode_settings(&settings)?, source_id.get()],
                )?;
            } else {
                return Err(failure(Code::NotReady));
            }
            ensure(
                tx.execute(
                    "UPDATE sources SET config_revision=?1 WHERE source_id=?2",
                    params![next, source_id.get()],
                )? == 1,
                Code::Storage,
            )?;
        }
        _ => return Err(failure(Code::InvalidInput)),
    }
    Ok(())
}

impl CatalogStore {
    pub(crate) fn model_discovery_input(
        &mut self,
        id: &OperationId,
        source: SourceId,
        expected_revision: i64,
    ) -> ControlResult<ModelDiscoveryInput> {
        // Step C2.3: 先匹配稳定请求摘要；重放不取材料、不发GET。
        self.usable()?;
        let (_, digest) = ControlCommand::DiscoverSourceModels {
            source_id: source,
            expected_revision,
        }
        .canonical()?;
        let tx = self.conn.transaction()?;
        if let Some(view) = replayed_operation(&tx, id, &digest)? {
            return Ok(ModelDiscoveryInput::Replayed(view));
        }
        let (view, settings, retiring) = source_record(&tx, source)?;
        ensure(
            view.config_revision == expected_revision,
            Code::RevisionConflict,
        )?;
        ensure(!retiring && view.retired_at.is_none(), Code::NotReady)?;
        let config = match settings {
            SourceSettings::Glm { upstream, .. } | SourceSettings::Volcano { upstream, .. } => {
                upstream.model_discovery
            }
        }
        .ok_or_else(|| failure(Code::NotReady))?;
        let material = super::store::read_material(&tx, source, false)?;
        Ok(ModelDiscoveryInput::Fetch {
            config,
            credential: material.credential,
        })
    }
}
