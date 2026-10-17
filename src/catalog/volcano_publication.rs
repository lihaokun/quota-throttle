//! 观测、共享来源状态、全部组目标及许可的一个事务；不发送远端请求。
#![cfg_attr(not(test), allow(dead_code))]
use super::control::{
    bump, ensure, failure, ControlErrorCode as Code, ControlResult, StatusDirective,
};
use super::execution::{group_work, load_work};
use super::ledger::observed::{ledger_ready, project_source, ProjectionBudget, SourceProjection};
use super::policy::{policy_error, read_group_policy, save_group_policy_tx, GroupPolicyState};
use super::store::{commit, read_snapshot};
use super::*;
use crate::config::Config;
use crate::reconcile::planner::{
    BackendReadiness, RoutingPermit, WorkSnapshot, OBSERVATION_MAX_AGE_SECS,
};
use crate::source_policy::volcano::{select_volcano_group, VolcanoState};
use rusqlite::{params, Transaction, TransactionBehavior};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VolcanoRefreshReport {
    pub(crate) preview_only: bool,
    pub(crate) sources: usize,
    pub(crate) observations: usize,
    pub(crate) pending_sources: usize,
    pub(crate) changed_groups: usize,
    pub(crate) changed_members: usize,
    pub(crate) permitted: usize,
    pub(crate) waiting: usize,
}
pub(crate) struct VolcanoPolicyCommit {
    pub(crate) report: VolcanoRefreshReport,
    pub(crate) scope: HashSet<MembershipId>,
    pub(crate) source_scope: HashSet<SourceId>,
    pub(crate) permits: HashMap<MembershipId, RoutingPermit>,
    pub(crate) disabled: HashMap<SourceId, bool>,
}
fn writable(w: &WorkSnapshot) -> bool {
    w.managed
        && w.member.channel_id.is_some()
        && !w.source.manual_paused
        && w.source.retired_at.is_none()
        && !w.source_retire_requested
        && !w.target.retire_requested
        && w.member.lifecycle != MembershipLifecycle::Retired
        && w.member.retired_at.is_none()
}
fn targets(
    tx: &Transaction<'_>,
    work: &[WorkSnapshot],
    mut priorities: BTreeMap<MembershipId, Option<i64>>,
    sources: &BTreeMap<SourceId, SourceProjection>,
    now: i64,
) -> ControlResult<Vec<WorkSnapshot>> {
    let mut changed = Vec::new();
    for w in work {
        let priority = priorities.remove(&w.member.membership_id).flatten();
        if !writable(w) {
            continue;
        }
        let source = sources
            .get(&w.source.source_id)
            .ok_or_else(|| failure(Code::Storage))?;
        let desired = priority.unwrap_or(w.target.desired_priority);
        let status = if source.disabled {
            StatusDirective::Disabled
        } else if source.ready && w.group.ready {
            StatusDirective::Enabled
        } else {
            w.target.status_directive
        };
        if desired == w.target.desired_priority && status == w.target.status_directive {
            continue;
        }
        let rev = bump(w.target.revision)?;
        if status == StatusDirective::Enabled
            && w.target.status_directive != StatusDirective::Enabled
        {
            let epoch: i64 = tx.query_row(
                "SELECT enable_epoch FROM member_execution WHERE membership_id=?1",
                [w.member.membership_id.get()],
                |r| r.get(0),
            )?;
            ensure(tx.execute("UPDATE member_execution SET enable_epoch=?1,enable_requested_revision=?2,enable_consumed=0 WHERE membership_id=?3",params![bump(epoch)?,rev,w.member.membership_id.get()])?==1,Code::Storage)?;
        }
        tx.execute("UPDATE control_operations SET phase='superseded',error_code=NULL,updated_at=?1 WHERE phase NOT IN ('succeeded','superseded') AND operation_id IN (SELECT operation_id FROM operation_targets WHERE membership_id=?2)",params![now,w.member.membership_id.get()])?;
        ensure(tx.execute("UPDATE member_control SET revision=?1,status_directive=?2,dirty=1,next_attempt_at=?3,last_error_code=NULL WHERE membership_id=?4 AND revision=?5",params![rev,status.as_str(),now,w.member.membership_id.get(),w.target.revision])?==1,Code::Storage)?;
        ensure(tx.execute("UPDATE memberships SET desired_priority=?1,applied_revision=NULL WHERE membership_id=?2",params![desired,w.member.membership_id.get()])?==1,Code::Storage)?;
        changed.push(load_work(tx, w.member.membership_id, source.disabled)?);
    }
    ensure(priorities.is_empty(), Code::Storage)?;
    Ok(changed)
}

impl CatalogStore {
    pub(crate) fn preview_volcano_round(
        &mut self,
        cfg: &Config,
        now: i64,
    ) -> ControlResult<VolcanoRefreshReport> {
        self.usable()?;
        super::glm_round::validate_config(cfg)?;
        ensure((0..JSON_SAFE).contains(&now), Code::InvalidInput)?;
        let tx = self.conn.transaction()?;
        let snap = read_snapshot(&tx)?;
        let volcano: HashSet<_> = snap
            .sources
            .iter()
            .filter(|s| s.kind == SourceKind::Volcano)
            .map(|s| s.source_id)
            .collect();
        let report = VolcanoRefreshReport {
            preview_only: true,
            sources: volcano.len(),
            observations: 0,
            pending_sources: 0,
            changed_groups: 0,
            changed_members: 0,
            permitted: 0,
            waiting: snap
                .memberships
                .iter()
                .filter(|m| volcano.contains(&m.source_id))
                .count(),
        };
        tx.rollback()?;
        Ok(report)
    }
    pub(crate) fn commit_volcano_round(
        &mut self,
        cfg: &Config,
        backend: BackendReadiness,
        previous: &HashMap<MembershipId, RoutingPermit>,
        now: i64,
    ) -> ControlResult<VolcanoPolicyCommit> {
        // Step O4.4: 一次事务内观测→状态/历史→所有组目标→最终版本许可；之后才可发布。
        self.usable()?;
        super::glm_round::validate_config(cfg)?;
        ensure(
            !cfg.dry_run && (0..JSON_SAFE).contains(&now),
            Code::InvalidInput,
        )?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut budget = ProjectionBudget::new();
        let snapshot = read_snapshot(&tx)?;
        let (initialized, observed_at) = ledger_ready(&tx, now)?;
        let mut sources = BTreeMap::new();
        for s in snapshot
            .sources
            .iter()
            .filter(|s| s.kind == SourceKind::Volcano)
        {
            sources.insert(
                s.source_id,
                project_source(
                    &tx,
                    s.source_id,
                    now,
                    initialized,
                    observed_at.is_some(),
                    &mut budget,
                )?,
            );
        }
        let states = sources
            .iter()
            .map(|(id, s)| {
                (
                    *id,
                    if s.ready
                        || s.runtime
                            .as_ref()
                            .is_some_and(|r| r.state(false) == VolcanoState::Exhausted)
                    {
                        s.runtime.clone()
                    } else {
                        None
                    },
                )
            })
            .collect();
        let mut result = VolcanoPolicyCommit {
            report: VolcanoRefreshReport {
                preview_only: false,
                sources: sources.len(),
                observations: sources.values().map(|s| s.consumed).sum(),
                pending_sources: sources.values().filter(|s| s.pending).count(),
                changed_groups: 0,
                changed_members: 0,
                permitted: 0,
                waiting: 0,
            },
            scope: snapshot
                .memberships
                .iter()
                .filter(|m| sources.contains_key(&m.source_id))
                .map(|m| m.membership_id)
                .collect(),
            source_scope: sources.keys().copied().collect(),
            disabled: sources.iter().map(|(id, s)| (*id, s.disabled)).collect(),
            permits: HashMap::new(),
        };
        for group in snapshot
            .groups
            .iter()
            .filter(|g| g.kind == SourceKind::Volcano)
        {
            budget.check()?;
            let mut work = group_work(&tx, group.group_id)?;
            let old = read_group_policy(&tx, group)?;
            let GroupPolicyState::Volcano(state) = &old.state else {
                return Err(failure(Code::Storage));
            };
            let decision =
                select_volcano_group(cfg, group, &work, &states, state).map_err(policy_error)?;
            let saved = save_group_policy_tx(
                &tx,
                group.group_id,
                group.revision,
                old.policy_revision,
                GroupPolicyState::Volcano(decision.state),
            )?;
            result.report.changed_groups +=
                usize::from(saved.policy_revision != old.policy_revision);
            let mut priorities = BTreeMap::new();
            for (id, p) in decision.priorities {
                ensure(priorities.insert(id, p).is_none(), Code::Storage)?;
            }
            let updates = targets(&tx, &work, priorities, &sources, now)?;
            result.report.changed_members += updates.len();
            let updates: HashMap<_, _> = updates
                .into_iter()
                .map(|w| (w.member.membership_id, w))
                .collect();
            for w in &mut work {
                if let Some(updated) = updates.get(&w.member.membership_id) {
                    *w = updated.clone();
                }
            }
            for w in &work {
                result.report.waiting += 1;
                let source = sources
                    .get(&w.source.source_id)
                    .ok_or_else(|| failure(Code::Storage))?;
                if !source.ready
                    || !writable(w)
                    || !group.ready
                    || !backend.ready_at(now)
                    || !decision.eligible.contains(&w.member.membership_id)
                {
                    continue;
                }
                let deadline = observed_at
                    .and_then(|at| at.checked_add(OBSERVATION_MAX_AGE_SECS))
                    .filter(|at| *at <= JSON_SAFE)
                    .ok_or_else(|| failure(Code::Storage))?
                    .min(backend.expires_at())
                    .min(
                        now.checked_add(OBSERVATION_MAX_AGE_SECS)
                            .ok_or_else(|| failure(Code::InvalidInput))?,
                    );
                if deadline <= now {
                    continue;
                }
                let p = RoutingPermit::from_policy(
                    w,
                    w.target.desired_priority,
                    cfg.priority_standby,
                    true,
                    now,
                    deadline,
                );
                ensure(p.valid_for(w, now), Code::Storage)?;
                if !previous
                    .get(&w.member.membership_id)
                    .is_some_and(|p| p.valid_for(w, now))
                    && w.target.dirty
                    && w.target.last_error_code == Some(Code::NotReady)
                    && w.target.next_attempt_at > now
                {
                    tx.execute("UPDATE member_control SET next_attempt_at=?1 WHERE membership_id=?2 AND dirty=1 AND last_error_code='not_ready' AND next_attempt_at>?1",params![now,w.member.membership_id.get()])?;
                }
                ensure(
                    result.permits.insert(w.member.membership_id, p).is_none(),
                    Code::Storage,
                )?;
                result.report.waiting -= 1;
            }
        }
        result.report.permitted = result.permits.len();
        budget.check()?;
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
}
