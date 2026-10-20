//! C-1 事务适配：当前目录计算、Keep 与最小目标写。尚无许可/worker/网络接线。
#![cfg_attr(not(test), allow(dead_code))]
use super::control::{
    bump, ensure, failure, member_target, ControlErrorCode as Code, ControlResult, StatusDirective,
};
use super::execution::group_work as load_group_work;
use super::policy::{
    group_record, read_group_policy, save_group_policy_tx, GroupPolicyState, StoredGroupPolicy,
};
use super::store::{decode_membership_view, read_snapshot, MEMBER_COLUMNS};
use super::types::*;
use crate::config::Config;
use crate::reconcile::planner::WorkSnapshot;
use crate::source_policy::glm::{decide_glm_group, GlmGroupDecision};
use crate::source_policy::probe::ProbeCycle;
use crate::source_policy::PolicyError;
use rusqlite::{params, Transaction};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy)]
pub(super) struct PolicyTarget {
    pub membership_id: MembershipId,
    pub expected_revision: i64,
    pub priority: i64,
}
pub(super) struct GlmGroupDraft {
    pub group: GroupView,
    pub policy: StoredGroupPolicy,
    pub decision: GlmGroupDecision,
    pub work: Vec<WorkSnapshot>,
    pub targets: Vec<PolicyTarget>,
}
pub(super) struct GlmRoundDraft {
    pub cycle_id: u64,
    pub groups: Vec<GlmGroupDraft>,
}
/// 仅事务内结果，不是已提交/已发布的许可或远端收据。
pub(super) struct AppliedGlmTargets {
    pub round: GlmRoundDraft,
    pub changed_groups: Vec<GroupId>,
    pub changed_members: Vec<MembershipId>,
}

fn input_error(e: PolicyError) -> super::control::ControlError {
    failure(match e {
        PolicyError::InvalidInput | PolicyError::Capacity => Code::InvalidInput,
        PolicyError::SequenceExhausted => Code::Storage,
    })
}
pub(crate) fn validate_config(cfg: &Config) -> ControlResult<()> {
    cfg.validate().map_err(|_| failure(Code::InvalidInput))?;
    ensure(
        [
            cfg.priority_active,
            cfg.priority_standby,
            cfg.priority_exhausted,
        ]
        .into_iter()
        .all(safe_number)
            && cfg.priority_active > cfg.priority_standby
            && cfg.priority_standby >= cfg.priority_exhausted,
        Code::InvalidInput,
    )
}
fn validate_round(cfg: &Config, cycle: &ProbeCycle, now_ms: i64) -> ControlResult<()> {
    validate_config(cfg)?;
    ensure(
        (0..=JSON_SAFE).contains(&now_ms)
            && (0..=now_ms).contains(&cycle.completed_at_ms())
            && cycle.cycle_id() > 0
            && cycle.observations().len() <= MAX_SOURCES,
        Code::InvalidInput,
    )?;
    let mut ids = HashSet::new();
    for r in cycle.observations() {
        ensure(
            ids.insert(r.source_id)
                && positive_revision(r.config_revision)
                && r.cycle_id > 0
                && (0..=cycle.completed_at_ms()).contains(&r.observed_at_ms),
            Code::InvalidInput,
        )?;
    }
    Ok(())
}
pub(super) fn editable(w: &WorkSnapshot) -> bool {
    w.managed
        && w.member.channel_id.is_some()
        && !w.source.manual_paused
        && w.source.retired_at.is_none()
        && !w.source_retire_requested
        && !w.target.retire_requested
        && w.member.lifecycle != MembershipLifecycle::Retired
        && w.member.retired_at.is_none()
}
/// caller 已确认组存在；该路径的 NotFound 是必需内部行丢失，不是用户查找不到实体。
fn required_group_work(tx: &Transaction<'_>, group: GroupId) -> ControlResult<Vec<WorkSnapshot>> {
    load_group_work(tx, group).map_err(|e| {
        if e.code == Code::NotFound {
            failure(Code::Storage)
        } else {
            e
        }
    })
}
fn complete_targets(
    work: &[WorkSnapshot],
    decision: &GlmGroupDecision,
) -> ControlResult<Vec<PolicyTarget>> {
    let mut priorities = BTreeMap::new();
    for (id, priority) in &decision.priorities {
        ensure(priorities.insert(*id, *priority).is_none(), Code::Storage)?;
    }
    let mut targets = Vec::with_capacity(work.len());
    for w in work {
        let selected = priorities.remove(&w.member.membership_id).flatten();
        let priority = if editable(w) {
            selected.unwrap_or(w.target.desired_priority)
        } else {
            w.target.desired_priority
        };
        targets.push(PolicyTarget {
            membership_id: w.member.membership_id,
            expected_revision: w.target.revision,
            priority,
        });
    }
    ensure(priorities.is_empty(), Code::Storage)?;
    Ok(targets)
}

pub(super) fn compute_glm_round_tx(
    tx: &Transaction<'_>,
    cfg: &Config,
    cycle: &ProbeCycle,
    now_ms: i64,
) -> ControlResult<GlmRoundDraft> {
    // Step C3: 当前同事务快照驱动原算法；Keep 和未接管成员都保留原目标。
    validate_round(cfg, cycle, now_ms)?;
    let snapshot = read_snapshot(tx)?;
    let mut groups = Vec::new();
    for group in snapshot
        .groups
        .into_iter()
        .filter(|g| g.kind == SourceKind::Glm)
    {
        let work = required_group_work(tx, group.group_id)?;
        let policy = read_group_policy(tx, &group)?;
        let GroupPolicyState::Glm(previous) = &policy.state else {
            return Err(failure(Code::Storage));
        };
        let decision =
            decide_glm_group(cfg, &group, &work, cycle, previous, now_ms).map_err(input_error)?;
        let targets = complete_targets(&work, &decision)?;
        groups.push(GlmGroupDraft {
            group,
            policy,
            decision,
            work,
            targets,
        });
    }
    Ok(GlmRoundDraft {
        cycle_id: cycle.cycle_id(),
        groups,
    })
}

pub(super) fn apply_policy_targets_tx(
    tx: &Transaction<'_>,
    group: &GroupView,
    targets: &[PolicyTarget],
    now: i64,
) -> ControlResult<Vec<MembershipId>> {
    ensure(
        group.kind == SourceKind::Glm
            && (0..=JSON_SAFE).contains(&now)
            && targets.len() <= MAX_MEMBERSHIPS,
        Code::InvalidInput,
    )?;
    ensure(
        group_record(tx, group.group_id)? == *group,
        Code::RevisionConflict,
    )?;
    let work = required_group_work(tx, group.group_id)?;
    apply_current_targets_tx(tx, &work, targets, now)
}

/// 只由本模块使用：work 为同一 IMMEDIATE 事务内的当前组快照。
/// 计算后只写独立的组策略表/其他组目标，不会改变本组的这些输入。
fn apply_current_targets_tx(
    tx: &Transaction<'_>,
    work: &[WorkSnapshot],
    targets: &[PolicyTarget],
    now: i64,
) -> ControlResult<Vec<MembershipId>> {
    ensure(targets.len() == work.len(), Code::InvalidInput)?;
    let mut by_id = BTreeMap::new();
    for target in targets {
        ensure(
            safe_number(target.priority)
                && positive_revision(target.expected_revision)
                && by_id.insert(target.membership_id, target).is_none(),
            Code::InvalidInput,
        )?;
    }
    let mut changes = Vec::new();
    for w in work {
        let target = by_id
            .remove(&w.member.membership_id)
            .ok_or_else(|| failure(Code::InvalidInput))?;
        ensure(
            target.expected_revision == w.target.revision,
            Code::RevisionConflict,
        )?;
        ensure(
            editable(w) || target.priority == w.target.desired_priority,
            Code::InvalidInput,
        )?;
        if target.priority == w.target.desired_priority {
            continue;
        }
        let revision = bump(w.target.revision).map_err(|_| failure(Code::Storage))?;
        let directive = if w.target.status_directive == StatusDirective::Enabled
            && (w.enable_consumed || w.member.applied_revision == Some(w.target.revision))
        {
            StatusDirective::Preserve
        } else {
            w.target.status_directive
        };
        changes.push((
            w.member.membership_id,
            w.target.revision,
            revision,
            directive,
            target.priority,
        ));
    }
    ensure(by_id.is_empty(), Code::InvalidInput)?;
    let mut changed = Vec::with_capacity(changes.len());
    for (id, old, revision, directive, priority) in changes {
        tx.execute("UPDATE control_operations SET phase='superseded',error_code=NULL,updated_at=?1 WHERE phase NOT IN ('succeeded','superseded') AND operation_id IN (SELECT operation_id FROM operation_targets WHERE membership_id=?2)",params![now,id.get()])?;
        let count = tx.execute("UPDATE member_control SET revision=?1,status_directive=?2,dirty=1,next_attempt_at=?3,last_error_code=NULL WHERE membership_id=?4 AND revision=?5",
            params![revision,directive.as_str(),now,id.get(),old])?;
        ensure(count == 1, Code::Storage)?;
        let count = tx.execute("UPDATE memberships SET desired_priority=?1,applied_revision=NULL WHERE membership_id=?2",params![priority,id.get()])?;
        ensure(count == 1, Code::Storage)?;
        changed.push(id);
    }
    Ok(changed)
}

/// 同一事务只改了成员/目标字段；其余输入及 pending-write 的依赖均未改写。
fn refresh_target_views_tx(
    tx: &Transaction<'_>,
    work: &mut [WorkSnapshot],
    changed: &[MembershipId],
) -> ControlResult<()> {
    if changed.is_empty() {
        return Ok(());
    }
    let mut remaining: HashSet<_> = changed.iter().copied().collect();
    let mut query = tx.prepare(&format!(
        "SELECT {MEMBER_COLUMNS} FROM memberships WHERE membership_id=?1"
    ))?;
    for w in work {
        let id = w.member.membership_id;
        if !remaining.remove(&id) {
            continue;
        }
        w.member = query.query_row([id.get()], |row| Ok(decode_membership_view(row)))??;
        w.target = member_target(tx, id).map_err(|e| {
            if e.code == Code::NotFound {
                failure(Code::Storage)
            } else {
                e
            }
        })?;
    }
    ensure(remaining.is_empty(), Code::Storage)
}

/// caller 持 IMMEDIATE 事务；任何 Err 必须回滚整个事务，成功仍须调用 commit。
pub(super) fn apply_glm_round_tx(
    tx: &Transaction<'_>,
    cfg: &Config,
    cycle: &ProbeCycle,
    now_ms: i64,
) -> ControlResult<AppliedGlmTargets> {
    // Step C4: 先完整计算，再在同一事务保存所有组选择和必要目标；不产生操作/许可。
    ensure(!cfg.dry_run, Code::InvalidInput)?; // dry_run 仅可调用上面的只读计算。
    let mut round = compute_glm_round_tx(tx, cfg, cycle, now_ms)?;
    let mut changed_groups = Vec::new();
    let mut changed_members = Vec::new();
    for g in &mut round.groups {
        let saved = save_group_policy_tx(
            tx,
            g.group.group_id,
            g.group.revision,
            g.policy.policy_revision,
            GroupPolicyState::Glm(g.decision.state.clone()),
        )?;
        if saved.policy_revision != g.policy.policy_revision {
            changed_groups.push(g.group.group_id);
        }
        let changed = apply_current_targets_tx(tx, &g.work, &g.targets, now_ms / 1000)?;
        g.policy = saved;
        refresh_target_views_tx(tx, &mut g.work, &changed)?;
        changed_members.extend(changed);
        g.targets = complete_targets(&g.work, &g.decision)?;
    }
    Ok(AppliedGlmTargets {
        round,
        changed_groups,
        changed_members,
    })
}
