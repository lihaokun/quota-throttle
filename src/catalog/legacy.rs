use super::types::*;
use crate::config::Config;
use crate::model_catalog::normalize_models_csv;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt;

pub struct LegacyChannel {
    pub id: i64,
    pub name: String,
    pub group: String,
    pub channel_type: i64,
    pub base_url: String,
    pub models_csv: String,
    pub priority: i64,
}
pub struct LegacyImportPlan {
    pub(super) group: ValidatedGroup,
    pub(super) sources: Vec<(ValidatedSource, ChannelId, i64)>,
    pub(super) digest: String,
}
impl fmt::Debug for LegacyImportPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LegacyImportPlan([REDACTED])")
    }
}

pub fn build_legacy_import(cfg: &Config, channels: &[LegacyChannel]) -> Result<LegacyImportPlan> {
    // Step P6: 多值索引保留名称歧义；明确 ID 不允许重复。
    require(
        !cfg.keys.is_empty() && cfg.keys.len() <= MAX_SOURCES && channels.len() <= MAX_MEMBERSHIPS,
        ErrorKind::InvalidInput,
    )?;
    let mut by_id = HashMap::new();
    let mut by_name: HashMap<&str, Vec<&LegacyChannel>> = HashMap::new();
    for channel in channels {
        ChannelId::new(channel.id)?;
        require(
            by_id.insert(channel.id, channel).is_none(),
            ErrorKind::ChannelConflict,
        )?;
        by_name.entry(&channel.name).or_default().push(channel);
    }
    let mut group_name: Option<String> = None;
    let mut seen_channels = HashSet::new();
    let mut seen_credentials = HashSet::new();
    let mut sources = Vec::new();
    for key in &cfg.keys {
        // Step P7: 显式 ID 优先；无显式 ID 时名字必须恰好匹配一个。
        let channel = if let Some(id) = key.channel_id {
            by_id
                .get(&id)
                .copied()
                .ok_or_else(|| error(ErrorKind::NotFound))?
        } else {
            let candidates = by_name
                .get(key.name.as_str())
                .ok_or_else(|| error(ErrorKind::NotFound))?;
            require(candidates.len() == 1, ErrorKind::ChannelConflict)?;
            candidates[0]
        };
        require(seen_channels.insert(channel.id), ErrorKind::ChannelConflict)?;
        // Step P8: 一个旧组；实际模型优先，模板只能提供匹配的基础配置。
        require(
            valid_group(&channel.group) && safe_number(channel.priority),
            ErrorKind::InvalidInput,
        )?;
        if let Some(name) = &group_name {
            require(name == &channel.group, ErrorKind::ImportConflict)?;
        } else {
            group_name = Some(channel.group.clone());
        }
        if let Some(template) = &cfg.new_api.channel_template {
            require(
                template.group == channel.group
                    && template.channel_type == channel.channel_type
                    && template.base_url == channel.base_url,
                ErrorKind::ImportConflict,
            )?;
        }
        // Step P9: 保留原顺序、全局和逐 key 的 selector，不合并请求 header。
        require(
            channel.models_csv.split(',').take(1001).count() <= 1000
                && cfg.zhipu.extra_headers.len() <= 64
                && key.quota_headers.len() <= 64,
            ErrorKind::InvalidInput,
        )?;
        let source = validate_source(SourceDraft {
            name: key.name.clone(),
            note: key.note.clone(),
            credential: SecretString::new(key.zhipu_api_key.clone())?,
            settings: SourceSettings::Glm {
                upstream: UpstreamProfile {
                    channel_type: channel.channel_type,
                    base_url: channel.base_url.clone(),
                    models: normalize_models_csv(&channel.models_csv),
                    model_discovery: cfg
                        .new_api
                        .channel_template
                        .as_ref()
                        .and_then(|t| t.model_discovery.clone()),
                },
                quota: GlmQuotaProfile {
                    quota_url: cfg.zhipu.quota_url.clone(),
                    global_headers: cfg.zhipu.extra_headers.clone(),
                    key_headers: key.quota_headers.clone(),
                },
            },
        })?;
        require(
            seen_credentials.insert(source.fingerprint.clone()),
            ErrorKind::DuplicateSource,
        )?;
        sources.push((source, ChannelId::new(channel.id)?, channel.priority));
    }
    let group_name = group_name.ok_or_else(|| error(ErrorKind::InvalidInput))?;
    let group = validate_group(GroupDraft {
        name: group_name.clone(),
        newapi_group: group_name,
        kind: SourceKind::Glm,
    })?;
    // Step P10: 固定顺序序列化规范内容；不包含路径/时钟/查询返回顺序。
    let rows: Vec<_> = sources
        .iter()
        .map(|(s, id, priority)| {
            (
                &s.draft.name,
                &s.draft.note,
                &s.draft.settings,
                &s.fingerprint,
                id,
                priority,
            )
        })
        .collect();
    let canonical = serde_json::to_vec(&(&group.0.newapi_group, rows))
        .map_err(|_| error(ErrorKind::Storage))?;
    Ok(LegacyImportPlan {
        group,
        sources,
        digest: format!("{:x}", Sha256::digest(&canonical)),
    })
}
