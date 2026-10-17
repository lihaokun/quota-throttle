//! 本地观测队列、请求相位与周期历史；所有 helper 属于调用方的单一事务。
use super::*;
use crate::catalog::control::{
    bump as revision_next, ensure, failure, ControlErrorCode as Code, ControlResult,
};
use crate::catalog::policy::{policy_error, read_runtime, source_record, write_runtime};
use crate::catalog::SourceSettings;
use crate::ledger::collection::RECENT_SECONDS;
use crate::ledger::observations::{
    classify_observation, observe_request, RequestObservationPhase, RequestObservationState,
};
use crate::source_policy::volcano::{
    self, EvidenceDisposition, PeriodTransition, VolcanoEvidence, VolcanoRuntime, VolcanoState,
};
use rusqlite::Transaction;
use std::time::{Duration, Instant};

mod usage;
pub(crate) const SOURCE_OBSERVATION_BATCH: usize = 16;
pub(crate) struct ProjectionBudget {
    start: Instant,
    scanned: usize,
}
impl ProjectionBudget {
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            scanned: 0,
        }
    }
    pub(crate) fn check(&self) -> ControlResult<()> {
        ensure(
            self.start.elapsed() < Duration::from_secs(10),
            Code::NotReady,
        )
    }
    fn row(&mut self) -> ControlResult<()> {
        self.check()?;
        ensure(
            self.scanned < crate::ledger::analytics::MAX_QUERY_ROWS,
            Code::NotReady,
        )?;
        self.scanned += 1;
        Ok(())
    }
}
pub(crate) struct SourceProjection {
    pub(crate) runtime: Option<VolcanoRuntime>,
    pub(crate) ready: bool,
    pub(crate) disabled: bool,
    pub(crate) pending: bool,
    pub(crate) consumed: usize,
}

pub(crate) fn enqueue_observation(
    tx: &Transaction<'_>,
    fact: &AttributedFact,
    at: i64,
) -> ControlResult<()> {
    // Step O4.1: 同事实/首次归属事务入队；旧历史仍计量，但不重播为当前健康或错误。
    ensure(safe(at), Code::InvalidInput)?;
    if fact.native.occurred_at < at.saturating_sub(RECENT_SECONDS) || fact.native.occurred_at > at {
        return Ok(());
    }
    let Some(candidate) = classify_observation(fact).map_err(|_| failure(Code::Storage))? else {
        return Ok(());
    };
    let (source, settings, retiring) = source_record(tx, candidate.source_id)?;
    if source.retired_at.is_some()
        || retiring
        || !matches!(settings, SourceSettings::Volcano { upstream, .. } if upstream.channel_type == 8)
    {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO volcano_observation_queue(native_log_id,source_id) VALUES(?1,?2) ON CONFLICT(native_log_id) DO NOTHING",
        params![fact.native.log_id, candidate.source_id.get()],
    )?;
    Ok(())
}

pub(crate) fn record_period(
    tx: &Transaction<'_>,
    source: SourceId,
    period: i64,
    t: &PeriodTransition,
) -> ControlResult<()> {
    tx.execute("INSERT INTO volcano_period_history(source_id,period_id,old_start,old_next,new_start,new_next,reset_minute,utc_offset_minutes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(source_id,period_id) DO NOTHING",params![source.get(),period,t.old_start,t.old_next,t.new_start,t.new_next,t.reset_minute,t.utc_offset_minutes])?;
    let recorded = tx.query_row("SELECT old_start,old_next,new_start,new_next,reset_minute,utc_offset_minutes FROM volcano_period_history WHERE source_id=?1 AND period_id=?2",params![source.get(),period],|r| Ok(PeriodTransition {old_start:r.get(0)?,old_next:r.get(1)?,new_start:r.get(2)?,new_next:r.get(3)?,reset_minute:r.get(4)?,utc_offset_minutes:r.get(5)?}))?;
    ensure(recorded == *t, Code::Storage)
}

fn request_state(
    tx: &Transaction<'_>,
    source: SourceId,
    request: &str,
) -> ControlResult<Option<RequestObservationState>> {
    tx.query_row("SELECT first_event_at,phase FROM volcano_request_observations WHERE source_id=?1 AND request_id=?2",params![source.get(),request],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).optional()?.map(|(at,phase)| {
        ensure(safe(at) && crate::ledger::valid_request_id(request),Code::Storage)?;
        Ok(RequestObservationState { source_id:source,request_id:request.into(),first_event_at:at,phase:match phase.as_str(){"failed"=>RequestObservationPhase::Failed,"token_flow"=>RequestObservationPhase::TokenFlow,_=>return Err(failure(Code::Storage))} })
    }).transpose()
}
fn save_request(tx: &Transaction<'_>, state: &RequestObservationState) -> ControlResult<()> {
    tx.execute("INSERT INTO volcano_request_observations(source_id,request_id,first_event_at,phase) VALUES(?1,?2,?3,?4) ON CONFLICT(source_id,request_id) DO UPDATE SET first_event_at=excluded.first_event_at,phase=excluded.phase",params![state.source_id.get(),state.request_id,state.first_event_at,match state.phase {RequestObservationPhase::Failed=>"failed",RequestObservationPhase::TokenFlow=>"token_flow"}])?;
    Ok(())
}

fn consume_queue(
    tx: &Transaction<'_>,
    source: SourceId,
    now: i64,
    budget: &ProjectionBudget,
) -> ControlResult<(Vec<volcano::OutcomeObservation>, usize, bool)> {
    let queued:Vec<(i64,i64)>=tx.prepare("SELECT sequence,native_log_id FROM volcano_observation_queue WHERE source_id=?1 ORDER BY sequence LIMIT ?2")?.query_map(params![source.get(),SOURCE_OBSERVATION_BATCH as i64],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut outcomes = Vec::new();
    let mut consumed = 0;
    for (seq, id) in queued {
        budget.check()?;
        let fact = tx
            .query_row(
                &format!(
                    "SELECT {} FROM ledger_facts WHERE native_log_id=?1",
                    rows::FACT_COLUMNS
                ),
                [id],
                |r| Ok(rows::fact_row(r)),
            )?
            .map_err(|_| failure(Code::Storage))?;
        ensure(fact.source_id == Some(source), Code::Storage)?;
        if fact.native.occurred_at > now {
            break;
        }
        if let Some(c) = classify_observation(&fact).map_err(|_| failure(Code::Storage))? {
            let previous = request_state(tx, source, &c.request_id)?;
            let mut d =
                observe_request(previous.as_ref(), &c).map_err(|_| failure(Code::Storage))?;
            // 未入队的旧规范事实也提供请求下界；不能把跨 cutoff/reset 的 S 当作新请求。
            let first:i64=tx.query_row("SELECT min(occurred_at) FROM ledger_facts INDEXED BY ledger_fact_source_request WHERE source_id=?1 AND request_id=?2",params![source.get(),c.request_id],|r|r.get(0))?;
            ensure(safe(first) && first <= c.occurred_at, Code::Storage)?;
            d.next.first_event_at = d.next.first_event_at.min(first);
            if previous.as_ref() != Some(&d.next) {
                save_request(tx, &d.next)?;
            }
            if let Some(mut o) = d.outcome {
                o.first_event_at = d.next.first_event_at;
                if o.observed_at >= now.saturating_sub(RECENT_SECONDS) {
                    outcomes.push(o);
                }
            }
        }
        ensure(
            tx.execute(
                "DELETE FROM volcano_observation_queue WHERE sequence=?1 AND source_id=?2",
                params![seq, source.get()],
            )? == 1,
            Code::Storage,
        )?;
        consumed += 1;
    }
    let pending: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM volcano_observation_queue WHERE source_id=?1)",
        [source.get()],
        |r| r.get(0),
    )?;
    Ok((outcomes, consumed, pending))
}

pub(crate) fn project_source(
    tx: &Transaction<'_>,
    id: SourceId,
    now: i64,
    initialized: bool,
    fresh: bool,
    budget: &mut ProjectionBudget,
) -> ControlResult<SourceProjection> {
    // Step O4.2: 观测相位/队列消费、周期历史和 Runtime 属于调用方同一事务。
    budget.check()?;
    let (source, settings, retiring) = source_record(tx, id)?;
    let SourceSettings::Volcano { upstream, policy } = settings else {
        return Err(failure(Code::Storage));
    };
    let old = read_runtime(tx, id, &policy)?;
    if source.retired_at.is_some() || retiring {
        return Ok(SourceProjection {
            runtime: old,
            ready: false,
            disabled: true,
            pending: false,
            consumed: 0,
        });
    }
    let mut next = match &old {
        Some(old) => {
            let (next, t) = volcano::advance_period(old, &policy, now).map_err(policy_error)?;
            if let Some(t) = t {
                record_period(tx, id, old.period_id, &t)?;
            }
            next
        }
        None if initialized => {
            volcano::initialize_runtime(id, &policy, now).map_err(policy_error)?
        }
        None => {
            return Ok(SourceProjection {
                runtime: None,
                ready: false,
                disabled: source.manual_paused,
                pending: false,
                consumed: 0,
            })
        }
    };
    let usable_time = now >= next.period_start;
    let mut pending = false;
    let mut consumed = 0;
    if initialized && usable_time {
        let totals = usage::source_usage(
            tx,
            id,
            ScanWindow {
                from: next.period_start,
                to: now
                    .checked_add(1)
                    .ok_or_else(|| failure(Code::InvalidInput))?
                    .min(next.next_reset_at),
            },
            budget,
        )?;
        let outcomes = if upstream.channel_type == 8 {
            let (o, n, p) = consume_queue(tx, id, now, budget)?;
            consumed = n;
            pending = p;
            o
        } else {
            Vec::new()
        };
        if next.evidence_cursor.is_none()
            || consumed > 0
            || totals.input != next.used_input
            || totals.output != next.used_output
        {
            let cursor = revision_next(next.evidence_cursor.unwrap_or(0))
                .map_err(|_| failure(Code::Storage))?;
            let batch = VolcanoEvidence {
                source_id: id,
                period_id: next.period_id,
                error_epoch: next.error_epoch,
                from_cursor: next.evidence_cursor,
                to_cursor: cursor,
                used_input: totals.input,
                used_output: totals.output,
                coverage_end: (now + 1).min(next.next_reset_at),
                outcomes,
            };
            let (updated, disposition) =
                volcano::apply_evidence(&next, &policy, &batch, now).map_err(policy_error)?;
            ensure(disposition == EvidenceDisposition::Applied, Code::Storage)?;
            next = updated;
        }
    }
    next.validate(&policy).map_err(policy_error)?;
    if old.as_ref() != Some(&next) {
        if let Some(old) = &old {
            next.revision = revision_next(old.revision).map_err(|_| failure(Code::Storage))?;
        }
        write_runtime(tx, &next)?;
    }
    let disabled = source.manual_paused || next.state(false) == VolcanoState::Exhausted;
    Ok(SourceProjection {
        runtime: Some(next),
        ready: initialized
            && usable_time
            && fresh
            && !pending
            && upstream.channel_type == 8
            && !disabled,
        disabled,
        pending,
        consumed,
    })
}

pub(crate) fn ledger_ready(tx: &Transaction<'_>, now: i64) -> ControlResult<(bool, Option<i64>)> {
    let s = state(tx).map_err(|_| failure(Code::Storage))?;
    let recent = s
        .scans
        .iter()
        .find(|s| s.lane == ScanLane::Recent)
        .ok_or_else(|| failure(Code::Storage))?;
    let at = recent.last_observed_at.filter(|at| {
        matches!(recent.phase, ScanPhase::Scanning | ScanPhase::Scanned)
            && recent.fault.is_none()
            && *at <= now
            && now - *at < crate::reconcile::planner::OBSERVATION_MAX_AGE_SECS
    });
    Ok((s.initial_requested_from.is_some(), at))
}

pub(crate) fn verify_observed(conn: &Connection) -> std::result::Result<(), CatalogError> {
    let bad:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM volcano_observation_queue q JOIN ledger_facts f USING(native_log_id) WHERE f.source_id IS NULL OR f.source_id!=q.source_id) OR EXISTS(SELECT 1 FROM volcano_period_history h JOIN volcano_runtime r USING(source_id) WHERE r.period_id IS NULL OR h.period_id>=r.period_id)",[],|r|r.get(0))?;
    if bad {
        return Err(error(ErrorKind::CorruptData));
    }
    let mut q =
        conn.prepare("SELECT request_id,first_event_at,phase FROM volcano_request_observations")?;
    let mut records = q.query([])?;
    while let Some(r) = records.next()? {
        let req: String = r.get(0)?;
        let at: i64 = r.get(1)?;
        let phase: String = r.get(2)?;
        if !crate::ledger::valid_request_id(&req)
            || !safe(at)
            || !matches!(phase.as_str(), "failed" | "token_flow")
        {
            return Err(error(ErrorKind::CorruptData));
        }
    }
    let seq: Option<i64> = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='volcano_observation_queue'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let max: Option<i64> = conn.query_row(
        "SELECT max(sequence) FROM volcano_observation_queue",
        [],
        |r| r.get(0),
    )?;
    if seq.is_some_and(|v| !safe(v)) || max.is_some_and(|v| seq.is_none_or(|s| s < v)) {
        return Err(error(ErrorKind::CorruptData));
    }
    Ok(())
}
