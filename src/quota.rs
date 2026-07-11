//! 智谱用量探针：调 quota/limit API，解析 5 小时 / 每周窗口的已用百分比。
//!
//! ⚠️ 字段名是照着社区脚本（cc-switch issue #1588）推断的，请务必用一把真实 key
//! 手动打一次这个端点，核对返回 JSON 的字段名（success / data.limits[].type /
//! percentage / nextResetTime）。不一致就改下面的 Raw* 结构体。
//!
//! 本模块与 new-api 完全解耦：将来迁到 litellm-rs 或自研网关，这块原样搬走即可。

use crate::config::{Window, ZhipuConfig};
use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct WindowStatus {
    /// 已用百分比，例如 95.0
    pub percentage: f64,
    /// 下次重置时间（智谱返回的原始 epoch 值，通常是毫秒）。
    /// 属于探针输出的一部分，当前控制逻辑未直接读取，留给调用方/日志用。
    #[allow(dead_code)]
    pub next_reset_time: i64,
}

#[derive(Debug, Clone, Default)]
pub struct QuotaStatus {
    pub five_hour: Option<WindowStatus>,
    pub weekly: Option<WindowStatus>,
    /// 套餐档位（如有）。探针输出的一部分，当前控制逻辑未直接读取。
    #[allow(dead_code)]
    pub level: Option<String>,
}

impl QuotaStatus {
    pub fn window(&self, w: Window) -> Option<&WindowStatus> {
        match w {
            Window::FiveHour => self.five_hour.as_ref(),
            Window::Weekly => self.weekly.as_ref(),
        }
    }
}

// ---------- 原始响应结构（宽松解析，未知字段忽略，缺字段给默认值） ----------

#[derive(Debug, Deserialize)]
struct RawResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    msg: Option<String>,
    data: Option<RawData>,
}

#[derive(Debug, Deserialize)]
struct RawData {
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    limits: Vec<RawLimit>,
}

#[derive(Debug, Deserialize)]
struct RawLimit {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    percentage: f64,
    #[serde(rename = "nextResetTime", default)]
    next_reset_time: i64,
}

pub struct QuotaProbe {
    client: reqwest::Client,
    url: String,
    extra_headers: Vec<(String, String)>,
}

impl QuotaProbe {
    pub fn new(cfg: &ZhipuConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: cfg.quota_url.clone(),
            extra_headers: cfg
                .extra_headers
                .iter()
                .map(|h| (h.key.clone(), h.value.clone()))
                .collect(),
        }
    }

    /// 查询某把 key 的用量。Authorization 直接放 key（该端点不是 Bearer 前缀，按社区脚本）。
    /// 团体/企业套餐所需的 org/project selector 走 extra_headers 追加。
    pub async fn query(&self, api_key: &str) -> Result<QuotaStatus> {
        let mut rb = self
            .client
            .get(&self.url)
            .header("Authorization", api_key)
            .header("Content-Type", "application/json");
        for (k, v) in &self.extra_headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        let resp = rb.send().await.context("请求智谱用量 API 失败")?;

        let raw: RawResponse = resp.json().await.context("解析智谱用量响应失败")?;
        if !raw.success {
            anyhow::bail!("智谱用量 API 返回失败: {}", raw.msg.unwrap_or_default());
        }
        let data = raw.data.context("响应缺少 data 字段")?;

        // 只看 TOKENS_LIMIT，按 nextResetTime 升序排：
        // 5 小时窗口重置更快 → nextResetTime 更早 → 排在前；每周窗口排在后。
        let mut token_limits: Vec<RawLimit> = data
            .limits
            .into_iter()
            .filter(|l| l.kind == "TOKENS_LIMIT")
            .collect();
        token_limits.sort_by_key(|l| l.next_reset_time);

        // success=true 但没有任何 TOKENS_LIMIT，多半是团体/企业套餐缺 org/project selector
        // header 导致 limits 为空。这时若静默返回会被当成 0% → 永不切换，故显式告警。
        if token_limits.is_empty() {
            warn!("智谱用量响应 limits 为空（团体套餐？请检查 zhipu.extra_headers 的 org/project selector）");
        }

        let mut status = QuotaStatus {
            level: data.level,
            ..Default::default()
        };
        if let Some(l) = token_limits.first() {
            status.five_hour = Some(WindowStatus {
                percentage: l.percentage,
                next_reset_time: l.next_reset_time,
            });
        }
        if let Some(l) = token_limits.get(1) {
            status.weekly = Some(WindowStatus {
                percentage: l.percentage,
                next_reset_time: l.next_reset_time,
            });
        }
        Ok(status)
    }
}
