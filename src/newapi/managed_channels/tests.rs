use super::*;
use crate::newapi::transport::tests::{Fake, Reply};
use proptest::prelude::*;
use std::sync::{Arc, Mutex};

fn channel(id: i64) -> Value {
    json!({"id":id,"name":format!("member-{id}"),"type":8,"group":format!("group-{id}"),
        "base_url":format!("https://example.invalid/{id}"),"models":format!("model-{id},shared"),
        "priority":id,"status":3,"auto_ban":1,"channel_info":{"is_multi_key":false},
        "key":"fake-secret-key","header_override":"{\"X-Secret\":\"fake-secret-header\"}",
        "unknown_config":{"keep":id}})
}
fn page(start: i64, count: usize, total: usize, number: usize) -> Value {
    json!({"items":(start..start+count as i64).map(channel).collect::<Vec<_>>(),"total":total,"page":number,"page_size":100})
}

#[test]
fn missing_fields_bad_ids_and_invalid_states_are_rejected() {
    let doc = RawChannelDocument::new(channel(1)).unwrap();
    let read_at = doc.observation().unwrap().observed_at;
    std::thread::sleep(std::time::Duration::from_millis(1));
    assert_eq!(
        doc.observation().unwrap().observed_at,
        read_at,
        "converting a cached document must not refresh its observation timestamp"
    );
    for field in [
        "id",
        "type",
        "group",
        "base_url",
        "models",
        "priority",
        "status",
        "auto_ban",
        "channel_info",
    ] {
        let mut value = channel(1);
        value.as_object_mut().unwrap().remove(field);
        assert!(RawChannelDocument::new(value).is_err(), "missing {field}");
    }
    for (field, value) in [
        ("id", json!(0)),
        ("priority", json!(null)),
        ("status", json!(0)),
        ("auto_ban", json!(2)),
    ] {
        let mut doc = channel(1);
        doc[field] = value;
        assert!(RawChannelDocument::new(doc).is_err());
    }
}

#[test]
fn malformed_pages_are_incomplete_not_empty_success() {
    for bad in [
        json!({}),
        page(1, 99, 101, 1),
        page(1, 100, 10001, 1),
        page(1, 100, 101, 2),
    ] {
        assert!(ScanAccumulator::default().append(1, &bad).is_err());
    }
    for second in [
        page(100, 1, 101, 2),
        page(101, 1, 102, 2),
        page(101, 0, 101, 2),
    ] {
        let mut scan = ScanAccumulator::default();
        scan.append(1, &page(1, 100, 101, 1)).unwrap();
        assert!(scan.append(2, &second).is_err());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pagination_preserves_complete_distinguishable_records(count in 0usize..550, step in 1i64..12) {
        let original: Vec<_> = (1..=count).map(|id| channel(id as i64 * step)).collect();
        let mut scan = ScanAccumulator::default();
        let pages = count.div_ceil(100).max(1);
        for p in 1..=pages {
            let end = (p*100).min(count); let start = ((p-1)*100).min(count);
            let data = json!({"items":original[start..end],"total":count,"page":p,"page_size":100});
            prop_assert_eq!(scan.append(p,&data).unwrap(), p == pages);
        }
        let actual: Vec<_> = scan.channels.into_iter().map(|d| d.0).collect();
        prop_assert_eq!(actual, original);
    }

    #[test]
    fn patches_preserve_unknown_identity_and_native_state_ownership(id in 1i64..10000, priority in -1000i64..1000, model in "[a-z]{3,12}") {
        let original = channel(id);
        let patch = ChannelPatch { priority:Some(priority),models:Some(model.clone()),disable_auto_ban:false };
        let changed = RawChannelDocument::new(original.clone()).unwrap().patch(&patch).unwrap().unwrap();
        let mut expected = original;
        expected["priority"] = json!(priority); expected["models"] = json!(model);
        expected.as_object_mut().unwrap().remove("status"); expected.as_object_mut().unwrap().remove("key");
        prop_assert_eq!(changed, expected);
    }

    #[test]
    fn r12_only_one_explicit_false_is_disabled(values in prop::collection::vec(prop_oneof![Just(json!("false")),Just(json!("true")),Just(json!(false)),Just(json!(null)),Just(json!(""))],0..5)) {
        let items: Vec<_> = values.iter().map(|v| json!({"key":"AutomaticEnableChannelEnabled","value":v})).collect();
        let actual = parse_recovery_option(&json!({"data":items}));
        let expected = if values == vec![json!("false")] { NativeRecoveryCheck::Disabled }
            else if values == vec![json!("true")] { NativeRecoveryCheck::Enabled } else { NativeRecoveryCheck::Unknown };
        prop_assert_eq!(actual,expected);
    }
}

#[tokio::test]
async fn real_http_scan_reaches_second_page_and_reports_partial_coverage() {
    let fake = Fake::start(|call| {
        assert!(call.path.contains("sort_by=id&sort_order=asc"));
        let data = if call.path.contains("p=1&") {
            page(1, 100, 101, 1)
        } else {
            page(101, 1, 101, 2)
        };
        Reply::json(json!({"success":true,"data":data}))
    })
    .await;
    let transport = fake.transport(true);
    let api = ManagedChannels::new(&transport);
    let scan = api.scan_channels(None).await.unwrap();
    assert!(scan.complete);
    assert_eq!(scan.channels.len(), 101);
    assert_eq!(scan.channels[100].observation().unwrap().name, "member-101");
    let wanted = HashSet::from([1, 50]);
    let partial = api.scan_channels(Some(&wanted)).await.unwrap();
    assert!(!partial.complete);
    assert_eq!(partial.total, 101);
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn missing_required_id_needs_full_scan_and_bad_total_is_rejected() {
    let fake = Fake::start(|call| {
        let data = if call.path.contains("p=1&") {
            page(1, 100, 101, 1)
        } else {
            page(101, 1, 102, 2)
        };
        Reply::json(json!({"success":true,"data":data}))
    })
    .await;
    let transport = fake.transport(true);
    assert!(ManagedChannels::new(&transport)
        .scan_channels(Some(&HashSet::from([101])))
        .await
        .is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn update_preserves_unowned_fields_and_skips_equal_model_set() {
    let state = Arc::new(Mutex::new(channel(7)));
    let server_state = state.clone();
    let fake = Fake::start(move |call| {
        if call.method == "PUT" {
            assert_eq!(call.path, "/api/channel/");
            assert!(call.body.get("key").is_none() && call.body.get("status").is_none());
            let mut original = server_state.lock().unwrap();
            let status = original["status"].clone();
            *original = call.body.clone();
            original["status"] = status;
            Reply::json(json!({"success":true}))
        } else {
            Reply::json(json!({"success":true,"data":*server_state.lock().unwrap()}))
        }
    })
    .await;
    let transport = fake.transport(true);
    let api = ManagedChannels::new(&transport);
    assert!(!api
        .write_channel_fields(
            7,
            ChannelPatch {
                models: Some(" shared,model-7,shared".into()),
                ..Default::default()
            }
        )
        .await
        .unwrap());
    assert!(api
        .write_channel_fields(
            7,
            ChannelPatch {
                priority: Some(42),
                ..Default::default()
            }
        )
        .await
        .unwrap());
    let current = state.lock().unwrap();
    assert_eq!(current["priority"], 42);
    assert_eq!(current["status"], 3);
    assert_eq!(current["auto_ban"], 1);
    assert_eq!(current["group"], "group-7");
    assert_eq!(current["unknown_config"], json!({"keep":7}));
    assert_eq!(current["header_override"], channel(7)["header_override"]);
    assert_eq!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.method == "PUT")
            .count(),
        1
    );
}

#[tokio::test]
async fn create_is_disabled_single_group_and_volcano_only_disables_auto_ban() {
    let fake = Fake::start(|_| Reply::json(json!({"success":true}))).await;
    let transport = fake.transport(true);
    let api = ManagedChannels::new(&transport);
    let key = SecretString::new("fake-upstream-secret".into()).unwrap();
    for kind in [SourceKind::Glm, SourceKind::Volcano] {
        api.create_disabled_channel(DisabledChannelSpec {
            name: "qt-instance-m1",
            source_kind: kind,
            channel_type: 8,
            key: &key,
            base_url: "https://fake.invalid/v1",
            models: "model-b,model-a",
            group: "source-group",
            standby_priority: 10,
        })
        .await
        .unwrap();
    }
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for c in calls.iter() {
        assert_eq!(c.path, "/api/channel/");
        assert_eq!(c.body["mode"], "single");
        assert_eq!(c.body["channel"]["status"], 2);
        assert_eq!(c.body["channel"]["group"], "source-group");
        assert_eq!(c.body["channel"]["priority"], 10);
        assert_eq!(c.body["channel"]["key"], "fake-upstream-secret");
    }
    assert!(calls[0].body["channel"].get("auto_ban").is_none());
    assert_eq!(calls[1].body["channel"]["auto_ban"], 0);
}

#[tokio::test]
async fn status_ack_deduplicates_ids_but_does_not_claim_readback() {
    let fake = Fake::start(|_| Reply::json(json!({"success":true,"data":0}))).await;
    let transport = fake.transport(true);
    let api = ManagedChannels::new(&transport);
    api.write_status(&[9, 1, 9], false).await.unwrap();
    api.write_status(&[7, 7], true).await.unwrap();
    assert!(api.write_status(&vec![1; 101], true).await.is_err());
    assert!(api.write_status(&[], false).await.is_err());
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].path, "/api/channel/status/batch");
    assert_eq!(calls[0].body, json!({"ids":[1,9],"status":2}));
    assert_eq!(calls[1].path, "/api/channel/7/status");
    assert_eq!(calls[1].body, json!({"status":1}));
}

#[tokio::test]
async fn recovery_preflight_is_read_only_and_never_defaults_unknown_to_false() {
    for value in [json!("true"), json!("false"), json!(null)] {
        let expected = match value.as_str() {
            Some("true") => NativeRecoveryCheck::Enabled,
            Some("false") => NativeRecoveryCheck::Disabled,
            _ => NativeRecoveryCheck::Unknown,
        };
        let fake = Fake::start(move |_| Reply::json(json!({"success":true,"data":[{"key":"AutomaticEnableChannelEnabled","value":value}]}))).await;
        assert_eq!(
            ManagedChannels::new(&fake.transport(true))
                .check_native_recovery()
                .await
                .unwrap(),
            expected
        );
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "GET");
        assert_eq!(calls[0].path, "/api/option/");
    }
}

#[tokio::test]
async fn mismatched_id_and_multi_key_cannot_be_updated() {
    let fake = Fake::start(|_| {
        let mut data = channel(5);
        data["channel_info"]["is_multi_key"] = json!(true);
        Reply::json(json!({"success":true,"data":data}))
    })
    .await;
    let transport = fake.transport(true);
    let api = ManagedChannels::new(&transport);
    assert!(api.get_channel(4).await.is_err());
    assert!(api
        .write_channel_fields(
            5,
            ChannelPatch {
                priority: Some(10),
                ..Default::default()
            }
        )
        .await
        .is_err());
    assert!(fake.calls.lock().unwrap().iter().all(|c| c.method == "GET"));
}

/// 显式指定只读二进制；新临时库/假管理员/随机端口，默认测试不启动原生进程。
#[tokio::test]
#[ignore = "requires explicit QUOTA_THROTTLE_NEWAPI_TEST_BINARY; isolated native process only"]
async fn native_managed_channel_contract_on_fresh_database() {
    use std::process::{Command, Stdio};
    use std::time::Duration;
    struct Native(std::process::Child);
    impl Drop for Native {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let binary = std::env::var_os("QUOTA_THROTTLE_NEWAPI_TEST_BINARY")
        .expect("explicit native binary required");
    assert!(std::path::Path::new(&binary).is_absolute());
    let directory = tempfile::tempdir().unwrap();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    assert!(![3000, 3001].contains(&port));
    drop(reservation);
    let database = directory.path().join("native-test.db");
    let child = Command::new(binary)
        .current_dir(directory.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PORT", port.to_string())
        .env("GIN_MODE", "release")
        .env("GOMAXPROCS", "2")
        .env("SQLITE_PATH", &database)
        .env(
            "SESSION_SECRET",
            directory.path().to_string_lossy().as_ref(),
        )
        .env("GLOBAL_API_RATE_LIMIT_ENABLE", "false")
        .env("CRITICAL_RATE_LIMIT_ENABLE", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut process = Native(child);
    let mut owns_listener = false;
    for _ in 0..100 {
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "isolated native process exited"
        );
        let output = Command::new("ss")
            .args(["-H", "-ltnp", &format!("sport = :{port}")])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&output.stdout).contains(&format!("pid={},", process.0.id())) {
            owns_listener = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        owns_listener && database.exists(),
        "must prove this process owns listener and test DB before setup"
    );
    let mut config: crate::config::NewApiConfig = toml::from_str("").unwrap();
    config.base_url = format!("http://127.0.0.1:{port}");
    config.root_username = "auditroot".into();
    config.root_password = format!("fake-password-{}", process.0.id());
    let client = crate::newapi::NewApiClient::new(&config).unwrap();
    client.authenticate().await.unwrap();
    let api = client.managed_channels();
    let key = SecretString::new("fake-native-upstream-key".into()).unwrap();
    for (name, group, kind) in [
        ("qt-test-m1", "audit-a", SourceKind::Glm),
        ("qt-test-m2", "audit-b", SourceKind::Volcano),
    ] {
        api.create_disabled_channel(DisabledChannelSpec {
            name,
            source_kind: kind,
            channel_type: 8,
            key: &key,
            base_url: "http://127.0.0.1:9/chat/completions",
            models: "fake-model-a,fake-model-b",
            group,
            standby_priority: 10,
        })
        .await
        .unwrap();
    }
    let initial = api.scan_channels(None).await.unwrap();
    assert!(initial.complete);
    assert_eq!(initial.total, 2);
    let first = initial.channels[0].observation().unwrap();
    let second = initial.channels[1].observation().unwrap();
    assert_eq!(client.list_channels().await.unwrap().len(), 2);
    client
        .set_channel_priority(first.channel_id, 50)
        .await
        .unwrap();
    assert_eq!(
        api.get_channel(first.channel_id)
            .await
            .unwrap()
            .observation()
            .unwrap()
            .priority,
        50
    );
    assert_eq!((first.status, second.status), (2, 2));
    assert_eq!(second.auto_ban, 0);
    api.write_channel_fields(
        first.channel_id,
        ChannelPatch {
            priority: Some(100),
            models: Some("fake-model-b".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let after = api
        .get_channel(first.channel_id)
        .await
        .unwrap()
        .observation()
        .unwrap();
    assert_eq!(
        (after.priority, after.status, after.auto_ban),
        (100, 2, first.auto_ban)
    );
    assert_eq!(after.models, "fake-model-b");
    assert_eq!(after.group, "audit-a");
    assert_eq!(
        api.get_channel(second.channel_id)
            .await
            .unwrap()
            .observation()
            .unwrap()
            .priority,
        10
    );
    api.write_status(&[first.channel_id, second.channel_id], true)
        .await
        .unwrap();
    for id in [first.channel_id, second.channel_id] {
        assert_eq!(
            api.get_channel(id)
                .await
                .unwrap()
                .observation()
                .unwrap()
                .status,
            1
        );
    }
    api.write_status(&[first.channel_id, second.channel_id], false)
        .await
        .unwrap();
    for id in [first.channel_id, second.channel_id] {
        assert_eq!(
            api.get_channel(id)
                .await
                .unwrap()
                .observation()
                .unwrap()
                .status,
            2
        );
    }
    assert_ne!(
        api.check_native_recovery().await.unwrap(),
        NativeRecoveryCheck::Unknown
    );
    // 仅对本测试自己新建的库造分页 fixture；不批量发送 99 次管理创建请求。
    let mut connection = rusqlite::Connection::open(&database).unwrap();
    let tx = connection.transaction().unwrap();
    for index in 3..=101 {
        tx.execute("INSERT INTO channels (type,key,status,name,weight,created_time,base_url,models,\"group\",used_quota,priority,auto_ban,channel_info) SELECT type,key,status,?1,weight,created_time,base_url,models,\"group\",used_quota,priority,auto_ban,channel_info FROM channels WHERE id=?2", rusqlite::params![format!("fixture-{index}"),second.channel_id]).unwrap();
    }
    tx.commit().unwrap();
    drop(connection);
    let all = api.scan_channels(None).await.unwrap();
    assert!(all.complete);
    assert_eq!(all.total, 101);
    assert_eq!(all.channels.len(), 101);
    assert_eq!(
        all.channels.last().unwrap().observation().unwrap().name,
        "fixture-101"
    );
    drop(process); // 只终止本测试持有的 Child；临时库随后自动清理。
}
