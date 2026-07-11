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
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// 管理认证优先用这个“系统访问令牌”；留空则用下面的 root 账号自动登录拿会话。
    #[serde(default)]
    pub admin_token: String,
    /// admin_token 为空时用它登录 new-api（首启默认 root/123456）。
    #[serde(default = "default_root_user")]
    pub root_username: String,
    #[serde(default = "default_root_pass")]
    pub root_password: String,
    /// 渠道管理路径。用 F12 核实你的版本，默认 /api/channel
    #[serde(default = "default_channel_path")]
    pub channel_path: String,
    /// 部分版本的管理 API 需要额外 header（如 New-Api-User: <管理员 user id>）。
    #[serde(default)]
    pub extra_headers: Vec<HeaderKV>,
    /// 由本工具下载并托管 new-api 进程（选项二：零前置一键起）。
    #[serde(default)]
    pub manage: Option<ManageConfig>,
    /// sync 建渠道用的模板（版本相关字段，F12 对齐）。缺省则不自动建渠道，只按 name 解析已有渠道。
    #[serde(default)]
    pub channel_template: Option<ChannelTemplate>,
}

fn default_base_url() -> String {
    "http://127.0.0.1:3000".to_string()
}
fn default_root_user() -> String {
    "root".to_string()
}
fn default_root_pass() -> String {
    // 新版 new-api 首启要求密码 ≥8 位；本工具首启会用它创建管理员。建议改成你自己的。
    "changeme123".to_string()
}
fn default_channel_path() -> String {
    "/api/channel".to_string()
}

/// 让本工具自己下载 new-api release 二进制并作为原生进程托管。
#[derive(Debug, Clone, Deserialize)]
pub struct ManageConfig {
    /// GitHub release tag，例如 v1.0.0-rc.20
    #[serde(default = "default_newapi_version")]
    pub version: String,
    /// 监听端口（应与 base_url 里的端口一致）
    #[serde(default = "default_newapi_port")]
    pub port: u16,
    /// 存放二进制 / SQLite / 日志 / PID 的目录
    #[serde(default = "default_newapi_data_dir")]
    pub data_dir: String,
    /// GitHub 仓库，默认官方 new-api
    #[serde(default = "default_newapi_repo")]
    pub repo: String,
}

fn default_newapi_version() -> String {
    "v1.0.0-rc.20".to_string()
}
fn default_newapi_port() -> u16 {
    3000
}
fn default_newapi_data_dir() -> String {
    "./.newapi".to_string()
}
fn default_newapi_repo() -> String {
    "QuantumNous/new-api".to_string()
}

/// 建渠道模板：sync 时把每把 key 的 name/key/priority 合并进来 POST /api/channel。
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelTemplate {
    /// 渠道类型码（F12 核实；智谱可用 OpenAI 兼容或专用类型）
    #[serde(rename = "type", default = "default_channel_type")]
    pub channel_type: i64,
    /// 智谱上游 base_url，例如 https://open.bigmodel.cn/api/paas/v4
    pub base_url: String,
    /// 逗号分隔的模型名，例如 "glm-4.6,glm-4.5"
    pub models: String,
    /// 分组名
    #[serde(default = "default_group")]
    pub group: String,
}

fn default_channel_type() -> i64 {
    1
}
fn default_group() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeaderKV {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyMapping {
    /// 便于日志辨认，例如 zhipu-1；sync 时也作为 new-api 渠道名，用于解析 channel_id
    pub name: String,
    /// 这把智谱 key（探针直接拿它调智谱用量 API；sync 建渠道时也用它）
    pub zhipu_api_key: String,
    /// 这把 key 在 new-api 里对应的渠道 id。可留空，交给 sync 按 name 自动解析/创建。
    #[serde(default)]
    pub channel_id: Option<i64>,
}

/// channel_id 解析完成后的可用条目（orchestrator 直接用它）。
#[derive(Debug, Clone)]
pub struct ResolvedKey {
    pub name: String,
    pub zhipu_api_key: String,
    pub channel_id: i64,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&text)?;
        Ok(cfg)
    }
}
