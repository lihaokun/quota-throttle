use super::*;
use crate::catalog::balance::{BalancePolicy, BALANCE_CAP};
use crate::newapi::tokens::TokenOwnerScope;
use crate::reconcile::worker::balance::RefillReport;
use std::time::Duration;

#[tokio::test]
async fn balance_routes_shape_budget_and_dry_run() {
    let refill = json!({"schema_version":1,"owner_scope":"issuer","amount":1});
    let policy = json!({"schema_version":1,"expected_revision":1,"auto_refill":false,"warn_below":1,"refill_to":2});
    for (method, path, body, seconds) in [
        ("POST", "/api/admin/v1/balance/refill", &refill, 60),
        ("PUT", "/api/admin/v1/balance/policy", &policy, 10),
    ] {
        let decoded = body_request(method, path, body).unwrap();
        assert!(decoded.writes());
        assert_eq!(decoded.budget(), Duration::from_secs(seconds));
        rejected(
            request(method, path, Some(""), &body.to_string()),
            400,
            "bad_request",
        );
        let mut extra = body.clone();
        extra["private_extra"] = json!(1);
        rejected(body_request(method, path, &extra), 400, "bad_request");
    }
    for scope in [
        json!("unknown"),
        json!({"issuer":null}),
        Value::Null,
        json!(1),
    ] {
        let mut bad = refill.clone();
        bad["owner_scope"] = scope;
        rejected(
            body_request("POST", "/api/admin/v1/balance/refill", &bad),
            400,
            "bad_request",
        );
    }
    for amount in [-1, 0, BALANCE_CAP + 1, crate::catalog::JSON_SAFE + 1] {
        let mut bad = refill.clone();
        bad["amount"] = json!(amount);
        rejected(
            body_request("POST", "/api/admin/v1/balance/refill", &bad),
            422,
            "invalid_input",
        );
    }
    let mut bad = policy.clone();
    bad["warn_below"] = json!(2);
    rejected(
        body_request("PUT", "/api/admin/v1/balance/policy", &bad),
        422,
        "invalid_input",
    );
    let f = support::fixture(true);
    let server = support::Running::start(f.worker.clone()).await;
    for (method, path, body) in [
        ("POST", "/balance/refill", &refill),
        ("PUT", "/balance/policy", &policy),
    ] {
        let (status, value) = server.call(method, path, Some(body)).await;
        assert_eq!(status, 403);
        assert_eq!(value["error"], "dry_run");
    }
    server.finish().await;
}

#[test]
fn balance_success_wires_keep_string_ids_and_explicit_null() {
    for after in [None, Some(-10)] {
        let response = output::encode(output::AdminReply::BalanceRefilled(RefillReport {
            owner_scope: TokenOwnerScope::Issuer,
            user_id: i64::MAX,
            before: -20,
            amount: 10,
            after,
        }))
        .unwrap();
        assert_eq!(response.status, "200 OK");
        let value: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(
            value,
            json!({"schema_version":1,"owner_scope":"issuer","user_id":i64::MAX.to_string(),"before":-20,"amount":10,"after":after})
        );
    }
    let response = output::encode(output::AdminReply::BalancePolicySaved(BalancePolicy {
        revision: 2,
        auto_refill: false,
        warn_below: 1,
        refill_to: 2,
    }))
    .unwrap();
    assert_eq!(response.status, "200 OK");
    assert_eq!(
        serde_json::from_slice::<Value>(&response.body).unwrap(),
        json!({"schema_version":1,"revision":2,"auto_refill":false,"warn_below":1,"refill_to":2})
    );
    assert_eq!(
        output::encode(output::AdminReply::Internal).unwrap().status,
        "500 Internal Server Error"
    );
}
