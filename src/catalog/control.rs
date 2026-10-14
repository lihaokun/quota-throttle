//! 持久控制意图及白名单回执；不执行 HTTP，不生成远端确认。
use super::store::{commit, CatalogStore};
use super::types::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

pub const OPERATION_SCHEMA_VERSION: i64 = 1;
macro_rules! control_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename=$wire)] $variant),+ }
        impl $name {
            pub const VALUES: &'static [&'static str] = &[$($wire),+];
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant=>$wire),+ } }
            pub(super) fn parse(value: &str) -> ControlResult<Self> {
                match value { $($wire=>Ok(Self::$variant)),+, _=>Err(failure(ControlErrorCode::Storage)) }
            }
        }
    }
}
control_enum!(ControlErrorCode {
    InvalidInput=>"invalid_input", NotFound=>"not_found", RevisionConflict=>"revision_conflict",
    IdempotencyConflict=>"idempotency_conflict", QueueFull=>"queue_full", NotReady=>"not_ready",
    BindingDrift=>"binding_drift", AmbiguousCreate=>"ambiguous_create", AuthFailed=>"auth_failed",
    RemoteRejected=>"remote_rejected", InvalidResponse=>"invalid_response", RateLimited=>"rate_limited",
    TransportUnknown=>"transport_unknown", Storage=>"storage"
});
control_enum!(OperationKind {
    EnsureMember=>"ensure_member", SetSourcePaused=>"set_source_paused", RetireMember=>"retire_member",
    RetireSource=>"retire_source", SetGroupPriorities=>"set_group_priorities", SyncSourceModels=>"sync_source_models"
});
control_enum!(OperationPhase { Queued=>"queued", Applying=>"applying", Waiting=>"waiting", Partial=>"partial",
    Unknown=>"unknown", Succeeded=>"succeeded", Failed=>"failed", Superseded=>"superseded" });
control_enum!(TargetType { Source=>"source", Group=>"group", Membership=>"membership" });
control_enum!(StatusDirective { Preserve=>"preserve", Enabled=>"enabled", Disabled=>"disabled" });
control_enum!(CreateState { Never=>"never", MayHaveBeenSent=>"may_have_been_sent", Resolved=>"resolved" });

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlError {
    pub code: ControlErrorCode,
}
impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "control: {}", self.code.as_str())
    }
}
impl std::error::Error for ControlError {}
impl From<CatalogError> for ControlError {
    fn from(_: CatalogError) -> Self {
        failure(ControlErrorCode::Storage)
    }
}
impl From<rusqlite::Error> for ControlError {
    fn from(_: rusqlite::Error) -> Self {
        failure(ControlErrorCode::Storage)
    }
}
pub type ControlResult<T> = std::result::Result<T, ControlError>;
pub(super) fn failure(code: ControlErrorCode) -> ControlError {
    ControlError { code }
}
pub(super) fn ensure(ok: bool, code: ControlErrorCode) -> ControlResult<()> {
    if ok {
        Ok(())
    } else {
        Err(failure(code))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OperationId(String);
impl OperationId {
    pub fn new(value: String) -> ControlResult<Self> {
        ensure(
            (16..=64).contains(&value.len())
                && value.as_bytes()[0].is_ascii_alphanumeric()
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
            ControlErrorCode::InvalidInput,
        )?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 进程内可信入口；没有任意 JSON patch、credential 或前端许可布尔值。
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlCommand {
    EnsureMember {
        membership_id: MembershipId,
        expected_revision: i64,
        status: StatusDirective,
    },
    SetSourcePaused {
        source_id: SourceId,
        expected_revision: i64,
        paused: bool,
    },
    RetireMember {
        membership_id: MembershipId,
        expected_revision: i64,
    },
    RetireSource {
        source_id: SourceId,
        expected_revision: i64,
    },
    SetGroupPriorities {
        group_id: GroupId,
        expected_revision: i64,
        priorities: Vec<(MembershipId, i64)>,
    },
    SyncSourceModels {
        source_id: SourceId,
        expected_revision: i64,
        models: Vec<String>,
    },
}
impl fmt::Debug for ControlCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ControlCommand([PRIVATE])")
    }
}
impl ControlCommand {
    fn target(&self) -> (OperationKind, TargetType, i64, i64) {
        match self {
            Self::EnsureMember {
                membership_id,
                expected_revision,
                ..
            } => (
                OperationKind::EnsureMember,
                TargetType::Membership,
                membership_id.get(),
                *expected_revision,
            ),
            Self::RetireMember {
                membership_id,
                expected_revision,
            } => (
                OperationKind::RetireMember,
                TargetType::Membership,
                membership_id.get(),
                *expected_revision,
            ),
            Self::SetSourcePaused {
                source_id,
                expected_revision,
                ..
            } => (
                OperationKind::SetSourcePaused,
                TargetType::Source,
                source_id.get(),
                *expected_revision,
            ),
            Self::RetireSource {
                source_id,
                expected_revision,
            } => (
                OperationKind::RetireSource,
                TargetType::Source,
                source_id.get(),
                *expected_revision,
            ),
            Self::SyncSourceModels {
                source_id,
                expected_revision,
                ..
            } => (
                OperationKind::SyncSourceModels,
                TargetType::Source,
                source_id.get(),
                *expected_revision,
            ),
            Self::SetGroupPriorities {
                group_id,
                expected_revision,
                ..
            } => (
                OperationKind::SetGroupPriorities,
                TargetType::Group,
                group_id.get(),
                *expected_revision,
            ),
        }
    }
    fn canonical(mut self) -> ControlResult<(Self, String)> {
        ensure(
            positive_revision(self.target().3),
            ControlErrorCode::InvalidInput,
        )?;
        match &mut self {
            Self::SetGroupPriorities { priorities, .. } => {
                ensure(
                    priorities.len() <= MAX_MEMBERSHIPS
                        && priorities.iter().all(|(_, p)| safe_number(*p)),
                    ControlErrorCode::InvalidInput,
                )?;
                priorities.sort_by_key(|(id, _)| *id);
                ensure(
                    !priorities.windows(2).any(|w| w[0].0 == w[1].0),
                    ControlErrorCode::InvalidInput,
                )?;
            }
            Self::SyncSourceModels { models, .. } => {
                ensure(
                    !models.is_empty()
                        && models.len() <= 1000
                        && models.iter().all(|m| !m.contains([',', '\0'])),
                    ControlErrorCode::InvalidInput,
                )?;
                *models = crate::model_catalog::normalize_models_csv(&models.join(","));
                models.sort();
                ensure(!models.is_empty(), ControlErrorCode::InvalidInput)?;
            }
            _ => {}
        }
        let encoded =
            serde_json::to_vec(&self).map_err(|_| failure(ControlErrorCode::InvalidInput))?;
        let digest = format!("{:x}", Sha256::digest(encoded));
        Ok((self, digest))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationView {
    pub schema_version: i64,
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub target_type: TargetType,
    pub target_id: String,
    pub phase: OperationPhase,
    pub target_revision: i64,
    pub total_targets: usize,
    pub confirmed_targets: usize,
    pub error_code: Option<ControlErrorCode>,
    pub updated_at: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberTarget {
    pub membership_id: MembershipId,
    pub revision: i64,
    pub desired_priority: i64,
    pub status_directive: StatusDirective,
    pub retire_requested: bool,
    pub create_state: CreateState,
    pub create_name: String,
    pub dirty: bool,
    pub next_attempt_at: i64,
    pub last_error_code: Option<ControlErrorCode>,
}

impl CatalogStore {
    pub fn operation(&self, id: &OperationId) -> ControlResult<OperationView> {
        self.usable()?;
        operation_projection(&self.conn, id)
    }
    pub fn member_target(&self, id: MembershipId) -> ControlResult<MemberTarget> {
        self.usable()?;
        member_target(&self.conn, id)
    }
    pub fn submit_operation(
        &mut self,
        id: OperationId,
        command: ControlCommand,
        now: i64,
    ) -> ControlResult<OperationView> {
        self.usable()?;
        ensure(valid_timestamp(Some(now)), ControlErrorCode::InvalidInput)?;
        let (command, digest) = command.canonical()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Step P9: 先查幂等 ID；旧 expected_revision 不影响完全相同的重放。
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_digest FROM control_operations WHERE operation_id=?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            ensure(existing == digest, ControlErrorCode::IdempotencyConflict)?;
            return operation_projection(&tx, &id);
        }
        let (kind, target_type, target_id, expected) = command.target();
        let current = target_revision(&tx, target_type, target_id)?;
        ensure(current == expected, ControlErrorCode::RevisionConflict)?;
        let next = bump(current)?;
        let members = affected_members(&tx, &command)?;
        // Step P10: 目标/实体 generation/操作及关联收据同事务，未提交不发布 queued。
        match &command {
            ControlCommand::SetSourcePaused { paused, .. } => {
                tx.execute(
                    "UPDATE sources SET manual_paused=?1,config_revision=?2 WHERE source_id=?3",
                    params![paused, next, target_id],
                )?;
            }
            ControlCommand::RetireSource { .. } => {
                tx.execute(
                    "UPDATE source_control SET retire_requested=1 WHERE source_id=?1",
                    [target_id],
                )?;
                tx.execute("UPDATE sources SET manual_paused=1,config_revision=?1,retired_at=?2 WHERE source_id=?3",params![next,if members.is_empty(){Some(now)}else{None},target_id])?;
            }
            ControlCommand::SyncSourceModels { models, .. } => {
                let json: String = tx.query_row(
                    "SELECT settings_json FROM sources WHERE source_id=?1",
                    [target_id],
                    |r| r.get(0),
                )?;
                let mut settings = decode_settings(&json)?;
                match &mut settings {
                    SourceSettings::Glm { upstream, .. }
                    | SourceSettings::Volcano { upstream, .. } => upstream.models = models.clone(),
                }
                let json = encode_settings(&settings)?;
                tx.execute(
                    "UPDATE sources SET settings_json=?1,config_revision=?2 WHERE source_id=?3",
                    params![json, next, target_id],
                )?;
            }
            ControlCommand::SetGroupPriorities { .. } => {
                tx.execute(
                    "UPDATE source_groups SET revision=?1 WHERE group_id=?2",
                    params![next, target_id],
                )?;
            }
            _ => {}
        }
        let phase = if members.is_empty() {
            OperationPhase::Succeeded
        } else {
            OperationPhase::Queued
        };
        tx.execute("INSERT INTO control_operations(operation_id,kind,target_type,target_id,payload_digest,target_revision,expected_targets,phase,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",params![id.as_str(),kind.as_str(),target_type.as_str(),target_id,digest,next,members.len() as i64,phase.as_str(),now])?;
        for mut member in members {
            let revision = bump(member.revision)?;
            match &command {
                ControlCommand::EnsureMember { status, .. } => member.status_directive = *status,
                ControlCommand::SetSourcePaused { paused, .. } => {
                    member.status_directive = if *paused {
                        StatusDirective::Disabled
                    } else {
                        StatusDirective::Enabled
                    }
                }
                ControlCommand::RetireSource { .. } | ControlCommand::RetireMember { .. } => {
                    member.retire_requested = true;
                    member.status_directive = StatusDirective::Disabled;
                }
                ControlCommand::SetGroupPriorities { priorities, .. } => {
                    member.desired_priority = priorities
                        .binary_search_by_key(&member.membership_id, |(id, _)| *id)
                        .ok()
                        .map(|index| priorities[index].1)
                        .ok_or_else(|| failure(ControlErrorCode::InvalidInput))?;
                }
                _ => {}
            }
            if member.retire_requested {
                member.status_directive = StatusDirective::Disabled;
            }
            if member.status_directive == StatusDirective::Enabled
                && matches!(
                    &command,
                    ControlCommand::EnsureMember {
                        status: StatusDirective::Enabled,
                        ..
                    } | ControlCommand::SetSourcePaused { paused: false, .. }
                )
            {
                let epoch: i64 = tx.query_row(
                    "SELECT enable_epoch FROM member_execution WHERE membership_id=?1",
                    [member.membership_id.get()],
                    |r| r.get(0),
                )?;
                tx.execute("UPDATE member_execution SET enable_epoch=?1,enable_requested_revision=?2,enable_consumed=0 WHERE membership_id=?3",params![bump(epoch)?,revision,member.membership_id.get()])?;
            }
            tx.execute("UPDATE control_operations SET phase='superseded',error_code=NULL,updated_at=?1 WHERE phase NOT IN ('succeeded','superseded') AND operation_id IN (SELECT operation_id FROM operation_targets WHERE membership_id=?2)",params![now,member.membership_id.get()])?;
            tx.execute("UPDATE member_control SET revision=?1,status_directive=?2,retire_requested=?3,dirty=1,next_attempt_at=?4,last_error_code=NULL WHERE membership_id=?5",params![revision,member.status_directive.as_str(),member.retire_requested,now,member.membership_id.get()])?;
            tx.execute("UPDATE memberships SET desired_priority=?1,applied_revision=NULL WHERE membership_id=?2",params![member.desired_priority,member.membership_id.get()])?;
            tx.execute("INSERT INTO operation_targets(operation_id,membership_id,target_member_revision) VALUES(?1,?2,?3)",params![id.as_str(),member.membership_id.get(),revision])?;
        }
        let view = operation_projection(&tx, &id)?;
        commit(tx, &mut self.poisoned)?;
        Ok(view)
    }
}

pub(super) fn bump(value: i64) -> ControlResult<i64> {
    value
        .checked_add(1)
        .filter(|v| positive_revision(*v))
        .ok_or_else(|| failure(ControlErrorCode::InvalidInput))
}
fn target_revision(conn: &Connection, target: TargetType, id: i64) -> ControlResult<i64> {
    let sql = match target {
        TargetType::Source => "SELECT config_revision FROM sources WHERE source_id=?1",
        TargetType::Group => "SELECT revision FROM source_groups WHERE group_id=?1",
        TargetType::Membership => "SELECT revision FROM member_control WHERE membership_id=?1",
    };
    conn.query_row(sql, [id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| failure(ControlErrorCode::NotFound))
}
pub(super) fn member_target(conn: &Connection, id: MembershipId) -> ControlResult<MemberTarget> {
    let row=conn.query_row("SELECT c.revision,m.desired_priority,c.status_directive,c.retire_requested,c.create_state,c.create_name,c.dirty,c.next_attempt_at,c.last_error_code FROM member_control c JOIN memberships m USING(membership_id) WHERE membership_id=?1",[id.get()],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,bool>(6)?,r.get::<_,i64>(7)?,r.get::<_,Option<String>>(8)?))).optional()?.ok_or_else(||failure(ControlErrorCode::NotFound))?;
    Ok(MemberTarget {
        membership_id: id,
        revision: row.0,
        desired_priority: row.1,
        status_directive: StatusDirective::parse(&row.2)?,
        retire_requested: row.3,
        create_state: CreateState::parse(&row.4)?,
        create_name: row.5,
        dirty: row.6,
        next_attempt_at: row.7,
        last_error_code: row.8.map(|s| ControlErrorCode::parse(&s)).transpose()?,
    })
}
fn affected_members(
    tx: &Transaction<'_>,
    command: &ControlCommand,
) -> ControlResult<Vec<MemberTarget>> {
    let (_, target, id, _) = command.target();
    let sql=match target {
        TargetType::Source=>{
            let active:bool=tx.query_row("SELECT s.retired_at IS NULL AND c.retire_requested=0 FROM sources s JOIN source_control c USING(source_id) WHERE source_id=?1",[id],|r|r.get(0))?;
            ensure(active,ControlErrorCode::NotReady)?;
            "SELECT membership_id FROM memberships WHERE source_id=?1 AND lifecycle!='retired' ORDER BY membership_id"
        }
        TargetType::Group=>"SELECT m.membership_id FROM memberships m JOIN member_control c USING(membership_id) JOIN sources s USING(source_id) JOIN source_control sc USING(source_id) WHERE m.group_id=?1 AND m.lifecycle!='retired' AND c.retire_requested=0 AND sc.retire_requested=0 AND s.retired_at IS NULL ORDER BY m.membership_id",
        TargetType::Membership=>{
            let active:bool=tx.query_row("SELECT m.lifecycle!='retired' AND c.retire_requested=0 AND s.retired_at IS NULL AND sc.retire_requested=0 FROM memberships m JOIN member_control c USING(membership_id) JOIN sources s USING(source_id) JOIN source_control sc USING(source_id) WHERE membership_id=?1",[id],|r|r.get(0))?;
            ensure(active,ControlErrorCode::NotReady)?;
            "SELECT membership_id FROM memberships WHERE membership_id=?1"
        }
    };
    let mut statement = tx.prepare(sql)?;
    let mut rows = statement.query([id])?;
    let mut members = Vec::new();
    while let Some(row) = rows.next()? {
        ensure(members.len() < MAX_MEMBERSHIPS, ControlErrorCode::Storage)?;
        let mut target = member_target(tx, MembershipId::new(row.get(0)?)?)?;
        if target.status_directive == StatusDirective::Enabled {
            let completed_glm:bool=tx.query_row("SELECT s.kind='glm' AND (COALESCE(m.applied_revision=?2,0) OR e.enable_consumed=1) FROM memberships m JOIN sources s USING(source_id) JOIN member_execution e USING(membership_id) WHERE membership_id=?1",params![target.membership_id.get(),target.revision],|r|r.get(0))?;
            if completed_glm {
                target.status_directive = StatusDirective::Preserve;
            }
        }
        members.push(target);
    }
    if let ControlCommand::SetGroupPriorities { priorities, .. } = command {
        ensure(
            members.len() == priorities.len()
                && members
                    .iter()
                    .zip(priorities)
                    .all(|(m, (id, _))| m.membership_id == *id),
            ControlErrorCode::InvalidInput,
        )?;
    }
    Ok(members)
}
pub(super) fn operation_projection(
    conn: &Connection,
    id: &OperationId,
) -> ControlResult<OperationView> {
    let row=conn.query_row("SELECT kind,target_type,target_id,phase,target_revision,error_code,updated_at,expected_targets FROM control_operations WHERE operation_id=?1",[id.as_str()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,i64>(6)?,r.get::<_,i64>(7)?))).optional()?.ok_or_else(||failure(ControlErrorCode::NotFound))?;
    let (total, confirmed): (i64, i64) = conn.query_row(
        "SELECT count(*),count(confirmed_at) FROM operation_targets WHERE operation_id=?1",
        [id.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let phase = OperationPhase::parse(&row.3)?;
    ensure(
        total == row.7
            && total >= 0
            && total <= MAX_MEMBERSHIPS as i64
            && confirmed <= total
            && (phase != OperationPhase::Succeeded || confirmed == total),
        ControlErrorCode::Storage,
    )?;
    Ok(OperationView {
        schema_version: OPERATION_SCHEMA_VERSION,
        operation_id: id.clone(),
        kind: OperationKind::parse(&row.0)?,
        target_type: TargetType::parse(&row.1)?,
        target_id: row.2.to_string(),
        phase,
        target_revision: row.4,
        total_targets: row.7 as usize,
        confirmed_targets: confirmed as usize,
        error_code: row.5.map(|s| ControlErrorCode::parse(&s)).transpose()?,
        updated_at: row.6,
    })
}

pub(super) fn verify_control(conn: &Connection) -> super::types::Result<()> {
    let broken:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sources s LEFT JOIN source_control c USING(source_id) WHERE c.source_id IS NULL) OR EXISTS(SELECT 1 FROM memberships m LEFT JOIN member_control c USING(membership_id) WHERE c.membership_id IS NULL) OR EXISTS(SELECT 1 FROM member_control c JOIN memberships m USING(membership_id) WHERE c.create_name != 'qt-'||(SELECT catalog_instance_id FROM catalog_meta)||'-m'||m.membership_id OR (c.create_state='resolved') != (m.channel_id IS NOT NULL))",[],|r|r.get(0))?;
    require(!broken, ErrorKind::CorruptData)?;
    let bad_targets:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM operation_targets t JOIN memberships m USING(membership_id) JOIN member_control c USING(membership_id) JOIN control_operations o USING(operation_id) WHERE t.target_member_revision>c.revision OR (o.target_type='source' AND o.target_id!=m.source_id) OR (o.target_type='group' AND o.target_id!=m.group_id) OR (o.target_type='membership' AND o.target_id!=m.membership_id))",[],|r|r.get(0))?;
    require(!bad_targets, ErrorKind::CorruptData)?;
    let missing_identity:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM control_operations o WHERE (target_type='source' AND NOT EXISTS(SELECT 1 FROM sources WHERE source_id=o.target_id)) OR (target_type='group' AND NOT EXISTS(SELECT 1 FROM source_groups WHERE group_id=o.target_id)) OR (target_type='membership' AND NOT EXISTS(SELECT 1 FROM memberships WHERE membership_id=o.target_id)))",[],|r|r.get(0))?;
    require(!missing_identity, ErrorKind::CorruptData)?;
    let missing_targets:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM control_operations o WHERE o.expected_targets!=(SELECT count(*) FROM operation_targets t WHERE t.operation_id=o.operation_id))",[],|r|r.get(0))?;
    require(!missing_targets, ErrorKind::CorruptData)?;
    let unconfirmed_success:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM control_operations o JOIN operation_targets t USING(operation_id) WHERE o.phase='succeeded' AND t.confirmed_at IS NULL)",[],|r|r.get(0))?;
    require(!unconfirmed_success, ErrorKind::CorruptData)
}
