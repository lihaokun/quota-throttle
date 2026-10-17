use super::*;
use crate::catalog::{build_legacy_import, LegacyChannel};
use crate::config::{Config, HeaderKV, KeyMapping, ModelDiscoveryAuth, ModelDiscoveryConfig};
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::os::unix::fs::{symlink, PermissionsExt};
use tempfile::TempDir;

#[path = "control_tests.rs"]
mod control;
#[path = "execution_tests.rs"]
mod execution;
#[path = "glm_round_tests.rs"]
mod glm_round;
#[path = "ledger_tests.rs"]
mod ledger;
#[path = "observed_publication_tests.rs"]
mod observed_publication;
#[path = "policy_tests.rs"]
mod policy;
#[path = "policy_command_tests.rs"]
mod policy_commands;

const DROP_LEDGER_V5: &str = "DROP TABLE ledger_minute_usage; DROP TABLE ledger_facts; DROP TABLE ledger_checkpoints; DROP TABLE ledger_state;";
const DROP_OBSERVED_V6: &str = "DROP TABLE IF EXISTS volcano_observation_queue; DROP TABLE IF EXISTS volcano_request_observations; DROP TABLE IF EXISTS volcano_period_history; DROP INDEX IF EXISTS ledger_cell_source_time; DROP INDEX IF EXISTS ledger_fact_source_usage; DROP INDEX IF EXISTS ledger_fact_source_request;";
fn drop_ledger_for_legacy(conn: &Connection) {
    replace_control_operations(conn, SCHEMA_V2).unwrap();
    conn.execute_batch(DROP_OBSERVED_V6).unwrap();
    conn.execute_batch(DROP_LEDGER_V5).unwrap();
}

fn setup() -> (TempDir, CatalogStore) {
    let dir = private_dir();
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    (dir, store)
}
fn private_dir() -> TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn source(key: &str, kind: SourceKind) -> SourceDraft {
    let upstream = UpstreamProfile {
        channel_type: 8,
        base_url: "https://example.test/chat/completions".into(),
        models: vec!["glm-example".into()],
        model_discovery: Some(ModelDiscoveryConfig {
            url: "https://example.test/models".into(),
            auth: ModelDiscoveryAuth::AuthorizationRaw,
        }),
    };
    let settings = match kind {
        SourceKind::Glm => SourceSettings::Glm {
            upstream,
            quota: GlmQuotaProfile {
                quota_url: "https://example.test/quota?type=2".into(),
                global_headers: vec![],
                key_headers: vec![],
            },
        },
        SourceKind::Volcano => SourceSettings::Volcano {
            upstream,
            policy: VolcanoPolicy::default(),
        },
    };
    SourceDraft {
        name: "示例来源".into(),
        note: String::new(),
        settings,
        credential: SecretString::new(key.to_owned()).unwrap(),
    }
}
fn group(name: &str, kind: SourceKind) -> GroupDraft {
    GroupDraft {
        name: name.into(),
        newapi_group: name.into(),
        kind,
    }
}
fn legacy(n: usize) -> (Config, Vec<LegacyChannel>) {
    let mut cfg: Config =
        toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\n").unwrap();
    cfg.zhipu.extra_headers = vec![HeaderKV {
        key: "X-Global".into(),
        value: "global-sentinel".into(),
    }];
    let mut channels = Vec::new();
    for i in 0..n {
        cfg.keys.push(KeyMapping {
            name: format!("k{i}"),
            zhipu_api_key: format!("fake-private-{i}"),
            channel_id: None,
            note: format!("note{i}"),
            quota_headers: vec![HeaderKV {
                key: "X-Key".into(),
                value: format!("selector-{i}"),
            }],
        });
        channels.push(LegacyChannel {
            id: 100 + i as i64,
            name: format!("k{i}"),
            group: "default".into(),
            channel_type: 8,
            base_url: "https://example.test/chat/completions".into(),
            models_csv: " m1,m2,m1 ".into(),
            priority: if i == 0 { 100 } else { 10 },
        });
    }
    (cfg, channels)
}
fn expect<T>(result: Result<T>, kind: ErrorKind) {
    match result {
        Err(e) => assert_eq!(e.kind, kind),
        Ok(_) => panic!("expected {kind:?}"),
    }
}

// 只从原输入构建预期，沿实际成员引用读取材料，避免错配被数组下标掩盖。
fn assert_legacy_fidelity(
    store: &mut CatalogStore,
    cfg: &Config,
    channels: &[LegacyChannel],
    receipt: &ImportReceipt,
) -> CatalogSnapshot {
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.sources.len(), cfg.keys.len());
    assert_eq!(snapshot.memberships.len(), cfg.keys.len());
    assert_eq!(receipt.source_ids.len(), cfg.keys.len());
    assert_eq!(receipt.membership_ids.len(), cfg.keys.len());
    assert_eq!(snapshot.groups.len(), 1);
    assert_eq!(snapshot.groups[0].group_id, receipt.group_id);
    for (position, key) in cfg.keys.iter().enumerate() {
        let matches: Vec<_> = channels
            .iter()
            .filter(|c| match key.channel_id {
                Some(id) => c.id == id,
                None => c.name == key.name,
            })
            .collect();
        assert_eq!(matches.len(), 1);
        let channel = matches[0];
        let member = snapshot
            .memberships
            .iter()
            .find(|m| m.channel_id.map(ChannelId::get) == Some(channel.id))
            .unwrap();
        assert_eq!(member.membership_id, receipt.membership_ids[position]);
        assert_eq!(member.source_id, receipt.source_ids[position]);
        assert_eq!(member.group_id, receipt.group_id);
        assert_eq!(snapshot.groups[0].newapi_group, channel.group);
        assert_eq!(member.order, position as i64);
        assert_eq!(member.desired_priority, channel.priority);
        assert_eq!(member.lifecycle, MembershipLifecycle::Imported);
        assert_eq!(
            (
                member.observed_status,
                member.observed_priority,
                member.applied_revision
            ),
            (None, None, None)
        );
        let stored = snapshot
            .sources
            .iter()
            .find(|s| s.source_id == member.source_id)
            .unwrap();
        assert_eq!(stored.name, key.name);
        assert_eq!(stored.note, key.note);
        assert_eq!(stored.kind, SourceKind::Glm);
        let material = store.read_source_material(member.source_id).unwrap();
        assert_eq!(material.credential.expose(), key.zhipu_api_key);
        let SourceSettings::Glm { upstream, quota } = material.settings else {
            panic!("import must preserve GLM type")
        };
        let mut expected_models = Vec::new();
        for raw in channel.models_csv.split(',') {
            let id = raw.trim();
            if !id.is_empty() && !expected_models.contains(&id) {
                expected_models.push(id);
            }
        }
        assert_eq!(upstream.models, expected_models);
        assert_eq!(upstream.channel_type, channel.channel_type);
        assert_eq!(upstream.base_url, channel.base_url);
        assert_eq!(
            upstream.model_discovery,
            cfg.new_api
                .channel_template
                .as_ref()
                .and_then(|t| t.model_discovery.clone())
        );
        assert_eq!(quota.quota_url, cfg.zhipu.quota_url);
        let pairs = |headers: &[HeaderKV]| {
            headers
                .iter()
                .map(|h| (h.key.clone(), h.value.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            pairs(&quota.global_headers),
            pairs(&cfg.zhipu.extra_headers)
        );
        assert_eq!(pairs(&quota.key_headers), pairs(&key.quota_headers));
    }
    snapshot
}

#[test]
fn regression_duplicates_at_capacity() {
    let (_dir, mut store) = setup();
    let (cfg, channels) = legacy(MAX_SOURCES);
    store
        .import_legacy(&build_legacy_import(&cfg, &channels).unwrap())
        .unwrap();
    for i in 1..MAX_GROUPS {
        store
            .create_group(group(&format!("g{i}"), SourceKind::Glm))
            .unwrap();
    }
    store.conn.execute("INSERT INTO memberships(source_id,group_id,kind,position,lifecycle,desired_priority) SELECT s.source_id,g.group_id,'glm',s.source_id-1,'planned',10 FROM sources s CROSS JOIN source_groups g WHERE g.group_id BETWEEN 2 AND 10",[]).unwrap();
    let before = store.snapshot().unwrap();
    assert_eq!(before.memberships.len(), MAX_MEMBERSHIPS);
    let duplicate = store
        .create_source(source(&cfg.keys[0].zhipu_api_key, SourceKind::Glm))
        .unwrap_err();
    assert_eq!(duplicate.kind, ErrorKind::DuplicateSource);
    assert_eq!(duplicate.source_id, Some(before.sources[0].source_id));
    expect(
        store.create_group(group("default", SourceKind::Volcano)),
        ErrorKind::DuplicateGroup,
    );
    expect(
        store.plan_membership(before.sources[0].source_id, before.groups[0].group_id, 10),
        ErrorKind::DuplicateMembership,
    );
    expect(
        store.create_source(source("new-over-capacity", SourceKind::Glm)),
        ErrorKind::InvalidInput,
    );
    expect(
        store.create_group(group("new-over-capacity", SourceKind::Glm)),
        ErrorKind::InvalidInput,
    );
    expect(
        store.plan_membership(before.sources[0].source_id, before.groups[10].group_id, 10),
        ErrorKind::InvalidInput,
    );
    assert_eq!(store.snapshot().unwrap(), before);
    store
        .conn
        .execute(
            "UPDATE sources SET retired_at=1 WHERE source_id=?1",
            [before.sources[0].source_id.get()],
        )
        .unwrap();
    let retired = store.snapshot().unwrap();
    let duplicate = store
        .create_source(source(&cfg.keys[0].zhipu_api_key, SourceKind::Glm))
        .unwrap_err();
    assert_eq!(duplicate.kind, ErrorKind::DuplicateSource);
    assert_eq!(duplicate.source_id, Some(before.sources[0].source_id));
    assert_eq!(store.snapshot().unwrap(), retired);
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM source_secrets", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        MAX_SOURCES as i64
    );
}

#[test]
fn regression_import_keeps_per_source_mapping_and_order() {
    let (_dir, mut store) = setup();
    let (mut cfg, mut channels) = legacy(3);
    cfg.keys.rotate_right(1);
    cfg.keys[1].channel_id = Some(100);
    for channel in &mut channels {
        channel.models_csv = format!(" model-{}, common, model-{} ", channel.id, channel.id);
        channel.priority = (channel.id - 100) * 10 - 5;
    }
    channels.reverse();
    let plan = build_legacy_import(&cfg, &channels).unwrap();
    let ImportOutcome::Applied(receipt) = store.import_legacy(&plan).unwrap() else {
        panic!()
    };
    assert_legacy_fidelity(&mut store, &cfg, &channels, &receipt);
}

#[test]
fn regression_secret_debug_and_display_are_redacted() {
    let secret = SecretString::new("known-fake-secret".into()).unwrap();
    assert_eq!(format!("{secret:?}"), "[REDACTED]");
    assert_eq!(secret.to_string(), "[REDACTED]");
}

#[test]
fn source_shared_across_groups_and_types_isolated() {
    let (_dir, mut store) = setup();
    let s = store
        .create_source(source("fake-s", SourceKind::Volcano))
        .unwrap();
    let a = store
        .create_group(group("vol-a", SourceKind::Volcano))
        .unwrap();
    let b = store
        .create_group(group("vol-b", SourceKind::Volcano))
        .unwrap();
    let g = store.create_group(group("glm", SourceKind::Glm)).unwrap();
    store.plan_membership(s.source_id, a.group_id, 100).unwrap();
    store.plan_membership(s.source_id, b.group_id, 10).unwrap();
    let before = store.snapshot().unwrap();
    expect(
        store.plan_membership(s.source_id, g.group_id, 10),
        ErrorKind::TypeMismatch,
    );
    expect(
        store.plan_membership(s.source_id, a.group_id, 99),
        ErrorKind::DuplicateMembership,
    );
    assert_eq!(before, store.snapshot().unwrap());
    assert_eq!(before.sources.len(), 1);
    assert_eq!(before.memberships.len(), 2);
    assert_eq!(before.memberships[0].desired_priority, 100);
    assert_eq!(before.memberships[1].desired_priority, 10);
    assert!(before.groups.iter().all(|g| !g.ready));
    assert!(before.memberships.iter().all(|m| m.channel_id.is_none()));
    let duplicate = store
        .create_source(source("fake-s", SourceKind::Glm))
        .unwrap_err();
    assert_eq!(duplicate.source_id, Some(s.source_id));
}

#[test]
fn invalid_input_never_partially_writes() {
    let (_dir, mut store) = setup();
    for name in ["auto", "two,groups", " leading", "", "带空格"] {
        expect(
            store.create_group(group(name, SourceKind::Glm)),
            ErrorKind::InvalidInput,
        );
    }
    let mut bad = source("s", SourceKind::Volcano);
    if let SourceSettings::Volcano { policy, .. } = &mut bad.settings {
        policy.daily_token_limit = Some(0);
    }
    expect(store.create_source(bad), ErrorKind::InvalidInput);
    let mut bad = source("s", SourceKind::Glm);
    if let SourceSettings::Glm { upstream, .. } = &mut bad.settings {
        upstream.base_url = "https://user:fake-secret@example.test/x".into();
    }
    let err = store.create_source(bad).unwrap_err();
    assert!(!format!("{err:?} {err}").contains("fake-secret"));
    let mut bad = source("s", SourceKind::Glm);
    if let SourceSettings::Glm { quota, .. } = &mut bad.settings {
        quota.key_headers = vec![HeaderKV {
            key: "X-Test".into(),
            value: "oops\r\nsecret".into(),
        }];
    }
    expect(store.create_source(bad), ErrorKind::InvalidInput);
    assert!(store.snapshot().unwrap().sources.is_empty());
    for bad in ["", " leading", "ending ", "line\nbreak", "nul\0key"] {
        expect(SecretString::new(bad.into()), ErrorKind::InvalidInput);
    }
    assert!(serde_json::from_str::<SourceId>("1").is_err());
    for bad in ["0", "01", "-1", "9223372036854775808", " 1"] {
        assert!(serde_json::from_value::<SourceId>(serde_json::json!(bad)).is_err());
    }
    let id = SourceId::new(i64::MAX).unwrap();
    assert_eq!(
        serde_json::from_str::<SourceId>(&serde_json::to_string(&id).unwrap()).unwrap(),
        id
    );
}

#[test]
fn legacy_replay_does_not_resurrect_modified_records() {
    let (dir, mut store) = setup();
    let (cfg, channels) = legacy(3);
    let plan = build_legacy_import(&cfg, &channels).unwrap();
    let ImportOutcome::Applied(receipt) = store.import_legacy(&plan).unwrap() else {
        panic!()
    };
    let initial = store.snapshot().unwrap();
    assert_eq!(
        initial
            .memberships
            .iter()
            .map(|m| m.channel_id.unwrap().get())
            .collect::<Vec<_>>(),
        vec![100, 101, 102]
    );
    assert!(initial
        .memberships
        .iter()
        .all(|m| m.lifecycle == MembershipLifecycle::Imported && m.applied_revision.is_none()));
    // 只在测试内部模拟未来状态机操作，不增加生产 setter。
    store
        .conn
        .execute(
            "UPDATE sources SET manual_paused=1,retired_at=10,name='changed' WHERE source_id=?1",
            [receipt.source_ids[0].get()],
        )
        .unwrap();
    let changed = store.snapshot().unwrap();
    drop(store);
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(
        store.import_legacy(&plan).unwrap(),
        ImportOutcome::AlreadyApplied(receipt)
    );
    assert_eq!(store.snapshot().unwrap(), changed);
    let (mut different, channels) = legacy(3);
    different.keys[0].note = "different".into();
    expect(
        store.import_legacy(&build_legacy_import(&different, &channels).unwrap()),
        ErrorKind::ImportConflict,
    );
    expect(
        store.read_source_material(changed.sources[0].source_id),
        ErrorKind::Retired,
    );
}

#[test]
fn legacy_rejects_missing_ambiguous_duplicate_or_mixed_channels() {
    let (mut cfg, mut channels) = legacy(2);
    expect(
        build_legacy_import(&cfg, &channels[..1]),
        ErrorKind::NotFound,
    );
    channels[1].name = channels[0].name.clone();
    expect(
        build_legacy_import(&cfg, &channels),
        ErrorKind::ChannelConflict,
    );
    cfg.keys[0].channel_id = Some(100);
    cfg.keys[1].channel_id = Some(101);
    assert!(build_legacy_import(&cfg, &channels).is_ok());
    channels[1].group = "other".into();
    expect(
        build_legacy_import(&cfg, &channels),
        ErrorKind::ImportConflict,
    );
    channels[1].group = "default".into();
    channels[1].id = 100;
    expect(
        build_legacy_import(&cfg, &channels),
        ErrorKind::ChannelConflict,
    );
    let (mut cfg, channels) = legacy(2);
    cfg.keys[1].zhipu_api_key = cfg.keys[0].zhipu_api_key.clone();
    expect(
        build_legacy_import(&cfg, &channels),
        ErrorKind::DuplicateSource,
    );
    let (_dir, mut store) = setup();
    store
        .create_group(group("existing", SourceKind::Glm))
        .unwrap();
    let (cfg, channels) = legacy(1);
    expect(
        store.import_legacy(&build_legacy_import(&cfg, &channels).unwrap()),
        ErrorKind::ImportRequiresEmpty,
    );
}

#[test]
fn lock_drop_releases_despite_duplicated_descriptor() {
    let (dir, mut store) = setup();
    store
        .create_source(source("fake-lock-reopen", SourceKind::Glm))
        .unwrap();
    let before = store.snapshot().unwrap();
    let path = dir.path().join("catalog.sqlite3");
    // try_clone 和 fork 继承都保留同一打开文件描述；无需依赖随机调度。
    let duplicate = store._lock.0.try_clone().unwrap();
    drop(store);
    let mut reopened = CatalogStore::open(&path).unwrap();
    assert_eq!(reopened.snapshot().unwrap(), before);
    drop(duplicate);
    // 旧句柄的后续关闭不能解掉新 Store 的独立锁。
    expect(CatalogStore::open(&path), ErrorKind::AlreadyOpen);
}

#[test]
fn lock_failed_acquisition_does_not_release_the_owner() {
    let (dir, store) = setup();
    let path = dir.path().join(".catalog.lock");
    let open = || {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap()
    };
    for _ in 0..3 {
        expect(CatalogLock::acquire(open()), ErrorKind::AlreadyOpen);
        assert!(matches!(
            open().try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
    }
    drop(store);
    let acquired = CatalogLock::acquire(open()).unwrap();
    assert!(matches!(
        open().try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(acquired);
    assert!(CatalogLock::acquire(open()).is_ok());
}

#[test]
fn lock_error_exit_releases_guard_with_duplicate_alive() {
    let (dir, store) = setup();
    drop(store);
    let path = dir.path().join(".catalog.lock");
    let mut duplicate = None;
    let result: Result<()> = (|| {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let lock = CatalogLock::acquire(file)?;
        duplicate = Some(lock.0.try_clone().unwrap());
        Err(error(ErrorKind::UnsupportedSchema))
    })();
    expect(result, ErrorKind::UnsupportedSchema);
    assert!(duplicate.is_some());
    let reopened = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    drop(duplicate);
    expect(
        CatalogStore::open(&dir.path().join("catalog.sqlite3")),
        ErrorKind::AlreadyOpen,
    );
    drop(reopened);
}

#[test]
fn lock_validation_error_releases_resources_without_repair() {
    let (dir, store) = setup();
    let path = dir.path().join("catalog.sqlite3");
    store.conn.pragma_update(None, "user_version", 99).unwrap();
    let duplicate = store._lock.0.try_clone().unwrap();
    drop(store);
    let before = fs::read(&path).unwrap();
    for _ in 0..3 {
        expect(CatalogStore::open(&path), ErrorKind::UnsupportedSchema);
        let probe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.path().join(".catalog.lock"))
            .unwrap();
        probe.try_lock().unwrap();
        probe.unlock().unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    drop(duplicate);
}

#[test]
fn lock_remains_held_during_sqlite_connection_close() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct CloseProbe {
        file: File,
        called: Arc<AtomicBool>,
        held: Arc<AtomicBool>,
    }
    unsafe extern "C" fn value(
        context: *mut rusqlite::ffi::sqlite3_context,
        _argc: std::ffi::c_int,
        _argv: *mut *mut rusqlite::ffi::sqlite3_value,
    ) {
        unsafe { rusqlite::ffi::sqlite3_result_null(context) };
    }
    unsafe extern "C" fn close_probe(data: *mut std::ffi::c_void) {
        // 只由 sqlite3_create_function_v2 的 xDestroy 一次消费 Box；不跨 FFI panic。
        let probe = unsafe { Box::from_raw(data.cast::<CloseProbe>()) };
        let result = probe.file.try_lock();
        probe.held.store(
            matches!(result, Err(std::fs::TryLockError::WouldBlock)),
            Ordering::SeqCst,
        );
        if result.is_ok() {
            let _ = probe.file.unlock();
        }
        probe.called.store(true, Ordering::SeqCst);
    }
    let (dir, store) = setup();
    let called = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let data = Box::into_raw(Box::new(CloseProbe {
        file: OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.path().join(".catalog.lock"))
            .unwrap(),
        called: called.clone(),
        held: held.clone(),
    }));
    // 测试专用析构观测；不更改持久 schema、不增加 rusqlite feature 或生产 hook。
    let result = unsafe {
        rusqlite::ffi::sqlite3_create_function_v2(
            store.conn.handle(),
            c"lock_close_probe".as_ptr(),
            0,
            rusqlite::ffi::SQLITE_UTF8,
            data.cast(),
            Some(value),
            None,
            None,
            Some(close_probe),
        )
    };
    assert_eq!(result, rusqlite::ffi::SQLITE_OK);
    assert!(!called.load(Ordering::SeqCst));
    drop(store);
    assert!(called.load(Ordering::SeqCst));
    assert!(
        held.load(Ordering::SeqCst),
        "catalog lock was released before SQLite closed"
    );
    assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_ok());
}

#[test]
fn lock_child_process_probe() {
    let Some(directory) = std::env::var_os("QT_CATALOG_LOCK_TEST_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    assert_eq!(directory.parent(), Some(std::env::temp_dir().as_path()));
    assert!(directory
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("quota-throttle-lock-child-"));
    let result = CatalogStore::open(&directory.join("catalog.sqlite3"));
    let expected = std::env::var("QT_CATALOG_LOCK_TEST_EXPECT").unwrap();
    match expected.as_str() {
        "blocked" => expect(result, ErrorKind::AlreadyOpen),
        "free" => {
            let _store = result.unwrap();
        }
        _ => panic!("invalid lock test expectation"),
    }
    println!("\nCATALOG_LOCK_CHILD_CHECK={expected}");
}

#[tokio::test]
async fn lock_is_exclusive_and_reopenable_across_processes() {
    let dir = tempfile::Builder::new()
        .prefix("quota-throttle-lock-child-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let duplicate = store._lock.0.try_clone().unwrap();
    async fn probe(directory: &Path, expected: &str, filter: &str) -> bool {
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([filter, "--exact", "--nocapture"])
                .env("QT_CATALOG_LOCK_TEST_DIR", directory)
                .env("QT_CATALOG_LOCK_TEST_EXPECT", expected)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        let marker = format!("CATALOG_LOCK_CHILD_CHECK={expected}");
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| *line == marker)
                .count()
                == 1
    }
    let filter = "catalog::store::tests::lock_child_process_probe";
    assert!(probe(dir.path(), "blocked", filter).await);
    // exact 名称错误时 harness 会零测试成功；握手必须拒绝这种假绿。
    assert!(!probe(dir.path(), "blocked", "missing_lock_child_probe").await);
    drop(store);
    assert!(probe(dir.path(), "free", filter).await);
    drop(duplicate);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn lock_pbt_owner_and_duplicate_lifetimes(actions in prop::collection::vec(0u8..4, 1..16)) {
        let (dir, store) = setup();
        let path = dir.path().join("catalog.sqlite3");
        let mut owner = Some(store);
        let mut expected_held = true;
        let mut duplicates = Vec::new();
        for action in actions {
            match action {
                0 => {
                    if expected_held { expect(CatalogStore::open(&path), ErrorKind::AlreadyOpen); }
                    else { owner = Some(CatalogStore::open(&path).unwrap()); expected_held = true; }
                }
                1 => { drop(owner.take()); expected_held = false; }
                2 if expected_held => duplicates.push(owner.as_ref().unwrap()._lock.0.try_clone().unwrap()),
                _ => { drop(duplicates.pop()); }
            }
            let probe = OpenOptions::new().read(true).write(true).open(dir.path().join(".catalog.lock")).unwrap();
            let result = probe.try_lock();
            if expected_held { prop_assert!(matches!(result, Err(std::fs::TryLockError::WouldBlock))); }
            else { prop_assert!(result.is_ok()); probe.unlock().unwrap(); }
        }
        drop(owner);
        let reopened = CatalogStore::open(&path).unwrap();
        drop(duplicates);
        expect(CatalogStore::open(&path), ErrorKind::AlreadyOpen);
        drop(reopened);
    }
}

#[test]
fn private_paths_process_lock_and_reopen() {
    let (dir, store) = setup();
    let path = dir.path().join("catalog.sqlite3");
    let start = Instant::now();
    expect(CatalogStore::open(&path), ErrorKind::AlreadyOpen);
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(store);
    let store = CatalogStore::open(&path).unwrap();
    drop(store);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    expect(CatalogStore::open(&path), ErrorKind::UnsafePath);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    let dir2 = private_dir();
    let link = dir2.path().join("catalog.sqlite3");
    symlink(&path, &link).unwrap();
    expect(CatalogStore::open(&link), ErrorKind::UnsafePath);
    fs::remove_file(&link).unwrap();
    fs::hard_link(&path, &link).unwrap();
    expect(CatalogStore::open(&link), ErrorKind::UnsafePath);
    expect(
        CatalogStore::open(Path::new("catalog.sqlite3")),
        ErrorKind::UnsafePath,
    );
    let wide = private_dir();
    fs::set_permissions(wide.path(), fs::Permissions::from_mode(0o755)).unwrap();
    expect(
        CatalogStore::open(&wide.path().join("catalog.sqlite3")),
        ErrorKind::UnsafePath,
    );
}

#[test]
fn foreign_and_future_databases_are_not_modified() {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = private_dir();
    let path = dir.path().join("catalog.sqlite3");
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE tokens(key TEXT); INSERT INTO tokens VALUES('fake-only');")
        .unwrap();
    drop(conn);
    let before = fs::read(&path).unwrap();
    expect(CatalogStore::open(&path), ErrorKind::ForeignDatabase);
    assert_eq!(fs::read(&path).unwrap(), before);
    let (dir, store) = setup();
    let path = dir.path().join("catalog.sqlite3");
    store.conn.pragma_update(None, "user_version", 99).unwrap();
    drop(store);
    let before = fs::read(&path).unwrap();
    expect(CatalogStore::open(&path), ErrorKind::UnsupportedSchema);
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn readonly_identity_probe_busy_is_not_foreign_database() {
    let (dir, store) = setup();
    let path = dir.path().join("catalog.sqlite3");
    drop(store);
    let external = Connection::open(&path).unwrap();
    external
        .pragma_update(None, "journal_mode", "DELETE")
        .unwrap();
    external.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let start = Instant::now();
    expect(CatalogStore::open(&path), ErrorKind::Busy);
    assert!(start.elapsed() < Duration::from_secs(3));
    external.execute_batch("ROLLBACK").unwrap();
    drop(external);
    assert!(CatalogStore::open(&path).is_ok());
}

#[test]
fn schema_and_private_data_corruption_fail_closed() {
    let (dir, store) = setup();
    store
        .conn
        .execute("DROP INDEX active_source_group", [])
        .unwrap();
    drop(store);
    expect(
        CatalogStore::open(&dir.path().join("catalog.sqlite3")),
        ErrorKind::CorruptData,
    );
    let (dir, mut store) = setup();
    let s = store
        .create_source(source("secret", SourceKind::Glm))
        .unwrap();
    store.conn.execute("UPDATE sources SET settings_json=json_set(settings_json,'$.settings.quota.global_headers',json('[{\"key\":\"X-Test\",\"value\":\"a\",\"unknown\":true}]'))",[]).unwrap();
    expect(
        store.read_source_material(s.source_id),
        ErrorKind::CorruptData,
    );
    drop(store);
    expect(
        CatalogStore::open(&dir.path().join("catalog.sqlite3")),
        ErrorKind::CorruptData,
    );
}

#[test]
fn persisted_discovery_auth_must_not_be_defaulted() {
    let (dir, mut store) = setup();
    let source = store
        .create_source(source("fake-auth", SourceKind::Glm))
        .unwrap();
    store.conn.execute("UPDATE sources SET settings_json=json_remove(settings_json,'$.settings.upstream.model_discovery.auth')",[]).unwrap();
    expect(
        store.read_source_material(source.source_id),
        ErrorKind::CorruptData,
    );
    drop(store);
    expect(
        CatalogStore::open(&dir.path().join("catalog.sqlite3")),
        ErrorKind::CorruptData,
    );
}

#[test]
fn private_sql_column_type_errors_are_corrupt_data() {
    for update in [
        "UPDATE sources SET settings_json=CAST(settings_json AS BLOB)",
        "UPDATE source_secrets SET raw_key=CAST(raw_key AS BLOB)",
    ] {
        let (dir, mut store) = setup();
        let s = store
            .create_source(source("fake-blob", SourceKind::Glm))
            .unwrap();
        store.conn.execute(update, []).unwrap();
        expect(
            store.read_source_material(s.source_id),
            ErrorKind::CorruptData,
        );
        drop(store);
        expect(
            CatalogStore::open(&dir.path().join("catalog.sqlite3")),
            ErrorKind::CorruptData,
        );
    }
}

#[test]
fn failed_backup_cleanup_is_explicit_and_identity_guarded() {
    let dir = private_dir();
    let path = dir.path().join("failed-backup");
    let file = File::create_new(&path).unwrap();
    let identity = file.metadata().unwrap();
    let err = cleanup_failed_backup(&path, &identity, |_| {
        Err(std::io::ErrorKind::PermissionDenied.into())
    });
    assert!(err.incomplete_backup && path.exists());
    assert!(err.to_string().contains("incomplete backup"));
    let other = dir.path().join("other");
    let other_file = File::create_new(&other).unwrap();
    let other_id = other_file.metadata().unwrap();
    let err = cleanup_failed_backup(&path, &other_id, |_| {
        panic!("must not delete mismatched identity")
    });
    assert!(err.incomplete_backup && path.exists());
    let err = cleanup_failed_backup(&path, &identity, |p| fs::remove_file(p));
    assert!(!err.incomplete_backup && !path.exists());
    let (_dir, store) = setup();
    let target = dir.path().join("backup.sqlite3");
    let sidecar = dir.path().join("backup.sqlite3-wal");
    File::create_new(&sidecar).unwrap();
    expect(store.backup_to(&target), ErrorKind::BackupExists);
    assert!(sidecar.exists() && !target.exists());
}

#[test]
fn legacy_raw_model_limit_is_checked_before_normalization() {
    let (cfg, mut channels) = legacy(1);
    channels[0].models_csv = vec!["same"; 1001].join(",");
    expect(
        build_legacy_import(&cfg, &channels),
        ErrorKind::InvalidInput,
    );
    channels[0].models_csv = vec!["same"; 1000].join(",");
    assert!(build_legacy_import(&cfg, &channels).is_ok());
}

#[test]
fn legacy_template_must_match_but_models_come_from_channel() {
    let (mut cfg, channels) = legacy(1);
    let template = crate::config::ChannelTemplate {
        channel_type: 8,
        base_url: channels[0].base_url.clone(),
        models: "stale-model".into(),
        group: "default".into(),
        model_discovery: Some(ModelDiscoveryConfig {
            url: "https://example.test/models".into(),
            auth: ModelDiscoveryAuth::AuthorizationRaw,
        }),
    };
    cfg.new_api.channel_template = Some(template.clone());
    let plan = build_legacy_import(&cfg, &channels).unwrap();
    let (_dir, mut store) = setup();
    let ImportOutcome::Applied(receipt) = store.import_legacy(&plan).unwrap() else {
        panic!()
    };
    let SourceSettings::Glm { upstream, .. } = store
        .read_source_material(receipt.source_ids[0])
        .unwrap()
        .settings
    else {
        panic!()
    };
    assert_eq!(upstream.models, vec!["m1", "m2"]);
    assert_eq!(upstream.model_discovery, template.model_discovery);
    for case in 0..3 {
        let mut mismatch = template.clone();
        match case {
            0 => mismatch.group = "other".into(),
            1 => mismatch.channel_type = 1,
            _ => mismatch.base_url = "https://other.test/chat/completions".into(),
        }
        cfg.new_api.channel_template = Some(mismatch);
        expect(
            build_legacy_import(&cfg, &channels),
            ErrorKind::ImportConflict,
        );
    }
}

#[test]
fn backup_includes_wal_and_never_overwrites_existing_file() {
    let (_dir, mut store) = setup();
    let s = store
        .create_source(source("backup-only-fake", SourceKind::Volcano))
        .unwrap();
    let backup = private_dir();
    let path = backup.path().join("catalog.sqlite3");
    store.backup_to(&path).unwrap();
    let mut restored = CatalogStore::open(&path).unwrap();
    assert_eq!(store.snapshot().unwrap(), restored.snapshot().unwrap());
    assert_eq!(
        restored
            .read_source_material(s.source_id)
            .unwrap()
            .credential
            .expose(),
        "backup-only-fake"
    );
    drop(restored);
    let before = fs::read(&path).unwrap();
    expect(store.backup_to(&path), ErrorKind::BackupExists);
    assert_eq!(fs::read(&path).unwrap(), before);
    expect(store.backup_to(&store.path), ErrorKind::BackupExists);
}

#[test]
fn retired_channels_remain_reserved_and_pause_does_not_enable_members() {
    let (_dir, mut store) = setup();
    let (cfg, channels) = legacy(2);
    store
        .import_legacy(&build_legacy_import(&cfg, &channels).unwrap())
        .unwrap();
    let snapshot = store.snapshot().unwrap();
    store
        .conn
        .execute(
            "UPDATE memberships SET lifecycle='retired',retired_at=1 WHERE channel_id=100",
            [],
        )
        .unwrap();
    assert!(store
        .conn
        .execute(
            "UPDATE memberships SET channel_id=100 WHERE channel_id=101",
            []
        )
        .is_err());
    store
        .conn
        .execute(
            "UPDATE sources SET manual_paused=1 WHERE source_id=?1",
            [snapshot.sources[0].source_id.get()],
        )
        .unwrap();
    let planned = store
        .plan_membership(
            snapshot.sources[0].source_id,
            snapshot.groups[0].group_id,
            10,
        )
        .unwrap();
    assert_eq!(planned.channel_id, None);
    assert_eq!(planned.order, 2);
    assert!(planned.membership_id.get() > snapshot.memberships[1].membership_id.get());
}

#[test]
fn busy_and_commit_unknown_are_not_success() {
    let (_dir, mut store) = setup();
    let external = Connection::open(&store.path).unwrap();
    external.execute_batch("BEGIN IMMEDIATE").unwrap();
    let start = Instant::now();
    expect(
        store.create_group(group("busy", SourceKind::Glm)),
        ErrorKind::Busy,
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    external.execute_batch("ROLLBACK").unwrap();
    let mut poisoned = false;
    // 延迟 FK 在 commit 才失败，验证未知提交的统一句柄失效分支。
    let tx = store.conn.transaction().unwrap();
    tx.pragma_update(None, "defer_foreign_keys", true).unwrap();
    tx.execute("INSERT INTO memberships(source_id,group_id,kind,position,lifecycle,desired_priority) VALUES(999,999,'glm',0,'planned',10)",[]).unwrap();
    expect(commit(tx, &mut poisoned), ErrorKind::CommitUnknown);
    assert!(poisoned);
    store.poisoned = poisoned;
    expect(store.snapshot(), ErrorKind::CommitUnknown);
    expect(
        store.read_source_material(SourceId::new(1).unwrap()),
        ErrorKind::CommitUnknown,
    );
}

#[test]
fn import_and_snapshot_scale_within_budget() {
    let (_dir, mut store) = setup();
    let (cfg, channels) = legacy(MAX_SOURCES);
    let plan = build_legacy_import(&cfg, &channels).unwrap();
    let start = Instant::now();
    store.import_legacy(&plan).unwrap();
    assert!(start.elapsed() < Duration::from_secs(10));
    let start = Instant::now();
    assert_eq!(store.snapshot().unwrap().sources.len(), MAX_SOURCES);
    assert!(start.elapsed() < Duration::from_secs(10));
    expect(
        store.create_source(source("over-capacity", SourceKind::Glm)),
        ErrorKind::InvalidInput,
    );
    for i in 1..MAX_GROUPS {
        store
            .create_group(group(&format!("g{i}"), SourceKind::Glm))
            .unwrap();
    }
    expect(
        store.create_group(group("over", SourceKind::Glm)),
        ErrorKind::InvalidInput,
    );
    store.conn.execute("INSERT INTO memberships(source_id,group_id,kind,position,lifecycle,desired_priority) SELECT s.source_id,g.group_id,'glm',s.source_id-1,'planned',10 FROM sources s CROSS JOIN source_groups g WHERE g.group_id BETWEEN 2 AND 10",[]).unwrap();
    let before = store.snapshot().unwrap();
    assert_eq!(before.memberships.len(), MAX_MEMBERSHIPS);
    expect(
        store.plan_membership(before.sources[0].source_id, before.groups[10].group_id, 10),
        ErrorKind::InvalidInput,
    );
    assert_eq!(before, store.snapshot().unwrap());
}

#[test]
fn model_header_and_policy_boundaries() {
    let (_dir, mut store) = setup();
    let mut draft = source("too-many-models", SourceKind::Volcano);
    if let SourceSettings::Volcano { upstream, .. } = &mut draft.settings {
        upstream.models = vec!["same".into(); 1001];
    }
    expect(store.create_source(draft), ErrorKind::InvalidInput);
    let mut draft = source("too-many-headers", SourceKind::Glm);
    if let SourceSettings::Glm { quota, .. } = &mut draft.settings {
        quota.global_headers = vec![
            HeaderKV {
                key: "X-Test".into(),
                value: "fake".into()
            };
            65
        ];
    }
    expect(store.create_source(draft), ErrorKind::InvalidInput);
    for (threshold, minute, offset, limit) in [
        (0, 60, 480, None),
        (101, 60, 480, None),
        (3, 1440, 480, None),
        (3, 60, 841, None),
        (3, 60, -721, None),
        (3, 60, 480, Some(JSON_SAFE + 1)),
    ] {
        let mut draft = source("bad-policy", SourceKind::Volcano);
        if let SourceSettings::Volcano { policy, .. } = &mut draft.settings {
            *policy = VolcanoPolicy {
                daily_token_limit: limit,
                failure_threshold: threshold,
                reset_minute: minute,
                utc_offset_minutes: offset,
            };
        }
        expect(store.create_source(draft), ErrorKind::InvalidInput);
    }
    assert!(store.snapshot().unwrap().sources.is_empty());
    for (i, (minute, offset)) in [(0, -720), (1439, 840)].into_iter().enumerate() {
        let mut draft = source(&format!("good-{i}"), SourceKind::Volcano);
        if let SourceSettings::Volcano { policy, .. } = &mut draft.settings {
            policy.reset_minute = minute;
            policy.utc_offset_minutes = offset;
        }
        assert!(store.create_source(draft).is_ok());
    }
}

#[test]
fn catalog_schema_output() {
    let (_dir, mut store) = setup();
    let (cfg, channels) = legacy(1);
    store
        .import_legacy(&build_legacy_import(&cfg, &channels).unwrap())
        .unwrap();
    let s = store
        .create_source(source("schema-volcano-fake", SourceKind::Volcano))
        .unwrap();
    let a = store
        .create_group(group("volcano-a", SourceKind::Volcano))
        .unwrap();
    let b = store
        .create_group(group("volcano-b", SourceKind::Volcano))
        .unwrap();
    store.plan_membership(s.source_id, a.group_id, 100).unwrap();
    store.plan_membership(s.source_id, b.group_id, 10).unwrap();
    let snapshot = store.snapshot().unwrap();
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("fake-private") && !json.contains("schema-volcano-fake"));
    println!("CATALOG_SCHEMA_JSON={json}");
    let kinds: Vec<_> = SourceKind::VALUES
        .iter()
        .map(|s| SourceKind::parse(s).unwrap())
        .collect();
    let lifecycles: Vec<_> = MembershipLifecycle::VALUES
        .iter()
        .map(|s| MembershipLifecycle::parse(s).unwrap())
        .collect();
    println!(
        "CATALOG_ENUM_JSON={}",
        serde_json::json!({"kind":kinds,"lifecycle":lifecycles})
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn property_c1_c2_reference_model(ops in prop::collection::vec((0u8..12,any::<bool>(),0u8..6,any::<bool>()),1..35)) {
        let (_dir,mut store)=setup();
        let mut sources=BTreeMap::new(); let mut groups=BTreeMap::new(); let mut pairs=HashSet::new();
        for (key,is_glm,group_no,group_glm) in ops {
            let kind=if is_glm {SourceKind::Glm} else {SourceKind::Volcano};
            let gkind=if group_glm {SourceKind::Glm} else {SourceKind::Volcano};
            let s=match sources.get(&key).copied() {
                Some((id,old_kind))=>{let result=store.create_source(source(&format!("key{key}"),kind)); prop_assert_eq!(result.unwrap_err().kind,ErrorKind::DuplicateSource); (id,old_kind)},
                None=>{let v=store.create_source(source(&format!("key{key}"),kind)).unwrap(); sources.insert(key,(v.source_id,kind)); (v.source_id,kind)}
            };
            let g=match groups.get(&group_no).copied() {
                Some(value)=>{let result=store.create_group(group(&format!("g{group_no}"),gkind)); prop_assert_eq!(result.unwrap_err().kind,ErrorKind::DuplicateGroup); value},
                None=>{let v=store.create_group(group(&format!("g{group_no}"),gkind)).unwrap(); groups.insert(group_no,(v.group_id,gkind)); (v.group_id,gkind)}
            };
            let outcome=store.plan_membership(s.0,g.0,10);
            if s.1!=g.1 {prop_assert_eq!(outcome.unwrap_err().kind,ErrorKind::TypeMismatch);}
            else if !pairs.insert((s.0,g.0)) {prop_assert_eq!(outcome.unwrap_err().kind,ErrorKind::DuplicateMembership);}
            else {prop_assert!(outcome.is_ok());}
            let snapshot=store.snapshot().unwrap();
            prop_assert_eq!(snapshot.sources.len(),sources.len());
            prop_assert_eq!(snapshot.groups.len(),groups.len());
            prop_assert_eq!(snapshot.memberships.iter().map(|m|(m.source_id,m.group_id)).collect::<HashSet<_>>(),pairs.clone());
            let secrets=store.conn.query_row("SELECT count(*) FROM source_secrets",[],|r|r.get::<_,i64>(0)).unwrap();
            prop_assert_eq!(secrets,sources.len() as i64);
            let private_rows: Vec<(i64,i64,String)> = store.conn.prepare("SELECT s.source_id,s.secret_id,k.raw_key FROM sources s LEFT JOIN source_secrets k ON k.secret_id=s.secret_id ORDER BY s.source_id").unwrap()
                .query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
            prop_assert_eq!(private_rows.len(),sources.len());
            prop_assert_eq!(private_rows.iter().map(|r|r.1).collect::<HashSet<_>>().len(),sources.len());
            for (logical_key,(id,kind)) in &sources {
                let actual=private_rows.iter().find(|r|r.0==id.get()).unwrap();
                prop_assert_eq!(&actual.2,&format!("key{logical_key}"));
                prop_assert_eq!(snapshot.sources.iter().find(|s|s.source_id==*id).unwrap().kind,*kind);
            }
            for (id,kind) in groups.values() {
                prop_assert_eq!(snapshot.groups.iter().find(|g|g.group_id==*id).unwrap().kind,*kind);
            }
        }
    }

    #[test]
    fn property_c3_retired_channel_identity(channel in 1i64..100000,offset in 1i64..1000) {
        let (_dir,mut store)=setup(); let (cfg,mut channels)=legacy(2);
        channels[0].id=channel; channels[1].id=channel+offset;
        store.import_legacy(&build_legacy_import(&cfg,&channels).unwrap()).unwrap();
        store.conn.execute("UPDATE memberships SET lifecycle='retired',retired_at=0 WHERE channel_id=?1",[channel]).unwrap();
        prop_assert!(store.conn.execute("UPDATE memberships SET channel_id=?1 WHERE channel_id=?2",params![channel,channel+offset]).is_err());
        let snapshot=store.snapshot().unwrap(); prop_assert_eq!(snapshot.memberships[0].channel_id.unwrap().get(),channel);
        channels[1].id=channel;
        prop_assert_eq!(build_legacy_import(&cfg,&channels).unwrap_err().kind,ErrorKind::ChannelConflict);
    }

    #[test]
    fn property_c4_c6_import_replay_and_conversion(n in 1usize..10,reverse in any::<bool>(),explicit in any::<bool>(),note in "[a-z]{0,12}",
        models in prop::collection::vec(0u8..8,1..16), global_values in prop::collection::vec("[a-z]{1,8}",1..5),
        key_values in prop::collection::vec("[a-z]{1,8}",1..5), rotation in 0usize..20) {
        let (dir,mut store)=setup(); let (mut cfg,mut channels)=legacy(n);
        cfg.zhipu.extra_headers=global_values.iter().enumerate().map(|(i,v)|HeaderKV {key:format!("X-Global-{}",i%2),value:format!("GLOBAL-{i}-{v}")}).collect();
        for c in &mut channels {
            c.models_csv=models.iter().map(|m|format!(" m{}-{m} , ",c.id)).collect::<Vec<_>>().join(",");
            c.priority=c.id*3-301;
        }
        if reverse {cfg.keys.reverse();}
        for (i,k) in cfg.keys.iter_mut().enumerate() {
            k.note=format!("note-{}-{note}",k.name);
            k.quota_headers=key_values.iter().enumerate().map(|(j,v)|HeaderKV {key:format!("X-{}-{}",k.name,j%2),value:format!("{}-{j}-{v}",k.name)}).collect();
            if explicit && i%2==0 {k.channel_id=Some(100+k.name[1..].parse::<i64>().unwrap());}
        }
        let plan=build_legacy_import(&cfg,&channels).unwrap();
        // 仅改变查询返回顺序，配置 key 顺序保持原样。
        channels.rotate_left(rotation % n); channels.reverse();
        let reordered_plan=build_legacy_import(&cfg,&channels).unwrap();
        prop_assert_eq!(&plan.digest,&reordered_plan.digest);
        let ImportOutcome::Applied(receipt)=store.import_legacy(&plan).unwrap() else {panic!()};
        let snapshot=assert_legacy_fidelity(&mut store,&cfg,&channels,&receipt);
        drop(store); let mut store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        prop_assert_eq!(store.import_legacy(&plan).unwrap(),ImportOutcome::AlreadyApplied(receipt));
        prop_assert_eq!(store.snapshot().unwrap(),snapshot);
    }

    #[test]
    fn property_c5_rollback_between_writes(table in prop::sample::select(vec!["source_secrets","sources","memberships","legacy_import"]),fail_at in 1i64..4) {
        let (dir,mut store)=setup(); let before=store.snapshot().unwrap();
        // 真实 SQLite trigger 注入 INSERT 错误，而非模拟成功返回值。
        let condition=if table=="legacy_import" {"1".to_owned()} else {format!("(SELECT count(*) FROM {table})={}",fail_at-1)};
        store.conn.execute_batch(&format!("CREATE TRIGGER injected BEFORE INSERT ON {table} WHEN {condition} BEGIN SELECT RAISE(ABORT,'injected'); END;")).unwrap();
        let (cfg,channels)=legacy(3); let plan=build_legacy_import(&cfg,&channels).unwrap();
        prop_assert!(store.import_legacy(&plan).is_err());
        prop_assert_eq!(store.snapshot().unwrap(),before.clone());
        prop_assert_eq!(store.conn.query_row("SELECT count(*) FROM source_secrets",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        store.conn.execute("DROP TRIGGER injected",[]).unwrap(); drop(store);
        let mut store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        prop_assert_eq!(store.snapshot().unwrap(),before);
        prop_assert!(matches!(store.import_legacy(&plan).unwrap(),ImportOutcome::Applied(_)));
    }

    #[test]
    fn property_c7_no_secret_in_snapshots_or_debug(suffix in "[a-zA-Z0-9]{8,40}") {
        use sha2::{Digest,Sha256};
        let (_dir,mut store)=setup(); let sentinel=format!("PRIVATE_{suffix}");
        let global=format!("GLOBAL_{suffix}"); let selector=format!("SELECTOR_{suffix}");
        let mut hash=Sha256::new(); hash.update(b"quota-throttle/source-key/v1\0"); hash.update(sentinel.as_bytes());
        let expected_fingerprint=format!("{:x}",hash.finalize());
        let sensitive=[sentinel.as_str(),global.as_str(),selector.as_str(),expected_fingerprint.as_str()];
        let mut draft=source(&sentinel,SourceKind::Glm);
        if let SourceSettings::Glm {quota,..}=&mut draft.settings {
            quota.global_headers=vec![HeaderKV {key:"X-Global".into(),value:global.clone()}];
            quota.key_headers=vec![HeaderKV {key:"X-Sentinel".into(),value:selector.clone()}];
        }
        let debug=format!("{:?}",draft.credential); prop_assert_eq!(&debug,"[REDACTED]");
        prop_assert_eq!(draft.credential.to_string(),"[REDACTED]");
        let validated=validate_source(SourceDraft {name:draft.name.clone(),note:draft.note.clone(),settings:draft.settings.clone(),credential:draft.credential.clone()}).unwrap();
        let mut diagnostics=vec![format!("{draft:?}"),format!("{:?}",draft.settings),format!("{validated:?}")];
        if let SourceSettings::Glm {quota,upstream}=&draft.settings {
            diagnostics.extend([format!("{quota:?}"),format!("{upstream:?}")]);
        }
        let s=store.create_source(draft).unwrap();
        let material=store.read_source_material(s.source_id).unwrap();
        diagnostics.extend([format!("{material:?}"),format!("{:?}",material.settings),format!("{:?}",material.credential),material.credential.to_string()]);
        prop_assert_eq!(material.credential.expose(),sentinel.as_str());
        let stored_fingerprint:String=store.conn.query_row("SELECT credential_fingerprint FROM sources WHERE source_id=?1",[s.source_id.get()],|r|r.get(0)).unwrap();
        prop_assert_eq!(stored_fingerprint,expected_fingerprint.clone());
        let duplicate=store.create_source(source(&sentinel,SourceKind::Glm)).unwrap_err();
        diagnostics.extend([format!("{duplicate:?}"),duplicate.to_string()]);
        let snapshot=store.snapshot().unwrap(); let json=serde_json::to_string(&snapshot).unwrap();
        prop_assert!(!json.contains("fingerprint")); diagnostics.push(json);
        let (mut cfg,channels)=legacy(1); cfg.keys[0].zhipu_api_key=sentinel.clone();
        let plan=build_legacy_import(&cfg,&channels).unwrap(); diagnostics.push(format!("{plan:?}"));
        for text in &diagnostics {for value in sensitive {prop_assert!(!text.contains(value));}}
    }
}
