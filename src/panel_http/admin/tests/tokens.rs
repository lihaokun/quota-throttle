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

/// 目录快照 + 本地身份页：被拒的首次提交前后必须完全相同（catalog 为 WAL 模式，不比较文件字节）。
async fn observed(f: &TokenFixture, server: &AdminTestServer) -> (Value, Value) {
    (
        serde_json::to_value(f.worker.snapshot().unwrap()).unwrap(),
        http(server, "GET", "/tokens", None, 200).await,
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
