use super::tests::{Fake, Reply};
use super::*;
use crate::catalog::BALANCE_CAP;
use crate::newapi::tokens::{TokenOwner, TokenOwnerScope};
use crate::newapi::NewApiClient;
use proptest::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn request() -> ManagementRequest {
    ManagementRequest::new(
        Method::GET,
        "/api/user/self",
        None,
        RequestSemantics::Read,
        SINGLE_BODY_LIMIT,
    )
    .unwrap()
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(20)
}
fn profile(id: i64, role: i64, quota: i64) -> Reply {
    Reply::json(
        json!({"success":true,"data":{"id":id,"role":role,"status":1,"group":"default","quota":quota}}),
    )
}
fn assert_not_sent(err: ManagementError, kind: ManagementErrorKind) {
    assert_eq!((err.kind, err.delivery), (kind, DeliveryEvidence::NotSent));
}
async fn entered(predicate: impl Fn() -> bool) {
    // 保持 runnable，不让 paused runtime 在本地 socket 尚未返回时自动跳到截止。
    for _ in 0..100_000 {
        if predicate() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("expected fixture entry or queue registration");
}
async fn finish<T>(task: tokio::task::JoinHandle<T>) -> T {
    for _ in 0..4_000 {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        if task.is_finished() {
            return task.await.unwrap();
        }
        tokio::time::advance(Duration::from_millis(10)).await;
    }
    task.abort();
    panic!("bounded balance call did not finish");
}

async fn finish_at<T>(task: tokio::task::JoinHandle<T>, end: Instant) -> T {
    // 只让已到期任务得到调度；绝不推进时钟去迁就被延长或漏传的截止。
    assert_eq!(Instant::now(), end);
    for _ in 0..100_000 {
        tokio::task::yield_now().await;
        if task.is_finished() {
            assert_eq!(Instant::now(), end);
            return task.await.unwrap();
        }
    }
    task.abort();
    panic!("balance call did not finish at its original deadline");
}

#[tokio::test]
async fn add_user_quota_request_shape_and_delivery() {
    for (status, body, expected) in [
        (200, "{\"success\":true}", None),
        (
            200,
            "{\"success\":false}",
            Some((
                ManagementErrorKind::RemoteRejected,
                DeliveryEvidence::ResponseReceived,
                None,
            )),
        ),
        (
            500,
            "{\"success\":false}",
            Some((
                ManagementErrorKind::TransportUnknown,
                DeliveryEvidence::ResponseReceived,
                Some(500),
            )),
        ),
        (
            429,
            "not json",
            Some((
                ManagementErrorKind::RateLimited,
                DeliveryEvidence::Uncertain,
                Some(429),
            )),
        ),
    ] {
        let entries = Arc::new(AtomicUsize::new(0));
        let hook = entries.clone();
        let fake = Fake::start(move |call| {
            hook.fetch_add(1, Ordering::SeqCst);
            assert_eq!(call.method, "POST");
            assert_eq!(call.path, "/api/user/manage");
            assert_eq!(
                call.body,
                json!({"id":41,"action":"add_quota","mode":"add","value":17})
            );
            Reply {
                status,
                body: body.into(),
                ..Reply::json(Value::Null)
            }
        })
        .await;
        let client = NewApiClient::new(&fake.config(true)).unwrap();
        let result = client
            .add_user_quota_until(41, 17, &|| true, deadline())
            .await;
        match expected {
            None => result.unwrap(),
            Some(expected) => {
                let err = result.unwrap_err();
                assert_eq!((err.kind, err.delivery, err.status), expected);
            }
        }
        assert_eq!(entries.load(Ordering::SeqCst), 1);
        assert_eq!(
            fake.calls.lock().unwrap().len(),
            1,
            "no ordinary write retry"
        );
    }
}

#[tokio::test]
async fn balance_add_invalid_input_guard_and_connect_failure_never_write() {
    let entries = Arc::new(AtomicUsize::new(0));
    let hook = entries.clone();
    let fake = Fake::start(move |_| {
        hook.fetch_add(1, Ordering::SeqCst);
        Reply::json(json!({"success":true}))
    })
    .await;
    let client = NewApiClient::new(&fake.config(true)).unwrap();
    for (id, amount) in [(0, 1), (-1, 1), (41, 0), (41, -1), (41, BALANCE_CAP + 1)] {
        assert_not_sent(
            client
                .add_user_quota_until(id, amount, &|| true, deadline())
                .await
                .unwrap_err(),
            ManagementErrorKind::InvalidInput,
        );
    }
    assert_not_sent(
        client
            .add_user_quota_until(41, 1, &|| false, deadline())
            .await
            .unwrap_err(),
        ManagementErrorKind::RevisionConflict,
    );
    assert_not_sent(
        client
            .add_user_quota_until(41, 1, &|| true, Instant::now())
            .await
            .unwrap_err(),
        ManagementErrorKind::QueueFull,
    );
    assert_eq!(entries.load(Ordering::SeqCst), 0);
    let large_inputs = [
        (41, BALANCE_CAP),
        (i64::from(i32::MAX) + 1, i64::from(i32::MAX) + 1),
        (i64::MAX, BALANCE_CAP - 1),
    ];
    for (id, amount) in large_inputs {
        client
            .add_user_quota_until(id, amount, &|| true, deadline())
            .await
            .unwrap();
        assert_eq!(
            fake.calls.lock().unwrap().last().unwrap().body,
            json!({"id":id,"action":"add_quota","mode":"add","value":amount}),
            "valid large values must reach the wire without narrowing"
        );
    }
    assert_eq!(
        entries.load(Ordering::SeqCst),
        large_inputs.len(),
        "positive fixture proves entry hook works"
    );
    let mut cfg = fake.config(true);
    cfg.base_url = "http://127.0.0.1:0".into();
    let disconnected = NewApiClient::new(&cfg).unwrap();
    assert_not_sent(
        disconnected
            .add_user_quota_until(41, 1, &|| true, deadline())
            .await
            .unwrap_err(),
        ManagementErrorKind::TransportUnknown,
    );
    assert_eq!(entries.load(Ordering::SeqCst), large_inputs.len());
}

#[tokio::test]
async fn quota_unit_parses_status_and_rejects_invalid() {
    for (data, valid) in [
        (
            json!({"quota_per_unit":500000,"quota_display_type":"USD"}),
            true,
        ),
        (
            json!({"quota_per_unit":7.3,"quota_display_type":"CNY"}),
            true,
        ),
        (
            json!({"quota_per_unit":5e-324,"quota_display_type":"0123456789AbCdEf"}),
            true,
        ),
        (
            json!({"quota_per_unit":0,"quota_display_type":"USD"}),
            false,
        ),
        (
            json!({"quota_per_unit":-1,"quota_display_type":"USD"}),
            false,
        ),
        (
            json!({"quota_per_unit":null,"quota_display_type":"USD"}),
            false,
        ),
        (
            json!({"quota_per_unit":"500000","quota_display_type":"USD"}),
            false,
        ),
        (json!({"quota_display_type":"USD"}), false),
        (json!({"quota_per_unit":1}), false),
        (json!({"quota_per_unit":1,"quota_display_type":""}), false),
        (
            json!({"quota_per_unit":1,"quota_display_type":"0123456789abcdefg"}),
            false,
        ),
        (
            json!({"quota_per_unit":1,"quota_display_type":"美元"}),
            false,
        ),
        (
            json!({"quota_per_unit":1,"quota_display_type":"US D"}),
            false,
        ),
    ] {
        let response = data.clone();
        let fake = Fake::start(move |call| {
            assert_eq!(call.path, "/api/status");
            assert_eq!(call.method, "GET");
            Reply::json(json!({"success":true,"data":response}))
        })
        .await;
        let client = NewApiClient::new(&fake.config(true)).unwrap();
        match client.quota_unit_until(deadline()).await {
            Ok(unit) => {
                assert!(valid);
                assert_eq!(
                    unit.quota_per_unit,
                    data["quota_per_unit"].as_f64().unwrap()
                );
                assert_eq!(
                    unit.display_type,
                    data["quota_display_type"].as_str().unwrap()
                );
            }
            Err(err) => {
                assert!(!valid);
                assert_eq!(err.kind, ManagementErrorKind::InvalidResponse);
            }
        }
    }
    let fake = Fake::start(|_| Reply {
        body:
            "{\"success\":true,\"data\":{\"quota_per_unit\":1e999,\"quota_display_type\":\"USD\"}}"
                .into(),
        ..Reply::json(Value::Null)
    })
    .await;
    assert_eq!(
        NewApiClient::new(&fake.config(true))
            .unwrap()
            .quota_unit_until(deadline())
            .await
            .unwrap_err()
            .kind,
        ManagementErrorKind::InvalidResponse
    );
}

#[tokio::test(start_paused = true)]
async fn balance_profiles_keep_owner_validation_and_observe_class() {
    for (id, role, quota, valid) in [
        (41, 1, -17, true),
        (42, 1, 10, false),
        (41, 100, 10, false),
        (41, 1, 9_007_199_254_740_992, false),
    ] {
        let fake = Fake::start(move |call| {
            assert_eq!(call.path, "/api/user/self");
            assert!(call.raw.to_ascii_lowercase().contains("new-api-user: 41"));
            profile(id, role, quota)
        })
        .await;
        let budget = ManagementBudget::new();
        let client =
            Arc::new(NewApiClient::with_budget(&fake.config(true), budget.clone()).unwrap());
        let owner = TokenOwner {
            user_id: 41,
            scope: TokenOwnerScope::Issuer,
        };
        for mode in 0..3 {
            let client = client.clone();
            let result = finish(tokio::spawn(async move {
                let tokens = client.managed_tokens(owner);
                match mode {
                    0 => tokens.observe_profile_until(deadline()).await,
                    1 => tokens.profile_until(deadline()).await,
                    _ => tokens.profile().await,
                }
            }))
            .await;
            assert_eq!(result.is_ok(), valid);
            if valid {
                assert_eq!(result.unwrap().quota, -17);
            }
        }
        let classes = budget
            .state
            .lock()
            .unwrap()
            .admissions
            .iter()
            .map(|(_, class)| *class)
            .collect::<Vec<_>>();
        assert_eq!(
            classes,
            vec![
                RequestClass::Observe,
                RequestClass::Control,
                RequestClass::Control
            ]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn balance_profile_entrypoints_keep_deadline_through_all_waits() {
    for observe in [false, true] {
        for stage in 0..3 {
            let entries = Arc::new(AtomicUsize::new(0));
            let hook = entries.clone();
            let fake = Fake::start(move |call| {
                hook.fetch_add(1, Ordering::SeqCst);
                match call.path.as_str() {
                    "/api/user/login" => Reply::json(json!({"success":true,"data":{"id":41}})),
                    "/api/user/self" => profile(41, 1, 100),
                    _ => panic!("unexpected profile fixture path"),
                }
            })
            .await;
            let budget = ManagementBudget::new();
            let client = Arc::new(
                NewApiClient::with_budget(&fake.config(stage == 0), budget.clone()).unwrap(),
            );
            // 0=已认证的业务预算，1=Pending refresh锁，2=Pending登录预算。
            let held = if stage == 1 {
                Vec::new()
            } else {
                vec![
                    budget.admit(RequestClass::Control).await.unwrap(),
                    budget.admit(RequestClass::Control).await.unwrap(),
                ]
            };
            let refresh = if stage == 1 {
                Some(client.transport.refresh.lock().await)
            } else {
                None
            };
            let end = Instant::now() + Duration::from_secs(1);
            let c = client.clone();
            let task = tokio::spawn(async move {
                let tokens = c.managed_tokens(TokenOwner {
                    user_id: 41,
                    scope: TokenOwnerScope::Issuer,
                });
                if observe {
                    tokens.observe_profile_until(end).await
                } else {
                    tokens.profile_until(end).await
                }
            });
            entered(|| budget.requests.available_permits() == QUEUE_LIMIT - 1).await;
            if stage != 1 {
                entered(|| !budget.state.lock().unwrap().queue.is_empty()).await;
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            assert_not_sent(
                finish_at(task, end).await.unwrap_err(),
                ManagementErrorKind::QueueFull,
            );
            assert_eq!(
                entries.load(Ordering::SeqCst),
                0,
                "stage={stage}, observe={observe}"
            );
            assert!(!client.transport.auth.lock().unwrap().failed);
            assert_eq!(budget.requests.available_permits(), QUEUE_LIMIT);
            assert!(budget.state.lock().unwrap().queue.is_empty());
            assert_eq!(budget.state.lock().unwrap().active, held.len());
            drop(refresh);
            drop(held);
            let state = budget.state.lock().unwrap();
            assert_eq!((state.active, state.background), (0, 0));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn balance_public_deadline_before_equal_after_with_idle_budget() {
    for relative_end in [-1i64, 0, 1] {
        for action in 0..4 {
            let entries = Arc::new(AtomicUsize::new(0));
            let hook = entries.clone();
            let fake = Fake::start(move |call| {
                hook.fetch_add(1, Ordering::SeqCst);
                match call.path.as_str() {
                    "/api/user/manage" => Reply::json(json!({"success":true})),
                    "/api/status" => Reply::json(json!({"success":true,"data":{"quota_per_unit":500000,"quota_display_type":"USD"}})),
                    "/api/user/self" => profile(41, 1, 100),
                    _ => panic!("unexpected deadline fixture path"),
                }
            }).await;
            let client = Arc::new(NewApiClient::new(&fake.config(true)).unwrap());
            let start = Instant::now();
            let end = match relative_end {
                -1 => start - Duration::from_secs(1),
                0 => start,
                _ => start + Duration::from_secs(1),
            };
            let c = client.clone();
            let result = finish(tokio::spawn(async move {
                let tokens = c.managed_tokens(TokenOwner {
                    user_id: 41,
                    scope: TokenOwnerScope::Issuer,
                });
                match action {
                    0 => c.add_user_quota_until(41, 1, &|| true, end).await,
                    1 => c.quota_unit_until(end).await.map(|_| ()),
                    2 => tokens.profile_until(end).await.map(|_| ()),
                    _ => tokens.observe_profile_until(end).await.map(|_| ()),
                }
            }))
            .await;
            if relative_end > 0 {
                result.unwrap();
                assert_eq!(
                    entries.load(Ordering::SeqCst),
                    1,
                    "positive fixture must reach HTTP"
                );
            } else {
                assert_not_sent(result.unwrap_err(), ManagementErrorKind::QueueFull);
                assert_eq!(
                    entries.load(Ordering::SeqCst),
                    0,
                    "action={action}, relative_end={relative_end}"
                );
                assert!(client
                    .transport
                    .budget
                    .state
                    .lock()
                    .unwrap()
                    .admissions
                    .is_empty());
                assert_eq!(
                    Instant::now(),
                    start,
                    "equal/expired call must finish without advancing time"
                );
            }
            assert_eq!(
                client.transport.budget.requests.available_permits(),
                QUEUE_LIMIT
            );
            let state = client.transport.budget.state.lock().unwrap();
            assert_eq!(
                (state.active, state.background, state.queue.len()),
                (0, 0, 0)
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn balance_transport_deadlines_bound_waits_without_cancelling_login() {
    // 持续 Control 或每秒延长冷却都不能延长未被选中的 Observe 截止。
    for renewed_cooldown in [false, true] {
        let entries = Arc::new(AtomicUsize::new(0));
        let hook = entries.clone();
        let fake = Fake::start(move |_| {
            hook.fetch_add(1, Ordering::SeqCst);
            Reply::json(json!({"success":true}))
        })
        .await;
        let budget = ManagementBudget::new();
        let mut controls = VecDeque::new();
        if renewed_cooldown {
            budget.cool_down(Duration::from_secs(30));
        } else {
            controls.push_back(budget.admit(RequestClass::Control).await.unwrap());
            controls.push_back(budget.admit(RequestClass::Control).await.unwrap());
        }
        let client =
            Arc::new(NewApiClient::with_budget(&fake.config(true), budget.clone()).unwrap());
        let start = Instant::now();
        let end = start + Duration::from_secs(20);
        let task = tokio::spawn(async move { client.quota_unit_until(end).await });
        entered(|| !budget.state.lock().unwrap().queue.is_empty()).await;
        for _ in 0..20 {
            if renewed_cooldown {
                budget.cool_down(Duration::from_secs(30));
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            if !renewed_cooldown {
                // 每秒替换一个 Control；没有永远占着的后台槽掩盖真正阻塞原因。
                drop(controls.pop_front());
                controls.push_back(budget.admit(RequestClass::Control).await.unwrap());
            }
        }
        assert_not_sent(
            finish(task).await.unwrap_err(),
            ManagementErrorKind::QueueFull,
        );
        assert_eq!(Instant::now(), end);
        assert_eq!(entries.load(Ordering::SeqCst), 0);
        assert!(budget.state.lock().unwrap().queue.is_empty());
        assert_eq!(budget.requests.available_permits(), QUEUE_LIMIT);
        drop(controls);
        assert_eq!(budget.state.lock().unwrap().active, 0);
    }
}

#[tokio::test(start_paused = true)]
async fn balance_pending_login_budget_and_refresh_lock_are_bounded_without_fuse() {
    for lock_wait in [false, true] {
        let fake = Fake::start(|_| Reply::json(json!({"success":true}))).await;
        let transport = Arc::new(fake.transport(false));
        let a = transport.budget.admit(RequestClass::Control).await.unwrap();
        let b = transport.budget.admit(RequestClass::Control).await.unwrap();
        let refresh = if lock_wait {
            Some(transport.refresh.lock().await)
        } else {
            None
        };
        let end = Instant::now() + Duration::from_secs(1);
        let t = transport.clone();
        let task = tokio::spawn(async move {
            t.send_fenced_until(request(), RequestClass::Control, &|| true, end)
                .await
        });
        entered(|| transport.budget.requests.available_permits() == QUEUE_LIMIT - 1).await;
        if !lock_wait {
            entered(|| !transport.budget.state.lock().unwrap().queue.is_empty()).await;
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_not_sent(
            finish_at(task, end).await.err().unwrap(),
            ManagementErrorKind::QueueFull,
        );
        assert!(!transport.auth.lock().unwrap().failed);
        assert!(fake.calls.lock().unwrap().is_empty());
        assert!(transport.budget.state.lock().unwrap().queue.is_empty());
        assert_eq!(transport.budget.requests.available_permits(), QUEUE_LIMIT);
        drop(refresh);
        drop(a);
        drop(b);
    }
}

#[tokio::test(start_paused = true)]
async fn balance_admitted_login_finishes_after_deadline_and_waiter_does_not_cancel_holder() {
    let logins = Arc::new(AtomicUsize::new(0));
    let hook = logins.clone();
    let fake = Fake::start(move |call| {
        assert_eq!(call.path, "/api/user/login");
        hook.fetch_add(1, Ordering::SeqCst);
        Reply {
            delay: Duration::from_secs(3),
            ..Reply::json(json!({"success":true,"data":{"id":41}}))
        }
    })
    .await;
    let transport = Arc::new(fake.transport(false));
    let end = Instant::now() + Duration::from_secs(1);
    let owner = transport.clone();
    let holder = tokio::spawn(async move {
        owner
            .send_fenced_until(request(), RequestClass::Control, &|| true, end)
            .await
    });
    entered(|| logins.load(Ordering::SeqCst) == 1).await;
    let waiter_client = transport.clone();
    let waiter = tokio::spawn(async move {
        waiter_client
            .send_fenced_until(request(), RequestClass::Observe, &|| true, end)
            .await
    });
    entered(|| transport.budget.requests.available_permits() == QUEUE_LIMIT - 2).await;
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_not_sent(
        finish_at(waiter, end).await.err().unwrap(),
        ManagementErrorKind::QueueFull,
    );
    assert!(!holder.is_finished());
    assert!(!transport.auth.lock().unwrap().failed);
    assert_not_sent(
        finish(holder).await.err().unwrap(),
        ManagementErrorKind::QueueFull,
    );
    let state = transport.auth.lock().unwrap();
    assert!(!state.failed);
    assert!(matches!(state.auth, Auth::Session(41)));
    assert_eq!(state.generation, 1);
    drop(state);
    assert_eq!(logins.load(Ordering::SeqCst), 1);
    assert_eq!(transport.budget.state.lock().unwrap().active, 0);
    assert_eq!(transport.budget.requests.available_permits(), QUEUE_LIMIT);
}

#[tokio::test(start_paused = true)]
async fn balance_401_recovery_keeps_deadline_and_rechecks_guard() {
    for mode in 0..3 {
        let writes = Arc::new(AtomicUsize::new(0));
        let hook = writes.clone();
        let allowed = Arc::new(AtomicBool::new(true));
        let marker = allowed.clone();
        let fake = Fake::start(move |call| {
            if call.path == "/api/user/login" {
                if mode == 1 {
                    marker.store(false, Ordering::SeqCst);
                }
                return Reply {
                    delay: if mode == 2 {
                        Duration::from_secs(3)
                    } else {
                        Duration::ZERO
                    },
                    ..Reply::json(json!({"success":true,"data":{"id":41}}))
                };
            }
            assert_eq!(call.path, "/api/user/manage");
            let n = hook.fetch_add(1, Ordering::SeqCst);
            Reply {
                status: if n == 0 { 401 } else { 200 },
                ..Reply::json(json!({"success":n!=0}))
            }
        })
        .await;
        let client = Arc::new(NewApiClient::new(&fake.config(false)).unwrap());
        client.transport.auth.lock().unwrap().auth = Auth::Session(41);
        let end = Instant::now() + Duration::from_secs(if mode == 2 { 2 } else { 20 });
        let c = client.clone();
        let result = finish(tokio::spawn(async move {
            c.add_user_quota_until(41, 17, &|| allowed.load(Ordering::SeqCst), end)
                .await
        }))
        .await;
        if mode == 0 {
            result.unwrap();
            assert_eq!(writes.load(Ordering::SeqCst), 2);
        } else {
            assert_not_sent(
                result.unwrap_err(),
                if mode == 1 {
                    ManagementErrorKind::RevisionConflict
                } else {
                    ManagementErrorKind::QueueFull
                },
            );
            assert_eq!(writes.load(Ordering::SeqCst), 1);
        }
        assert_eq!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.path == "/api/user/login")
                .count(),
            1
        );
        assert!(!client.transport.auth.lock().unwrap().failed);
    }
}

#[tokio::test(start_paused = true)]
async fn balance_401_waits_share_original_deadline_before_login_admission() {
    for lock_wait in [false, true] {
        let budget = ManagementBudget::new();
        let shared_budget = budget.clone();
        let writes = Arc::new(AtomicUsize::new(0));
        let logins = Arc::new(AtomicUsize::new(0));
        let write_hook = writes.clone();
        let login_hook = logins.clone();
        let fake = Fake::start(move |call| {
            if call.path == "/api/user/login" {
                login_hook.fetch_add(1, Ordering::SeqCst);
                return Reply::json(json!({"success":true,"data":{"id":41}}));
            }
            assert_eq!(call.path, "/api/user/manage");
            write_hook.fetch_add(1, Ordering::SeqCst);
            if !lock_wait {
                shared_budget.cool_down(Duration::from_secs(30));
            }
            Reply {
                status: 401,
                ..Reply::json(json!({"success":false}))
            }
        })
        .await;
        let client =
            Arc::new(NewApiClient::with_budget(&fake.config(false), budget.clone()).unwrap());
        client.transport.auth.lock().unwrap().auth = Auth::Session(41);
        let lock = if lock_wait {
            Some(client.transport.refresh.lock().await)
        } else {
            None
        };
        let end = Instant::now() + Duration::from_secs(2);
        let c = client.clone();
        let task = tokio::spawn(async move { c.add_user_quota_until(41, 1, &|| true, end).await });
        entered(|| writes.load(Ordering::SeqCst) == 1).await;
        entered(|| budget.state.lock().unwrap().active == 0).await;
        if !lock_wait {
            entered(|| !budget.state.lock().unwrap().queue.is_empty()).await;
        }
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_not_sent(
            finish_at(task, end).await.unwrap_err(),
            ManagementErrorKind::QueueFull,
        );
        assert_eq!(Instant::now(), end);
        assert_eq!(logins.load(Ordering::SeqCst), 0);
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert!(!client.transport.auth.lock().unwrap().failed);
        assert!(budget.state.lock().unwrap().queue.is_empty());
        assert_eq!(budget.requests.available_permits(), QUEUE_LIMIT);
        drop(lock);
    }
}

#[tokio::test(start_paused = true)]
async fn balance_auth_waiters_respect_64_slots_and_return_them_at_deadline() {
    let fake = Fake::start(|_| Reply::json(json!({"success":true}))).await;
    let transport = Arc::new(fake.transport(false));
    let lock = transport.refresh.lock().await;
    let end = Instant::now() + Duration::from_secs(1);
    let mut tasks = Vec::new();
    for _ in 0..QUEUE_LIMIT {
        let t = transport.clone();
        tasks.push(tokio::spawn(async move {
            t.send_fenced_until(request(), RequestClass::Control, &|| true, end)
                .await
        }));
    }
    entered(|| transport.budget.requests.available_permits() == 0).await;
    assert_not_sent(
        transport
            .send_fenced_until(request(), RequestClass::Control, &|| true, end)
            .await
            .err()
            .unwrap(),
        ManagementErrorKind::QueueFull,
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    for task in tasks {
        assert_not_sent(
            finish_at(task, end).await.err().unwrap(),
            ManagementErrorKind::QueueFull,
        );
    }
    assert_eq!(transport.budget.requests.available_permits(), QUEUE_LIMIT);
    assert!(!transport.auth.lock().unwrap().failed);
    assert!(transport.budget.state.lock().unwrap().queue.is_empty());
    assert!(fake.calls.lock().unwrap().is_empty());
    drop(lock);
}

#[tokio::test(start_paused = true)]
async fn balance_profile_rejects_rebound_principal_after_401() {
    let reads = Arc::new(AtomicUsize::new(0));
    let hook = reads.clone();
    let fake = Fake::start(move |call| {
        if call.path == "/api/user/login" {
            return Reply::json(json!({"success":true,"data":{"id":42}}));
        }
        assert_eq!(call.path, "/api/user/self");
        hook.fetch_add(1, Ordering::SeqCst);
        Reply {
            status: 401,
            ..profile(41, 1, 100)
        }
    })
    .await;
    let client = Arc::new(NewApiClient::new(&fake.config(false)).unwrap());
    client.transport.auth.lock().unwrap().auth = Auth::Session(41);
    let c = client.clone();
    let result = finish(tokio::spawn(async move {
        c.managed_tokens(TokenOwner {
            user_id: 41,
            scope: TokenOwnerScope::Issuer,
        })
        .profile_until(deadline())
        .await
    }))
    .await;
    assert_not_sent(result.unwrap_err(), ManagementErrorKind::AuthFailed);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "must not replay profile as another owner"
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn balance_admitted_add_retains_real_result_past_deadline() {
    for timeout in [false, true] {
        let entries = Arc::new(AtomicUsize::new(0));
        let hook = entries.clone();
        let fake = Fake::start(move |call| {
            assert_eq!(call.path, "/api/user/manage");
            hook.fetch_add(1, Ordering::SeqCst);
            Reply {
                delay: Duration::from_secs(if timeout { 30 } else { 3 }),
                ..Reply::json(json!({"success":true}))
            }
        })
        .await;
        let client = Arc::new(NewApiClient::new(&fake.config(true)).unwrap());
        let c = client.clone();
        let end = Instant::now() + Duration::from_secs(1);
        let task = tokio::spawn(async move { c.add_user_quota_until(41, 1, &|| true, end).await });
        entered(|| entries.load(Ordering::SeqCst) == 1).await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(
            !task.is_finished(),
            "ordinary deadline must not cancel an admitted add"
        );
        let result = finish(task).await;
        if timeout {
            let err = result.unwrap_err();
            assert_eq!(err.delivery, DeliveryEvidence::Uncertain);
            assert_eq!(err.kind, ManagementErrorKind::TransportUnknown);
        } else {
            result.unwrap();
        }
        assert_eq!(entries.load(Ordering::SeqCst), 1);
        assert_eq!(client.transport.budget.state.lock().unwrap().active, 0);
        assert_eq!(
            client.transport.budget.requests.available_permits(),
            QUEUE_LIMIT
        );
    }
}

#[tokio::test(start_paused = true)]
async fn balance_ready_admission_rechecks_deadline_after_slow_guard() {
    let fake = Fake::start(|_| Reply::json(json!({"success":true}))).await;
    let client = NewApiClient::new(&fake.config(true)).unwrap();
    let called = AtomicUsize::new(0);
    let end = Instant::now() + Duration::from_millis(100);
    let guard = || {
        called.fetch_add(1, Ordering::SeqCst);
        // 调用 guard 前冻结时钟，避免高负载下尚未进入 guard 就已过期的伪覆盖。
        tokio::time::resume();
        std::thread::sleep(Duration::from_millis(150));
        true
    };
    assert_not_sent(
        client
            .add_user_quota_until(41, 1, &guard, end)
            .await
            .unwrap_err(),
        ManagementErrorKind::QueueFull,
    );
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert!(fake.calls.lock().unwrap().is_empty());
    assert_eq!(client.transport.budget.state.lock().unwrap().active, 0);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn balance_deadline_pbt_unselected_waiter_never_leaks_or_calls_guard(class in 0u8..3, millis in 1u64..500) {
        tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap().block_on(async {
            let budget=ManagementBudget::new();
            let a=budget.admit(RequestClass::Control).await.unwrap();
            let b=budget.admit(RequestClass::Observe).await.unwrap();
            let hits=AtomicUsize::new(0);let check=||{hits.fetch_add(1,Ordering::SeqCst);Ok(())};
            let class=match class {0=>RequestClass::Control,1=>RequestClass::Observe,_=>RequestClass::Backfill};
            let end=Instant::now()+Duration::from_millis(millis);
            let mut waiting=Box::pin(budget.admit_checked_until(class,&check,end));
            assert!(tokio::time::timeout(Duration::ZERO,&mut waiting).await.is_err());
            assert_eq!(budget.state.lock().unwrap().queue.len(),1);
            tokio::time::advance(Duration::from_millis(millis)).await;
            let result=tokio::time::timeout(Duration::from_millis(1),waiting).await.expect("expired waiter must finish even when never selected");
            assert_not_sent(result.err().unwrap(),ManagementErrorKind::QueueFull);
            assert_eq!(hits.load(Ordering::SeqCst),0); assert!(budget.state.lock().unwrap().queue.is_empty());
            drop(a);drop(b);let state=budget.state.lock().unwrap();assert_eq!((state.active,state.background),(0,0));
        });
    }
}
