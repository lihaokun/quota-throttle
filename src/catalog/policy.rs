//! 来源/组策略的本地持久值。无渠道意图、HTTP 或运行期接线。
// 写入口在 C 批接到单写者；当前只由本模块测试消费，保持 crate 内可见。
#![cfg_attr(not(test), allow(dead_code))]
use super::control::{bump, ensure, failure, ControlErrorCode as Code, ControlResult};
use super::store::{commit, decode_group_view, decode_source_view, GROUP_COLUMNS, SOURCE_COLUMNS};
use super::{types::*, CatalogStore};
use crate::orchestrator::Regime;
use crate::source_policy::glm::GlmGroupState;
use crate::source_policy::volcano::{
    self, EvidenceDisposition, PeriodTransition, VolcanoEvidence, VolcanoGroupState, VolcanoRuntime,
};
use crate::source_policy::PolicyError;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupPolicyState {
    Glm(GlmGroupState),
    Volcano(VolcanoGroupState),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredGroupPolicy {
    pub group_id: GroupId,
    pub policy_revision: i64,
    pub state: GroupPolicyState,
}
#[derive(Debug, Clone)]
pub(crate) enum SourcePolicyUpdate {
    Tick,
    Evidence(VolcanoEvidence),
    ClearError,
    ReplacePolicy(VolcanoPolicy),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourcePolicyWrite {
    pub source: SourceView,
    pub runtime: VolcanoRuntime,
    pub disposition: Option<EvidenceDisposition>,
    pub transition: Option<PeriodTransition>,
}

impl CatalogStore {
    pub(crate) fn load_source_policy(&self, id: SourceId) -> ControlResult<Option<VolcanoRuntime>> {
        self.usable()?;
        let (_, settings, _) = source_record(&self.conn, id)?;
        read_runtime(&self.conn, id, volcano_policy(&settings)?)
    }

    pub(crate) fn load_group_policy(&self, id: GroupId) -> ControlResult<StoredGroupPolicy> {
        self.usable()?;
        let group = group_record(&self.conn, id)?;
        read_group_policy(&self.conn, &group)
    }

    pub(crate) fn initialize_source_policy(
        &mut self,
        id: SourceId,
        expected_source_revision: i64,
        now: i64,
    ) -> ControlResult<VolcanoRuntime> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = initialize_source_policy_tx(&tx, id, expected_source_revision, now)?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }

    pub(crate) fn apply_source_policy(
        &mut self,
        id: SourceId,
        expected_source_revision: i64,
        expected_runtime_revision: i64,
        update: SourcePolicyUpdate,
        now: i64,
    ) -> ControlResult<SourcePolicyWrite> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = apply_source_policy_tx(
            &tx,
            id,
            expected_source_revision,
            expected_runtime_revision,
            update,
            now,
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }

    pub(crate) fn save_group_policy(
        &mut self,
        id: GroupId,
        expected_group_revision: i64,
        expected_policy_revision: i64,
        state: GroupPolicyState,
    ) -> ControlResult<StoredGroupPolicy> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = save_group_policy_tx(
            &tx,
            id,
            expected_group_revision,
            expected_policy_revision,
            state,
        )?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
}

fn policy_error(e: PolicyError) -> super::control::ControlError {
    failure(match e {
        PolicyError::InvalidInput | PolicyError::Capacity => Code::InvalidInput,
        PolicyError::SequenceExhausted => Code::Storage,
    })
}
fn volcano_policy(settings: &SourceSettings) -> ControlResult<&VolcanoPolicy> {
    match settings {
        SourceSettings::Volcano { policy, .. } => Ok(policy),
        _ => Err(failure(Code::InvalidInput)),
    }
}
fn source_record(
    conn: &Connection,
    id: SourceId,
) -> ControlResult<(SourceView, SourceSettings, bool)> {
    let (source, json, retiring) = conn.query_row(
        &format!("SELECT {SOURCE_COLUMNS},settings_json,(SELECT retire_requested FROM source_control c WHERE c.source_id=sources.source_id) FROM sources WHERE source_id=?1"),
        [id.get()], |r| Ok((decode_source_view(r),r.get::<_,String>(7)?,r.get::<_,i64>(8)?)),
    ).optional()?.ok_or_else(|| failure(Code::NotFound))?;
    let source = source?;
    let settings = decode_settings(&json)?;
    ensure(
        source.kind == settings.kind() && matches!(retiring, 0 | 1),
        Code::Storage,
    )?;
    Ok((source, settings, retiring == 1))
}
pub(super) fn group_record(conn: &Connection, id: GroupId) -> ControlResult<GroupView> {
    Ok(conn
        .query_row(
            &format!("SELECT {GROUP_COLUMNS} FROM source_groups WHERE group_id=?1"),
            [id.get()],
            |r| Ok(decode_group_view(r)),
        )
        .optional()?
        .ok_or_else(|| failure(Code::NotFound))??)
}
fn get<T: rusqlite::types::FromSql>(row: &Row<'_>, name: &str) -> ControlResult<T> {
    Ok(row.get(name)?)
}
fn decode_runtime(row: &Row<'_>, id: SourceId) -> ControlResult<Option<VolcanoRuntime>> {
    let kind: String = get(row, "kind")?;
    let revision: i64 = get(row, "revision")?;
    let period_id: Option<i64> = get(row, "period_id")?;
    let period_start: Option<i64> = get(row, "period_start")?;
    let next_reset_at: Option<i64> = get(row, "next_reset_at")?;
    let used_input: i64 = get(row, "used_input")?;
    let used_output: i64 = get(row, "used_output")?;
    let consecutive_failures: i64 = get(row, "consecutive_failures")?;
    let error_exhausted: i64 = get(row, "error_exhausted")?;
    let limit_exhausted: i64 = get(row, "limit_exhausted")?;
    let error_epoch: i64 = get(row, "error_epoch")?;
    let error_cutoff_at: Option<i64> = get(row, "error_cutoff_at")?;
    let evidence_cursor: Option<i64> = get(row, "evidence_cursor")?;
    let coverage_end: Option<i64> = get(row, "coverage_end")?;
    ensure(
        kind == "volcano" && matches!(error_exhausted, 0 | 1) && matches!(limit_exhausted, 0 | 1),
        Code::Storage,
    )?;
    if revision == 0 {
        ensure(
            period_id.is_none()
                && period_start.is_none()
                && next_reset_at.is_none()
                && used_input == 0
                && used_output == 0
                && consecutive_failures == 0
                && error_exhausted == 0
                && limit_exhausted == 0
                && error_epoch == 0
                && error_cutoff_at.is_none()
                && evidence_cursor.is_none()
                && coverage_end.is_none(),
            Code::Storage,
        )?;
        return Ok(None);
    }
    Ok(Some(VolcanoRuntime {
        source_id: id,
        revision,
        period_id: period_id.ok_or_else(|| failure(Code::Storage))?,
        period_start: period_start.ok_or_else(|| failure(Code::Storage))?,
        next_reset_at: next_reset_at.ok_or_else(|| failure(Code::Storage))?,
        used_input,
        used_output,
        consecutive_failures,
        error_exhausted: error_exhausted == 1,
        limit_exhausted: limit_exhausted == 1,
        error_epoch,
        error_cutoff_at,
        evidence_cursor,
        coverage_end,
    }))
}
fn read_runtime(
    conn: &Connection,
    id: SourceId,
    policy: &VolcanoPolicy,
) -> ControlResult<Option<VolcanoRuntime>> {
    let runtime = conn
        .query_row(
            "SELECT * FROM volcano_runtime WHERE source_id=?1",
            [id.get()],
            |r| Ok(decode_runtime(r, id)),
        )
        .optional()?
        .ok_or_else(|| failure(Code::Storage))??;
    if let Some(runtime) = &runtime {
        runtime
            .validate(policy)
            .map_err(|_| failure(Code::Storage))?;
    }
    Ok(runtime)
}
fn write_runtime(tx: &Transaction<'_>, r: &VolcanoRuntime) -> ControlResult<()> {
    let rows = tx.execute("UPDATE volcano_runtime SET revision=?2,period_id=?3,period_start=?4,next_reset_at=?5,used_input=?6,used_output=?7,consecutive_failures=?8,error_exhausted=?9,limit_exhausted=?10,error_epoch=?11,error_cutoff_at=?12,evidence_cursor=?13,coverage_end=?14 WHERE source_id=?1",
        params![r.source_id.get(),r.revision,r.period_id,r.period_start,r.next_reset_at,r.used_input,r.used_output,r.consecutive_failures,r.error_exhausted,r.limit_exhausted,r.error_epoch,r.error_cutoff_at,r.evidence_cursor,r.coverage_end])?;
    ensure(rows == 1, Code::Storage)
}

pub(crate) fn initialize_source_policy_tx(
    tx: &Transaction<'_>,
    id: SourceId,
    expected_source_revision: i64,
    now: i64,
) -> ControlResult<VolcanoRuntime> {
    ensure(
        positive_revision(expected_source_revision) && (0..=JSON_SAFE).contains(&now),
        Code::InvalidInput,
    )?;
    let (source, settings, retiring) = source_record(tx, id)?;
    let policy = volcano_policy(&settings)?;
    ensure(!retiring && source.retired_at.is_none(), Code::NotReady)?;
    ensure(
        source.config_revision == expected_source_revision,
        Code::RevisionConflict,
    )?;
    if let Some(runtime) = read_runtime(tx, id, policy)? {
        return Ok(runtime);
    }
    let runtime = volcano::initialize_runtime(id, policy, now).map_err(policy_error)?;
    write_runtime(tx, &runtime)?;
    Ok(runtime)
}

pub(crate) fn apply_source_policy_tx(
    tx: &Transaction<'_>,
    id: SourceId,
    expected_source_revision: i64,
    expected_runtime_revision: i64,
    update: SourcePolicyUpdate,
    now: i64,
) -> ControlResult<SourcePolicyWrite> {
    // Step B9: IMMEDIATE 事务中核两个版本，副本计算后一次保存；不生成远端收据。
    ensure(
        positive_revision(expected_source_revision) && positive_revision(expected_runtime_revision),
        Code::InvalidInput,
    )?;
    let (mut source, mut settings, retiring) = source_record(tx, id)?;
    let old_policy = volcano_policy(&settings)?.clone();
    ensure(!retiring && source.retired_at.is_none(), Code::NotReady)?;
    ensure(
        source.config_revision == expected_source_revision,
        Code::RevisionConflict,
    )?;
    let old = read_runtime(tx, id, &old_policy)?.ok_or_else(|| failure(Code::NotReady))?;
    ensure(
        old.revision == expected_runtime_revision,
        Code::RevisionConflict,
    )?;
    let mut disposition = None;
    let mut config_changed = false;
    let (mut runtime, transition) = match update {
        SourcePolicyUpdate::ReplacePolicy(new) => {
            let result =
                volcano::replace_policy(&old, &old_policy, &new, now).map_err(policy_error)?;
            config_changed = new != old_policy;
            if let SourceSettings::Volcano { policy, .. } = &mut settings {
                *policy = new;
            }
            result
        }
        update => {
            let (mut runtime, transition) =
                volcano::advance_period(&old, &old_policy, now).map_err(policy_error)?;
            match update {
                SourcePolicyUpdate::Tick => {}
                SourcePolicyUpdate::ClearError => {
                    runtime = volcano::clear_error(&runtime, now).map_err(policy_error)?
                }
                SourcePolicyUpdate::Evidence(batch) => {
                    let (next, result) =
                        volcano::apply_evidence(&runtime, &old_policy, &batch, now)
                            .map_err(policy_error)?;
                    ensure(result != EvidenceDisposition::Gap, Code::NotReady)?;
                    runtime = next;
                    disposition = Some(result);
                }
                SourcePolicyUpdate::ReplacePolicy(_) => unreachable!(),
            }
            (runtime, transition)
        }
    };
    runtime
        .validate(volcano_policy(&settings)?)
        .map_err(policy_error)?;
    if config_changed {
        source.config_revision =
            bump(source.config_revision).map_err(|_| failure(Code::Storage))?;
        let rows = tx.execute(
            "UPDATE sources SET settings_json=?1,config_revision=?2 WHERE source_id=?3",
            params![
                encode_settings(&settings)?,
                source.config_revision,
                id.get()
            ],
        )?;
        ensure(rows == 1, Code::Storage)?;
    }
    if runtime != old {
        runtime.revision = bump(old.revision).map_err(|_| failure(Code::Storage))?;
        write_runtime(tx, &runtime)?;
    }
    Ok(SourcePolicyWrite {
        source,
        runtime,
        disposition,
        transition,
    })
}

fn decode_group_policy(row: &Row<'_>, group: &GroupView) -> ControlResult<StoredGroupPolicy> {
    let kind: String = get(row, "kind")?;
    let revision: i64 = get(row, "revision")?;
    let active = get::<Option<i64>>(row, "active_member_id")?
        .map(MembershipId::new)
        .transpose()?;
    let pinned = get::<Option<i64>>(row, "pinned_member_id")?
        .map(MembershipId::new)
        .transpose()?;
    let regime: Option<String> = get(row, "regime")?;
    let order: Option<i64> = get(row, "cursor_order")?;
    let cursor = get::<Option<i64>>(row, "cursor_member_id")?
        .map(MembershipId::new)
        .transpose()?;
    ensure(
        kind == group.kind.as_str() && positive_revision(revision),
        Code::Storage,
    )?;
    let state = match group.kind {
        SourceKind::Glm => {
            ensure(order.is_none() && cursor.is_none(), Code::Storage)?;
            let regime = match regime.as_deref() {
                Some("normal") => Regime::Normal,
                Some("degraded") => Regime::Degraded,
                _ => return Err(failure(Code::Storage)),
            };
            GroupPolicyState::Glm(GlmGroupState {
                active,
                pinned,
                regime,
            })
        }
        SourceKind::Volcano => {
            ensure(
                pinned.is_none() && regime.is_none() && order.is_some() == cursor.is_some(),
                Code::Storage,
            )?;
            let state = VolcanoGroupState {
                preferred: active,
                cursor: order.zip(cursor),
            };
            state.validate_shape().map_err(|_| failure(Code::Storage))?;
            GroupPolicyState::Volcano(state)
        }
    };
    Ok(StoredGroupPolicy {
        group_id: group.group_id,
        policy_revision: revision,
        state,
    })
}
fn validate_group_state(
    conn: &Connection,
    group: &GroupView,
    state: &GroupPolicyState,
    code: Code,
) -> ControlResult<()> {
    let ids = match (group.kind, state) {
        (SourceKind::Glm, GroupPolicyState::Glm(s)) => [s.active, s.pinned, None],
        (SourceKind::Volcano, GroupPolicyState::Volcano(s)) => {
            s.validate_shape().map_err(|_| failure(code))?;
            [s.preferred, s.cursor.map(|(_, id)| id), None]
        }
        _ => return Err(failure(code)),
    };
    for id in ids.into_iter().flatten() {
        let belongs: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM memberships WHERE membership_id=?1 AND group_id=?2 AND kind=?3)",
            params![id.get(),group.group_id.get(),group.kind.as_str()], |r| r.get(0))?;
        ensure(belongs, code)?;
    }
    Ok(())
}
pub(super) fn read_group_policy(
    conn: &Connection,
    group: &GroupView,
) -> ControlResult<StoredGroupPolicy> {
    let result = conn
        .query_row(
            "SELECT * FROM group_policy_state WHERE group_id=?1",
            [group.group_id.get()],
            |r| Ok(decode_group_policy(r, group)),
        )
        .optional()?
        .ok_or_else(|| failure(Code::Storage))??;
    validate_group_state(conn, group, &result.state, Code::Storage)?;
    Ok(result)
}
pub(crate) fn save_group_policy_tx(
    tx: &Transaction<'_>,
    id: GroupId,
    expected_group_revision: i64,
    expected_policy_revision: i64,
    state: GroupPolicyState,
) -> ControlResult<StoredGroupPolicy> {
    ensure(
        positive_revision(expected_group_revision) && positive_revision(expected_policy_revision),
        Code::InvalidInput,
    )?;
    let group = group_record(tx, id)?;
    ensure(
        group.revision == expected_group_revision,
        Code::RevisionConflict,
    )?;
    let old = read_group_policy(tx, &group)?;
    ensure(
        old.policy_revision == expected_policy_revision,
        Code::RevisionConflict,
    )?;
    validate_group_state(tx, &group, &state, Code::InvalidInput)?;
    if old.state == state {
        return Ok(old);
    }
    let revision = bump(old.policy_revision).map_err(|_| failure(Code::Storage))?;
    let (active, pinned, regime, order, cursor) = match &state {
        GroupPolicyState::Glm(s) => (
            s.active,
            s.pinned,
            Some(match s.regime {
                Regime::Normal => "normal",
                Regime::Degraded => "degraded",
            }),
            None,
            None,
        ),
        GroupPolicyState::Volcano(s) => (
            s.preferred,
            None,
            None,
            s.cursor.map(|(n, _)| n),
            s.cursor.map(|(_, id)| id),
        ),
    };
    let rows = tx.execute("UPDATE group_policy_state SET revision=?2,active_member_id=?3,pinned_member_id=?4,regime=?5,cursor_order=?6,cursor_member_id=?7 WHERE group_id=?1",
        params![id.get(),revision,active.map(MembershipId::get),pinned.map(MembershipId::get),regime,order,cursor.map(MembershipId::get)])?;
    ensure(rows == 1, Code::Storage)?;
    Ok(StoredGroupPolicy {
        group_id: id,
        policy_revision: revision,
        state,
    })
}

pub(super) fn verify_policy(conn: &Connection) -> Result<()> {
    // Step B10: 必需行/严格值域/跨表归属全部核对；绝不补缺行或隐式推进时间。
    let source_ids = conn
        .prepare(&format!(
            "SELECT source_id FROM sources WHERE kind='volcano' ORDER BY source_id LIMIT {}",
            MAX_SOURCES + 1
        ))?
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let group_ids = conn
        .prepare(&format!(
            "SELECT group_id FROM source_groups ORDER BY group_id LIMIT {}",
            MAX_GROUPS + 1
        ))?
        .query_map([], |r| r.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let sources: i64 = conn.query_row("SELECT count(*) FROM volcano_runtime", [], |r| r.get(0))?;
    let groups: i64 =
        conn.query_row("SELECT count(*) FROM group_policy_state", [], |r| r.get(0))?;
    require(
        source_ids.len() <= MAX_SOURCES
            && group_ids.len() <= MAX_GROUPS
            && sources == source_ids.len() as i64
            && groups == group_ids.len() as i64,
        ErrorKind::CorruptData,
    )?;
    let verify = || -> ControlResult<()> {
        for id in source_ids {
            let id = SourceId::new(id)?;
            let (_, settings, _) = source_record(conn, id)?;
            read_runtime(conn, id, volcano_policy(&settings)?)?;
        }
        for id in group_ids {
            let group = group_record(conn, GroupId::new(id)?)?;
            read_group_policy(conn, &group)?;
        }
        Ok(())
    };
    verify().map_err(|_| error(ErrorKind::CorruptData))
}
