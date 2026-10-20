//! 身份绑定、实际回读收据与持久工作调度；不执行网络请求。
use super::control::{
    bump, ensure, failure, operation_projection, ControlErrorCode, ControlResult, OperationId,
    OperationPhase, OperationView, StatusDirective,
};
use super::execution::{load_work, read_ticket, ExecutionEffect, WorkTicket, PENDING_EFFECT_SQL};
use super::store::{commit, CatalogStore};
use super::types::*;
use crate::newapi::managed_channels::ChannelObservation;
use crate::reconcile::planner::{
    fresh, plan_next_action, resolve_created, BackendReadiness, CreationMatch, PendingWrite,
    PlanContext, PlanStep, Revision, StampedObservation,
};
use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::json;
use sha2::{Digest, Sha256};

/// 仅可信观察器提交；身份/新鲜度在调用端核实，目录仍核对完整 revision。
pub(crate) struct ObservedFields {
    pub member: MembershipId,
    pub revision: Revision,
    pub status: Option<i64>,
    pub priority: Option<i64>,
}

impl CatalogStore {
    pub fn pending_creation(&mut self, id: MembershipId) -> ControlResult<WorkTicket> {
        self.usable()?;
        let tx = self.conn.transaction()?;
        creation_ticket(&tx, id)
    }

    pub fn bind_created(
        &mut self,
        ticket: &WorkTicket,
        complete: bool,
        channels: &[ChannelObservation],
        backend: BackendReadiness,
        now: i64,
    ) -> ControlResult<ChannelId> {
        self.usable()?;
        ensure(valid_timestamp(Some(now)), ControlErrorCode::InvalidInput)?;
        ensure(backend.ready_at(now), ControlErrorCode::NotReady)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = ticket.membership_id();
        ensure(
            creation_ticket(&tx, id)? == *ticket,
            ControlErrorCode::RevisionConflict,
        )?;
        let work = load_work(&tx, id, false)?;
        let ExecutionEffect::Create {
            name,
            channel_type,
            group,
            base_url,
            kind,
            ..
        } = ticket.effect()
        else {
            return Err(failure(ControlErrorCode::InvalidInput));
        };
        ensure(
            name == &work.target.create_name
                && *channel_type == work.identity.channel_type
                && group == &work.identity.group
                && base_url == &work.identity.base_url
                && *kind == work.source.kind,
            ControlErrorCode::BindingDrift,
        )?;
        let channel = match resolve_created(&work, complete, channels).map_err(failure)? {
            CreationMatch::Missing => return Err(failure(ControlErrorCode::TransportUnknown)),
            CreationMatch::Unique(id) => id,
        };
        let observed = channels
            .iter()
            .find(|c| c.channel_id == channel.get() && c.name == *name)
            .ok_or_else(|| failure(ControlErrorCode::InvalidResponse))?;
        ensure(fresh(observed, now), ControlErrorCode::NotReady)?;
        if let Some(existing) = work.member.channel_id {
            ensure(existing == channel, ControlErrorCode::BindingDrift)?;
            return Ok(channel);
        }
        let reserved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memberships WHERE channel_id=?1)",
            [channel.get()],
            |r| r.get(0),
        )?;
        ensure(!reserved, ControlErrorCode::BindingDrift)?;
        // Step P14: 旧创建只登记身份，当前 gate 不被旧启用意图覆盖；下一次观察须用新序号。
        tx.execute("UPDATE memberships SET channel_id=?1,lifecycle='created_disabled' WHERE membership_id=?2",params![channel.get(),id.get()])?;
        tx.execute("UPDATE member_control SET create_state='resolved',dirty=1,next_attempt_at=?1,last_error_code=NULL WHERE membership_id=?2",params![now,id.get()])?;
        tx.execute(
            "UPDATE member_execution SET sequence=?1 WHERE membership_id=?2",
            params![bump(work.execution_revision)?, id.get()],
        )?;
        // 已物化但未结算的 HTTP 交付仍未知，不能伪造 responded；释放唯一 prepared 槽。
        tx.execute("UPDATE execution_attempts SET state='uncertain',error_code='transport_unknown',finished_at=MAX(started_at,?1) WHERE membership_id=?2 AND sequence=?3 AND state='prepared'",params![now,id.get(),ticket.revision().execution])?;
        refresh_operations(
            &tx,
            id,
            work.target.revision,
            OperationPhase::Waiting,
            None,
            now,
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(channel)
    }

    pub fn confirm_member(
        &mut self,
        id: MembershipId,
        expected: Revision,
        policy_disabled: bool,
        observation: Option<&StampedObservation>,
        context: &PlanContext<'_>,
    ) -> ControlResult<()> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let work = load_work(&tx, id, policy_disabled)?;
        ensure(
            work.revision() == expected,
            ControlErrorCode::RevisionConflict,
        )?;
        let step = plan_next_action(&work, observation, context);
        ensure(
            matches!(step, PlanStep::Confirm | PlanStep::RetireAbsent),
            match step {
                PlanStep::Wait(code) | PlanStep::Reject(code) => code,
                _ => ControlErrorCode::NotReady,
            },
        )?;
        ensure(
            work.pending_write == PendingWrite::None,
            ControlErrorCode::TransportUnknown,
        )?;
        let retired = work.target.retire_requested
            || work.source_retire_requested
            || work.member.lifecycle == MembershipLifecycle::Retired;
        let absent = matches!(step, PlanStep::RetireAbsent);
        let observed = if absent {
            None
        } else {
            Some(
                &observation
                    .ok_or_else(|| failure(ControlErrorCode::NotReady))?
                    .channel,
            )
        };
        let model_digest = observed.map(|o| {
            let mut models = crate::model_catalog::normalize_models_csv(&o.models);
            models.sort();
            format!("{:x}", Sha256::digest(models.join(",")))
        });
        let receipt=json!({"format_version":1,"membership_id":id,"channel_id":work.member.channel_id,"member_revision":expected.member,"source_revision":expected.source,"group_revision":expected.group,"execution_revision":expected.execution,"status":observed.map(|o|o.status),"priority":observed.map(|o|o.priority),"models_digest":model_digest,"retired_absent":absent}).to_string();
        // Step P15: 实际回读与目标匹配才首次保存收据；ACK 数量不参与计数。
        tx.execute("UPDATE memberships SET observed_status=?1,observed_priority=?2,applied_revision=?3,lifecycle=?4,retired_at=CASE WHEN ?5 THEN COALESCE(retired_at,?6) ELSE retired_at END WHERE membership_id=?7",params![observed.map(|o|o.status),observed.map(|o|o.priority),expected.member,if retired {"retired"}else {"ready"},retired,context.now,id.get()])?;
        tx.execute("UPDATE member_control SET dirty=0,next_attempt_at=0,last_error_code=NULL WHERE membership_id=?1",[id.get()])?;
        if work.target.status_directive == StatusDirective::Enabled
            && observed.is_some_and(|o| o.status == 1)
        {
            tx.execute("UPDATE member_execution SET enable_consumed=1 WHERE membership_id=?1 AND enable_epoch>0",[id.get()])?;
        }
        tx.execute("UPDATE operation_targets SET confirmed_at=?1,receipt_json=?2 WHERE membership_id=?3 AND target_member_revision=?4 AND confirmed_at IS NULL AND operation_id IN (SELECT operation_id FROM control_operations WHERE phase NOT IN ('succeeded','superseded'))",params![context.now,receipt,id.get(),expected.member])?;
        if work.source_retire_requested {
            tx.execute("UPDATE sources SET retired_at=COALESCE(retired_at,?1) WHERE source_id=?2 AND NOT EXISTS(SELECT 1 FROM memberships WHERE source_id=?2 AND lifecycle!='retired')",params![context.now,work.source.source_id.get()])?;
        }
        refresh_operations(
            &tx,
            id,
            expected.member,
            OperationPhase::Waiting,
            None,
            context.now,
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(())
    }

    pub fn defer_member(
        &mut self,
        id: MembershipId,
        expected: Revision,
        code: ControlErrorCode,
        now: i64,
        retry_at: Option<i64>,
    ) -> ControlResult<()> {
        self.usable()?;
        ensure(
            valid_timestamp(Some(now))
                && retry_at.is_none_or(|at| valid_timestamp(Some(at)) && at >= now),
            ControlErrorCode::InvalidInput,
        )?;
        ensure(
            !matches!(
                code,
                ControlErrorCode::RevisionConflict
                    | ControlErrorCode::IdempotencyConflict
                    | ControlErrorCode::Storage
            ),
            ControlErrorCode::InvalidInput,
        )?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let work = load_work(&tx, id, false)?;
        ensure(
            work.revision() == expected,
            ControlErrorCode::RevisionConflict,
        )?;
        ensure(
            work.managed && work.member.lifecycle != MembershipLifecycle::Retired,
            ControlErrorCode::NotReady,
        )?;
        let due = now
            .checked_add(30)
            .filter(|n| valid_timestamp(Some(*n)))
            .ok_or_else(|| failure(ControlErrorCode::InvalidInput))?
            .max(retry_at.unwrap_or(now));
        // Step P16: 暂时失败保留到期工作；需修正的失败等待显式重试，不清原意图。
        tx.execute("UPDATE member_control SET dirty=?1,next_attempt_at=?2,last_error_code=?3 WHERE membership_id=?4",params![!terminal(code),due,code.as_str(),id.get()])?;
        refresh_operations(
            &tx,
            id,
            expected.member,
            OperationPhase::Waiting,
            Some(code),
            now,
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(())
    }

    pub fn retry_operation(&mut self, id: &OperationId, now: i64) -> ControlResult<OperationView> {
        self.usable()?;
        ensure(valid_timestamp(Some(now)), ControlErrorCode::InvalidInput)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = operation_projection(&tx, id)?;
        if before.target_type == super::control::TargetType::Token {
            let result = super::tokens::retry_tx(&tx, id, now)?;
            commit(tx, &mut self.poisoned)?;
            return Ok(result);
        }
        if before.phase == OperationPhase::Succeeded {
            return Ok(before);
        }
        ensure(
            before.phase != OperationPhase::Superseded,
            ControlErrorCode::RevisionConflict,
        )?;
        let mismatch:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM operation_targets t JOIN member_control c USING(membership_id) WHERE t.operation_id=?1 AND t.target_member_revision!=c.revision)",[id.as_str()],|r|r.get(0))?;
        ensure(!mismatch, ControlErrorCode::RevisionConflict)?;
        tx.execute("UPDATE member_control SET dirty=1,next_attempt_at=?1,last_error_code=NULL WHERE membership_id IN (SELECT membership_id FROM operation_targets WHERE operation_id=?2 AND confirmed_at IS NULL)",params![now,id.as_str()])?;
        project_operation(&tx, id, OperationPhase::Queued, None, now)?;
        let result = operation_projection(&tx, id)?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }

    pub fn due_members(&self, now: i64) -> ControlResult<Vec<MembershipId>> {
        self.due_members_with_stops(now, &[])
    }

    pub(crate) fn due_members_with_stops(
        &self,
        now: i64,
        stopped: &[SourceId],
    ) -> ControlResult<Vec<MembershipId>> {
        self.usable()?;
        ensure(valid_timestamp(Some(now)), ControlErrorCode::InvalidInput)?;
        ensure(stopped.len() <= MAX_SOURCES, ControlErrorCode::InvalidInput)?;
        let stopped = serde_json::to_string(&stopped.iter().map(|s| s.get()).collect::<Vec<_>>())
            .map_err(|_| failure(ControlErrorCode::InvalidInput))?;
        let mut statement=self.conn.prepare("SELECT m.membership_id FROM memberships m JOIN member_control c USING(membership_id) JOIN sources s USING(source_id) JOIN source_control sc USING(source_id) WHERE c.dirty=1 AND c.next_attempt_at<=?1 AND m.lifecycle!='retired' ORDER BY CASE WHEN s.manual_paused OR sc.retire_requested OR c.retire_requested OR c.status_directive='disabled' OR m.source_id IN (SELECT value FROM json_each(?2)) THEN 0 WHEN m.observed_priority IS NOT NULL AND m.desired_priority<m.observed_priority THEN 1 ELSE 2 END,c.next_attempt_at,m.membership_id LIMIT 64")?;
        let mut rows = statement.query(params![now, stopped])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(MembershipId::new(row.get(0)?)?);
        }
        Ok(result)
    }

    pub(crate) fn record_observed(&mut self, updates: &[ObservedFields]) -> ControlResult<()> {
        self.usable()?;
        ensure(
            updates.len() <= MAX_MEMBERSHIPS,
            ControlErrorCode::InvalidInput,
        )?;
        let mut seen = std::collections::HashSet::new();
        for u in updates {
            ensure(
                seen.insert(u.member)
                    && u.status.is_some() == u.priority.is_some()
                    && u.status.is_none_or(|s| (1..=3).contains(&s))
                    && u.priority.is_none_or(safe_number),
                ControlErrorCode::InvalidInput,
            )?;
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for u in updates {
            let w = load_work(&tx, u.member, false)?;
            ensure(
                w.revision() == u.revision,
                ControlErrorCode::RevisionConflict,
            )?;
            tx.execute("UPDATE memberships SET observed_status=?1,observed_priority=?2 WHERE membership_id=?3",params![u.status,u.priority,u.member.get()])?;
        }
        commit(tx, &mut self.poisoned)?;
        Ok(())
    }

    pub fn next_attempt_at(&self) -> ControlResult<Option<i64>> {
        self.usable()?;
        Ok(self.conn.query_row("SELECT MIN(c.next_attempt_at) FROM member_control c JOIN memberships m USING(membership_id) WHERE c.dirty=1 AND m.lifecycle!='retired'",[],|r|r.get(0))?)
    }

    pub(crate) fn wake_pending(
        &mut self,
        source: Option<SourceId>,
        include_confirmed: bool,
        now: i64,
    ) -> ControlResult<()> {
        self.usable()?;
        ensure(valid_timestamp(Some(now)), ControlErrorCode::InvalidInput)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut eligible = Vec::new();
        {
            let mut q=tx.prepare("SELECT c.membership_id,c.last_error_code FROM member_control c JOIN memberships m USING(membership_id) WHERE m.lifecycle!='retired' AND (?1 IS NULL OR m.source_id=?1) AND (?2 OR c.dirty=1) AND EXISTS(SELECT 1 FROM operation_targets t WHERE t.membership_id=m.membership_id)")?;
            let mut rows = q.query(params![source.map(SourceId::get), include_confirmed])?;
            while let Some(row) = rows.next()? {
                let code: Option<String> = row.get(1)?;
                if code
                    .map(|s| ControlErrorCode::parse(&s))
                    .transpose()?
                    .is_none_or(|e| !terminal(e))
                {
                    eligible.push(row.get::<_, i64>(0)?);
                }
            }
        }
        for id in eligible {
            tx.execute(
                "UPDATE member_control SET dirty=1,next_attempt_at=?1 WHERE membership_id=?2",
                params![now, id],
            )?;
        }
        commit(tx, &mut self.poisoned)?;
        Ok(())
    }
}

fn creation_ticket(conn: &Connection, id: MembershipId) -> ControlResult<WorkTicket> {
    let (count,sequence):(i64,Option<i64>)=conn.query_row("SELECT count(*),MIN(sequence) FROM execution_attempts WHERE membership_id=?1 AND action='create' AND state!='not_sent'",[id.get()],|r|Ok((r.get(0)?,r.get(1)?)))?;
    ensure(
        count == 1,
        if count == 0 {
            ControlErrorCode::NotFound
        } else {
            ControlErrorCode::AmbiguousCreate
        },
    )?;
    read_ticket(
        conn,
        id,
        sequence.ok_or_else(|| failure(ControlErrorCode::Storage))?,
    )
}

pub(crate) fn terminal(code: ControlErrorCode) -> bool {
    matches!(
        code,
        ControlErrorCode::AuthFailed
            | ControlErrorCode::BindingDrift
            | ControlErrorCode::AmbiguousCreate
            | ControlErrorCode::RemoteRejected
            | ControlErrorCode::InvalidInput
            | ControlErrorCode::NotFound
    )
}

pub(super) fn refresh_operations(
    conn: &Connection,
    id: MembershipId,
    revision: i64,
    fallback: OperationPhase,
    code: Option<ControlErrorCode>,
    now: i64,
) -> ControlResult<()> {
    let mut statement=conn.prepare("SELECT o.operation_id FROM control_operations o JOIN operation_targets t USING(operation_id) WHERE t.membership_id=?1 AND t.target_member_revision=?2 AND o.phase NOT IN ('succeeded','superseded') ORDER BY o.operation_id")?;
    let mut rows = statement.query(params![id.get(), revision])?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next()? {
        ensure(ids.len() < MAX_MEMBERSHIPS, ControlErrorCode::Storage)?;
        ids.push(OperationId::new(row.get(0)?)?);
    }
    for id in ids {
        project_operation(conn, &id, fallback, code, now)?;
    }
    Ok(())
}

fn project_operation(
    conn: &Connection,
    id: &OperationId,
    fallback: OperationPhase,
    code: Option<ControlErrorCode>,
    now: i64,
) -> ControlResult<()> {
    let before = operation_projection(conn, id)?; // 同时检查冻结 expected_targets，不能因丢目标行伪成功。
    if matches!(
        before.phase,
        OperationPhase::Succeeded | OperationPhase::Superseded
    ) {
        return Ok(());
    }
    let (mismatch,unknown,prepared):(bool,bool,bool)=conn.query_row(&format!("SELECT COALESCE(MAX(t.target_member_revision!=c.revision),0),COALESCE(MAX(EXISTS(SELECT 1 FROM execution_attempts a WHERE a.membership_id=t.membership_id AND a.state='uncertain' AND ({PENDING_EFFECT_SQL}))),0),COALESCE(MAX(EXISTS(SELECT 1 FROM execution_attempts a WHERE a.membership_id=t.membership_id AND a.state='prepared' AND ({PENDING_EFFECT_SQL}))),0) FROM operation_targets t JOIN memberships m USING(membership_id) JOIN member_control c USING(membership_id) WHERE t.operation_id=?1"),[id.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let error = terminal_error(conn, id)?;
    let (phase, error) = if mismatch {
        (OperationPhase::Superseded, None)
    } else if before.confirmed_targets == before.total_targets && !unknown && !prepared {
        (OperationPhase::Succeeded, None)
    } else if unknown {
        (
            OperationPhase::Unknown,
            Some(ControlErrorCode::TransportUnknown),
        )
    } else if let Some(e) = error {
        (OperationPhase::Failed, Some(e))
    } else if before.confirmed_targets > 0 {
        (OperationPhase::Partial, code)
    } else if prepared {
        (OperationPhase::Applying, code)
    } else {
        (fallback, code)
    };
    conn.execute(
        "UPDATE control_operations SET phase=?1,error_code=?2,updated_at=?3 WHERE operation_id=?4",
        params![
            phase.as_str(),
            error.map(ControlErrorCode::as_str),
            now,
            id.as_str()
        ],
    )?;
    Ok(())
}

fn terminal_error(conn: &Connection, id: &OperationId) -> ControlResult<Option<ControlErrorCode>> {
    let mut q=conn.prepare("SELECT c.last_error_code FROM member_control c JOIN operation_targets t USING(membership_id) WHERE t.operation_id=?1 AND t.confirmed_at IS NULL AND c.last_error_code IS NOT NULL ORDER BY t.membership_id")?;
    let mut rows = q.query([id.as_str()])?;
    while let Some(row) = rows.next()? {
        let code = ControlErrorCode::parse(&row.get::<_, String>(0)?)?;
        if terminal(code) {
            return Ok(Some(code));
        }
    }
    Ok(None)
}
