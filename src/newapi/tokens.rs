//! 固定owner的原生令牌能力；不保存凭据副本，不代替上层持久操作及回读确认。
#![cfg_attr(not(test), allow(dead_code))]
use super::transport::{
    error, DeliveryEvidence, ManagementErrorKind as Code, ManagementRequest, ManagementResponse,
    ManagementTransport, RequestClass, RequestSemantics, Result, LIST_BODY_LIMIT,
    SINGLE_BODY_LIMIT,
};
use crate::catalog::{SecretString, JSON_SAFE};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub const NATIVE_TOKEN_SCHEMA_VERSION: i64 = 1;
const PAGE_SIZE: usize = 100;
const MAX_TOKENS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenOwnerScope {
    Issuer,
    LegacyAdmin,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenOwner {
    pub user_id: i64,
    pub scope: TokenOwnerScope,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenOwnerProfile {
    pub user_id: i64,
    pub role: i64,
    pub account_group: String,
    pub quota: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenStatus {
    Enabled,
    Disabled,
    Expired,
    Exhausted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeTokenView {
    pub schema_version: i64,
    pub token_id: String,
    pub owner_user_id: String,
    pub owner_scope: TokenOwnerScope,
    pub name: String,
    pub masked_key: String,
    pub explicit_group: String,
    pub effective_group: String,
    pub status: TokenStatus,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub remaining_quota: i64,
    pub used_quota: i64,
    pub unlimited_quota: bool,
    pub model_limits_enabled: bool,
    pub model_limits: String,
    pub allow_ips: Option<String>,
    pub cross_group_retry: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct NativeTokenPage {
    pub items: Vec<NativeTokenView>,
    pub total: usize,
    pub page: usize,
    pub page_size: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenCreateSpec {
    pub correlation_name: String,
    pub group: String,
    pub expires_at: Option<i64>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum CreationLookup {
    Missing,
    One(Box<NativeTokenView>),
    Ambiguous,
}
pub struct RevealedToken(SecretString);
impl RevealedToken {
    pub(crate) fn expose(&self) -> &str {
        self.0.expose()
    }
}

fn invalid() -> super::transport::ManagementError {
    error(Code::InvalidResponse).with_delivery(DeliveryEvidence::Uncertain)
}
fn ensure(ok: bool, kind: Code) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(error(kind))
    }
}
fn integer(value: &Value, name: &str) -> Result<i64> {
    value.get(name).and_then(Value::as_i64).ok_or_else(invalid)
}
fn string(value: &Value, name: &str) -> Result<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(invalid)
}
fn boolean(value: &Value, name: &str) -> Result<bool> {
    value.get(name).and_then(Value::as_bool).ok_or_else(invalid)
}
fn unsigned_safe(n: i64) -> bool {
    (0..=JSON_SAFE).contains(&n)
}
fn signed_safe(n: i64) -> bool {
    (-JSON_SAFE..=JSON_SAFE).contains(&n)
}
fn now() -> Result<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .filter(|n| unsigned_safe(*n))
        .ok_or_else(|| error(Code::InvalidInput))
}
fn validate_spec(spec: &TokenCreateSpec) -> Result<()> {
    // Step T5A.1: 先验证结构；历史查回不使用创建时的未来截止限制。
    let suffix = spec.correlation_name.strip_prefix("qt-t-");
    ensure(
        suffix.is_some_and(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) && crate::catalog::types::valid_group(&spec.group)
            && spec.expires_at.is_none_or(unsigned_safe),
        Code::InvalidInput,
    )
}
fn mask_key(key: &str) -> Result<String> {
    if key.is_empty()
        || key.len() > 128
        || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'*')
    {
        return Err(invalid());
    }
    Ok(match key.len() {
        0..=4 => "*".repeat(key.len()),
        5..=8 => format!("{}****{}", &key[..2], &key[key.len() - 2..]),
        _ => format!("{}**********{}", &key[..4], &key[key.len() - 4..]),
    })
}
fn parse_profile(value: &Value, owner: TokenOwner) -> Result<TokenOwnerProfile> {
    let user_id = integer(value, "id")?;
    let role = integer(value, "role")?;
    let active = integer(value, "status")? == 1;
    let role_ok = match owner.scope {
        TokenOwnerScope::Issuer => role == 1,
        TokenOwnerScope::LegacyAdmin => matches!(role, 10 | 100),
    };
    ensure(
        user_id == owner.user_id && active && role_ok,
        Code::AuthFailed,
    )?;
    let account_group = string(value, "group")?;
    let quota = integer(value, "quota")?;
    if account_group.is_empty() || !signed_safe(quota) {
        return Err(invalid());
    }
    Ok(TokenOwnerProfile {
        user_id,
        role,
        account_group,
        quota,
    })
}
fn parse_token(
    value: &Value,
    profile: &TokenOwnerProfile,
    owner: TokenOwner,
) -> Result<NativeTokenView> {
    let id = integer(value, "id")?;
    let user_id = integer(value, "user_id")?;
    if id <= 0 || user_id != owner.user_id || user_id != profile.user_id {
        return Err(invalid());
    }
    let status = match integer(value, "status")? {
        1 => TokenStatus::Enabled,
        2 => TokenStatus::Disabled,
        3 => TokenStatus::Expired,
        4 => TokenStatus::Exhausted,
        _ => return Err(invalid()),
    };
    let created_at = integer(value, "created_time")?;
    let expires = integer(value, "expired_time")?;
    let remaining_quota = integer(value, "remain_quota")?;
    let used_quota = integer(value, "used_quota")?;
    if !unsigned_safe(created_at)
        || !(expires == -1 || unsigned_safe(expires))
        || !signed_safe(remaining_quota)
        || !unsigned_safe(used_quota)
    {
        return Err(invalid());
    }
    let group = string(value, "group")?;
    let allow_ips = match value.get("allow_ips") {
        Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return Err(invalid()),
    };
    Ok(NativeTokenView {
        schema_version: NATIVE_TOKEN_SCHEMA_VERSION,
        token_id: id.to_string(),
        owner_user_id: user_id.to_string(),
        owner_scope: owner.scope,
        name: string(value, "name")?,
        masked_key: mask_key(&string(value, "key")?)?,
        effective_group: if group.is_empty() {
            profile.account_group.clone()
        } else {
            group.clone()
        },
        explicit_group: group,
        status,
        created_at,
        expires_at: if expires == -1 { None } else { Some(expires) },
        remaining_quota,
        used_quota,
        unlimited_quota: boolean(value, "unlimited_quota")?,
        model_limits_enabled: boolean(value, "model_limits_enabled")?,
        model_limits: string(value, "model_limits")?,
        allow_ips,
        cross_group_retry: boolean(value, "cross_group_retry")?,
    })
}
fn parse_page(
    value: &Value,
    page: usize,
    page_size: usize,
    profile: &TokenOwnerProfile,
    owner: TokenOwner,
) -> Result<NativeTokenPage> {
    let total = usize::try_from(integer(value, "total")?).map_err(|_| invalid())?;
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if total > MAX_TOKENS
        || integer(value, "page")? != page as i64
        || integer(value, "page_size")? != page_size as i64
        || items.len() != total.saturating_sub((page - 1) * page_size).min(page_size)
    {
        return Err(invalid());
    }
    let mut result = Vec::with_capacity(items.len());
    let mut previous = None;
    for item in items {
        let view = parse_token(item, profile, owner)?;
        let id = integer(item, "id")?;
        if previous.is_some_and(|p| id >= p) {
            return Err(invalid());
        }
        previous = Some(id);
        result.push(view);
    }
    Ok(NativeTokenPage {
        items: result,
        total,
        page,
        page_size,
    })
}
fn matches_spec(view: &NativeTokenView, spec: &TokenCreateSpec) -> bool {
    view.name == spec.correlation_name
        && view.explicit_group == spec.group
        && view.expires_at == spec.expires_at
        && view.unlimited_quota
        && !view.model_limits_enabled
        && view.model_limits.is_empty()
        && view.allow_ips.as_ref().is_none_or(String::is_empty)
        && !view.cross_group_retry
}
fn same_settings(a: &NativeTokenView, b: &NativeTokenView) -> bool {
    a.name == b.name
        && a.created_at == b.created_at
        && a.explicit_group == b.explicit_group
        && a.expires_at == b.expires_at
        && a.unlimited_quota == b.unlimited_quota
        && a.model_limits_enabled == b.model_limits_enabled
        && a.model_limits == b.model_limits
        && a.allow_ips == b.allow_ips
        && a.cross_group_retry == b.cross_group_retry
}

pub struct ManagedTokens<'a> {
    transport: &'a ManagementTransport,
    owner: TokenOwner,
    guard: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
}
impl<'a> ManagedTokens<'a> {
    pub(super) fn new(transport: &'a ManagementTransport, owner: TokenOwner) -> Self {
        Self {
            transport,
            owner,
            guard: None,
        }
    }
    pub fn fenced<'b>(&'b self, guard: &'b (dyn Fn() -> bool + Send + Sync)) -> ManagedTokens<'b> {
        ManagedTokens {
            transport: self.transport,
            owner: self.owner,
            guard: Some(guard),
        }
    }
    fn validate_owner(&self) -> Result<()> {
        ensure(self.owner.user_id > 0, Code::InvalidInput)
    }
    async fn send(
        &self,
        request: ManagementRequest,
        class: RequestClass,
    ) -> Result<ManagementResponse> {
        // Step T5A.4: 固定owner，复用同一budget/认证与最终准入guard，不新建会话或重试器。
        let request = request.for_user(self.owner.user_id)?;
        let response = match self.guard {
            Some(g) => self.transport.send_fenced(request, class, g).await,
            None => self.transport.send(request, class).await,
        }?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(invalid());
        }
        Ok(response)
    }
    async fn read(&self, path: &str, limit: usize, class: RequestClass) -> Result<Value> {
        self.send(
            ManagementRequest::new(Method::GET, path, None, RequestSemantics::Read, limit)?,
            class,
        )
        .await?
        .envelope()
    }
    async fn write(
        &self,
        method: Method,
        path: &str,
        payload: Value,
        semantics: RequestSemantics,
    ) -> Result<Value> {
        self.send(
            ManagementRequest::new(method, path, Some(payload), semantics, SINGLE_BODY_LIMIT)?,
            RequestClass::Control,
        )
        .await?
        .envelope()
    }
    pub async fn profile(&self) -> Result<TokenOwnerProfile> {
        // Step T5A.2: 每次核固定账号/角色；不把上次profile当作永久授权。
        self.validate_owner()?;
        let body = self
            .read("/api/user/self", SINGLE_BODY_LIMIT, RequestClass::Control)
            .await?;
        parse_profile(body.get("data").ok_or_else(invalid)?, self.owner)
    }
    async fn groups_for_owner(&self) -> Result<Vec<String>> {
        let body = self
            .read(
                "/api/user/self/groups",
                SINGLE_BODY_LIMIT,
                RequestClass::Control,
            )
            .await?;
        let map = body
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(invalid)?;
        let mut groups = Vec::new();
        for (name, value) in map {
            if name == "auto" {
                continue;
            }
            if name.is_empty()
                || !value
                    .get("ratio")
                    .and_then(Value::as_f64)
                    .is_some_and(|v| v.is_finite() && v >= 0.0)
            {
                return Err(invalid());
            }
            groups.push(name.clone());
        }
        groups.sort();
        Ok(groups)
    }
    pub async fn available_groups(&self) -> Result<Vec<String>> {
        self.profile().await?;
        self.groups_for_owner().await
    }
    pub async fn list_page(&self, page: usize) -> Result<NativeTokenPage> {
        ensure(
            (1..=MAX_TOKENS / PAGE_SIZE).contains(&page),
            Code::InvalidInput,
        )?;
        let profile = self.profile().await?;
        let body = self
            .read(
                &format!("/api/token/?p={page}&page_size={PAGE_SIZE}"),
                LIST_BODY_LIMIT,
                RequestClass::Observe,
            )
            .await?;
        parse_page(
            body.get("data").ok_or_else(invalid)?,
            page,
            PAGE_SIZE,
            &profile,
            self.owner,
        )
    }
    async fn get_for_owner(
        &self,
        id: i64,
        profile: &TokenOwnerProfile,
    ) -> Result<Option<NativeTokenView>> {
        let response = self
            .send(ManagementRequest::token_lookup(id)?, RequestClass::Control)
            .await?;
        if response.token_missing() {
            return Ok(None);
        }
        let body = response.envelope()?;
        let view = parse_token(body.get("data").ok_or_else(invalid)?, profile, self.owner)?;
        if view.token_id != id.to_string() {
            return Err(invalid());
        }
        Ok(Some(view))
    }
    pub async fn get(&self, id: i64) -> Result<Option<NativeTokenView>> {
        ensure(id > 0, Code::InvalidInput)?;
        let profile = self.profile().await?;
        self.get_for_owner(id, &profile).await
    }
    pub async fn lookup_created(&self, spec: &TokenCreateSpec) -> Result<CreationLookup> {
        validate_spec(spec)?;
        ensure(
            self.owner.scope == TokenOwnerScope::Issuer,
            Code::InvalidInput,
        )?;
        let profile = self.profile().await?;
        let body = self
            .read(
                &format!(
                    "/api/token/search?keyword={}&p=1&page_size=2",
                    spec.correlation_name
                ),
                LIST_BODY_LIMIT,
                RequestClass::Control,
            )
            .await?;
        let mut page = parse_page(
            body.get("data").ok_or_else(invalid)?,
            1,
            2,
            &profile,
            self.owner,
        )?;
        match page.total {
            0 => Ok(CreationLookup::Missing),
            1 => {
                let view = page.items.pop().ok_or_else(invalid)?;
                if !matches_spec(&view, spec) {
                    return Err(invalid());
                }
                Ok(CreationLookup::One(Box::new(view)))
            }
            _ => Ok(CreationLookup::Ambiguous),
        }
    }
    pub async fn create(&self, spec: &TokenCreateSpec) -> Result<()> {
        // Step T5A.3: 单组创建只限issuer；只有ACK，不提供未核归属的ID或key。
        validate_spec(spec)?;
        ensure(
            self.owner.scope == TokenOwnerScope::Issuer,
            Code::InvalidInput,
        )?;
        let at = now()?;
        ensure(spec.expires_at.is_none_or(|t| t > at), Code::InvalidInput)?;
        let profile = self.profile().await.map_err(|e| e.before_write())?;
        ensure(profile.quota > 0, Code::RemoteRejected)?;
        let groups = self
            .groups_for_owner()
            .await
            .map_err(|e| e.before_write())?;
        ensure(groups.contains(&spec.group), Code::RemoteRejected)?;
        let guard = || {
            self.guard.is_none_or(|g| g())
                && now()
                    .ok()
                    .is_some_and(|t| spec.expires_at.is_none_or(|e| e > t))
        };
        self.fenced(&guard).write(Method::POST,"/api/token/",json!({"name":spec.correlation_name,"group":spec.group,
            "expired_time":spec.expires_at.unwrap_or(-1),"unlimited_quota":true,"remain_quota":0,"model_limits_enabled":false,"model_limits":"","allow_ips":"","cross_group_retry":false}),RequestSemantics::Create).await?;
        Ok(())
    }
    pub async fn set_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        ensure(id > 0, Code::InvalidInput)?;
        let profile = self.profile().await.map_err(|e| e.before_write())?;
        let before = self
            .get_for_owner(id, &profile)
            .await
            .map_err(|e| e.before_write())?
            .ok_or_else(|| error(Code::RemoteRejected))?;
        let body = self
            .write(
                Method::PUT,
                "/api/token/?status_only=true",
                json!({"id":id,"status":if enabled{1}else{2}}),
                RequestSemantics::IdempotentWrite,
            )
            .await?;
        let after = parse_token(body.get("data").ok_or_else(invalid)?, &profile, self.owner)?;
        if after.token_id != id.to_string()
            || after.status
                != if enabled {
                    TokenStatus::Enabled
                } else {
                    TokenStatus::Disabled
                }
            || !same_settings(&before, &after)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub async fn delete(&self, id: i64) -> Result<()> {
        ensure(id > 0, Code::InvalidInput)?;
        self.profile().await.map_err(|e| e.before_write())?;
        let body = self
            .write(
                Method::POST,
                "/api/token/batch",
                json!({"ids":[id]}),
                RequestSemantics::IdempotentWrite,
            )
            .await?;
        if !body
            .get("data")
            .and_then(Value::as_i64)
            .is_some_and(|n| matches!(n, 0 | 1))
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub async fn reveal(&self, id: i64) -> Result<RevealedToken> {
        ensure(id > 0, Code::InvalidInput)?;
        let profile = self.profile().await.map_err(|e| e.before_write())?;
        self.get_for_owner(id, &profile)
            .await
            .map_err(|e| e.before_write())?
            .ok_or_else(|| error(Code::RemoteRejected))?;
        let body = self
            .write(
                Method::POST,
                &format!("/api/token/{id}/key"),
                json!({}),
                RequestSemantics::IdempotentWrite,
            )
            .await?;
        let raw = body
            .get("data")
            .and_then(|v| v.get("key"))
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if !(16..=128).contains(&raw.len()) || !raw.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(invalid());
        }
        Ok(RevealedToken(
            SecretString::new(format!("sk-{raw}")).map_err(|_| invalid())?,
        ))
    }
}

#[cfg(test)]
mod tests;
