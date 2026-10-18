//! 同库令牌身份与持久意图；不执行HTTP，不缓存原生key或路由可用状态。
#![cfg_attr(not(test), allow(dead_code))]
use super::control::{
    bump, ensure, failure, operation_projection, replayed_operation, ControlErrorCode as Code,
    ControlResult, CreateState, OperationId, OperationKind, OperationPhase, OperationView,
};
use super::store::{commit, CatalogStore};
use super::types::*;
use crate::newapi::tokens::{
    CreationLookup, NativeTokenView, TokenCreateSpec, TokenOwner, TokenOwnerScope, TokenStatus,
    NATIVE_TOKEN_SCHEMA_VERSION,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fmt;

mod execution;
mod verify;
pub(super) use execution::retry_tx;
pub(super) use verify::verify_tokens;

pub const TOKEN_IDENTITY_SCHEMA_VERSION: i64 = 1;
pub const MAX_MANAGED_TOKENS: usize = 10_000;
const RECORD_COLUMNS: &str = "token_id,owner_user_id,owner_scope,native_token_id,group_id,explicit_group,display_name,revision,evidence_revision,create_operation_id,create_group_revision,expires_at,created_at,deleted_at";
const PENDING: &str = "a.state IN ('prepared','uncertain') AND NOT (a.action='create' AND t.native_token_id IS NOT NULL) AND NOT (a.action='delete' AND t.deleted_at IS NOT NULL)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenOrigin {
    Created,
    Imported,
}
impl TokenOrigin {
    pub const VALUES: &'static [&'static str] = &["created", "imported"];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenIdentityView {
    pub schema_version: i64,
    pub token_id: ManagedTokenId,
    pub native_token_id: Option<String>,
    pub owner_user_id: String,
    pub owner_scope: TokenOwnerScope,
    pub origin: TokenOrigin,
    pub group_id: Option<GroupId>,
    pub explicit_group: String,
    pub display_name: String,
    pub revision: i64,
    pub create_state: CreateState,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Clone)]
pub enum TokenCommand {
    CreateToken {
        owner: TokenOwner,
        group_id: GroupId,
        expected_group_revision: i64,
        display_name: String,
        expires_at: Option<i64>,
    },
    SetTokenEnabled {
        token_id: ManagedTokenId,
        expected_revision: i64,
        enabled: bool,
    },
    DeleteToken {
        token_id: ManagedTokenId,
        expected_revision: i64,
    },
}
impl fmt::Debug for TokenCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenCommand([PRIVATE])")
    }
}
impl TokenCommand {
    fn canonical(&self) -> ControlResult<(OperationKind, String)> {
        // Step T5B.1: 易变的expiry/ready不参与结构准入，确保历史重放仍可恢复。
        let (kind, payload) = match self {
            Self::CreateToken {
                owner,
                group_id,
                expected_group_revision,
                display_name,
                expires_at,
            } => {
                ensure(
                    owner.user_id > 0
                        && positive_revision(*expected_group_revision)
                        && label(display_name)
                        && valid_timestamp(*expires_at),
                    Code::InvalidInput,
                )?;
                (
                    OperationKind::CreateToken,
                    json!([
                        owner.user_id,
                        owner.scope,
                        group_id,
                        expected_group_revision,
                        display_name,
                        expires_at
                    ]),
                )
            }
            Self::SetTokenEnabled {
                token_id,
                expected_revision,
                enabled,
            } => {
                ensure(positive_revision(*expected_revision), Code::InvalidInput)?;
                (
                    OperationKind::SetTokenEnabled,
                    json!([token_id, expected_revision, enabled]),
                )
            }
            Self::DeleteToken {
                token_id,
                expected_revision,
            } => {
                ensure(positive_revision(*expected_revision), Code::InvalidInput)?;
                (
                    OperationKind::DeleteToken,
                    json!([token_id, expected_revision]),
                )
            }
        };
        let bytes = serde_json::to_vec(&json!(["qt-token-command-v1", kind, payload]))
            .map_err(|_| failure(Code::InvalidInput))?;
        Ok((kind, format!("{:x}", Sha256::digest(bytes))))
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Record {
    id: ManagedTokenId,
    owner: TokenOwner,
    native_id: Option<i64>,
    group: Option<GroupId>,
    explicit_group: String,
    display_name: String,
    revision: i64,
    evidence: i64,
    creation: Option<OperationId>,
    group_revision: Option<i64>,
    expires_at: Option<i64>,
    created_at: i64,
    deleted_at: Option<i64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Create,
    Enable,
    Disable,
    Delete,
}

#[derive(Clone)]
pub struct TokenWork {
    instance: String,
    record: Record,
    operation: OperationView,
    action: Action,
    create_state: CreateState,
    loaded_at: i64,
}
impl fmt::Debug for TokenWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenWork([PRIVATE])")
    }
}
impl TokenWork {
    pub fn token_id(&self) -> ManagedTokenId {
        self.record.id
    }
    pub fn owner(&self) -> TokenOwner {
        self.record.owner
    }
    pub fn native_token_id(&self) -> Option<i64> {
        self.record.native_id
    }
    pub fn phase(&self) -> OperationPhase {
        self.operation.phase
    }
    pub fn create_state(&self) -> CreateState {
        self.create_state
    }
    pub fn create_spec(&self) -> ControlResult<TokenCreateSpec> {
        creation_spec(&self.instance, &self.record)
    }
}

/// 仅可信A读取的结果；没有Deserialize或前端布尔确认入口。
pub enum TokenObservation {
    Creation(CreationLookup),
    Existing(Option<NativeTokenView>),
}

#[derive(Clone, PartialEq, Eq)]
pub struct TokenTicket {
    instance: String,
    operation: OperationId,
    record: Record,
    sequence: i64,
    action: Action,
}
impl fmt::Debug for TokenTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenTicket([PRIVATE])")
    }
}
impl TokenTicket {
    pub fn owner(&self) -> TokenOwner {
        self.record.owner
    }
    pub fn native_token_id(&self) -> Option<i64> {
        self.record.native_id
    }
    pub fn create_spec(&self) -> ControlResult<TokenCreateSpec> {
        creation_spec(&self.instance, &self.record)
    }
    pub fn enabled(&self) -> Option<bool> {
        match self.action {
            Action::Enable => Some(true),
            Action::Disable => Some(false),
            _ => None,
        }
    }
    pub fn is_delete(&self) -> bool {
        self.action == Action::Delete
    }
}

fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn clock(now: i64) -> ControlResult<()> {
    ensure(valid_timestamp(Some(now)), Code::InvalidInput)
}
fn encode<T: Serialize>(value: T) -> ControlResult<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| failure(Code::Storage))
}
fn decode<T: DeserializeOwned>(value: String) -> ControlResult<T> {
    serde_json::from_value(serde_json::Value::String(value)).map_err(|_| failure(Code::Storage))
}
fn native_id(value: &str) -> ControlResult<i64> {
    let id = value
        .parse::<i64>()
        .map_err(|_| failure(Code::InvalidResponse))?;
    ensure(id > 0 && id.to_string() == value, Code::InvalidResponse)?;
    Ok(id)
}
fn instance(conn: &Connection) -> ControlResult<String> {
    Ok(conn.query_row(
        "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?)
}
fn create_name(instance: &str, id: &OperationId) -> ControlResult<String> {
    ensure(
        instance.len() == 32
            && instance
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        Code::Storage,
    )?;
    let input = serde_json::to_vec(&json!(["qt-token-create-v1", instance, id]))
        .map_err(|_| failure(Code::Storage))?;
    Ok(format!(
        "qt-t-{}",
        &format!("{:x}", Sha256::digest(input))[..32]
    ))
}
fn creation_spec(instance: &str, row: &Record) -> ControlResult<TokenCreateSpec> {
    Ok(TokenCreateSpec {
        correlation_name: create_name(
            instance,
            row.creation
                .as_ref()
                .ok_or_else(|| failure(Code::InvalidInput))?,
        )?,
        group: row.explicit_group.clone(),
        expires_at: row.expires_at,
    })
}
fn record(conn: &Connection, id: ManagedTokenId) -> ControlResult<Record> {
    let row = conn
        .query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM managed_tokens WHERE token_id=?1"),
            [id.get()],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<i64>>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, i64>(12)?,
                    r.get::<_, Option<i64>>(13)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| failure(Code::NotFound))?;
    Ok(Record {
        id: ManagedTokenId::new(row.0)?,
        owner: TokenOwner {
            user_id: row.1,
            scope: decode(row.2)?,
        },
        native_id: row.3,
        group: row.4.map(GroupId::new).transpose()?,
        explicit_group: row.5,
        display_name: row.6,
        revision: row.7,
        evidence: row.8,
        creation: row.9.map(OperationId::new).transpose()?,
        group_revision: row.10,
        expires_at: row.11,
        created_at: row.12,
        deleted_at: row.13,
    })
}
fn create_state(conn: &Connection, row: &Record) -> ControlResult<CreateState> {
    if row.native_id.is_some() {
        return Ok(CreateState::Resolved);
    }
    let sent: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM token_execution_attempts WHERE token_id=?1 AND action='create' AND state!='not_sent')", [row.id.get()], |r| r.get(0))?;
    Ok(if sent {
        CreateState::MayHaveBeenSent
    } else {
        CreateState::Never
    })
}
fn projection(conn: &Connection, row: &Record) -> ControlResult<TokenIdentityView> {
    Ok(TokenIdentityView {
        schema_version: TOKEN_IDENTITY_SCHEMA_VERSION,
        token_id: row.id,
        native_token_id: row.native_id.map(|v| v.to_string()),
        owner_user_id: row.owner.user_id.to_string(),
        owner_scope: row.owner.scope,
        origin: if row.creation.is_some() {
            TokenOrigin::Created
        } else {
            TokenOrigin::Imported
        },
        group_id: row.group,
        explicit_group: row.explicit_group.clone(),
        display_name: row.display_name.clone(),
        revision: row.revision,
        create_state: create_state(conn, row)?,
        created_at: row.created_at,
        deleted_at: row.deleted_at,
    })
}
fn owner_consistent(conn: &Connection, owner: TokenOwner) -> ControlResult<()> {
    ensure(owner.user_id > 0, Code::InvalidInput)?;
    let different: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM managed_tokens WHERE owner_user_id=?1 AND owner_scope!=?2)",
        params![owner.user_id, encode(owner.scope)?],
        |r| r.get(0),
    )?;
    ensure(!different, Code::BindingDrift)
}
fn capacity(conn: &Connection) -> ControlResult<()> {
    let n: i64 = conn.query_row("SELECT count(*) FROM managed_tokens", [], |r| r.get(0))?;
    ensure(n < MAX_MANAGED_TOKENS as i64, Code::QueueFull)
}
fn pending(conn: &Connection, id: ManagedTokenId) -> ControlResult<(bool, bool)> {
    Ok(conn.query_row(&format!("SELECT COALESCE(MAX(a.state='prepared'),0),COALESCE(MAX(a.state='uncertain'),0) FROM token_execution_attempts a JOIN managed_tokens t USING(token_id) WHERE t.token_id=?1 AND ({PENDING})"), [id.get()], |r| Ok((r.get(0)?,r.get(1)?)))?)
}
fn active_other(
    conn: &Connection,
    id: ManagedTokenId,
    except: Option<&OperationId>,
) -> ControlResult<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM token_operation_targets t JOIN control_operations o USING(operation_id) WHERE t.token_id=?1 AND (?2 IS NULL OR o.operation_id!=?2) AND o.phase NOT IN ('succeeded','failed'))", params![id.get(), except.map(OperationId::as_str)], |r| r.get(0))?)
}
fn action(kind: OperationKind, enabled: Option<bool>) -> ControlResult<Action> {
    match (kind, enabled) {
        (OperationKind::CreateToken, None) => Ok(Action::Create),
        (OperationKind::DeleteToken, None) => Ok(Action::Delete),
        (OperationKind::SetTokenEnabled, Some(v)) => {
            Ok(if v { Action::Enable } else { Action::Disable })
        }
        _ => Err(failure(Code::Storage)),
    }
}
fn load_work(conn: &Connection, id: &OperationId, now: i64) -> ControlResult<TokenWork> {
    let operation = operation_projection(conn, id)?;
    ensure(
        operation.target_type == super::control::TargetType::Token,
        Code::InvalidInput,
    )?;
    let (token, enabled): (i64, Option<bool>) = conn.query_row(
        "SELECT token_id,desired_enabled FROM token_operation_targets WHERE operation_id=?1",
        [id.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let record = record(conn, ManagedTokenId::new(token)?)?;
    Ok(TokenWork {
        instance: instance(conn)?,
        action: action(operation.kind, enabled)?,
        create_state: create_state(conn, &record)?,
        record,
        operation,
        loaded_at: now,
    })
}
fn current_work(conn: &Connection, old: &TokenWork, now: i64) -> ControlResult<TokenWork> {
    clock(now)?;
    ensure(
        now.checked_sub(old.loaded_at)
            .is_some_and(|age| (0..=60).contains(&age)),
        Code::NotReady,
    )?;
    let new = load_work(conn, &old.operation.operation_id, now)?;
    ensure(
        new.instance == old.instance
            && new.record == old.record
            && new.action == old.action
            && new.operation == old.operation
            && new.record.revision == new.operation.target_revision,
        Code::RevisionConflict,
    )?;
    Ok(new)
}
fn bump_evidence(conn: &Connection, row: &Record) -> ControlResult<i64> {
    let next = bump(row.evidence)?;
    conn.execute(
        "UPDATE managed_tokens SET evidence_revision=?1 WHERE token_id=?2",
        params![next, row.id.get()],
    )?;
    Ok(next)
}

impl CatalogStore {
    pub(crate) fn token_identity(&self, id: ManagedTokenId) -> ControlResult<TokenIdentityView> {
        self.usable()?;
        projection(&self.conn, &record(&self.conn, id)?)
    }
    pub(crate) fn list_token_identities(
        &mut self,
        after: Option<ManagedTokenId>,
    ) -> ControlResult<Vec<TokenIdentityView>> {
        self.usable()?;
        let tx = self.conn.transaction()?;
        let mut q = tx.prepare(
            "SELECT token_id FROM managed_tokens WHERE token_id>?1 ORDER BY token_id LIMIT 100",
        )?;
        let ids = q.query_map([after.map_or(0, ManagedTokenId::get)], |r| {
            r.get::<_, i64>(0)
        })?;
        ids.map(|id| {
            let row = record(&tx, ManagedTokenId::new(id?)?)?;
            projection(&tx, &row)
        })
        .collect()
    }
    pub(crate) fn register_token(
        &mut self,
        owner: TokenOwner,
        view: &NativeTokenView,
        display_name: String,
        now: i64,
    ) -> ControlResult<TokenIdentityView> {
        // Step T5B.3: 显式原生身份登记，不借列表缺项推删，不重写原生设置。
        self.usable()?;
        clock(now)?;
        ensure(
            owner.user_id > 0 && label(&display_name) && view.explicit_group.len() <= 256,
            Code::InvalidInput,
        )?;
        ensure(
            view.schema_version == NATIVE_TOKEN_SCHEMA_VERSION
                && native_id(&view.owner_user_id)? == owner.user_id
                && view.owner_scope == owner.scope,
            Code::BindingDrift,
        )?;
        let native = native_id(&view.token_id)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT token_id FROM managed_tokens WHERE native_token_id=?1",
                [native],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            let row = record(&tx, ManagedTokenId::new(id)?)?;
            ensure(row.owner == owner, Code::BindingDrift)?;
            return projection(&tx, &row);
        }
        owner_consistent(&tx, owner)?;
        capacity(&tx)?;
        if view.name.starts_with("qt-t-") {
            let inst = instance(&tx)?;
            let mut q = tx.prepare("SELECT create_operation_id FROM managed_tokens WHERE create_operation_id IS NOT NULL AND native_token_id IS NULL")?;
            for id in q.query_map([], |r| r.get::<_, String>(0))? {
                ensure(
                    create_name(&inst, &OperationId::new(id?)?)? != view.name,
                    Code::NotReady,
                )?;
            }
        }
        let group: Option<i64> = tx.query_row("SELECT group_id FROM source_groups WHERE newapi_group=?1 AND ?1!='' AND ?1!='auto'", [&view.explicit_group], |r| r.get(0)).optional()?;
        tx.execute("INSERT INTO managed_tokens(owner_user_id,owner_scope,native_token_id,group_id,explicit_group,display_name,revision,evidence_revision,created_at) VALUES(?1,?2,?3,?4,?5,?6,1,0,?7)",params![owner.user_id,encode(owner.scope)?,native,group,view.explicit_group,display_name,now])?;
        let result = projection(
            &tx,
            &record(&tx, ManagedTokenId::new(tx.last_insert_rowid())?)?,
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
    pub(crate) fn submit_token_operation(
        &mut self,
        id: OperationId,
        command: TokenCommand,
        now: i64,
    ) -> ControlResult<OperationView> {
        self.usable()?;
        clock(now)?;
        let (kind, digest) = command.canonical()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Step T5B.4: 统一幂等命名空间，重放先于现时资格；三个记录同事务。
        if let Some(view) = replayed_operation(&tx, &id, &digest)? {
            return Ok(view);
        }
        let (token, revision, enabled) = match &command {
            TokenCommand::CreateToken {
                owner,
                group_id,
                expected_group_revision,
                display_name,
                expires_at,
            } => {
                ensure(
                    owner.scope == TokenOwnerScope::Issuer && expires_at.is_none_or(|v| v > now),
                    Code::InvalidInput,
                )?;
                owner_consistent(&tx, *owner)?;
                capacity(&tx)?;
                let (group, rev, ready): (String, i64, bool) = tx
                    .query_row(
                        "SELECT newapi_group,revision,ready FROM source_groups WHERE group_id=?1",
                        [group_id.get()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?
                    .ok_or_else(|| failure(Code::NotFound))?;
                ensure(rev == *expected_group_revision, Code::RevisionConflict)?;
                ensure(ready && valid_group(&group), Code::NotReady)?;
                tx.execute("INSERT INTO managed_tokens(owner_user_id,owner_scope,group_id,explicit_group,display_name,revision,evidence_revision,create_operation_id,create_group_revision,expires_at,created_at) VALUES(?1,?2,?3,?4,?5,1,0,?6,?7,?8,?9)",params![owner.user_id,encode(owner.scope)?,group_id.get(),group,display_name,id.as_str(),expected_group_revision,expires_at,now])?;
                (ManagedTokenId::new(tx.last_insert_rowid())?, 1, None)
            }
            TokenCommand::SetTokenEnabled {
                token_id,
                expected_revision,
                ..
            }
            | TokenCommand::DeleteToken {
                token_id,
                expected_revision,
            } => {
                let row = record(&tx, *token_id)?;
                ensure(row.revision == *expected_revision, Code::RevisionConflict)?;
                ensure(
                    row.deleted_at.is_none() && row.native_id.is_some(),
                    Code::NotReady,
                )?;
                ensure(
                    !active_other(&tx, *token_id, None)?
                        && pending(&tx, *token_id)? == (false, false),
                    Code::NotReady,
                )?;
                let rev = bump(row.revision)?;
                tx.execute(
                    "UPDATE managed_tokens SET revision=?1,evidence_revision=?2 WHERE token_id=?3",
                    params![rev, bump(row.evidence)?, token_id.get()],
                )?;
                (
                    *token_id,
                    rev,
                    match &command {
                        TokenCommand::SetTokenEnabled { enabled, .. } => Some(*enabled),
                        _ => None,
                    },
                )
            }
        };
        tx.execute("INSERT INTO control_operations(operation_id,kind,target_type,target_id,payload_digest,target_revision,expected_targets,phase,created_at,updated_at) VALUES(?1,?2,'token',?3,?4,?5,1,'queued',?6,?6)",params![id.as_str(),kind.as_str(),token.get(),digest,revision,now])?;
        tx.execute("INSERT INTO token_operation_targets(operation_id,token_id,token_revision,desired_enabled) VALUES(?1,?2,?3,?4)",params![id.as_str(),token.get(),revision,enabled])?;
        let result = operation_projection(&tx, &id)?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
    pub(crate) fn token_work(&mut self, id: &OperationId, now: i64) -> ControlResult<TokenWork> {
        self.usable()?;
        clock(now)?;
        let tx = self.conn.transaction()?;
        load_work(&tx, id, now)
    }
    pub(crate) fn pending_token_operations(&self, now: i64) -> ControlResult<Vec<OperationId>> {
        self.usable()?;
        clock(now)?;
        let mut q=self.conn.prepare("SELECT operation_id FROM control_operations INDEXED BY token_pending WHERE target_type = 'token' AND phase IN ('queued','waiting','unknown') AND (phase='queued' OR updated_at<=?1) ORDER BY updated_at,operation_id LIMIT 64")?;
        let rows = q.query_map([now.checked_sub(30).unwrap_or(-1)], |r| {
            r.get::<_, String>(0)
        })?;
        rows.map(|v| OperationId::new(v?)).collect()
    }
    pub(crate) fn token_reveal_target(
        &self,
        id: ManagedTokenId,
        expected_revision: i64,
    ) -> ControlResult<(TokenOwner, i64)> {
        self.usable()?;
        let row = record(&self.conn, id)?;
        ensure(row.revision == expected_revision, Code::RevisionConflict)?;
        ensure(
            row.deleted_at.is_none()
                && !active_other(&self.conn, id, None)?
                && pending(&self.conn, id)? == (false, false),
            Code::NotReady,
        )?;
        if let Some(op) = &row.creation {
            ensure(
                operation_projection(&self.conn, op)?.phase == OperationPhase::Succeeded,
                Code::NotReady,
            )?;
        }
        Ok((
            row.owner,
            row.native_id.ok_or_else(|| failure(Code::NotReady))?,
        ))
    }
}
