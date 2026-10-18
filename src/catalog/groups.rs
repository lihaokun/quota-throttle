//! 组配置就绪与已知本地引用；不提供删除/换类型许可。
#![cfg_attr(not(test), allow(dead_code))]
use super::control::{bump, ensure, failure, ControlErrorCode as Code, ControlResult};
use super::store::{commit, decode_group_view, GROUP_COLUMNS};
use super::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

pub const GROUP_REFERENCES_SCHEMA_VERSION: i64 = 1;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupReferences {
    pub schema_version: i64,
    pub group_id: GroupId,
    pub active_memberships: u32,
    pub historical_memberships: u32,
    pub live_tokens: u32,
    pub pending_creations: u32,
    pub historical_tokens: u32,
    pub unknown_group_tokens: u32,
    pub known_local_only: bool,
    pub deletion_authorized: bool,
}
fn read_group(conn: &Connection, id: GroupId) -> ControlResult<GroupView> {
    let mut q = conn.prepare(&format!(
        "SELECT {GROUP_COLUMNS} FROM source_groups WHERE group_id=?1"
    ))?;
    let mut rows = q.query([id.get()])?;
    Ok(decode_group_view(
        rows.next()?.ok_or_else(|| failure(Code::NotFound))?,
    )?)
}
impl CatalogStore {
    pub(crate) fn group_for_name(&self, name: &str) -> ControlResult<Option<GroupView>> {
        self.usable()?;
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT group_id FROM source_groups WHERE newapi_group=?1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        id.map(|id| self.group(GroupId::new(id)?)).transpose()
    }
    pub(crate) fn group(&self, id: GroupId) -> ControlResult<GroupView> {
        self.usable()?;
        read_group(&self.conn, id)
    }
    pub(crate) fn set_group_ready(
        &mut self,
        id: GroupId,
        expected: i64,
        ready: bool,
        now: i64,
    ) -> ControlResult<GroupView> {
        // Step T5C.2: 只保存可信配置检查，引用计数不产生删除或正向路由许可。
        self.usable()?;
        ensure(
            valid_timestamp(Some(now)) && (1..=JSON_SAFE).contains(&expected),
            Code::InvalidInput,
        )?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut group = read_group(&tx, id)?;
        ensure(group.revision == expected, Code::RevisionConflict)?;
        if group.ready == ready {
            return Ok(group);
        }
        group.revision = bump(group.revision)?;
        group.ready = ready;
        tx.execute(
            "UPDATE source_groups SET ready=?1,revision=?2 WHERE group_id=?3",
            params![ready, group.revision, id.get()],
        )?;
        let mut eligible = Vec::new();
        {
            let mut q = tx.prepare("SELECT c.membership_id,c.last_error_code FROM member_control c JOIN memberships m USING(membership_id) WHERE m.group_id=?1 AND m.lifecycle!='retired' AND c.dirty=1 AND EXISTS(SELECT 1 FROM operation_targets t WHERE t.membership_id=m.membership_id)")?;
            let mut rows = q.query([id.get()])?;
            while let Some(row) = rows.next()? {
                let code: Option<String> = row.get(1)?;
                if code
                    .map(|v| Code::parse(&v))
                    .transpose()?
                    .is_none_or(|c| !super::confirmation::terminal(c))
                {
                    eligible.push(row.get::<_, i64>(0)?);
                }
            }
        }
        for member in eligible {
            tx.execute(
                "UPDATE member_control SET next_attempt_at=?1 WHERE membership_id=?2",
                params![now, member],
            )?;
        }
        commit(tx, &mut self.poisoned)?;
        Ok(group)
    }
    pub(crate) fn group_references(&mut self, id: GroupId) -> ControlResult<GroupReferences> {
        self.usable()?;
        let tx = self.conn.transaction()?;
        let group = read_group(&tx, id)?;
        let (active_memberships, historical_memberships) = tx.query_row("SELECT count(CASE WHEN lifecycle!='retired' THEN 1 END),count(CASE WHEN lifecycle='retired' THEN 1 END) FROM memberships WHERE group_id=?1",[id.get()],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let (live_tokens, pending_creations, historical_tokens) = tx.query_row("SELECT count(CASE WHEN deleted_at IS NULL AND native_token_id IS NOT NULL THEN 1 END),count(CASE WHEN deleted_at IS NULL AND native_token_id IS NULL THEN 1 END),count(CASE WHEN deleted_at IS NOT NULL THEN 1 END) FROM managed_tokens WHERE group_id=?1 OR explicit_group=?2",params![id.get(),group.newapi_group],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let unknown_group_tokens = tx.query_row("SELECT count(*) FROM managed_tokens WHERE deleted_at IS NULL AND explicit_group IN ('','auto')",[],|r|r.get(0))?;
        Ok(GroupReferences {
            schema_version: GROUP_REFERENCES_SCHEMA_VERSION,
            group_id: id,
            active_memberships,
            historical_memberships,
            live_tokens,
            pending_creations,
            historical_tokens,
            unknown_group_tokens,
            known_local_only: true,
            deletion_authorized: false,
        })
    }
}
