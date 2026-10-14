//! 单实例共享管理预算、独立身份会话。没有业务重试，也不输出请求/响应载荷。
use crate::config::NewApiConfig;
use reqwest::{header, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

pub const SINGLE_BODY_LIMIT: usize = 1024 * 1024;
pub const LIST_BODY_LIMIT: usize = 8 * SINGLE_BODY_LIMIT;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const QUEUE_LIMIT: usize = 64;
const TOKEN_NS: u128 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestClass {
    Control,
    Observe,
    Backfill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestSemantics {
    Read,
    IdempotentWrite,
    Create,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagementErrorKind {
    InvalidInput,
    QueueFull,
    AuthFailed,
    RemoteRejected,
    InvalidResponse,
    RateLimited,
    TransportUnknown,
}

/// 不保存底层错误链（URL、header 和服务器正文均可能含秘密）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagementError {
    pub kind: ManagementErrorKind,
    pub status: Option<u16>,
    pub retry_after: Option<Duration>,
}
impl fmt::Display for ManagementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "management: {:?}", self.kind)?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        Ok(())
    }
}
impl std::error::Error for ManagementError {}
impl ManagementError {
    fn for_request(mut self, semantics: RequestSemantics) -> Self {
        if semantics != RequestSemantics::Read && self.kind == ManagementErrorKind::InvalidResponse
        {
            self.kind = ManagementErrorKind::TransportUnknown;
        }
        self
    }
}
pub type Result<T> = std::result::Result<T, ManagementError>;
pub(super) fn error(kind: ManagementErrorKind) -> ManagementError {
    ManagementError {
        kind,
        status: None,
        retry_after: None,
    }
}

/// 字段私有，不实现 Debug/Serialize，防止渠道 key 经诊断泄漏。
pub struct ManagementRequest {
    method: Method,
    path: String,
    body: Option<Vec<u8>>,
    semantics: RequestSemantics,
    response_limit: usize,
}
impl ManagementRequest {
    pub fn new(
        method: Method,
        path: &str,
        body: Option<Value>,
        semantics: RequestSemantics,
        response_limit: usize,
    ) -> Result<Self> {
        // Step P1: 校验管理路径、语义和大小；拒绝绝对 URL、路径跳转及隐式重定向。
        if !path.starts_with("/api/")
            || path.len() > 8192
            || path.contains(['#', '\\', '%'])
            || path
                .split('?')
                .next()
                .unwrap_or("")
                .split('/')
                .any(|p| p == ".." || p == ".")
            || path.contains("//")
            || path.bytes().any(|b| b.is_ascii_control())
            || response_limit == 0
            || response_limit > LIST_BODY_LIMIT
            || !matches!(
                (&method, semantics),
                (&Method::GET, RequestSemantics::Read)
                    | (&Method::PUT, RequestSemantics::IdempotentWrite)
                    | (
                        &Method::POST,
                        RequestSemantics::IdempotentWrite | RequestSemantics::Create
                    )
            )
            || (method == Method::GET && body.is_some())
        {
            return Err(error(ManagementErrorKind::InvalidInput));
        }
        let body = body
            .map(|b| serde_json::to_vec(&b))
            .transpose()
            .map_err(|_| error(ManagementErrorKind::InvalidInput))?;
        if body.as_ref().is_some_and(|b| b.len() > SINGLE_BODY_LIMIT) {
            return Err(error(ManagementErrorKind::InvalidInput));
        }
        Ok(Self {
            method,
            path: path.into(),
            body,
            semantics,
            response_limit,
        })
    }
}

pub struct ManagementResponse {
    status: StatusCode,
    body: Vec<u8>,
}
impl ManagementResponse {
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub async fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|_| error(ManagementErrorKind::InvalidResponse))
    }
    pub fn envelope(&self) -> Result<Value> {
        let body: Value = serde_json::from_slice(&self.body)
            .map_err(|_| error(ManagementErrorKind::InvalidResponse))?;
        match body.get("success").and_then(Value::as_bool) {
            Some(true) => Ok(body),
            Some(false) => Err(error(ManagementErrorKind::RemoteRejected)),
            None => Err(error(ManagementErrorKind::InvalidResponse)),
        }
    }
}

struct Waiter {
    id: u64,
    class: RequestClass,
}
struct BudgetState {
    credit: u128,
    last: Instant,
    cooldown: Instant,
    unbounded_cooldown: bool,
    next_id: u64,
    queue: VecDeque<Waiter>,
    active: usize,
    background: usize,
}
/// 显式共享而不是全局单例；后续普通签发身份必须复用同一个 Arc。
pub struct ManagementBudget {
    state: Mutex<BudgetState>,
    changed: Notify,
    requests: Arc<Semaphore>,
}
impl ManagementBudget {
    pub fn new() -> Arc<Self> {
        let now = Instant::now();
        Arc::new(Self {
            state: Mutex::new(BudgetState {
                credit: 2 * TOKEN_NS,
                last: now,
                cooldown: now,
                unbounded_cooldown: false,
                next_id: 0,
                queue: VecDeque::new(),
                active: 0,
                background: 0,
            }),
            changed: Notify::new(),
            requests: Arc::new(Semaphore::new(QUEUE_LIMIT)),
        })
    }

    fn register_request(&self) -> Result<OwnedSemaphorePermit> {
        self.requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| error(ManagementErrorKind::QueueFull))
    }
    fn cool_down(&self, delay: Duration) {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        // 不可表示的截止时间保持熔断，不截短服务器要求。
        match now.checked_add(delay) {
            Some(until) => state.cooldown = state.cooldown.max(until),
            None => state.unbounded_cooldown = true,
        }
        drop(state);
        self.changed.notify_waiters();
    }
    async fn admit(self: &Arc<Self>, class: RequestClass) -> Result<Admission> {
        let id = {
            let mut state = self.state.lock().unwrap();
            if state.queue.len() >= QUEUE_LIMIT {
                return Err(error(ManagementErrorKind::QueueFull));
            }
            let id = state.next_id;
            state.next_id = state
                .next_id
                .checked_add(1)
                .ok_or_else(|| error(ManagementErrorKind::QueueFull))?;
            state.queue.push_back(Waiter { id, class });
            id
        };
        let mut admission = Admission {
            budget: self.clone(),
            id,
            class,
            active: false,
        };
        loop {
            // 注册再检查，避免 drop/新高优先请求的通知落在检查与等待之间。
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let wake = {
                let now = Instant::now();
                let mut state = self.state.lock().unwrap();
                state.credit =
                    (state.credit + now.duration_since(state.last).as_nanos()).min(2 * TOKEN_NS);
                state.last = now;
                if state.unbounded_cooldown
                    || state.cooldown.saturating_duration_since(now) > Duration::from_secs(60)
                {
                    return Err(ManagementError {
                        kind: ManagementErrorKind::RateLimited,
                        status: Some(429),
                        retry_after: Some(if state.unbounded_cooldown {
                            Duration::from_secs(u64::MAX)
                        } else {
                            state.cooldown - now
                        }),
                    });
                }
                let selected = state
                    .queue
                    .iter()
                    .filter(|q| q.class == RequestClass::Control || state.background == 0)
                    .min_by_key(|q| (q.class, q.id))
                    .map(|q| q.id);
                if state.active < 2
                    && selected == Some(id)
                    && state.cooldown <= now
                    && state.credit >= TOKEN_NS
                {
                    state.queue.retain(|q| q.id != id);
                    state.credit -= TOKEN_NS;
                    state.active += 1;
                    state.background += usize::from(class != RequestClass::Control);
                    admission.active = true;
                    drop(state);
                    self.changed.notify_waiters();
                    return Ok(admission);
                }
                if state.active < 2 && selected == Some(id) {
                    Some(state.cooldown.max(
                        now + Duration::from_nanos(TOKEN_NS.saturating_sub(state.credit) as u64),
                    ))
                } else {
                    None
                }
            };
            match wake {
                Some(at) => {
                    tokio::select! { _ = &mut changed => {}, _ = tokio::time::sleep_until(at) => {} }
                }
                None => changed.await,
            }
        }
    }
}
struct Admission {
    budget: Arc<ManagementBudget>,
    id: u64,
    class: RequestClass,
    active: bool,
}
impl Drop for Admission {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock().unwrap();
        if self.active {
            state.active -= 1;
            state.background -= usize::from(self.class != RequestClass::Control);
        } else {
            state.queue.retain(|q| q.id != self.id);
        }
        drop(state);
        self.budget.changed.notify_waiters();
    }
}

#[derive(Clone)]
enum Auth {
    Token(header::HeaderValue),
    Pending,
    Session(i64),
}
struct AuthState {
    auth: Auth,
    generation: u64,
    failed: bool,
}
pub struct ManagementTransport {
    client: reqwest::Client,
    base_url: String,
    budget: Arc<ManagementBudget>,
    auth: Mutex<AuthState>,
    refresh: AsyncMutex<()>,
    username: String,
    password: String,
    extra_headers: header::HeaderMap,
}
impl ManagementTransport {
    pub fn new(cfg: &NewApiConfig, budget: Arc<ManagementBudget>) -> Result<Self> {
        let url =
            Url::parse(&cfg.base_url).map_err(|_| error(ManagementErrorKind::InvalidInput))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(error(ManagementErrorKind::InvalidInput));
        }
        let auth = if cfg.admin_token.trim().is_empty() {
            Auth::Pending
        } else {
            let mut value = header::HeaderValue::from_str(&format!("Bearer {}", cfg.admin_token))
                .map_err(|_| error(ManagementErrorKind::InvalidInput))?;
            value.set_sensitive(true);
            Auth::Token(value)
        };
        let mut extra_headers = header::HeaderMap::new();
        for kv in &cfg.extra_headers {
            let name = header::HeaderName::from_bytes(kv.key.as_bytes())
                .map_err(|_| error(ManagementErrorKind::InvalidInput))?;
            if matches!(name.as_str(), "authorization" | "cookie" | "host") {
                return Err(error(ManagementErrorKind::InvalidInput));
            }
            let mut value = header::HeaderValue::from_str(&kv.value)
                .map_err(|_| error(ManagementErrorKind::InvalidInput))?;
            value.set_sensitive(true);
            extra_headers.insert(name, value);
        }
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|_| error(ManagementErrorKind::InvalidInput))?;
        Ok(Self {
            client,
            base_url: url.as_str().trim_end_matches('/').into(),
            budget,
            auth: Mutex::new(AuthState {
                auth,
                generation: 0,
                failed: false,
            }),
            refresh: AsyncMutex::new(()),
            username: cfg.root_username.clone(),
            password: cfg.root_password.clone(),
            extra_headers,
        })
    }

    pub async fn send(
        &self,
        request: ManagementRequest,
        class: RequestClass,
    ) -> Result<ManagementResponse> {
        // 包括等待 refresh 锁的调用；不能让认证等待绕开 64 槽上限。
        let _request = self.budget.register_request()?;
        let pending = {
            let auth = self.auth.lock().unwrap();
            if auth.failed {
                return Err(error(ManagementErrorKind::AuthFailed));
            }
            matches!(auth.auth, Auth::Pending)
        };
        if pending {
            self.refresh_session(0).await?;
        }
        for attempt in 0..2 {
            // Step P2: 每次实际发送均先获共享预算；普通请求不持有 auth 锁。
            let admission = self.budget.admit(class).await?;
            let (auth, generation) = {
                let state = self.auth.lock().unwrap();
                if state.failed {
                    return Err(error(ManagementErrorKind::AuthFailed));
                }
                (state.auth.clone(), state.generation)
            };
            let response = self.execute(&request, Some(&auth)).await;
            drop(admission);
            // Step P3: 释放在途槽后才能单飞登录；仅明确 401 允许重放创建。
            match response {
                Ok(response) if response.status == StatusCode::UNAUTHORIZED => {
                    if attempt != 0 || matches!(auth, Auth::Token(_)) {
                        self.fail_generation(generation);
                        return Err(error(ManagementErrorKind::AuthFailed));
                    }
                    self.refresh_session(generation).await?;
                }
                Ok(response) => {
                    response
                        .envelope()
                        .map_err(|err| err.for_request(request.semantics))?;
                    return Ok(response);
                }
                Err(err) => return Err(err.for_request(request.semantics)),
            }
        }
        unreachable!("at most one authenticated replay")
    }

    fn fail_generation(&self, generation: u64) {
        let mut state = self.auth.lock().unwrap();
        if state.generation == generation {
            state.failed = true;
        }
    }

    async fn refresh_session(&self, observed_generation: u64) -> Result<()> {
        let _refresh = self.refresh.lock().await;
        {
            let state = self.auth.lock().unwrap();
            if state.failed {
                return Err(error(ManagementErrorKind::AuthFailed));
            }
            if state.generation != observed_generation {
                return Ok(());
            }
            if matches!(state.auth, Auth::Token(_)) {
                return Err(error(ManagementErrorKind::AuthFailed));
            }
        }
        let request = ManagementRequest::new(
            Method::POST,
            "/api/user/login",
            Some(json!({
                "username": self.username, "password": self.password
            })),
            RequestSemantics::Create,
            SINGLE_BODY_LIMIT,
        )?;
        let _admission = self.budget.admit(RequestClass::Control).await?;
        // 登录已经发出后取消也不能让下一位等待者无条件再打登录。
        let mut cancellation = RefreshCancellation {
            transport: self,
            generation: observed_generation,
            completed: false,
        };
        let result = self
            .execute(&request, None)
            .await
            .and_then(|r| r.envelope())
            .and_then(|b| {
                b.get("data")
                    .and_then(|d| d.get("id"))
                    .and_then(Value::as_i64)
                    .filter(|id| *id > 0)
                    .ok_or_else(|| error(ManagementErrorKind::AuthFailed))
            });
        cancellation.completed = true;
        let mut state = self.auth.lock().unwrap();
        match result {
            Ok(user_id) => {
                state.auth = Auth::Session(user_id);
                state.generation += 1;
                Ok(())
            }
            Err(_) => {
                state.failed = true;
                Err(error(ManagementErrorKind::AuthFailed))
            }
        }
    }

    /// 显式管理动作才可解除认证熔断；不隐式 setup，不重放旧业务命令。
    pub async fn retry_authentication(&self) -> Result<()> {
        let _request = self.budget.register_request()?;
        {
            let _refresh = self.refresh.lock().await;
            self.auth.lock().unwrap().failed = false;
        }
        let generation = {
            let state = self.auth.lock().unwrap();
            if matches!(state.auth, Auth::Token(_)) {
                return Ok(());
            }
            state.generation
        };
        self.refresh_session(generation).await
    }

    /// 仅旧 CLI 的显式 authenticate 首启引导可调用；自动 401 恢复绝不调用它。
    pub async fn bootstrap(&self) -> Result<()> {
        let _request = self.budget.register_request()?;
        let _refresh = self.refresh.lock().await;
        {
            let state = self.auth.lock().unwrap();
            if state.failed {
                return Err(error(ManagementErrorKind::AuthFailed));
            }
            if !matches!(state.auth, Auth::Pending) {
                return Ok(());
            }
        }
        let request = ManagementRequest::new(
            Method::GET,
            "/api/setup",
            None,
            RequestSemantics::Read,
            SINGLE_BODY_LIMIT,
        )?;
        let body = self.unauthenticated(&request).await?.envelope()?;
        let data = body
            .get("data")
            .ok_or_else(|| error(ManagementErrorKind::InvalidResponse))?;
        let status = data
            .get("status")
            .and_then(Value::as_bool)
            .ok_or_else(|| error(ManagementErrorKind::InvalidResponse))?;
        let root_init = data
            .get("root_init")
            .and_then(Value::as_bool)
            .ok_or_else(|| error(ManagementErrorKind::InvalidResponse))?;
        if !status && !root_init {
            let request = ManagementRequest::new(
                Method::POST,
                "/api/setup",
                Some(json!({
                    "username": self.username, "password": self.password, "confirmPassword": self.password,
                    "SelfUseModeEnabled": true, "DemoSiteEnabled": false
                })),
                RequestSemantics::Create,
                SINGLE_BODY_LIMIT,
            )?;
            self.unauthenticated(&request).await?.envelope()?;
        }
        drop(_refresh);
        self.refresh_session(0).await
    }

    async fn unauthenticated(&self, request: &ManagementRequest) -> Result<ManagementResponse> {
        let _admission = self.budget.admit(RequestClass::Control).await?;
        let response = self
            .execute(request, None)
            .await
            .map_err(|err| err.for_request(request.semantics))?;
        response
            .envelope()
            .map_err(|err| err.for_request(request.semantics))?;
        Ok(response)
    }

    async fn execute(
        &self,
        request: &ManagementRequest,
        auth: Option<&Auth>,
    ) -> Result<ManagementResponse> {
        let future = async {
            let mut headers = self.extra_headers.clone();
            match auth {
                Some(Auth::Token(value)) => {
                    headers.insert(header::AUTHORIZATION, value.clone());
                }
                Some(Auth::Session(id)) => {
                    // RequestBuilder::header 是 append；必须先在 HeaderMap 中覆盖旧静态 ID。
                    headers.insert(
                        "new-api-user",
                        header::HeaderValue::from_str(&id.to_string())
                            .map_err(|_| error(ManagementErrorKind::InvalidInput))?,
                    );
                }
                _ => {}
            }
            let mut builder = self
                .client
                .request(
                    request.method.clone(),
                    format!("{}{}", self.base_url, request.path),
                )
                .headers(headers);
            if let Some(body) = &request.body {
                builder = builder
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body.clone());
            }
            let mut response = builder
                .send()
                .await
                .map_err(|_| error(ManagementErrorKind::TransportUnknown))?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                let delay = retry_after(
                    response.headers().get(header::RETRY_AFTER),
                    SystemTime::now(),
                );
                self.budget.cool_down(delay);
                return Err(ManagementError {
                    kind: ManagementErrorKind::RateLimited,
                    status: Some(429),
                    retry_after: Some(delay),
                });
            }
            // 401 的鉴权拒绝已明确，不读取不可信错误正文；其余写响应不能假定未执行。
            if status == StatusCode::UNAUTHORIZED {
                return Ok(ManagementResponse {
                    status,
                    body: Vec::new(),
                });
            }
            if !status.is_success() {
                return Err(ManagementError {
                    kind: if status.is_server_error() && request.semantics != RequestSemantics::Read
                    {
                        ManagementErrorKind::TransportUnknown
                    } else {
                        ManagementErrorKind::RemoteRejected
                    },
                    status: Some(status.as_u16()),
                    retry_after: None,
                });
            }
            if response
                .content_length()
                .is_some_and(|n| n > request.response_limit as u64)
            {
                return Err(error(ManagementErrorKind::InvalidResponse));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| error(ManagementErrorKind::TransportUnknown))?
            {
                if chunk.len() > request.response_limit - body.len() {
                    return Err(error(ManagementErrorKind::InvalidResponse));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(ManagementResponse { status, body })
        };
        tokio::time::timeout(HTTP_TIMEOUT, future)
            .await
            .map_err(|_| error(ManagementErrorKind::TransportUnknown))?
    }
}

fn retry_after(value: Option<&header::HeaderValue>, now: SystemTime) -> Duration {
    let Some(text) = value.and_then(|v| v.to_str().ok()) else {
        return Duration::from_secs(4);
    };
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return Duration::from_secs(text.parse::<u64>().unwrap_or(u64::MAX));
    }
    httpdate::parse_http_date(text)
        .map(|at| at.duration_since(now).unwrap_or_default())
        .unwrap_or(Duration::from_secs(4))
}

struct RefreshCancellation<'a> {
    transport: &'a ManagementTransport,
    generation: u64,
    completed: bool,
}
impl Drop for RefreshCancellation<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.transport.fail_generation(self.generation);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
