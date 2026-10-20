//! 新管理面HTTP入口，尚未接CLI；单一supervisor拥有连接、投影与业务blocking句柄。
#![cfg_attr(not(test), allow(dead_code))]
use crate::reconcile::worker::ReconcileWorker;
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio::time::{timeout_at, Instant, MissedTickBehavior};

mod admin;
#[cfg(test)]
pub(crate) use admin::tests::support::Running as AdminTestServer;
mod publication;
mod request;
mod ui;
use publication::{wall_ms, ActivePublication, PanelCache, SnapshotReader, PUBLICATION_TTL};
use request::{PanelOrigin, RequestError};
pub(crate) use ui::UiOptions;

const MAX_CONNECTIONS: usize = 16;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(45);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const PUBLISH_INTERVAL: Duration = Duration::from_secs(5);
const STATE_PATH: &str = "/api/admin/v1/state";

macro_rules! http_errors {
    ($($variant:ident => ($wire:literal, $status:literal)),+ $(,)?) => {
        #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
        pub(crate) enum HttpErrorCode { $(#[serde(rename = $wire)] $variant),+ }
        impl HttpErrorCode {
            const VALUES: &'static [Self] = &[$(Self::$variant),+];
            fn status(self) -> &'static str { match self { $(Self::$variant => $status),+ } }
        }
    };
}
http_errors! {
    BadRequest => ("bad_request", "400 Bad Request"),
    HeaderTooLarge => ("header_too_large", "431 Request Header Fields Too Large"),
    BodyTooLarge => ("body_too_large", "413 Payload Too Large"),
    TargetTooLong => ("target_too_long", "414 URI Too Long"),
    LengthRequired => ("length_required", "411 Length Required"),
    UnsupportedMediaType => ("unsupported_media_type", "415 Unsupported Media Type"),
    ExpectationFailed => ("expectation_failed", "417 Expectation Failed"),
    UnsupportedVersion => ("unsupported_version", "505 HTTP Version Not Supported"),
    Forbidden => ("forbidden", "403 Forbidden"),
    NotFound => ("not_found", "404 Not Found"),
    MethodNotAllowed => ("method_not_allowed", "405 Method Not Allowed"),
    Overloaded => ("overloaded", "503 Service Unavailable"),
    RequestTimeout => ("request_timeout", "408 Request Timeout"),
    SnapshotUnavailable => ("snapshot_unavailable", "503 Service Unavailable"),
    Internal => ("internal", "500 Internal Server Error"),
}
#[derive(Serialize)]
struct HttpErrorBody {
    schema_version: u8,
    error: HttpErrorCode,
}
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum StartupError {
    InvalidAddress,
    InvalidOrigin,
    BindFailed,
    InvalidUiConfig,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ContentType {
    Json,
    Html,
    Css,
    JavaScript,
}
impl ContentType {
    fn header(self) -> &'static str {
        match self {
            Self::Json => "application/json; charset=utf-8",
            Self::Html => "text/html; charset=utf-8",
            Self::Css => "text/css; charset=utf-8",
            Self::JavaScript => "text/javascript; charset=utf-8",
        }
    }
}
struct Response {
    status: &'static str,
    body: Arc<[u8]>,
    age_ms: Option<u64>,
    allow: Option<&'static str>,
    content_type: ContentType,
}
impl Response {
    fn error(error: HttpErrorCode) -> Self {
        let body = serde_json::to_vec(&HttpErrorBody {
            schema_version: 1,
            error,
        })
        .unwrap_or_else(|_| br#"{"schema_version":1,"error":"internal"}"#.to_vec());
        Self {
            status: error.status(),
            body: body.into(),
            age_ms: None,
            allow: (error == HttpErrorCode::MethodNotAllowed).then_some("GET"),
            content_type: ContentType::Json,
        }
    }
}

pub(crate) struct PanelServer {
    listener: TcpListener,
    origin: Arc<PanelOrigin>,
    connections: Arc<Semaphore>,
    cache: Arc<PanelCache>,
}
impl PanelServer {
    pub(crate) async fn bind(
        listen: SocketAddr,
        browser_origin: Option<&str>,
    ) -> Result<Self, StartupError> {
        // Step H1.1: 固定回环与唯一authority；显式配置先校验，端口0仅由绑定结果补定。
        if !matches!(listen.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST)
            && !matches!(listen.ip(), IpAddr::V6(ip) if ip == Ipv6Addr::LOCALHOST)
        {
            return Err(StartupError::InvalidAddress);
        }
        let explicit = browser_origin.map(PanelOrigin::parse).transpose()?;
        let listener = TcpListener::bind(listen)
            .await
            .map_err(|_| StartupError::BindFailed)?;
        let actual = listener
            .local_addr()
            .map_err(|_| StartupError::BindFailed)?;
        let origin = match explicit {
            Some(origin) => origin,
            None => PanelOrigin::parse(&format!("http://{actual}"))?,
        };
        Ok(Self {
            listener,
            origin: Arc::new(origin),
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            cache: Arc::new(PanelCache::default()),
        })
    }
    pub(crate) async fn serve(
        self,
        worker: Arc<ReconcileWorker>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<(), HttpErrorCode> {
        self.serve_ui(worker, UiOptions::default(), shutdown).await
    }
    pub(crate) async fn serve_ui(
        self,
        worker: Arc<ReconcileWorker>,
        options: UiOptions,
        shutdown: watch::Receiver<bool>,
    ) -> Result<(), HttpErrorCode> {
        let publisher_worker = worker.clone();
        self.serve_with_ui(
            Arc::new(move || {
                publisher_worker
                    .panel_snapshot()
                    .map_err(|_| publication::PublishError)
            }),
            worker,
            options,
            shutdown,
        )
        .await
    }
    async fn serve_with_worker(
        self,
        reader: SnapshotReader,
        worker: Arc<ReconcileWorker>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<(), HttpErrorCode> {
        self.serve_with_ui(reader, worker, UiOptions::default(), shutdown)
            .await
    }
    async fn serve_with_ui(
        self,
        reader: SnapshotReader,
        worker: Arc<ReconcileWorker>,
        options: UiOptions,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), HttpErrorCode> {
        let ui = Arc::new(ui::UiBundle::new(options, worker.is_dry_run())?);
        // Step H1.7: 唯一supervisor保留每个blocking句柄，timeout不提前腾出active。
        let mut active: Option<ActivePublication> = None;
        let mut clients = JoinSet::new();
        let (business_client, mut business) = admin::Supervisor::new(worker.clone());
        let mut interval = tokio::time::interval(PUBLISH_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut fatal = None;
        loop {
            if *shutdown.borrow() {
                break;
            }
            let due = active
                .as_ref()
                .map(|a| a.started_at + PUBLICATION_TTL)
                .unwrap_or_else(|| Instant::now() + PUBLICATION_TTL);
            let expirable = active.as_ref().is_some_and(|a| !a.expired);
            tokio::select! {
                biased;
                _ = shutdown.changed() => { if *shutdown.borrow() || shutdown.has_changed().is_err() { break; } }
                result = business.jobs.join_next(), if !business.jobs.is_empty() => {
                    if admin::Supervisor::finish(result) { fatal = Some(HttpErrorCode::Internal); }
                }
                done = async {
                    match active.as_mut() {
                        Some(a) => (&mut a.task).await,
                        None => std::future::pending().await,
                    }
                }, if active.is_some() => {
                    if let Some(finished) = active.take() {
                        if let Err(error) = publication::finish_publication(finished, done, &self.cache, Instant::now(), wall_ms(), *shutdown.borrow()) {
                            fatal = Some(error); break;
                        }
                    }
                }
                _ = tokio::time::sleep_until(due), if expirable => {
                    if let Some(job) = active.as_mut() {
                        if let Err(error) = publication::expire_publication(job, &self.cache) { fatal = Some(error); break; }
                    }
                }
                result = clients.join_next(), if !clients.is_empty() => {
                    if matches!(result, Some(Err(_))) { tracing::debug!("panel_connection_failed"); }
                }
                pending = business.receiver.recv(), if business.can_start() => {
                    if let Some(pending) = pending { business.start(pending); }
                }
                _ = interval.tick() => {
                    if active.is_none() && !*shutdown.borrow() {
                        active = Some(publication::start_publication(reader.clone(), Instant::now()));
                    }
                }
                accepted = self.listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let accepted_at = Instant::now();
                            if let Ok(permit) = self.connections.clone().try_acquire_owned() {
                                let (origin, cache) = (self.origin.clone(), self.cache.clone());
                                let business_client = business_client.clone();
                                let ui = ui.clone();
                                clients.spawn(async move {
                                    let _ = timeout_at(accepted_at + CONNECTION_TIMEOUT, serve_connection(stream, permit, origin, cache, business_client, ui, accepted_at)).await;
                                });
                            }
                        }
                        Err(_) => {
                            tracing::debug!("panel_accept_failed");
                            tokio::select! {
                                _ = shutdown.changed() => {},
                                _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                            }
                        }
                    }
                }
            }
        }
        // Step H2.7: 先关闭准入，连接可abort；所有实际业务/publisher与查询尾部必须退出。
        business.close();
        if let Err(error) = self.cache.replace(None) {
            fatal = Some(error);
        }
        drop(self.listener);
        clients.abort_all();
        while clients.join_next().await.is_some() {}
        while !business.jobs.is_empty() {
            if admin::Supervisor::finish(business.jobs.join_next().await) {
                fatal = Some(HttpErrorCode::Internal);
            }
        }
        if let Some(job) = active {
            let _ = job.task.await;
        }
        if worker.wait_usage_idle().await.is_err() {
            fatal = Some(HttpErrorCode::Internal);
        }
        fatal.map_or(Ok(()), Err)
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    _permit: OwnedSemaphorePermit,
    origin: Arc<PanelOrigin>,
    cache: Arc<PanelCache>,
    business: admin::Client,
    ui: Arc<ui::UiBundle>,
    accepted_at: Instant,
) {
    // Step H1.6: 每连接仅首条请求，错误不反射输入，发送失败不追加第二响应。
    let connection_end = accepted_at + CONNECTION_TIMEOUT;
    let (head, buffered, head_completed_at) =
        match request::read_head(&mut stream, accepted_at).await {
            Ok(parsed) => parsed,
            Err(RequestError::Reject(code, head_only)) => {
                respond(
                    &mut stream,
                    Response::error(code),
                    head_only,
                    connection_end,
                )
                .await;
                return;
            }
            Err(RequestError::Closed) => return,
        };
    let head_only = head.method == "HEAD";
    let response = match request::authorize(&head, &origin) {
        Err(code) => Response::error(code),
        Ok(()) => {
            let deadline = (head_completed_at + request::BODY_TIMEOUT).min(connection_end);
            match request::read_body(
                &mut stream,
                buffered,
                head.content_length.unwrap_or(0),
                deadline,
            )
            .await
            {
                Err(RequestError::Closed) => return,
                Err(RequestError::Reject(code, _)) => Response::error(code),
                Ok(body) => {
                    if Instant::now() >= connection_end {
                        return;
                    }
                    if let Some(response) = ui.response(&head) {
                        response
                    } else if head.path != STATE_PATH {
                        business.call(&head, body, connection_end).await
                    } else if head.query.is_some() {
                        Response::error(HttpErrorCode::BadRequest)
                    } else if head.method != "GET" {
                        Response::error(HttpErrorCode::MethodNotAllowed)
                    } else {
                        match cache.get(Instant::now(), wall_ms()) {
                            Ok((body, age_ms)) => Response {
                                status: "200 OK",
                                body,
                                age_ms: Some(age_ms),
                                allow: None,
                                content_type: ContentType::Json,
                            },
                            Err(code) => Response::error(code),
                        }
                    }
                }
            }
        }
    };
    respond(&mut stream, response, head_only, connection_end).await;
}
async fn respond<W: AsyncWrite + Unpin>(
    stream: &mut W,
    response: Response,
    head_only: bool,
    connection_end: Instant,
) -> bool {
    let deadline = (Instant::now() + WRITE_TIMEOUT).min(connection_end);
    if Instant::now() >= deadline {
        return false;
    }
    let mut header = format!("HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nCross-Origin-Resource-Policy: same-origin\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\n", response.status, response.content_type.header(), response.body.len());
    if response.content_type == ContentType::Html {
        header.push_str(ui::CSP_HEADER);
    }
    if let Some(allow) = response.allow {
        header.push_str(&format!("Allow: {allow}\r\n"));
    }
    if let Some(age) = response.age_ms {
        header.push_str(&format!("X-QT-Snapshot-Age-Ms: {age}\r\n"));
    }
    header.push_str("\r\n");
    let body: &[u8] = if head_only { &[] } else { &response.body };
    for bytes in [header.as_bytes(), body] {
        let mut offset = 0;
        while offset < bytes.len() {
            if Instant::now() >= deadline {
                return false;
            }
            match timeout_at(deadline, stream.write(&bytes[offset..])).await {
                Ok(Ok(n)) if n > 0 => offset += n,
                _ => return false,
            }
            if Instant::now() >= deadline {
                return false;
            }
        }
    }
    if Instant::now() >= deadline {
        return false;
    }
    matches!(timeout_at(deadline, stream.flush()).await, Ok(Ok(()))) && Instant::now() < deadline
}

#[cfg(test)]
mod tests;
