//! 同一应用库上的有界只读快照；只读连接存活期间继承原持锁对象。
use super::*;
use crate::catalog::store::{CatalogLock, APPLICATION_ID};
use crate::catalog::DATABASE_SCHEMA_VERSION;
use crate::ledger::analytics::*;
use rusqlite::OpenFlags;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct UsageReadTarget {
    path: PathBuf,
    instance: String,
    _lock: Arc<CatalogLock>,
}
// 字段按连接、target 声明：正常/异常退出都先关闭 SQLite，再释放 guard。
struct UsageReader {
    conn: Connection,
    target: UsageReadTarget,
}
impl CatalogStore {
    pub(crate) fn usage_read_target(&self) -> AnalyticsResult<UsageReadTarget> {
        self.usable()?;
        Ok(UsageReadTarget {
            path: self.path.clone(),
            instance: instance(&self.conn)?,
            _lock: self._lock.clone(),
        })
    }
}
impl UsageReadTarget {
    #[cfg(test)]
    pub(crate) fn test_query_observed(
        self,
        query: UsageQuery,
        control: &QueryControl,
        mut step: impl FnMut(usize),
    ) -> AnalyticsResult<UsageSnapshot> {
        self.query_inner(query, control, MAX_QUERY_ROWS, &mut step)
    }
    #[cfg(unix)]
    fn open(self, control: &QueryControl) -> AnalyticsResult<UsageReader> {
        use std::os::unix::fs::PermissionsExt;
        control.check()?;
        let unsafe_path = || AnalyticsError::from(error(ErrorKind::UnsafePath));
        let parent = self.path.parent().ok_or_else(unsafe_path)?;
        let metadata = std::fs::symlink_metadata(parent).map_err(|_| unsafe_path())?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(unsafe_path());
        }
        super::super::store::check_private_file(&self.path, false)?;
        super::super::store::check_private_file(&parent.join(".catalog.lock"), false)?;
        for suffix in ["catalog.sqlite3-wal", "catalog.sqlite3-shm"] {
            super::super::store::check_private_file(&parent.join(suffix), true)?;
        }
        let conn = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(Duration::ZERO)?;
        control.register_interrupt(conn.get_interrupt_handle())?;
        conn.pragma_update(None, "query_only", true)?;
        for suffix in ["catalog.sqlite3-wal", "catalog.sqlite3-shm"] {
            super::super::store::check_private_file(&parent.join(suffix), true)?;
        }
        control.check()?;
        Ok(UsageReader { conn, target: self })
    }
    #[cfg(not(unix))]
    fn open(self, _: &QueryControl) -> AnalyticsResult<UsageReader> {
        Err(error(ErrorKind::UnsupportedPlatform).into())
    }
    pub(crate) fn query(
        self,
        query: UsageQuery,
        control: &QueryControl,
    ) -> AnalyticsResult<UsageSnapshot> {
        self.query_inner(query, control, MAX_QUERY_ROWS, &mut |_| {})
    }
    fn query_inner(
        self,
        query: UsageQuery,
        control: &QueryControl,
        limit: usize,
        step: &mut impl FnMut(usize),
    ) -> AnalyticsResult<UsageSnapshot> {
        let result = self
            .open(control)
            .and_then(|reader| reader.read(query, control, limit, step));
        // SQLite interrupt 不带用户正文；取消/截止以本请求上下文为准。
        control.check()?;
        result
    }
    #[cfg(test)]
    pub(crate) fn test_query(
        self,
        query: UsageQuery,
        control: &QueryControl,
        limit: usize,
        mut step: impl FnMut(),
    ) -> AnalyticsResult<UsageSnapshot> {
        self.query_inner(query, control, limit, &mut |_| step())
    }
}
impl UsageReader {
    fn read(
        self,
        query: UsageQuery,
        control: &QueryControl,
        limit: usize,
        step: &mut impl FnMut(usize),
    ) -> AnalyticsResult<UsageSnapshot> {
        // Step B3.3: 一个 WAL 读事务覆盖状态、完整分钟、秒级边缘与全部分项。
        self.conn.execute_batch("BEGIN DEFERRED")?;
        let app: i64 = self
            .conn
            .pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        let journal: String = self
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        if app != APPLICATION_ID {
            return Err(error(ErrorKind::ForeignDatabase).into());
        }
        if version != DATABASE_SCHEMA_VERSION {
            return Err(error(ErrorKind::UnsupportedSchema).into());
        }
        if journal != "wal" || instance(&self.conn)? != self.target.instance {
            return Err(corrupt().into());
        }
        let state = state(&self.conn)?;
        step(0);
        control.check()?;
        if state.initial_requested_from.is_none() {
            self.conn.execute_batch("ROLLBACK")?;
            return Ok(UsageSnapshot {
                query,
                state,
                usage: None,
            });
        }
        let partition = query.partition();
        let mut acc = UsageAccumulator::new(query.clone(), control)?;
        let mut scanned = 0;
        if let Some(window) = partition.whole {
            self.scan(
                &query,
                window,
                true,
                &mut acc,
                control,
                &mut scanned,
                limit,
                step,
            )?;
        }
        for window in partition.edges {
            self.scan(
                &query,
                window,
                false,
                &mut acc,
                control,
                &mut scanned,
                limit,
                step,
            )?;
        }
        let snapshot = acc.finish(state, control)?;
        self.conn.execute_batch("ROLLBACK")?;
        control.check()?;
        Ok(snapshot)
    }
    #[allow(clippy::too_many_arguments)]
    fn scan(
        &self,
        query: &UsageQuery,
        window: ScanWindow,
        cells: bool,
        acc: &mut UsageAccumulator,
        control: &QueryControl,
        scanned: &mut usize,
        limit: usize,
        step: &mut impl FnMut(usize),
    ) -> AnalyticsResult<()> {
        let token = match query.filter().token {
            DimensionFilter::Any => None,
            DimensionFilter::Unknown => Some(None),
            DimensionFilter::Exact(id) => Some(Some(id)),
        };
        let sql = [
            page_sql(cells, token.is_some(), false),
            page_sql(cells, token.is_some(), true),
        ];
        let mut cursor = (window.from - 1, i64::MAX);
        let mut same_time = false;
        loop {
            control.check()?;
            let take = QUERY_PAGE_ROWS.min(limit.saturating_sub(*scanned) + 1);
            let mut statement = self.conn.prepare(&sql[usize::from(same_time)])?;
            let mut records = statement.query(params![
                window.from,
                window.to,
                cursor.0,
                cursor.1,
                token.flatten(),
                take as i64
            ])?;
            let mut count = 0;
            while let Some(row) = records.next()? {
                control.check()?;
                if *scanned == limit {
                    return Err(AnalyticsError::LimitExceeded);
                }
                *scanned += 1;
                count += 1;
                if cells {
                    let (key, totals) = rows::cell_row(row)?;
                    cursor = (key.minute, row.get(10)?);
                    acc.push(
                        UsageDimensions {
                            source: key.source_id,
                            group: key.group_id,
                            token: key.token_id,
                            model: key.model.as_deref(),
                        },
                        key.minute,
                        totals,
                        control,
                    )?;
                } else {
                    let fact = rows::fact_row(row)?;
                    let native = &fact.native;
                    cursor = (native.occurred_at, native.log_id);
                    if native.kind == FactKind::Consume {
                        acc.push(
                            UsageDimensions {
                                source: fact.source_id,
                                group: fact.group_id,
                                token: native.token_id,
                                model: native.model.as_deref(),
                            },
                            native.occurred_at,
                            UsageTotals {
                                input: native.input,
                                output: native.output,
                                consumption_rows: 1,
                            },
                            control,
                        )?;
                    }
                }
            }
            drop(records);
            drop(statement);
            step(*scanned);
            control.check()?;
            if !same_time && count < take {
                return Ok(());
            }
            same_time = count == take;
        }
    }
}
fn page_sql(cells: bool, token: bool, same_time: bool) -> String {
    let (table, columns, time, id, index) = if cells {
        (
            "ledger_minute_usage",
            rows::CELL_COLUMNS,
            "minute",
            "rowid",
            if token {
                "ledger_cell_token_time"
            } else {
                "ledger_cell_time"
            },
        )
    } else {
        (
            "ledger_facts",
            rows::FACT_COLUMNS,
            "occurred_at",
            "native_log_id",
            if token {
                "ledger_fact_token_time"
            } else {
                "ledger_fact_time"
            },
        )
    };
    let token_clause = if token {
        "AND token_id IS ?5"
    } else {
        "AND ?5 IS NULL"
    };
    // 不能并列静态下界与 tuple 下界：SQLite 可能重扫相同时间的整个前缀。
    // 初始 cursor.time=from-1，后续只取已读时间；严格后继天然满足窗口下界。
    let seek = if same_time {
        format!("{time}=?3 AND {id}>?4 AND {time}>=?1")
    } else {
        format!("{time}>?3 AND ?1 IS NOT NULL AND ?4 IS NOT NULL")
    };
    format!("SELECT {columns},{id} FROM {table} INDEXED BY {index} WHERE {seek} AND {time}<?2 {token_clause} ORDER BY {time},{id} LIMIT ?6")
}

#[cfg(test)]
pub(crate) fn test_page_sql(cells: bool, token: bool, same_time: bool) -> String {
    page_sql(cells, token, same_time)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    fn setup() -> (tempfile::TempDir, CatalogStore) {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        (dir, store)
    }
    #[test]
    fn query_connection_flags_are_readonly_nonwaiting_and_reject_writes() {
        let (_dir, store) = setup();
        let c = QueryControl::new(std::time::Instant::now() + Duration::from_secs(60));
        let reader = store.usage_read_target().unwrap().open(&c).unwrap();
        assert!(reader.conn.is_readonly("main").unwrap());
        assert_eq!(
            reader
                .conn
                .pragma_query_value(None, "query_only", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            reader
                .conn
                .pragma_query_value(None, "busy_timeout", |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(reader.conn.execute("DELETE FROM ledger_state", []).is_err());
    }
    #[test]
    fn query_connection_closes_before_last_directory_guard_unlocks() {
        use rusqlite::ffi;
        struct Probe {
            file: std::fs::File,
            held: Arc<AtomicBool>,
        }
        unsafe extern "C" fn value(
            ctx: *mut ffi::sqlite3_context,
            _n: i32,
            _args: *mut *mut ffi::sqlite3_value,
        ) {
            unsafe { ffi::sqlite3_result_int(ctx, 0) };
        }
        unsafe extern "C" fn closed(p: *mut std::ffi::c_void) {
            let probe = unsafe { Box::from_raw(p as *mut Probe) };
            let held = matches!(
                probe.file.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            );
            probe.held.store(held, Ordering::SeqCst);
            if !held {
                let _ = probe.file.unlock();
            }
        }
        let (dir, store) = setup();
        let c = QueryControl::new(std::time::Instant::now() + Duration::from_secs(60));
        let reader = store.usage_read_target().unwrap().open(&c).unwrap();
        let held = Arc::new(AtomicBool::new(false));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.path().join(".catalog.lock"))
            .unwrap();
        let probe = Box::into_raw(Box::new(Probe {
            file,
            held: held.clone(),
        }));
        let rc = unsafe {
            ffi::sqlite3_create_function_v2(
                reader.conn.handle(),
                c"query_close_probe".as_ptr(),
                0,
                ffi::SQLITE_UTF8,
                probe.cast(),
                Some(value),
                None,
                None,
                Some(closed),
            )
        };
        assert_eq!(rc, ffi::SQLITE_OK);
        drop(store);
        drop(reader);
        assert!(held.load(Ordering::SeqCst));
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_ok());
    }
}
