use super::probe::{ProbeCycle, QuotaReading};
use super::PolicyError;
use crate::catalog::{
    GroupId, GroupView, MembershipId, MembershipLifecycle, SourceKind, JSON_SAFE, MAX_MEMBERSHIPS,
    MAX_SOURCES,
};
use crate::config::Config;
use crate::orchestrator::{decide, glm_target_priority, max_watch_pct, Regime, WeeklyInfo};
use crate::reconcile::planner::WorkSnapshot;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlmGroupState {
    pub active: Option<MembershipId>,
    pub pinned: Option<MembershipId>,
    pub regime: Regime,
}
impl Default for GlmGroupState {
    fn default() -> Self {
        Self {
            active: None,
            pinned: None,
            regime: Regime::Normal,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct GlmPinRelease {
    pub membership_id: MembershipId,
    pub pct: f64,
    pub limit: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct GlmGroupDecision {
    pub group_id: GroupId,
    pub group_revision: i64,
    pub cycle_id: u64,
    pub state: GlmGroupState,
    pub eligible: Vec<MembershipId>,
    pub imminent: Vec<MembershipId>,
    pub pin_release: Option<GlmPinRelease>,
    /// None 为 Keep，不能作为全量 priority 命令中的 0。
    pub priorities: Vec<(MembershipId, Option<i64>)>,
}

pub fn decide_glm_group(
    cfg: &Config,
    group: &GroupView,
    members: &[WorkSnapshot],
    cycle: &ProbeCycle,
    previous: &GlmGroupState,
    now_ms: i64,
) -> Result<GlmGroupDecision, PolicyError> {
    // Step P4: 先校验完整快照，再过滤明确停止的来源及成员，不过滤原生 status。
    if members.len() > MAX_MEMBERSHIPS || cycle.observations.len() > MAX_SOURCES {
        return Err(PolicyError::Capacity);
    }
    let lookahead_ms = i64::try_from(cfg.weekly_reset_lookahead_hours)
        .ok()
        .and_then(|hours| hours.checked_mul(3_600_000))
        .ok_or(PolicyError::InvalidInput)?;
    if group.kind != SourceKind::Glm
        || !(1..=JSON_SAFE).contains(&group.revision)
        || now_ms < 0
        || cycle.cycle_id == 0
    {
        return Err(PolicyError::InvalidInput);
    }
    let mut member_ids = HashSet::new();
    let mut sources = HashMap::new();
    let mut candidates = Vec::new();
    let mut active_sources = HashSet::new();
    for w in members {
        if w.group != *group
            || w.member.group_id != group.group_id
            || w.member.source_id != w.source.source_id
            || w.source.kind != SourceKind::Glm
            || !(1..=JSON_SAFE).contains(&w.source.config_revision)
            || w.target.membership_id != w.member.membership_id
            || !member_ids.insert(w.member.membership_id)
        {
            return Err(PolicyError::InvalidInput);
        }
        if let Some(old) = sources.insert(w.source.source_id, &w.source) {
            if old != &w.source {
                return Err(PolicyError::InvalidInput);
            }
        }
        if sources.len() > MAX_SOURCES {
            return Err(PolicyError::Capacity);
        }
        if w.member.lifecycle != MembershipLifecycle::Retired
            && !active_sources.insert(w.source.source_id)
        {
            return Err(PolicyError::InvalidInput);
        }
        if w.source.manual_paused
            || w.source.retired_at.is_some()
            || w.source_retire_requested
            || w.target.retire_requested
            || w.member.lifecycle == MembershipLifecycle::Retired
            || w.member.retired_at.is_some()
            || w.member.channel_id.is_none()
        {
            continue;
        }
        candidates.push(w);
    }
    candidates.sort_by_key(|w| (w.member.order, w.member.membership_id));
    let identities: HashMap<_, _> = candidates
        .iter()
        .map(|w| (w.member.membership_id.get(), w.member.membership_id))
        .collect();
    let ids: Vec<_> = candidates
        .iter()
        .map(|w| w.member.membership_id.get())
        .collect();
    let active = previous
        .active
        .filter(|id| identities.contains_key(&id.get()));
    let mut pinned = previous
        .pinned
        .filter(|id| identities.contains_key(&id.get()));

    // Step P5: 共享同一轮源观测；旧配置、旧轮次、缺失或失败均为未知。
    let mut readings = HashMap::new();
    for reading in &cycle.observations {
        if readings.insert(reading.source_id, reading).is_some() {
            return Err(PolicyError::InvalidInput);
        }
    }
    let mut pct = HashMap::new();
    let mut weekly = HashMap::new();
    for w in &candidates {
        let Some(reading) = readings.get(&w.source.source_id) else {
            continue;
        };
        if reading.config_revision != w.source.config_revision || reading.cycle_id != cycle.cycle_id
        {
            continue;
        }
        let QuotaReading::Available(status) = &reading.reading else {
            continue;
        };
        let id = w.member.membership_id.get();
        let max_pct = max_watch_pct(cfg, status);
        pct.insert(id, max_pct);
        if let Some(window) = &status.weekly {
            if window.next_reset_time.checked_sub(now_ms).is_none() {
                return Err(PolicyError::InvalidInput);
            }
            weekly.insert(
                id,
                WeeklyInfo {
                    reset_ms: window.next_reset_time,
                    pct: window.percentage,
                    max_pct,
                },
            );
        }
    }
    // Step P6: 复用旧算法，并保留旧调用方的 None/解除固定语义。
    let decision = decide(
        &ids,
        &pct,
        &weekly,
        now_ms,
        lookahead_ms,
        active.map(MembershipId::get),
        pinned.map(MembershipId::get),
        cfg.throttle_threshold,
        cfg.restore_threshold,
        cfg.exhausted_threshold,
    );
    if decision.pin_release.is_some() {
        pinned = None;
    }
    let active = decision.active.map(|id| identities[&id]).or(active);
    // Step P7: 同旧 tick 的有序 priority 分支，不将未知填成 0。
    let priorities = ids
        .iter()
        .map(|&id| {
            (
                identities[&id],
                glm_target_priority(
                    id,
                    active.map(MembershipId::get),
                    decision.eligible.contains(&id),
                    pct.contains_key(&id),
                    cfg,
                ),
            )
        })
        .collect();
    Ok(GlmGroupDecision {
        group_id: group.group_id,
        group_revision: group.revision,
        cycle_id: cycle.cycle_id,
        state: GlmGroupState {
            active,
            pinned,
            regime: decision.regime,
        },
        eligible: decision.eligible.iter().map(|id| identities[id]).collect(),
        imminent: decision.imminent.iter().map(|id| identities[id]).collect(),
        pin_release: decision.pin_release.map(|release| GlmPinRelease {
            membership_id: identities[&release.channel_id],
            pct: release.pct,
            limit: release.limit,
        }),
        priorities,
    })
}
