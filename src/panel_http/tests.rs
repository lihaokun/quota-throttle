use super::*;
use crate::catalog::panel::PanelCatalog;
use crate::reconcile::worker::panel::{GlmPanelView, PanelSnapshot};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

impl PanelServer {
    // H1用自定义publisher做故障注入；业务层仍使用真实临时worker，测试结束才释放目录。
    async fn serve_with(
        self,
        reader: SnapshotReader,
        shutdown: watch::Receiver<bool>,
    ) -> Result<(), HttpErrorCode> {
        let fixture = admin::tests::support::fixture(false);
        let result = self
            .serve_with_worker(reader, fixture.worker.clone(), shutdown)
            .await;
        drop(fixture);
        result
    }
}

pub(crate) fn empty(at: i64) -> PanelSnapshot {
    PanelSnapshot {
        schema_version: 1,
        captured_at_ms: at,
        backend_ready: false,
        catalog: PanelCatalog {
            sources: vec![],
            groups: vec![],
            memberships: vec![],
        },
        glm: GlmPanelView {
            cycle_id: None,
            completed_at_ms: None,
            sources: vec![],
            groups: vec![],
        },
    }
}
fn populated(cache: &PanelCache) -> Arc<[u8]> {
    let at = wall_ms().unwrap();
    let body = publication::encode_panel(&empty(at)).unwrap();
    cache
        .replace(Some(publication::PublishedPanel {
            bytes: body.clone(),
            captured_at_ms: at,
            started_at: Instant::now(),
        }))
        .unwrap();
    body
}
fn response_parts(bytes: &[u8]) -> (&str, &[u8]) {
    let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    (
        std::str::from_utf8(&bytes[..split]).unwrap(),
        &bytes[split..],
    )
}
fn get(authority: &str) -> String {
    format!("GET {STATE_PATH} HTTP/1.1\r\nHost: {authority}\r\nX-QT-Panel: 1\r\n\r\n")
}
async fn exchange(addr: SocketAddr, bytes: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(bytes).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut result = Vec::new();
    stream.read_to_end(&mut result).await.unwrap();
    result
}
async fn one_request(bytes: Vec<u8>, cache: Arc<PanelCache>) -> Vec<u8> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    assert!(![3000, 3001].contains(&addr.port()));
    let client = tokio::spawn(async move { exchange(addr, &bytes).await });
    let (stream, _) = listener.accept().await.unwrap();
    let sem = Arc::new(Semaphore::new(1));
    let fixture = admin::tests::support::fixture(false);
    let (business, _supervisor) = admin::Supervisor::new(fixture.worker.clone());
    serve_connection(
        stream,
        sem.acquire_owned().await.unwrap(),
        Arc::new(PanelOrigin::parse("http://localhost:13001").unwrap()),
        cache,
        business,
        Arc::new(ui::UiBundle::new(UiOptions::default(), false).unwrap()),
        Instant::now(),
    )
    .await;
    client.await.unwrap()
}
// paused runtime保持runnable，让真实OS socket/阻塞任务先推进；只由case显式推进虚拟时钟。
async fn drive(future: impl std::future::Future<Output = ()> + Send + 'static) {
    let task = tokio::spawn(future);
    for _ in 0..500_000 {
        if task.is_finished() {
            task.await.unwrap();
            return;
        }
        tokio::task::yield_now().await;
    }
    task.abort();
    panic!("panel test exceeded bounded yields");
}
async fn until(mut check: impl FnMut() -> bool) {
    for _ in 0..100_000 {
        if check() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("panel test condition not reached");
}
#[derive(Default)]
struct Gate {
    opened: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn wait(&self) {
        let mut open = self.opened.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
    }
    fn open(&self) {
        *self.opened.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.open();
    }
}

#[tokio::test]
async fn h1_bind_rejects_nonloopback_and_canonicalizes_fixed_origin() {
    assert_eq!(MAX_CONNECTIONS, 16);
    assert_eq!(CONNECTION_TIMEOUT, Duration::from_secs(45));
    assert_eq!(WRITE_TIMEOUT, Duration::from_secs(5));
    assert_eq!(PUBLISH_INTERVAL, Duration::from_secs(5));
    for addr in ["0.0.0.0:0", "192.0.2.1:0", "[::]:0", "127.0.0.2:0"] {
        assert!(matches!(
            PanelServer::bind(addr.parse().unwrap(), None).await,
            Err(StartupError::InvalidAddress)
        ));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    assert!(matches!(
        PanelServer::bind(addr, Some("http://evil.example")).await,
        Err(StartupError::InvalidOrigin)
    ));
    assert!(matches!(
        PanelServer::bind(addr, None).await,
        Err(StartupError::BindFailed)
    ));
    let server = PanelServer::bind("127.0.0.1:0".parse().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(
        server.origin.origin,
        format!("http://{}", server.listener.local_addr().unwrap())
    );
    let alias = PanelServer::bind("127.0.0.1:0".parse().unwrap(), Some("http://LOCALHOST:80/"))
        .await
        .unwrap();
    assert_eq!(alias.origin.authority, "localhost");
}

#[tokio::test]
async fn h1_http_state_uses_exact_bytes_headers_and_one_request_only() {
    let cache = Arc::new(PanelCache::default());
    let expected = populated(&cache);
    let mut bytes = get("localhost:13001").into_bytes();
    bytes.extend_from_slice(
        b"POST /api/keys HTTP/1.1\r\nHost: evil.example\r\nContent-Length: 0\r\n\r\n",
    );
    let result = one_request(bytes, cache.clone()).await;
    let (header, body) = response_parts(&result);
    assert!(header.starts_with("HTTP/1.1 200 OK\r\n"));
    assert_eq!(body, expected.as_ref());
    assert_eq!(header.matches("HTTP/1.1").count(), 1);
    for fixed in [
        "Cache-Control: no-store",
        "Connection: close",
        "X-Content-Type-Options: nosniff",
        "Cross-Origin-Resource-Policy: same-origin",
        "Referrer-Policy: no-referrer",
        "X-Frame-Options: DENY",
    ] {
        assert!(header.contains(fixed));
    }
    assert!(!header.to_ascii_lowercase().contains("access-control-allow"));
    assert!(!header.contains("ETag:"));
    assert!(header.contains(&format!("Content-Length: {}\r\n", body.len())));
    let age: u64 = header
        .lines()
        .find_map(|l| l.strip_prefix("X-QT-Snapshot-Age-Ms: "))
        .unwrap()
        .parse()
        .unwrap();
    assert!(age < 10_000);
    assert!(Arc::ptr_eq(
        &cache.get(Instant::now(), wall_ms()).unwrap().0,
        &expected
    ));
}

#[tokio::test]
async fn h1_http_rejections_are_safe_and_head_never_has_body() {
    let cache = Arc::new(PanelCache::default());
    let expected = populated(&cache);
    let original = get("localhost:13001");
    for (bytes, status, code) in [
        (original.replace("localhost:13001", "evil.example"), "403", "forbidden"),
        (original.replace("X-QT-Panel: 1\r\n", ""), "403", "forbidden"),
        (original.replace("\r\n\r\n", "\r\nOrigin: null\r\n\r\n"), "403", "forbidden"),
        (original.replace("\r\n\r\n", "\r\nOrigin: http://localhost:13002\r\n\r\n"), "403", "forbidden"),
        (original.replace(STATE_PATH, "/api/status"), "404", "not_found"),
        (original.replace(STATE_PATH, &format!("{STATE_PATH}?")), "400", "bad_request"),
        (original.replace("GET ", "OPTIONS "), "405", "method_not_allowed"),
        (original.replace("GET ", "TRACE "), "405", "method_not_allowed"),
        (original.replace("GET ", "POST ").replace("\r\n\r\n", "\r\nContent-Length: 1\r\nOrigin: http://localhost:13001\r\nContent-Type: application/json\r\n\r\n"), "400", "bad_request"),
    ] {
        let result = one_request(bytes.into_bytes(), cache.clone()).await;
        let (header, body) = response_parts(&result);
        assert!(header.starts_with(&format!("HTTP/1.1 {status}")), "{header}");
        assert_eq!(serde_json::from_slice::<serde_json::Value>(body).unwrap(), serde_json::json!({"schema_version":1,"error":code}));
        assert!(Arc::ptr_eq(&cache.get(Instant::now(), wall_ms()).unwrap().0, &expected));
    }
    for bad_origin in [false, true] {
        let mut bytes = original.replace("GET ", "HEAD ");
        if bad_origin {
            bytes = bytes.replace("localhost:13001", "wrong.example");
        }
        let result = one_request(bytes.into_bytes(), cache.clone()).await;
        let (header, body) = response_parts(&result);
        assert!(body.is_empty());
        assert!(header.starts_with(if bad_origin {
            "HTTP/1.1 403"
        } else {
            "HTTP/1.1 405"
        }));
        if !bad_origin {
            assert!(header.contains("Allow: GET\r\n"));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn h1_response_stalled_writer_has_one_absolute_deadline() {
    for budget in [1, 100] {
        let (mut out, mut incoming) = tokio::io::duplex(16);
        let start = Instant::now();
        respond(
            &mut out,
            Response::error(HttpErrorCode::Internal),
            false,
            start + Duration::from_secs(budget),
        )
        .await;
        assert_eq!(Instant::now() - start, Duration::from_secs(budget.min(5)));
        drop(out);
        let mut bytes = Vec::new();
        incoming.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes, b"HTTP/1.1 500 Int");
    }
}

#[tokio::test(start_paused = true)]
async fn h1_ready_writer_must_not_start_after_connection_deadline() {
    let (mut out, mut input) = tokio::io::duplex(4096);
    respond(
        &mut out,
        Response::error(HttpErrorCode::Internal),
        false,
        Instant::now(),
    )
    .await;
    drop(out);
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes).await.unwrap();
    assert!(
        bytes.is_empty(),
        "expired connection must not start a response"
    );
}

struct DeadlineWriter {
    ready: std::pin::Pin<Box<tokio::time::Sleep>>,
    delay_write: bool,
    bytes: Vec<u8>,
}
impl AsyncWrite for DeadlineWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::future::Future;
        if self.delay_write {
            std::task::ready!(self.ready.as_mut().poll(cx));
        }
        self.bytes.extend_from_slice(bytes);
        std::task::Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::future::Future;
        if !self.delay_write {
            std::task::ready!(self.ready.as_mut().poll(cx));
        }
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}
#[tokio::test(start_paused = true)]
async fn h1_ready_write_and_flush_recheck_deadline_before_reporting_completion() {
    for delay_write in [true, false] {
        let end = Instant::now() + Duration::from_secs(1);
        let mut writer = DeadlineWriter {
            ready: Box::pin(tokio::time::sleep_until(end)),
            delay_write,
            bytes: vec![],
        };
        let completed = respond(
            &mut writer,
            Response::error(HttpErrorCode::Internal),
            false,
            end,
        )
        .await;
        assert!(!completed);
        let (header, body) = response_parts(&writer.bytes);
        assert_eq!(header.matches("HTTP/1.1").count(), 1);
        if delay_write {
            assert!(body.is_empty());
        } else {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(body).unwrap()["error"],
                "internal"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn h1_unified_publication_is_not_request_driven_and_failure_clears_cache() {
    drive(async {
        let server = PanelServer::bind("127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let addr = server.listener.local_addr().unwrap();
        let authority = server.origin.authority.clone();
        let cache = server.cache.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let reader: SnapshotReader = Arc::new(move || {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(empty(wall_ms().unwrap()))
            } else {
                Err(publication::PublishError)
            }
        });
        let (stop, shutdown) = watch::channel(false);
        let task = tokio::spawn(server.serve_with(reader, shutdown));
        until(|| cache.get(Instant::now(), wall_ms()).is_ok()).await;
        let expected = cache.get(Instant::now(), wall_ms()).unwrap().0;
        for _ in 0..20 {
            let result = exchange(addr, get(&authority).as_bytes()).await;
            assert_eq!(response_parts(&result).1, expected.as_ref());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(PUBLISH_INTERVAL).await;
        until(|| {
            calls.load(Ordering::SeqCst) == 2 && cache.get(Instant::now(), wall_ms()).is_err()
        })
        .await;
        let result = exchange(addr, get(&authority).as_bytes()).await;
        assert!(response_parts(&result).0.starts_with("HTTP/1.1 503"));
        stop.send(true).unwrap();
        task.await.unwrap().unwrap();
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn h1_expired_blocking_publication_keeps_ownership_and_shutdown_drains() {
    drive(async {
        let server = PanelServer::bind("127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let cache = server.cache.clone();
        populated(&cache);
        let gate = Arc::new(Gate::default());
        let release = Release(gate.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let reader: SnapshotReader = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            gate.wait();
            done.store(true, Ordering::SeqCst);
            Ok(empty(wall_ms().unwrap()))
        });
        let (stop, shutdown) = watch::channel(false);
        let task = tokio::spawn(server.serve_with(reader, shutdown));
        until(|| calls.load(Ordering::SeqCst) == 1).await;
        tokio::time::advance(PUBLICATION_TTL).await;
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            cache.get(Instant::now(), wall_ms()).err(),
            Some(HttpErrorCode::SnapshotUnavailable)
        );
        tokio::time::advance(PUBLISH_INTERVAL * 3).await;
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        stop.send(true).unwrap();
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert!(!task.is_finished());
        assert!(!finished.load(Ordering::SeqCst));
        drop(release);
        task.await.unwrap().unwrap();
        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(cache.get(Instant::now(), wall_ms()).is_err());
    })
    .await;
}

#[tokio::test]
async fn h1_blocking_panic_and_preclosed_shutdown_are_fail_closed() {
    for initial_stop in [false, true] {
        let server = PanelServer::bind("127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let addr = server.listener.local_addr().unwrap();
        let cache = server.cache.clone();
        populated(&cache);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let reader: SnapshotReader = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            panic!("test-only publication panic")
        });
        let (_tx, rx) = watch::channel(initial_stop);
        let result = server.serve_with(reader, rx).await;
        assert_eq!(
            result,
            if initial_stop {
                Ok(())
            } else {
                Err(HttpErrorCode::Internal)
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(!initial_stop));
        assert!(cache.get(Instant::now(), wall_ms()).is_err());
        let rebound = TcpListener::bind(addr).await.unwrap();
        drop(rebound);
    }
}

#[tokio::test(start_paused = true)]
async fn h1_connection_limit_closes_excess_and_recovers_without_queue() {
    drive(async {
        let server = PanelServer::bind("127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let addr = server.listener.local_addr().unwrap();
        let authority = server.origin.authority.clone();
        let slots = server.connections.clone();
        let cache = server.cache.clone();
        let (stop, shutdown) = watch::channel(false);
        let task =
            tokio::spawn(server.serve_with(Arc::new(|| Ok(empty(wall_ms().unwrap()))), shutdown));
        until(|| cache.get(Instant::now(), wall_ms()).is_ok()).await;
        let mut idle = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            idle.push(TcpStream::connect(addr).await.unwrap());
        }
        until(|| slots.available_permits() == 0).await;
        let mut extra = TcpStream::connect(addr).await.unwrap();
        let mut bytes = Vec::new();
        let _ = extra
            .write_all(b"HEAD /api/admin/v1/state HTTP/1.1\r\n")
            .await;
        let _ = extra.read_to_end(&mut bytes).await;
        assert!(bytes.is_empty());
        assert_eq!(slots.available_permits(), 0);
        drop(idle);
        until(|| slots.available_permits() == MAX_CONNECTIONS).await;
        let result = exchange(addr, get(&authority).as_bytes()).await;
        assert!(response_parts(&result).0.starts_with("HTTP/1.1 200"));
        stop.send(true).unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(slots.available_permits(), MAX_CONNECTIONS);
    })
    .await;
}

#[tokio::test]
async fn h1_real_worker_publication_keeps_private_source_material_outside_http() {
    use crate::catalog::*;
    use crate::config::{Config, HeaderKV};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    store
        .create_source(SourceDraft {
            name: "visible source".into(),
            note: "visible note".into(),
            credential: SecretString::new("private-h1-upstream-key".into()).unwrap(),
            settings: SourceSettings::Glm {
                upstream: UpstreamProfile {
                    channel_type: 8,
                    base_url: "https://private-h1.example/coding".into(),
                    models: vec!["glm-test".into()],
                    model_discovery: None,
                },
                quota: GlmQuotaProfile {
                    quota_url: "https://private-h1.example/quota".into(),
                    global_headers: vec![],
                    key_headers: vec![HeaderKV {
                        key: "X-Selector".into(),
                        value: "private-h1-selector".into(),
                    }],
                },
            },
        })
        .unwrap();
    let cfg:Config=toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'\nadmin_token='private-h1-admin'").unwrap();
    let api = Arc::new(crate::newapi::NewApiClient::new(&cfg.new_api).unwrap());
    let worker = ReconcileWorker::new(store, api, Arc::new(cfg)).unwrap();
    let before = worker.snapshot().unwrap();
    let server = PanelServer::bind("127.0.0.1:0".parse().unwrap(), None)
        .await
        .unwrap();
    let addr = server.listener.local_addr().unwrap();
    let authority = server.origin.authority.clone();
    let cache = server.cache.clone();
    let (stop, shutdown) = watch::channel(false);
    let task = tokio::spawn(server.serve(worker.clone(), shutdown));
    until(|| cache.get(Instant::now(), wall_ms()).is_ok()).await;
    let result = exchange(addr, get(&authority).as_bytes()).await;
    let (_, body) = response_parts(&result);
    let value: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(
        value["catalog"]["sources"][0]["source"]["name"],
        "visible source"
    );
    let text = std::str::from_utf8(body).unwrap();
    for private in [
        "private-h1-",
        "credential",
        "secret_ref",
        "settings",
        "fingerprint",
    ] {
        assert!(!text.contains(private));
    }
    assert_eq!(worker.snapshot().unwrap(), before);
    println!("HTTP_PANEL_JSON={text}");
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
}
