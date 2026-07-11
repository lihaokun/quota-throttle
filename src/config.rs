//! 配置加载。所有可能随 new-api 版本变化的东西（路径、header）都放到配置里，
//! 不写死在代码，方便你 F12 抓到真实接口后直接改。

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// 轮询间隔（秒）
    pub poll_interval_secs: u64,

    /// 活动 key 的最大窗口用量达到这个百分比 → 切换到下一把有额度的 key。
    /// 同时也是「有额度」判定线：pct < throttle 才算还能用。
    #[serde(default = "default_throttle")]
    pub throttle_threshold: f64,

    /// 挑「新活动 key」时要求 pct < 这个值（比 throttle 低，多留余量，
    /// 让新活动 key 能撑更久 → 切换更少 → 缓存局部性更好）。
    #[serde(default = "default_restore")]
    pub restore_threshold: f64,

    /// 空跑模式：只打印决策，不真的调用 new-api。先用它验证逻辑。
    #[serde(default)]
    pub dry_run: bool,

    /// 监控哪些窗口。默认同时看 5 小时和每周（取最大使用率）。
    #[serde(default = "default_windows")]
    pub watch_windows: Vec<Window>,

    /// 三档 priority 的取值。new-api 优先路由最高 priority 的渠道；
    /// active 独占最高档 → 所有正常流量都走它；standby 作 429 兜底；
    /// exhausted 是最后手段。一般无需改。
    #[serde(default = "default_p_active")]
    pub priority_active: i64,
    #[serde(default = "default_p_standby")]
    pub priority_standby: i64,
    #[serde(default = "default_p_exhausted")]
    pub priority_exhausted: i64,

    pub zhipu: ZhipuConfig,
    pub new_api: NewApiConfig,
    pub keys: Vec<KeyMapping>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    FiveHour,
    Weekly,
}

fn default_throttle() -> f64 {
    95.0
}
fn default_restore() -> f64 {
    90.0
}
fn default_windows() -> Vec<Window> {
    vec![Window::FiveHour, Window::Weekly]
}
fn default_p_active() -> i64 {
    100
}
fn default_p_standby() -> i64 {
    10
}
fn default_p_exhausted() -> i64 {
    0
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZhipuConfig {
    /// 智谱用量查询端点。国内版默认如下；国际版 z.ai 换成对应地址。
    #[serde(default = "default_quota_url")]
    pub quota_url: String,
    /// 团体/企业套餐通常要额外的 selector header（如 Bigmodel-Organization /
    /// Bigmodel-Project），否则接口可能返回 success 但 limits 为空 → 用量恒为 0、永不切换。
    /// 用 F12 抓一次浏览器里的真实请求核实 header 名再填。个人套餐一般留空即可。
    #[serde(default)]
    pub extra_headers: Vec<HeaderKV>,
}

fn default_quota_url() -> String {
    "https://open.bigmodel.cn/api/monitor/usage/quota/limit".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewApiConfig {
    /// 例如 http://127.0.0.1:3000
    pub base_url: String,
    /// new-api 后台【个人设置】里生成的“系统访问令牌”
    pub admin_token: String,
    /// 渠道管理路径。用 F12 核实你的版本，默认 /api/channel
    #[serde(default = "default_channel_path")]
    pub channel_path: String,
    /// 部分版本的管理 API 需要额外 header（如 New-Api-User: <管理员 user id>），
    /// 用 F12 抓一次确认后填这里。
    #[serde(default)]
    pub extra_headers: Vec<HeaderKV>,
}

fn default_channel_path() -> String {
    "/api/channel".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeaderKV {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyMapping {
    /// 便于日志辨认，例如 zhipu-1
    pub name: String,
    /// 这把智谱 key（探针直接拿它调智谱用量 API）
    pub zhipu_api_key: String,
    /// 这把 key 在 new-api 里对应的渠道 id
    pub channel_id: i64,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&text)?;
        Ok(cfg)
    }
}
