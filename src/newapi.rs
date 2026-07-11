//! new-api 管理 API 客户端。
//!
//! 设计上刻意不依赖渠道的完整 schema：改字段用「GET 渠道 → 改目标字段 → PUT 回去」，
//! 渠道对象当成 serde_json::Value 整体搬运，只动我们关心的那个字段。这样对 new-api
//! 版本差异最鲁棒。
//!
//! ⚠️ channel_path 和 extra_headers 请用 F12 抓一次后台“编辑渠道→保存”的真实请求核实。
//! 同时确认渠道对象里确实有 `priority` 字段，且 new-api 路由是“优先取最高 priority”。

use crate::config::NewApiConfig;
use anyhow::{Context, Result};
use serde_json::Value;

pub struct NewApiClient {
    client: reqwest::Client,
    base_url: String,
    channel_path: String,
    admin_token: String,
    extra_headers: Vec<(String, String)>,
}

impl NewApiClient {
    pub fn new(cfg: &NewApiConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            channel_path: cfg.channel_path.clone(),
            admin_token: cfg.admin_token.clone(),
            extra_headers: cfg
                .extra_headers
                .iter()
                .map(|h| (h.key.clone(), h.value.clone()))
                .collect(),
        }
    }

    fn apply_headers(&self, mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb = rb.header("Authorization", format!("Bearer {}", self.admin_token));
        for (k, v) in &self.extra_headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        rb
    }

    /// GET /api/channel/{id} → 返回渠道对象（从 `data` 字段里取出）
    pub async fn get_channel(&self, id: i64) -> Result<Value> {
        let url = format!("{}{}/{}", self.base_url, self.channel_path, id);
        let rb = self.apply_headers(self.client.get(&url));
        let resp = rb.send().await.context("获取渠道失败")?;
        let body: Value = resp.json().await.context("解析渠道响应失败")?;
        let data = body
            .get("data")
            .cloned()
            .context("渠道响应缺少 data 字段（请用 F12 核实实际结构）")?;
        Ok(data)
    }

    /// 取渠道 → 设置某个整数字段 → PUT 回去。整体搬运，只动这一个字段。
    async fn set_channel_field(&self, id: i64, field: &str, value: i64) -> Result<()> {
        let mut channel = self.get_channel(id).await?;
        match channel.as_object_mut() {
            Some(obj) => {
                obj.insert(field.to_string(), Value::from(value));
            }
            None => anyhow::bail!("渠道 {} 返回的不是 JSON 对象", id),
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
            anyhow::bail!(
                "更新渠道 {} 字段 {} 失败: HTTP {} body={}",
                id,
                field,
                status,
                body
            );
        }
        Ok(())
    }

    /// 设置渠道 priority。这是本工具「钉住单把活动 key」的唯一杠杆：
    /// 活动 key 给最高 priority 独占，其余给低档作 429 兜底。
    pub async fn set_channel_priority(&self, id: i64, priority: i64) -> Result<()> {
        self.set_channel_field(id, "priority", priority).await
    }
}
