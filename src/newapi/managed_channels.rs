//! 受管渠道原生接口：严格读取、禁用创建、有限字段更新；回执不是协调完成。
use super::transport::{
    error, ManagementErrorKind, ManagementRequest, ManagementTransport, RequestClass,
    RequestSemantics, Result, LIST_BODY_LIMIT, SINGLE_BODY_LIMIT,
};
use crate::catalog::{SecretString, SourceKind};
use crate::model_catalog::{model_sets_equal, normalize_models_csv};
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::SystemTime;

#[derive(Clone)]
pub struct ChannelObservation {
    pub observed_at: SystemTime,
    pub channel_id: i64,
    pub channel_type: i64,
    pub name: String,
    pub group: String,
    pub base_url: String,
    pub models: String,
    pub priority: i64,
    pub status: i64,
    pub auto_ban: i64,
    pub is_multi_key: bool,
}
/// 不实现 Debug/Serialize；未知字段（可能含 override 凭据）只用于一次安全更新。
pub struct RawChannelDocument(Value, SystemTime);
impl RawChannelDocument {
    fn new(value: Value) -> Result<Self> {
        let doc = Self(value, SystemTime::now());
        doc.observation()?;
        Ok(doc)
    }
    pub fn observation(&self) -> Result<ChannelObservation> {
        let value = &self.0;
        let id = int(value, "id")?;
        let channel_type = int(value, "type")?;
        let status = int(value, "status")?;
        let auto_ban = int(value, "auto_ban")?;
        let multi = value
            .get("channel_info")
            .and_then(|v| v.get("is_multi_key"))
            .and_then(Value::as_bool)
            .ok_or_else(invalid)?;
        if id <= 0
            || channel_type <= 0
            || !(1..=3).contains(&status)
            || !(0..=1).contains(&auto_ban)
        {
            return Err(invalid());
        }
        Ok(ChannelObservation {
            observed_at: self.1,
            channel_id: id,
            channel_type,
            name: string(value, "name")?,
            group: string(value, "group")?,
            base_url: string(value, "base_url")?,
            models: string(value, "models")?,
            priority: int(value, "priority")?,
            status,
            auto_ban,
            is_multi_key: multi,
        })
    }
    fn patch(mut self, patch: &ChannelPatch) -> Result<Option<Value>> {
        let before = self.observation()?;
        let object = self.0.as_object_mut().ok_or_else(invalid)?;
        let mut changed = false;
        if let Some(priority) = patch.priority {
            if !(-crate::catalog::JSON_SAFE..=crate::catalog::JSON_SAFE).contains(&priority) {
                return Err(error(ManagementErrorKind::InvalidInput));
            }
            if priority != before.priority {
                object.insert("priority".into(), json!(priority));
                changed = true;
            }
        }
        if let Some(models) = &patch.models {
            let normalized = normalize_models_csv(models);
            if normalized.is_empty() {
                return Err(error(ManagementErrorKind::InvalidInput));
            }
            if !model_sets_equal(&before.models, models) {
                object.insert("models".into(), json!(normalized.join(",")));
                changed = true;
            }
        }
        if patch.disable_auto_ban && before.auto_ban != 0 {
            object.insert("auto_ban".into(), json!(0));
            changed = true;
        }
        object.remove("status");
        object.remove("key");
        Ok(changed.then_some(self.0))
    }
}
#[derive(Default, Clone, PartialEq, Eq)]
pub struct ChannelPatch {
    pub priority: Option<i64>,
    pub models: Option<String>,
    pub disable_auto_ban: bool,
}
pub struct DisabledChannelSpec<'a> {
    pub name: &'a str,
    pub source_kind: SourceKind,
    pub channel_type: i64,
    pub key: &'a SecretString,
    pub base_url: &'a str,
    pub models: &'a str,
    pub group: &'a str,
    pub standby_priority: i64,
}
pub struct ChannelScan {
    pub channels: Vec<RawChannelDocument>,
    pub complete: bool,
    pub total: usize,
}

/// 仅自动恢复配置前提，不冒充实例身份、组权限、策略许可均已就绪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeRecoveryCheck {
    Disabled,
    Enabled,
    Unknown,
}

pub struct ManagedChannels<'a> {
    transport: &'a ManagementTransport,
    guard: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
}
impl<'a> ManagedChannels<'a> {
    pub fn new(transport: &'a ManagementTransport) -> Self {
        Self {
            transport,
            guard: None,
        }
    }

    pub fn fenced<'b>(
        &'b self,
        guard: &'b (dyn Fn() -> bool + Send + Sync),
    ) -> ManagedChannels<'b> {
        ManagedChannels {
            transport: self.transport,
            guard: Some(guard),
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        semantics: RequestSemantics,
        class: RequestClass,
        limit: usize,
    ) -> Result<Value> {
        let request = ManagementRequest::new(method, path, body, semantics, limit)?;
        let response = match self.guard {
            Some(guard) => self.transport.send_fenced(request, class, guard).await?,
            None => self.transport.send(request, class).await?,
        };
        response.envelope()
    }

    pub async fn get_channel(&self, id: i64) -> Result<RawChannelDocument> {
        if id <= 0 {
            return Err(error(ManagementErrorKind::InvalidInput));
        }
        let body = self
            .request(
                Method::GET,
                &format!("/api/channel/{id}"),
                None,
                RequestSemantics::Read,
                RequestClass::Control,
                SINGLE_BODY_LIMIT,
            )
            .await?;
        let doc = RawChannelDocument::new(body.get("data").cloned().ok_or_else(invalid)?)?;
        if doc.observation()?.channel_id != id {
            return Err(invalid());
        }
        Ok(doc)
    }

    /// None 要求完整扫描；Some 返回的 partial 仅覆盖明确列出的 required_ids。
    pub async fn scan_channels(&self, required_ids: Option<&HashSet<i64>>) -> Result<ChannelScan> {
        if required_ids
            .is_some_and(|ids| ids.is_empty() || ids.len() > 10000 || ids.iter().any(|id| *id <= 0))
        {
            return Err(error(ManagementErrorKind::InvalidInput));
        }
        let mut scan = ScanAccumulator::default();
        // Step P4: p=1 起，有界分页，按稳定 ID 升序，严格校验总数/页码/重复 ID。
        for page in 1..=101 {
            let body = self
                .request(
                    Method::GET,
                    &format!("/api/channel/?p={page}&page_size=100&sort_by=id&sort_order=asc"),
                    None,
                    RequestSemantics::Read,
                    RequestClass::Observe,
                    LIST_BODY_LIMIT,
                )
                .await?;
            let complete = scan.append(page, body.get("data").ok_or_else(invalid)?)?;
            // Step P5: 只在全量完成或所需 ID 全覆盖时退出；不能把首屏缺失当不存在。
            let covered = required_ids.is_some_and(|ids| ids.is_subset(&scan.ids));
            if complete || covered {
                return Ok(ChannelScan {
                    channels: scan.channels,
                    complete,
                    total: scan.total.unwrap(),
                });
            }
        }
        Err(invalid())
    }

    pub async fn create_disabled_channel(&self, spec: DisabledChannelSpec<'_>) -> Result<()> {
        // Step P6: 单 key、单组、固定禁用与备用优先级；创建后不凭 ACK 认领 ID。
        let payload = creation_payload(spec)?;
        self.request(
            Method::POST,
            "/api/channel/",
            Some(payload),
            RequestSemantics::Create,
            RequestClass::Control,
            SINGLE_BODY_LIMIT,
        )
        .await?;
        Ok(())
    }

    /// true 仅表示写请求得到明确回执；协调器仍必须按本代次逐字段回读。
    pub async fn write_channel_fields(&self, id: i64, patch: ChannelPatch) -> Result<bool> {
        let document = self.get_channel(id).await?;
        if document.observation()?.is_multi_key {
            return Err(error(ManagementErrorKind::InvalidInput));
        }
        let Some(body) = document.patch(&patch)? else {
            return Ok(false);
        };
        self.request(
            Method::PUT,
            "/api/channel/",
            Some(body),
            RequestSemantics::IdempotentWrite,
            RequestClass::Control,
            SINGLE_BODY_LIMIT,
        )
        .await?;
        Ok(true)
    }

    pub async fn write_status(&self, ids: &[i64], enabled: bool) -> Result<()> {
        // Step P7: 状态专用接口，批量不超过 100；changed 不作为目标完成数量。
        if ids.is_empty() || ids.len() > 100 || ids.iter().any(|id| *id <= 0) {
            return Err(error(ManagementErrorKind::InvalidInput));
        }
        let unique: HashSet<_> = ids.iter().copied().collect();
        let mut ids: Vec<_> = unique.into_iter().collect();
        ids.sort_unstable();
        let status = if enabled { 1 } else { 2 };
        let (path, body) = if ids.len() == 1 {
            (
                format!("/api/channel/{}/status", ids[0]),
                json!({"status":status}),
            )
        } else {
            (
                "/api/channel/status/batch".into(),
                json!({"ids":ids,"status":status}),
            )
        };
        self.request(
            Method::POST,
            &path,
            Some(body),
            RequestSemantics::IdempotentWrite,
            RequestClass::Control,
            SINGLE_BODY_LIMIT,
        )
        .await?;
        Ok(())
    }

    pub async fn check_native_recovery(&self) -> Result<NativeRecoveryCheck> {
        let body = self
            .request(
                Method::GET,
                "/api/option/",
                None,
                RequestSemantics::Read,
                RequestClass::Control,
                LIST_BODY_LIMIT,
            )
            .await?;
        Ok(parse_recovery_option(&body))
    }
}

#[derive(Default)]
struct ScanAccumulator {
    channels: Vec<RawChannelDocument>,
    ids: HashSet<i64>,
    total: Option<usize>,
    last_id: i64,
}
impl ScanAccumulator {
    fn append(&mut self, page: usize, data: &Value) -> Result<bool> {
        let total = data
            .get("total")
            .and_then(Value::as_u64)
            .filter(|n| *n <= 10000)
            .ok_or_else(invalid)? as usize;
        if data.get("page").and_then(Value::as_u64) != Some(page as u64)
            || data.get("page_size").and_then(Value::as_u64) != Some(100)
            || self.total.is_some_and(|previous| previous != total)
        {
            return Err(invalid());
        }
        let items = data
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        if items.len() != total.saturating_sub(self.channels.len()).min(100) {
            return Err(invalid());
        }
        self.total = Some(total);
        for value in items {
            let doc = RawChannelDocument::new(value.clone())?;
            let id = doc.observation()?.channel_id;
            if id <= self.last_id || !self.ids.insert(id) {
                return Err(invalid());
            }
            self.last_id = id;
            self.channels.push(doc);
        }
        Ok(self.channels.len() == total)
    }
}

fn creation_payload(spec: DisabledChannelSpec<'_>) -> Result<Value> {
    let models = normalize_models_csv(spec.models);
    if spec.name.is_empty()
        || spec.name.len() > 128
        || spec.channel_type <= 0
        || models.is_empty()
        || spec.group.is_empty()
        || spec.group.contains(',')
        || spec.group.trim() != spec.group
        || spec.key.expose().is_empty()
        || spec.base_url.is_empty()
        || !(-crate::catalog::JSON_SAFE..=crate::catalog::JSON_SAFE)
            .contains(&spec.standby_priority)
    {
        return Err(error(ManagementErrorKind::InvalidInput));
    }
    let mut channel = json!({"name":spec.name,"type":spec.channel_type,"key":spec.key.expose(),
        "base_url":spec.base_url,"models":models.join(","),"group":spec.group,
        "priority":spec.standby_priority,"weight":0,"status":2});
    if spec.source_kind == SourceKind::Volcano {
        channel["auto_ban"] = json!(0);
    }
    Ok(json!({"mode":"single","channel":channel}))
}
fn parse_recovery_option(body: &Value) -> NativeRecoveryCheck {
    let Some(items) = body.get("data").and_then(Value::as_array) else {
        return NativeRecoveryCheck::Unknown;
    };
    let values: Vec<_> = items
        .iter()
        .filter(|v| v.get("key").and_then(Value::as_str) == Some("AutomaticEnableChannelEnabled"))
        .collect();
    if values.len() != 1 {
        return NativeRecoveryCheck::Unknown;
    }
    match values[0].get("value").and_then(Value::as_str) {
        Some("false") => NativeRecoveryCheck::Disabled,
        Some("true") => NativeRecoveryCheck::Enabled,
        _ => NativeRecoveryCheck::Unknown,
    }
}
fn invalid() -> super::transport::ManagementError {
    error(ManagementErrorKind::InvalidResponse)
}
fn int(value: &Value, key: &str) -> Result<i64> {
    value.get(key).and_then(Value::as_i64).ok_or_else(invalid)
}
fn string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(invalid)
}

#[cfg(test)]
mod tests;
