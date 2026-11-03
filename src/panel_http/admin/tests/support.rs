use crate::catalog::CatalogStore;
use crate::config::Config;
use crate::reconcile::worker::ReconcileWorker;
use std::os::unix::fs::PermissionsExt;
use std::sync::{mpsc, Arc};

pub struct Fixture {
    pub worker: Arc<ReconcileWorker>,
    pub dir: tempfile::TempDir,
}
pub fn fixture(dry_run: bool) -> Fixture {
    fixture_with(dry_run, |_| {})
}
pub fn fixture_with(dry_run: bool, prepare: impl FnOnce(&mut CatalogStore)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    prepare(&mut store);
    let mut cfg: Config = toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'\nadmin_token='private-h2-admin'").unwrap();
    cfg.dry_run = dry_run;
    let api = Arc::new(crate::newapi::NewApiClient::new(&cfg.new_api).unwrap());
    Fixture {
        worker: ReconcileWorker::new(store, api, Arc::new(cfg)).unwrap(),
        dir,
    }
}
pub struct CoreHold {
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl CoreHold {
    pub fn new(worker: Arc<ReconcileWorker>) -> Self {
        let (release, released) = mpsc::channel();
        let (ready, entered) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            worker.admin_test_core_locked(|| {
                ready.send(()).unwrap();
                let _ = released.recv();
            })
        });
        entered.recv().unwrap();
        Self {
            release: Some(release),
            thread: Some(thread),
        }
    }
}
impl Drop for CoreHold {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub async fn until(mut check: impl FnMut() -> bool) {
    for _ in 0..500_000 {
        if check() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("admin test condition not reached");
}
pub async fn drive(future: impl std::future::Future<Output = ()> + Send + 'static) {
    let task = tokio::spawn(future);
    until(|| task.is_finished()).await;
    task.await.unwrap();
}

pub struct Running {
    pub addr: std::net::SocketAddr,
    pub authority: String,
    pub stop: tokio::sync::watch::Sender<bool>,
    pub task: tokio::task::JoinHandle<Result<(), crate::panel_http::HttpErrorCode>>,
}

// 手动poll真实serve_connection，分段发送期间不让其抢先读取；peek副本从不消费字节。
// current_thread + 明确的已发送/已消费证明使4s头、9s body各自计入accepted_at。
pub struct SlowRefillConnection {
    pub accepted_at: tokio::time::Instant,
    pub response: tokio::task::JoinHandle<Vec<u8>>,
    connection: tokio::task::JoinHandle<()>,
    supervisor: tokio::task::JoinHandle<()>,
}
impl SlowRefillConnection {
    pub async fn start(worker: Arc<ReconcileWorker>) -> Self {
        use crate::panel_http::{
            admin::Supervisor,
            publication::PanelCache,
            request::PanelOrigin,
            serve_connection,
            ui::{UiBundle, UiOptions},
        };
        use std::{future::Future, pin::Pin, task::Poll, time::Duration};
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::{TcpListener, TcpStream},
            sync::Semaphore,
            time::Instant,
        };

        async fn consumed<F: Future<Output = ()>>(
            future: &mut Pin<Box<F>>,
            monitor: &std::net::TcpStream,
        ) {
            // future尚未poll本段：先看到字节确已进入接收端，再让生产parser消费。
            until(|| match monitor.peek(&mut [0; 1]) {
                Ok(n) => n > 0,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
                other => panic!("unexpected socket before parsing: {other:?}"),
            })
            .await;
            for _ in 0..100_000 {
                let pending =
                    std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending()))
                        .await;
                assert!(pending, "connection ended before the complete request");
                match monitor.peek(&mut [0; 1]) {
                    Ok(0) => return,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
                    Ok(_) => tokio::task::yield_now().await,
                    other => panic!("unexpected socket after parsing: {other:?}"),
                }
            }
            panic!("serve_connection did not consume the sent segment");
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(![3000, 3001].contains(&addr.port()));
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.set_nodelay(true).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let accepted_at = Instant::now();
        let socket = stream.into_std().unwrap();
        let monitor = socket.try_clone().unwrap();
        let stream = TcpStream::from_std(socket).unwrap();
        let (business, mut supervisor) = Supervisor::new(worker);
        let supervisor = tokio::spawn(async move {
            let mut open = true;
            while open || !supervisor.jobs.is_empty() {
                tokio::select! {
                    pending = supervisor.receiver.recv(), if open => {
                        if let Some(pending) = pending { supervisor.start(pending); }
                        else { open = false; supervisor.close(); }
                    }
                    result = supervisor.jobs.join_next(), if !supervisor.jobs.is_empty() => {
                        assert!(!Supervisor::finish(result));
                    }
                }
            }
        });
        let permit = Arc::new(Semaphore::new(1)).acquire_owned().await.unwrap();
        let mut connection = Box::pin(serve_connection(
            stream,
            permit,
            Arc::new(PanelOrigin::parse(&format!("http://{addr}")).unwrap()),
            Arc::new(PanelCache::default()),
            business,
            Arc::new(UiBundle::new(UiOptions::default(), false).unwrap()),
            accepted_at,
        ));
        let body = br#"{"schema_version":1,"owner_scope":"issuer","amount":1}"#;
        let head = format!("POST /api/admin/v1/balance/refill HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\nX-QT-Panel: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len());
        client.write_all(&head.as_bytes()[..10]).await.unwrap();
        consumed(&mut connection, &monitor).await;
        tokio::time::advance(Duration::from_secs(4)).await;
        let mut remaining = head.as_bytes()[10..].to_vec();
        remaining.extend_from_slice(&body[..body.len() - 1]);
        client.write_all(&remaining).await.unwrap();
        consumed(&mut connection, &monitor).await;
        tokio::time::advance(Duration::from_secs(9)).await;
        client.write_all(&body[body.len() - 1..]).await.unwrap();
        client.shutdown().await.unwrap();
        consumed(&mut connection, &monitor).await;
        assert_eq!(accepted_at.elapsed(), Duration::from_secs(13));
        drop(monitor);
        let connection = tokio::spawn(connection);
        let response = tokio::spawn(async move {
            let mut bytes = Vec::new();
            client.read_to_end(&mut bytes).await.unwrap();
            bytes
        });
        Self {
            accepted_at,
            response,
            connection,
            supervisor,
        }
    }
    pub async fn finish(self) -> (u16, serde_json::Value) {
        let bytes = self.response.await.unwrap();
        self.connection.await.unwrap();
        self.supervisor.await.unwrap();
        let end = bytes
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .expect("complete HTTP response")
            + 4;
        let status = std::str::from_utf8(&bytes[..end])
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, serde_json::from_slice(&bytes[end..]).unwrap())
    }
}
impl Running {
    pub async fn start(worker: Arc<ReconcileWorker>) -> Self {
        let server = crate::panel_http::PanelServer::bind("127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let addr = server.listener.local_addr().unwrap();
        assert!(![3000, 3001].contains(&addr.port()));
        let authority = server.origin.authority.clone();
        let (stop, shutdown) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(server.serve(worker, shutdown));
        Self {
            addr,
            authority,
            stop,
            task,
        }
    }
    pub fn wire(&self, method: &str, path: &str, body: &str) -> Vec<u8> {
        format!("{method} /api/admin/v1{path} HTTP/1.1\r\nHost: {}\r\nOrigin: http://{}\r\nX-QT-Panel: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", self.authority, self.authority, body.len()).into_bytes()
    }
    pub async fn raw(&self, bytes: &[u8]) -> (u16, String, Vec<u8>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut socket = tokio::net::TcpStream::connect(self.addr).await.unwrap();
        socket.write_all(bytes).await.unwrap();
        socket.shutdown().await.unwrap();
        let mut received = Vec::new();
        socket.read_to_end(&mut received).await.unwrap();
        let end = received.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let header = std::str::from_utf8(&received[..end]).unwrap().to_owned();
        assert!(header.contains("Cache-Control: no-store\r\n"));
        assert!(header.contains("X-Content-Type-Options: nosniff\r\n"));
        let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, header, received[end..].to_vec())
    }
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> (u16, serde_json::Value) {
        let bytes = self.wire(
            method,
            path,
            &body.map(ToString::to_string).unwrap_or_default(),
        );
        let (status, _, received) = self.raw(&bytes).await;
        let response = serde_json::from_slice(&received).unwrap();
        record_wire(method, path, body, &response);
        (status, response)
    }
    pub async fn finish(self) {
        self.stop.send(true).unwrap();
        self.task.await.unwrap().unwrap();
    }
}

pub fn emit(schema: &str, value: &serde_json::Value) {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(directory) = std::env::var_os("SGM6_A22_WIRE_DIR") {
        let directory = std::path::PathBuf::from(directory).canonicalize().unwrap();
        assert!(directory.starts_with("/tmp"));
        assert!(schema
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        let id = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::fs::write(
            directory.join(format!("{schema}-{}-{id}.json", std::process::id())),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }
}
fn record_wire(
    method: &str,
    path: &str,
    input: Option<&serde_json::Value>,
    response: &serde_json::Value,
) {
    if method == "GET" && path == "/state" {
        return;
    }
    if method == "GET" && (path == "/tokens" || path.starts_with("/tokens?")) {
        let fields = path
            .split_once('?')
            .map(|(_, q)| serde_json::json!({"after":q.strip_prefix("after=").unwrap()}))
            .unwrap_or_else(|| serde_json::json!({}));
        emit("request_IdentityQuery", &fields);
    }
    if method == "GET" && path.starts_with("/native-tokens?") {
        let query = path.split_once('?').unwrap().1;
        let mut fields = serde_json::Map::new();
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap();
            fields.insert(
                k.to_owned(),
                if k == "page" {
                    serde_json::json!(v.parse::<usize>().unwrap())
                } else {
                    serde_json::json!(v)
                },
            );
        }
        emit("request_NativeTokenQuery", &fields.into());
    }
    let path = path.split('?').next().unwrap();
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    let (request, output) = match (method, parts.as_slice()) {
        ("POST", ["sources"]) => ("CreateSource", "SourceCreated"),
        ("GET", ["sources", _, "models"]) => ("", "SourceModels"),
        ("PUT", ["sources", _, "models"]) => ("SetModels", "original_control_operation"),
        ("POST", ["sources", _, "models", "discover"]) => {
            ("OperationRevision", "original_control_operation")
        }
        ("PUT", ["sources", _, "paused"]) => ("SetSourcePaused", "original_control_operation"),
        (
            "POST",
            ["sources", _, "retire"] | ["memberships", _, "ensure"] | ["memberships", _, "retire"],
        ) => ("OperationRevision", "original_control_operation"),
        ("POST", ["sources", _, "clear-error"]) => {
            ("ClearVolcanoError", "original_control_operation")
        }
        ("PUT", ["sources", _, "volcano-policy"]) => {
            ("UpdateVolcanoPolicy", "original_control_operation")
        }
        ("POST", ["groups"]) => ("CreateGroup", "GroupCreated"),
        ("POST", ["groups", _, "check"]) => ("CheckGroup", "original_group_check"),
        ("GET", ["groups", _, "references"]) => ("", "original_group_references"),
        ("PUT", ["groups", _, "pin"]) => ("SetGroupPin", "original_control_operation"),
        ("POST", ["memberships"]) => ("PlanMembership", "MemberPlanned"),
        ("GET", ["operations", _]) => ("", "original_control_operation"),
        ("POST", ["operations", _, "retry"]) => ("VersionOnly", "original_control_operation"),
        ("GET", ["tokens"]) => ("", "IdentityPage"),
        ("POST", ["tokens"]) => ("CreateToken", "original_control_operation"),
        ("GET", ["tokens", _]) => ("", "original_token_identity"),
        ("PUT", ["tokens", _, "enabled"]) => ("SetTokenEnabled", "original_control_operation"),
        ("DELETE", ["tokens", _]) => ("OperationRevision", "original_control_operation"),
        ("POST", ["tokens", _, "reveal"]) => ("RevealToken", "TokenReveal"),
        ("GET", ["native-tokens"]) => ("", "NativePage"),
        ("POST", ["tokens", "register"]) => ("RegisterToken", "original_token_identity"),
        ("POST", ["usage", "query"]) => ("UsageQuery", "UsageSnapshot"),
        ("POST", ["balance", "refill"]) => ("RefillBalance", "BalanceRefilled"),
        ("PUT", ["balance", "policy"]) => ("SetBalancePolicy", "BalancePolicySaved"),
        _ => panic!("wire fixture route needs explicit schema"),
    };
    if matches!(output, "BalanceRefilled" | "BalancePolicySaved") && response.get("error").is_none()
    {
        emit_balance_pair(
            request,
            input.expect("balance request body"),
            output,
            response,
        );
        return;
    }
    if let Some(input) = input {
        emit(&format!("request_{request}"), input);
    }
    emit(
        &format!(
            "response_{}",
            if response.get("existing_source_id").is_some() {
                "AdminError"
            } else {
                output
            }
        ),
        response,
    );
}

// 同次请求/回复共用一个pair序号；上下文只存原请求文件名，不能由回执反造。
fn emit_balance_pair(
    request: &str,
    input: &serde_json::Value,
    response: &str,
    output: &serde_json::Value,
) {
    static PAIR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(directory) = std::env::var_os("SGM6_A22_WIRE_DIR") {
        let directory = std::path::PathBuf::from(directory).canonicalize().unwrap();
        assert!(directory.starts_with("/tmp"));
        let id = PAIR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // 单独序号空间加高位，避免与原emit同族文件相撞。
        let pair = id.checked_add(1_u64 << 63).unwrap();
        let pid = std::process::id();
        let request_file = format!("request_{request}-{pid}-{pair}.json");
        let response_file = format!("response_{response}-{pid}-{pair}.json");
        std::fs::write(
            directory.join(&request_file),
            serde_json::to_vec(input).unwrap(),
        )
        .unwrap();
        std::fs::write(
            directory.join(&response_file),
            serde_json::to_vec(output).unwrap(),
        )
        .unwrap();
        let context = directory.join("balance-context");
        std::fs::create_dir_all(&context).unwrap();
        std::fs::write(
            context.join(response_file),
            serde_json::to_vec(&serde_json::json!({"request_file":request_file})).unwrap(),
        )
        .unwrap();
    }
}
