use super::legacy::LegacyImportPlan;
use super::types::*;
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Row, Transaction, TransactionBehavior,
};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) const APPLICATION_ID: i64 = 0x51545343;
const SCHEMA: &str = include_str!("schema_v1.sql");
const SCHEMA_V2: &str = include_str!("schema_v2.sql");
const SCHEMA_V3: &str = include_str!("schema_v3.sql");
const SCHEMA_V4: &str = include_str!("schema_v4.sql");
const SCHEMA_V5: &str = include_str!("schema_v5.sql");
const SCHEMA_V6: &str = include_str!("schema_v6.sql");
pub(super) const SOURCE_COLUMNS: &str =
    "source_id,kind,name,note,manual_paused,retired_at,config_revision";
pub(super) const GROUP_COLUMNS: &str = "group_id,newapi_group,name,kind,revision,ready";
pub(super) const MEMBER_COLUMNS: &str = "membership_id,source_id,group_id,channel_id,position,lifecycle,desired_priority,observed_status,observed_priority,applied_revision,retired_at";

impl From<rusqlite::Error> for CatalogError {
    fn from(value: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode::{DatabaseBusy, DatabaseLocked};
        if matches!(
            value,
            rusqlite::Error::InvalidColumnType(..)
                | rusqlite::Error::FromSqlConversionFailure(..)
                | rusqlite::Error::IntegralValueOutOfRange(..)
        ) {
            return error(ErrorKind::CorruptData);
        }
        error(
            if matches!(
                value.sqlite_error_code(),
                Some(DatabaseBusy | DatabaseLocked)
            ) {
                ErrorKind::Busy
            } else {
                ErrorKind::Storage
            },
        )
    }
}

/// 仅成功持锁后构造；关闭本句柄不足以释放被其他描述符继承的 flock。
pub(super) struct CatalogLock(File);

impl CatalogLock {
    fn acquire(file: File) -> Result<Self> {
        file.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => error(ErrorKind::AlreadyOpen),
            _ => error(ErrorKind::Storage),
        })?;
        Ok(Self(file))
    }
}

impl Drop for CatalogLock {
    fn drop(&mut self) {
        // 析构不能 panic；失败时仍关闭句柄，后续重开继续按实际 OS 锁结果判断。
        let _ = self.0.unlock();
    }
}

/// 字段顺序保证 SQLite 连接先于进程锁释放。不要在 Store::drop 提前解锁。
pub struct CatalogStore {
    pub(super) conn: Connection,
    pub(super) _lock: Arc<CatalogLock>,
    pub(super) path: PathBuf,
    pub(super) poisoned: bool,
}

impl CatalogStore {
    #[cfg(not(unix))]
    pub fn open(_path: &Path) -> Result<Self> {
        Err(error(ErrorKind::UnsupportedPlatform))
    }

    #[cfg(unix)]
    pub fn open(path: &Path) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        // Step P1: 显式私有目录；不 chmod 已有路径，不跟随叶端链接。
        require(
            path.is_absolute() && path.file_name().is_some_and(|v| v == "catalog.sqlite3"),
            ErrorKind::UnsafePath,
        )?;
        let parent =
            prepare_private_directory(path.parent().ok_or_else(|| error(ErrorKind::UnsafePath))?)?;
        let path = parent.join("catalog.sqlite3");
        let lock_path = parent.join(".catalog.lock");
        for file in [
            &path,
            &lock_path,
            &path.with_file_name("catalog.sqlite3-wal"),
            &path.with_file_name("catalog.sqlite3-shm"),
        ] {
            check_private_file(file, true)?;
        }
        // Step P2: 非阻塞排他锁；不截断已有数据库。
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|_| error(ErrorKind::UnsafePath))?;
        let lock = CatalogLock::acquire(lock)?;
        let new_file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(_) => return Err(error(ErrorKind::Storage)),
        };
        // Step P3: 外库/高版本在任何持久 pragma 之前拒绝。
        let mut version = 1;
        if !new_file {
            let probe = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(map_probe_error)?;
            probe.busy_timeout(Duration::from_millis(1000))?;
            let app: i64 = probe
                .pragma_query_value(None, "application_id", |r| r.get(0))
                .map_err(map_probe_error)?;
            require(app == APPLICATION_ID, ErrorKind::ForeignDatabase)?;
            version = probe.pragma_query_value(None, "user_version", |r| r.get(0))?;
            require(
                (1..=DATABASE_SCHEMA_VERSION).contains(&version),
                ErrorKind::UnsupportedSchema,
            )?;
            // Step P8: 旧库在只读事务完整验证后才备份/迁移，不初始化成空库。
            probe.execute_batch("BEGIN")?;
            verify_schema(&probe, version)?;
            verify_core_data(&probe)?;
            if version >= 2 {
                super::control::verify_control(&probe)?;
            }
            if version >= 3 {
                super::execution::verify_execution(&probe)?;
            }
            if version >= 4 {
                super::policy::verify_policy(&probe)?;
            }
            if version >= 5 {
                super::ledger::verify_ledger(&probe)?;
            }
            if version >= 6 {
                super::ledger::observed::verify_observed(&probe)?;
            }
            probe.execute_batch("ROLLBACK")?;
        }
        // Step P4: 仅初始化本次创建的库；schema 与版本标记同事务。
        let mut conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.busy_timeout(Duration::from_millis(1000))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        if new_file {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(SCHEMA)?;
            tx.execute("INSERT INTO catalog_meta VALUES(1,1)", [])?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        let mut store = Self {
            conn,
            _lock: Arc::new(lock),
            path,
            poisoned: false,
        };
        if version == 1 {
            if !new_file {
                let suffix: String =
                    store
                        .conn
                        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
                store.backup_to(&parent.join(format!("catalog-v1-before-v2-{suffix}.sqlite3")))?;
            }
            store.migrate_v1()?;
        }
        if version <= 2 {
            if !new_file && version == 2 {
                let suffix: String =
                    store
                        .conn
                        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
                store.backup_to(&parent.join(format!("catalog-v2-before-v3-{suffix}.sqlite3")))?;
            }
            store.migrate_v2()?;
        }
        if version <= 3 {
            if !new_file && version == 3 {
                let suffix: String =
                    store
                        .conn
                        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
                store.backup_to(&parent.join(format!("catalog-v3-before-v4-{suffix}.sqlite3")))?;
            }
            store.migrate_v3()?;
        }
        if version <= 4 {
            if !new_file && version == 4 {
                let suffix: String =
                    store
                        .conn
                        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
                store.backup_to(&parent.join(format!("catalog-v4-before-v5-{suffix}.sqlite3")))?;
            }
            store.migrate_v4()?;
        }
        if version <= 5 {
            if !new_file && version == 5 {
                let suffix: String =
                    store
                        .conn
                        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
                store.backup_to(&parent.join(format!("catalog-v5-before-v6-{suffix}.sqlite3")))?;
            }
            store.migrate_v5()?;
        }
        let conn = &store.conn;
        // Step P5: 配置回读及数据检查完成才发布 Store。
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        let journal: String = conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        let sync: i64 = conn.pragma_query_value(None, "synchronous", |r| r.get(0))?;
        let foreign: i64 = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
        require(
            journal == "wal" && sync == 2 && foreign == 1,
            ErrorKind::Storage,
        )?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        require(check == "ok", ErrorKind::CorruptData)?;
        let fk_errors: i64 =
            conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        require(fk_errors == 0, ErrorKind::CorruptData)?;
        verify_schema(conn, DATABASE_SCHEMA_VERSION)?;
        super::control::verify_control(conn)?;
        super::execution::verify_execution(conn)?;
        super::policy::verify_policy(conn)?;
        super::ledger::observed::verify_observed(conn)?;
        let snapshot = store.snapshot()?;
        for source in &snapshot.sources {
            store.read_material(source.source_id, true)?;
        }
        let secrets: i64 =
            store
                .conn
                .query_row("SELECT count(*) FROM source_secrets", [], |r| r.get(0))?;
        require(
            secrets == snapshot.sources.len() as i64,
            ErrorKind::CorruptData,
        )?;
        check_private_file(&store.path, false)?;
        for suffix in ["catalog.sqlite3-wal", "catalog.sqlite3-shm"] {
            check_private_file(&parent.join(suffix), true)?;
        }
        store.recover_executions()?;
        Ok(store)
    }

    pub(crate) fn usable(&self) -> Result<()> {
        require(!self.poisoned, ErrorKind::CommitUnknown)
    }

    fn migrate_v1(&mut self) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DROP TABLE catalog_meta", [])?;
        tx.execute_batch(SCHEMA_V2)?;
        tx.execute(
            "INSERT INTO catalog_meta VALUES(1,2,lower(hex(randomblob(16))))",
            [],
        )?;
        tx.execute("INSERT INTO source_control(source_id,retire_requested) SELECT source_id,0 FROM sources",[])?;
        tx.execute("INSERT INTO member_control(membership_id,revision,status_directive,retire_requested,create_state,create_name,dirty,next_attempt_at) SELECT membership_id,1,'preserve',0,CASE WHEN channel_id IS NULL THEN 'never' ELSE 'resolved' END,'qt-'||(SELECT catalog_instance_id FROM catalog_meta)||'-m'||membership_id,0,0 FROM memberships",[])?;
        tx.pragma_update(None, "user_version", 2)?;
        verify_schema(&tx, 2)?;
        super::control::verify_control(&tx)?;
        commit(tx, &mut self.poisoned)
    }

    fn migrate_v2(&mut self) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let instance: String = tx.query_row(
            "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        tx.execute("DROP TABLE catalog_meta", [])?;
        tx.execute_batch(SCHEMA_V3)?;
        tx.execute("INSERT INTO catalog_meta VALUES(1,3,?1)", [instance])?;
        tx.execute("INSERT INTO member_execution SELECT m.membership_id,0,CASE WHEN c.status_directive='enabled' THEN 1 ELSE 0 END,CASE WHEN c.status_directive='enabled' THEN c.revision ELSE 0 END,CASE WHEN c.status_directive='enabled' AND m.applied_revision=c.revision THEN 1 ELSE 0 END FROM memberships m JOIN member_control c USING(membership_id)",[])?;
        tx.pragma_update(None, "user_version", 3)?;
        verify_schema(&tx, 3)?;
        super::execution::verify_execution(&tx)?;
        commit(tx, &mut self.poisoned)
    }

    fn migrate_v3(&mut self) -> Result<()> {
        // Step B8: 旧库只读验证及原版本备份后，一事务追加状态并保留实例身份。
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let instance: String = tx.query_row(
            "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        tx.execute("DROP TABLE catalog_meta", [])?;
        tx.execute_batch(SCHEMA_V4)?;
        tx.execute("INSERT INTO catalog_meta VALUES(1,4,?1)", [instance])?;
        tx.execute("INSERT INTO volcano_runtime(source_id) SELECT source_id FROM sources WHERE kind='volcano'", [])?;
        tx.execute("INSERT INTO group_policy_state(group_id,kind,regime) SELECT group_id,kind,CASE WHEN kind='glm' THEN 'normal' END FROM source_groups", [])?;
        tx.pragma_update(None, "user_version", 4)?;
        verify_schema(&tx, 4)?;
        super::policy::verify_policy(&tx)?;
        commit(tx, &mut self.poisoned)
    }

    fn migrate_v4(&mut self) -> Result<()> {
        // Step B1.1: 旧版本验证/备份先完成；事实表与版本在同一事务初始化。
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let instance: String = tx.query_row(
            "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        tx.execute("DROP TABLE catalog_meta", [])?;
        tx.execute_batch(SCHEMA_V5)?;
        tx.execute("INSERT INTO catalog_meta VALUES(1,5,?1)", [instance])?;
        tx.execute("INSERT INTO ledger_state VALUES(1,1,NULL)", [])?;
        tx.execute("INSERT INTO ledger_checkpoints VALUES('recent',0,NULL,NULL,1,'idle',NULL,NULL),('history',0,NULL,NULL,1,'idle',NULL,NULL)", [])?;
        tx.pragma_update(None, "user_version", 5)?;
        verify_schema(&tx, 5)?;
        super::ledger::verify_ledger(&tx)?;
        commit(tx, &mut self.poisoned)
    }

    fn migrate_v5(&mut self) -> Result<()> {
        // Step O4.0: 队列/相位/历史与版本一起建立，不回放旧错误或清旧状态。
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let instance: String = tx.query_row(
            "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        tx.execute("DROP TABLE catalog_meta", [])?;
        tx.execute_batch(SCHEMA_V6)?;
        tx.execute("INSERT INTO catalog_meta VALUES(1,6,?1)", [instance])?;
        tx.pragma_update(None, "user_version", 6)?;
        verify_schema(&tx, 6)?;
        super::ledger::observed::verify_observed(&tx)?;
        commit(tx, &mut self.poisoned)
    }

    pub fn create_source(&mut self, draft: SourceDraft) -> Result<SourceView> {
        self.usable()?;
        let source = validate_source(draft)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let view = insert_source(&tx, &source)?;
        commit(tx, &mut self.poisoned)?;
        Ok(view)
    }
    pub fn create_group(&mut self, draft: GroupDraft) -> Result<GroupView> {
        self.usable()?;
        let group = validate_group(draft)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let view = insert_group(&tx, &group)?;
        commit(tx, &mut self.poisoned)?;
        Ok(view)
    }
    pub fn plan_membership(
        &mut self,
        source: SourceId,
        group: GroupId,
        standby_priority: i64,
    ) -> Result<MembershipView> {
        self.usable()?;
        require(safe_number(standby_priority), ErrorKind::InvalidInput)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let kind = member_parents(&tx, source, group)?;
        let max: Option<i64> = tx.query_row(
            "SELECT max(position) FROM memberships WHERE group_id=?1",
            [group.get()],
            |r| r.get(0),
        )?;
        let position = match max {
            None => 0,
            Some(v) => v
                .checked_add(1)
                .filter(|v| *v <= JSON_SAFE)
                .ok_or_else(|| error(ErrorKind::InvalidInput))?,
        };
        let view = insert_membership(
            &tx,
            &NewMembership {
                source,
                group,
                kind,
                channel: None,
                position,
                lifecycle: MembershipLifecycle::Planned,
                priority: standby_priority,
            },
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(view)
    }

    pub fn import_legacy(&mut self, plan: &LegacyImportPlan) -> Result<ImportOutcome> {
        self.usable()?;
        // Step P11: 相同摘要重放收据，不回写可能已被人工修改的实体。
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT digest,receipt_json FROM legacy_import WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((digest, json)) = existing {
            require(digest == plan.digest, ErrorKind::ImportConflict)?;
            let receipt: ImportReceipt =
                serde_json::from_str(&json).map_err(|_| error(ErrorKind::CorruptData))?;
            verify_receipt(&tx, &receipt)?;
            return Ok(ImportOutcome::AlreadyApplied(receipt));
        }
        // Step P12: 首次导入仅限空目录，不默默并入用户新数据。
        let count: i64 = tx.query_row("SELECT (SELECT count(*) FROM sources)+(SELECT count(*) FROM source_groups)+(SELECT count(*) FROM memberships)+(SELECT count(*) FROM source_secrets)", [], |r| r.get(0))?;
        require(count == 0, ErrorKind::ImportRequiresEmpty)?;
        let group = insert_group(&tx, &plan.group)?;
        let mut receipt = ImportReceipt {
            group_id: group.group_id,
            source_ids: Vec::new(),
            membership_ids: Vec::new(),
        };
        // Step P13: 顺序与 ID 保留；imported 明确表示未核对。
        for (position, (source, channel, priority)) in plan.sources.iter().enumerate() {
            let source = insert_source(&tx, source)?;
            let member = insert_membership(
                &tx,
                &NewMembership {
                    source: source.source_id,
                    group: group.group_id,
                    kind: group.kind,
                    channel: Some(*channel),
                    position: position as i64,
                    lifecycle: MembershipLifecycle::Imported,
                    priority: *priority,
                },
            )?;
            receipt.source_ids.push(source.source_id);
            receipt.membership_ids.push(member.membership_id);
        }
        // Step P14: 标记与实体必须在同一提交点。
        let json = serde_json::to_string(&receipt).map_err(|_| error(ErrorKind::Storage))?;
        tx.execute(
            "INSERT INTO legacy_import VALUES(1,?1,?2)",
            params![plan.digest, json],
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(ImportOutcome::Applied(receipt))
    }

    pub fn snapshot(&mut self) -> Result<CatalogSnapshot> {
        self.usable()?;
        let tx = self.conn.transaction()?;
        let snapshot = read_snapshot(&tx)?;
        // 只读事务结束不提交写入，不可能生成新的外部效果。
        tx.rollback()?;
        Ok(snapshot)
    }
    pub fn read_source_material(&self, id: SourceId) -> Result<SourceMaterial> {
        self.read_material(id, false)
    }
    fn read_material(&self, id: SourceId, allow_retired: bool) -> Result<SourceMaterial> {
        self.usable()?;
        read_material(&self.conn, id, allow_retired)
    }

    #[cfg(not(unix))]
    pub fn backup_to(&self, _destination: &Path) -> Result<()> {
        Err(error(ErrorKind::UnsupportedPlatform))
    }

    #[cfg(unix)]
    pub fn backup_to(&self, destination: &Path) -> Result<()> {
        use rusqlite::backup::{Backup, StepResult};
        use std::os::unix::fs::OpenOptionsExt;
        self.usable()?;
        require(destination.is_absolute(), ErrorKind::UnsafePath)?;
        let parent = prepare_private_directory(
            destination
                .parent()
                .ok_or_else(|| error(ErrorKind::UnsafePath))?,
        )?;
        let target = parent.join(
            destination
                .file_name()
                .ok_or_else(|| error(ErrorKind::UnsafePath))?,
        );
        require(target != self.path, ErrorKind::BackupExists)?;
        // 旧 sidecar 也属于用户已有文件，不能让 SQLite 覆盖或恢复它们。
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = target.as_os_str().to_os_string();
            sidecar.push(suffix);
            match fs::symlink_metadata(PathBuf::from(sidecar)) {
                Ok(_) => return Err(error(ErrorKind::BackupExists)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err(error(ErrorKind::UnsafePath)),
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)
            .map_err(|e| {
                error(if e.kind() == std::io::ErrorKind::AlreadyExists {
                    ErrorKind::BackupExists
                } else {
                    ErrorKind::BackupFailed
                })
            })?;
        let identity = file.metadata().map_err(|_| CatalogError {
            incomplete_backup: true,
            ..error(ErrorKind::BackupFailed)
        })?;
        let result = (|| -> Result<()> {
            let mut dest = Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            dest.busy_timeout(Duration::from_millis(0))?;
            {
                let backup = Backup::new(&self.conn, &mut dest)?;
                let start = Instant::now();
                let mut waits = 0;
                let mut done = false;
                for _ in 0..10000 {
                    require(
                        start.elapsed() < Duration::from_secs(10),
                        ErrorKind::BackupFailed,
                    )?;
                    match backup.step(128)? {
                        StepResult::Done => {
                            done = true;
                            break;
                        }
                        StepResult::Busy | StepResult::Locked => {
                            waits += 1;
                            require(waits <= 100, ErrorKind::BackupFailed)?;
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        StepResult::More => (),
                        _ => return Err(error(ErrorKind::BackupFailed)),
                    }
                }
                require(done, ErrorKind::BackupFailed)?;
            }
            dest.close().map_err(|_| error(ErrorKind::BackupFailed))?;
            file.sync_all()
                .map_err(|_| error(ErrorKind::BackupFailed))?;
            File::open(&parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| error(ErrorKind::BackupFailed))?;
            Ok(())
        })();
        if result.is_err() {
            return Err(cleanup_failed_backup(&target, &identity, |p| {
                fs::remove_file(p)
            }));
        }
        Ok(())
    }
}

pub(super) fn read_snapshot(conn: &Connection) -> Result<CatalogSnapshot> {
    let snapshot = CatalogSnapshot {
        schema_version: CATALOG_SNAPSHOT_VERSION,
        sources: read_rows(
            conn,
            &format!("SELECT {SOURCE_COLUMNS} FROM sources ORDER BY source_id"),
            MAX_SOURCES,
            decode_source_view,
        )?,
        groups: read_rows(
            conn,
            &format!("SELECT {GROUP_COLUMNS} FROM source_groups ORDER BY group_id"),
            MAX_GROUPS,
            decode_group_view,
        )?,
        memberships: read_rows(
            conn,
            &format!("SELECT {MEMBER_COLUMNS} FROM memberships ORDER BY membership_id"),
            MAX_MEMBERSHIPS,
            decode_membership_view,
        )?,
    };
    verify_snapshot(&snapshot)?;
    Ok(snapshot)
}
pub(super) fn read_material(
    conn: &Connection,
    id: SourceId,
    allow_retired: bool,
) -> Result<SourceMaterial> {
    let row: Option<(String, String, String, String, Option<i64>)> = conn.query_row(
            "SELECT s.kind,s.settings_json,k.raw_key,s.credential_fingerprint,s.retired_at FROM sources s JOIN source_secrets k ON s.secret_id=k.secret_id WHERE s.source_id=?1",
            [id.get()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let (kind, json, raw_key, hash, retired) = row.ok_or_else(|| error(ErrorKind::NotFound))?;
    require(allow_retired || retired.is_none(), ErrorKind::Retired)?;
    let settings = decode_settings(&json)?;
    let credential = SecretString::new(raw_key).map_err(|_| error(ErrorKind::CorruptData))?;
    require(
        settings.kind().as_str() == kind && fingerprint(&credential) == hash,
        ErrorKind::CorruptData,
    )?;
    Ok(SourceMaterial {
        settings,
        credential,
    })
}

#[cfg(unix)]
fn cleanup_failed_backup(
    target: &Path,
    identity: &fs::Metadata,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> CatalogError {
    use std::os::unix::fs::MetadataExt;
    let incomplete = match fs::symlink_metadata(target) {
        Ok(current)
            if current.is_file()
                && current.dev() == identity.dev()
                && current.ino() == identity.ino() =>
        {
            remove(target).is_err()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        _ => true, // 身份变更或无法检查时，不删除不确定的目标。
    };
    CatalogError {
        incomplete_backup: incomplete,
        ..error(ErrorKind::BackupFailed)
    }
}

pub(super) fn commit(tx: Transaction<'_>, poisoned: &mut bool) -> Result<()> {
    match tx.commit() {
        Ok(()) => Ok(()),
        Err(e) => {
            // IO/未知错误保守处理；busy 的事务已由 RAII 回滚。
            let e = CatalogError::from(e);
            if e.kind == ErrorKind::Busy {
                Err(e)
            } else {
                *poisoned = true;
                Err(error(ErrorKind::CommitUnknown))
            }
        }
    }
}

#[cfg(unix)]
fn prepare_private_directory(path: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    require(path.is_absolute(), ErrorKind::UnsafePath)?;
    match fs::symlink_metadata(path) {
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|_| error(ErrorKind::UnsafePath))?;
        }
        Err(_) => return Err(error(ErrorKind::UnsafePath)),
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| error(ErrorKind::UnsafePath))?;
    require(
        metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
        ErrorKind::UnsafePath,
    )?;
    path.canonicalize()
        .map_err(|_| error(ErrorKind::UnsafePath))
}
#[cfg(unix)]
pub(super) fn check_private_file(path: &Path, allow_missing: bool) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    match fs::symlink_metadata(path) {
        Ok(m) => require(
            m.is_file() && m.nlink() == 1 && m.permissions().mode() & 0o177 == 0,
            ErrorKind::UnsafePath,
        ),
        Err(e) if allow_missing && e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(error(ErrorKind::UnsafePath)),
    }
}
fn map_probe_error(err: rusqlite::Error) -> CatalogError {
    if err.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase) {
        error(ErrorKind::ForeignDatabase)
    } else {
        err.into()
    }
}

fn verify_schema(conn: &Connection, expected_version: i64) -> Result<()> {
    let version: i64 = conn
        .query_row(
            "SELECT schema_version FROM catalog_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| error(ErrorKind::CorruptData))?;
    require(version == expected_version, ErrorKind::CorruptData)?;
    // 比较本版本 DDL，检测被删除/弱化的索引、FK、CHECK，不自动补表。
    let statements = SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|s| {
            !(s.is_empty() || expected_version >= 2 && s.starts_with("CREATE TABLE catalog_meta"))
        })
        .chain(SCHEMA_V2.split(';').map(str::trim).filter(|s| {
            expected_version >= 2
                && !s.is_empty()
                && !(expected_version >= 3 && s.starts_with("CREATE TABLE catalog_meta"))
        }))
        .chain(SCHEMA_V3.split(';').map(str::trim).filter(|s| {
            expected_version >= 3
                && !s.is_empty()
                && !(expected_version >= 4 && s.starts_with("CREATE TABLE catalog_meta"))
        }))
        .chain(SCHEMA_V4.split(';').map(str::trim).filter(|s| {
            expected_version >= 4
                && !s.is_empty()
                && !(expected_version >= 5 && s.starts_with("CREATE TABLE catalog_meta"))
        }))
        .chain(SCHEMA_V5.split(';').map(str::trim).filter(|s| {
            expected_version >= 5
                && !s.is_empty()
                && !(expected_version >= 6 && s.starts_with("CREATE TABLE catalog_meta"))
        }))
        .chain(
            SCHEMA_V6
                .split(';')
                .map(str::trim)
                .filter(|s| expected_version >= 6 && !s.is_empty()),
        );
    for statement in statements {
        let tail = statement
            .strip_prefix("CREATE TABLE ")
            .or_else(|| statement.strip_prefix("CREATE UNIQUE INDEX "))
            .or_else(|| statement.strip_prefix("CREATE INDEX "))
            .ok_or_else(|| error(ErrorKind::CorruptData))?;
        let name = tail
            .split([' ', '('])
            .next()
            .ok_or_else(|| error(ErrorKind::CorruptData))?;
        let actual: Option<String> = conn
            .query_row("SELECT sql FROM sqlite_master WHERE name=?1", [name], |r| {
                r.get(0)
            })
            .optional()?;
        require(
            actual.as_deref().is_some_and(|v| v.trim() == statement),
            ErrorKind::CorruptData,
        )?;
    }
    Ok(())
}

fn verify_core_data(conn: &Connection) -> Result<()> {
    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    require(check == "ok", ErrorKind::CorruptData)?;
    let fk: i64 = conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
        r.get(0)
    })?;
    require(fk == 0, ErrorKind::CorruptData)?;
    let snapshot = read_snapshot(conn)?;
    for source in &snapshot.sources {
        read_material(conn, source.source_id, true)?;
    }
    let secrets: i64 = conn.query_row("SELECT count(*) FROM source_secrets", [], |r| r.get(0))?;
    require(
        secrets == snapshot.sources.len() as i64,
        ErrorKind::CorruptData,
    )?;
    let receipt: Option<(String, String)> = conn
        .query_row(
            "SELECT digest,receipt_json FROM legacy_import WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((digest, receipt)) = receipt {
        require(
            digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit()),
            ErrorKind::CorruptData,
        )?;
        let receipt = serde_json::from_str(&receipt).map_err(|_| error(ErrorKind::CorruptData))?;
        verify_receipt(conn, &receipt)?;
    }
    Ok(())
}
fn capacity(tx: &Transaction<'_>, table: &'static str, limit: usize) -> Result<()> {
    // table 只来自本模块常量，不接收用户文本。
    let count: i64 = tx.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
    require(count < limit as i64, ErrorKind::InvalidInput)
}
fn insert_source(tx: &Transaction<'_>, source: &ValidatedSource) -> Result<SourceView> {
    let existing: Option<i64> = tx
        .query_row(
            "SELECT source_id FROM sources WHERE credential_fingerprint=?1",
            [&source.fingerprint],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Err(CatalogError {
            kind: ErrorKind::DuplicateSource,
            source_id: Some(SourceId::new(id)?),
            incomplete_backup: false,
        });
    }
    // 重复身份不新增记录，不能让容量限制遮盖原 SourceId。
    capacity(tx, "sources", MAX_SOURCES)?;
    tx.execute(
        "INSERT INTO source_secrets(raw_key) VALUES(?1)",
        [source.draft.credential.expose()],
    )?;
    let secret = SecretRef(tx.last_insert_rowid());
    let json = encode_settings(&source.draft.settings)?;
    let kind = source.draft.settings.kind();
    tx.execute("INSERT INTO sources(kind,name,note,secret_id,credential_fingerprint,settings_json,config_revision,manual_paused) VALUES(?1,?2,?3,?4,?5,?6,1,0)",
        params![kind.as_str(), source.draft.name, source.draft.note, secret.0, source.fingerprint, json])?;
    let source_id = SourceId::new(tx.last_insert_rowid())?;
    tx.execute(
        "INSERT INTO source_control(source_id,retire_requested) VALUES(?1,0)",
        [source_id.get()],
    )?;
    if kind == SourceKind::Volcano {
        tx.execute(
            "INSERT INTO volcano_runtime(source_id) VALUES(?1)",
            [source_id.get()],
        )?;
    }
    Ok(SourceView {
        source_id,
        kind,
        name: source.draft.name.clone(),
        note: source.draft.note.clone(),
        manual_paused: false,
        retired_at: None,
        config_revision: 1,
    })
}
fn insert_group(tx: &Transaction<'_>, group: &ValidatedGroup) -> Result<GroupView> {
    let g = &group.0;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM source_groups WHERE newapi_group=?1)",
        [&g.newapi_group],
        |r| r.get(0),
    )?;
    require(!exists, ErrorKind::DuplicateGroup)?;
    capacity(tx, "source_groups", MAX_GROUPS)?;
    tx.execute(
        "INSERT INTO source_groups(newapi_group,name,kind,revision,ready) VALUES(?1,?2,?3,1,0)",
        params![g.newapi_group, g.name, g.kind.as_str()],
    )?;
    let group_id = GroupId::new(tx.last_insert_rowid())?;
    tx.execute(
        "INSERT INTO group_policy_state(group_id,kind,regime) VALUES(?1,?2,?3)",
        params![
            group_id.get(),
            g.kind.as_str(),
            if g.kind == SourceKind::Glm {
                Some("normal")
            } else {
                None
            }
        ],
    )?;
    Ok(GroupView {
        group_id,
        newapi_group: g.newapi_group.clone(),
        name: g.name.clone(),
        kind: g.kind,
        revision: 1,
        ready: false,
    })
}
fn member_parents(tx: &Transaction<'_>, source: SourceId, group: GroupId) -> Result<SourceKind> {
    let s: Option<(String, Option<i64>)> = tx
        .query_row(
            "SELECT kind,retired_at FROM sources WHERE source_id=?1",
            [source.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (kind, retired) = s.ok_or_else(|| error(ErrorKind::NotFound))?;
    let g: Option<String> = tx
        .query_row(
            "SELECT kind FROM source_groups WHERE group_id=?1",
            [group.get()],
            |r| r.get(0),
        )
        .optional()?;
    let g = g.ok_or_else(|| error(ErrorKind::NotFound))?;
    require(retired.is_none(), ErrorKind::Retired)?;
    let retiring: bool = tx.query_row(
        "SELECT retire_requested FROM source_control WHERE source_id=?1",
        [source.get()],
        |r| r.get(0),
    )?;
    require(!retiring, ErrorKind::Retired)?;
    require(kind == g, ErrorKind::TypeMismatch)?;
    SourceKind::parse(&kind)
}
struct NewMembership {
    source: SourceId,
    group: GroupId,
    kind: SourceKind,
    channel: Option<ChannelId>,
    position: i64,
    lifecycle: MembershipLifecycle,
    priority: i64,
}
fn insert_membership(tx: &Transaction<'_>, row: &NewMembership) -> Result<MembershipView> {
    require(
        safe_number(row.priority)
            && (0..=JSON_SAFE).contains(&row.position)
            && matches!(
                (row.lifecycle, row.channel),
                (MembershipLifecycle::Planned, None) | (MembershipLifecycle::Imported, Some(_))
            ),
        ErrorKind::InvalidInput,
    )?;
    require(
        member_parents(tx, row.source, row.group)? == row.kind,
        ErrorKind::TypeMismatch,
    )?;
    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM memberships WHERE source_id=?1 AND group_id=?2 AND lifecycle!='retired')", params![row.source.get(), row.group.get()], |r| r.get(0))?;
    require(!exists, ErrorKind::DuplicateMembership)?;
    if let Some(channel) = row.channel {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memberships WHERE channel_id=?1)",
            [channel.get()],
            |r| r.get(0),
        )?;
        require(!exists, ErrorKind::ChannelConflict)?;
    }
    capacity(tx, "memberships", MAX_MEMBERSHIPS)?;
    tx.execute("INSERT INTO memberships(source_id,group_id,kind,channel_id,position,lifecycle,desired_priority) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![row.source.get(),row.group.get(),row.kind.as_str(),row.channel.map(ChannelId::get),row.position,row.lifecycle.as_str(),row.priority])?;
    let membership_id = MembershipId::new(tx.last_insert_rowid())?;
    tx.execute("INSERT INTO member_control(membership_id,revision,status_directive,retire_requested,create_state,create_name,dirty,next_attempt_at) VALUES(?1,1,'preserve',0,?2,'qt-'||(SELECT catalog_instance_id FROM catalog_meta)||'-m'||?1,0,0)",params![membership_id.get(),if row.channel.is_some(){"resolved"}else{"never"}])?;
    tx.execute(
        "INSERT INTO member_execution VALUES(?1,0,0,0,0)",
        [membership_id.get()],
    )?;
    Ok(MembershipView {
        membership_id,
        source_id: row.source,
        group_id: row.group,
        channel_id: row.channel,
        order: row.position,
        lifecycle: row.lifecycle,
        desired_priority: row.priority,
        observed_status: None,
        observed_priority: None,
        applied_revision: None,
        retired_at: None,
    })
}
fn field<T: rusqlite::types::FromSql>(row: &Row<'_>, name: &str) -> Result<T> {
    row.get(name).map_err(|_| error(ErrorKind::CorruptData))
}
pub(super) fn decode_source_view(row: &Row<'_>) -> Result<SourceView> {
    let paused: i64 = field(row, "manual_paused")?;
    let s = SourceView {
        source_id: SourceId::new(field(row, "source_id")?)
            .map_err(|_| error(ErrorKind::CorruptData))?,
        kind: SourceKind::parse(&field::<String>(row, "kind")?)?,
        name: field(row, "name")?,
        note: field(row, "note")?,
        manual_paused: paused == 1,
        retired_at: field(row, "retired_at")?,
        config_revision: field(row, "config_revision")?,
    };
    require(
        matches!(paused, 0 | 1)
            && valid_name(&s.name)
            && s.note.chars().count() <= 2048
            && !s.note.contains('\0')
            && valid_timestamp(s.retired_at)
            && positive_revision(s.config_revision),
        ErrorKind::CorruptData,
    )?;
    Ok(s)
}
pub(super) fn decode_group_view(row: &Row<'_>) -> Result<GroupView> {
    let ready: i64 = field(row, "ready")?;
    let g = GroupView {
        group_id: GroupId::new(field(row, "group_id")?)
            .map_err(|_| error(ErrorKind::CorruptData))?,
        newapi_group: field(row, "newapi_group")?,
        name: field(row, "name")?,
        kind: SourceKind::parse(&field::<String>(row, "kind")?)?,
        revision: field(row, "revision")?,
        ready: ready == 1,
    };
    require(
        matches!(ready, 0 | 1)
            && valid_name(&g.name)
            && valid_group(&g.newapi_group)
            && positive_revision(g.revision),
        ErrorKind::CorruptData,
    )?;
    Ok(g)
}
pub(super) fn decode_membership_view(row: &Row<'_>) -> Result<MembershipView> {
    let decode = || -> Result<MembershipView> {
        Ok(MembershipView {
            membership_id: MembershipId::new(field(row, "membership_id")?)?,
            source_id: SourceId::new(field(row, "source_id")?)?,
            group_id: GroupId::new(field(row, "group_id")?)?,
            channel_id: field::<Option<i64>>(row, "channel_id")?
                .map(ChannelId::new)
                .transpose()?,
            order: field(row, "position")?,
            lifecycle: MembershipLifecycle::parse(&field::<String>(row, "lifecycle")?)?,
            desired_priority: field(row, "desired_priority")?,
            observed_status: field(row, "observed_status")?,
            observed_priority: field(row, "observed_priority")?,
            applied_revision: field(row, "applied_revision")?,
            retired_at: field(row, "retired_at")?,
        })
    };
    let m = decode().map_err(|_| error(ErrorKind::CorruptData))?;
    require(
        (0..=JSON_SAFE).contains(&m.order)
            && safe_number(m.desired_priority)
            && m.observed_status.is_none_or(|s| matches!(s, 1..=3))
            && m.observed_priority.is_none_or(safe_number)
            && m.applied_revision.is_none_or(positive_revision)
            && valid_timestamp(m.retired_at)
            && (m.lifecycle == MembershipLifecycle::Retired) == m.retired_at.is_some()
            && match m.lifecycle {
                MembershipLifecycle::Planned => m.channel_id.is_none(),
                MembershipLifecycle::Retired => true,
                _ => m.channel_id.is_some(),
            },
        ErrorKind::CorruptData,
    )?;
    Ok(m)
}
fn read_rows<T>(
    conn: &Connection,
    sql: &str,
    limit: usize,
    decode: fn(&Row<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        require(out.len() < limit, ErrorKind::CorruptData)?;
        out.push(decode(row)?);
    }
    Ok(out)
}
fn verify_snapshot(snapshot: &CatalogSnapshot) -> Result<()> {
    let sources: HashMap<_, _> = snapshot
        .sources
        .iter()
        .map(|s| (s.source_id, s.kind))
        .collect();
    let groups: HashMap<_, _> = snapshot
        .groups
        .iter()
        .map(|g| (g.group_id, g.kind))
        .collect();
    let mut channels = HashSet::new();
    let mut pairs = HashSet::new();
    let mut positions = HashSet::new();
    for m in &snapshot.memberships {
        let kind = sources
            .get(&m.source_id)
            .ok_or_else(|| error(ErrorKind::CorruptData))?;
        require(
            groups.get(&m.group_id) == Some(kind),
            ErrorKind::CorruptData,
        )?;
        if let Some(channel) = m.channel_id {
            require(channels.insert(channel), ErrorKind::CorruptData)?;
        }
        if m.lifecycle != MembershipLifecycle::Retired {
            require(
                pairs.insert((m.source_id, m.group_id)) && positions.insert((m.group_id, m.order)),
                ErrorKind::CorruptData,
            )?;
        }
    }
    Ok(())
}
fn verify_receipt(conn: &Connection, receipt: &ImportReceipt) -> Result<()> {
    require(
        !receipt.source_ids.is_empty()
            && receipt.source_ids.len() == receipt.membership_ids.len()
            && receipt.source_ids.len() <= MAX_SOURCES,
        ErrorKind::CorruptData,
    )?;
    let mut sources = HashSet::new();
    let mut members = HashSet::new();
    for (source, member) in receipt.source_ids.iter().zip(&receipt.membership_ids) {
        require(
            sources.insert(source) && members.insert(member),
            ErrorKind::CorruptData,
        )?;
        let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM memberships m JOIN sources s ON s.source_id=m.source_id JOIN source_groups g ON g.group_id=m.group_id WHERE m.membership_id=?1 AND s.source_id=?2 AND g.group_id=?3)", params![member.get(),source.get(),receipt.group_id.get()], |r| r.get(0))?;
        require(exists, ErrorKind::CorruptData)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "tests.rs"]
mod tests;
