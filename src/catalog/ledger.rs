//! 应用库事务入账；没有网络、定时任务或策略发布。
#![allow(dead_code)] // B2接线前只由显式本地入口和测试使用。
use super::{
    store::commit, types::error, CatalogError, CatalogStore, ChannelId, ErrorKind, GroupId,
    SourceId, JSON_SAFE,
};
use crate::ledger::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;

mod attribution;
pub(crate) use attribution::*;
pub(crate) mod observed;
pub(crate) mod query;
mod rows;
mod types;
mod verify;
pub(crate) use types::*;
pub(super) use verify::verify_ledger;

fn corrupt() -> LedgerError {
    error(ErrorKind::CorruptData).into()
}
fn check(ok: bool) -> LedgerResult<()> {
    if ok {
        Ok(())
    } else {
        Err(LedgerError::InvalidCheckpoint)
    }
}
fn safe(n: i64) -> bool {
    (0..=JSON_SAFE).contains(&n)
}
fn increment(n: i64) -> LedgerResult<i64> {
    n.checked_add(1)
        .filter(|n| safe(*n))
        .ok_or(LedgerError::InvalidCheckpoint)
}
fn valid_window(w: ScanWindow) -> bool {
    safe(w.from) && safe(w.to) && w.from < w.to
}

fn progress(conn: &Connection, lane: ScanLane) -> LedgerResult<ScanProgress> {
    let raw=conn.query_row("SELECT generation,window_from,window_to,next_page,phase,last_observed_at,fault_code FROM ledger_checkpoints WHERE lane=?1",[lane.as_str()],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<String>>(6)?))).optional()?.ok_or_else(corrupt)?;
    let window = match (raw.1, raw.2) {
        (None, None) => None,
        (Some(from), Some(to)) => Some(ScanWindow { from, to }),
        _ => return Err(corrupt()),
    };
    let p = ScanProgress {
        lane,
        generation: raw.0,
        window,
        next_page: raw.3,
        phase: ScanPhase::parse(&raw.4)?,
        last_observed_at: raw.5,
        fault: raw.6.as_deref().map(ScanFault::parse).transpose()?,
    };
    let shape = if p.phase == ScanPhase::Idle {
        p.generation == 0
            && p.window.is_none()
            && p.next_page == 1
            && p.last_observed_at.is_none()
            && p.fault.is_none()
    } else {
        p.generation > 0
            && p.window.is_some_and(valid_window)
            && match p.phase {
                ScanPhase::Scanning => {
                    p.fault.is_none()
                        && p.last_observed_at.is_none_or(|n| n >= p.window.unwrap().to)
                }
                ScanPhase::Scanned => {
                    p.fault.is_none()
                        && p.last_observed_at
                            .is_some_and(|n| n >= p.window.unwrap().to)
                }
                ScanPhase::Stalled => p.fault.is_some() && p.last_observed_at.is_some(),
                ScanPhase::Idle => false,
            }
    };
    if !shape
        || !safe(p.generation)
        || !(1..=JSON_SAFE).contains(&p.next_page)
        || p.last_observed_at.is_some_and(|n| !safe(n))
    {
        return Err(corrupt());
    }
    Ok(p)
}
fn state(conn: &Connection) -> LedgerResult<LedgerState> {
    let (revision, initial_requested_from) = conn
        .query_row(
            "SELECT revision,initial_requested_from FROM ledger_state WHERE singleton=1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
        )
        .optional()?
        .ok_or_else(corrupt)?;
    let counts: (i64, i64) = conn.query_row(
        "SELECT (SELECT count(*) FROM ledger_state),(SELECT count(*) FROM ledger_checkpoints)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if !(1..=JSON_SAFE).contains(&revision)
        || initial_requested_from.is_some_and(|n| !safe(n))
        || counts != (1, 2)
    {
        return Err(corrupt());
    }
    let scans = [
        progress(conn, ScanLane::Recent)?,
        progress(conn, ScanLane::History)?,
    ];
    if initial_requested_from.is_none() {
        let occupied:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM ledger_facts) OR EXISTS(SELECT 1 FROM ledger_minute_usage)",[],|r|r.get(0))?;
        if occupied || scans.iter().any(|p| p.phase != ScanPhase::Idle) {
            return Err(corrupt());
        }
    }
    Ok(LedgerState {
        revision,
        initial_requested_from,
        scans,
    })
}
fn initialized(conn: &Connection) -> LedgerResult<LedgerState> {
    let s = state(conn)?;
    if s.initial_requested_from.is_none() {
        Err(LedgerError::NotInitialized)
    } else {
        Ok(s)
    }
}
fn instance(conn: &Connection) -> LedgerResult<String> {
    let id: String = conn.query_row(
        "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(corrupt());
    }
    Ok(id)
}
fn save_progress(conn: &Connection, p: &ScanProgress) -> LedgerResult<()> {
    let n=conn.execute("UPDATE ledger_checkpoints SET generation=?2,window_from=?3,window_to=?4,next_page=?5,phase=?6,last_observed_at=?7,fault_code=?8 WHERE lane=?1",params![p.lane.as_str(),p.generation,p.window.map(|w|w.from),p.window.map(|w|w.to),p.next_page,p.phase.as_str(),p.last_observed_at,p.fault.map(|f|f.as_str())])?;
    if n != 1 {
        return Err(corrupt());
    }
    Ok(())
}
fn bump(conn: &Connection, old: i64) -> LedgerResult<i64> {
    let next = increment(old)?;
    if conn.execute(
        "UPDATE ledger_state SET revision=?1 WHERE singleton=1 AND revision=?2",
        params![next, old],
    )? != 1
    {
        return Err(corrupt());
    }
    Ok(next)
}
fn matched(conn: &Connection, t: &ScanTicket) -> LedgerResult<ScanProgress> {
    let p = progress(conn, t.progress.lane)?;
    if instance(conn)? != t.instance || p != t.progress || p.phase != ScanPhase::Scanning {
        return Err(LedgerError::RevisionConflict);
    }
    Ok(p)
}

impl CatalogStore {
    pub(crate) fn load_ledger_state(&self) -> LedgerResult<LedgerState> {
        self.usable()?;
        let tx = self.conn.unchecked_transaction()?;
        let s = state(&tx)?;
        tx.rollback()?;
        Ok(s)
    }
    pub(crate) fn initialize_ledger(&mut self, from: i64, now: i64) -> LedgerResult<LedgerState> {
        // Step B1.2: 只建立请求起点，空库不能被初始化为完整零用量。
        self.usable()?;
        check(safe(from) && safe(now) && from < now)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = state(&tx)?;
        match old.initial_requested_from {
            Some(value) if value != from => return Err(LedgerError::RevisionConflict),
            Some(_) => {
                tx.rollback()?;
                return Ok(old);
            }
            None => {}
        }
        tx.execute(
            "UPDATE ledger_state SET initial_requested_from=?1 WHERE singleton=1",
            [from],
        )?;
        bump(&tx, old.revision)?;
        let result = state(&tx)?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
    pub(crate) fn begin_scan(
        &mut self,
        lane: ScanLane,
        expected_generation: i64,
        window: ScanWindow,
        now: i64,
    ) -> LedgerResult<ScanTicket> {
        // Step B1.3: 持久代次防旧响应；活动窗口不能静默替换。
        self.usable()?;
        check(valid_window(window) && safe(now) && window.to <= now)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let s = initialized(&tx)?;
        let mut p = progress(&tx, lane)?;
        if p.generation != expected_generation {
            return Err(LedgerError::RevisionConflict);
        }
        if p.phase == ScanPhase::Scanning {
            if p.window != Some(window) {
                return Err(LedgerError::RevisionConflict);
            }
            let ticket = ScanTicket {
                instance: instance(&tx)?,
                progress: p,
            };
            tx.rollback()?;
            return Ok(ticket);
        }
        if p.phase == ScanPhase::Stalled && p.window != Some(window) {
            return Err(LedgerError::RevisionConflict);
        }
        p.generation = increment(p.generation)?;
        p.window = Some(window);
        p.next_page = 1;
        p.phase = ScanPhase::Scanning;
        p.last_observed_at = None;
        p.fault = None;
        save_progress(&tx, &p)?;
        bump(&tx, s.revision)?;
        let ticket = ScanTicket {
            instance: instance(&tx)?,
            progress: p,
        };
        commit(tx, &mut self.poisoned)?;
        Ok(ticket)
    }
    pub(crate) fn restart_scan(
        &mut self,
        ticket: &ScanTicket,
        now: i64,
    ) -> LedgerResult<ScanTicket> {
        // Step B2.3: 完整验票后同窗新代次，不改原事实、投影或 begin_scan 重放语义。
        self.usable()?;
        check(safe(now))?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let s = initialized(&tx)?;
        let mut p = matched(&tx, ticket)?;
        check(
            now >= p.window.ok_or(LedgerError::InvalidCheckpoint)?.to
                && p.last_observed_at.is_none_or(|at| now >= at),
        )?;
        p.generation = increment(p.generation)?;
        p.next_page = 1;
        p.last_observed_at = None;
        p.fault = None;
        save_progress(&tx, &p)?;
        bump(&tx, s.revision)?;
        let next = ScanTicket {
            instance: instance(&tx)?,
            progress: p,
        };
        commit(tx, &mut self.poisoned)?;
        Ok(next)
    }
    pub(crate) fn load_scan_ticket(&self, lane: ScanLane) -> LedgerResult<Option<ScanTicket>> {
        self.usable()?;
        let tx = self.conn.unchecked_transaction()?;
        initialized(&tx)?;
        let p = progress(&tx, lane)?;
        let ticket = if p.phase == ScanPhase::Scanning {
            Some(ScanTicket {
                instance: instance(&tx)?,
                progress: p,
            })
        } else {
            None
        };
        tx.rollback()?;
        Ok(ticket)
    }
    pub(crate) fn commit_fact_page(
        &mut self,
        ticket: &ScanTicket,
        input: &[Value],
        end: PageEnd,
        observed_at: i64,
    ) -> LedgerResult<LedgerCommit> {
        // Step B1.4: 规范去重、历史归属、联合增量和页进度只提交一次事务。
        self.usable()?;
        let facts = decode_native_batch(input)?;
        let window = ticket
            .progress
            .window
            .ok_or(LedgerError::InvalidCheckpoint)?;
        check(
            valid_window(window)
                && safe(observed_at)
                && observed_at >= window.to
                && facts
                    .iter()
                    .all(|f| f.occurred_at >= window.from && f.occurred_at < window.to),
        )?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let s = initialized(&tx)?;
        let mut p = matched(&tx, ticket)?;
        check(p.last_observed_at.is_none_or(|t| observed_at >= t))?;
        let h = rows::history(&tx)?;
        let mut fresh = Vec::new();
        for f in facts {
            match rows::read_fact(&tx, f.log_id, &h)? {
                Some(old) if old.native != f => return Err(FactError::ConflictingIdentity.into()),
                Some(_) => {}
                None => fresh.push(f),
            }
        }
        let history: Vec<_> = h.into_values().collect();
        let attributed = attribute_batch(&fresh, &history)?;
        let delta = usage_delta(&attributed)?;
        for f in &attributed {
            rows::insert_fact(&tx, f)?;
            observed::enqueue_observation(&tx, f, observed_at)
                .map_err(|_| LedgerError::Catalog(error(ErrorKind::Storage)))?;
        }
        for (k, t) in &delta.cells {
            rows::add_cell(&tx, k, *t)?;
        }
        match end {
            PageEnd::More => p.next_page = increment(p.next_page)?,
            PageEnd::ReachedEnd => p.phase = ScanPhase::Scanned,
        };
        p.last_observed_at = Some(observed_at);
        p.fault = None;
        save_progress(&tx, &p)?;
        let revision = bump(&tx, s.revision)?;
        let result = LedgerCommit {
            inserted: fresh.len(),
            duplicates: input
                .iter()
                .filter(|r| matches!(r.get("type").and_then(Value::as_i64), Some(2 | 5)))
                .count()
                - fresh.len(),
            changed_cells: delta.cells.len(),
            ledger_revision: revision,
            progress: p,
        };
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
    pub(crate) fn mark_scan_fault(
        &mut self,
        ticket: &ScanTicket,
        fault: ScanFault,
        observed_at: i64,
    ) -> LedgerResult<ScanProgress> {
        // Step B1.5: 独立诊断事务，绝不推进失败页或补写未知提交。
        self.usable()?;
        check(safe(observed_at))?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let s = initialized(&tx)?;
        let mut p = matched(&tx, ticket)?;
        check(p.last_observed_at.is_none_or(|t| observed_at >= t))?;
        p.phase = ScanPhase::Stalled;
        p.fault = Some(fault);
        p.last_observed_at = Some(observed_at);
        save_progress(&tx, &p)?;
        bump(&tx, s.revision)?;
        commit(tx, &mut self.poisoned)?;
        Ok(p)
    }
}
