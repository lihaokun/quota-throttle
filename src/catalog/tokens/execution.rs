use super::*;
use crate::catalog::execution::ExecutionOutcome;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub format_version: i64,
    pub token_id: ManagedTokenId,
    pub native_token_id: String,
    pub owner_user_id: String,
    pub owner_scope: TokenOwnerScope,
    pub token_revision: i64,
    pub evidence_revision: i64,
    pub status: Option<TokenStatus>,
    pub missing: bool,
    pub confirmed_at: i64,
}

fn creation_allowed(conn: &Connection, row: &Record, now: i64) -> ControlResult<bool> {
    let Some(group) = row.group else {
        return Ok(false);
    };
    let current: Option<(String, i64, bool)> = conn
        .query_row(
            "SELECT newapi_group,revision,ready FROM source_groups WHERE group_id=?1",
            [group.get()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(current.is_some_and(|(name, rev, ready)| {
        ready && Some(rev) == row.group_revision && name == row.explicit_group
    }) && row.expires_at.is_none_or(|at| at > now))
}

fn validate_ticket(
    conn: &Connection,
    ticket: &TokenTicket,
) -> ControlResult<(Record, String, Option<Code>, i64)> {
    ensure(instance(conn)? == ticket.instance, Code::RevisionConflict)?;
    let row = record(conn, ticket.record.id)?;
    let work = load_work(conn, &ticket.operation, 0)?;
    let (revision,act,state,code,started):(i64,String,String,Option<String>,i64)=conn.query_row("SELECT token_revision,action,state,error_code,started_at FROM token_execution_attempts WHERE token_id=?1 AND sequence=?2 AND operation_id=?3",params![row.id.get(),ticket.sequence,ticket.operation.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?.ok_or_else(||failure(Code::NotFound))?;
    ensure(
        work.record.id == ticket.record.id
            && work.action == ticket.action
            && decode::<Action>(act)? == ticket.action
            && revision == ticket.record.revision
            && work.operation.target_revision == revision
            && ticket.record.evidence.checked_add(1) == Some(ticket.sequence)
            && ticket.sequence <= row.evidence
            && row.owner == ticket.record.owner
            && row.group == ticket.record.group
            && row.explicit_group == ticket.record.explicit_group
            && row.creation == ticket.record.creation
            && row.group_revision == ticket.record.group_revision
            && row.expires_at == ticket.record.expires_at
            && (ticket.action == Action::Create || row.native_id == ticket.record.native_id),
        Code::RevisionConflict,
    )?;
    Ok((
        row,
        state,
        code.map(|v| Code::parse(&v)).transpose()?,
        started,
    ))
}

fn project(
    conn: &Connection,
    work: &TokenWork,
    code: Option<Code>,
    failed_response: bool,
    now: i64,
) -> ControlResult<OperationView> {
    let id = &work.operation.operation_id;
    let old = operation_projection(conn, id)?;
    if old.phase == OperationPhase::Succeeded {
        return Ok(old);
    }
    let (prepared, uncertain) = pending(conn, work.record.id)?;
    let may_create = work.action == Action::Create
        && create_state(conn, &work.record)? == CreateState::MayHaveBeenSent;
    let phase = if prepared {
        OperationPhase::Applying
    } else if uncertain || (may_create && code.is_some()) {
        OperationPhase::Unknown
    } else if failed_response || code.is_some_and(super::super::confirmation::terminal) {
        OperationPhase::Failed
    } else {
        OperationPhase::Waiting
    };
    let code = if phase == OperationPhase::Unknown {
        code.or(Some(Code::TransportUnknown))
    } else {
        code
    };
    conn.execute("UPDATE control_operations SET phase=?1,error_code=?2,updated_at=MAX(updated_at,?3) WHERE operation_id=?4",params![phase.as_str(),code.map(Code::as_str),now,id.as_str()])?;
    operation_projection(conn, id)
}

fn problem(
    conn: &Connection,
    work: &TokenWork,
    code: Code,
    now: i64,
) -> ControlResult<OperationView> {
    bump_evidence(conn, &work.record)?;
    project(conn, work, Some(code), false, now)
}

fn observed_identity(row: &Record, view: &NativeTokenView) -> ControlResult<i64> {
    ensure(
        view.schema_version == NATIVE_TOKEN_SCHEMA_VERSION,
        Code::InvalidResponse,
    )?;
    let id = native_id(&view.token_id)?;
    ensure(
        native_id(&view.owner_user_id)? == row.owner.user_id
            && view.owner_scope == row.owner.scope
            && row.native_id.is_none_or(|expected| expected == id)
            && view.explicit_group == row.explicit_group,
        Code::BindingDrift,
    )?;
    Ok(id)
}
fn matches_creation(work: &TokenWork, view: &NativeTokenView) -> ControlResult<i64> {
    let id = observed_identity(&work.record, view)?;
    let spec = work.create_spec()?;
    ensure(
        view.name == spec.correlation_name
            && view.expires_at == spec.expires_at
            && view.unlimited_quota
            && !view.model_limits_enabled
            && view.model_limits.is_empty()
            && view.allow_ips.as_deref().is_none_or(str::is_empty)
            && !view.cross_group_retry,
        Code::BindingDrift,
    )?;
    Ok(id)
}

impl CatalogStore {
    pub(crate) fn prepare_token_execution(
        &mut self,
        old: &TokenWork,
        now: i64,
    ) -> ControlResult<TokenTicket> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let work = current_work(&tx, old, now)?;
        ensure(
            work.operation.phase == OperationPhase::Queued
                && work.record.deleted_at.is_none()
                && !active_other(&tx, work.record.id, Some(&work.operation.operation_id))?
                && pending(&tx, work.record.id)? == (false, false),
            Code::NotReady,
        )?;
        if work.action == Action::Create {
            ensure(
                work.create_state == CreateState::Never
                    && creation_allowed(&tx, &work.record, now)?,
                Code::NotReady,
            )?;
        } else {
            ensure(work.record.native_id.is_some(), Code::NotReady)?;
        }
        // Step T5B.5: 票据只在prepared持久提交后返回，旧读/旧发送资格立即失效。
        let sequence = bump_evidence(&tx, &work.record)?;
        tx.execute("INSERT INTO token_execution_attempts(token_id,sequence,operation_id,token_revision,action,state,started_at) VALUES(?1,?2,?3,?4,?5,'prepared',?6)",params![work.record.id.get(),sequence,work.operation.operation_id.as_str(),work.record.revision,encode(work.action)?,now])?;
        tx.execute("UPDATE control_operations SET phase='applying',error_code=NULL,updated_at=MAX(updated_at,?1) WHERE operation_id=?2",params![now,work.operation.operation_id.as_str()])?;
        let ticket = TokenTicket {
            instance: work.instance,
            operation: work.operation.operation_id,
            record: work.record,
            sequence,
            action: work.action,
        };
        commit(tx, &mut self.poisoned)?;
        Ok(ticket)
    }
    pub(crate) fn token_send_allowed(&self, ticket: &TokenTicket, now: i64) -> ControlResult<bool> {
        self.usable()?;
        clock(now)?;
        let (row, state, _, _) = validate_ticket(&self.conn, ticket)?;
        let operation = operation_projection(&self.conn, &ticket.operation)?;
        Ok(row.evidence == ticket.sequence
            && row.revision == ticket.record.revision
            && row.deleted_at.is_none()
            && state == "prepared"
            && operation.phase == OperationPhase::Applying
            && (ticket.action != Action::Create || creation_allowed(&self.conn, &row, now)?))
    }
    pub(crate) fn settle_token_execution(
        &mut self,
        ticket: &TokenTicket,
        outcome: ExecutionOutcome,
        now: i64,
    ) -> ControlResult<()> {
        self.usable()?;
        clock(now)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (row, state, old_code, started) = validate_ticket(&tx, ticket)?;
        let (next, code) = outcome.columns();
        if state == next && old_code == code {
            return Ok(());
        }
        ensure(
            state == "prepared" || state == "uncertain",
            Code::RevisionConflict,
        )?;
        ensure(
            !(ticket.action == Action::Create
                && row.native_id.is_some()
                && matches!(outcome, ExecutionOutcome::NotSent(_))),
            Code::RevisionConflict,
        )?;
        // Step T5B.6: 交付证据与业务确认分离；迟到终止只属于原attempt。
        tx.execute("UPDATE token_execution_attempts SET state=?1,error_code=?2,finished_at=?3 WHERE token_id=?4 AND sequence=?5",params![next,code.map(Code::as_str),now.max(started),row.id.get(),ticket.sequence])?;
        bump_evidence(&tx, &row)?;
        let work = load_work(&tx, &ticket.operation, now)?;
        project(
            &tx,
            &work,
            code,
            matches!(
                outcome,
                ExecutionOutcome::NotSent(_) | ExecutionOutcome::Responded(Some(_))
            ),
            now,
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(())
    }
    pub(crate) fn confirm_token(
        &mut self,
        old: &TokenWork,
        observation: TokenObservation,
        now: i64,
    ) -> ControlResult<OperationView> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let work = current_work(&tx, old, now)?;
        if work.operation.phase == OperationPhase::Succeeded {
            return Ok(work.operation);
        }
        ensure(
            matches!(
                (&observation, work.action),
                (TokenObservation::Creation(_), Action::Create)
                    | (
                        TokenObservation::Existing(_),
                        Action::Enable | Action::Disable | Action::Delete
                    )
            ),
            Code::InvalidInput,
        )?;
        let decision: ControlResult<(i64, Option<TokenStatus>, bool)> = match observation {
            TokenObservation::Creation(lookup) => {
                if work.create_state == CreateState::Never {
                    return Err(failure(Code::NotReady));
                }
                match lookup {
                    CreationLookup::Missing => Err(failure(Code::TransportUnknown)),
                    CreationLookup::Ambiguous => Err(failure(Code::AmbiguousCreate)),
                    CreationLookup::One(view) => matches_creation(&work,&view).and_then(|id| {
                        let collision:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM managed_tokens WHERE native_token_id=?1 AND token_id!=?2)",params![id,work.record.id.get()],|r|r.get(0))?;
                        ensure(!collision,Code::BindingDrift)?;
                        Ok((id,Some(view.status),false))
                    }),
                }
            }
            TokenObservation::Existing(view) => match (work.action, view) {
                (Action::Delete, None) => Ok((
                    work.record
                        .native_id
                        .ok_or_else(|| failure(Code::Storage))?,
                    None,
                    true,
                )),
                (_, None) => Err(failure(Code::NotFound)),
                (action, Some(view)) => {
                    let checked = if work.record.creation.is_some() {
                        matches_creation(&work, &view)
                    } else {
                        observed_identity(&work.record, &view)
                    };
                    checked.and_then(|id| {
                        if action == Action::Delete {
                            return Err(failure(Code::NotReady));
                        }
                        ensure(
                            pending(&tx, work.record.id)? == (false, false),
                            Code::TransportUnknown,
                        )?;
                        ensure(
                            view.status
                                == if action == Action::Enable {
                                    TokenStatus::Enabled
                                } else {
                                    TokenStatus::Disabled
                                },
                            Code::NotReady,
                        )?;
                        Ok((id, Some(view.status), false))
                    })
                }
            },
        };
        let (native, status, missing) = match decision {
            Ok(result) => result,
            Err(e) if e.code != Code::Storage => {
                let result = problem(&tx, &work, e.code, now)?;
                commit(tx, &mut self.poisoned)?;
                return Ok(result);
            }
            Err(e) => return Err(e),
        };
        let evidence = bump_evidence(&tx, &work.record)?;
        ensure(now >= work.record.created_at, Code::InvalidInput)?;
        tx.execute("UPDATE managed_tokens SET native_token_id=?1,deleted_at=CASE WHEN ?2 THEN COALESCE(deleted_at,?3) ELSE deleted_at END WHERE token_id=?4",params![native,missing,now,work.record.id.get()])?;
        if matches!(work.action, Action::Create | Action::Delete) {
            tx.execute("UPDATE token_execution_attempts SET state='uncertain',error_code='transport_unknown',finished_at=MAX(started_at,?1) WHERE operation_id=?2 AND state='prepared'",params![now,work.operation.operation_id.as_str()])?;
        }
        let receipt = Receipt {
            format_version: 1,
            token_id: work.record.id,
            native_token_id: native.to_string(),
            owner_user_id: work.record.owner.user_id.to_string(),
            owner_scope: work.record.owner.scope,
            token_revision: work.record.revision,
            evidence_revision: evidence,
            status,
            missing,
            confirmed_at: now,
        };
        let encoded = serde_json::to_string(&receipt).map_err(|_| failure(Code::Storage))?;
        tx.execute("UPDATE token_operation_targets SET confirmed_at=?1,receipt_json=?2 WHERE operation_id=?3 AND confirmed_at IS NULL",params![now,encoded,work.operation.operation_id.as_str()])?;
        tx.execute("UPDATE control_operations SET phase='succeeded',error_code=NULL,updated_at=MAX(updated_at,?1) WHERE operation_id=?2",params![now,work.operation.operation_id.as_str()])?;
        let result = operation_projection(&tx, &work.operation.operation_id)?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
    pub(crate) fn note_token_problem(
        &mut self,
        old: &TokenWork,
        code: Code,
        now: i64,
    ) -> ControlResult<OperationView> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let work = current_work(&tx, old, now)?;
        if work.operation.phase == OperationPhase::Succeeded {
            return Ok(work.operation);
        }
        let result = problem(&tx, &work, code, now)?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
    pub(crate) fn recover_token_executions(&mut self) -> Result<()> {
        self.usable()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|v| i64::try_from(v.as_secs()).ok())
            .filter(|v| valid_timestamp(Some(*v)))
            .ok_or_else(|| error(ErrorKind::Storage))?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let affected: Vec<(i64, String)> = {
            let mut q=tx.prepare("SELECT token_id,operation_id FROM token_execution_attempts WHERE state='prepared' ORDER BY token_id")?;
            let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        require(affected.len() <= MAX_MANAGED_TOKENS, ErrorKind::CorruptData)?;
        for (id, op) in affected {
            let row =
                record(&tx, ManagedTokenId::new(id)?).map_err(|_| error(ErrorKind::CorruptData))?;
            bump_evidence(&tx, &row).map_err(|_| error(ErrorKind::Storage))?;
            tx.execute("UPDATE token_execution_attempts SET state='uncertain',error_code='transport_unknown',finished_at=MAX(started_at,?1) WHERE token_id=?2 AND state='prepared'",params![now,id])?;
            tx.execute("UPDATE control_operations SET phase='unknown',error_code='transport_unknown',updated_at=MAX(updated_at,?1) WHERE operation_id=?2 AND phase!='succeeded'",params![now,op])?;
        }
        commit(tx, &mut self.poisoned)
    }
}

pub(crate) fn retry_tx(
    conn: &Connection,
    id: &OperationId,
    now: i64,
) -> ControlResult<OperationView> {
    let work = load_work(conn, id, now)?;
    if work.operation.phase == OperationPhase::Succeeded {
        return Ok(work.operation);
    }
    ensure(
        work.record.revision == work.operation.target_revision
            && work.record.deleted_at.is_none()
            && !active_other(conn, work.record.id, Some(id))?,
        Code::RevisionConflict,
    )?;
    let (prepared, uncertain) = pending(conn, work.record.id)?;
    ensure(!prepared, Code::NotReady)?;
    ensure(
        !(uncertain && matches!(work.action, Action::Enable | Action::Disable)),
        Code::TransportUnknown,
    )?;
    let phase = if uncertain
        || (work.action == Action::Create && work.create_state == CreateState::MayHaveBeenSent)
    {
        OperationPhase::Unknown
    } else {
        OperationPhase::Queued
    };
    if work.operation.phase == OperationPhase::Queued && phase == OperationPhase::Queued {
        return Ok(work.operation);
    }
    bump_evidence(conn, &work.record)?;
    conn.execute("UPDATE control_operations SET phase=?1,error_code=?2,updated_at=MAX(updated_at,?3) WHERE operation_id=?4",params![phase.as_str(),if phase==OperationPhase::Unknown {Some(Code::TransportUnknown.as_str())}else{None},now,id.as_str()])?;
    operation_projection(conn, id)
}
