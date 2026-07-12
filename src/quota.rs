//! 智谱用量探针：调 quota/limit API，解析 5 小时 / 每周窗口的已用百分比。
//!
//! ⚠️ 字段名是照着社区脚本（cc-switch issue #1588）推断的，请务必用一把真实 key
//! 手动打一次这个端点，核对返回 JSON 的字段名（success / data.limits[].type /
//! percentage / nextResetTime）。不一致就改下面的 Raw* 结构体。
//!
//! 本模块与 new-api 完全解耦：将来迁到 litellm-rs 或自研网关，这块原样搬走即可。

use crate::config::{HeaderKV, Window, ZhipuConfig};
use anyhow::{Context, Result};
use serde::Deserialize;

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
    /// 时间单位枚举（智谱：3=小时，6=周），配合 number 精确区分窗口。
    #[serde(default)]
    unit: Option<i64>,
    /// 单位数量（如 5 小时的 5、1 周的 1）。
    #[serde(default)]
    number: Option<i64>,
}

impl RawLimit {
    fn to_window(&self) -> WindowStatus {
        WindowStatus {
            percentage: self.percentage,
            next_reset_time: self.next_reset_time,
        }
    }
}

pub struct QuotaProbe {
    client: reqwest::Client,
    url: String,
    /// 全局兜底 selector header（各 key 未单独配置时用）
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

    /// 查询单把 key 的 5 小时 / 每周窗口已用百分比。
    ///
    /// 鉴权：`Authorization: Bearer <key>`（**必须带 Bearer**）。
    /// 团体套餐还必须满足两点，否则返回「当前用户不存在coding plan」：
    ///   1. url 带 `?type=2`（团队额度作用域）；
    ///   2. 带 selector header：Bigmodel-Organization / Bigmodel-Project。
    /// selector 按 key 传入（不同 key 可能属不同组织/项目）；为空则回退全局 extra_headers。
    pub async fn query(&self, api_key: &str, key_headers: &[HeaderKV]) -> Result<QuotaStatus> {
        let mut rb = self
            .client
            .get(&self.url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("User-Agent", "quota-throttle/0.1");
        // 先全局兜底，再 per-key 覆盖
        for (k, v) in &self.extra_headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        for h in key_headers {
            rb = rb.header(h.key.as_str(), h.value.as_str());
        }
        let resp = rb.send().await.context("请求智谱用量 API 失败")?;

        let raw: RawResponse = resp.json().await.context("解析智谱用量响应失败")?;
        if !raw.success {
            anyhow::bail!("智谱用量 API 返回失败: {}", raw.msg.unwrap_or_default());
        }
        let data = raw.data.context("响应缺少 data 字段")?;

        // 只看 TOKENS_LIMIT（TIME_LIMIT 是 MCP 搜索次数，不是用量窗口）。
        let token_limits: Vec<RawLimit> = data
            .limits
            .into_iter()
            .filter(|l| l.kind == "TOKENS_LIMIT")
            .collect();

        // success=true 但没有任何 TOKENS_LIMIT ⇒ **必须报错，绝不能返回空 status**。
        //
        // 这是本项目最危险的一条路径：空 status 的 max_watch_pct 会算出 **0.0**，于是一把
        // selector 配错的 key 在调度器眼里就是「用量 0%」——它会被选成活动 key 并且**永远
        // 不会被切走**，直到线上真的撞墙才发现。而智谱对这种情况 **success=true、不报错**
        // （url 缺 `?type=2`、或缺 Bigmodel-Organization / Bigmodel-Project selector 时，
        // 它只是安静地回一个空 data）。
        //
        // 报错的后果是安全的：该 key 本轮「查询失败」⇒ 不参与决策、不动它的 priority、
        // 看板显示「查询失败」而不是骗你说 0%。
        if token_limits.is_empty() {
            anyhow::bail!(
                "智谱返回了 success 但没有任何用量窗口（limits 为空）。团体套餐请确认：\
                 ① quota_url 带 ?type=2；② 这把 key 配了 Bigmodel-Organization / \
                 Bigmodel-Project selector（不同 key 可能属不同组织/项目，须按 key 配）"
            );
        }

        let mut status = QuotaStatus {
            level: data.level,
            ..Default::default()
        };

        // 首选：按智谱返回的 unit/number 精确分窗（unit=3&number=5 → 5小时；unit=6&number=1 → 每周）。
        let mut classified = false;
        for l in &token_limits {
            match (l.unit, l.number) {
                (Some(3), Some(5)) => {
                    status.five_hour = Some(l.to_window());
                    classified = true;
                }
                (Some(6), Some(1)) => {
                    status.weekly = Some(l.to_window());
                    classified = true;
                }
                _ => {}
            }
        }

        // 回退：老版本/字段缺失时，照 nextResetTime 升序猜（5小时重置更早 → 排前）。
        if !classified && !token_limits.is_empty() {
            let mut sorted = token_limits;
            sorted.sort_by_key(|l| l.next_reset_time);
            if let Some(l) = sorted.first() {
                status.five_hour = Some(l.to_window());
            }
            if let Some(l) = sorted.get(1) {
                status.weekly = Some(l.to_window());
            }
        }
        Ok(status)
    }
}
