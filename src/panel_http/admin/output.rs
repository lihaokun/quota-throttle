use super::{AdminError, ExtraCode};
use crate::catalog::admin::SourceModelsView;
use crate::catalog::control::OperationView;
use crate::catalog::groups::GroupReferences;
use crate::catalog::tokens::TokenIdentityView;
use crate::catalog::{GroupView, ManagedTokenId, MembershipView, SourceView};
use crate::ledger::analytics::UsageSnapshot;
use crate::newapi::tokens::{NativeTokenPage, RevealedToken, TokenOwnerScope};
use crate::panel_http::{publication, Response};
use crate::reconcile::worker::token_service::GroupCheckReport;
use serde::Serialize;

#[cfg(test)]
mod tests;
mod usage;
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ModelOrigin {
    Discovered,
    Manual,
    Fallback,
}
#[derive(Serialize)]
pub(super) struct SourceCreated {
    pub schema_version: u8,
    pub source: SourceView,
    pub model_origin: ModelOrigin,
}
pub(super) enum AdminReply {
    SourceCreated(SourceCreated),
    Models(SourceModelsView),
    Group(GroupView),
    Membership(MembershipView),
    Accepted(OperationView),
    Operation(OperationView),
    GroupCheck(GroupCheckReport),
    GroupReferences(GroupReferences),
    Identities {
        items: Vec<TokenIdentityView>,
        after: Option<ManagedTokenId>,
    },
    Identity(TokenIdentityView),
    NativeTokens {
        page: NativeTokenPage,
        scope: TokenOwnerScope,
    },
    Reveal {
        token: ManagedTokenId,
        secret: RevealedToken,
    },
    Usage(UsageSnapshot),
}
fn response<T: Serialize>(status: &'static str, value: &T) -> Result<Response, AdminError> {
    Ok(Response {
        status,
        body: publication::encode_bounded(value)
            .map_err(|_| AdminError::extra(ExtraCode::ResponseTooLarge))?,
        age_ms: None,
        allow: None,
        content_type: super::super::ContentType::Json,
    })
}
pub(super) fn encode(reply: AdminReply) -> Result<Response, AdminError> {
    match reply {
        AdminReply::SourceCreated(v) => response("201 Created", &v),
        AdminReply::Models(v) => response("200 OK", &v),
        AdminReply::Group(group) => {
            #[derive(Serialize)]
            struct Wire {
                schema_version: u8,
                group: GroupView,
            }
            response(
                "201 Created",
                &Wire {
                    schema_version: 1,
                    group,
                },
            )
        }
        AdminReply::Membership(membership) => {
            #[derive(Serialize)]
            struct Wire {
                schema_version: u8,
                membership: MembershipView,
            }
            response(
                "201 Created",
                &Wire {
                    schema_version: 1,
                    membership,
                },
            )
        }
        AdminReply::Accepted(v) => response("202 Accepted", &v),
        AdminReply::Operation(v) => response("200 OK", &v),
        AdminReply::GroupCheck(v) => response("200 OK", &v),
        AdminReply::GroupReferences(v) => response("200 OK", &v),
        AdminReply::Identity(v) => response("200 OK", &v),
        AdminReply::Identities { items, after } => {
            #[derive(Serialize)]
            struct Wire {
                schema_version: u8,
                items: Vec<TokenIdentityView>,
                after: Option<ManagedTokenId>,
                next_after: Option<ManagedTokenId>,
            }
            let next_after = if items.len() == 100 {
                items.last().map(|v| v.token_id)
            } else {
                None
            };
            response(
                "200 OK",
                &Wire {
                    schema_version: 1,
                    items,
                    after,
                    next_after,
                },
            )
        }
        AdminReply::NativeTokens { page, scope } => {
            #[derive(Serialize)]
            struct Wire {
                schema_version: u8,
                owner_scope: TokenOwnerScope,
                page: usize,
                page_size: usize,
                total: usize,
                items: Vec<crate::newapi::tokens::NativeTokenView>,
            }
            response(
                "200 OK",
                &Wire {
                    schema_version: 1,
                    owner_scope: scope,
                    page: page.page,
                    page_size: page.page_size,
                    total: page.total,
                    items: page.items,
                },
            )
        }
        AdminReply::Reveal { token, secret } => encode_reveal(token, &secret),
        AdminReply::Usage(snapshot) => response("200 OK", &usage::project(&snapshot)?),
    }
}
fn encode_reveal(token_id: ManagedTokenId, secret: &RevealedToken) -> Result<Response, AdminError> {
    // Step H2.5: 唯一明文编码出口；内部RevealedToken及请求不获得Serialize/Debug，结果不进cache。
    #[derive(Serialize)]
    struct Wire<'a> {
        schema_version: u8,
        token_id: ManagedTokenId,
        key: &'a str,
    }
    response(
        "200 OK",
        &Wire {
            schema_version: 1,
            token_id,
            key: secret.expose(),
        },
    )
}
