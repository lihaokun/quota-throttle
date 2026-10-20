use super::*;
use proptest::prelude::*;

#[test]
fn log_capability_startup_evidence_is_bounded_exact_and_private() {
    assert!(validate_environment(b"SQL_DSN=local\0LOG_SQL_DSN=local\0UNRELATED=secret\0").is_ok());
    for raw in [
        b"".as_slice(),
        b"SQL_DSN=local\0",
        b"SQL_DSN=\0LOG_SQL_DSN=local\0",
        b"SQL_DSN=local\0LOG_SQL_DSN=local",
        b"SQL_DSN=local\0LOG_SQL_DSN=local\0SQL_DSN=local\0",
        b"SQL_DSN=local-file\0LOG_SQL_DSN=local\0",
        b"SQL_DSN=local\0LOG_SQL_DSN=postgres://secret\0",
    ] {
        assert_eq!(
            validate_environment(raw).err(),
            Some(LogCapabilityIssue::SqliteUnverified)
        );
    }
    let mut max = b"SQL_DSN=local\0LOG_SQL_DSN=local\0X=".to_vec();
    max.resize(ENV_LIMIT - 1, b'x');
    max.push(0);
    assert!(validate_environment(&max).is_ok());
    max.push(0);
    assert_eq!(
        validate_environment(&max).err(),
        Some(LogCapabilityIssue::SqliteUnverified)
    );
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(LocalLogLease, std::fmt::Debug);
    assert_not_impl!(LocalLogLease, serde::Serialize);
    assert_not_impl!(LocalLogLease, serde::Deserialize<'static>);
}
#[tokio::test(start_paused = true)]
async fn log_capability_lease_cannot_survive_monotonic_expiry_or_backwards_wall_clock() {
    let lease = LocalLogLease::test_fixture();
    assert!(lease.check_time().is_ok());
    assert_eq!(
        lease.check_time_at(lease.started - 1, Instant::now()).err(),
        Some(LogCapabilityIssue::Expired)
    );
    assert_eq!(
        lease.check_time_at(lease.expires, Instant::now()).err(),
        Some(LogCapabilityIssue::Expired)
    );
    tokio::time::advance(std::time::Duration::from_secs(59)).await;
    assert!(lease.check_time().is_ok());
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    assert_eq!(lease.check_time().err(), Some(LogCapabilityIssue::Expired));
}
#[tokio::test]
async fn log_capability_missing_local_binding_performs_no_management_io() {
    use crate::newapi::transport::tests::{Fake, Reply};
    let fake = Fake::start(|_| Reply::json(serde_json::json!({"success":true}))).await;
    let mut cfg: crate::config::ManageConfig = toml::from_str("").unwrap();
    let dir = tempfile::tempdir().unwrap();
    cfg.data_dir = dir.path().to_string_lossy().into();
    cfg.port = reqwest::Url::parse(&fake.url).unwrap().port().unwrap();
    let process = NewApiProcess::new(&cfg, &fake.url).unwrap();
    let client = NewApiClient::new(&fake.config(true)).unwrap();
    assert_eq!(
        client.check_local_logs(&process).await.err(),
        Some(LogCapabilityIssue::LocalBinding)
    );
    assert_eq!(
        client
            .recheck_local_logs(&process, &LocalLogLease::test_fixture())
            .await
            .err(),
        Some(LogCapabilityIssue::LocalBinding)
    );
    assert!(fake.calls.lock().unwrap().is_empty());
    assert_eq!(
        sqlite_startup_evidence(u32::MAX).await.err(),
        Some(LogCapabilityIssue::SqliteUnverified)
    );
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb6_log_startup_evidence_never_defaults_missing_or_ambiguous_to_sqlite(mode in 0u8..7, suffix in prop::collection::vec(any::<u8>(),0..100), reverse in any::<bool>()) {
        let mut entries=vec![b"SQL_DSN=local".to_vec(),b"LOG_SQL_DSN=local".to_vec()];
        match mode {0=>{},1=>{entries.pop();},2=>entries[0]=b"SQL_DSN=".to_vec(),3=>entries.push(entries[1].clone()),4=>entries[1]=b"LOG_SQL_DSN=mysql-secret".to_vec(),5=>entries[0]=b"XSQL_DSN=local".to_vec(),_=>entries[1]=b"LOG_SQL_DSN=LOCAL".to_vec()}
        let mut unrelated=b"OTHER=".to_vec();unrelated.extend(suffix.into_iter().filter(|b|*b!=0));entries.push(unrelated);
        if reverse {entries.reverse();}
        let mut raw=Vec::new();for entry in entries{raw.extend(entry);raw.push(0);}
        prop_assert_eq!(validate_environment(&raw).is_ok(),mode==0);
        if let Err(issue)=validate_environment(&raw){prop_assert_eq!(format!("{issue:?}"),"SqliteUnverified");}
    }
}
