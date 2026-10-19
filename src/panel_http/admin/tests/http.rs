use super::support::{fixture, Running};
use super::*;
use crate::catalog::control::OperationId;
use crate::newapi::transport::tests::{Fake, Reply};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn h2_tcp_policy_retirement_and_model_update_routes_preserve_version_categories() {
    use crate::catalog::{
        SecretString, SourceDraft, SourceSettings, UpstreamProfile, VolcanoPolicy,
    };
    let f = super::support::fixture_with(false, |store| {
        let source = store
            .create_source(SourceDraft {
                name: "v".into(),
                note: String::new(),
                credential: SecretString::new("private-h2-runtime".into()).unwrap(),
                settings: SourceSettings::Volcano {
                    upstream: UpstreamProfile {
                        channel_type: 8,
                        base_url: "https://unused.invalid/v1".into(),
                        models: vec!["old".into()],
                        model_discovery: None,
                    },
                    policy: VolcanoPolicy::default(),
                },
            })
            .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        store
            .initialize_source_policy(source.source_id, 1, now)
            .unwrap();
    });
    let server = Running::start(f.worker.clone()).await;
    let clear = json!({"schema_version":1,"operation_id":"h2_clear_error_01","expected_revision":1,"expected_runtime_revision":1});
    assert_eq!(
        expect(&server, "POST", "/sources/1/clear-error", Some(&clear), 202).await["kind"],
        "clear_volcano_error"
    );
    // 撤错会推进error epoch/runtime revision；配置revision也沿原操作推进。
    let policy = json!({"schema_version":1,"operation_id":"h2_update_policy1","expected_revision":2,"expected_runtime_revision":2,"policy":{"daily_token_limit":20,"failure_threshold":4,"reset_minute":70,"utc_offset_minutes":480}});
    assert_eq!(
        expect(
            &server,
            "PUT",
            "/sources/1/volcano-policy",
            Some(&policy),
            202
        )
        .await["kind"],
        "update_volcano_policy"
    );
    let update = json!({"schema_version":1,"operation_id":"h2_update_models1","expected_revision":3,"models":["new"]});
    assert_eq!(
        expect(&server, "PUT", "/sources/1/models", Some(&update), 202).await["kind"],
        "sync_source_models"
    );
    assert_eq!(
        expect(&server, "GET", "/sources/1/models", None, 200).await["models"],
        json!(["new"])
    );
    let group = json!({"schema_version":1,"name":"glm","newapi_group":"glm","type":"glm"});
    expect(&server, "POST", "/groups", Some(&group), 201).await;
    let pin = json!({"schema_version":1,"operation_id":"h2_clear_pin_1234","expected_revision":1,"expected_policy_revision":1,"membership_id":null});
    assert_eq!(
        expect(&server, "PUT", "/groups/1/pin", Some(&pin), 202).await["kind"],
        "set_group_pin"
    );
    let vg = json!({"schema_version":1,"name":"v","newapi_group":"v","type":"volcano"});
    expect(&server, "POST", "/groups", Some(&vg), 201).await;
    expect(
        &server,
        "POST",
        "/memberships",
        Some(&json!({"schema_version":1,"source_id":"1","group_id":"2"})),
        201,
    )
    .await;
    let retire =
        json!({"schema_version":1,"operation_id":"h2_retire_member1","expected_revision":1});
    assert_eq!(
        expect(&server, "POST", "/memberships/1/retire", Some(&retire), 202).await["kind"],
        "retire_member"
    );
    let retire =
        json!({"schema_version":1,"operation_id":"h2_retire_source1","expected_revision":4});
    assert_eq!(
        expect(&server, "POST", "/sources/1/retire", Some(&retire), 202).await["kind"],
        "retire_source"
    );
    expect(&server, "GET", "/sources/1/models", None, 200).await;
    server.finish().await;
}

fn manual_source(n: usize) -> Value {
    json!({"schema_version":1,"name":format!("source-{n}"),"credential":format!("private-h2-{n}"),"settings":{"type":"volcano","upstream":{"channel_type":8,"base_url":"https://unused.invalid/v1"}},"model_selection":{"mode":"manual","manual_models":["z-model","a-model"]}})
}
async fn expect(
    server: &Running,
    method: &str,
    path: &str,
    body: Option<&Value>,
    status: u16,
) -> Value {
    let (got, value) = server.call(method, path, body).await;
    assert_eq!(got, status, "{method} {path}: {value}");
    assert!(!value.to_string().contains("private-h2-"));
    value
}

#[tokio::test]
async fn h2_tcp_catalog_control_and_operation_recovery_keep_original_receipts() {
    let f = fixture(false);
    let server = Running::start(f.worker.clone()).await;
    let source = expect(&server, "POST", "/sources", Some(&manual_source(1)), 201).await;
    assert_eq!(source["source"]["source_id"], "1");
    assert_eq!(source["model_origin"], "manual");
    let models = expect(&server, "GET", "/sources/1/models", None, 200).await;
    assert_eq!(models["models"], json!(["a-model", "z-model"]));
    let duplicate = expect(&server, "POST", "/sources", Some(&manual_source(1)), 409).await;
    assert_eq!(duplicate["existing_source_id"], "1");
    let group = expect(
        &server,
        "POST",
        "/groups",
        Some(
            &json!({"schema_version":1,"name":"volcano","newapi_group":"volcano","type":"volcano"}),
        ),
        201,
    )
    .await;
    assert_eq!(group["group"]["ready"], false);
    let member = expect(
        &server,
        "POST",
        "/memberships",
        Some(&json!({"schema_version":1,"source_id":"1","group_id":"1"})),
        201,
    )
    .await;
    assert_eq!(member["membership"]["lifecycle"], "planned");
    assert_eq!(
        member["membership"]["desired_priority"],
        f.worker.standby_priority()
    );
    let ensure =
        json!({"schema_version":1,"operation_id":"h2_ensure_1234567","expected_revision":1});
    let receipt = expect(&server, "POST", "/memberships/1/ensure", Some(&ensure), 202).await;
    assert_eq!(receipt["schema_version"], 3);
    assert_eq!(receipt["operation_id"], "h2_ensure_1234567");
    assert_eq!(receipt["kind"], "ensure_member");
    assert_eq!(
        expect(&server, "POST", "/memberships/1/ensure", Some(&ensure), 202).await,
        receipt
    );
    assert_eq!(
        expect(&server, "GET", "/operations/h2_ensure_1234567", None, 200).await,
        receipt
    );
    let native = f
        .worker
        .operation(&OperationId::new("h2_ensure_1234567".into()).unwrap())
        .unwrap();
    assert_eq!(serde_json::to_value(native).unwrap(), receipt);
    let conflict =
        json!({"schema_version":1,"operation_id":"h2_ensure_1234567","expected_revision":2});
    assert_eq!(
        expect(
            &server,
            "POST",
            "/memberships/1/ensure",
            Some(&conflict),
            409
        )
        .await["error"],
        "idempotency_conflict"
    );
    let refs = expect(&server, "GET", "/groups/1/references", None, 200).await;
    assert_eq!(refs["deletion_authorized"], false);
    let snapshot = f.worker.snapshot().unwrap();
    assert_eq!(snapshot.sources.len(), 1);
    assert_eq!(snapshot.groups.len(), 1);
    assert_eq!(snapshot.memberships.len(), 1);
    server.finish().await;
}

#[tokio::test]
async fn h2_tcp_bad_dto_and_cross_origin_never_mutate_catalog() {
    let f = fixture(false);
    let server = Running::start(f.worker.clone()).await;
    let body = json!({"schema_version":1,"name":"g","newapi_group":"g","type":"glm"});
    let request = String::from_utf8(server.wire("POST", "/groups", &body.to_string())).unwrap();
    let evil = request.replace(
        &format!("Origin: http://{}", server.authority),
        "Origin: http://evil.invalid",
    );
    assert_eq!(server.raw(evil.as_bytes()).await.0, 403);
    let malformed = body
        .to_string()
        .replacen("\"name\":\"g\"", "\"name\":\"g\",\"name\":\"g\"", 1);
    assert_eq!(
        server
            .raw(&server.wire("POST", "/groups", &malformed))
            .await
            .0,
        400
    );
    let (status, header, body) = server
        .raw(&server.wire("HEAD", "/sources/01/models", ""))
        .await;
    assert_eq!(status, 405);
    assert!(header.contains("Allow: GET, PUT\r\n"));
    assert!(body.is_empty());
    assert!(f.worker.snapshot().unwrap().groups.is_empty());
    server.finish().await;
}

#[tokio::test]
async fn h2_tcp_model_discovery_is_authoritative_and_duplicate_precedes_network() {
    let f = fixture(false);
    let fail = Arc::new(AtomicBool::new(false));
    let failed = fail.clone();
    let upstream = Fake::start(move |r| {
        assert_eq!(r.path, "/models");
        assert!(r
            .raw
            .to_ascii_lowercase()
            .contains("authorization: bearer private-h2-1"));
        if failed.load(Ordering::SeqCst) {
            Reply::json(json!({"error":"private-h2-provider-error"}))
        } else {
            Reply::json(json!({"data":[{"id":"discovered-z"},{"id":"discovered-a"}]}))
        }
    })
    .await;
    let server = Running::start(f.worker.clone()).await;
    let mut source = manual_source(1);
    source["settings"]["upstream"]["model_discovery"] =
        json!({"url":format!("{}/models",upstream.url)});
    source["model_selection"] = json!({"mode":"discover_or_manual","manual_models":["fallback"]});
    let saved = expect(&server, "POST", "/sources", Some(&source), 201).await;
    assert_eq!(saved["model_origin"], "discovered");
    assert_eq!(
        expect(&server, "GET", "/sources/1/models", None, 200).await["models"],
        json!(["discovered-a", "discovered-z"])
    );
    fail.store(true, Ordering::SeqCst);
    assert_eq!(
        expect(&server, "POST", "/sources", Some(&source), 409).await["error"],
        "duplicate_source"
    );
    assert_eq!(upstream.calls.lock().unwrap().len(), 1);
    let discover =
        json!({"schema_version":1,"operation_id":"h2_discover_12345","expected_revision":1});
    assert_eq!(
        expect(
            &server,
            "POST",
            "/sources/1/models/discover",
            Some(&discover),
            502
        )
        .await["error"],
        "invalid_response"
    );
    assert_eq!(
        expect(&server, "GET", "/sources/1/models", None, 200).await["source_revision"],
        1
    );
    assert_eq!(f.worker.snapshot().unwrap().sources.len(), 1);
    server.finish().await;
}

#[tokio::test(start_paused = true)]
async fn h2_busy_discovery_cannot_use_manual_fallback_or_create_placeholder() {
    super::support::drive(async {
        let f = fixture(false);
        let upstream = Fake::start(|_| Reply {
            delay: std::time::Duration::from_secs(1),
            ..Reply::json(json!({"data":[{"id":"discovered"}]}))
        })
        .await;
        let server = Arc::new(Running::start(f.worker.clone()).await);
        let mut first = manual_source(1);
        first.as_object_mut().unwrap().remove("model_selection");
        first["settings"]["upstream"]["model_discovery"] =
            json!({"url":format!("{}/models",upstream.url)});
        let mut second = first.clone();
        second["credential"] = json!("private-h2-busy-second");
        second["model_selection"] =
            json!({"mode":"discover_or_manual","manual_models":["fallback"]});
        let running_server = server.clone();
        let running =
            tokio::spawn(
                async move { running_server.call("POST", "/sources", Some(&first)).await },
            );
        super::support::until(|| upstream.calls.lock().unwrap().len() == 1).await;
        assert_eq!(
            expect(&server, "POST", "/sources", Some(&second), 503).await["error"],
            "queue_full"
        );
        assert!(f.worker.snapshot().unwrap().sources.is_empty());
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert_eq!(running.await.unwrap().0, 201);
        assert_eq!(f.worker.snapshot().unwrap().sources.len(), 1);
        assert_eq!(upstream.calls.lock().unwrap().len(), 1);
        Arc::try_unwrap(server).ok().unwrap().finish().await;
    })
    .await;
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb2_admin_dry_run_and_stale_revisions_cannot_write(dry in any::<bool>(), old in 2i64..1000, pause in any::<bool>()) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let f = fixture(dry);
            let server = Running::start(f.worker.clone()).await;
            let before = std::fs::read(f.dir.path().join("catalog.sqlite3")).unwrap();
            if dry {
                assert_eq!(expect(&server,"POST","/sources",Some(&manual_source(1)),403).await["error"],"dry_run");
                assert_eq!(expect(&server,"POST","/groups/1/check",Some(&json!({"schema_version":1,"expected_revision":1,"register":false})),403).await["error"],"dry_run");
                assert_eq!(expect(&server,"POST","/tokens/1/reveal",Some(&json!({"schema_version":1,"expected_revision":1})),403).await["error"],"dry_run");
                let usage = expect(&server,"POST","/usage/query",Some(&usage_request()),200).await;
                assert_eq!(usage["usage"],Value::Null);
                assert_eq!(usage["ledger"]["initial_requested_from"],Value::Null);
                assert_eq!(before,std::fs::read(f.dir.path().join("catalog.sqlite3")).unwrap());
            } else {
                expect(&server,"POST","/sources",Some(&manual_source(1)),201).await;
                let before = serde_json::to_value(f.worker.source_models(crate::catalog::SourceId::new(1).unwrap()).unwrap()).unwrap();
                let op = json!({"schema_version":1,"operation_id":"h2_pause_stale_12","expected_revision":old,"paused":pause});
                assert_eq!(expect(&server,"PUT","/sources/1/paused",Some(&op),409).await["error"],"revision_conflict");
                assert_eq!(expect(&server,"GET","/sources/1/models",None,200).await,before);
                assert!(!f.worker.snapshot().unwrap().sources[0].manual_paused);
            }
            server.finish().await;
        });
    }

    #[test]
    fn pb3_admin_same_operation_replays_but_different_target_conflicts(pause in any::<bool>(), suffix in 0u64..1_000_000) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let f = fixture(false);
            let server = Running::start(f.worker.clone()).await;
            for n in 1..=2 { expect(&server,"POST","/sources",Some(&manual_source(n)),201).await; }
            let id = format!("h2_replay_{suffix:016}");
            let op = json!({"schema_version":1,"operation_id":id,"expected_revision":1,"paused":pause});
            let receipt = expect(&server,"PUT","/sources/1/paused",Some(&op),202).await;
            assert_eq!(expect(&server,"PUT","/sources/1/paused",Some(&op),202).await,receipt);
            assert_eq!(expect(&server,"GET",&format!("/operations/{id}"),None,200).await,receipt);
            assert_eq!(expect(&server,"PUT","/sources/2/paused",Some(&op),409).await["error"],"idempotency_conflict");
            assert_eq!(expect(&server,"GET","/sources/2/models",None,200).await["source_revision"],1);
            server.finish().await;
        });
    }

    #[test]
    fn pb4_admin_discovery_failure_fallback_and_duplicate_never_overwrite(succeeds in any::<bool>(), fallback in any::<bool>(), suffix in 0u16..1000) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let f = fixture(false);
            let model = format!("discovered-{suffix}");
            let returned = model.clone();
            let upstream = Fake::start(move |_| Reply::json(if succeeds { json!({"data":[{"id":returned}]}) } else { json!({"data":[]}) })).await;
            let server = Running::start(f.worker.clone()).await;
            let mut source = manual_source(1);
            source["settings"]["upstream"]["model_discovery"] = json!({"url":format!("{}/models",upstream.url)});
            source["model_selection"] = if fallback {json!({"mode":"discover_or_manual","manual_models":["fallback"]})} else {json!({"mode":"discover"})};
            if !fallback && suffix%2==0 { source.as_object_mut().unwrap().remove("model_selection"); }
            if !succeeds && !fallback {
                assert_eq!(expect(&server,"POST","/sources",Some(&source),502).await["error"],"invalid_response");
                assert!(f.worker.snapshot().unwrap().sources.is_empty());
            } else {
                let response = expect(&server,"POST","/sources",Some(&source),201).await;
                assert_eq!(response["model_origin"],if succeeds {"discovered"} else {"fallback"});
                let models = expect(&server,"GET","/sources/1/models",None,200).await;
                assert_eq!(models["models"],json!([if succeeds {model.as_str()} else {"fallback"}]));
                source["model_selection"] = json!({"mode":"manual","manual_models":["overwrite-attempt"]});
                assert_eq!(expect(&server,"POST","/sources",Some(&source),409).await["existing_source_id"],"1");
                assert_eq!(expect(&server,"GET","/sources/1/models",None,200).await,models);
            }
            assert_eq!(upstream.calls.lock().unwrap().len(),1);
            server.finish().await;
        });
    }
}
