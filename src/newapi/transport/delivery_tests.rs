use super::tests::{Fake, Reply};
use super::*;
use crate::newapi::managed_channels::{ChannelPatch, ManagedChannels};
use proptest::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn write_request() -> ManagementRequest {
    ManagementRequest::new(
        Method::POST,
        "/api/channel/1/status",
        Some(json!({"status":2})),
        RequestSemantics::IdempotentWrite,
        SINGLE_BODY_LIMIT,
    )
    .unwrap()
}
fn channel() -> Value {
    json!({"id":1,"name":"fake-channel","type":8,"group":"fake-group","base_url":"https://fake.invalid/v1","models":"m1","priority":10,"status":2,"auto_ban":0,"channel_info":{"is_multi_key":false}})
}

#[tokio::test]
async fn delivery_pre_admission_auth_and_connect_failures_are_not_sent() {
    let fake = Fake::start(|_| Reply::json(json!({"success":true}))).await;
    let transport = fake.transport(true);
    let error = transport
        .send_fenced(write_request(), RequestClass::Control, &|| false)
        .await
        .err()
        .unwrap();
    assert_eq!(
        (error.kind, error.delivery),
        (
            ManagementErrorKind::RevisionConflict,
            DeliveryEvidence::NotSent
        )
    );
    assert!(fake.calls.lock().unwrap().is_empty());
    let auth = Fake::start(|_| Reply {
        status: 401,
        ..Reply::json(json!({"success":false}))
    })
    .await;
    let error = auth
        .transport(true)
        .send(write_request(), RequestClass::Control)
        .await
        .err()
        .unwrap();
    assert_eq!(
        (error.kind, error.delivery),
        (ManagementErrorKind::AuthFailed, DeliveryEvidence::NotSent)
    );
    let mut cfg = fake.config(true);
    cfg.base_url = "http://127.0.0.1:0".into();
    let error = ManagementTransport::new(&cfg, ManagementBudget::new())
        .unwrap()
        .send(write_request(), RequestClass::Control)
        .await
        .err()
        .unwrap();
    assert_eq!(error.delivery, DeliveryEvidence::NotSent);
}

#[tokio::test]
async fn delivery_complete_application_rejection_is_not_unknown_or_success() {
    for status in [200, 400, 503] {
        let fake = Fake::start(move |_| Reply {
            status,
            ..Reply::json(json!({"success":false,"message":"fake-private-response"}))
        })
        .await;
        let error = fake
            .transport(true)
            .send(write_request(), RequestClass::Control)
            .await
            .err()
            .unwrap();
        assert_eq!(error.delivery, DeliveryEvidence::ResponseReceived);
        assert!(!format!("{error:?} {error}").contains("fake-private-response"));
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn delivery_complete_proxy_errors_malformed_and_oversize_are_uncertain() {
    for (status, body) in [
        (504, "proxy error".into()),
        (200, "not-json".into()),
        (200, "{}".into()),
        (200, "{\"success\":1}".into()),
        (200, "x".repeat(SINGLE_BODY_LIMIT + 1)),
    ] {
        let fake = Fake::start(move |_| Reply {
            status,
            body: body.clone(),
            headers: String::new(),
            delay: Duration::ZERO,
        })
        .await;
        let error = fake
            .transport(true)
            .send(write_request(), RequestClass::Control)
            .await
            .err()
            .unwrap();
        assert_eq!(error.delivery, DeliveryEvidence::Uncertain);
        assert_eq!(error.kind, ManagementErrorKind::TransportUnknown);
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn delivery_truncated_body_does_not_become_a_completed_response() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(![3000, 3001].contains(&port));
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            let n = socket.read(&mut chunk).await.unwrap();
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"success\":true}").await.unwrap();
    });
    let mut cfg: NewApiConfig = toml::from_str("").unwrap();
    cfg.base_url = format!("http://127.0.0.1:{port}");
    cfg.admin_token = "fake-only".into();
    let error = ManagementTransport::new(&cfg, ManagementBudget::new())
        .unwrap()
        .send(write_request(), RequestClass::Control)
        .await
        .err()
        .unwrap();
    assert_eq!(error.delivery, DeliveryEvidence::Uncertain);
    assert_eq!(error.status, Some(200));
    server.await.unwrap();
}

#[tokio::test]
async fn delivery_429_cools_down_before_body_finishes_and_keeps_retry_after() {
    let fake = Fake::start(|_| Reply {
        status: 429,
        body: "slow proxy body".into(),
        headers: "Retry-After: 90\r\n".into(),
        delay: Duration::from_secs(20),
    })
    .await;
    let budget = ManagementBudget::new();
    let transport = ManagementTransport::new(&fake.config(true), budget.clone()).unwrap();
    let task =
        tokio::spawn(async move { transport.send(write_request(), RequestClass::Control).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if budget.state.lock().unwrap().cooldown > Instant::now() + Duration::from_secs(60) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    let other = ManagementTransport::new(&fake.config(true), budget).unwrap();
    let queued = other
        .send(write_request(), RequestClass::Control)
        .await
        .err()
        .unwrap();
    assert_eq!(queued.delivery, DeliveryEvidence::NotSent);
    assert_eq!(queued.kind, ManagementErrorKind::RateLimited);
    let error = task.await.unwrap().err().unwrap();
    assert_eq!(error.delivery, DeliveryEvidence::Uncertain);
    assert_eq!(error.status, Some(429));
    assert_eq!(error.retry_after, Some(Duration::from_secs(90)));
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn delivery_field_get_failure_never_marks_put_as_dispatched() {
    for status in [200, 503] {
        let fake = Fake::start(move |_| Reply {
            status,
            body: "malformed-prerequisite".into(),
            headers: String::new(),
            delay: Duration::ZERO,
        })
        .await;
        let transport = fake.transport(true);
        let error = ManagedChannels::new(&transport)
            .write_channel_fields(
                1,
                ChannelPatch {
                    priority: Some(20),
                    ..Default::default()
                },
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.delivery, DeliveryEvidence::NotSent);
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "GET");
    }
}

#[tokio::test]
async fn delivery_field_put_outcome_and_noop_are_distinct() {
    for success in [true, false] {
        let fake = Fake::start(move |r| {
            if r.method == "GET" {
                Reply::json(json!({"success":true,"data":channel()}))
            } else {
                Reply::json(json!({"success":success}))
            }
        })
        .await;
        let transport = fake.transport(true);
        let api = ManagedChannels::new(&transport);
        assert!(!api
            .write_channel_fields(
                1,
                ChannelPatch {
                    priority: Some(10),
                    ..Default::default()
                }
            )
            .await
            .unwrap());
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
        let result = api
            .write_channel_fields(
                1,
                ChannelPatch {
                    priority: Some(20),
                    ..Default::default()
                },
            )
            .await;
        if success {
            assert!(result.unwrap());
        } else {
            assert_eq!(
                result.err().unwrap().delivery,
                DeliveryEvidence::ResponseReceived
            );
        }
        assert_eq!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == "PUT")
                .count(),
            1
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn delivery_pbt_envelope_requires_boolean_success(marker in 0usize..5,secret in "[a-z]{8,32}") {
        let value=match marker {0=>json!({"success":true,"message":secret}),1=>json!({"success":false,"message":secret}),2=>json!({"success":0,"message":secret}),3=>json!({"message":secret}),_=>json!([{"success":true,"message":secret}])};
        let response=ManagementResponse {status:StatusCode::OK,body:serde_json::to_vec(&value).unwrap()};
        let result=response.envelope();
        if marker==0 {prop_assert!(result.is_ok());}else {let error=result.err().unwrap();prop_assert_eq!(error.delivery,if marker==1 {DeliveryEvidence::ResponseReceived}else {DeliveryEvidence::Uncertain});let diagnostic=format!("{error:?} {error}");prop_assert!(!diagnostic.contains(&secret));}
    }
}
