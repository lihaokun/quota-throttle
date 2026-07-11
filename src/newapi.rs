//! new-api 管理 API 客户端。
//!
//! 两件事：
//!   1. 鉴权——优先用配置里的 admin_token(Bearer)；没有就用 root 账号登录拿会话 cookie。
//!   2. 渠道——列出/创建（sync 按 key 列表对齐渠道并解析 channel_id）、以及运行期改 priority。
//!
//! 改 priority 仍用「GET 渠道 → 只改 priority → PUT 回」，整体搬运，对版本差异最鲁棒。
//! ⚠️ channel_path / 建渠道字段 / 是否需要 New-Api-User，请用 F12 抓真实请求核实。

use crate::config::{ChannelTemplate, NewApiConfig};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{info, warn};

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

        let items = body
            .get("data")
            .and_then(|d| d.get("items").and_then(|v| v.as_array()).or_else(|| d.as_array()))
            .cloned()
            .unwrap_or_default();

        let mut map = HashMap::new();
        for it in items {
            if let (Some(name), Some(id)) = (
                it.get("name").and_then(|v| v.as_str()),
                it.get("id").and_then(|v| v.as_i64()),
            ) {
                map.insert(name.to_string(), id);
            }
        }
        Ok(map)
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
