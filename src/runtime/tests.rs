use super::*;
use proptest::prelude::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

const SAMPLE: &str = r#"
poll_interval_secs=10
status_addr="127.0.0.1:39191"
keys=[]
[zhipu]
[new_api]
base_url="http://127.0.0.1:39190"
root_username="qtadmin"
root_password="SYNTHETIC_ADMIN"
[new_api.manage]
version="v1.0.0-rc.20"
port=39190
data_dir="/tmp/qt-runtime-native"
[source_groups]
catalog_path="/tmp/qt-runtime-state/catalog.sqlite3"
public_api_base_url="http://127.0.0.1:39190"
[source_groups.issuer]
user_id=2
username="qtsigner"
password="SYNTHETIC_ISSUER"
"#;

#[test]
fn r1_valid_config_and_legacy_configuration_stays_independent() {
    let loaded = config::decode(SAMPLE).unwrap();
    assert!(loaded.policy.keys.is_empty());
    assert_eq!(loaded.addr.port(), 39191);
    assert_eq!(loaded.options.issuer.unwrap().user_id, 2);
    let legacy: crate::config::Config =
        toml::from_str(SAMPLE.split("[source_groups]").next().unwrap()).unwrap();
    assert_eq!(legacy.poll_interval_secs, 10);
    assert!(config::decode(SAMPLE.split("[source_groups]").next().unwrap()).is_err());
    assert!(config::decode(&SAMPLE.replace(
        "[source_groups.issuer]",
        "legacy_admin_user_id=2\n[source_groups.issuer]"
    ))
    .is_err());
}

#[test]
fn r1_bad_configuration_is_rejected_without_secret_echo() {
    for (from, to) in [
        ("poll_interval_secs=10", "poll_interval_secs=0"),
        ("127.0.0.1:39191", "0.0.0.0:39191"),
        ("127.0.0.1:39191", "127.0.0.1:39190"),
        (
            "/tmp/qt-runtime-state/catalog.sqlite3",
            "/tmp/qt-runtime-state/one-api.db",
        ),
        (
            "/tmp/qt-runtime-state/catalog.sqlite3",
            "/tmp/../catalog.sqlite3",
        ),
        ("http://127.0.0.1:39190", "http://example.test:39190"),
        ("v1.0.0-rc.20", "unverified-version"),
        ("user_id=2", "user_id=0"),
        ("username=\"qtsigner\"", "username=\"qtadmin\""),
        (
            "[source_groups]\n",
            "[source_groups]\nunknown_private=\"SYNTHETIC_SECRET\"\n",
        ),
        (
            "keys=[]",
            "keys=[{name=\"old\",zhipu_api_key=\"SYNTHETIC_SECRET\"}]",
        ),
        ("SYNTHETIC_ADMIN", "BROKEN_SECRET\" \""),
    ] {
        let err = config::decode(&SAMPLE.replace(from, to)).err().unwrap();
        assert_eq!(err, RuntimeError::Config);
        assert_eq!(format!("{err}"), "source-groups runtime: Config");
    }
    assert!(config::decode(&SAMPLE.replace(
        "public_api_base_url=\"http://127.0.0.1:39190\"",
        "public_api_base_url=\"http://127.0.0.1:39190/v1\""
    ))
    .is_err());
}

#[test]
fn r1_load_requires_private_regular_bounded_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, SAMPLE).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(config::load(&path).is_ok());
    let link = dir.path().join("link.toml");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(config::load(&link).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    assert!(config::load(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&path, " ".repeat(1024 * 1024 + 1)).unwrap();
    assert!(config::load(&path).is_err());
}

#[test]
fn r2_private_directory_never_chmods_existing_path_or_follows_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("new");
    config::private_directory(&target).unwrap();
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let link = dir.path().join("linked");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(config::private_directory(&link).is_err());
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(config::private_directory(&target).is_err());
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[tokio::test]
async fn r3_startup_stop_joins_tasks_even_when_startup_already_completed() {
    let (stop, shutdown) = watch::channel(false);
    let completed = Arc::new(AtomicUsize::new(0));
    let mut tasks = JoinSet::new();
    for _ in 0..5 {
        let mut rx = shutdown.clone();
        let completed = completed.clone();
        tasks.spawn(async move {
            stopped(&mut rx).await;
            completed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
    }
    let result = supervise_startup(
        std::future::ready(Ok(Some(tasks))),
        stop,
        std::future::ready(Ok(())),
    )
    .await;
    assert_eq!(result, Ok(()));
    assert_eq!(completed.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn r3_background_panic_stops_and_joins_remaining_tasks() {
    let (stop, mut shutdown) = watch::channel(false);
    let completed = Arc::new(AtomicUsize::new(0));
    let done = completed.clone();
    let mut tasks = JoinSet::new();
    tasks.spawn(async {
        panic!("synthetic background panic");
    });
    tasks.spawn(async move {
        stopped(&mut shutdown).await;
        done.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    assert_eq!(
        supervise(tasks, stop, std::future::pending()).await,
        Err(RuntimeError::Background)
    );
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn r3_owned_job_keeps_blocking_handle_until_synchronous_work_exits() {
    let (stop, shutdown) = watch::channel(false);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let release = gate.clone();
    let finished = Arc::new(AtomicUsize::new(0));
    let done = finished.clone();
    let task = tokio::spawn(owned_job(shutdown, async move {
        entered.send(()).unwrap();
        let (lock, signal) = &*gate;
        let mut opened = lock.lock().unwrap();
        while !*opened {
            opened = signal.wait(opened).unwrap();
        }
        done.fetch_add(1, Ordering::SeqCst);
    }));
    ready.await.unwrap();
    stop.send_replace(true);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    let early = task.is_finished();
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    task.await.unwrap().unwrap();
    assert!(!early, "stop must not release the actual blocking work");
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn r2_stop_during_real_health_wait_never_spawns_native_process() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(![3000, 3001].contains(&port));
    let (entered, wait_entered) = tokio::sync::oneshot::channel();
    let (release, wait_release) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        assert!(stream.read(&mut buf).await.unwrap() > 0);
        entered.send(()).unwrap();
        let _ = wait_release.await;
    });
    let dir = tempfile::tempdir().unwrap();
    let mut cfg: crate::config::ManageConfig = toml::from_str("").unwrap();
    cfg.port = port;
    cfg.data_dir = dir.path().to_string_lossy().into_owned();
    let process = NewApiProcess::new(&cfg, &format!("http://127.0.0.1:{port}")).unwrap();
    std::fs::write(&process.binary, "this fixture must never be executed").unwrap();
    let (stop, shutdown) = watch::channel(false);
    let boot_stop = shutdown.clone();
    let boot = tokio::spawn(owned_job(shutdown, async move {
        process.ensure_group_running(&boot_stop).await
    }));
    wait_entered.await.unwrap();
    stop.send_replace(true);
    assert!(boot.await.unwrap().unwrap().is_none());
    assert!(!dir.path().join("new-api.pid").exists());
    assert!(!dir.path().join("new-api.log").exists());
    release.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn r4_r5_stopped_job_never_starts_and_delay_has_no_catchup_ticks() {
    let (_, mut shutdown) = watch::channel(true);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    assert_eq!(
        owned_job(shutdown.clone(), async move {
            counter.fetch_add(1, Ordering::SeqCst);
            7
        })
        .await
        .unwrap(),
        None
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!delay(&mut shutdown, MAINTENANCE_INTERVAL).await);
    assert_eq!(MAINTENANCE_INTERVAL, Duration::from_secs(5));
    assert_eq!(BACKEND_INTERVAL, Duration::from_secs(30));
    assert_eq!(
        control::<()>(Err(crate::catalog::control::ControlError {
            code: ControlErrorCode::Storage
        })),
        Err(RuntimeError::Storage)
    );
}

#[tokio::test]
async fn r5_fresh_ledger_waits_for_collection_instead_of_terminating_service() {
    use crate::catalog::ledger::LedgerError;
    use crate::ledger::analytics::AnalyticsError;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let cfg = config::decode(SAMPLE).unwrap().policy;
    let client = Arc::new(NewApiClient::new(&cfg.new_api).unwrap());
    let worker = ReconcileWorker::new(store, client, cfg).unwrap();
    let result = worker.repair_attribution_once().await;
    assert!(matches!(
        &result,
        Err(AnalyticsError::Ledger(LedgerError::NotInitialized))
    ));
    assert_eq!(repair_outcome(result), Ok(()));
    assert_eq!(
        repair_outcome::<()>(Err(AnalyticsError::Ledger(LedgerError::InvalidCheckpoint))),
        Err(RuntimeError::Storage)
    );
    assert_eq!(
        repair_outcome::<()>(Err(AnalyticsError::Control(ControlErrorCode::Storage))),
        Err(RuntimeError::Storage)
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn r1_pbt_configuration_admission_matches_independent_bounds(poll in 0u64..5000, port in 0u16..65535, public in any::<bool>(), legacy in any::<bool>()) {
        let text = SAMPLE.replace("poll_interval_secs=10", &format!("poll_interval_secs={poll}"))
            .replace("127.0.0.1:39191", &format!("{}:{port}", if public {"0.0.0.0"} else {"127.0.0.1"}))
            .replace("keys=[]", if legacy {"keys=[{name=\"old\",zhipu_api_key=\"synthetic\"}]"} else {"keys=[]"});
        let expected = poll >= 5 && poll <= 3600 && port != 0 && port != 39190 && !public && !legacy;
        prop_assert_eq!(config::decode(&text).is_ok(), expected);
    }
    #[test]
    fn r3_pbt_supervision_joins_every_task_for_any_first_completion(count in 1usize..8, first in 0usize..8, external in any::<bool>(), failure in any::<bool>()) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (stop, shutdown) = watch::channel(false);
            let completed = Arc::new(AtomicUsize::new(0));
            let mut tasks = JoinSet::new();
            let mut gates = Vec::new();
            for _ in 0..count {
                let (send, mut receive) = tokio::sync::oneshot::channel(); gates.push(Some(send));
                let (done, mut rx) = (completed.clone(), shutdown.clone());
                tasks.spawn(async move {
                    let result = tokio::select! { biased; _=stopped(&mut rx)=>Ok(()), value=&mut receive=>value.unwrap() };
                    done.fetch_add(1, Ordering::SeqCst); result
                });
            }
            let (send, receive) = tokio::sync::oneshot::channel();
            if external { send.send(()).unwrap(); }
            else { gates[first % count].take().unwrap().send(if failure {Err(RuntimeError::Storage)} else {Ok(())}).unwrap(); }
            let signal = async { let _ = receive.await; Ok(()) };
            let result = tokio::time::timeout(Duration::from_secs(2), supervise(tasks, stop, signal)).await.unwrap();
            assert_eq!(completed.load(Ordering::SeqCst), count);
            assert_eq!(result, if external {Ok(())} else if failure {Err(RuntimeError::Storage)} else {Err(RuntimeError::Background)});
        });
    }
}
