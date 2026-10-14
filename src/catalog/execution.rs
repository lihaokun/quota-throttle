//! 持久执行日志。只记录计划中的写及其终止证据，不发送 HTTP，也不确认操作成功。
use super::control::{bump, ensure, failure, member_target, ControlErrorCode, ControlResult};
use super::store::{
    commit, decode_group_view, decode_membership_view, decode_source_view, CatalogStore,
    GROUP_COLUMNS, MEMBER_COLUMNS, SOURCE_COLUMNS,
};
use super::types::*;
use crate::newapi::managed_channels::ChannelPatch;
use crate::reconcile::planner::{
    plan_next_action, ChannelIdentity, PendingWrite, PlanContext, PlanStep, Revision,
    StampedObservation, WorkSnapshot,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

/// 查询别名固定为 a=attempt、m=membership、c=control；仅此常量定义创建物化豁免。
pub(super) const PENDING_EFFECT_SQL: &str =
    "a.action!='create' OR m.channel_id IS NULL OR c.create_state!='resolved'";

/// 白名单效果；无 credential、请求头或原生渠道正文。仅私有数据库序列化。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionEffect {
    Create {
        name: String,
        channel_type: i64,
        group: String,
        base_url: String,
        models: Vec<String>,
        priority: i64,
        kind: SourceKind,
    },
    Fields {
        priority: Option<i64>,
        models: Option<String>,
        disable_auto_ban: bool,
    },
    Enable,
    Disable,
}
impl fmt::Debug for ExecutionEffect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExecutionEffect([PRIVATE])")
    }
}
impl ExecutionEffect {
    fn action(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create",
            Self::Fields { .. } => "fields",
            Self::Enable => "enable",
            Self::Disable => "disable",
        }
    }
    fn from_plan(step: PlanStep, work: &WorkSnapshot) -> ControlResult<Self> {
        Ok(match step {
            PlanStep::CreateDisabled => Self::Create {
                name: work.target.create_name.clone(),
                channel_type: work.identity.channel_type,
                group: work.identity.group.clone(),
                base_url: work.identity.base_url.clone(),
                models: work.models.clone(),
                priority: work.target.desired_priority,
                kind: work.source.kind,
            },
            PlanStep::Patch(ChannelPatch {
                priority,
                models,
                disable_auto_ban,
            }) => Self::Fields {
                priority,
                models,
                disable_auto_ban,
            },
            PlanStep::Enable => Self::Enable,
            PlanStep::Disable => Self::Disable,
            PlanStep::Wait(code) | PlanStep::Reject(code) => return Err(failure(code)),
            _ => return Err(failure(ControlErrorCode::NotReady)),
        })
    }
}

/// 无 Deserialize/公开构造器；只能由已提交的 prepare_execution 产生。
#[derive(Clone, PartialEq, Eq)]
pub struct WorkTicket {
    catalog_instance_id: String,
    membership_id: MembershipId,
    revision: Revision,
    enable_epoch: i64,
    effect: ExecutionEffect,
}
impl fmt::Debug for WorkTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkTicket([PRIVATE])")
    }
}
impl WorkTicket {
    pub fn membership_id(&self) -> MembershipId {
        self.membership_id
    }
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn effect(&self) -> &ExecutionEffect {
        &self.effect
    }
}

/// 可信执行器提供的证据，不接受用户/远端 JSON。Responded(None) 是明确成功回执，仍非回读确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionOutcome {
    NotSent(ControlErrorCode),
    Responded(Option<ControlErrorCode>),
    Uncertain,
}
impl ExecutionOutcome {
    fn columns(self) -> (&'static str, Option<ControlErrorCode>) {
        match self {
            Self::NotSent(e) => ("not_sent", Some(e)),
            Self::Responded(e) => ("responded", e),
            Self::Uncertain => ("uncertain", Some(ControlErrorCode::TransportUnknown)),
        }
    }
}

impl CatalogStore {
    pub fn load_work(
        &mut self,
        id: MembershipId,
        policy_disabled: bool,
    ) -> ControlResult<WorkSnapshot> {
        self.usable()?;
        let tx = self.conn.transaction()?;
        load_work(&tx, id, policy_disabled)
    }

    /// 完整可能路由集合包含退役中成员；不复用可编辑 priority 集。
    pub fn load_group_work(&mut self, group: GroupId) -> ControlResult<Vec<WorkSnapshot>> {
        self.usable()?;
        let tx = self.conn.transaction()?;
        group_work(&tx, group)
    }

    pub fn prepare_execution(
        &mut self,
        id: MembershipId,
        expected: Revision,
        policy_disabled: bool,
        observation: Option<&StampedObservation>,
        context: &PlanContext<'_>,
    ) -> ControlResult<WorkTicket> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let work = load_work(&tx, id, policy_disabled)?;
        ensure(
            work.revision() == expected,
            ControlErrorCode::RevisionConflict,
        )?;
        // Step P13: 在当前目录重新计划，先持久 prepared/May，再允许执行器获取票据。
        let step = plan_next_action(&work, observation, context);
        let routes = matches!(&step, PlanStep::Enable)
            || matches!(&step,PlanStep::Patch(p) if p.priority.is_some_and(|priority| observation.is_some_and(|o| o.channel.status==1 && priority>o.channel.priority)));
        if routes {
            verify_peers(&tx, &work, context)?;
        }
        let effect = ExecutionEffect::from_plan(step, &work)?;
        let sequence = bump(work.execution_revision)?;
        let epoch: i64 = tx.query_row(
            "SELECT enable_epoch FROM member_execution WHERE membership_id=?1",
            [id.get()],
            |r| r.get(0),
        )?;
        ensure(
            !matches!(effect, ExecutionEffect::Enable) || epoch > 0,
            ControlErrorCode::NotReady,
        )?;
        let json =
            serde_json::to_string(&effect).map_err(|_| failure(ControlErrorCode::Storage))?;
        ensure(
            json.len() <= crate::newapi::transport::SINGLE_BODY_LIMIT,
            ControlErrorCode::InvalidInput,
        )?;
        tx.execute(
            "UPDATE member_execution SET sequence=?1 WHERE membership_id=?2",
            params![sequence, id.get()],
        )?;
        tx.execute("INSERT INTO execution_attempts(membership_id,sequence,member_revision,source_revision,group_revision,enable_epoch,action,effect_json,state,started_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'prepared',?9)",params![id.get(),sequence,expected.member,expected.source,expected.group,epoch,effect.action(),json,context.now])?;
        if matches!(effect, ExecutionEffect::Create { .. }) {
            tx.execute("UPDATE member_control SET create_state='may_have_been_sent' WHERE membership_id=?1",[id.get()])?;
        }
        tx.execute(
            "UPDATE member_control SET dirty=1 WHERE membership_id=?1",
            [id.get()],
        )?;
        // 尚未完成的操作中再次写这个成员，旧回读不能替代本次写后的确认。
        // 已 succeeded 的历史收据不修改，GLM 已消费/曾 applied 的授权也不重新打开。
        tx.execute("UPDATE operation_targets SET confirmed_at=NULL,receipt_json=NULL WHERE membership_id=?1 AND target_member_revision=?2 AND operation_id IN (SELECT operation_id FROM control_operations WHERE phase NOT IN ('succeeded','superseded'))",params![id.get(),expected.member])?;
        mark_operations(&tx, id, expected.member, "applying", None, context.now)?;
        let ticket = WorkTicket {
            catalog_instance_id: tx.query_row(
                "SELECT catalog_instance_id FROM catalog_meta",
                [],
                |r| r.get(0),
            )?,
            membership_id: id,
            revision: Revision {
                execution: sequence,
                ..expected
            },
            enable_epoch: epoch,
            effect,
        };
        commit(tx, &mut self.poisoned)?;
        Ok(ticket)
    }

    /// 只结算原尝试；即使成功，observed/applied 和 operation_targets 也保持不变。
    pub fn settle_execution(
        &mut self,
        ticket: &WorkTicket,
        outcome: ExecutionOutcome,
        now: i64,
    ) -> ControlResult<()> {
        self.usable()?;
        ensure(valid_timestamp(Some(now)), ControlErrorCode::InvalidInput)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let instance: String =
            tx.query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
                r.get(0)
            })?;
        ensure(
            instance == ticket.catalog_instance_id,
            ControlErrorCode::RevisionConflict,
        )?;
        let row=tx.query_row("SELECT member_revision,source_revision,group_revision,enable_epoch,effect_json,state,error_code,started_at FROM execution_attempts WHERE membership_id=?1 AND sequence=?2",params![ticket.membership_id.get(),ticket.revision.execution],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,i64>(7)?))).optional()?.ok_or_else(||failure(ControlErrorCode::NotFound))?;
        let effect: ExecutionEffect =
            serde_json::from_str(&row.4).map_err(|_| failure(ControlErrorCode::Storage))?;
        ensure(
            (row.0, row.1, row.2, row.3)
                == (
                    ticket.revision.member,
                    ticket.revision.source,
                    ticket.revision.group,
                    ticket.enable_epoch,
                )
                && effect == ticket.effect,
            ControlErrorCode::RevisionConflict,
        )?;
        let (state, code) = outcome.columns();
        if row.5 == state && row.6.as_deref() == code.map(ControlErrorCode::as_str) {
            return Ok(());
        }
        ensure(
            row.5 == "prepared" || row.5 == "uncertain",
            ControlErrorCode::RevisionConflict,
        )?;
        // Step P14: 迟到票只能结算自身；新授权不被旧成功消费，新目标没有成功收据。
        tx.execute("UPDATE execution_attempts SET state=?1,error_code=?2,finished_at=?3 WHERE membership_id=?4 AND sequence=?5",params![state,code.map(ControlErrorCode::as_str),now.max(row.7),ticket.membership_id.get(),ticket.revision.execution])?;
        let current = load_work(&tx, ticket.membership_id, false)?;
        ensure(
            !(matches!(outcome, ExecutionOutcome::NotSent(_))
                && matches!(effect, ExecutionEffect::Create { .. })
                && current.member.channel_id.is_some()),
            ControlErrorCode::RevisionConflict,
        )?;
        // 终止证据改变 pending 状态，也必须使在终止之前发起的 GET 失效。
        tx.execute(
            "UPDATE member_execution SET sequence=?1 WHERE membership_id=?2",
            params![
                bump(current.execution_revision)?,
                ticket.membership_id.get()
            ],
        )?;
        if matches!(outcome, ExecutionOutcome::Responded(None))
            && matches!(effect, ExecutionEffect::Enable)
        {
            tx.execute("UPDATE member_execution SET enable_consumed=1 WHERE membership_id=?1 AND enable_epoch=?2",params![ticket.membership_id.get(),ticket.enable_epoch])?;
        }
        if matches!(outcome, ExecutionOutcome::NotSent(_))
            && matches!(effect, ExecutionEffect::Create { .. })
        {
            tx.execute("UPDATE member_control SET create_state='never' WHERE membership_id=?1 AND create_state='may_have_been_sent' AND NOT EXISTS(SELECT 1 FROM execution_attempts WHERE membership_id=?1 AND action='create' AND state!='not_sent') AND NOT EXISTS(SELECT 1 FROM memberships WHERE membership_id=?1 AND channel_id IS NOT NULL)",[ticket.membership_id.get()])?;
        }
        tx.execute(
            "UPDATE member_control SET dirty=1 WHERE membership_id=?1",
            [ticket.membership_id.get()],
        )?;
        if (
            current.target.revision,
            current.source.config_revision,
            current.group.revision,
        ) == (
            ticket.revision.member,
            ticket.revision.source,
            ticket.revision.group,
        ) {
            mark_operations(
                &tx,
                ticket.membership_id,
                ticket.revision.member,
                "waiting",
                code,
                now,
            )?;
        }
        commit(tx, &mut self.poisoned)?;
        Ok(())
    }

    pub(super) fn recover_executions(&mut self) -> Result<()> {
        self.usable()?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .filter(|n| valid_timestamp(Some(*n)))
            .ok_or_else(|| error(ErrorKind::Storage))?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let affected: Vec<(i64, i64)> = {
            let mut q=tx.prepare("SELECT DISTINCT c.membership_id,c.revision FROM member_control c JOIN execution_attempts a USING(membership_id) WHERE a.state='prepared'")?;
            let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        // Step P16: 崩溃窗口保守按未知；不清暂停、原始创建意图或历史映射。
        tx.execute("UPDATE member_execution SET sequence=sequence+1 WHERE membership_id IN (SELECT membership_id FROM execution_attempts WHERE state='prepared')",[])?;
        tx.execute("UPDATE member_control SET dirty=1,next_attempt_at=0 WHERE membership_id IN (SELECT membership_id FROM execution_attempts WHERE state='prepared')",[])?;
        tx.execute("UPDATE execution_attempts SET state='uncertain',error_code='transport_unknown',finished_at=MAX(started_at,?1) WHERE state='prepared'",[now])?;
        for (id, revision) in affected {
            super::confirmation::refresh_operations(
                &tx,
                MembershipId::new(id)?,
                revision,
                super::control::OperationPhase::Waiting,
                None,
                now,
            )
            .map_err(|_| error(ErrorKind::CorruptData))?;
        }
        commit(tx, &mut self.poisoned)
    }
}

fn mark_operations(
    conn: &Connection,
    id: MembershipId,
    revision: i64,
    phase: &str,
    code: Option<ControlErrorCode>,
    now: i64,
) -> ControlResult<()> {
    super::confirmation::refresh_operations(
        conn,
        id,
        revision,
        super::control::OperationPhase::parse(phase)?,
        code,
        now,
    )
}

pub(super) fn load_work(
    conn: &Connection,
    id: MembershipId,
    policy_disabled: bool,
) -> ControlResult<WorkSnapshot> {
    let mut q = conn.prepare(&format!(
        "SELECT {MEMBER_COLUMNS} FROM memberships WHERE membership_id=?1"
    ))?;
    let mut rows = q.query([id.get()])?;
    let member = decode_membership_view(
        rows.next()?
            .ok_or_else(|| failure(ControlErrorCode::NotFound))?,
    )?;
    let mut q = conn.prepare(&format!(
        "SELECT {SOURCE_COLUMNS} FROM sources WHERE source_id=?1"
    ))?;
    let mut rows = q.query([member.source_id.get()])?;
    let source = decode_source_view(
        rows.next()?
            .ok_or_else(|| failure(ControlErrorCode::Storage))?,
    )?;
    let mut q = conn.prepare(&format!(
        "SELECT {GROUP_COLUMNS} FROM source_groups WHERE group_id=?1"
    ))?;
    let mut rows = q.query([member.group_id.get()])?;
    let group = decode_group_view(
        rows.next()?
            .ok_or_else(|| failure(ControlErrorCode::Storage))?,
    )?;
    let target = member_target(conn, id)?;
    let json: String = conn.query_row(
        "SELECT settings_json FROM sources WHERE source_id=?1",
        [source.source_id.get()],
        |r| r.get(0),
    )?;
    let upstream = match decode_settings(&json)? {
        SourceSettings::Glm { upstream, .. } | SourceSettings::Volcano { upstream, .. } => upstream,
    };
    let (sequence,consumed,origin):(i64,bool,i64)=conn.query_row("SELECT sequence,enable_consumed,enable_requested_revision FROM member_execution WHERE membership_id=?1",[id.get()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let (prepared, unknown) = pending_writes(conn, id)?;
    let managed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM operation_targets WHERE membership_id=?1)",
        [id.get()],
        |r| r.get(0),
    )?;
    let retire: bool = conn.query_row(
        "SELECT retire_requested FROM source_control WHERE source_id=?1",
        [source.source_id.get()],
        |r| r.get(0),
    )?;
    Ok(WorkSnapshot {
        identity: ChannelIdentity {
            channel_type: upstream.channel_type,
            group: group.newapi_group.clone(),
            base_url: upstream.base_url,
        },
        models: upstream.models,
        source,
        group,
        member,
        target,
        managed,
        source_retire_requested: retire,
        policy_disabled,
        pending_write: if prepared {
            PendingWrite::InFlight
        } else if unknown {
            PendingWrite::Unknown
        } else {
            PendingWrite::None
        },
        execution_revision: sequence,
        enable_consumed: consumed,
        enable_requested_revision: origin,
    })
}

pub(super) fn pending_writes(conn: &Connection, id: MembershipId) -> ControlResult<(bool, bool)> {
    // 唯一绑定证明单次禁用创建已物化，不伪造 HTTP 响应；一般未知写不豁免。
    Ok(conn.query_row(&format!("SELECT COALESCE(MAX(a.state='prepared'),0),COALESCE(MAX(a.state='uncertain'),0) FROM execution_attempts a JOIN memberships m USING(membership_id) JOIN member_control c USING(membership_id) WHERE a.membership_id=?1 AND ({PENDING_EFFECT_SQL})"),[id.get()],|r|Ok((r.get(0)?,r.get(1)?)))?)
}

pub(super) fn read_ticket(
    conn: &Connection,
    id: MembershipId,
    sequence: i64,
) -> ControlResult<WorkTicket> {
    let row=conn.query_row("SELECT member_revision,source_revision,group_revision,enable_epoch,effect_json FROM execution_attempts WHERE membership_id=?1 AND sequence=?2",params![id.get(),sequence],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?))).optional()?.ok_or_else(||failure(ControlErrorCode::NotFound))?;
    Ok(WorkTicket {
        catalog_instance_id: conn.query_row(
            "SELECT catalog_instance_id FROM catalog_meta",
            [],
            |r| r.get(0),
        )?,
        membership_id: id,
        revision: Revision {
            member: row.0,
            source: row.1,
            group: row.2,
            execution: sequence,
        },
        enable_epoch: row.3,
        effect: serde_json::from_str(&row.4).map_err(|_| failure(ControlErrorCode::Storage))?,
    })
}

fn group_work(conn: &Connection, group: GroupId) -> ControlResult<Vec<WorkSnapshot>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM source_groups WHERE group_id=?1)",
        [group.get()],
        |r| r.get(0),
    )?;
    ensure(exists, ControlErrorCode::NotFound)?;
    let mut statement=conn.prepare("SELECT membership_id FROM memberships WHERE group_id=?1 AND lifecycle!='retired' ORDER BY membership_id")?;
    let mut rows = statement.query([group.get()])?;
    let mut work = Vec::new();
    while let Some(row) = rows.next()? {
        ensure(work.len() < MAX_MEMBERSHIPS, ControlErrorCode::Storage)?;
        work.push(load_work(conn, MembershipId::new(row.get(0)?)?, false)?);
    }
    Ok(work)
}

fn verify_peers(
    conn: &Connection,
    own: &WorkSnapshot,
    context: &PlanContext<'_>,
) -> ControlResult<()> {
    let group = group_work(conn, own.group.group_id)?;
    let mut expected = context.expected_members.to_vec();
    expected.sort();
    ensure(
        expected
            == group
                .iter()
                .map(|w| w.member.membership_id)
                .collect::<Vec<_>>(),
        ControlErrorCode::RevisionConflict,
    )?;
    for work in group
        .iter()
        .filter(|w| w.member.membership_id != own.member.membership_id)
    {
        let p = context
            .peers
            .iter()
            .find(|p| p.member_id == work.member.membership_id)
            .ok_or_else(|| failure(ControlErrorCode::RevisionConflict))?;
        ensure(
            p.revision == work.revision()
                && p.group_id == work.group.group_id
                && p.channel_id == work.member.channel_id
                && p.create_state == work.target.create_state
                && p.pending_write == work.pending_write
                && p.desired_priority == work.target.desired_priority
                && p.identity.channel_type == work.identity.channel_type
                && p.identity.group == work.identity.group
                && p.identity.base_url == work.identity.base_url,
            ControlErrorCode::RevisionConflict,
        )?;
        // policy 可以增加停止约束，不能掩盖持久暂停/退休/disabled。
        if work.source.manual_paused
            || work.source_retire_requested
            || work.target.retire_requested
            || work.target.status_directive == super::control::StatusDirective::Disabled
        {
            ensure(p.desired_disabled, ControlErrorCode::RevisionConflict)?;
        }
    }
    Ok(())
}

pub(super) fn verify_execution(conn: &Connection) -> Result<()> {
    let broken:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM memberships m LEFT JOIN member_execution e USING(membership_id) WHERE e.membership_id IS NULL) OR EXISTS(SELECT 1 FROM member_execution e JOIN member_control c USING(membership_id) WHERE e.enable_requested_revision>c.revision OR e.enable_epoch>c.revision OR (c.status_directive='enabled' AND e.enable_epoch=0)) OR EXISTS(SELECT 1 FROM execution_attempts a JOIN member_execution e USING(membership_id) JOIN member_control c USING(membership_id) JOIN memberships m USING(membership_id) JOIN sources s USING(source_id) JOIN source_groups g USING(group_id) WHERE a.sequence>e.sequence OR a.member_revision>c.revision OR a.source_revision>s.config_revision OR a.group_revision>g.revision OR a.enable_epoch>e.enable_epoch OR (a.action='enable' AND a.enable_epoch=0) OR (a.finished_at IS NOT NULL AND a.finished_at<a.started_at)) OR EXISTS(SELECT membership_id FROM execution_attempts WHERE state='prepared' GROUP BY membership_id HAVING count(*)>1)",[],|r|r.get(0))?;
    require(!broken, ErrorKind::CorruptData)?;
    let mut statement=conn.prepare("SELECT action,effect_json,state,error_code FROM execution_attempts ORDER BY membership_id,sequence")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let action: String = row.get(0)?;
        let json: String = row.get(1)?;
        require(
            json.len() <= crate::newapi::transport::SINGLE_BODY_LIMIT,
            ErrorKind::CorruptData,
        )?;
        let effect: ExecutionEffect =
            serde_json::from_str(&json).map_err(|_| error(ErrorKind::CorruptData))?;
        require(effect.action() == action, ErrorKind::CorruptData)?;
        let state: String = row.get(2)?;
        let code: Option<String> = row.get(3)?;
        require(
            (state != "uncertain" || code.as_deref() == Some("transport_unknown"))
                && (state != "not_sent" || code.is_some())
                && (state != "prepared" || code.is_none()),
            ErrorKind::CorruptData,
        )?;
    }
    Ok(())
}
