use super::*;
use crate::catalog::balance::{verify_balance, BalancePolicy, BalancePolicyDraft, BALANCE_CAP};
use crate::catalog::control::ControlErrorCode as Code;
use rusqlite::types::Value;

fn defaults() -> BalancePolicy {
    BalancePolicy {
        revision: 1,
        auto_refill: true,
        warn_below: 100_000_000_000_000,
        refill_to: 1_000_000_000_000_000,
    }
}

fn draft(policy: BalancePolicy) -> BalancePolicyDraft {
    BalancePolicyDraft {
        auto_refill: policy.auto_refill,
        warn_below: policy.warn_below,
        refill_to: policy.refill_to,
    }
}

fn image(conn: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
    let names = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut query = conn
                .prepare(&format!(
                    "SELECT * FROM \"{}\" ORDER BY rowid",
                    name.replace('"', "\"\"")
                ))
                .unwrap();
            let n = query.column_count();
            let rows = query
                .query_map([], |row| (0..n).map(|i| row.get(i)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}

fn as_v8(store: &mut CatalogStore) {
    let tx = store.conn.transaction().unwrap();
    let instance: String = tx
        .query_row(
            "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    tx.execute_batch("DROP TABLE balance_policy; DROP TABLE catalog_meta")
        .unwrap();
    tx.execute_batch(SCHEMA_V8.split(';').next().unwrap())
        .unwrap();
    tx.execute("INSERT INTO catalog_meta VALUES(1,8,?1)", [instance])
        .unwrap();
    tx.pragma_update(None, "user_version", 8).unwrap();
    tx.commit().unwrap();
    verify_schema(&store.conn, 8).unwrap();
}

fn backups(dir: &TempDir) -> Vec<PathBuf> {
    fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sqlite3")
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("catalog-v8-before-v9-")
        })
        .collect()
}

#[test]
fn balance_policy_defaults_validation_and_revision() {
    let (dir, mut store) = setup();
    assert_eq!(BALANCE_CAP, 4_000_000_000_000_000);
    assert_eq!(store.balance_policy().unwrap(), defaults());
    let before = image(&store.conn);
    assert_eq!(
        store.save_balance_policy(1, draft(defaults())).unwrap(),
        defaults()
    );
    assert_eq!(image(&store.conn), before);
    let changed = BalancePolicyDraft {
        auto_refill: false,
        warn_below: 1,
        refill_to: BALANCE_CAP,
    };
    let policy = store.save_balance_policy(1, changed).unwrap();
    assert_eq!(
        policy,
        BalancePolicy {
            revision: 2,
            auto_refill: false,
            warn_below: 1,
            refill_to: BALANCE_CAP
        }
    );
    let mut expected = before;
    expected.insert(
        "balance_policy".into(),
        vec![vec![1, 2, 0, 1, 4_000_000_000_000_000]
            .into_iter()
            .map(Value::Integer)
            .collect()],
    );
    assert_eq!(
        image(&store.conn),
        expected,
        "only the singleton policy may change"
    );
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(store.balance_policy().unwrap(), policy);
    assert_eq!(
        backups(&dir).len(),
        0,
        "fresh v9 must not claim to back up an old v8"
    );
}

#[test]
fn balance_validation_conflict_and_revision_exhaustion_are_read_only() {
    let (_dir, mut store) = setup();
    let before = image(&store.conn);
    for (revision, warn_below, refill_to) in [
        (0, 1, 2),
        (-1, 1, 2),
        (9_007_199_254_740_992, 1, 2),
        (1, 0, 2),
        (1, -1, 2),
        (1, 2, 2),
        (1, 3, 2),
        (1, 1, 0),
        (1, 1, -1),
        (1, 1, BALANCE_CAP + 1),
        (1, i64::MAX - 1, i64::MAX),
    ] {
        let input = BalancePolicyDraft {
            auto_refill: false,
            warn_below,
            refill_to,
        };
        assert_eq!(
            store.save_balance_policy(revision, input).unwrap_err().code,
            Code::InvalidInput
        );
        assert_eq!(image(&store.conn), before);
    }
    // 即使内容相同，也不能让旧 revision 绕过比较。
    assert_eq!(
        store
            .save_balance_policy(2, draft(defaults()))
            .unwrap_err()
            .code,
        Code::RevisionConflict
    );
    assert_eq!(image(&store.conn), before);
    store
        .conn
        .execute("UPDATE balance_policy SET revision=9007199254740991", [])
        .unwrap();
    let mut current = defaults();
    current.revision = 9_007_199_254_740_991;
    assert_eq!(
        store
            .save_balance_policy(current.revision, draft(current))
            .unwrap(),
        current
    );
    let before = image(&store.conn);
    let mut input = draft(current);
    input.auto_refill = false;
    assert_eq!(
        store
            .save_balance_policy(current.revision, input)
            .unwrap_err()
            .code,
        Code::InvalidInput
    );
    assert_eq!(image(&store.conn), before);
    assert!(store.usable().is_ok());
}

#[test]
fn balance_policy_busy_is_queue_full_not_storage() {
    for commit_busy in [false, true] {
        let (_dir, mut store) = setup();
        store.conn.busy_timeout(Duration::ZERO).unwrap();
        if commit_busy {
            // WAL 提交不会被读者阻塞；用回滚日志真实触发 commit 的 SQLITE_BUSY。
            store
                .conn
                .pragma_update(None, "journal_mode", "DELETE")
                .unwrap();
        }
        let other = Connection::open(&store.path).unwrap();
        other.busy_timeout(Duration::ZERO).unwrap();
        other
            .execute_batch(if commit_busy {
                "BEGIN; SELECT * FROM balance_policy;"
            } else {
                "BEGIN IMMEDIATE;"
            })
            .unwrap();
        let before = image(&store.conn);
        let mut input = draft(defaults());
        input.auto_refill = false;
        assert_eq!(
            store.save_balance_policy(1, input).unwrap_err().code,
            Code::QueueFull,
            "commit_busy={commit_busy}"
        );
        other.execute_batch("ROLLBACK").unwrap();
        assert!(store.usable().is_ok());
        assert!(store.conn.is_autocommit());
        assert_eq!(store.balance_policy().unwrap(), defaults());
        assert_eq!(image(&store.conn), before);
        assert_eq!(store.save_balance_policy(1, input).unwrap().revision, 2);
    }
}

#[test]
fn v8_migration_adds_default_balance_policy_and_backup() {
    let (dir, mut store) = policy_commands::token_history_fixture();
    // 另加一个已导入身份，确保 v8 token 数据不是空夹具。
    store.conn.execute("INSERT INTO managed_tokens(owner_user_id,owner_scope,native_token_id,explicit_group,display_name,revision,evidence_revision,created_at) VALUES(41,'issuer',10001,'','balance fixture',1,0,0)", []).unwrap();
    for table in [
        "ledger_facts",
        "ledger_minute_usage",
        "execution_attempts",
        "managed_tokens",
    ] {
        assert!(
            store
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap()
                > 0
        );
    }
    let expected = image(&store.conn);
    as_v8(&mut store);
    let old = image(&store.conn);
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&store.conn), expected);
    assert_eq!(store.balance_policy().unwrap(), defaults());
    assert_eq!(
        store
            .conn
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        9
    );
    let files = backups(&dir);
    assert_eq!(files.len(), 1);
    assert_eq!(
        fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let saved = Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    verify_schema(&saved, 8).unwrap();
    assert_eq!(image(&saved), old);
    drop(saved);
    drop(store);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&store.conn), expected);
    assert_eq!(backups(&dir).len(), 1, "reopening must not migrate twice");
}

#[test]
fn balance_migration_failure_keeps_v8_and_backup() {
    let (dir, mut store) = setup();
    as_v8(&mut store);
    store
        .conn
        .execute_batch(
            "CREATE TABLE balance_policy(marker TEXT); INSERT INTO balance_policy VALUES('keep')",
        )
        .unwrap();
    let before = image(&store.conn);
    drop(store);
    assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
    let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&conn), before);
    verify_schema(&conn, 8).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        8
    );
    let files = backups(&dir);
    assert_eq!(files.len(), 1);
    let saved = Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(image(&saved), before);
}

#[test]
fn balance_corrupt_v8_rejected_before_backup_or_migration() {
    let (dir, mut store) = setup();
    as_v8(&mut store);
    store
        .conn
        .execute("DROP INDEX ledger_cell_source_time", [])
        .unwrap();
    let before = image(&store.conn);
    drop(store);
    let error = match CatalogStore::open(&dir.path().join("catalog.sqlite3")) {
        Ok(_) => panic!("corrupt v8 accepted"),
        Err(error) => error,
    };
    assert_eq!(error.kind, ErrorKind::CorruptData);
    assert!(backups(&dir).is_empty());
    let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&conn), before);
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        8
    );
}

#[test]
fn balance_corrupt_v9_never_repaired_or_defaulted() {
    for damage in [
        "DELETE FROM balance_policy",
        "UPDATE balance_policy SET revision=0",
        "UPDATE balance_policy SET revision=9007199254740992",
        "UPDATE balance_policy SET auto_refill=2",
        "UPDATE balance_policy SET warn_below=0",
        "UPDATE balance_policy SET warn_below=refill_to",
        "UPDATE balance_policy SET refill_to=4000000000000001",
        "UPDATE balance_policy SET singleton=2",
        "INSERT INTO balance_policy VALUES(2,1,1,1,2)",
    ] {
        let (dir, store) = setup();
        store
            .conn
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        store.conn.execute_batch(damage).unwrap();
        assert_eq!(
            verify_balance(&store.conn).unwrap_err().kind,
            ErrorKind::CorruptData,
            "{damage}"
        );
        if !damage.starts_with("INSERT") {
            assert_eq!(store.balance_policy().unwrap_err().code, Code::Storage);
        }
        let before = image(&store.conn);
        drop(store);
        let error = match CatalogStore::open(&dir.path().join("catalog.sqlite3")) {
            Ok(_) => panic!("corrupt v9 accepted: {damage}"),
            Err(error) => error,
        };
        assert_eq!(error.kind, ErrorKind::CorruptData, "{damage}");
        let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(image(&conn), before);
    }
}

#[test]
fn balance_commit_unknown_poisoning_requires_reopen() {
    let (dir, mut store) = setup();
    store.conn.execute_batch("CREATE TABLE balance_test_parent(id INTEGER PRIMARY KEY); CREATE TABLE balance_test_child(id INTEGER REFERENCES balance_test_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER balance_deferred_fault AFTER UPDATE ON balance_policy BEGIN INSERT INTO balance_test_child VALUES(1); END;").unwrap();
    let before = image(&store.conn);
    let mut input = draft(defaults());
    input.auto_refill = false;
    assert_eq!(
        store.save_balance_policy(1, input).unwrap_err().code,
        Code::Storage
    );
    assert!(
        store.poisoned,
        "non-Busy commit failure must close this store"
    );
    assert!(store.usable().is_err());
    assert!(store.conn.is_autocommit());
    assert_eq!(
        image(&store.conn),
        before,
        "failed commit rolls back the attempted policy"
    );
    assert_eq!(store.balance_policy().unwrap_err().code, Code::Storage);
    assert_eq!(
        store.save_balance_policy(1, input).unwrap_err().code,
        Code::Storage
    );
    store.conn.execute_batch("DROP TRIGGER balance_deferred_fault; DROP TABLE balance_test_child; DROP TABLE balance_test_parent;").unwrap();
    drop(store);
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(store.balance_policy().unwrap(), defaults());
    assert_eq!(store.save_balance_policy(1, input).unwrap().revision, 2);
}

#[test]
fn balance_unusable_store_rejects_reads_and_writes() {
    let (_dir, mut store) = setup();
    let before = image(&store.conn);
    store.poisoned = true;
    assert_eq!(store.balance_policy().unwrap_err().code, Code::Storage);
    assert_eq!(
        store
            .save_balance_policy(1, draft(defaults()))
            .unwrap_err()
            .code,
        Code::Storage
    );
    assert_eq!(image(&store.conn), before);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn balance_pb_policy_trace_matches_independent_revision_model(
        trace in prop::collection::vec((any::<bool>(), 0i64..12, 0i64..12, 0u8..3, any::<bool>()), 1..24)
    ) {
        let (dir, mut store) = setup();
        let mut model = defaults();
        for (auto_refill, warn_below, refill_to, revision_mode, reopen) in trace {
            let revision = match revision_mode { 0 => model.revision, 1 => model.revision + 1, _ => 0 };
            let input = BalancePolicyDraft { auto_refill, warn_below, refill_to };
            let before = image(&store.conn);
            let result = store.save_balance_policy(revision, input);
            if revision <= 0 || warn_below <= 0 || warn_below >= refill_to {
                prop_assert_eq!(result.unwrap_err().code, Code::InvalidInput);
                prop_assert_eq!(image(&store.conn), before);
            } else if revision != model.revision {
                prop_assert_eq!(result.unwrap_err().code, Code::RevisionConflict);
                prop_assert_eq!(image(&store.conn), before);
            } else {
                let changed = (auto_refill, warn_below, refill_to) != (model.auto_refill, model.warn_below, model.refill_to);
                model = BalancePolicy { revision: model.revision + i64::from(changed), auto_refill, warn_below, refill_to };
                prop_assert_eq!(result.unwrap(), model);
            }
            if reopen {
                drop(store);
                store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
            }
            prop_assert_eq!(store.balance_policy().unwrap(), model);
        }
    }
}
