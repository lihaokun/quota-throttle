use super::*;
use crate::config::ManageConfig;
use crate::newapi::transport::tests::{Fake, Reply};
use proptest::prelude::*;
use serde_json::json;

pub(crate) fn ready_fixture() -> BackendCheck {
    let at = utc_now().unwrap();
    ready(at, at).unwrap()
}

fn managed(url: &str) -> NewApiProcess {
    let mut cfg: ManageConfig = toml::from_str("").unwrap();
    cfg.port = Url::parse(url).unwrap().port().unwrap();
    let mut process = NewApiProcess::new(&cfg, url).unwrap();
    // HTTP fake 是本测试进程持有的真实监听器；不创建/读取生产 PID 文件。
    process.data_dir = std::env::current_dir().unwrap();
    process.binary = std::env::current_exe().unwrap();
    process
}
fn reply(path: &str, version: &str, recovery: Option<&str>) -> Reply {
    if path == "/api/status" {
        Reply::json(json!({"success":true,"data":{"version":version}}))
    } else if path == "/api/option/" {
        Reply::json(
            json!({"success":true,"data":recovery.map(|s|vec![json!({"key":"AutomaticEnableChannelEnabled","value":s})]).unwrap_or_default()}),
        )
    } else {
        Reply::json(json!({"success":true,"data":{"page":1,"page_size":1,"total":0,"items":[]}}))
    }
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn local_preflight_uses_real_process_and_readonly_management_budget() {
    let fake = Fake::start(|r| reply(&r.path, SUPPORTED_BACKEND_VERSION, Some("false"))).await;
    let client = NewApiClient::new(&fake.config(true)).unwrap();
    let process = managed(&fake.url);
    let result = client
        .probe_pid(&process, std::process::id(), utc_now().unwrap())
        .await
        .unwrap();
    assert_eq!(result.issue(), None);
    assert!(result.readiness().ready_at(utc_now().unwrap()));
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|c| c.method == "GET"));
    assert!(calls[2].at.duration_since(calls[0].at) >= Duration::from_millis(900));
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn local_preflight_rejects_version_recovery_and_capability_conflicts_without_writes() {
    for (version, recovery, issue) in [
        (
            "future-version",
            Some("false"),
            BackendIssue::UnsupportedVersion,
        ),
        ("v0.0.0", Some("false"), BackendIssue::UnsupportedVersion),
        (
            SUPPORTED_BACKEND_VERSION,
            Some("true"),
            BackendIssue::RecoveryEnabled,
        ),
        (
            SUPPORTED_BACKEND_VERSION,
            None,
            BackendIssue::RecoveryUnknown,
        ),
        (
            SUPPORTED_BACKEND_VERSION,
            Some("FALSE"),
            BackendIssue::RecoveryUnknown,
        ),
    ] {
        let fake = Fake::start(move |r| reply(&r.path, version, recovery)).await;
        let client = NewApiClient::new(&fake.config(true)).unwrap();
        let process = managed(&fake.url);
        assert_eq!(
            client
                .probe_pid(&process, std::process::id(), utc_now().unwrap())
                .await
                .err()
                .unwrap(),
            issue
        );
        assert!(fake.calls.lock().unwrap().iter().all(|c| c.method == "GET"));
    }
    let fake = Fake::start(|r| {
        if r.path.starts_with("/api/channel/") {
            Reply::json(json!({"success":true,"data":{"total":1,"items":[]}}))
        } else {
            reply(&r.path, SUPPORTED_BACKEND_VERSION, Some("false"))
        }
    })
    .await;
    let client = NewApiClient::new(&fake.config(true)).unwrap();
    let process = managed(&fake.url);
    assert_eq!(
        client
            .probe_pid(&process, std::process::id(), utc_now().unwrap())
            .await
            .err()
            .unwrap(),
        BackendIssue::Management(ManagementErrorKind::InvalidResponse)
    );
}

#[tokio::test]
async fn local_preflight_missing_or_mismatched_binding_never_queries_server() {
    let fake = Fake::start(|r| reply(&r.path, SUPPORTED_BACKEND_VERSION, Some("false"))).await;
    let client = NewApiClient::new(&fake.config(true)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut process = managed(&fake.url);
    process.data_dir = dir.path().into();
    let result = client.check_local_backend(&process).await;
    assert_eq!(result.issue(), Some(BackendIssue::LocalBinding));
    assert!(!result.readiness().ready_at(utc_now().unwrap()));
    std::fs::write(
        dir.path().join("new-api.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    assert_eq!(
        client.check_local_backend(&process).await.issue(),
        Some(BackendIssue::LocalBinding)
    ); // cwd 不符。
    process = managed(&fake.url);
    process.binary = dir.path().join("not-the-running-exe");
    assert_eq!(
        client
            .probe_pid(&process, std::process::id(), utc_now().unwrap())
            .await
            .err()
            .unwrap(),
        BackendIssue::LocalBinding
    );
    process = managed(&fake.url);
    process.port = process.port.saturating_sub(1);
    assert_eq!(
        client
            .probe_pid(&process, std::process::id(), utc_now().unwrap())
            .await
            .err()
            .unwrap(),
        BackendIssue::LocalBinding
    );
    process = managed(&fake.url);
    process.base_url = "http://example.invalid:3000".into();
    assert_eq!(
        client.check_local_backend(&process).await.issue(),
        Some(BackendIssue::LocalBinding)
    );
    assert!(fake.calls.lock().unwrap().is_empty());
}

#[test]
fn local_preflight_private_report_does_not_expose_process_paths() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(BackendCheck, std::fmt::Debug);
    assert_not_impl!(BackendCheck, serde::Serialize);
    assert_not_impl!(ProcessSnapshot, std::fmt::Debug);
    assert_not_impl!(ProcessSnapshot, serde::Serialize);
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn local_preflight_rechecks_local_binding_after_management_reads() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let alias = dir.path().join("alias");
    let other = dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    symlink(std::env::current_dir().unwrap(), &alias).unwrap();
    let link = alias.clone();
    let fake = Fake::start(move |r| {
        if r.path.starts_with("/api/channel/") {
            std::fs::remove_file(&link).unwrap();
            symlink(&other, &link).unwrap();
        }
        reply(&r.path, SUPPORTED_BACKEND_VERSION, Some("false"))
    })
    .await;
    let client = NewApiClient::new(&fake.config(true)).unwrap();
    let mut process = managed(&fake.url);
    process.data_dir = alias;
    assert_eq!(
        client
            .probe_pid(&process, std::process::id(), utc_now().unwrap())
            .await
            .err()
            .unwrap(),
        BackendIssue::LocalBinding
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
}

#[test]
fn local_preflight_bounds_pid_and_stat_files_and_lease_edges() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid");
    std::fs::write(&path, "9".repeat(33)).unwrap();
    assert!(read_small(&path, 32).is_err());
    assert!(read_small(dir.path(), 32).is_err());
    for pid in ["", "0", "+12", "12\n13", "4294967296", "bad"] {
        assert!(parse_pid(pid).is_err());
    }
    for at in [JSON_SAFE, JSON_SAFE - 59, -1] {
        assert!(ready(at, at).is_err());
    }
    assert!(ready(JSON_SAFE - 60, JSON_SAFE - 1).is_ok());
    assert!(process_start("1 (fixture) Z 0", 1).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn local_preflight_pbt_pid_process_owner_and_expiring_lease(pid in 1u32..100000, ticks in 1u64..1_000_000_000, label in "[a-z ()]{1,20}",at in 0i64..1_000_000_000,delta in -2i64..65) {
        prop_assert_eq!(parse_pid(&format!("{pid}\n")).unwrap(),pid);
        let mut fields=vec!["S".to_string()];fields.extend((0..18).map(|_|"0".to_string()));fields.push(ticks.to_string());
        let stat=format!("{pid} ({label}) {}",fields.join(" "));
        prop_assert_eq!(process_start(&stat,pid).unwrap(),ticks);
        prop_assert!(process_start(&stat,pid+1).is_err());
        let listener=format!("LISTEN 0 4096 *:1234 *:* users:((\"fixture\",pid={pid},fd=9))");
        prop_assert!(owns_listener(&listener,pid));prop_assert!(!owns_listener(&listener,pid+1));
        let duplicate=format!("{listener}\n{listener}");prop_assert!(!owns_listener(&duplicate,pid));
        let report=ready(at,at+delta);
        prop_assert_eq!(report.is_ok(),(0..60).contains(&delta));
        if let Ok(report)=report {prop_assert!(report.readiness().ready_at(at+delta));prop_assert!(!report.readiness().ready_at(at+60));}
        prop_assert!(!unavailable().ready_at(at));
    }
    #[test]
    fn local_preflight_pbt_page_and_local_target_never_default_unknown_to_ready(total in 0i64..10000,valid in any::<bool>(),id in 1i64..10000) {
        let items=if total==0 {vec![]}else{vec![json!({"id":id})]};
        let mut value=json!({"success":true,"data":{"page":1,"page_size":1,"total":total,"items":items}});
        if !valid {value["data"]["page_size"]=json!(0);}
        prop_assert_eq!(page_capability(&value),valid);
        for host in ["127.0.0.1","localhost","[::1]"] {let url=Url::parse(&format!("http://{host}:1234")).unwrap();prop_assert!(endpoint_is_loopback(&url));}
        for host in ["localhost.example.invalid","example.invalid","192.0.2.1"] {let url=Url::parse(&format!("http://{host}:1234")).unwrap();prop_assert!(!endpoint_is_loopback(&url));}
    }
}
