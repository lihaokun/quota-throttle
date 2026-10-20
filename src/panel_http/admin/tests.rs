use super::input::AdminRequest;
use super::*;
use proptest::prelude::*;
use serde_json::{json, Value};

mod http;
pub(in crate::panel_http) mod support;

fn request(
    method: &str,
    path: &str,
    query: Option<&str>,
    body: &str,
) -> Result<AdminRequest, AdminFailure> {
    input::decode(route::parse(method, path, query)?, body.as_bytes())
}
fn body_request(method: &str, path: &str, value: &Value) -> Result<AdminRequest, AdminFailure> {
    request(method, path, None, &value.to_string())
}
fn rejected(result: Result<AdminRequest, AdminFailure>, status: u16, code: &str) {
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("request unexpectedly accepted"),
    };
    let response = error.response();
    assert_eq!(response.status[..3].parse::<u16>().unwrap(), status);
    let value: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(value["error"], code);
    assert!(!String::from_utf8_lossy(&response.body).contains("private-h2-"));
}
fn shape_accepted(result: Result<AdminRequest, AdminFailure>) -> bool {
    match result {
        Ok(_) => true,
        Err(e) => {
            rejected(Err(e), 400, "bad_request");
            false
        }
    }
}
fn source_request() -> Value {
    json!({"schema_version":1,"name":" source ","credential":"private-h2-key",
        "settings":{"type":"glm","upstream":{"channel_type":8,"base_url":"https://private-h2.example/chat",
        "model_discovery":{"url":"https://private-h2.example/models"}},"quota":{"quota_url":"https://private-h2.example/quota?type=2",
        "key_headers":[{"key":"X-Selector","value":"private-h2-selector"}]}}})
}
fn usage_request() -> Value {
    json!({"schema_version":1,"from":0,"to":3600,"utc_offset_minutes":480,"grain":"hour"})
}
fn op() -> Value {
    json!({"schema_version":1,"operation_id":"h2_operation_1234","expected_revision":7})
}

#[test]
fn h2_business_error_codes_are_exact_and_only_duplicate_has_context() {
    use crate::catalog::control::ControlErrorCode;
    let mut errors: Vec<_> = ExtraCode::VALUES
        .into_iter()
        .map(|code| {
            if matches!(code, ExtraCode::DuplicateSource) {
                AdminError::duplicate(crate::catalog::SourceId::new(7).unwrap())
            } else {
                AdminError::extra(code)
            }
        })
        .collect();
    errors.extend(
        ControlErrorCode::VALUES
            .iter()
            .map(|code| AdminError::control(serde_json::from_value(json!(code)).unwrap())),
    );
    assert_eq!(errors.len(), 28);
    let mut unique = std::collections::BTreeSet::new();
    for error in errors {
        let response = AdminFailure::Business(error).response();
        let wire: Value = serde_json::from_slice(&response.body).unwrap();
        support::emit("response_AdminError", &wire);
        let code = wire["error"].as_str().unwrap();
        assert!(unique.insert(code.to_owned()));
        let status = match code {
            "invalid_input"
            | "type_mismatch"
            | "invalid_query"
            | "limit_exceeded"
            | "arithmetic_overflow" => "422",
            "not_found" => "404",
            "duplicate_source"
            | "duplicate_group"
            | "duplicate_membership"
            | "retired"
            | "channel_conflict"
            | "revision_conflict"
            | "idempotency_conflict"
            | "binding_drift"
            | "ambiguous_create" => "409",
            "dry_run" => "403",
            "rate_limited" => "429",
            "queue_full" | "not_ready" | "query_cancelled" => "503",
            "auth_failed" | "remote_rejected" | "invalid_response" => "502",
            "transport_unknown" | "query_timeout" | "result_unconfirmed" => "504",
            "storage" | "response_too_large" => "500",
            _ => panic!("unexpected business code"),
        };
        assert_eq!(&response.status[..3], status);
        assert_eq!(
            wire["existing_source_id"],
            if code == "duplicate_source" {
                json!("7")
            } else {
                Value::Null
            }
        );
    }
}

#[test]
fn h2_private_request_work_and_secret_replies_have_no_debug_or_serialize() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(AdminRequest, std::fmt::Debug);
    assert_not_impl!(AdminRequest, serde::Serialize);
    assert_not_impl!(input::NewSource, std::fmt::Debug);
    assert_not_impl!(input::NewSource, serde::Serialize);
    assert_not_impl!(output::AdminReply, std::fmt::Debug);
    assert_not_impl!(output::AdminReply, serde::Serialize);
    assert_not_impl!(bridge::Pending, std::fmt::Debug);
    assert_not_impl!(bridge::Pending, serde::Serialize);
    assert_not_impl!(bridge::JobOutcome, std::fmt::Debug);
    assert_not_impl!(bridge::JobOutcome, serde::Serialize);
    assert_not_impl!(crate::newapi::tokens::RevealedToken, std::fmt::Debug);
    assert_not_impl!(crate::newapi::tokens::RevealedToken, serde::Serialize);
}

#[test]
fn h2_action_budgets_and_dry_run_roles_follow_semantics_not_http_method() {
    let mut manual = source_request();
    manual["model_selection"] = json!({"mode":"manual","manual_models":["model"]});
    let cases = [
        ("POST", "/sources", None, source_request(), true, 30),
        ("POST", "/sources", None, manual, true, 10),
        ("GET", "/sources/1/models", None, Value::Null, false, 10),
        ("GET", "/groups/1/references", None, Value::Null, false, 10),
        (
            "GET",
            "/operations/h2_operation_1234",
            None,
            Value::Null,
            false,
            10,
        ),
        ("GET", "/tokens", None, Value::Null, false, 10),
        ("GET", "/tokens/1", None, Value::Null, false, 10),
        (
            "GET",
            "/native-tokens",
            Some("owner_scope=issuer"),
            Value::Null,
            false,
            30,
        ),
        ("POST", "/usage/query", None, usage_request(), false, 15),
        (
            "POST",
            "/tokens/1/reveal",
            None,
            json!({"schema_version":1,"expected_revision":1}),
            true,
            30,
        ),
        (
            "POST",
            "/groups/1/check",
            None,
            json!({"schema_version":1,"expected_revision":1,"register":false}),
            true,
            30,
        ),
        (
            "POST",
            "/operations/h2_operation_1234/retry",
            None,
            json!({"schema_version":1}),
            true,
            30,
        ),
        (
            "POST",
            "/tokens/register",
            None,
            json!({"schema_version":1,"owner_scope":"legacy_admin","native_token_id":"77","display_name":"old"}),
            true,
            30,
        ),
        ("POST", "/sources/1/models/discover", None, op(), true, 30),
        ("POST", "/memberships/1/ensure", None, op(), true, 10),
    ];
    for (method, path, query, body, writes, seconds) in cases {
        let value = request(
            method,
            &format!("/api/admin/v1{path}"),
            query,
            &if body.is_null() {
                String::new()
            } else {
                body.to_string()
            },
        )
        .unwrap();
        assert_eq!(value.writes(), writes, "{method} {path}");
        assert_eq!(
            value.budget(),
            std::time::Duration::from_secs(seconds),
            "{method} {path}"
        );
    }
}

#[test]
#[ignore = "independent release performance gate"]
fn performance_gate_admin_request_decode() {
    // runner以--release串行执行；普通suite忽略本独立墙钟门禁。
    let mut source = source_request();
    source["credential"] = json!("");
    let overhead = source.to_string().len();
    source["credential"] = json!("x".repeat(crate::panel_http::request::MAX_BODY - overhead));
    let body = source.to_string();
    assert_eq!(body.len(), 1024 * 1024);
    let started = std::time::Instant::now();
    assert!(request("POST", "/api/admin/v1/sources", None, &body).is_ok());
    let elapsed = started.elapsed();
    eprintln!(
        "ADMIN_DTO_BYTES={} ADMIN_DTO_MS={}",
        body.len(),
        elapsed.as_millis()
    );
    assert!(elapsed <= std::time::Duration::from_secs(1));
}

#[test]
fn h2_route_matrix_and_allow_precedence_are_exact() {
    let paths = [
        ("POST", "/sources", "POST"),
        ("GET", "/sources/2/models", "GET, PUT"),
        ("PUT", "/sources/2/models", "GET, PUT"),
        ("POST", "/sources/2/models/discover", "POST"),
        ("PUT", "/sources/2/paused", "PUT"),
        ("POST", "/sources/2/retire", "POST"),
        ("POST", "/sources/2/clear-error", "POST"),
        ("PUT", "/sources/2/volcano-policy", "PUT"),
        ("POST", "/groups", "POST"),
        ("POST", "/groups/3/check", "POST"),
        ("GET", "/groups/3/references", "GET"),
        ("PUT", "/groups/3/pin", "PUT"),
        ("POST", "/memberships", "POST"),
        ("POST", "/memberships/4/ensure", "POST"),
        ("POST", "/memberships/4/retire", "POST"),
        ("GET", "/operations/h2_operation_1234", "GET"),
        ("POST", "/operations/h2_operation_1234/retry", "POST"),
        ("GET", "/tokens", "GET, POST"),
        ("POST", "/tokens", "GET, POST"),
        ("GET", "/tokens/5", "GET, DELETE"),
        ("PUT", "/tokens/5/enabled", "PUT"),
        ("DELETE", "/tokens/5", "GET, DELETE"),
        ("POST", "/tokens/5/reveal", "POST"),
        ("GET", "/native-tokens", "GET"),
        ("POST", "/tokens/register", "POST"),
        ("POST", "/usage/query", "POST"),
    ];
    assert_eq!(paths.len(), 26);
    for (method, path, allow) in paths {
        let path = format!("/api/admin/v1{path}");
        let q = path
            .ends_with("/native-tokens")
            .then_some("owner_scope=issuer");
        assert!(route::parse(method, &path, q).is_ok(), "{method} {path}");
        let error = route::parse("HEAD", &path, None).err().unwrap().response();
        assert_eq!(error.status, "405 Method Not Allowed");
        assert_eq!(error.allow, Some(allow));
    }
    assert!(matches!(
        route::parse("GET", "/api/admin/v1/state", None).unwrap(),
        route::Route::State
    ));
    let bad = route::parse("HEAD", "/api/admin/v1/state", Some(""))
        .err()
        .unwrap()
        .response();
    assert_eq!(bad.status, "400 Bad Request");
    assert_eq!(bad.allow, None);
    for path in [
        "/api/admin/v1/sources/",
        "/api/admin/v1/commands",
        "/api/keys",
        "/api/admin/v1/groups/3/delete",
    ] {
        assert_eq!(
            route::parse("GET", path, None)
                .err()
                .unwrap()
                .response()
                .status,
            "404 Not Found"
        );
    }
}
#[test]
fn h2_query_and_id_boundaries_do_not_alias() {
    for q in [
        "",
        "owner_scope=issuer&owner_scope=legacy_admin",
        "owner_scope=issuer&page_size=100",
        "owner_scope=",
        "owner_scope=%69ssuer",
        "owner_scope=iss+uer",
        "page=1",
        "owner_scope=other",
    ] {
        let r = route::parse("GET", "/api/admin/v1/native-tokens", Some(q))
            .err()
            .unwrap()
            .response();
        assert_eq!(&r.status[..3], "400", "{q}");
    }
    for q in [
        "owner_scope=issuer&page=0",
        "owner_scope=issuer&page=101",
        "owner_scope=issuer&page=-1",
        "owner_scope=issuer&page=999999999999999999999999",
        "owner_scope=issuer&page=1.0",
    ] {
        assert_eq!(
            &route::parse("GET", "/api/admin/v1/native-tokens", Some(q))
                .err()
                .unwrap()
                .response()
                .status[..3],
            "422"
        );
    }
    for id in [
        "0",
        "01",
        "-1",
        "not-an-id",
        "9223372036854775808",
        "9999999999999999999999",
    ] {
        let path = format!("/api/admin/v1/tokens/{id}");
        assert_eq!(
            &route::parse("GET", &path, None)
                .err()
                .unwrap()
                .response()
                .status[..3],
            "422"
        );
    }
    assert!(matches!(
        route::parse(
            "GET",
            "/api/admin/v1/native-tokens",
            Some("owner_scope=legacy_admin&page=100")
        )
        .unwrap(),
        route::Route::NativeTokens(crate::newapi::tokens::TokenOwnerScope::LegacyAdmin, 100)
    ));
    assert!(route::parse(
        "GET",
        "/api/admin/v1/tokens",
        Some("after=9223372036854775807")
    )
    .is_ok());
    assert_eq!(
        &route::parse("GET", "/api/admin/v1/tokens/register", Some("bad=1"))
            .err()
            .unwrap()
            .response()
            .status[..3],
        "405"
    );
}
#[test]
fn h2_json_duplicates_required_null_and_privileged_fields_are_rejected() {
    for body in [
        r#"{"schema_version":1,"schema_version":1,"expected_revision":7}"#,
        r#"{"schema_version":1,"expected_revision":7,"expected_revision":8}"#,
        r#"{"schema_version":1,"expected_revision":7,"owner_user_id":"9"}"#,
        r#"{"schema_version":1,"expected_revision":7}{}"#,
        r#"{"schema_version":2,"expected_revision":7}"#,
        r#"{"schema_version":1,"expected_revision":7.0}"#,
        r#"{"expected_revision":7}"#,
    ] {
        rejected(
            request("POST", "/api/admin/v1/tokens/5/reveal", None, body),
            400,
            "bad_request",
        );
    }
    let create = json!({"schema_version":1,"operation_id":"h2_operation_1234","group_id":"3","expected_group_revision":7,"display_name":"token"});
    rejected(
        body_request("POST", "/api/admin/v1/tokens", &create),
        400,
        "bad_request",
    );
    let mut create = create;
    create["expires_at"] = Value::Null;
    assert!(body_request("POST", "/api/admin/v1/tokens", &create).is_ok());
    create["owner_scope"] = json!("legacy_admin");
    rejected(
        body_request("POST", "/api/admin/v1/tokens", &create),
        400,
        "bad_request",
    );
    let mut pin = op();
    pin["expected_policy_revision"] = json!(4);
    rejected(
        body_request("PUT", "/api/admin/v1/groups/3/pin", &pin),
        400,
        "bad_request",
    );
    pin["membership_id"] = Value::Null;
    assert!(body_request("PUT", "/api/admin/v1/groups/3/pin", &pin).is_ok());
    rejected(
        request(
            "POST",
            "/api/admin/v1/memberships",
            None,
            r#"{"schema_version":1,"source_id":"2","group_id":"3","priority":99}"#,
        ),
        400,
        "bad_request",
    );
    let d = source_request().to_string().replace(
        r#""model_discovery":{"url":"https://private-h2.example/models"}"#,
        r#""model_discovery":null,"model_discovery":{"url":"https://private-h2.example/models"}"#,
    );
    rejected(
        request("POST", "/api/admin/v1/sources", None, &d),
        400,
        "bad_request",
    );
}
#[test]
fn h2_unit_model_mode_rejects_unknown_fields() {
    let mut v = source_request();
    v["model_selection"] = json!({"mode":"discover","extra":"private-h2-input"});
    rejected(
        body_request("POST", "/api/admin/v1/sources", &v),
        400,
        "bad_request",
    );
}
#[test]
fn h2_unit_usage_modes_reject_unknown_fields() {
    for mode in ["any", "unknown"] {
        let mut v = usage_request();
        v["filter"] = json!({"model":{"mode":mode,"value":"private-h2-input"}});
        rejected(
            body_request("POST", "/api/admin/v1/usage/query", &v),
            400,
            "bad_request",
        );
    }
}
#[test]
fn h2_source_defaults_models_and_three_text_domains_stay_distinct() {
    let new = match body_request("POST", "/api/admin/v1/sources", &source_request()).unwrap() {
        AdminRequest::CreateSource(s) => s,
        _ => panic!(),
    };
    assert_eq!(new.name, "source");
    assert_eq!(new.note, "");
    assert!(new.upstream.models.is_empty());
    assert!(matches!(new.selection, input::ModelSelection::Discover));
    assert!(matches!(
        new.upstream.model_discovery.unwrap().auth,
        crate::config::ModelDiscoveryAuth::Bearer
    ));
    let mut v = source_request();
    v["settings"] = json!({"type":"volcano","upstream":{"channel_type":8,"base_url":"https://example.invalid/chat"}});
    v["model_selection"] = json!({"mode":"manual","manual_models":[" z ","a","z","a\nb"]});
    let new = match body_request("POST", "/api/admin/v1/sources", &v).unwrap() {
        AdminRequest::CreateSource(s) => s,
        _ => panic!(),
    };
    match new.kind {
        input::SourceKindInput::Volcano(p) => {
            assert_eq!(p, crate::catalog::VolcanoPolicy::default());
        }
        _ => panic!(),
    }
    match new.selection {
        input::ModelSelection::Manual(m) => assert_eq!(m, vec!["a", "a\nb", "z"]),
        _ => panic!(),
    }
    let mut q = usage_request();
    q["filter"] = json!({"model":{"mode":"exact","value":"a\nb"}});
    rejected(
        body_request("POST", "/api/admin/v1/usage/query", &q),
        422,
        "invalid_query",
    );
    q["filter"] = json!({"model":{"mode":"exact","value":" model "},"native_token_id":{"mode":"exact","value":"77"}});
    let query = match body_request("POST", "/api/admin/v1/usage/query", &q).unwrap() {
        AdminRequest::Usage(q) => q,
        _ => panic!(),
    };
    assert!(
        matches!(&query.filter().model,crate::ledger::analytics::DimensionFilter::Exact(s) if s==" model ")
    );
    assert!(matches!(
        query.filter().token,
        crate::ledger::analytics::DimensionFilter::Exact(77)
    ));
    let register = json!({"schema_version":1,"owner_scope":"issuer","native_token_id":"77","display_name":" token "});
    rejected(
        body_request("POST", "/api/admin/v1/tokens/register", &register),
        422,
        "invalid_input",
    );
}
#[test]
fn h2_command_mapping_preserves_targets_revisions_and_intent() {
    let mut p = op();
    p["expected_policy_revision"] = json!(9);
    p["membership_id"] = json!("4");
    let request = body_request("PUT", "/api/admin/v1/groups/3/pin", &p).unwrap();
    match request {
        AdminRequest::Control { id, command } => {
            assert_eq!(id.as_str(), "h2_operation_1234");
            let value = serde_json::to_value(command).unwrap();
            assert_eq!(
                value,
                json!({"kind":"set_group_pin","group_id":"3","expected_revision":7,"expected_policy_revision":9,"membership_id":"4"})
            );
        }
        _ => panic!(),
    }
    let request = body_request("POST", "/api/admin/v1/memberships/4/ensure", &op()).unwrap();
    match request {
        AdminRequest::Control { command, .. } => {
            let v = serde_json::to_value(command).unwrap();
            assert_eq!(v["membership_id"], "4");
            assert_eq!(v["status"], "enabled");
            assert_eq!(v["expected_revision"], 7);
        }
        _ => panic!(),
    }
    let mut clear = op();
    clear["expected_runtime_revision"] = json!(0);
    rejected(
        body_request("POST", "/api/admin/v1/sources/2/clear-error", &clear),
        422,
        "invalid_input",
    );
    clear["policy"] = json!({"daily_token_limit":null,"failure_threshold":3,"reset_minute":60,"utc_offset_minutes":480});
    assert!(body_request("PUT", "/api/admin/v1/sources/2/volcano-policy", &clear).is_ok());
    let mut bad = clear;
    bad["policy"]
        .as_object_mut()
        .unwrap()
        .remove("daily_token_limit");
    rejected(
        body_request("PUT", "/api/admin/v1/sources/2/volcano-policy", &bad),
        400,
        "bad_request",
    );
}
#[test]
fn h2_error_families_are_safe_and_keep_duplicate_source_id() {
    use crate::catalog::{CatalogError, ErrorKind, SourceId};
    let response = AdminFailure::from(CatalogError {
        kind: ErrorKind::DuplicateSource,
        source_id: Some(SourceId::new(91).unwrap()),
        incomplete_backup: false,
    })
    .response();
    assert_eq!(response.status, "409 Conflict");
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body).unwrap(),
        json!({"schema_version":1,"error":"duplicate_source","existing_source_id":"91"})
    );
    let missing = AdminFailure::from(CatalogError {
        kind: ErrorKind::DuplicateSource,
        source_id: None,
        incomplete_backup: false,
    })
    .response();
    assert_eq!(missing.status, "500 Internal Server Error");
    let storage = AdminFailure::from(CatalogError {
        kind: ErrorKind::BackupFailed,
        source_id: Some(SourceId::new(1).unwrap()),
        incomplete_backup: true,
    })
    .response();
    assert_eq!(
        serde_json::from_slice::<Value>(&storage.body).unwrap(),
        json!({"schema_version":1,"error":"storage","existing_source_id":null})
    );
}

#[test]
fn h2_serde_top_level_arrays_are_not_objects() {
    let mut accepted = Vec::new();
    for (label, method, path, body) in [
        ("reveal", "POST", "/api/admin/v1/tokens/5/reveal", "[1,7]"),
        (
            "retry",
            "POST",
            "/api/admin/v1/operations/h2_operation_1234/retry",
            "[1]",
        ),
        (
            "group",
            "POST",
            "/api/admin/v1/groups",
            r#"[1,"name","native","glm"]"#,
        ),
    ] {
        if shape_accepted(request(method, path, None, body)) {
            accepted.push(label);
        }
    }
    assert!(
        accepted.is_empty(),
        "array-shaped bodies accepted: {accepted:?}"
    );
}
#[test]
fn h2_serde_nested_arrays_are_not_objects() {
    let mut accepted = Vec::new();
    let mut base = source_request();
    base["model_selection"] = json!({"mode":"manual","manual_models":["a"]});
    for label in [
        "settings",
        "upstream",
        "discovery",
        "quota",
        "header",
        "policy",
    ] {
        let mut v = base.clone();
        match label {
            "settings" => {
                v["settings"] = json!([
                    "glm",
                    base["settings"]["upstream"],
                    base["settings"]["quota"]
                ]);
            }
            "upstream" => {
                v["settings"]["upstream"] = json!([8, "https://example.invalid/chat", null])
            }
            "discovery" => {
                v["settings"]["upstream"]["model_discovery"] =
                    json!(["https://example.invalid/models", "bearer"])
            }
            "quota" => v["settings"]["quota"] = json!(["https://example.invalid/quota", [], []]),
            "header" => {
                v["settings"]["quota"]["key_headers"] =
                    json!([["X-Selector", "private-h2-selector"]])
            }
            "policy" => {
                v["settings"] = json!({"type":"volcano","upstream":{"channel_type":8,"base_url":"https://example.invalid/chat"},"policy":[null,3,60,480]})
            }
            _ => unreachable!(),
        }
        if shape_accepted(body_request("POST", "/api/admin/v1/sources", &v)) {
            accepted.push(label);
        }
    }
    let mut q = usage_request();
    q["filter"] = json!([]);
    if shape_accepted(body_request("POST", "/api/admin/v1/usage/query", &q)) {
        accepted.push("filters");
    }
    let mut policy = op();
    policy["expected_runtime_revision"] = json!(0);
    policy["policy"] = json!([null, 3, 60, 480]);
    if shape_accepted(body_request(
        "PUT",
        "/api/admin/v1/sources/2/volcano-policy",
        &policy,
    )) {
        accepted.push("policy command");
    }
    assert!(
        accepted.is_empty(),
        "array-shaped nested objects accepted: {accepted:?}"
    );
}
#[test]
fn h2_serde_tagged_modes_require_objects() {
    let mut accepted = Vec::new();
    let mut s = source_request();
    s["model_selection"] = json!(["discover"]);
    if shape_accepted(body_request("POST", "/api/admin/v1/sources", &s)) {
        accepted.push("discover");
    }
    for mode in ["any", "unknown"] {
        let mut q = usage_request();
        q["filter"] = json!({"model":[mode]});
        if shape_accepted(body_request("POST", "/api/admin/v1/usage/query", &q)) {
            accepted.push(mode);
        }
    }
    assert!(
        accepted.is_empty(),
        "array-shaped tagged modes accepted: {accepted:?}"
    );
}
#[test]
fn h2_serde_scalar_enums_require_strings() {
    let mut accepted = Vec::new();
    let g = json!({"schema_version":1,"name":"group","newapi_group":"native","type":{"glm":null}});
    if shape_accepted(body_request("POST", "/api/admin/v1/groups", &g)) {
        accepted.push("kind");
    }
    let mut s = source_request();
    s["settings"]["upstream"]["model_discovery"]["auth"] = json!({"bearer":null});
    if shape_accepted(body_request("POST", "/api/admin/v1/sources", &s)) {
        accepted.push("auth");
    }
    let r = json!({"schema_version":1,"owner_scope":{"issuer":null},"native_token_id":"77","display_name":"key"});
    if shape_accepted(body_request("POST", "/api/admin/v1/tokens/register", &r)) {
        accepted.push("scope");
    }
    let mut q = usage_request();
    q["grain"] = json!({"hour":null});
    if shape_accepted(body_request("POST", "/api/admin/v1/usage/query", &q)) {
        accepted.push("grain");
    }
    assert!(
        accepted.is_empty(),
        "object-shaped scalar enums accepted: {accepted:?}"
    );
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn h2_pb1_routes_and_dtos_match_independent_rejection_model(n in 1i64..i64::MAX, style in 0u8..5, method_bad in any::<bool>(), query_bad in any::<bool>(), extra in "extra_[a-z]{1,12}") {
        let id=match style {0=>n.to_string(),1=>format!("0{n}"),2=>"0".into(),3=>"9223372036854775808".into(),_=>"text".into()};
        let path=format!("/api/admin/v1/groups/{id}/references");
        let accepted=route::parse(if method_bad {"HEAD"}else{"GET"},&path,query_bad.then_some("extra=1")).is_ok();
        prop_assert_eq!(accepted,style==0 && !method_bad && !query_bad);
        let mut v=source_request();v["model_selection"]=json!({"mode":"discover",extra:"private-h2-random"});
        prop_assert!(body_request("POST","/api/admin/v1/sources",&v).is_err());
    }
}
