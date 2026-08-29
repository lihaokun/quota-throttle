//! new-api 管理 API 客户端。
//!
//! 两件事：
//!   1. 鉴权——优先用配置里的 admin_token(Bearer)；没有就用 root 账号登录拿会话 cookie。
//!   2. 渠道——列出/创建（sync 按 key 列表对齐渠道并解析 channel_id）、以及运行期改 priority。
//!
//! 改 priority 仍用「GET 渠道 → 只改 priority → PUT 回」，整体搬运，对版本差异最鲁棒。
//! ⚠️ channel_path / 建渠道字段 / 是否需要 New-Api-User，请用 F12 抓真实请求核实。

use crate::config::{ChannelTemplate, ClaudeChannelTemplate, KeyMapping, NewApiConfig};
use crate::status::{ChannelState, RequestLog};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tracing::{info, warn};

/// new-api 列表响应兼容：新版 `data.items[]`，旧版 `data[]`。
fn extract_items(body: &Value) -> Vec<Value> {
    body.get("data")
        .and_then(|d| {
            d.get("items")
                .and_then(|v| v.as_array())
                .or_else(|| d.as_array())
        })
        .cloned()
        .unwrap_or_default()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}
fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64())
}

enum Auth {
    Token(String),
    /// 已登录，会话在 cookie 里；user_id 用于 New-Api-User 头
    Session { user_id: Option<i64> },
    /// 还没登录（admin_token 为空，需调 login）
    Pending,
}

/// 建渠道参数：两种格式模板（OpenAI/Anthropic）归一到同一 payload 形状。
/// 字段私有——外部只能经 From 转换拿到，杜绝手拼。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelParams<'a> {
    channel_type: i64,
    base_url: &'a str,
    models: &'a str,
    group: &'a str,
}

impl<'a> From<&'a ChannelTemplate> for ChannelParams<'a> {
    fn from(t: &'a ChannelTemplate) -> Self {
        Self {
            channel_type: t.channel_type,
            base_url: &t.base_url,
            models: &t.models,
            group: &t.group,
        }
    }
}
impl<'a> From<&'a ClaudeChannelTemplate> for ChannelParams<'a> {
    fn from(t: &'a ClaudeChannelTemplate) -> Self {
        Self {
            channel_type: t.channel_type,
            base_url: &t.base_url,
            models: &t.models,
            group: &t.group,
        }
    }
}

/// 渠道操作计划（`plan_channel_ops` 的输出，纯数据）。
#[derive(Debug, PartialEq)]
enum ChannelOpKind {
    OpenAi,
    Claude,
}

#[derive(Debug, PartialEq)]
enum ChannelOp<'a> {
    /// 已存在（按名幂等跳过）
    Skip { name: String, kind: ChannelOpKind },
    /// 需要创建
    Create {
        name: String,
        kind: ChannelOpKind,
        params: ChannelParams<'a>,
        key: &'a str,
    },
    /// openai 槽缺渠道且未配模板（现有 warn 语义）
    Missing { name: String },
}

/// 纯函数：keys × 现有渠道名集合 × 两模板 → 渠道操作计划（不执行、零 IO）。
///
/// 每把 key 两个槽位：
///   · openai 槽（名 = key.name）：已存在→Skip；缺且配了模板→Create；缺且没模板→Missing。
///   · claude 槽（名 = key.name+suffix，仅配了模板才存在）：已存在→Skip；缺→Create。
///     未配模板 = 功能关，**不产任何 op 也不 warn**。
fn plan_channel_ops<'a>(
    keys: &'a [KeyMapping],
    existing: &HashSet<String>,
    openai: Option<&'a ChannelTemplate>,
    claude: Option<&'a ClaudeChannelTemplate>,
) -> Vec<ChannelOp<'a>> {
    let mut ops = Vec::new();
    for k in keys {
        if existing.contains(&k.name) {
            ops.push(ChannelOp::Skip {
                name: k.name.clone(),
                kind: ChannelOpKind::OpenAi,
            });
        } else if let Some(t) = openai {
            ops.push(ChannelOp::Create {
                name: k.name.clone(),
                kind: ChannelOpKind::OpenAi,
                params: t.into(),
                key: &k.zhipu_api_key,
            });
        } else {
            ops.push(ChannelOp::Missing { name: k.name.clone() });
        }
        if let Some(t) = claude {
            let name = t.channel_name(&k.name);
            if existing.contains(&name) {
                ops.push(ChannelOp::Skip {
                    name,
                    kind: ChannelOpKind::Claude,
                });
            } else {
                ops.push(ChannelOp::Create {
                    name,
                    kind: ChannelOpKind::Claude,
                    params: t.into(),
                    key: &k.zhipu_api_key,
                });
            }
        }
    }
    ops
}

/// sync 结果：按 **key 名** 索引两侧渠道 id（resolve_keys 不需要知道 suffix）。
#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// key 名 → 主渠道（OpenAI 格式）id
    pub primary: HashMap<String, i64>,
    /// key 名 → claude 渠道（`<name>-cc`）id。缺项 = 该 key 无 claude 侧（建失败/未建）。
    pub claude: HashMap<String, i64>,
}

/// option 的 value 是「JSON 的字符串」（`Interface2String` 产物），剥一层。
fn parse_option_json(it: &Value) -> Result<Value> {
    let raw = s(it, "value");
    serde_json::from_str(&raw).with_context(|| format!("option value 不是合法 JSON: {raw}"))
}

pub struct NewApiClient {
    client: reqwest::Client,
    base_url: String,
    channel_path: String,
    auth: Auth,
    root_username: String,
    root_password: String,
    extra_headers: Vec<(String, String)>,
}

impl NewApiClient {
    pub fn new(cfg: &NewApiConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .build()
            .context("构建 HTTP client 失败")?;
        let auth = if cfg.admin_token.trim().is_empty() {
            Auth::Pending
        } else {
            Auth::Token(cfg.admin_token.clone())
        };
        Ok(Self {
            client,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            channel_path: cfg.channel_path.clone(),
            auth,
            root_username: cfg.root_username.clone(),
            root_password: cfg.root_password.clone(),
            extra_headers: cfg
                .extra_headers
                .iter()
                .map(|h| (h.key.clone(), h.value.clone()))
                .collect(),
        })
    }

    fn apply_headers(&self, mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            Auth::Token(t) => {
                rb = rb.header("Authorization", format!("Bearer {t}"));
            }
            Auth::Session { user_id } => {
                if let Some(id) = user_id {
                    rb = rb.header("New-Api-User", id.to_string());
                }
            }
            Auth::Pending => {}
        }
        for (k, v) in &self.extra_headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        rb
    }

    /// 新版 new-api 首启不再自带 root：需先 POST /api/setup 建管理员。幂等——已初始化则跳过。
    async fn ensure_setup(&self) -> Result<()> {
        let url = format!("{}/api/setup", self.base_url);
        let body: Value = self
            .client
            .get(&url)
            .send()
            .await
            .context("查询 new-api setup 状态失败")?
            .json()
            .await
            .unwrap_or(Value::Null);
        let data = body.get("data");
        let status = data
            .and_then(|d| d.get("status"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let root_init = data
            .and_then(|d| d.get("root_init"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if status || root_init {
            return Ok(()); // 已初始化
        }
        info!(user = %self.root_username, "new-api 首启未初始化，创建管理员");
        let payload = json!({
            "username": self.root_username,
            "password": self.root_password,
            "confirmPassword": self.root_password,
            "SelfUseModeEnabled": true,   // 自用网关，关掉多租户计费等检查
            "DemoSiteEnabled": false,
        });
        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .context("初始化 new-api 失败")?;
        let st = resp.status();
        let rb: Value = resp.json().await.unwrap_or(Value::Null);
        let ok = rb.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok {
            bail!("new-api 初始化失败: HTTP {st} body={rb}（密码需≥8位、用户名≤12）");
        }
        info!("new-api 管理员已创建");
        Ok(())
    }

    /// 确保已鉴权：Token 模式无需动作；Pending 则（必要时先 setup）用 root 登录换会话。
    pub async fn authenticate(&mut self) -> Result<()> {
        if !matches!(self.auth, Auth::Pending) {
            return Ok(());
        }
        self.ensure_setup().await?;
        let url = format!("{}/api/user/login", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&json!({ "username": self.root_username, "password": self.root_password }))
            .send()
            .await
            .context("登录 new-api 失败")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let ok = body.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok {
            bail!(
                "new-api 登录失败: HTTP {} body={}（默认 root/123456，改过密码就填 admin_token 或 root_password）",
                status,
                body
            );
        }
        let user_id = body
            .get("data")
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_i64());
        info!(user_id = ?user_id, "已登录 new-api（会话模式）");
        self.auth = Auth::Session { user_id };
        Ok(())
    }

    /// 列出渠道，返回 name → id。兼容 data.items 和 data 直接数组两种结构。
    pub async fn list_channels(&self) -> Result<HashMap<String, i64>> {
        let url = format!("{}{}/?p=0&page_size=100", self.base_url, self.channel_path);
        let rb = self.apply_headers(self.client.get(&url));
        let body: Value = rb
            .send()
            .await
            .context("列出渠道失败")?
            .json()
            .await
            .context("解析渠道列表失败")?;

        let mut map = HashMap::new();
        for it in extract_items(&body) {
            if let (Some(name), Some(id)) = (
                it.get("name").and_then(|v| v.as_str()),
                it.get("id").and_then(|v| v.as_i64()),
            ) {
                map.insert(name.to_string(), id);
            }
        }
        Ok(map)
    }

    /// 【看板】拉取渠道**完整状态**（status / priority / weight / used_quota / auto_ban）。
    ///
    /// **纯读，零副作用**——已核实 new-api 的 `GetAllChannels` 内无任何写/测试调用。
    /// 首要用途：暴露「渠道被 new-api 自动禁用」这个盲区——我们只改 priority、从不碰 status，
    /// 渠道一旦被禁，priority=100 也不会有流量。
    pub async fn list_channel_states(&self) -> Result<Vec<ChannelState>> {
        let url = format!("{}{}/?p=0&page_size=100", self.base_url, self.channel_path);
        let body: Value = self
            .apply_headers(self.client.get(&url))
            .send()
            .await
            .context("拉取渠道状态失败")?
            .json()
            .await
            .context("解析渠道状态失败")?;

        let mut out: Vec<ChannelState> = extract_items(&body)
            .iter()
            .filter_map(|it| {
                let id = i(it, "id")?;
                let status_raw = i(it, "status").unwrap_or(0);
                Some(ChannelState {
                    id,
                    name: s(it, "name"),
                    enabled: status_raw == 1,
                    status_raw,
                    priority: i(it, "priority"),
                    weight: i(it, "weight"),
                    used_quota: i(it, "used_quota").unwrap_or(0),
                    auto_ban: i(it, "auto_ban"),
                    models: s(it, "models"),
                    group: s(it, "group"),
                })
            })
            .collect();
        out.sort_by_key(|c| c.id); // 顺序稳定，看板不跳动
        Ok(out)
    }

    /// 【看板】最近 n 条**真实请求**。纯读（`/api/log/` handler 无写操作）。
    ///
    /// 过滤依据用**字段语义**（model_name 非空 且 channel != 0）而非 `type` 枚举值——
    /// 后者随 new-api 版本可能变，前者稳。日志里混有登录等系统条目（实测 type=7）。
    pub async fn recent_logs(&self, n: usize) -> Result<Vec<RequestLog>> {
        let url = format!("{}/api/log/?p=0&page_size={n}", self.base_url);
        let body: Value = self
            .apply_headers(self.client.get(&url))
            .send()
            .await
            .context("拉取请求日志失败")?
            .json()
            .await
            .context("解析请求日志失败")?;

        let mut out: Vec<RequestLog> = extract_items(&body)
            .iter()
            .filter_map(|it| {
                let model_name = s(it, "model_name");
                let channel = i(it, "channel").unwrap_or(0);
                if model_name.is_empty() || channel == 0 {
                    return None; // 系统日志（登录等），非真实请求
                }
                Some(RequestLog {
                    created_at: i(it, "created_at").unwrap_or(0),
                    channel,
                    channel_name: s(it, "channel_name"),
                    model_name,
                    prompt_tokens: i(it, "prompt_tokens").unwrap_or(0),
                    completion_tokens: i(it, "completion_tokens").unwrap_or(0),
                    quota: i(it, "quota").unwrap_or(0),
                    use_time: i(it, "use_time").unwrap_or(0),
                    is_stream: it.get("is_stream").and_then(|v| v.as_bool()).unwrap_or(false),
                    token_name: s(it, "token_name"),
                })
            })
            .collect();
        // 自行排序，不依赖服务端返回顺序
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(out)
    }

    /// 创建一个渠道（把 name/key/priority 合并进模板参数 POST）。
    ///
    /// 两种格式共用同一 payload 形状——type 14 (Anthropic) 与 type 8 (Custom) 字段同名，
    /// 仅 type/base_url/models/group 取值不同。`model/channel.go` 对 models 无正则校验，
    /// `glm-5.2[1m]` 这类带后缀的名字可直接挂（调研 §2 已核）。
    pub async fn create_channel(
        &self,
        name: &str,
        key: &str,
        priority: i64,
        p: &ChannelParams<'_>,
    ) -> Result<()> {
        // new-api 的 AddChannel 期望 { mode, channel:{...} }，channel 是指针，缺了会 nil-panic。
        let payload = json!({
            "mode": "single",
            "channel": {
                "name": name,
                "type": p.channel_type,
                "key": key,
                "base_url": p.base_url,
                "models": p.models,
                "group": p.group,
                "priority": priority,
                "weight": 0,
                "status": 1,
            }
        });
        let url = format!("{}{}", self.base_url, self.channel_path);
        let rb = self.apply_headers(self.client.post(&url)).json(&payload);
        let resp = rb.send().await.context("创建渠道失败")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let ok = body
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(status.is_success());
        if !ok {
            bail!("创建渠道 {name} 失败: HTTP {status} body={body}");
        }
        Ok(())
    }

    /// 按 key 列表对齐 new-api 渠道：缺的就（用模板）建出来。每把 key 两个槽位——
    /// openai 槽恒有（未配模板则只 warn 不建，现有语义）；claude 槽仅在配了模板时存在。
    /// **错误分级**：openai 建失败 → 硬错终止（地基不齐别进切换循环）；
    /// claude 建失败 → warn 降级（叠加层坏了不连累现有链路，该 key 的 CC 侧无人受管而已）。
    pub async fn sync_channels(
        &self,
        keys: &[KeyMapping],
        openai_tpl: Option<&ChannelTemplate>,
        claude_tpl: Option<&ClaudeChannelTemplate>,
        standby_priority: i64,
    ) -> Result<SyncOutcome> {
        let existing = self.list_channels().await?;
        let names: HashSet<String> = existing.keys().cloned().collect();
        let plan = plan_channel_ops(keys, &names, openai_tpl, claude_tpl);

        let mut created = false;
        for op in &plan {
            match op {
                ChannelOp::Skip { name, .. } => info!(name = %name, "渠道已存在，跳过创建"),
                ChannelOp::Missing { name } => warn!(
                    name = %name,
                    "渠道不存在且未配 channel_template，无法自动创建"
                ),
                ChannelOp::Create { name, kind, params, key } => {
                    info!(name = %name, kind = ?kind, "创建渠道");
                    match self.create_channel(name, key, standby_priority, params).await {
                        Ok(()) => created = true,
                        Err(e) if matches!(kind, ChannelOpKind::OpenAi) => return Err(e),
                        Err(e) => warn!(
                            name = %name,
                            error = %e,
                            "创建 claude 渠道失败，降级：该 key 暂无 Claude Code 侧"
                        ),
                    }
                }
            }
        }

        // 有新建就重新拉一遍，拿到新 id；否则用第一遍的
        let latest = if created {
            self.list_channels().await?
        } else {
            existing
        };

        let mut out = SyncOutcome::default();
        for k in keys {
            if let Some(id) = latest.get(&k.name) {
                out.primary.insert(k.name.clone(), *id);
            }
            if let Some(t) = claude_tpl {
                let cc = t.channel_name(&k.name);
                if let Some(id) = latest.get(&cc) {
                    out.claude.insert(k.name.clone(), *id);
                }
                // cc 缺项不另 warn：创建失败的上面已 warn 过，别刷屏
            }
        }
        Ok(out)
    }

    /// 把 group 注册进 new-api，让「绑定该 group 的令牌」可用。**两个层面缺一不可**：
    /// ① `UserUsableGroups`（用户可用组，flat map group→描述）——令牌的 group 不在
    ///    用户可用组里的话，TokenAuth 直接 403「无权访问 x 分组」
    ///    （`middleware/auth.go:421-435`，2026-08 实测踩过）；
    /// ② 分组倍率（新形态 `group_ratio_setting.group_ratio` / 旧形态 `GroupRatio`）——计费层。
    /// 幂等：都注册过就零写请求。写回后回读断言。
    pub async fn ensure_group(&self, group: &str) -> Result<()> {
        let options = self.list_options().await?;

        // ① 用户可用组（硬门槛：缺了令牌直接 403，CC 全断）
        if let Some(it) = options.iter().find(|it| s(it, "key") == "UserUsableGroups") {
            let mut map = parse_option_json(it)?;
            let obj = map
                .as_object_mut()
                .context("UserUsableGroups 不是 JSON 对象")?;
            if !obj.contains_key(group) {
                obj.insert(group.to_string(), Value::from("Claude Code 专用分组"));
                self.put_option_and_verify("UserUsableGroups", &map, group)
                    .await?;
                info!(group, "已加入用户可用组（UserUsableGroups）");
            }
        } else {
            bail!("option 列表里没有 UserUsableGroups 键——该版本形态未知，\
                   CC 令牌会 403「无权访问 {group} 分组」，请到 new-api UI 手动把该分组加入用户可用组");
        }

        // ② 分组倍率（计费层；新形态优先，旧形态兜底）
        let (opt_key, mut value, nested) = if let Some(it) = options
            .iter()
            .find(|it| s(it, "key") == "group_ratio_setting")
        {
            ("group_ratio_setting", parse_option_json(it)?, true)
        } else if let Some(it) = options.iter().find(|it| s(it, "key") == "GroupRatio") {
            ("GroupRatio", parse_option_json(it)?, false)
        } else {
            bail!(
                "option 列表里既没有 group_ratio_setting 也没有 GroupRatio——\
                 该版本的分组倍率形态未知，跳过倍率注册（不影响路由，仅计费倍率）"
            );
        };
        let target = if nested {
            value
                .get_mut("group_ratio")
                .context("group_ratio_setting 缺 group_ratio 子对象")?
        } else {
            &mut value
        };
        let map = target.as_object_mut().context("分组配置不是 JSON 对象")?;
        if !map.contains_key(group) {
            map.insert(group.to_string(), Value::from(1));
            self.put_option_and_verify(opt_key, &value, group).await?;
            info!(group, opt_key, "已注册分组倍率（ratio=1）");
        }
        Ok(())
    }

    /// GET /api/option/ 的条目列表（data: [{key, value}...]，value 是字符串化的 JSON）。
    async fn list_options(&self) -> Result<Vec<Value>> {
        let url = format!("{}/api/option/", self.base_url);
        let body: Value = self
            .apply_headers(self.client.get(&url))
            .send()
            .await
            .context("拉取 new-api option 列表失败")?
            .json()
            .await
            .context("解析 option 列表失败")?;
        Ok(body
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// PUT 一个 option（值是「JSON 的字符串」）并**回读断言**目标 group 键已可见
    /// （GET 读的是内存 OptionMap，PUT 成功即应生效，断言失败如实报错）。
    async fn put_option_and_verify(
        &self,
        opt_key: &str,
        new_value: &Value,
        group: &str,
    ) -> Result<()> {
        let url = format!("{}/api/option/", self.base_url);
        let payload = json!({
            "key": opt_key,
            "value": serde_json::to_string(new_value).context("序列化 option 失败")?,
        });
        let resp = self
            .apply_headers(self.client.put(&url))
            .json(&payload)
            .send()
            .await
            .context("写回 option 失败")?;
        let status = resp.status();
        let rb: Value = resp.json().await.unwrap_or(Value::Null);
        let ok = rb
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(status.is_success());
        if !ok {
            bail!("写回 option {opt_key} 失败: HTTP {status} body={rb}");
        }
        let options = self.list_options().await?;
        let it = options
            .iter()
            .find(|it| s(it, "key") == opt_key)
            .context("回读不到刚写的 option")?;
        let v2 = parse_option_json(it)?;
        // 兼容两种形态：平铺 map（GroupRatio/UserUsableGroups）与嵌套
        // group_ratio_setting.group_ratio——group 可能在根也可能在子对象里
        let has = v2.get(group).is_some()
            || v2.get("group_ratio").and_then(|g| g.get(group)).is_some();
        anyhow::ensure!(
            has,
            "PUT 后回读仍不见 {group}（{opt_key}），请到 new-api UI 手动确认"
        );
        Ok(())
    }

    /// 找到（或建出）绑定 claude group 的 CC 专用令牌，返回**完整 key**。
    ///
    /// key 每次现取现用：`POST /api/token/:id/key` 专门回完整值（GetTokenKey），
    /// 列表里的 key 是打码的（CLAUDE.md 血泪）。不落任何本地状态。
    /// 同名令牌必须语义唯一：多条/属别的 group/被禁用都 bail 让用户人工处理，不猜。
    pub async fn ensure_claude_token(&self, name: &str, group: &str) -> Result<String> {
        let mut tokens = self.list_tokens().await?;
        let id = match tokens
            .iter()
            .filter(|t| s(t, "name") == name)
            .collect::<Vec<_>>()
            .as_slice()
        {
            [] => {
                // 建：绑定 group、无限额度、永不过期（AddToken 契约见细化设计文首源码事实）
                info!(name, group, "创建 Claude Code 专用令牌");
                let url = format!("{}/api/token/", self.base_url);
                let payload = json!({
                    "name": name,
                    "group": group,
                    "expired_time": -1,
                    "unlimited_quota": true,
                    "remain_quota": 0,
                    "model_limits_enabled": false,
                });
                let resp = self
                    .apply_headers(self.client.post(&url))
                    .json(&payload)
                    .send()
                    .await
                    .context("创建令牌失败")?;
                let status = resp.status();
                let rb: Value = resp.json().await.unwrap_or(Value::Null);
                let ok = rb
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(status.is_success());
                if !ok {
                    bail!("创建令牌 {name} 失败: HTTP {status} body={rb}（name ≤50 字符）");
                }
                // AddToken 响应不含 id → 重拉列表按名取
                tokens = self.list_tokens().await?;
                let t = tokens
                    .iter()
                    .find(|t| s(t, "name") == name)
                    .with_context(|| format!("令牌已创建但列表里找不到：{name}"))?;
                i(t, "id").context("令牌响应缺 id")?
            }
            [one] => {
                let g = s(one, "group");
                if !g.is_empty() && g != group {
                    bail!(
                        "已存在同名令牌 {name} 但 group 是 \"{g}\"（要 \"{group}\"）：\
                         请改 token_name 或到 new-api 里人工处理"
                    );
                }
                let status_raw = i(one, "status").unwrap_or(1);
                if status_raw != 1 {
                    bail!("令牌 {name} 处于禁用状态（status={status_raw}），请到 new-api 里启用");
                }
                i(one, "id").context("令牌响应缺 id")?
            }
            _ => bail!("存在多个同名令牌 {name}，语义不明，请到 new-api 里清理后重试"),
        };

        // 完整 key：专用端点（列表打码，这里取真值）
        let url = format!("{}/api/token/{id}/key", self.base_url);
        let resp = self
            .apply_headers(self.client.post(&url))
            .send()
            .await
            .context("取令牌完整 key 失败")?;
        let status = resp.status();
        let rb: Value = resp.json().await.unwrap_or(Value::Null);
        let ok = rb
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(status.is_success());
        if !ok {
            bail!("取令牌 {name} 完整 key 失败: HTTP {status} body={rb}");
        }
        rb.get("data")
            .and_then(|d| d.get("key"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .context("取 key 响应缺 data.key")
    }

    /// 当前用户的令牌列表（key 打码，只用 id/name/group/status）。
    async fn list_tokens(&self) -> Result<Vec<Value>> {
        let url = format!("{}/api/token/?p=0&page_size=100", self.base_url);
        let body: Value = self
            .apply_headers(self.client.get(&url))
            .send()
            .await
            .context("拉取令牌列表失败")?
            .json()
            .await
            .context("解析令牌列表失败")?;
        Ok(extract_items(&body))
    }

    /// 【看板】用量统计（new-api 自己按**小时**聚合好的 `quota_data`）。纯读。
    /// 返回 (model, hour_epoch_sec, tokens, count)。供时序曲线 + 按模型汇总两用。
    ///
    /// ⚠️ 该接口 `Group("model_name, created_at")` —— **不带渠道维度**（new-api 从不暴露按渠道的用量）。
    pub async fn usage_data(&self, start: i64, end: i64) -> Result<Vec<(String, i64, i64, i64)>> {
        let url = format!(
            "{}/api/data/?start_timestamp={start}&end_timestamp={end}",
            self.base_url
        );
        let body: Value = self
            .apply_headers(self.client.get(&url))
            .send()
            .await
            .context("拉取用量统计失败")?
            .json()
            .await
            .context("解析用量统计失败")?;
        let items = body
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(items
            .iter()
            .filter_map(|it| {
                let m = s(it, "model_name");
                if m.is_empty() {
                    return None;
                }
                Some((
                    m,
                    i(it, "created_at").unwrap_or(0),
                    i(it, "token_used").unwrap_or(0),
                    i(it, "count").unwrap_or(0),
                ))
            })
            .collect())
    }

    /// 【看板】读 new-api 的**内部虚拟余额**（当前登录用户）。纯读。
    ///
    /// ⚠️ new-api 按「按量付费倍率」给包月编码套餐虚构记账，余额见底会**直接挡住转发**
    /// （报「预扣费额度失败」），跟智谱额度毫无关系。看板据此在见底前告警。
    pub async fn user_quota(&self) -> Result<i64> {
        let url = format!("{}/api/user/self", self.base_url);
        let body: Value = self
            .apply_headers(self.client.get(&url))
            .send()
            .await
            .context("拉取 new-api 用户余额失败")?
            .json()
            .await
            .context("解析用户余额失败")?;
        body.get("data")
            .and_then(|d| d.get("quota"))
            .and_then(|v| v.as_i64())
            .context("响应缺少 data.quota")
    }

    /// GET /api/channel/{id} → 渠道对象（从 data 取出）
    pub async fn get_channel(&self, id: i64) -> Result<Value> {
        let url = format!("{}{}/{}", self.base_url, self.channel_path, id);
        let rb = self.apply_headers(self.client.get(&url));
        let body: Value = rb
            .send()
            .await
            .context("获取渠道失败")?
            .json()
            .await
            .context("解析渠道响应失败")?;
        body.get("data")
            .cloned()
            .context("渠道响应缺少 data 字段（请用 F12 核实实际结构）")
    }

    /// 取渠道 → 改某整数字段 → PUT 回。整体搬运，只动这一个字段。
    async fn set_channel_field(&self, id: i64, field: &str, value: i64) -> Result<()> {
        let mut channel = self.get_channel(id).await?;
        match channel.as_object_mut() {
            Some(obj) => {
                obj.insert(field.to_string(), Value::from(value));
                // new-api 的 UpdateChannel 明确拒绝请求体里带 status（判为 Invalid parameters），
                // 必须剔除。GET 回来的 key 是空串，UpdateChannel 对空 key 会保留原值，安全。
                obj.remove("status");
            }
            None => bail!("渠道 {id} 返回的不是 JSON 对象"),
        }
        let url = format!("{}{}", self.base_url, self.channel_path);
        let rb = self.apply_headers(self.client.put(&url)).json(&channel);
        let resp = rb.send().await.context("更新渠道失败")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let ok = body
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(status.is_success());
        if !ok {
            bail!("更新渠道 {id} 字段 {field} 失败: HTTP {status} body={body}");
        }
        Ok(())
    }

    /// 设置渠道 priority——本工具「钉住单把活动 key」的唯一运行期杠杆。
    pub async fn set_channel_priority(&self, id: i64, priority: i64) -> Result<()> {
        self.set_channel_field(id, "priority", priority).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> KeyMapping {
        KeyMapping {
            name: name.into(),
            zhipu_api_key: format!("k-{name}"),
            channel_id: None,
            note: String::new(),
            quota_headers: Vec::new(),
        }
    }

    fn openai_tpl() -> ChannelTemplate {
        ChannelTemplate {
            channel_type: 8,
            base_url: "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions".into(),
            models: "glm-5.2".into(),
            group: "default".into(),
        }
    }

    fn claude_tpl() -> ClaudeChannelTemplate {
        ClaudeChannelTemplate {
            channel_type: 14,
            base_url: "https://open.bigmodel.cn/api/anthropic".into(),
            models: "glm-5.3[1m]".into(),
            group: "claude".into(),
            name_suffix: "-cc".into(),
            token_name: "claude-code".into(),
        }
    }

    fn names(ops: &[ChannelOp]) -> Vec<String> {
        ops.iter()
            .map(|o| match o {
                ChannelOp::Skip { name, .. } | ChannelOp::Create { name, .. } | ChannelOp::Missing { name } => name.clone(),
            })
            .collect()
    }

    fn kinds(ops: &[ChannelOp]) -> Vec<&'static str> {
        ops.iter()
            .map(|o| match o {
                ChannelOp::Skip { kind, .. } | ChannelOp::Create { kind, .. } => match kind {
                    ChannelOpKind::OpenAi => "openai",
                    ChannelOpKind::Claude => "claude",
                },
                ChannelOp::Missing { .. } => "missing",
            })
            .collect()
    }

    fn existing(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 全新建_双渠道各一个create() {
        let keys = [key("zhipu-1")];
        let (o, c) = (openai_tpl(), claude_tpl());
        let ops = plan_channel_ops(&keys, &existing(&[]), Some(&o), Some(&c));
        assert_eq!(names(&ops), vec!["zhipu-1", "zhipu-1-cc"]);
        assert_eq!(kinds(&ops), vec!["openai", "claude"]);
        // Create 的 key 必须指向这把 key 的智谱 key（两种渠道同一把 key）
        for op in &ops {
            if let ChannelOp::Create { key, .. } = op {
                assert_eq!(*key, "k-zhipu-1");
            }
        }
    }

    #[test]
    fn 全已存在_双渠道都skip() {
        let keys = [key("zhipu-1")];
        let (o, c) = (openai_tpl(), claude_tpl());
        let ops = plan_channel_ops(&keys, &existing(&["zhipu-1", "zhipu-1-cc"]), Some(&o), Some(&c));
        assert_eq!(kinds(&ops), vec!["openai", "claude"]); // 全 Skip，无 Create
        assert!(ops.iter().all(|op| matches!(op, ChannelOp::Skip { .. })));
    }

    #[test]
    fn 未配claude模板_只产openai槽_不warn不建() {
        let keys = [key("zhipu-1")];
        let o = openai_tpl();
        let ops = plan_channel_ops(&keys, &existing(&[]), Some(&o), None);
        assert_eq!(names(&ops), vec!["zhipu-1"]);
        assert_eq!(kinds(&ops), vec!["openai"]);
    }

    #[test]
    fn openai无模板且渠道缺_产missing() {
        let keys = [key("zhipu-1")];
        let ops = plan_channel_ops(&keys, &existing(&[]), None, None);
        assert_eq!(
            ops,
            vec![ChannelOp::Missing { name: "zhipu-1".into() }]
        );
    }

    #[test]
    fn 混合_key1全skip_key2双create() {
        let keys = [key("zhipu-1"), key("zhipu-2")];
        let (o, c) = (openai_tpl(), claude_tpl());
        let ops = plan_channel_ops(
            &keys,
            &existing(&["zhipu-1", "zhipu-1-cc"]),
            Some(&o),
            Some(&c),
        );
        assert_eq!(names(&ops), vec!["zhipu-1", "zhipu-1-cc", "zhipu-2", "zhipu-2-cc"]);
        assert_eq!(
            kinds(&ops),
            vec!["openai", "claude", "openai", "claude"]
        );
        // 前两个 Skip、后两个 Create
        assert!(matches!(ops[0], ChannelOp::Skip { .. }));
        assert!(matches!(ops[1], ChannelOp::Skip { .. }));
        assert!(matches!(ops[2], ChannelOp::Create { .. }));
        assert!(matches!(ops[3], ChannelOp::Create { .. }));
    }

    #[test]
    fn 单侧存在_只补缺的那侧() {
        // 主渠道在、cc 不在 → 只建 cc（对拍「半途而废」的存量状态）
        let keys = [key("zhipu-1")];
        let (o, c) = (openai_tpl(), claude_tpl());
        let ops = plan_channel_ops(&keys, &existing(&["zhipu-1"]), Some(&o), Some(&c));
        assert_eq!(names(&ops), vec!["zhipu-1", "zhipu-1-cc"]);
        assert!(matches!(ops[0], ChannelOp::Skip { .. }));
        assert!(matches!(ops[1], ChannelOp::Create { .. }));
    }
}
