use super::*;
use std::collections::HashSet;

pub(crate) fn verify_tokens(conn: &Connection) -> Result<()> {
    check(conn).map_err(|_| error(ErrorKind::CorruptData))
}
fn check(conn: &Connection) -> ControlResult<()> {
    let n: i64 = conn.query_row("SELECT count(*) FROM managed_tokens", [], |r| r.get(0))?;
    ensure((0..=MAX_MANAGED_TOKENS as i64).contains(&n), Code::Storage)?;
    let broken:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM managed_tokens GROUP BY owner_user_id HAVING count(DISTINCT owner_scope)>1) OR EXISTS(SELECT 1 FROM control_operations o LEFT JOIN token_operation_targets t USING(operation_id) WHERE (o.target_type='token')!=(t.operation_id IS NOT NULL)) OR EXISTS(SELECT 1 FROM operation_targets t JOIN control_operations o USING(operation_id) WHERE o.target_type='token') OR EXISTS(SELECT 1 FROM token_operation_targets t JOIN control_operations o USING(operation_id) WHERE o.phase NOT IN ('succeeded','failed') GROUP BY t.token_id HAVING count(*)>1)",[],|r|r.get(0))?;
    ensure(!broken, Code::Storage)?;
    let inst = instance(conn)?;
    let mut names = HashSet::new();
    let mut records = conn.prepare("SELECT token_id FROM managed_tokens ORDER BY token_id")?;
    for id in records.query_map([], |r| r.get::<_, i64>(0))? {
        let row = record(conn, ManagedTokenId::new(id?)?)?;
        ensure(
            row.owner.user_id > 0
                && row.native_id.is_none_or(|id| id > 0)
                && label(&row.display_name)
                && row.explicit_group.len() <= 256
                && positive_revision(row.revision)
                && (0..=JSON_SAFE).contains(&row.evidence)
                && valid_timestamp(Some(row.created_at))
                && valid_timestamp(row.deleted_at)
                && valid_timestamp(row.expires_at)
                && row
                    .deleted_at
                    .is_none_or(|at| at >= row.created_at && row.native_id.is_some()),
            Code::Storage,
        )?;
        // 反向证据：合法字段并不代表终态/命令版本可由已保存历史推出。
        let (count, distinct, first, last): (i64, i64, Option<i64>, Option<i64>) = conn.query_row(
            "SELECT count(*),count(DISTINCT token_revision),min(token_revision),max(token_revision) FROM token_operation_targets WHERE token_id=?1",
            [row.id.get()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let start = if row.creation.is_some() { 1 } else { 2 };
        ensure(
            count == row.revision - (start - 1)
                && distinct == count
                && (count == 0 || (first == Some(start) && last == Some(row.revision))),
            Code::Storage,
        )?;
        if let Some(deleted_at) = row.deleted_at {
            let proofs: i64 = conn.query_row("SELECT count(*) FROM token_operation_targets t JOIN control_operations o USING(operation_id) WHERE t.token_id=?1 AND o.kind='delete_token' AND o.phase='succeeded' AND t.confirmed_at=?2 AND json_extract(t.receipt_json,'$.missing')=1", params![row.id.get(),deleted_at], |r| r.get(0))?;
            ensure(proofs == 1, Code::Storage)?;
        }
        if let Some(group) = row.group {
            let name: String = conn.query_row(
                "SELECT newapi_group FROM source_groups WHERE group_id=?1",
                [group.get()],
                |r| r.get(0),
            )?;
            ensure(name == row.explicit_group, Code::Storage)?;
        }
        if let Some(op) = &row.creation {
            ensure(
                row.owner.scope == TokenOwnerScope::Issuer
                    && row.group.is_some()
                    && row.group_revision.is_some_and(positive_revision)
                    && valid_group(&row.explicit_group),
                Code::Storage,
            )?;
            ensure(names.insert(create_name(&inst, op)?), Code::Storage)?;
            let original = operation_projection(conn, op)?;
            ensure(
                original.kind == OperationKind::CreateToken
                    && original.target_id == row.id.get().to_string()
                    && original.target_revision == 1
                    && (row.native_id.is_none() || original.phase == OperationPhase::Succeeded),
                Code::Storage,
            )?;
            if row.native_id.is_some() {
                let sent: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM token_execution_attempts WHERE token_id=?1 AND operation_id=?2 AND action='create' AND state!='not_sent')", params![row.id.get(),op.as_str()], |r| r.get(0))?;
                ensure(sent, Code::Storage)?;
            }
        } else {
            ensure(
                row.native_id.is_some() && row.group_revision.is_none() && row.expires_at.is_none(),
                Code::Storage,
            )?;
        }
    }
    // 历史操作/attempt逐行验证，不把无界历史收进Vec或错误截断成1万条。
    let mut targets=conn.prepare("SELECT operation_id,token_id,token_revision,desired_enabled,confirmed_at,receipt_json FROM token_operation_targets ORDER BY operation_id")?;
    let mut targets = targets.query([])?;
    while let Some(target) = targets.next()? {
        let id = OperationId::new(target.get(0)?)?;
        let row = record(conn, ManagedTokenId::new(target.get(1)?)?)?;
        let revision: i64 = target.get(2)?;
        let enabled: Option<bool> = target.get(3)?;
        let confirmed: Option<i64> = target.get(4)?;
        let receipt: Option<String> = target.get(5)?;
        let op = operation_projection(conn, &id)?;
        let act = action(op.kind, enabled)?;
        ensure(
            op.target_type == super::super::control::TargetType::Token
                && op.target_id == row.id.get().to_string()
                && op.target_revision == revision
                && revision <= row.revision
                && positive_revision(revision)
                && op.total_targets == 1
                && !matches!(
                    op.phase,
                    OperationPhase::Partial | OperationPhase::Superseded
                )
                && (confirmed.is_some() == (op.phase == OperationPhase::Succeeded))
                && (confirmed.is_some() == receipt.is_some()),
            Code::Storage,
        )?;
        if act == Action::Create {
            ensure(row.creation.as_ref() == Some(&id), Code::Storage)?;
        }
        if !matches!(op.phase, OperationPhase::Succeeded | OperationPhase::Failed) {
            ensure(
                revision == row.revision && row.deleted_at.is_none(),
                Code::Storage,
            )?;
        }
        let (prepared, uncertain) = operation_pending(conn, &row, &id)?;
        let may =
            act == Action::Create && create_state(conn, &row)? == CreateState::MayHaveBeenSent;
        ensure(
            !prepared || op.phase == OperationPhase::Applying,
            Code::Storage,
        )?;
        ensure(
            !uncertain || op.phase == OperationPhase::Unknown,
            Code::Storage,
        )?;
        ensure(
            op.phase != OperationPhase::Applying || prepared,
            Code::Storage,
        )?;
        ensure(
            op.phase != OperationPhase::Unknown || uncertain || may,
            Code::Storage,
        )?;
        ensure(
            !(may && matches!(op.phase, OperationPhase::Failed | OperationPhase::Queued)),
            Code::Storage,
        )?;
        if let Some(encoded) = receipt {
            let r: execution::Receipt =
                serde_json::from_str(&encoded).map_err(|_| failure(Code::Storage))?;
            ensure(
                r.format_version == 1
                    && r.token_id == row.id
                    && Some(native_id(&r.native_token_id)?) == row.native_id
                    && native_id(&r.owner_user_id)? == row.owner.user_id
                    && r.owner_scope == row.owner.scope
                    && r.token_revision == revision
                    && positive_revision(r.evidence_revision)
                    && r.evidence_revision <= row.evidence
                    && Some(r.confirmed_at) == confirmed
                    && r.confirmed_at >= row.created_at
                    && valid_timestamp(confirmed)
                    && r.missing == (act == Action::Delete)
                    && r.status.is_none() == r.missing,
                Code::Storage,
            )?;
            match act {
                Action::Enable => ensure(r.status == Some(TokenStatus::Enabled), Code::Storage)?,
                Action::Disable => ensure(r.status == Some(TokenStatus::Disabled), Code::Storage)?,
                Action::Delete => ensure(row.deleted_at == confirmed, Code::Storage)?,
                Action::Create => {}
            }
        }
    }
    let mut attempts=conn.prepare("SELECT token_id,sequence,operation_id,token_revision,action,state,error_code,started_at,finished_at FROM token_execution_attempts ORDER BY token_id,sequence")?;
    let mut attempts = attempts.query([])?;
    while let Some(a) = attempts.next()? {
        let row = record(conn, ManagedTokenId::new(a.get(0)?)?)?;
        let sequence: i64 = a.get(1)?;
        let op = OperationId::new(a.get(2)?)?;
        let revision: i64 = a.get(3)?;
        let act: Action = decode(a.get(4)?)?;
        let state: String = a.get(5)?;
        let code = a
            .get::<_, Option<String>>(6)?
            .map(|v| Code::parse(&v))
            .transpose()?;
        let started: i64 = a.get(7)?;
        let finished: Option<i64> = a.get(8)?;
        let work = load_work(conn, &op, 0)?;
        ensure(
            work.record.id == row.id
                && act == work.action
                && revision == work.operation.target_revision
                && positive_revision(sequence)
                && sequence <= row.evidence
                && valid_timestamp(Some(started))
                && valid_timestamp(finished)
                && finished.is_none_or(|at| at >= started),
            Code::Storage,
        )?;
        ensure(
            match state.as_str() {
                "prepared" => finished.is_none() && code.is_none(),
                "uncertain" => finished.is_some() && code == Some(Code::TransportUnknown),
                "not_sent" => finished.is_some() && code.is_some(),
                "responded" => finished.is_some(),
                _ => false,
            },
            Code::Storage,
        )?;
    }
    Ok(())
}
fn operation_pending(
    conn: &Connection,
    row: &Record,
    id: &OperationId,
) -> ControlResult<(bool, bool)> {
    Ok(conn.query_row(&format!("SELECT COALESCE(MAX(a.state='prepared'),0),COALESCE(MAX(a.state='uncertain'),0) FROM token_execution_attempts a JOIN managed_tokens t USING(token_id) WHERE a.operation_id=?1 AND t.token_id=?2 AND ({PENDING})"),params![id.as_str(),row.id.get()],|r|Ok((r.get(0)?,r.get(1)?)))?)
}
