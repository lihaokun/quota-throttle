use super::*;
use proptest::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub(crate) struct Recorded {
    pub method: String,
    pub path: String,
    pub raw: String,
    pub body: Value,
    pub at: Instant,
}
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
    pub headers: String,
    pub delay: Duration,
}
impl Reply {
    pub fn json(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            headers: String::new(),
            delay: Duration::ZERO,
        }
    }
    fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::json(json!({"message":"fake-secret-error"}))
        }
    }
}
pub(crate) struct Fake {
    pub url: String,
    pub calls: Arc<Mutex<Vec<Recorded>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fake {
    pub async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(![3000, 3001].contains(&port));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let log = calls.clone();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            let mut children = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let log = log.clone(); let handler = handler.clone();
                        children.spawn(async move {
                            let mut buffer = Vec::new(); let mut chunk = [0; 4096];
                            let header_end = loop {
                                let n = socket.read(&mut chunk).await.unwrap();
                                if n == 0 { return; }
                                buffer.extend_from_slice(&chunk[..n]);
                                if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") { break pos + 4; }
                            };
                            let raw = String::from_utf8(buffer[..header_end].to_vec()).unwrap();
                            let size: usize = raw.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                            while buffer.len() < header_end + size {
                                let n = socket.read(&mut chunk).await.unwrap(); if n == 0 { return; }
                                buffer.extend_from_slice(&chunk[..n]);
                            }
                            let mut first = raw.lines().next().unwrap().split_whitespace();
                            let record = Recorded { method: first.next().unwrap().into(), path: first.next().unwrap().into(),
                                body: serde_json::from_slice(&buffer[header_end..]).unwrap_or(Value::Null), raw, at: Instant::now() };
                            let reply = handler(&record);
                            log.lock().unwrap().push(record);
                            let header = format!("HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n", reply.status, reply.body.len(), reply.headers);
                            if socket.write_all(header.as_bytes()).await.is_err() { return; }
                            tokio::time::sleep(reply.delay).await;
                            let _ = socket.write_all(reply.body.as_bytes()).await;
                        });
                    },
                    Some(_) = children.join_next() => {},
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            calls,
            task,
        }
    }
    pub fn config(&self, token: bool) -> NewApiConfig {
        let mut config: NewApiConfig = toml::from_str("").unwrap();
        config.base_url = self.url.clone();
        config.admin_token = if token {
            "fake-admin-key".into()
        } else {
            String::new()
        };
        config.root_password = "fake-root-password".into();
        config
    }
    pub fn transport(&self, token: bool) -> ManagementTransport {
        ManagementTransport::new(&self.config(token), ManagementBudget::new()).unwrap()
    }
}
fn read() -> ManagementRequest {
    ManagementRequest::new(
        Method::GET,
        "/api/channel/1",
        None,
        RequestSemantics::Read,
        SINGLE_BODY_LIMIT,
    )
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn budget_burst_rate_and_background_reservation() {
    let budget = ManagementBudget::new();
    let background = budget.admit(RequestClass::Observe).await.unwrap();
    let mut second_background = Box::pin(budget.admit(RequestClass::Backfill));
    assert!(tokio::time::timeout(Duration::ZERO, &mut second_background)
        .await
        .is_err());
    let control = budget.admit(RequestClass::Control).await.unwrap();
    assert_eq!(budget.state.lock().unwrap().active, 2);
    drop(control);
    drop(background);
    let start = Instant::now();
    drop(second_background.await.unwrap());
    assert_eq!(Instant::now() - start, Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn queue_capacity_cancellation_and_control_priority() {
    let budget = ManagementBudget::new();
    let a = budget.admit(RequestClass::Control).await.unwrap();
    let b = budget.admit(RequestClass::Control).await.unwrap();
    let mut pending = Vec::new();
    for _ in 0..64 {
        let mut future = Box::pin(budget.admit(RequestClass::Backfill));
        assert!(tokio::time::timeout(Duration::ZERO, &mut future)
            .await
            .is_err());
        pending.push(future);
    }
    assert_eq!(
        budget
            .admit(RequestClass::Control)
            .await
            .err()
            .unwrap()
            .kind,
        ManagementErrorKind::QueueFull
    );
    pending.pop();
    let mut control = Box::pin(budget.admit(RequestClass::Control));
    assert!(tokio::time::timeout(Duration::ZERO, &mut control)
        .await
        .is_err());
    drop(a);
    drop(b);
    drop(control.await.unwrap());
    assert_eq!(budget.state.lock().unwrap().queue.len(), 63);
    drop(pending);
    assert!(budget.state.lock().unwrap().queue.is_empty());
}

#[tokio::test(start_paused = true)]
async fn cooldown_is_shared_and_long_delays_are_not_truncated() {
    let budget = ManagementBudget::new();
    budget.cool_down(Duration::from_secs(90));
    let err = budget.admit(RequestClass::Control).await.err().unwrap();
    assert_eq!(err.retry_after, Some(Duration::from_secs(90)));
    assert!(budget.state.lock().unwrap().queue.is_empty());
    tokio::time::advance(Duration::from_secs(40)).await;
    let start = Instant::now();
    drop(budget.admit(RequestClass::Observe).await.unwrap());
    assert_eq!(Instant::now() - start, Duration::from_secs(50));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn r8_budget_all_time_windows(events in prop::collection::vec((0u64..3500, 0u8..3), 2..65)) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
        runtime.block_on(async {
            let budget = ManagementBudget::new();
            let start = Instant::now(); let mut trace = Vec::new();
            for (gap, class) in events {
                tokio::time::advance(Duration::from_millis(gap)).await;
                let class = [RequestClass::Control, RequestClass::Observe, RequestClass::Backfill][class as usize];
                let permit = budget.admit(class).await.unwrap();
                trace.push((Instant::now() - start).as_nanos());
                drop(permit);
            }
            // 独立轨迹 oracle：所有子区间，不借用被测桶的 credit/last。
            for i in 0..trace.len() { for j in i..trace.len() {
                assert!((j - i + 1) as u128 <= 2 + (trace[j] - trace[i]) / 1_000_000_000);
            }}
        });
    }

    #[test]
    fn r10_errors_never_include_private_payload(secret in "[a-zA-Z0-9]{25,70}") {
        let response = ManagementResponse { status: StatusCode::OK, body: json!({"success":false,"message":secret}).to_string().into_bytes() };
        let err = response.envelope().err().unwrap();
        let diagnostic = format!("{err:?} {err}");
        prop_assert!(!diagnostic.contains(&secret));
        let request = ManagementRequest::new(Method::POST, "/api/channel/", Some(json!({"key":secret})), RequestSemantics::Create, SINGLE_BODY_LIMIT).unwrap();
        // 内部 wrapper 没有 Debug/Serialize；检查真正的解析错误，不只测外层容器。
        prop_assert!(request.body.is_some());
        let malformed = ManagementResponse { status: StatusCode::OK, body: secret.as_bytes().to_vec() };
        let diagnostic = format!("{:?}", malformed.envelope().err().unwrap());
        prop_assert!(!diagnostic.contains(&secret));
    }
}

#[test]
fn retry_after_supports_delta_http_date_invalid_and_overflow() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    for (value, expected) in [
        (None, 4),
        (Some("90"), 90),
        (Some("-1"), 4),
        (Some("abc"), 4),
        (Some("0"), 0),
    ] {
        let header = value.map(|v| header::HeaderValue::from_str(v).unwrap());
        assert_eq!(
            retry_after(header.as_ref(), now),
            Duration::from_secs(expected)
        );
    }
    let date =
        header::HeaderValue::from_str(&httpdate::fmt_http_date(now + Duration::from_secs(120)))
            .unwrap();
    assert_eq!(retry_after(Some(&date), now), Duration::from_secs(120));
    let huge = header::HeaderValue::from_static("9999999999999999999999999999");
    assert_eq!(retry_after(Some(&huge), now), Duration::from_secs(u64::MAX));
}

#[test]
fn invalid_paths_and_bodies_are_rejected_before_queueing() {
    for path in [
        "https://evil/api/channel/",
        "//evil/api/",
        "/api/../setup",
        "/api/%2e%2e/",
        "/api/a#secret",
        "/api/a\\b",
    ] {
        assert_eq!(
            ManagementRequest::new(
                Method::GET,
                path,
                None,
                RequestSemantics::Read,
                SINGLE_BODY_LIMIT
            )
            .err()
            .unwrap()
            .kind,
            ManagementErrorKind::InvalidInput
        );
    }
    assert!(ManagementRequest::new(
        Method::GET,
        "/api/channel/",
        Some(json!({})),
        RequestSemantics::Read,
        SINGLE_BODY_LIMIT
    )
    .is_err());
    assert!(ManagementRequest::new(
        Method::POST,
        "/api/channel/",
        Some(json!("x".repeat(SINGLE_BODY_LIMIT))),
        RequestSemantics::Create,
        SINGLE_BODY_LIMIT
    )
    .is_err());
}

#[tokio::test]
async fn token_401_never_logs_in_and_opens_circuit() {
    let fake = Fake::start(|_| Reply::status(401)).await;
    let client = fake.transport(true);
    for _ in 0..4 {
        assert_eq!(
            client
                .send(read(), RequestClass::Control)
                .await
                .err()
                .unwrap()
                .kind,
            ManagementErrorKind::AuthFailed
        );
    }
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    client.retry_authentication().await.unwrap();
    assert!(client.send(read(), RequestClass::Control).await.is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
    assert!(fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.path == "/api/channel/1"));
}

#[tokio::test]
async fn concurrent_401_singleflight_releases_slots_and_counts_login_budget() {
    let login_count = Arc::new(AtomicUsize::new(0));
    let count = login_count.clone();
    let fake = Fake::start(move |call| {
        if call.path == "/api/user/login" {
            count.fetch_add(1, Ordering::SeqCst);
            let mut reply = Reply::json(json!({"success":true,"data":{"id":42}}));
            reply.headers = "Set-Cookie: session=fresh; Path=/\r\n".into();
            reply
        } else if call.raw.contains("session=fresh") {
            assert!(call.raw.to_ascii_lowercase().contains("new-api-user: 42"));
            Reply::json(json!({"success":true,"data":{}}))
        } else {
            Reply::status(401)
        }
    })
    .await;
    let client = fake.transport(false);
    client.auth.lock().unwrap().auth = Auth::Session(1);
    let run = async {
        tokio::join!(
            client.send(read(), RequestClass::Control),
            client.send(read(), RequestClass::Control)
        )
    };
    let (a, b) = tokio::time::timeout(Duration::from_secs(8), run)
        .await
        .unwrap();
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(login_count.load(Ordering::SeqCst), 1);
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 5);
    assert!(!calls.iter().any(|c| c.path.contains("setup")));
    assert!(calls[4].at.duration_since(calls[0].at) >= Duration::from_millis(2900));
}

#[tokio::test]
async fn ordinary_response_does_not_hold_auth_lock() {
    let fake = Fake::start(|_| Reply {
        delay: Duration::from_secs(2),
        ..Reply::json(json!({"success":true}))
    })
    .await;
    let client = fake.transport(true);
    let request = client.send(read(), RequestClass::Observe);
    tokio::pin!(request);
    tokio::select! { _ = &mut request => panic!("response should remain pending"), _ = tokio::time::sleep(Duration::from_millis(100)) => {} }
    assert!(client.auth.try_lock().is_ok());
    assert_eq!(client.budget.state.lock().unwrap().active, 1);
    // Drop 取消读请求时释放在途槽；远端写的 unknown 由协调器持久意图处理。
}

#[tokio::test]
async fn failed_login_latches_until_explicit_retry_and_does_not_setup() {
    let fake =
        Fake::start(|_| Reply::json(json!({"success":false,"message":"fake-password"}))).await;
    let client = fake.transport(false);
    for _ in 0..5 {
        assert_eq!(
            client
                .send(read(), RequestClass::Control)
                .await
                .err()
                .unwrap()
                .kind,
            ManagementErrorKind::AuthFailed
        );
    }
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    assert!(client.retry_authentication().await.is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
    assert!(fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.path == "/api/user/login"));
}

#[tokio::test]
async fn authentication_waiters_are_bounded_and_cancelled_login_opens_circuit() {
    let fake = Fake::start(|_| Reply {
        delay: Duration::from_secs(20),
        ..Reply::json(json!({"success":true,"data":{"id":42}}))
    })
    .await;
    let client = fake.transport(false);
    let mut pending = Vec::new();
    for _ in 0..64 {
        let mut request = Box::pin(client.send(read(), RequestClass::Control));
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut request)
            .await
            .is_err());
        pending.push(request);
    }
    assert_eq!(
        client
            .send(read(), RequestClass::Control)
            .await
            .err()
            .unwrap()
            .kind,
        ManagementErrorKind::QueueFull
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    drop(pending.remove(0)); // 已发登录被取消，后续等待者不能自动再登录。
    assert!(pending.remove(0).await.is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    drop(pending);
    assert_eq!(client.budget.requests.available_permits(), 64);
    assert_eq!(client.budget.state.lock().unwrap().active, 0);
}

#[tokio::test]
async fn control_request_proceeds_while_background_body_is_slow() {
    let fake = Fake::start(|call| Reply {
        delay: if call.path.ends_with("/1") {
            Duration::from_secs(5)
        } else {
            Duration::ZERO
        },
        ..Reply::json(json!({"success":true}))
    })
    .await;
    let client = fake.transport(true);
    let mut slow = Box::pin(client.send(read(), RequestClass::Observe));
    assert!(tokio::time::timeout(Duration::from_millis(100), &mut slow)
        .await
        .is_err());
    let request = ManagementRequest::new(
        Method::GET,
        "/api/channel/2",
        None,
        RequestSemantics::Read,
        SINGLE_BODY_LIMIT,
    )
    .unwrap();
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        client.send(request, RequestClass::Control)
    )
    .await
    .unwrap()
    .is_ok());
    drop(slow);
    assert_eq!(client.budget.state.lock().unwrap().active, 0);
}

#[tokio::test]
async fn session_user_id_replaces_static_header_but_token_keeps_it() {
    let fake = Fake::start(|_| Reply::json(json!({"success":true}))).await;
    let budget = ManagementBudget::new();
    for token in [false, true] {
        let mut config = fake.config(token);
        config.extra_headers.push(crate::config::HeaderKV {
            key: "New-Api-User".into(),
            value: "9".into(),
        });
        let client = ManagementTransport::new(&config, budget.clone()).unwrap();
        if !token {
            client.auth.lock().unwrap().auth = Auth::Session(42);
        }
        client.send(read(), RequestClass::Control).await.unwrap();
    }
    let calls = fake.calls.lock().unwrap();
    for (call, expected) in calls.iter().zip(["new-api-user: 42", "new-api-user: 9"]) {
        let headers: Vec<_> = call
            .raw
            .lines()
            .map(str::to_ascii_lowercase)
            .filter(|s| s.starts_with("new-api-user:"))
            .collect();
        assert_eq!(headers, vec![expected.to_string()]);
    }
}

#[tokio::test]
async fn second_401_is_terminal_even_after_successful_login() {
    let fake = Fake::start(|call| {
        if call.path == "/api/user/login" {
            Reply::json(json!({"success":true,"data":{"id":42}}))
        } else {
            Reply::status(401)
        }
    })
    .await;
    let client = fake.transport(false);
    client.auth.lock().unwrap().auth = Auth::Session(1);
    assert_eq!(
        client
            .send(read(), RequestClass::Control)
            .await
            .err()
            .unwrap()
            .kind,
        ManagementErrorKind::AuthFailed
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
    assert!(client.send(read(), RequestClass::Control).await.is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn non_json_429_applies_cooldown_to_all_identities() {
    let fake = Fake::start(|_| Reply {
        status: 429,
        body: "rate limit fake-secret".into(),
        headers: "Retry-After: 90\r\n".into(),
        delay: Duration::ZERO,
    })
    .await;
    let budget = ManagementBudget::new();
    let a = ManagementTransport::new(&fake.config(true), budget.clone()).unwrap();
    let b = ManagementTransport::new(&fake.config(false), budget).unwrap();
    let err = a.send(read(), RequestClass::Control).await.err().unwrap();
    assert_eq!(err.kind, ManagementErrorKind::RateLimited);
    assert_eq!(err.retry_after, Some(Duration::from_secs(90)));
    assert!(!format!("{err:?}").contains("fake-secret"));
    assert!(b.send(read(), RequestClass::Backfill).await.is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn redirects_are_not_followed_even_on_same_origin() {
    let fake = Fake::start(|_| Reply {
        headers: "Location: /api/leak\r\n".into(),
        ..Reply::status(307)
    })
    .await;
    let client = fake.transport(true);
    assert_eq!(
        client
            .send(read(), RequestClass::Control)
            .await
            .err()
            .unwrap()
            .kind,
        ManagementErrorKind::RemoteRejected
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn malformed_oversize_and_false_success_are_not_accepted() {
    for reply_body in [
        "not-json".into(),
        "{}".into(),
        "{\"success\":false}".into(),
        "x".repeat(SINGLE_BODY_LIMIT + 1),
    ] {
        let fake = Fake::start(move |_| Reply {
            status: 200,
            body: reply_body.clone(),
            headers: String::new(),
            delay: Duration::ZERO,
        })
        .await;
        assert!(fake
            .transport(true)
            .send(read(), RequestClass::Control)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn create_5xx_is_unknown_and_is_never_automatically_repeated() {
    let fake = Fake::start(|_| Reply::status(503)).await;
    let request = ManagementRequest::new(
        Method::POST,
        "/api/channel/",
        Some(json!({"key":"fake-source-secret"})),
        RequestSemantics::Create,
        SINGLE_BODY_LIMIT,
    )
    .unwrap();
    let err = fake
        .transport(true)
        .send(request, RequestClass::Control)
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ManagementErrorKind::TransportUnknown);
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn malformed_or_oversize_write_receipt_is_unknown_not_a_definite_rejection() {
    for body in [
        "not-json".into(),
        "{}".into(),
        "x".repeat(SINGLE_BODY_LIMIT + 1),
    ] {
        let fake = Fake::start(move |_| Reply {
            body: body.clone(),
            ..Reply::json(json!({}))
        })
        .await;
        let client = fake.transport(true);
        for (method, semantics) in [
            (Method::POST, RequestSemantics::Create),
            (Method::PUT, RequestSemantics::IdempotentWrite),
        ] {
            let request = ManagementRequest::new(
                method,
                "/api/channel/",
                Some(json!({"fake":true})),
                semantics,
                SINGLE_BODY_LIMIT,
            )
            .unwrap();
            assert_eq!(
                client
                    .send(request, RequestClass::Control)
                    .await
                    .err()
                    .unwrap()
                    .kind,
                ManagementErrorKind::TransportUnknown
            );
        }
        assert_eq!(fake.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn total_timeout_includes_reading_body_and_frees_slot() {
    let fake = Fake::start(|_| Reply {
        delay: Duration::from_secs(20),
        ..Reply::json(json!({"success":true}))
    })
    .await;
    let client = fake.transport(true);
    let start = Instant::now();
    let err = client
        .send(read(), RequestClass::Control)
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ManagementErrorKind::TransportUnknown);
    assert!(start.elapsed() >= Duration::from_secs(9) && start.elapsed() < Duration::from_secs(12));
    assert_eq!(client.budget.state.lock().unwrap().active, 0);
}

#[tokio::test]
async fn explicit_bootstrap_sets_up_once_and_refresh_does_not() {
    let fake = Fake::start(|call| match call.path.as_str() {
        "/api/setup" if call.method == "GET" => {
            Reply::json(json!({"success":true,"data":{"status":false,"root_init":false}}))
        }
        "/api/setup" => Reply::json(json!({"success":true})),
        "/api/user/login" => Reply::json(json!({"success":true,"data":{"id":42}})),
        _ => panic!("unexpected path"),
    })
    .await;
    let client = fake.transport(false);
    client.bootstrap().await.unwrap();
    client.bootstrap().await.unwrap();
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
    client.retry_authentication().await.unwrap();
    assert_eq!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.path == "/api/setup")
            .count(),
        2
    );
}

#[tokio::test]
async fn bootstrap_setup_malformed_write_receipt_is_unknown_without_login_or_retry() {
    let fake = Fake::start(|call| {
        if call.method == "GET" {
            Reply::json(json!({"success":true,"data":{"status":false,"root_init":false}}))
        } else {
            Reply {
                body: "malformed".into(),
                ..Reply::json(json!({}))
            }
        }
    })
    .await;
    let client = fake.transport(false);
    assert_eq!(
        client.bootstrap().await.err().unwrap().kind,
        ManagementErrorKind::TransportUnknown
    );
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(calls.iter().all(|c| c.path == "/api/setup"));
}
