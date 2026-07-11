//! new-api 管理 API 客户端。
//!
//! 两件事：
//!   1. 鉴权——优先用配置里的 admin_token(Bearer)；没有就用 root 账号登录拿会话 cookie。
//!   2. 渠道——列出/创建（sync 按 key 列表对齐渠道并解析 channel_id）、以及运行期改 priority。
//!
//! 改 priority 仍用「GET 渠道 → 只改 priority → PUT 回」，整体搬运，对版本差异最鲁棒。
//! ⚠️ channel_path / 建渠道字段 / 是否需要 New-Api-User，请用 F12 抓真实请求核实。

use crate::config::{ChannelTemplate, NewApiConfig};
use crate::status::{ChannelState, RequestLog};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
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

    /// 创建一把 key 对应的渠道（把 name/key/priority 合并进模板 POST）。
    pub async fn create_channel(
        &self,
        tpl: &ChannelTemplate,
        name: &str,
        key: &str,
        priority: i64,
    ) -> Result<()> {
        // new-api 的 AddChannel 期望 { mode, channel:{...} }，channel 是指针，缺了会 nil-panic。
        let payload = json!({
            "mode": "single",
            "channel": {
                "name": name,
                "type": tpl.channel_type,
                "key": key,
                "base_url": tpl.base_url,
                "models": tpl.models,
                "group": tpl.group,
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

    /// 按 key 列表对齐 new-api 渠道：缺的就（用模板）建出来，返回 name → channel_id。
    /// 没配 channel_template 时不建，只解析已有同名渠道。
    pub async fn sync_channels(
        &self,
        keys: &[crate::config::KeyMapping],
        tpl: Option<&ChannelTemplate>,
        standby_priority: i64,
    ) -> Result<HashMap<String, i64>> {
        let existing = self.list_channels().await?;
        let mut created = false;
        for k in keys {
            if existing.contains_key(&k.name) {
                info!(name = %k.name, "渠道已存在，跳过创建");
                continue;
            }
            match tpl {
                Some(tpl) => {
                    info!(name = %k.name, "创建渠道");
                    self.create_channel(tpl, &k.name, &k.zhipu_api_key, standby_priority)
                        .await?;
                    created = true;
                }
                None => warn!(name = %k.name, "渠道不存在且未配 channel_template，无法自动创建"),
            }
        }
        // 有新建就重新拉一遍，拿到新 id
        if created {
            self.list_channels().await
        } else {
            Ok(existing)
        }
    }

    /// 【看板】某渠道**最近 60 秒**的 (rpm, tpm)。纯读。
    ///
    /// 依据：new-api `model/log.go` 的 `SumUsedQuota` —— `// 只统计最近60秒的rpm和tpm`，
    /// 且 `rpmTpmQuery.Where("channel_id = ?", channel)` 支持按渠道过滤。
    /// **rpm > 0 ⟺ 流量正在走这把 key**（「有没有连上」的直接答案）。
    pub async fn channel_rate(&self, channel_id: i64) -> Result<(i64, i64)> {
        // type=2 = LogTypeConsume；start/end 只影响 quota 字段，rpm/tpm 的 60s 窗口由服务端固定
        let url = format!(
            "{}/api/log/stat?type=2&channel={channel_id}&start_timestamp=0&end_timestamp=9999999999",
            self.base_url
        );
        let body: Value = self
            .apply_headers(self.client.get(&url))
            .send()
            .await
            .context("拉取渠道实时速率失败")?
            .json()
            .await
            .context("解析实时速率失败")?;
        let d = body.get("data");
        Ok((
            d.and_then(|x| x.get("rpm")).and_then(|v| v.as_i64()).unwrap_or(0),
            d.and_then(|x| x.get("tpm")).and_then(|v| v.as_i64()).unwrap_or(0),
        ))
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
