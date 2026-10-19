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
