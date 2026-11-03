//! 编译期固定资源白名单；请求不执行文件或业务 I/O。
use super::{request::RequestHead, ContentType, HttpErrorCode, Response, StartupError};
use std::sync::Arc;

pub(super) const CSP_HEADER: &str = "Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'none'\r\n";
const CONFIG_PATH: &str = "/api/admin/v1/ui-config";
const MAX_URL: usize = 8192;
const ASSETS: &[(&str, ContentType, &[u8])] = &[
    ("/", ContentType::Html, include_bytes!("ui/index.html")),
    (
        "/assets/panel.css",
        ContentType::Css,
        include_bytes!("ui/panel.css"),
    ),
    (
        "/assets/app.mjs",
        ContentType::JavaScript,
        include_bytes!("ui/app.mjs"),
    ),
    (
        "/assets/client.mjs",
        ContentType::JavaScript,
        include_bytes!("ui/client.mjs"),
    ),
    (
        "/assets/core.mjs",
        ContentType::JavaScript,
        include_bytes!("ui/core.mjs"),
    ),
    (
        "/assets/render.mjs",
        ContentType::JavaScript,
        include_bytes!("ui/render.mjs"),
    ),
    (
        "/assets/charts.mjs",
        ContentType::JavaScript,
        include_bytes!("ui/charts.mjs"),
    ),
    (
        "/assets/contract.mjs",
        ContentType::JavaScript,
        include_bytes!("ui/contract.mjs"),
    ),
];

#[derive(Default)]
pub(crate) struct UiOptions {
    public_api_base_url: Option<String>,
}
impl UiOptions {
    pub(crate) fn new(public_api_base_url: Option<String>) -> Result<Self, StartupError> {
        // Step U1: 公开推理根地址只能由可信启动参数显式提供。
        let public_api_base_url = public_api_base_url.map(normalize_root).transpose()?;
        Ok(Self {
            public_api_base_url,
        })
    }
}
fn normalize_root(raw: String) -> Result<String, StartupError> {
    let bad = StartupError::InvalidUiConfig;
    if raw.len() > MAX_URL || raw.trim() != raw || raw.chars().any(char::is_control) {
        return Err(bad);
    }
    let url = reqwest::Url::parse(&raw).map_err(|_| bad)?;
    // 空 userinfo 和反斜杠也拒绝，不能依赖 URL 解析器悄悄归一化。
    let authority = raw
        .split_once("://")
        .ok_or(bad)?
        .1
        .split('/')
        .next()
        .ok_or(bad)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || authority.contains('@')
        || raw.contains('\\')
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(bad);
    }
    // URL 解析器容忍的非法转义仍违反公开 URI 契约。
    let mut path_bytes = url.path().bytes();
    while let Some(b) = path_bytes.next() {
        if !b.is_ascii_alphanumeric() && !b"-._~!$&'()*+,;=:@/%".contains(&b) {
            return Err(bad);
        }
        if b == b'%'
            && !(path_bytes.next().is_some_and(|b| b.is_ascii_hexdigit())
                && path_bytes.next().is_some_and(|b| b.is_ascii_hexdigit()))
        {
            return Err(bad);
        }
    }
    let segment = url.path().rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
    let mut decoded = Vec::with_capacity(segment.len());
    let mut bytes = segment.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = bytes
                .next()
                .and_then(|v| (v as char).to_digit(16))
                .ok_or(bad)?;
            let lo = bytes
                .next()
                .and_then(|v| (v as char).to_digit(16))
                .ok_or(bad)?;
            decoded.push((hi * 16 + lo) as u8);
        } else {
            decoded.push(b);
        }
    }
    if decoded.eq_ignore_ascii_case(b"v1") || url.as_str().len() > MAX_URL {
        return Err(bad);
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub(super) struct UiBundle {
    assets: Vec<(&'static str, ContentType, Arc<[u8]>)>,
    config: Arc<[u8]>,
}
impl UiBundle {
    pub(super) fn new(options: UiOptions, dry_run: bool) -> Result<Self, HttpErrorCode> {
        // 在 accept 循环前仅编码和分配一次。
        let config = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "dry_run": dry_run,
            "public_api_base_url": options.public_api_base_url,
        }))
        .map_err(|_| HttpErrorCode::Internal)?;
        if config.len() > 16 * 1024
            || ASSETS.iter().any(|(_, _, b)| b.len() > 512 * 1024)
            || ASSETS.iter().map(|(_, _, b)| b.len()).sum::<usize>() > 1024 * 1024
        {
            return Err(HttpErrorCode::Internal);
        }
        Ok(Self {
            assets: ASSETS
                .iter()
                .map(|(p, t, b)| (*p, *t, Arc::from(*b)))
                .collect(),
            config: config.into(),
        })
    }
    pub(super) fn response(&self, head: &RequestHead) -> Option<Response> {
        let (content_type, body, allow) = if head.path == CONFIG_PATH {
            (ContentType::Json, &self.config, "GET")
        } else {
            let (_, mime, bytes) = self.assets.iter().find(|(path, _, _)| *path == head.path)?;
            (*mime, bytes, "GET, HEAD")
        };
        if head.query.is_some() {
            return Some(Response::error(HttpErrorCode::BadRequest));
        }
        if head.method != "GET" && !(head.method == "HEAD" && allow == "GET, HEAD") {
            let mut response = Response::error(HttpErrorCode::MethodNotAllowed);
            response.allow = Some(allow);
            return Some(response);
        }
        Some(Response {
            status: "200 OK",
            body: body.clone(),
            age_ms: None,
            allow: None,
            content_type,
        })
    }
}

#[cfg(test)]
mod tests;
