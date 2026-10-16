//! C-1 提交与许可：只有当前事务的最终成员版本可以得到正向执行资格。
#![cfg_attr(not(test), allow(dead_code))]
use super::control::{ensure, failure, ControlErrorCode as Code, ControlResult};
use super::glm_round::{apply_glm_round_tx, compute_glm_round_tx, editable, GlmRoundDraft};
use super::store::commit;
use super::*;
use crate::config::Config;
use crate::reconcile::planner::{BackendReadiness, RoutingPermit, OBSERVATION_MAX_AGE_SECS};
use crate::source_policy::probe::{ProbeCycle, QuotaReading};
use rusqlite::{params, Transaction, TransactionBehavior};
use std::collections::{HashMap, HashSet};

/// 仅本地提交/预览计数，不是远端收敛或凭据报告。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PolicyRefreshReport {
    pub cycle_id: u64,
    pub preview_only: bool,
    pub changed_groups: usize,
    pub changed_members: usize,
    pub permitted: usize,
    pub waiting: usize,
}
pub(crate) struct GlmPolicyCommit {
    pub report: PolicyRefreshReport,
    pub permits: HashMap<MembershipId, RoutingPermit>,
    pub changed_groups: Vec<GroupId>,
    pub changed_members: Vec<MembershipId>,
}

fn build_glm_permits(
    cfg: &Config,
    cycle: &ProbeCycle,
    backend: BackendReadiness,
    round: &GlmRoundDraft,
    now_ms: i64,
) -> ControlResult<HashMap<MembershipId, RoutingPermit>> {
    // Step C5: 最终成员版本 + 当前资格 + 来源观测/预检/发布预算的交集。
    let now = now_ms / 1000;
    let deadline = |at: i64| {
        at.checked_add(OBSERVATION_MAX_AGE_SECS)
            .filter(|v| (0..=JSON_SAFE).contains(v))
            .ok_or_else(|| failure(Code::InvalidInput))
    };
    let publication_end = deadline(now)?;
    let observations: HashMap<_, _> = cycle
        .observations()
        .iter()
        .map(|o| (o.source_id, o))
        .collect();
    let mut permits = HashMap::new();
    if !backend.ready_at(now) {
        return Ok(permits);
    }
    for g in &round.groups {
        let eligible: HashSet<_> = g.decision.eligible.iter().copied().collect();
        for w in &g.work {
            if !editable(w) || !w.group.ready || !eligible.contains(&w.member.membership_id) {
                continue;
            }
            let Some(o) = observations.get(&w.source.source_id).filter(|o| {
                o.cycle_id == cycle.cycle_id()
                    && o.config_revision == w.source.config_revision
                    && o.observed_at_ms <= now_ms
                    && matches!(o.reading, QuotaReading::Available(_))
            }) else {
                continue;
            };
            let end = deadline(o.observed_at_ms / 1000)?
                .min(backend.expires_at())
                .min(publication_end);
            if end <= now {
                continue;
            }
            let permit = RoutingPermit::from_policy(
                w,
                w.target.desired_priority,
                cfg.priority_standby,
                true,
                now,
                end,
            );
            ensure(permit.valid_for(w, now), Code::Storage)?;
            ensure(
                permits.insert(w.member.membership_id, permit).is_none(),
                Code::Storage,
            )?;
        }
    }
    Ok(permits)
}

fn wake_recovered_tx(
    tx: &Transaction<'_>,
    round: &GlmRoundDraft,
    previous: &HashMap<MembershipId, RoutingPermit>,
    permits: &HashMap<MembershipId, RoutingPermit>,
    now: i64,
) -> ControlResult<()> {
    for w in round.groups.iter().flat_map(|g| &g.work) {
        let id = w.member.membership_id;
        if permits.contains_key(&id)
            && !previous.get(&id).is_some_and(|p| p.valid_for(w, now))
            && w.target.dirty
            && w.target.last_error_code == Some(Code::NotReady)
            && w.target.next_attempt_at > now
        {
            // 当前许可已经排除暂停/退役/未接管；只提前既有 NotReady，不重置退避或授权。
            tx.execute(
                "UPDATE member_control SET next_attempt_at=?1 WHERE membership_id=?2 AND dirty=1 AND last_error_code='not_ready' AND next_attempt_at>?1",
                params![now, id.get()],
            )?;
        }
    }
    Ok(())
}

impl CatalogStore {
    pub(crate) fn preview_glm_round(
        &mut self,
        cfg: &Config,
        cycle: &ProbeCycle,
        now_ms: i64,
    ) -> ControlResult<PolicyRefreshReport> {
        self.usable()?;
        let tx = self.conn.transaction()?;
        let round = compute_glm_round_tx(&tx, cfg, cycle, now_ms)?;
        let report = PolicyRefreshReport {
            cycle_id: round.cycle_id,
            preview_only: true,
            changed_groups: round
                .groups
                .iter()
                .filter(|g| {
                    g.policy.state != super::policy::GroupPolicyState::Glm(g.decision.state.clone())
                })
                .count(),
            changed_members: round
                .groups
                .iter()
                .flat_map(|g| g.work.iter().zip(&g.targets))
                .filter(|(w, t)| w.target.desired_priority != t.priority)
                .count(),
            permitted: 0,
            waiting: round.groups.iter().map(|g| g.work.len()).sum(),
        };
        tx.rollback()?;
        Ok(report)
    }

    /// 只能由持有 Core 的策略发布入口调用；提交之前构造所有结果，之后没有可失败查询。
    pub(crate) fn commit_glm_round(
        &mut self,
        cfg: &Config,
        cycle: &ProbeCycle,
        backend: BackendReadiness,
        previous: &HashMap<MembershipId, RoutingPermit>,
        now_ms: i64,
    ) -> ControlResult<GlmPolicyCommit> {
        self.usable()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let applied = apply_glm_round_tx(&tx, cfg, cycle, now_ms)?;
        let permits = build_glm_permits(cfg, cycle, backend, &applied.round, now_ms)?;
        wake_recovered_tx(&tx, &applied.round, previous, &permits, now_ms / 1000)?;
        let result = GlmPolicyCommit {
            report: PolicyRefreshReport {
                cycle_id: applied.round.cycle_id,
                preview_only: false,
                changed_groups: applied.changed_groups.len(),
                changed_members: applied.changed_members.len(),
                permitted: permits.len(),
                waiting: applied
                    .round
                    .groups
                    .iter()
                    .map(|g| g.work.len())
                    .sum::<usize>()
                    - permits.len(),
            },
            permits,
            changed_groups: applied.changed_groups,
            changed_members: applied.changed_members,
        };
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
}
