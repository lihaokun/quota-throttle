use super::{bad_request, invalid_input, AdminFailure};
use crate::catalog::control::OperationId;
use crate::catalog::{GroupId, ManagedTokenId, MembershipId, SourceId};
use crate::newapi::tokens::TokenOwnerScope;
use crate::panel_http::{HttpErrorCode, STATE_PATH};
use std::collections::BTreeMap;

pub(super) enum Route {
    State,
    CreateSource,
    SourceModels(SourceId),
    SetModels(SourceId),
    DiscoverModels(SourceId),
    SetSourcePaused(SourceId),
    RetireSource(SourceId),
    ClearVolcanoError(SourceId),
    UpdateVolcanoPolicy(SourceId),
    CreateGroup,
    CheckGroup(GroupId),
    GroupReferences(GroupId),
    SetGroupPin(GroupId),
    PlanMembership,
    EnsureMember(MembershipId),
    RetireMember(MembershipId),
    Operation(OperationId),
    Retry(OperationId),
    Identities(Option<ManagedTokenId>),
    CreateToken,
    Identity(ManagedTokenId),
    SetTokenEnabled(ManagedTokenId),
    DeleteToken(ManagedTokenId),
    RevealToken(ManagedTokenId),
    NativeTokens(TokenOwnerScope, usize),
    RegisterToken,
    Usage,
}
pub(super) fn id(s: &str) -> Result<i64, AdminFailure> {
    if s.is_empty() || s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid_input());
    }
    s.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(invalid_input)
}
pub(super) fn operation(s: &str) -> Result<OperationId, AdminFailure> {
    OperationId::new(s.to_owned()).map_err(Into::into)
}
fn query<'a>(
    q: Option<&'a str>,
    allowed: &[&str],
) -> Result<BTreeMap<&'a str, &'a str>, AdminFailure> {
    let mut values = BTreeMap::new();
    if let Some(q) = q {
        if q.is_empty() || q.contains(['%', '+']) {
            return Err(bad_request());
        }
        for pair in q.split('&') {
            let (k, v) = pair.split_once('=').ok_or_else(bad_request)?;
            if k.is_empty()
                || v.is_empty()
                || !allowed.contains(&k)
                || values.insert(k, v).is_some()
            {
                return Err(bad_request());
            }
        }
    }
    Ok(values)
}
pub(super) fn parse(
    method: &str,
    path: &str,
    raw_query: Option<&str>,
) -> Result<Route, AdminFailure> {
    // Step H2.1: 先白名单方法/路径，后强类型ID/query；state保持H1原优先级。
    if path == STATE_PATH {
        if raw_query.is_some() {
            return Err(bad_request());
        }
        return if method == "GET" {
            Ok(Route::State)
        } else {
            Err(AdminFailure::Http(
                HttpErrorCode::MethodNotAllowed,
                Some("GET"),
            ))
        };
    }
    let suffix = path
        .strip_prefix("/api/admin/v1/")
        .ok_or(AdminFailure::Http(HttpErrorCode::NotFound, None))?;
    let parts: Vec<_> = suffix.split('/').collect();
    let allow = match parts.as_slice() {
        ["sources"]
        | ["groups"]
        | ["memberships"]
        | ["tokens", "register"]
        | ["usage", "query"]
        | ["sources", _, "models", "discover"]
        | ["sources", _, "retire"]
        | ["sources", _, "clear-error"]
        | ["groups", _, "check"]
        | ["memberships", _, "ensure"]
        | ["memberships", _, "retire"]
        | ["operations", _, "retry"]
        | ["tokens", _, "reveal"] => "POST",
        ["sources", _, "models"] => "GET, PUT",
        ["sources", _, "paused"]
        | ["sources", _, "volcano-policy"]
        | ["groups", _, "pin"]
        | ["tokens", _, "enabled"] => "PUT",
        ["groups", _, "references"] | ["operations", _] | ["native-tokens"] => "GET",
        ["tokens"] => "GET, POST",
        ["tokens", _] => "GET, DELETE",
        _ => return Err(AdminFailure::Http(HttpErrorCode::NotFound, None)),
    };
    if !allow.split(", ").any(|m| m == method) {
        return Err(AdminFailure::Http(
            HttpErrorCode::MethodNotAllowed,
            Some(allow),
        ));
    }
    let has_query = (parts.as_slice() == ["tokens"] && method == "GET")
        || parts.as_slice() == ["native-tokens"];
    if !has_query && raw_query.is_some() {
        return Err(bad_request());
    }
    let result = match (method, parts.as_slice()) {
        ("POST", ["sources"]) => Route::CreateSource,
        ("GET", ["sources", s, "models"]) => Route::SourceModels(SourceId::new(id(s)?)?),
        ("PUT", ["sources", s, "models"]) => Route::SetModels(SourceId::new(id(s)?)?),
        ("POST", ["sources", s, "models", "discover"]) => {
            Route::DiscoverModels(SourceId::new(id(s)?)?)
        }
        ("PUT", ["sources", s, "paused"]) => Route::SetSourcePaused(SourceId::new(id(s)?)?),
        ("POST", ["sources", s, "retire"]) => Route::RetireSource(SourceId::new(id(s)?)?),
        ("POST", ["sources", s, "clear-error"]) => Route::ClearVolcanoError(SourceId::new(id(s)?)?),
        ("PUT", ["sources", s, "volcano-policy"]) => {
            Route::UpdateVolcanoPolicy(SourceId::new(id(s)?)?)
        }
        ("POST", ["groups"]) => Route::CreateGroup,
        ("POST", ["groups", g, "check"]) => Route::CheckGroup(GroupId::new(id(g)?)?),
        ("GET", ["groups", g, "references"]) => Route::GroupReferences(GroupId::new(id(g)?)?),
        ("PUT", ["groups", g, "pin"]) => Route::SetGroupPin(GroupId::new(id(g)?)?),
        ("POST", ["memberships"]) => Route::PlanMembership,
        ("POST", ["memberships", m, "ensure"]) => Route::EnsureMember(MembershipId::new(id(m)?)?),
        ("POST", ["memberships", m, "retire"]) => Route::RetireMember(MembershipId::new(id(m)?)?),
        ("GET", ["operations", op]) => Route::Operation(operation(op)?),
        ("POST", ["operations", op, "retry"]) => Route::Retry(operation(op)?),
        ("GET", ["tokens"]) => {
            let q = query(raw_query, &["after"])?;
            Route::Identities(
                q.get("after")
                    .map(|s| id(s).and_then(|v| ManagedTokenId::new(v).map_err(Into::into)))
                    .transpose()?,
            )
        }
        ("POST", ["tokens"]) => Route::CreateToken,
        ("POST", ["tokens", "register"]) => Route::RegisterToken,
        ("GET", ["tokens", t]) => Route::Identity(ManagedTokenId::new(id(t)?)?),
        ("PUT", ["tokens", t, "enabled"]) => Route::SetTokenEnabled(ManagedTokenId::new(id(t)?)?),
        ("DELETE", ["tokens", t]) => Route::DeleteToken(ManagedTokenId::new(id(t)?)?),
        ("POST", ["tokens", t, "reveal"]) => Route::RevealToken(ManagedTokenId::new(id(t)?)?),
        ("GET", ["native-tokens"]) => {
            let q = query(raw_query, &["owner_scope", "page"])?;
            let scope = match q.get("owner_scope").copied() {
                Some("issuer") => TokenOwnerScope::Issuer,
                Some("legacy_admin") => TokenOwnerScope::LegacyAdmin,
                _ => return Err(bad_request()),
            };
            let page = match q.get("page") {
                None => 1,
                Some(s) if s.bytes().all(|b| b.is_ascii_digit()) => {
                    s.parse::<usize>().map_err(|_| invalid_input())?
                }
                _ => return Err(invalid_input()),
            };
            if !(1..=100).contains(&page) {
                return Err(invalid_input());
            }
            Route::NativeTokens(scope, page)
        }
        ("POST", ["usage", "query"]) => Route::Usage,
        _ => return Err(AdminFailure::Http(HttpErrorCode::Internal, None)),
    };
    Ok(result)
}
