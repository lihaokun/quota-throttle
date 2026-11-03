//! 原生组配置的有限合并；不猜倍率别名真值，不修改无关账号权限。
#![cfg_attr(not(test), allow(dead_code))]
use super::transport::{
    error, DeliveryEvidence, ManagementErrorKind as Code, ManagementRequest, ManagementTransport,
    RequestClass, RequestSemantics, Result, LIST_BODY_LIMIT, SINGLE_BODY_LIMIT,
};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const RATIO: &str = "GroupRatio";
const ALIAS: &str = "group_ratio_setting.group_ratio";
const SPECIAL: &str = "group_ratio_setting.group_special_usable_group";
const LIMIT: usize = 10_000;
type Ratios = BTreeMap<String, f64>;
type Special = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Clone, PartialEq)]
struct Options {
    ratio: Ratios,
    alias: Ratios,
    special: Special,
}
fn invalid() -> super::transport::ManagementError {
    error(Code::InvalidResponse).with_delivery(DeliveryEvidence::Uncertain)
}
fn ensure(value: bool) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(invalid())
    }
}
fn parse<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|_| invalid())
}
fn registered(body: &Value) -> Result<BTreeSet<String>> {
    let values = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    ensure(values.len() <= LIMIT)?;
    let mut names = BTreeSet::new();
    for item in values {
        let name = item
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(invalid)?;
        ensure(names.insert(name.to_owned()))?;
    }
    Ok(names)
}
fn options(body: &Value) -> Result<Options> {
    let values = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    ensure(values.len() <= LIMIT)?;
    let mut names = BTreeSet::new();
    let mut selected = BTreeMap::new();
    for value in values {
        let name = value
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        ensure(names.insert(name))?;
        if [RATIO, ALIAS, SPECIAL].contains(&name) {
            selected.insert(
                name,
                value
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?,
            );
        }
    }
    let ratio: Ratios = parse(selected.get(RATIO).ok_or_else(invalid)?)?;
    let alias: Ratios = parse(selected.get(ALIAS).ok_or_else(invalid)?)?;
    let special: Special = parse(selected.get(SPECIAL).ok_or_else(invalid)?)?;
    for map in [&ratio, &alias] {
        ensure(
            map.len() <= LIMIT
                && map
                    .iter()
                    .all(|(k, v)| !k.is_empty() && v.is_finite() && *v >= 0.0),
        )?;
    }
    ensure(special.len() <= LIMIT && special.values().map(BTreeMap::len).sum::<usize>() <= LIMIT)?;
    Ok(Options {
        ratio,
        alias,
        special,
    })
}
fn add_ratio(old: &Options, names: &BTreeSet<String>, group: &str) -> Result<Option<Ratios>> {
    if names.contains(group) {
        return Ok(None);
    }
    ensure(old.ratio == old.alias && old.ratio.keys().cloned().collect::<BTreeSet<_>>() == *names)?;
    ensure(old.ratio.len() < LIMIT)?;
    let mut next = old.ratio.clone();
    next.insert(group.into(), 1.0);
    Ok(Some(next))
}
fn add_permission(old: &Special, account: &str, group: &str) -> Result<Option<Special>> {
    if let Some(scoped) = old.get(account) {
        if scoped.contains_key(&format!("-:{group}")) {
            return Err(error(Code::RemoteRejected));
        }
        if scoped.contains_key(group) || scoped.contains_key(&format!("+:{group}")) {
            return Ok(None);
        }
    }
    ensure(
        old.values().map(BTreeMap::len).sum::<usize>() < LIMIT
            && (old.contains_key(account) || old.len() < LIMIT),
    )?;
    let mut next = old.clone();
    next.entry(account.into())
        .or_default()
        .insert(group.into(), group.into());
    Ok(Some(next))
}

pub struct ManagedGroups<'a> {
    transport: &'a ManagementTransport,
    guard: &'a (dyn Fn() -> bool + Send + Sync),
}
impl<'a> ManagedGroups<'a> {
    pub(super) fn new(
        transport: &'a ManagementTransport,
        guard: &'a (dyn Fn() -> bool + Send + Sync),
    ) -> Self {
        Self { transport, guard }
    }
    async fn request(&self, method: Method, path: &str, payload: Option<Value>) -> Result<Value> {
        let semantics = if method == Method::GET {
            RequestSemantics::Read
        } else {
            RequestSemantics::IdempotentWrite
        };
        let request = ManagementRequest::new(
            method,
            path,
            payload,
            semantics,
            if semantics == RequestSemantics::Read {
                LIST_BODY_LIMIT
            } else {
                SINGLE_BODY_LIMIT
            },
        )?;
        let response = self
            .transport
            .send_fenced(request, RequestClass::Control, self.guard)
            .await?;
        ensure(response.status() == reqwest::StatusCode::OK)?;
        response.envelope()
    }
    pub async fn registered(&self) -> Result<BTreeSet<String>> {
        registered(&self.request(Method::GET, "/api/group/", None).await?)
    }
    async fn read_options(&self) -> Result<Options> {
        options(&self.request(Method::GET, "/api/option/", None).await?)
    }
    async fn put(&self, key: &str, value: &impl serde::Serialize) -> Result<()> {
        let value = serde_json::to_string(value).map_err(|_| invalid())?;
        self.request(
            Method::PUT,
            "/api/option/",
            Some(json!({"key":key,"value":value})),
        )
        .await?;
        Ok(())
    }
    pub async fn register_for(&self, group: &str, account: &str) -> Result<()> {
        // Step T5C.1: 只从已核实一致基线补缺项；未知中间态停止，不猜别名权威。
        if !crate::catalog::types::valid_group(group)
            || account.is_empty()
            || account.len() > 256
            || account.chars().any(char::is_control)
        {
            return Err(error(Code::InvalidInput));
        }
        let names = self.registered().await?;
        let mut old = self.read_options().await?;
        ensure(
            old.ratio == old.alias && old.ratio.keys().cloned().collect::<BTreeSet<_>>() == names,
        )?;
        // 人工deny必须在任何倍率写之前拒绝，不能留下无必要的部分更改。
        let _ = add_permission(&old.special, account, group)?;
        if let Some(next) = add_ratio(&old, &names, group)? {
            self.put(RATIO, &next).await?;
            let between = self.read_options().await?;
            ensure(
                between.ratio == next
                    && between.alias == old.alias
                    && between.special == old.special,
            )?;
            self.put(ALIAS, &next).await?;
            let after = self.read_options().await?;
            ensure(after.ratio == next && after.alias == next && after.special == old.special)?;
            ensure(self.registered().await? == next.keys().cloned().collect())?;
            old = after;
        }
        if let Some(next) = add_permission(&old.special, account, group)? {
            ensure(self.read_options().await? == old)?;
            self.put(SPECIAL, &next).await?;
            let after = self.read_options().await?;
            ensure(after.special == next && after.ratio == old.ratio && after.alias == old.alias)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
