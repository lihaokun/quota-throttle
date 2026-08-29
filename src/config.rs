//! 配置加载。所有可能随 new-api 版本变化的东西（路径、header）都放到配置里，
//! 不写死在代码，方便你 F12 抓到真实接口后直接改。

use anyhow::{bail, Context};
use serde::Deserialize;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// 本配置文件的路径。面板加/删 key 时要写回它（**config.toml 是唯一数据源**）。
    #[serde(skip)]
    pub source_path: String,

    /// 轮询间隔（秒）
    pub poll_interval_secs: u64,

    /// **预防线**：活动 key 的最大窗口用量达到这个百分比 → 在撞墙前切到下一把。
    /// 正常情况下的「还能用」判定线：pct < throttle 才会被选作活动 key。
    #[serde(default = "default_throttle")]
    pub throttle_threshold: f64,

    /// 挑「新活动 key」时要求 pct < 这个值（比 throttle 低，多留余量，
    /// 让新活动 key 能撑更久 → 切换更少 → 缓存局部性更好）。
    #[serde(default = "default_restore")]
    pub restore_threshold: f64,

    /// **真·用尽线**：这把 key 真的没余量了。与 throttle 是两条不同的线——
    /// throttle(95%) 是「还有余量但该提前换了」，exhausted(100%) 是「物理上没了」。
    /// 全部 key 都过了预防线时（降级档），判定放宽到这条线：只要 pct < exhausted
    /// 就还能用，避免明知有余量还去撞 429。
    /// ⚠️ 智谱的 TOKENS_LIMIT 只返回**整数** percentage（无 usage/remaining），
    /// 所以这里的分辨率就是 1%，取 100 意味着「榨到智谱自己报 100 为止」。
    #[serde(default = "default_exhausted")]
    pub exhausted_threshold: f64,

    /// 空跑模式：只打印决策，不真的调用 new-api。先用它验证逻辑。
    #[serde(default)]
    pub dry_run: bool,

    /// 状态看板监听地址。留空则不启用看板（只跑切换循环）。
    /// 看板是附属功能：监听失败只降级记 error，绝不影响切换。
    #[serde(default = "default_status_addr")]
    pub status_addr: String,

    /// 看板**面板数据**的刷新间隔（秒）。new-api 是**本地**服务（毫秒级纯读），可以高频；
    /// 与智谱用量轮询 `poll_interval_secs` **分离**——后者是外部 API，该低频。
    #[serde(default = "default_panel_interval")]
    pub panel_interval_secs: u64,

    /// 周窗口重置进入这个时间窗（小时）内的 key 视为「临期」：切换时优先选它
    /// （先烧快清零的额度，最大化使用效率）。0 = 关闭本策略（行为与旧版完全一致）。
    /// 建议 6–48h；上限 7 天（周窗口最长一周，再大等于全年轮询）。
    #[serde(default = "default_weekly_lookahead")]
    pub weekly_reset_lookahead_hours: u64,

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

    /// 高峰时段（智谱自己的概念）。缺省则看板不显示这块。
    #[serde(default)]
    pub peak: Option<PeakConfig>,
}

/// 智谱的**高峰时段扣减系数**。这不是限额，是「同一个请求在高峰期烧掉几倍额度」。
///
/// 依据（2026-07 官方文档交叉验证：coding-plan/faq + coding-plan/overview）：
///   · 高峰期 = **每日 14:00–18:00（UTC+8）**，固定，不随流量浮动。
///   · GLM-5.2 / GLM-5-Turbo：高峰 **3 倍**，非高峰 **2 倍**；
///     限时福利——非高峰仅 **1 倍**，**持续到 9 月底**（到期后要把 off_peak 改回 2.0）。
///   · GLM-4.7 等普通模型：1 倍。
///
/// ⚠️ 智谱**没有任何接口**能查「现在是不是高峰」（quota/limit 的响应里没有这个字段，
/// 官方文档也没有该接口）。所以只能按时钟算——好在窗口是固定的，纯函数即可。
#[derive(Debug, Clone, Deserialize)]
pub struct PeakConfig {
    #[serde(default = "default_peak_start")]
    pub start_hour: i64,
    #[serde(default = "default_peak_end")]
    pub end_hour: i64,
    /// 高峰窗口是按 **UTC+8** 定义的。**不要用本机时区**——换台机器就错了。
    #[serde(default = "default_peak_tz")]
    pub tz_offset_hours: i64,
    /// 看板上显示的备注（如福利到期日）
    #[serde(default)]
    pub note: String,
    /// 受系数影响的模型；未列出的模型一律按 1 倍，不展示。
    #[serde(default)]
    pub coefficients: Vec<PeakCoefficient>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PeakCoefficient {
    pub model: String,
    pub peak: f64,
    pub off_peak: f64,
}

fn default_peak_start() -> i64 {
    14
}
fn default_peak_end() -> i64 {
    18
}
fn default_peak_tz() -> i64 {
    8
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    FiveHour,
    Weekly,
}

fn default_status_addr() -> String {
    "127.0.0.1:3001".to_string()
}
fn default_panel_interval() -> u64 {
    5
}
fn default_throttle() -> f64 {
    95.0
}
fn default_restore() -> f64 {
    90.0
}
fn default_exhausted() -> f64 {
    100.0
}
fn default_weekly_lookahead() -> u64 {
    24
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
    /// 智谱用量查询端点。
    /// ⚠️ 团体套餐必须带 `?type=2`（团队额度作用域）——不带会返回「当前用户不存在coding plan」。
    /// 个人套餐去掉该查询参数即可。国际版 z.ai 换成对应主机。
    #[serde(default = "default_quota_url")]
    pub quota_url: String,

    /// 全局默认 selector header（各 key 未单独配置时的兜底）。
    /// 多把 key 同组织/项目时只写一遍即可。
    #[serde(default)]
    pub extra_headers: Vec<HeaderKV>,
}

fn default_quota_url() -> String {
    "https://open.bigmodel.cn/api/monitor/usage/quota/limit?type=2".to_string()
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
    /// Claude Code（Anthropic 格式）渠道模板。配了它 = 开启双渠道：每把 key 额外挂一个
    /// `<name>-cc` 的 type 14 渠道（智谱 anthropic 口），Claude Code 用专用 group + 令牌接入。
    /// 不配 = 行为与现状完全一致。
    #[serde(default)]
    pub channel_template_claude: Option<ClaudeChannelTemplate>,
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
    // 8 = Custom：原样透传 base_url 全路径。智谱 coding 口 /v4/chat/completions
    // 用 OpenAI 类型(1) 会被拼成 /v4/v1/... 而 404，故默认 Custom。
    8
}
fn default_group() -> String {
    "default".to_string()
}

/// Claude Code 渠道模板：每把 key 额外建一个 Anthropic 格式渠道（type 14），
/// 上游指到智谱的 Claude 兼容口。
///
/// 与 opencode 的 OpenAI 格式渠道靠 **group 物理隔离**：new-api 分发器按 (group, model)
/// 选渠道、**不按请求格式过滤**，同 group 混挂两种格式渠道会互抢流量做跨格式转换
/// （调研 docs/research/claude-code-routing-research.md §2）。三条不变量在 `validate()` 拦截。
#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeChannelTemplate {
    /// new-api 渠道类型码。14 = Anthropic Claude（constant/channel.go），
    /// 上游 URL 自动拼 `{base_url}/v1/messages`（所以 base_url 不带 /v1/messages）。
    #[serde(rename = "type", default = "default_claude_channel_type")]
    pub channel_type: i64,
    /// 智谱 Claude 兼容口（官方文档）：https://open.bigmodel.cn/api/anthropic
    #[serde(default = "default_claude_base_url")]
    pub base_url: String,
    /// 逗号分隔模型名。⚠️ 必须含 `[1m]` 后缀变体——Claude Code 配了
    /// `ANTHROPIC_DEFAULT_*_MODEL=glm-5.3[1m]` 时实际发出的就是带后缀的名字（实测基线）。
    #[serde(default = "default_claude_models")]
    pub models: String,
    /// 专用分组。⚠️ 必须与 [new_api.channel_template].group 不同（validate 拦截）。
    #[serde(default = "default_claude_group")]
    pub group: String,
    /// 渠道名后缀：claude 渠道名 = `<key name> + name_suffix`，sync 按名幂等匹配用。
    #[serde(default = "default_claude_suffix")]
    pub name_suffix: String,
    /// CC 专用令牌名（绑定 claude group、无限额度；sync 时自动创建并打印完整 key）。
    #[serde(default = "default_claude_token_name")]
    pub token_name: String,
}

impl ClaudeChannelTemplate {
    /// 这把 key 对应的 claude 渠道名。
    pub fn channel_name(&self, key_name: &str) -> String {
        format!("{key_name}{}", self.name_suffix)
    }
}

fn default_claude_channel_type() -> i64 {
    14
}
fn default_claude_base_url() -> String {
    "https://open.bigmodel.cn/api/anthropic".to_string()
}
fn default_claude_models() -> String {
    // 与 OpenAI 模板同源（models.dev），外加 [1m] 1M 上下文变体（Claude Code 实际会发的名字）
    "glm-4.7,glm-5.1,glm-5.2,glm-5.2[1m],glm-5.3,glm-5.3[1m],glm-5-turbo,glm-5v-turbo,glm-4.6v,glm-4.5-air"
        .to_string()
}
fn default_claude_group() -> String {
    "claude".to_string()
}
fn default_claude_suffix() -> String {
    "-cc".to_string()
}
fn default_claude_token_name() -> String {
    "claude-code".to_string()
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

    /// 人类可读的备注（如持有人名字），只在看板显示，不参与任何逻辑。留空即不显示。
    #[serde(default)]
    pub note: String,

    /// 该 key 查询用量时附加的 selector header。团体套餐必需
    /// （Bigmodel-Organization / Bigmodel-Project）——**不同 key 可能属于不同组织/项目，
    /// 故按 key 配置**。留空则回退到 [zhipu].extra_headers 的全局兜底。
    #[serde(default)]
    pub quota_headers: Vec<HeaderKV>,
}

/// channel_id 解析完成后的可用条目（orchestrator 直接用它）。
#[derive(Debug, Clone)]
pub struct ResolvedKey {
    pub name: String,
    pub zhipu_api_key: String,
    pub channel_id: i64,
    /// 人类可读的备注（透传自 KeyMapping），只显示不参与逻辑
    pub note: String,
    /// 该 key 的 claude 渠道（`<name>-cc`）id。None = 没有 claude 侧
    /// （未配模板 / 建失败 / 未 sync），该 key 只在 openai 侧受管。
    /// **决策身份永远是主 channel_id**——active/pinned/eligible 都以它为键。
    pub claude_channel_id: Option<i64>,
    /// per-key 的用量查询 selector header（透传自 KeyMapping）
    pub quota_headers: Vec<HeaderKV>,
}

/// 面板提交的新 key。
#[derive(Debug, Clone, Deserialize)]
pub struct NewKeySpec {
    pub name: String,
    pub api_key: String,
    /// 人类可读备注（如持有人名字），可选
    #[serde(default)]
    pub note: String,
    /// 团体套餐的 selector。个人套餐留空。
    #[serde(default)]
    pub org: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

impl NewKeySpec {
    /// selector → 查用量时要带的 header。**团体套餐缺了它就查不到**（返回 limits 空），
    /// 而空 limits 会被误当成 0% 用量 → 这把 key 永远不切换。所以录入时必须探活。
    pub fn headers(&self) -> Vec<HeaderKV> {
        [
            ("Bigmodel-Organization", self.org.as_deref()),
            ("Bigmodel-Project", self.project.as_deref()),
        ]
        .into_iter()
        .filter_map(|(k, v)| {
            let v = v.map(str::trim).filter(|s| !s.is_empty())?;
            Some(HeaderKV {
                key: k.to_string(),
                value: v.to_string(),
            })
        })
        .collect()
    }
}

/// 原子写：临时文件 → fsync → rename。
/// 直接覆写 config.toml 的话，进程若在写一半时挂掉，用户的配置就被截断了。
fn write_atomic(path: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = format!("{path}.tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("创建临时文件失败: {tmp}"))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("替换 {path} 失败"))?;
    Ok(())
}

/// 往 config.toml 追加一条 `[[keys]]`。
///
/// 用 **toml_edit**（格式保留式编辑）而不是 `toml::to_string` 重新序列化——后者会把用户
/// 手写的注释、空行、排版**全部冲掉**，而这个项目的 config.toml 里写满了踩坑说明。
pub fn append_key(path: &str, spec: &NewKeySpec) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("读取 {path} 失败"))?;
    let mut doc: toml_edit::DocumentMut = text.parse().context("config.toml 不是合法 TOML")?;

    if !doc.contains_key("keys") {
        doc["keys"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    let keys = doc["keys"]
        .as_array_of_tables_mut()
        .context("config.toml 里的 keys 不是 [[keys]] 表数组")?;
    if keys
        .iter()
        .any(|t| t.get("name").and_then(|v| v.as_str()) == Some(spec.name.as_str()))
    {
        bail!("config.toml 里已存在同名 key: {}", spec.name);
    }

    let mut t = toml_edit::Table::new();
    t["name"] = toml_edit::value(spec.name.clone());
    t["zhipu_api_key"] = toml_edit::value(spec.api_key.clone());
    if !spec.note.trim().is_empty() {
        t["note"] = toml_edit::value(spec.note.trim());
    }
    let hs = spec.headers();
    if !hs.is_empty() {
        let mut arr = toml_edit::ArrayOfTables::new();
        for h in hs {
            let mut ht = toml_edit::Table::new();
            ht["key"] = toml_edit::value(h.key);
            ht["value"] = toml_edit::value(h.value);
            arr.push(ht);
        }
        t.insert("quota_headers", toml_edit::Item::ArrayOfTables(arr));
    }
    keys.push(t);

    write_atomic(path, doc.to_string().as_bytes())
}

/// 从 config.toml 摘掉一条 `[[keys]]`（同样保留其余部分的注释与排版）。
pub fn remove_key(path: &str, name: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("读取 {path} 失败"))?;
    let mut doc: toml_edit::DocumentMut = text.parse().context("config.toml 不是合法 TOML")?;
    let keys = doc["keys"]
        .as_array_of_tables_mut()
        .context("config.toml 里的 keys 不是 [[keys]] 表数组")?;
    let before = keys.len();
    keys.retain(|t| t.get("name").and_then(|v| v.as_str()) != Some(name));
    if keys.len() == before {
        bail!("config.toml 里没有名为 {name} 的 key");
    }
    write_atomic(path, doc.to_string().as_bytes())
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        let mut cfg: Config = toml::from_str(&text)?;
        cfg.source_path = path.to_string_lossy().into_owned();
        cfg.validate()?;
        Ok(cfg)
    }

    /// 阈值必须满足 0 < restore ≤ throttle ≤ exhausted ≤ 100。
    /// 配错阈值是**静默灾难**（例如 throttle > exhausted 会让合格集恒空、永不切换），
    /// 所以启动就失败，别等到线上才发现。
    fn validate(&self) -> anyhow::Result<()> {
        let (r, t, e) = (
            self.restore_threshold,
            self.throttle_threshold,
            self.exhausted_threshold,
        );
        anyhow::ensure!(
            r > 0.0 && r <= t && t <= e && e <= 100.0,
            "阈值非法：要求 0 < restore({r}) ≤ throttle({t}) ≤ exhausted({e}) ≤ 100"
        );
        // 周临期时间窗：0 = 关闭；上限 7 天 = 周窗口周期（再大等于把全部 key 都算临期）
        anyhow::ensure!(
            self.weekly_reset_lookahead_hours <= 24 * 7,
            "weekly_reset_lookahead_hours 非法：{}（要求 0–168；0 = 关闭临期优先，168 = 一周 = 全临期）",
            self.weekly_reset_lookahead_hours
        );
        if let Some(p) = &self.peak {
            anyhow::ensure!(
                (0..=24).contains(&p.start_hour)
                    && (0..=24).contains(&p.end_hour)
                    && p.start_hour < p.end_hour,
                "[peak] 时段非法：要求 0 ≤ start_hour({}) < end_hour({}) ≤ 24",
                p.start_hour,
                p.end_hour
            );
        }
        // —— claude 渠道模板三条不变量（细化设计 §5.1 C2）——
        if let Some(cc) = &self.new_api.channel_template_claude {
            if cc.name_suffix.trim().is_empty() || cc.name_suffix != cc.name_suffix.trim() {
                bail!(
                    "[new_api.channel_template_claude].name_suffix 非法（不能为空、不能带首尾空白）：\
                     claude 渠道名 = <key 名> + 后缀，为空会与主渠道同名、带空白会拼出怪渠道名，\
                     sync 按名匹配都会互相误判"
                );
            }
            if let Some(oc) = &self.new_api.channel_template {
                if oc.group == cc.group {
                    bail!(
                        "两个渠道模板的 group 相同（\"{}\"）：new-api 分发器按 (group, model) 选渠道、\
                         不按请求格式过滤，同 group 下 OpenAI/Claude 两种格式渠道会互抢流量做跨格式转换。\
                         把 claude 模板的 group 改成别的（如 \"claude\"）",
                        cc.group
                    );
                }
            }
            // suffix 撞名：某把 key 的 cc 渠道名不得恰好等于另一把 key 的主渠道名
            for a in &self.keys {
                for b in &self.keys {
                    if b.name == cc.channel_name(&a.name) {
                        bail!(
                            "key \"{}\" 的 claude 渠道名 \"{}\" 与另一把 key 的主渠道名相同：\
                             sync 按名匹配会把主渠道误认成 cc 渠道。换 name_suffix 或改 key 名",
                            a.name,
                            b.name
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份**带注释、带排版**的配置：这正是我们要保住的东西。
    const SAMPLE: &str = r#"# 顶部注释：别被冲掉
poll_interval_secs = 60
throttle_threshold = 95.0   # 行尾注释
restore_threshold = 90.0

[zhipu]
quota_url = "https://open.bigmodel.cn/api/monitor/usage/quota/limit?type=2"

[new_api]
base_url = "http://127.0.0.1:3000"

# 下面是 key 列表
[[keys]]
name = "zhipu-1"
zhipu_api_key = "k1"
[[keys.quota_headers]]
key = "Bigmodel-Organization"
value = "org-1"
"#;

    fn tmp(tag: &str) -> String {
        let p = std::env::temp_dir().join(format!("qt-cfg-{}-{tag}.toml", std::process::id()));
        std::fs::write(&p, SAMPLE).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn spec(name: &str) -> NewKeySpec {
        NewKeySpec {
            name: name.into(),
            api_key: "k2".into(),
            note: String::new(),
            org: Some("org-2".into()),
            project: Some("proj-2".into()),
        }
    }

    #[test]
    fn 追加key_不冲掉注释与排版() {
        let p = tmp("append");
        append_key(&p, &spec("zhipu-2")).unwrap();
        let out = std::fs::read_to_string(&p).unwrap();

        // 注释、行尾注释、原有内容一个字符都不能少
        assert!(out.contains("# 顶部注释：别被冲掉"));
        assert!(out.contains("throttle_threshold = 95.0   # 行尾注释"));
        assert!(out.contains("# 下面是 key 列表"));
        assert!(out.contains(r#"value = "org-1""#));

        // 新 key 进去了，且能被正常解析回来
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.keys.len(), 2);
        let k = &cfg.keys[1];
        assert_eq!(k.name, "zhipu-2");
        assert_eq!(k.zhipu_api_key, "k2");
        assert_eq!(k.quota_headers.len(), 2);
        assert_eq!(k.quota_headers[0].key, "Bigmodel-Organization");
        assert_eq!(k.quota_headers[1].value, "proj-2");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 同名key_拒绝追加() {
        let p = tmp("dup");
        assert!(append_key(&p, &spec("zhipu-1")).is_err());
        // 失败时不能留下任何改动
        assert_eq!(std::fs::read_to_string(&p).unwrap(), SAMPLE);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 删除key_只摘掉那一条_其余原样() {
        let p = tmp("remove");
        append_key(&p, &spec("zhipu-2")).unwrap();
        remove_key(&p, "zhipu-2").unwrap();
        let out = std::fs::read_to_string(&p).unwrap();

        assert!(out.contains("# 顶部注释：别被冲掉"));
        assert!(out.contains("# 下面是 key 列表"));
        assert!(!out.contains("zhipu-2"));
        let cfg: Config = toml::from_str(&out).unwrap();
        assert_eq!(cfg.keys.len(), 1);
        assert_eq!(cfg.keys[0].name, "zhipu-1");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 删除不存在的key_报错() {
        let p = tmp("nomatch");
        assert!(remove_key(&p, "不存在").is_err());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 个人套餐_不填selector_则不写quota_headers() {
        let p = tmp("nosel");
        append_key(
            &p,
            &NewKeySpec {
                name: "personal".into(),
                api_key: "k3".into(),
                note: String::new(),
                org: None,
                project: Some("  ".into()), // 空白应被当作没填
            },
        )
        .unwrap();
        let cfg: Config = toml::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(cfg.keys[1].quota_headers.is_empty());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 阈值非法_启动即失败() {
        // throttle > exhausted 会让合格集恒空、永不切换 —— 必须在启动时就拦住
        let p = std::env::temp_dir().join(format!("qt-cfg-{}-bad.toml", std::process::id()));
        std::fs::write(&p, SAMPLE.replace("throttle_threshold = 95.0", "throttle_threshold = 101.0")).unwrap();
        assert!(Config::load(&p).is_err());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 周临期时间窗_缺省24_填0合法_超一周失败() {
        // 缺省 = 24（策略默认开）
        let p = tmp("lk-default");
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.weekly_reset_lookahead_hours, 24);
        std::fs::remove_file(&p).ok();

        // 0 = 显式关闭（⚠️ 必须写在 [[keys]] 之前——TOML 里 key 归属最近的表头，
        // 追加在 keys 表之后会被挂进 keys.quota_headers 而不是顶层）
        let p = tmp("lk-zero");
        std::fs::write(
            &p,
            SAMPLE.replace("poll_interval_secs", "weekly_reset_lookahead_hours = 0\npoll_interval_secs"),
        )
        .unwrap();
        assert_eq!(Config::load(&p).unwrap().weekly_reset_lookahead_hours, 0);
        std::fs::remove_file(&p).ok();

        // 168 = 一周（上限，合法）；169 = 手误，启动即失败
        let p = tmp("lk-max");
        std::fs::write(
            &p,
            SAMPLE.replace("poll_interval_secs", "weekly_reset_lookahead_hours = 169\npoll_interval_secs"),
        )
        .unwrap();
        assert!(Config::load(&p).is_err(), "169h 超过周周期，应拦截");
        std::fs::remove_file(&p).ok();
    }

    // ——— claude 渠道模板（细化设计 §5.1 C1/C2）———

    /// SAMPLE + 追加一段 claude 模板（及可选的 openai 模板），落盘后返回路径。
    fn claude_cfg(extra: &str) -> String {
        let p = std::env::temp_dir().join(format!(
            "qt-cfg-{}-cc-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&p, format!("{SAMPLE}{extra}")).unwrap();
        p.to_string_lossy().into_owned()
    }

    const CLAUDE_TPL: &str = r#"
[new_api.channel_template_claude]
type = 14
base_url = "https://open.bigmodel.cn/api/anthropic"
models = "glm-5.2[1m]"
group = "claude"
name_suffix = "-cc"
token_name = "claude-code"
"#;

    const OPENAI_TPL: &str = r#"
[new_api.channel_template]
type = 8
base_url = "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions"
models = "glm-5.2"
group = "default"
"#;

    #[test]
    fn claude模板_未配置_为None且不触发校验() {
        let p = claude_cfg("");
        let cfg = Config::load(&p).unwrap();
        assert!(cfg.new_api.channel_template_claude.is_none());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn claude模板_与openai同group_启动即失败() {
        // 同 group 两种格式渠道会互抢流量做跨格式转换 —— 本设计要消灭的故障模式
        let p = claude_cfg(&format!("{OPENAI_TPL}{}", CLAUDE_TPL.replace("group = \"claude\"", "group = \"default\"")));
        let err = Config::load(&p).unwrap_err().to_string();
        assert!(err.contains("group 相同"), "实际错误：{err}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn claude模板_空suffix_启动即失败() {
        // 空 suffix 会让 cc 渠道名与主渠道同名，sync 按名匹配互相误判
        let p = claude_cfg(&CLAUDE_TPL.replace("name_suffix = \"-cc\"", "name_suffix = \"\""));
        let err = Config::load(&p).unwrap_err().to_string();
        assert!(err.contains("name_suffix"), "实际错误：{err}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn claude模板_suffix撞另一把key的主渠道名_启动即失败() {
        // zhipu-1 的 cc 渠道名 "zhipu-1-cc" 恰好是另一把 key 的名字 → sync 会把
        // 那把 key 的主渠道误认成 zhipu-1 的 cc 渠道
        let p = claude_cfg(&format!(
            "{CLAUDE_TPL}\n[[keys]]\nname = \"zhipu-1-cc\"\nzhipu_api_key = \"k2\"\n"
        ));
        let err = Config::load(&p).unwrap_err().to_string();
        assert!(err.contains("误认"), "实际错误：{err}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn claude模板_合法配置_加载成功_默认值齐全() {
        // 空表 = 全默认值（功能开关即配置块本身，D4）
        let p = claude_cfg("\n[new_api.channel_template_claude]\n");
        let cfg = Config::load(&p).unwrap();
        let cc = cfg.new_api.channel_template_claude.as_ref().unwrap();
        assert_eq!(cc.channel_type, 14);
        assert_eq!(cc.base_url, "https://open.bigmodel.cn/api/anthropic");
        assert_eq!(cc.group, "claude");
        assert_eq!(cc.name_suffix, "-cc");
        assert_eq!(cc.token_name, "claude-code");
        assert!(cc.models.contains("glm-5.3[1m]"), "默认模型表必须含 [1m] 变体");
        assert_eq!(cc.channel_name("zhipu-1"), "zhipu-1-cc");
        std::fs::remove_file(&p).ok();
    }
}
