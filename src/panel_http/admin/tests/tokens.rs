use super::*;
use crate::panel_http::AdminTestServer;

async fn http(
    server: &AdminTestServer,
    method: &str,
    path: &str,
    body: Option<&Value>,
    expected: u16,
) -> Value {
    let (status, value) = server.call(method, path, body).await;
    assert_eq!(status, expected, "{method} {path}: {value}");
    if !path.ends_with("/reveal") {
        assert!(!value.to_string().contains(ACCESS_KEY));
    }
    value
}

#[tokio::test]
async fn h2_tcp_token_create_native_local_reveal_enable_delete_use_original_service() {
    let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
    let server = AdminTestServer::start(f.worker.clone()).await;
    let group = f.worker.snapshot().unwrap().groups[0].clone();
    let check = http(
        &server,
        "POST",
        "/groups/1/check",
        Some(&json!({"schema_version":1,"expected_revision":group.revision,"register":false})),
        200,
    )
    .await;
    assert_eq!(check["ready"], true);
    let revision = f.worker.snapshot().unwrap().groups[0].revision;
    let body = json!({"schema_version":1,"operation_id":"http_token_create_01","group_id":"1","expected_group_revision":revision,"display_name":"client key","expires_at":null});
    let receipt = http(&server, "POST", "/tokens", Some(&body), 202).await;
    assert_eq!(receipt["kind"], "create_token");
    assert_eq!(receipt["target_id"], "1");
    assert_eq!(puts(&f, "/api/token/"), 0);
    let op = OperationId::new("http_token_create_01".into()).unwrap();
    token_step(&f, &op).await.unwrap();
    token_step(&f, &op).await.unwrap();
    let identity = http(&server, "GET", "/tokens/1", None, 200).await;
    assert_eq!(identity["native_token_id"], "101");
    assert_eq!(identity["owner_scope"], "issuer");
    assert_eq!(identity["owner_user_id"], "41");
    let page = http(&server, "GET", "/tokens", None, 200).await;
    assert_eq!(page["items"], json!([identity.clone()]));
    assert_eq!(
        http(&server, "GET", "/tokens?after=1", None, 200).await["items"],
        json!([])
    );
    assert_eq!(
        http(&server, "GET", "/tokens/101", None, 404).await["error"],
        "not_found"
    );
    let native = http(
        &server,
        "GET",
        "/native-tokens?owner_scope=issuer&page=1",
        None,
        200,
    )
    .await;
    assert_eq!(native["items"].as_array().unwrap().len(), 1);
    assert_eq!(native["owner_scope"], "issuer");
    let reveal = http(
        &server,
        "POST",
        "/tokens/1/reveal",
        Some(&json!({"schema_version":1,"expected_revision":1})),
        200,
    )
    .await;
    assert_eq!(reveal["token_id"], "1");
    assert_eq!(reveal["key"], format!("sk-{ACCESS_KEY}"));
    let (status, state) = server.call("GET", "/state", None).await;
    assert_eq!(status, 200, "copy isolation needs a real published state");
    assert_eq!(state["catalog"]["sources"].as_array().unwrap().len(), 1);
    assert!(!state.to_string().contains(ACCESS_KEY));
    let replay = http(&server, "POST", "/tokens", Some(&body), 202).await;
    assert_eq!(replay["phase"], "succeeded");
    assert_eq!(
        http(
            &server,
            "POST",
            "/operations/http_token_create_01/retry",
            Some(&json!({"schema_version":1})),
            200
        )
        .await,
        replay
    );
    assert_eq!(puts(&f, "/api/token/"), 1);
    http(&server,"PUT","/tokens/1/enabled",Some(&json!({"schema_version":1,"operation_id":"http_token_disable_1","expected_revision":1,"enabled":false})),202).await;
    let op = OperationId::new("http_token_disable_1".into()).unwrap();
    token_step(&f, &op).await.unwrap();
    token_step(&f, &op).await.unwrap();
    assert_eq!(f.state.tokens.lock().unwrap()[&101]["status"], 2);
    http(&server,"DELETE","/tokens/1",Some(&json!({"schema_version":1,"operation_id":"http_token_delete_01","expected_revision":2})),202).await;
    let op = OperationId::new("http_token_delete_01".into()).unwrap();
    token_step(&f, &op).await.unwrap();
    token_step(&f, &op).await.unwrap();
    assert!(http(&server, "GET", "/tokens/1", None, 200).await["deleted_at"].is_number());
    assert_eq!(
        http(&server, "GET", "/tokens", None, 200).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(f.state.tokens.lock().unwrap().is_empty());
    server.finish().await;
}

/// 直接读目录、身份、store策略和余额槽位；不让发布缓存掩盖拒绝后的写入。
async fn observed(f: &TokenFixture, server: &AdminTestServer) -> (Value, Value, Value, Value) {
    (
        serde_json::to_value(f.worker.snapshot().unwrap()).unwrap(),
        http(server, "GET", "/tokens", None, 200).await,
        serde_json::to_value(
            f.worker
                .core
                .lock()
                .unwrap()
                .store
                .balance_policy()
                .unwrap(),
        )
        .unwrap(),
        f.worker.balance_test_last_refills().unwrap(),
    )
}

/// 架构 I8：以新操作 ID 首次提交、返回 500 / 504 以外的错误时，后端不留任何记录。按提交族逐类钉住。
#[tokio::test]
async fn h2_first_submit_rejections_leave_no_record() {
    let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
    let server = AdminTestServer::start(f.worker.clone()).await;
    let group = f.worker.snapshot().unwrap().groups[0].clone();
    http(
        &server,
        "POST",
        "/groups/1/check",
        Some(&json!({"schema_version":1,"expected_revision":group.revision,"register":false})),
        200,
    )
    .await;
    let revision = f.worker.snapshot().unwrap().groups[0].revision;
    let create = |id: &str| json!({"schema_version":1,"operation_id":id,"group_id":"1","expected_group_revision":revision,"display_name":"client key","expires_at":null});
    // 身份 1 创建完成；身份 2 只提交、不执行，保持「从未创建」（事故现场）。
    http(
        &server,
        "POST",
        "/tokens",
        Some(&create("t1_token_created_01")),
        202,
    )
    .await;
    let op = OperationId::new("t1_token_created_01".into()).unwrap();
    token_step(&f, &op).await.unwrap();
    token_step(&f, &op).await.unwrap();
    http(
        &server,
        "POST",
        "/tokens",
        Some(&create("t1_token_never_0001")),
        202,
    )
    .await;
    // 计划一个没有渠道的 GLM 成员，供固定命令拒绝。
    let source_body = json!({"schema_version":1,"name":"planned","credential":"private-test-planned",
        "settings":{"type":"glm","upstream":{"channel_type":8,"base_url":"https://planned.invalid/v1"},
        "quota":{"quota_url":"https://quota.invalid","key_headers":[]}},
        "model_selection":{"mode":"manual","manual_models":["planned-model"]}});
    let source = http(&server, "POST", "/sources", Some(&source_body), 201).await;
    let plan = json!({"schema_version":1,"source_id":source["source"]["source_id"],"group_id":"1"});
    let member = http(&server, "POST", "/memberships", Some(&plan), 201).await;
    assert_eq!(member["membership"]["lifecycle"], "planned");
    let (_, state) = server.call("GET", "/state", None).await;
    let panel_group = &state["catalog"]["groups"][0];
    let rejected: [(&str, &str, Value, u16, &str, Option<&str>); 8] = [
        // ① 删除从未创建的 Key
        (
            "DELETE",
            "/tokens/2",
            json!({"schema_version":1,"operation_id":"t1_delete_never_01","expected_revision":1}),
            503,
            "not_ready",
            Some("t1_delete_never_01"),
        ),
        // ② 删除 Key 带错误的身份版本
        (
            "DELETE",
            "/tokens/1",
            json!({"schema_version":1,"operation_id":"t1_delete_stale_01","expected_revision":99}),
            409,
            "revision_conflict",
            Some("t1_delete_stale_01"),
        ),
        // ③ 暂停来源带过期版本
        (
            "PUT",
            "/sources/1/paused",
            json!({"schema_version":1,"operation_id":"t1_pause_stale_001","expected_revision":999,"paused":true}),
            409,
            "revision_conflict",
            Some("t1_pause_stale_001"),
        ),
        // ④ 固定一个不可编辑（计划中、无渠道）的 GLM 成员
        (
            "PUT",
            "/groups/1/pin",
            json!({"schema_version":1,"operation_id":"t1_pin_planned_001","expected_revision":panel_group["group"]["revision"],"expected_policy_revision":panel_group["policy_revision"],"membership_id":member["membership"]["membership_id"]}),
            503,
            "not_ready",
            Some("t1_pin_planned_001"),
        ),
        // ⑤ 建组使用重复的 NewAPI 分组标识（目录类写，没有操作 ID）
        (
            "POST",
            "/groups",
            json!({"schema_version":1,"name":"dup","newapi_group":"managed","type":"glm"}),
            409,
            "duplicate_group",
            None,
        ),
        // ⑥ 创建 Key 带过期的组版本
        (
            "POST",
            "/tokens",
            json!({"schema_version":1,"operation_id":"t1_create_stale_01","group_id":"1","expected_group_revision":999,"display_name":"stale","expires_at":null}),
            409,
            "revision_conflict",
            Some("t1_create_stale_01"),
        ),
        // ⑦ 用已登记的凭据再建来源（面板有专门文案的错误码）
        (
            "POST",
            "/sources",
            source_body.clone(),
            409,
            "duplicate_source",
            None,
        ),
        // ⑧ 同一来源再次计划进同一组
        (
            "POST",
            "/memberships",
            plan.clone(),
            409,
            "duplicate_membership",
            None,
        ),
    ];
    for (method, path, body, status, code, operation) in rejected {
        let before = observed(&f, &server).await;
        assert_eq!(
            http(&server, method, path, Some(&body), status).await["error"],
            code,
            "{method} {path}"
        );
        if let Some(id) = operation {
            assert_eq!(
                http(&server, "GET", &format!("/operations/{id}"), None, 404).await["error"],
                "not_found"
            );
        }
        assert_eq!(observed(&f, &server).await, before, "{method} {path}");
    }
    // ⑨ 先真实更新，再用旧revision改另一值；拒绝不能覆盖目录库或内存策略。
    let policy = json!({"schema_version":1,"expected_revision":1,"auto_refill":false,"warn_below":100,"refill_to":1000});
    assert_eq!(
        http(&server, "PUT", "/balance/policy", Some(&policy), 200).await["revision"],
        2
    );
    let before = observed(&f, &server).await;
    let mut stale = policy.clone();
    stale["warn_below"] = json!(200);
    assert_eq!(
        http(&server, "PUT", "/balance/policy", Some(&stale), 409).await["error"],
        "revision_conflict"
    );
    assert_eq!(observed(&f, &server).await, before);

    // 入口计数先用真实成功add证明有效；不以未知路径panic后的空日志证明零发送。
    let adds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let profiles = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (add_count, profile_count) = (adds.clone(), profiles.clone());
    *f.state.hook.lock().unwrap() = Some(Arc::new(move |r| {
        if r.method == "POST" && r.path == "/api/user/manage" {
            add_count.fetch_add(1, Ordering::SeqCst);
        }
        if r.method == "GET" && r.path == "/api/user/self" {
            profile_count.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let positive = json!({"schema_version":1,"owner_scope":"issuer","amount":1});
    f.state.balance_quotas.lock().unwrap().insert(41, 10000);
    let quota_before = f.state.balance_quotas.lock().unwrap()[&41];
    http(&server, "POST", "/balance/refill", Some(&positive), 200).await;
    assert_eq!(adds.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.state.balance_quotas.lock().unwrap()[&41],
        quota_before + 1
    );
    adds.store(0, Ordering::SeqCst);
    profiles.store(0, Ordering::SeqCst);
    // ⑩ amount本身合法，但before+amount超cap，必须到worker第4步才拒绝。
    f.state.balance_quotas.lock().unwrap().insert(41, 10000);
    let before = observed(&f, &server).await;
    let excessive = json!({"schema_version":1,"owner_scope":"issuer","amount":crate::catalog::balance::BALANCE_CAP});
    assert_eq!(
        http(&server, "POST", "/balance/refill", Some(&excessive), 422).await["error"],
        "invalid_input"
    );
    assert!(profiles.load(Ordering::SeqCst) > 0);
    assert_eq!(adds.load(Ordering::SeqCst), 0);
    assert_eq!(f.state.balance_quotas.lock().unwrap()[&41], 10000);
    let after = observed(&f, &server).await;
    assert_eq!(
        (&after.0, &after.1, &after.2),
        (&before.0, &before.1, &before.2)
    );
    assert_eq!(after.3[1], before.3[1]);
    assert_eq!(after.3.as_array().unwrap().len(), 2);
    assert_eq!(after.3[0]["owner_scope"], "issuer");
    let at = after.3[0]["last_refill"]["at_ms"].as_i64().unwrap();
    assert!(at >= before.3[0]["last_refill"]["at_ms"].as_i64().unwrap());
    assert_eq!(
        after.3[0]["last_refill"],
        json!({"at_ms":at,"source":"manual","outcome":"not_sent","amount":null,"error_code":"invalid_input"})
    );
    server.finish().await;
}

#[tokio::test]
async fn h2_tcp_legacy_only_register_reveal_and_manage_do_not_require_issuer() {
    let f = tokens_fixture(SourceKind::Glm, &[1], false, true).await;
    f.state
        .tokens
        .lock()
        .unwrap()
        .insert(77, token(77, 1, "managed"));
    let server = AdminTestServer::start(f.worker.clone()).await;
    let identity = http(&server,"POST","/tokens/register",Some(&json!({"schema_version":1,"owner_scope":"legacy_admin","native_token_id":"77","display_name":"legacy"})),200).await;
    assert_eq!(identity["token_id"], "1");
    assert_eq!(identity["native_token_id"], "77");
    assert_eq!(identity["owner_scope"], "legacy_admin");
    let reveal = http(
        &server,
        "POST",
        "/tokens/1/reveal",
        Some(&json!({"schema_version":1,"expected_revision":1})),
        200,
    )
    .await;
    assert_eq!(reveal["key"], format!("sk-{ACCESS_KEY}"));
    let revision = f.worker.snapshot().unwrap().groups[0].revision;
    assert_eq!(http(&server,"POST","/tokens",Some(&json!({"schema_version":1,"operation_id":"http_legacy_no_new_1","group_id":"1","expected_group_revision":revision,"display_name":"new","expires_at":null})),503).await["error"],"not_ready");
    http(&server,"PUT","/tokens/1/enabled",Some(&json!({"schema_version":1,"operation_id":"http_legacy_disable1","expected_revision":1,"enabled":false})),202).await;
    let op = OperationId::new("http_legacy_disable1".into()).unwrap();
    token_step(&f, &op).await.unwrap();
    token_step(&f, &op).await.unwrap();
    assert_eq!(f.state.tokens.lock().unwrap()[&77]["status"], 2);
    assert_eq!(puts(&f, "/api/token/"), 0);
    assert!(f
        .fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.path.starts_with("/api/token/"))
        .all(|r| caller(r) == 1));
    server.finish().await;
}

#[tokio::test]
async fn balance_http_success_and_failed_readback_bind_original_request() {
    let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
    let server = AdminTestServer::start(f.worker.clone()).await;
    for (scope, user) in [("issuer", 41), ("legacy_admin", 1)] {
        f.state.balance_quotas.lock().unwrap().insert(user, -10);
        let request = json!({"schema_version":1,"owner_scope":scope,"amount":19});
        let response = http(&server, "POST", "/balance/refill", Some(&request), 200).await;
        assert_eq!(
            response,
            json!({"schema_version":1,"owner_scope":scope,"user_id":user.to_string(),"before":-10,"amount":19,"after":9})
        );
        assert_eq!(f.state.balance_quotas.lock().unwrap()[&user], 9);
    }
    f.state
        .balance_read_fail_after_add
        .store(true, Ordering::SeqCst);
    // 清掉计数，使下一次资料读取可用，而add确认后的回读才失败。
    f.state.balance_adds.store(0, Ordering::SeqCst);
    let request = json!({"schema_version":1,"owner_scope":"issuer","amount":5});
    let response = http(&server, "POST", "/balance/refill", Some(&request), 200).await;
    assert_eq!(
        response,
        json!({"schema_version":1,"owner_scope":"issuer","user_id":"41","before":9,"amount":5,"after":null})
    );
    let records = f.worker.balance_test_last_refills().unwrap();
    assert_eq!(records[0]["last_refill"]["outcome"], "refilled");
    assert_eq!(records[0]["last_refill"]["amount"], 5);
    server.finish().await;
}

#[tokio::test]
async fn balance_http_uncertain_is_504_and_preserves_lock_record() {
    for (mode, status, code, outcome) in [
        (1, 502, "remote_rejected", "rejected"),
        (2, 504, "result_unconfirmed", "uncertain"),
    ] {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        f.state.balance_mode.store(mode, Ordering::SeqCst);
        let server = AdminTestServer::start(f.worker.clone()).await;
        let body = json!({"schema_version":1,"owner_scope":"issuer","amount":7});
        assert_eq!(
            http(&server, "POST", "/balance/refill", Some(&body), status).await["error"],
            code
        );
        let before = f.worker.balance_test_last_refills().unwrap();
        assert_eq!(before[0]["last_refill"]["outcome"], outcome);
        assert_eq!(before[0]["last_refill"]["amount"], 7);
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        if mode == 2 {
            assert_eq!(before[0]["last_refill"]["error_code"], "transport_unknown");
            assert_eq!(
                http(&server, "POST", "/balance/refill", Some(&body), 503).await["error"],
                "not_ready"
            );
            assert_eq!(f.worker.balance_test_last_refills().unwrap(), before);
            assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        }
        server.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn balance_http_refill_completes_after_old_connection_limit() {
    use crate::panel_http::{admin_test_drive as drive, admin_test_until as until};
    drive(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let hold = f
            .worker
            .balance_test_owner_lock(TokenOwnerScope::Issuer)
            .await;
        let server = Arc::new(AdminTestServer::start(f.worker.clone()).await);
        let caller = server.clone();
        let task = tokio::spawn(async move {
            caller
                .call(
                    "POST",
                    "/balance/refill",
                    Some(&json!({"schema_version":1,"owner_scope":"issuer","amount":1})),
                )
                .await
        });
        until(|| f.worker.balance_test_waiters(TokenOwnerScope::Issuer) == 1).await;
        let waiting_at = tokio::time::Instant::now();
        tokio::time::advance(Duration::from_secs(50)).await;
        assert!(!task.is_finished());
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 0);
        drop(hold);
        // profile和add耗尽初始两份配额；确认add已结算后，显式推进第三次回读所需的1s。
        // 只yield会把暂停时钟永远留在50s，令回读预算等待无法结束。
        until(|| {
            f.worker.balance_test_last_refills().unwrap()[0]["last_refill"]["outcome"] == "refilled"
        })
        .await;
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), 1);
        assert!(!task.is_finished());
        tokio::time::advance(Duration::from_secs(1)).await;
        until(|| task.is_finished()).await;
        let (status, response) = task.await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(response["amount"], 1);
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 1);
        assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), 2);
        assert_eq!(waiting_at.elapsed(), Duration::from_secs(51));
        Arc::try_unwrap(server).ok().unwrap().finish().await;
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn balance_http_refill_waiting_lock_stops_at_route_deadline_without_send() {
    use crate::panel_http::{admin_test_drive as drive, admin_test_until as until};
    drive(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let hold = f
            .worker
            .balance_test_owner_lock(TokenOwnerScope::Issuer)
            .await;
        let before = f.worker.balance_test_last_refills().unwrap();
        let server = Arc::new(AdminTestServer::start(f.worker.clone()).await);
        let caller = server.clone();
        let task = tokio::spawn(async move {
            caller
                .call(
                    "POST",
                    "/balance/refill",
                    Some(&json!({"schema_version":1,"owner_scope":"issuer","amount":1})),
                )
                .await
        });
        until(|| f.worker.balance_test_waiters(TokenOwnerScope::Issuer) == 1).await;
        tokio::time::advance(Duration::from_secs(60)).await;
        let (status, response) = task.await.unwrap();
        assert_eq!(status, 504);
        assert_eq!(response["error"], "result_unconfirmed");
        until(|| f.worker.balance_test_waiters(TokenOwnerScope::Issuer) == 0).await;
        drop(hold);
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 0);
        assert_eq!(f.worker.balance_test_last_refills().unwrap(), before);
        Arc::try_unwrap(server).ok().unwrap().finish().await;
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn balance_http_slow_head_and_body_keep_original_accept_deadline() {
    use crate::panel_http::{
        admin_test_drive as drive, admin_test_until as until, AdminSlowRefillConnection,
    };
    drive(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let held = f
            .worker
            .balance_test_owner_lock(TokenOwnerScope::Issuer)
            .await;
        let before = f.worker.balance_test_last_refills().unwrap();
        let connection = AdminSlowRefillConnection::start(f.worker.clone()).await;
        until(|| f.worker.balance_test_waiters(TokenOwnerScope::Issuer) == 1).await;
        assert_eq!(connection.accepted_at.elapsed(), Duration::from_secs(13));
        // entered+60=accepted+73；必须以原accepted+75−5=accepted+70为业务截止。
        tokio::time::advance(Duration::from_secs(56)).await;
        assert!(!connection.response.is_finished());
        assert_eq!(f.worker.balance_test_waiters(TokenOwnerScope::Issuer), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        until(|| connection.response.is_finished()).await;
        assert_eq!(connection.accepted_at.elapsed(), Duration::from_secs(70));
        until(|| f.worker.balance_test_waiters(TokenOwnerScope::Issuer) == 0).await;
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 0);
        assert_eq!(f.state.balance_reads.load(Ordering::SeqCst), 0);
        assert_eq!(f.worker.balance_test_last_refills().unwrap(), before);
        drop(held);
        let (status, response) = connection.finish().await;
        assert_eq!(status, 504);
        assert_eq!(response["error"], "result_unconfirmed");
        assert_eq!(f.state.balance_adds.load(Ordering::SeqCst), 0);
    })
    .await;
}
