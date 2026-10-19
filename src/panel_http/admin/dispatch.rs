use super::input::{AdminRequest, ModelSelection, NewSource, SourceKindInput};
use super::output::{AdminReply, ModelOrigin, SourceCreated};
use super::{AdminError, ExtraCode};
use crate::catalog::control::ControlErrorCode;
use crate::catalog::{SourceDraft, SourceSettings};
use crate::reconcile::worker::ReconcileWorker;
use std::sync::Arc;

async fn create_source(
    worker: &ReconcileWorker,
    source: NewSource,
) -> Result<AdminReply, AdminError> {
    // Step H2.3: 先查同凭据身份；本次发现成功权威，只有明确fallback才兜底，未成功解析不落来源。
    if let Some(id) = worker.source_for_credential(&source.credential)? {
        return Err(AdminError::duplicate(id));
    }
    let (models, model_origin) = match source.selection {
        ModelSelection::Manual(models) => (models, ModelOrigin::Manual),
        selection => {
            let config = source
                .upstream
                .model_discovery
                .as_ref()
                .ok_or_else(|| AdminError::control(ControlErrorCode::InvalidInput))?;
            match worker
                .discover_candidate_models(config, &source.credential)
                .await
            {
                Ok(models) => (models, ModelOrigin::Discovered),
                Err(error) => {
                    if error.code != ControlErrorCode::InvalidResponse {
                        return Err(error.into());
                    }
                    match selection {
                        ModelSelection::DiscoverOrManual(models) => (models, ModelOrigin::Fallback),
                        _ => return Err(error.into()),
                    }
                }
            }
        }
    };
    let mut upstream = source.upstream;
    upstream.models = models;
    let settings = match source.kind {
        SourceKindInput::Glm(quota) => SourceSettings::Glm { upstream, quota },
        SourceKindInput::Volcano(policy) => SourceSettings::Volcano { upstream, policy },
    };
    let source = worker.create_source(SourceDraft {
        name: source.name,
        note: source.note,
        credential: source.credential,
        settings,
    })?;
    Ok(AdminReply::SourceCreated(SourceCreated {
        schema_version: 1,
        source,
        model_origin,
    }))
}
pub(super) async fn dispatch(
    worker: &Arc<ReconcileWorker>,
    request: AdminRequest,
) -> Result<AdminReply, AdminError> {
    // Step H2.4: 固定callee和版本；目录201、持久202及查询200沿原回执，不额外执行/retry。
    use AdminRequest as R;
    Ok(match request {
        R::CreateSource(source) => return create_source(worker, source).await,
        R::SourceModels(id) => AdminReply::Models(worker.source_models(id)?),
        R::CreateGroup(group) => AdminReply::Group(worker.create_group(group)?),
        R::PlanMembership { source, group } => AdminReply::Membership(worker.plan_membership(
            source,
            group,
            worker.standby_priority(),
        )?),
        R::Control { id, command } => AdminReply::Accepted(worker.submit(id, command)?),
        R::DiscoverModels {
            id,
            source,
            revision,
        } => AdminReply::Accepted(worker.discover_source_models(id, source, revision).await?),
        R::CheckGroup {
            group,
            revision,
            register,
        } => AdminReply::GroupCheck(worker.check_group(group, revision, register).await?),
        R::GroupReferences(id) => AdminReply::GroupReferences(worker.group_references(id)?),
        R::Operation(id) => AdminReply::Operation(worker.operation(&id)?),
        R::Retry(id) => AdminReply::Operation(worker.retry(&id).await?),
        R::Identities(after) => AdminReply::Identities {
            items: worker.token_identities(after)?,
            after,
        },
        R::Identity(id) => AdminReply::Identity(worker.token_identity(id)?),
        R::CreateToken {
            id,
            group,
            revision,
            name,
            expires,
        } => AdminReply::Accepted(worker.create_token(id, group, revision, name, expires)?),
        R::SetTokenEnabled {
            id,
            token,
            revision,
            enabled,
        } => AdminReply::Accepted(worker.set_token_enabled(id, token, revision, enabled)?),
        R::DeleteToken {
            id,
            token,
            revision,
        } => AdminReply::Accepted(worker.delete_token(id, token, revision)?),
        R::RevealToken { token, revision } => AdminReply::Reveal {
            token,
            secret: worker.reveal_token(token, revision).await?,
        },
        R::NativeTokens { scope, page } => AdminReply::NativeTokens {
            page: worker.list_tokens(scope, page).await?,
            scope,
        },
        R::RegisterToken {
            scope,
            native,
            name,
        } => AdminReply::Identity(worker.register_token(scope, native, name).await?),
        R::Usage(query) => AdminReply::Usage(worker.query_usage(query).await?),
    })
}
pub(super) fn dry_run_error() -> AdminError {
    AdminError::extra(ExtraCode::DryRun)
}
