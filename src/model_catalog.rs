//! 上游模型目录发现。
//!
//! 这里只知道 OpenAI 兼容的 `GET /models`，不知道 new-api 渠道、分组或 priority。
//! 上游 key 是敏感信息：错误上下文只写 URL/状态，绝不打印 header 或 key。

use crate::config::{ModelDiscoveryAuth, ModelDiscoveryConfig};
use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderName, AUTHORIZATION};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

#[derive(Clone)]
pub struct ModelCatalogClient {
    client: reqwest::Client,
}

impl ModelCatalogClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// 发现某一把 key 当前实际可见的模型。成功结果保证非空、trim 且去重。
    pub async fn discover(&self, cfg: &ModelDiscoveryConfig, key: &str) -> Result<Vec<String>> {
        let request = apply_discovery_auth(
            self.client.get(&cfg.url).timeout(Duration::from_secs(10)),
            cfg.auth,
            key,
        );

        let response = request
            .send()
            .await
            .with_context(|| format!("请求模型目录失败: {}", cfg.url))?;
        let status = response.status();
        if !status.is_success() {
            bail!("模型目录返回 HTTP {status}: {}", cfg.url);
        }
        let body: Value = response
            .json()
            .await
            .with_context(|| format!("模型目录不是合法 JSON: {}", cfg.url))?;
        parse_model_ids(&body).with_context(|| format!("模型目录结构非法: {}", cfg.url))
    }
}

fn apply_discovery_auth(
    request: reqwest::RequestBuilder,
    auth: ModelDiscoveryAuth,
    key: &str,
) -> reqwest::RequestBuilder {
    match auth {
        ModelDiscoveryAuth::Bearer => request.bearer_auth(key),
        ModelDiscoveryAuth::AuthorizationRaw => request.header(AUTHORIZATION, key),
        ModelDiscoveryAuth::XApiKey => request.header(HeaderName::from_static("x-api-key"), key),
    }
}

/// 解析 OpenAI 兼容的 `{ "data": [{ "id": "..." }] }`。
fn parse_model_ids(body: &Value) -> Result<Vec<String>> {
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .context("缺少 data 数组")?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for item in data {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let id = id.trim();
        if !id.is_empty() && seen.insert(id.to_string()) {
            models.push(id.to_string());
        }
    }
    if models.is_empty() {
        bail!("data 中没有非空模型 id");
    }
    Ok(models)
}

/// CSV 规范化：trim、丢空、按首次出现去重。供 fallback 和 new-api 回读比较共用。
pub fn normalize_models_csv(csv: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    csv.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert((*s).to_string()))
        .map(str::to_string)
        .collect()
}

/// new-api 的 models 顺序无语义；只比较规范化后的集合，避免上游换序造成无效 PUT。
pub fn model_sets_equal(left: &str, right: &str) -> bool {
    let left: HashSet<String> = normalize_models_csv(left).into_iter().collect();
    let right: HashSet<String> = normalize_models_csv(right).into_iter().collect();
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 解析模型列表_trim去空并保持首次顺序() {
        let body = json!({
            "data": [
                {"id": " glm-5.3 "},
                {"id": ""},
                {"id": "glm-5.3"},
                {"missing": true},
                {"id": "glm-5.3-flash"}
            ]
        });
        assert_eq!(
            parse_model_ids(&body).unwrap(),
            vec!["glm-5.3", "glm-5.3-flash"]
        );
    }

    #[test]
    fn 空目录与错误结构都拒绝() {
        assert!(parse_model_ids(&json!({"data": []})).is_err());
        assert!(parse_model_ids(&json!({"data": [{"id": "  "}]})).is_err());
        assert!(parse_model_ids(&json!({"models": []})).is_err());
    }

    #[test]
    fn 模型集合比较忽略顺序空白与重复() {
        assert!(model_sets_equal(
            "glm-5.3, glm-5.3-flash,glm-5.3",
            "glm-5.3-flash,glm-5.3"
        ));
        assert!(!model_sets_equal("glm-5.3", "glm-5.3,glm-5.3-flash"));
    }

    #[test]
    fn 三种鉴权头都按配置构造() {
        let cases = [
            (
                ModelDiscoveryAuth::Bearer,
                "authorization",
                "Bearer secret-key",
            ),
            (
                ModelDiscoveryAuth::AuthorizationRaw,
                "authorization",
                "secret-key",
            ),
            (ModelDiscoveryAuth::XApiKey, "x-api-key", "secret-key"),
        ];
        let client = reqwest::Client::new();
        for (auth, header, expected) in cases {
            let request = apply_discovery_auth(
                client.get("https://example.test/v1/models"),
                auth,
                "secret-key",
            )
            .build()
            .unwrap();
            assert_eq!(request.headers()[header], expected);
        }
    }
}
