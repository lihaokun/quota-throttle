use super::*;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct NativeSpec {
    base_url: String,
    port: u16,
    binary: PathBuf,
    root_id: i64,
    issuer_id: i64,
    root_password: String,
    issuer_password: String,
    channel_id: i64,
    outside_channel_id: i64,
    upstream_url: String,
    model: String,
}
async fn actual_step(worker: &Arc<ReconcileWorker>, id: &OperationId) {
    let _lane = worker.lane.lock().await;
    worker
        .token_step(id, &AtomicBool::new(false))
        .await
        .unwrap();
}
async fn inference(
    client: &reqwest::Client,
    spec: &NativeSpec,
    key: &str,
    claude: bool,
) -> reqwest::StatusCode {
    let path = if claude {
        "/v1/messages"
    } else {
        "/v1/chat/completions"
    };
    let request=client.post(format!("{}{path}",spec.base_url)).json(&json!({"model":spec.model,"max_tokens":16,"stream":false,"messages":[{"role":"user","content":"isolated fixture"}],"group":"outside"}));
    let request = if claude {
        request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
    } else {
        request.bearer_auth(key)
    };
    request.send().await.unwrap().status()
}
#[tokio::test]
#[ignore = "仅通过隔离native组合探针显式运行"]
async fn t5c_native_token_service_combination() {
    let path = PathBuf::from(
        std::env::var_os("SGM5_C_NATIVE_SPEC").expect("isolated probe spec required"),
    )
    .canonicalize()
    .unwrap();
    let directory = path.parent().unwrap();
    assert!(
        directory.starts_with("/tmp")
            && directory
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("quota-throttle-sgm5-c-combination-")
    );
    let spec: NativeSpec = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let url = reqwest::Url::parse(&spec.base_url).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.port(), Some(spec.port));
    assert!(![3000, 3001].contains(&spec.port));
    let upstream = reqwest::Url::parse(&spec.upstream_url).unwrap();
    assert_eq!(upstream.host_str(), Some("127.0.0.1"));
    assert!(![Some(3000), Some(3001)].contains(&upstream.port()));
    let mut manage: crate::config::ManageConfig = toml::from_str("").unwrap();
    manage.data_dir = directory.to_string_lossy().into_owned();
    manage.port = spec.port;
    manage.version = SUPPORTED_BACKEND_VERSION.into();
    let mut process = crate::boot::NewApiProcess::new(&manage, &spec.base_url).unwrap();
    process.binary = spec.binary.clone();
    let mut config: crate::config::NewApiConfig = toml::from_str("").unwrap();
    config.base_url = spec.base_url.clone();
    config.root_username = "qtaudit".into();
    config.root_password = spec.root_password.clone();
    config.admin_token.clear();
    let budget = ManagementBudget::new();
    let admin = Arc::new(NewApiClient::with_budget(&config, budget.clone()).unwrap());
    config.root_username = "qtsigner".into();
    config.root_password = spec.issuer_password.clone();
    let issuer = Arc::new(NewApiClient::with_budget(&config, budget).unwrap());
    // 先真实只读核验受控PID/目录/端口/版本；不向未确认实例发写。
    assert_eq!(admin.check_local_backend(&process).await.issue(), None);
    let app_path = directory.join("catalog.sqlite3");
    let mut store = CatalogStore::open(&app_path).unwrap();
    let source = store
        .create_source(SourceDraft {
            name: "native source".into(),
            note: String::new(),
            credential: SecretString::new("fake-upstream".into()).unwrap(),
            settings: SourceSettings::Glm {
                upstream: UpstreamProfile {
                    channel_type: 8,
                    base_url: spec.upstream_url.clone(),
                    models: vec![spec.model.clone()],
                    model_discovery: None,
                },
                quota: GlmQuotaProfile {
                    quota_url: "https://unused.invalid".into(),
                    global_headers: vec![],
                    key_headers: vec![],
                },
            },
        })
        .unwrap();
    let group = store
        .create_group(GroupDraft {
            name: "native group".into(),
            newapi_group: "sgm-native".into(),
            kind: SourceKind::Glm,
        })
        .unwrap();
    let member = store
        .plan_membership(source.source_id, group.group_id, 100)
        .unwrap();
    let conn = rusqlite::Connection::open(&app_path).unwrap();
    conn.execute(
        "UPDATE memberships SET channel_id=?1,lifecycle='imported' WHERE membership_id=?2",
        rusqlite::params![spec.channel_id, member.membership_id.get()],
    )
    .unwrap();
    conn.execute(
        "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
        [member.membership_id.get()],
    )
    .unwrap();
    store
        .submit_operation(
            operation(0),
            ControlCommand::EnsureMember {
                membership_id: member.membership_id,
                expected_revision: 1,
                status: StatusDirective::Enabled,
            },
            now(),
        )
        .unwrap();
    let worker = ReconcileWorker::new_with_token_owners(
        store,
        admin.clone(),
        policy_config(),
        TokenOwners::new(Some((issuer, spec.issuer_id)), Some(spec.root_id)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        worker.refresh_backend(&process).await.unwrap().issue(),
        None
    );
    let report = worker.check_group(group.group_id, 1, true).await.unwrap();
    assert!(report.ready);
    assert_eq!(report.revision, 2);
    // 策略许可是本组合的受控依赖fixture；不发真实GLM额度请求，不宣称SGM7全系统验收。
    publish(&worker, member.membership_id, false);
    for _ in 0..8 {
        worker.reconcile_once().await.unwrap();
        if !worker.work(member.membership_id).unwrap().target.dirty {
            break;
        }
    }
    assert!(!worker.work(member.membership_id).unwrap().target.dirty);
    assert_eq!(
        admin
            .managed_channels()
            .get_channel(spec.channel_id)
            .await
            .unwrap()
            .observation()
            .unwrap()
            .status,
        1
    );
    assert_eq!(
        worker.refresh_backend(&process).await.unwrap().issue(),
        None
    );
    let created = worker
        .create_token(token_op(0), group.group_id, 2, "native access".into(), None)
        .unwrap();
    let id = ManagedTokenId::new(created.target_id.parse().unwrap()).unwrap();
    actual_step(&worker, &token_op(0)).await;
    assert_eq!(
        worker.operation(&token_op(0)).unwrap().phase,
        OperationPhase::Waiting
    );
    assert!(worker.token_identity(id).unwrap().native_token_id.is_none());
    actual_step(&worker, &token_op(0)).await;
    assert_eq!(
        worker.operation(&token_op(0)).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert_eq!(
        worker.refresh_backend(&process).await.unwrap().issue(),
        None
    );
    let secret = worker.reveal_token(id, 1).await.unwrap();
    assert!(secret.expose().starts_with("sk-"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    for claude in [false, true] {
        assert_eq!(
            inference(&client, &spec, secret.expose(), claude).await,
            reqwest::StatusCode::OK
        );
        let suffix = format!("{}-{}", secret.expose(), spec.outside_channel_id);
        assert!(inference(&client, &spec, &suffix, claude)
            .await
            .is_client_error());
    }
    admin
        .managed_channels()
        .write_status(&[spec.channel_id], false)
        .await
        .unwrap();
    for claude in [false, true] {
        assert!(!inference(&client, &spec, secret.expose(), claude)
            .await
            .is_success());
    }
    admin
        .managed_channels()
        .write_status(&[spec.channel_id], true)
        .await
        .unwrap();
    assert_eq!(
        worker.refresh_backend(&process).await.unwrap().issue(),
        None
    );
    worker.set_token_enabled(token_op(1), id, 1, false).unwrap();
    actual_step(&worker, &token_op(1)).await;
    actual_step(&worker, &token_op(1)).await;
    assert_eq!(
        worker.operation(&token_op(1)).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert!(!inference(&client, &spec, secret.expose(), false)
        .await
        .is_success());
    worker.set_token_enabled(token_op(2), id, 2, true).unwrap();
    actual_step(&worker, &token_op(2)).await;
    actual_step(&worker, &token_op(2)).await;
    assert_eq!(
        worker.operation(&token_op(2)).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert_eq!(
        worker.refresh_backend(&process).await.unwrap().issue(),
        None
    );
    worker.delete_token(token_op(3), id, 3).unwrap();
    actual_step(&worker, &token_op(3)).await;
    actual_step(&worker, &token_op(3)).await;
    assert_eq!(
        worker.operation(&token_op(3)).unwrap().phase,
        OperationPhase::Succeeded
    );
    assert!(worker.token_identity(id).unwrap().deleted_at.is_some());
    let references = worker.group_references(group.group_id).unwrap();
    assert_eq!(
        (references.live_tokens, references.historical_tokens),
        (0, 1)
    );
    assert_eq!(
        worker
            .list_tokens(TokenOwnerScope::LegacyAdmin, 1)
            .await
            .unwrap()
            .total,
        0
    );
    assert!(worker
        .snapshot()
        .unwrap()
        .sources
        .iter()
        .any(|s| s.source_id == source.source_id));
    println!("NATIVE_TOKEN_SERVICE_OK");
}
