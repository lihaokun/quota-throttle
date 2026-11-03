//! 消费已去重观测的火山策略；成功 token 活动清计数，不自动解除耗尽。
//! 没有日志查询、时钟读取、数据库或渠道写入。
use super::PolicyError;
use crate::catalog::{
    GroupId, GroupView, MembershipId, MembershipLifecycle, SourceId, SourceKind, VolcanoPolicy,
    JSON_SAFE, MAX_MEMBERSHIPS, MAX_SOURCES,
};
use crate::config::Config;
use crate::reconcile::planner::WorkSnapshot;
use std::collections::{BTreeMap, BTreeSet};

pub const DAY_SECONDS: i64 = 86_400;
pub const MAX_POLICY_OUTCOMES: usize = 1024;
type Result<T> = std::result::Result<T, PolicyError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolcanoState {
    Paused,
    Available,
    Exhausted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolcanoRuntime {
    pub source_id: SourceId,
    pub revision: i64,
    pub period_id: i64,
    pub period_start: i64,
    pub next_reset_at: i64,
    pub used_input: i64,
    pub used_output: i64,
    pub consecutive_failures: i64,
    pub error_exhausted: bool,
    pub limit_exhausted: bool,
    pub error_epoch: i64,
    pub error_cutoff_at: Option<i64>,
    pub evidence_cursor: Option<i64>,
    pub coverage_end: Option<i64>,
}
impl VolcanoRuntime {
    pub fn state(&self, manual_paused: bool) -> VolcanoState {
        if manual_paused {
            VolcanoState::Paused
        } else if self.error_exhausted || self.limit_exhausted {
            VolcanoState::Exhausted
        } else {
            VolcanoState::Available
        }
    }

    /// 选择器没有私有来源配置，只能校验形状；存储/更新还必须调用 validate。
    pub(crate) fn validate_shape(&self) -> Result<()> {
        ensure(
            [self.revision, self.period_id, self.error_epoch]
                .into_iter()
                .all(positive)
                && [
                    self.period_start,
                    self.next_reset_at,
                    self.used_input,
                    self.used_output,
                    self.consecutive_failures,
                ]
                .into_iter()
                .all(nonnegative)
                && self.period_start < self.next_reset_at
                && sum(self.used_input, self.used_output).is_ok()
                && self
                    .error_cutoff_at
                    .is_none_or(|t| self.period_start <= t && t < self.next_reset_at)
                && self.evidence_cursor.is_none_or(nonnegative)
                && self
                    .coverage_end
                    .is_none_or(|t| self.period_start <= t && t <= self.next_reset_at)
                && (self.evidence_cursor.is_none() || self.coverage_end.is_some()),
        )
    }

    pub(crate) fn validate(&self, policy: &VolcanoPolicy) -> Result<()> {
        validate_policy(policy)?;
        self.validate_shape()?;
        ensure(
            self.limit_exhausted == at_limit(policy, sum(self.used_input, self.used_output)?)
                && (self.error_exhausted || self.consecutive_failures < policy.failure_threshold)
                && (self.next_reset_at + policy.utc_offset_minutes * 60 - policy.reset_minute * 60)
                    .rem_euclid(DAY_SECONDS)
                    == 0,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Success,
    Unavailable,
    Ignored,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeObservation {
    /// 已见有效日志的最早时刻，不证明请求开始。
    pub first_event_at: i64,
    /// 本条观测日志的时刻，不证明请求最终结束。
    pub observed_at: i64,
    pub kind: OutcomeKind,
}
impl OutcomeObservation {
    fn validate(&self) -> Result<()> {
        ensure(
            nonnegative(self.first_event_at)
                && nonnegative(self.observed_at)
                && self.first_event_at <= self.observed_at,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolcanoEvidence {
    pub source_id: SourceId,
    pub period_id: i64,
    pub error_epoch: i64,
    pub from_cursor: Option<i64>,
    pub to_cursor: i64,
    pub used_input: i64,
    pub used_output: i64,
    pub coverage_end: i64,
    pub outcomes: Vec<OutcomeObservation>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceDisposition {
    Applied,
    Duplicate,
    Stale,
    Gap,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodTransition {
    pub old_start: i64,
    pub old_next: i64,
    pub new_start: i64,
    pub new_next: i64,
    pub reset_minute: i64,
    pub utc_offset_minutes: i64,
}

fn ensure(condition: bool) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(PolicyError::InvalidInput)
    }
}
fn nonnegative(n: i64) -> bool {
    (0..=JSON_SAFE).contains(&n)
}
fn positive(n: i64) -> bool {
    (1..=JSON_SAFE).contains(&n)
}
fn sum(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b)
        .filter(|n| nonnegative(*n))
        .ok_or(PolicyError::SequenceExhausted)
}
fn increment(n: i64) -> Result<i64> {
    sum(n, 1)
}
fn validate_policy(policy: &VolcanoPolicy) -> Result<()> {
    policy.validate().map_err(|_| PolicyError::InvalidInput)
}
fn at_limit(policy: &VolcanoPolicy, used: i64) -> bool {
    policy.daily_token_limit.is_some_and(|limit| used >= limit)
}

pub fn daily_bounds(now: i64, policy: &VolcanoPolicy) -> Result<(i64, i64)> {
    // Step B1: 复用配置校验，固定偏移定位日格，不读取主机时区。
    validate_policy(policy)?;
    ensure(nonnegative(now))?;
    // 偏移参数已受小范围约束；日格与时间端点仍显式检查算术。
    let shift = policy.utc_offset_minutes * 60 - policy.reset_minute * 60;
    let start = now
        .checked_add(shift)
        .and_then(|value| value.div_euclid(DAY_SECONDS).checked_mul(DAY_SECONDS))
        .and_then(|value| value.checked_sub(shift))
        .ok_or(PolicyError::SequenceExhausted)?;
    ensure(nonnegative(start))?;
    let next = sum(start, DAY_SECONDS)?;
    ensure(start <= now && now < next)?;
    Ok((start, next))
}

pub fn initialize_runtime(
    source_id: SourceId,
    policy: &VolcanoPolicy,
    now: i64,
) -> Result<VolcanoRuntime> {
    // Step B2: 构造初始化值；有无旧记录由下一步的存储入口负责核对。
    let (period_start, next_reset_at) = daily_bounds(now, policy)?;
    Ok(VolcanoRuntime {
        source_id,
        revision: 1,
        period_id: 1,
        period_start,
        next_reset_at,
        used_input: 0,
        used_output: 0,
        consecutive_failures: 0,
        error_exhausted: false,
        limit_exhausted: false,
        error_epoch: 1,
        error_cutoff_at: None,
        evidence_cursor: None,
        coverage_end: None,
    })
}

pub fn advance_period(
    runtime: &VolcanoRuntime,
    policy: &VolcanoPolicy,
    now: i64,
) -> Result<(VolcanoRuntime, Option<PeriodTransition>)> {
    // Step B3: 到期一次推进到当前日格；回拨/重复 Tick 不倒退、不再重置。
    runtime.validate(policy)?;
    ensure(nonnegative(now))?;
    if now < runtime.next_reset_at {
        return Ok((runtime.clone(), None));
    }
    let days = (now - runtime.next_reset_at) / DAY_SECONDS;
    let elapsed = days
        .checked_mul(DAY_SECONDS)
        .ok_or(PolicyError::SequenceExhausted)?;
    let new_start = sum(runtime.next_reset_at, elapsed)?;
    let new_next = sum(new_start, DAY_SECONDS)?;
    let mut next = runtime.clone();
    next.period_id = increment(runtime.period_id)?;
    next.error_epoch = increment(runtime.error_epoch)?;
    next.period_start = new_start;
    next.next_reset_at = new_next;
    next.used_input = 0;
    next.used_output = 0;
    next.consecutive_failures = 0;
    next.error_exhausted = false;
    next.limit_exhausted = false;
    next.error_cutoff_at = None;
    next.evidence_cursor = None;
    next.coverage_end = None;
    let transition = PeriodTransition {
        old_start: runtime.period_start,
        old_next: runtime.next_reset_at,
        new_start,
        new_next,
        reset_minute: policy.reset_minute,
        utc_offset_minutes: policy.utc_offset_minutes,
    };
    next.validate(policy)?;
    Ok((next, Some(transition)))
}

pub fn apply_usage(
    runtime: &VolcanoRuntime,
    policy: &VolcanoPolicy,
    input: i64,
    output: i64,
) -> Result<VolcanoRuntime> {
    // 编辑上限也复用此函数，不能先用新上限检验旧的 limit_exhausted。
    validate_policy(policy)?;
    runtime.validate_shape()?;
    ensure(
        nonnegative(input)
            && nonnegative(output)
            && input >= runtime.used_input
            && output >= runtime.used_output,
    )?;
    let total = sum(input, output)?;
    let mut next = runtime.clone();
    next.used_input = input;
    next.used_output = output;
    next.limit_exhausted = at_limit(policy, total);
    Ok(next)
}

pub fn apply_outcome(
    runtime: &VolcanoRuntime,
    policy: &VolcanoPolicy,
    outcome: &OutcomeObservation,
) -> Result<VolcanoRuntime> {
    // Step O3: 既有时间/代次门禁不变；Success 清计数，耗尽原因仍锁存。
    runtime.validate(policy)?;
    outcome.validate()?;
    let mut next = runtime.clone();
    if outcome.first_event_at < runtime.period_start
        || outcome.observed_at >= runtime.next_reset_at
        || runtime
            .error_cutoff_at
            .is_some_and(|cutoff| outcome.first_event_at <= cutoff)
    {
        return Ok(next);
    }
    match outcome.kind {
        OutcomeKind::Ignored => {}
        OutcomeKind::Success => next.consecutive_failures = 0,
        OutcomeKind::Unavailable => {
            next.consecutive_failures = increment(runtime.consecutive_failures)?;
            next.error_exhausted |= next.consecutive_failures >= policy.failure_threshold;
        }
    }
    Ok(next)
}

pub fn apply_evidence(
    runtime: &VolcanoRuntime,
    policy: &VolcanoPolicy,
    batch: &VolcanoEvidence,
    now: i64,
) -> Result<(VolcanoRuntime, EvidenceDisposition)> {
    // Step B4: 完整批次在副本计算，失败不消费游标或部分事实。
    runtime.validate(policy)?;
    if batch.outcomes.len() > MAX_POLICY_OUTCOMES {
        return Err(PolicyError::Capacity);
    }
    ensure(
        batch.source_id == runtime.source_id
            && positive(batch.period_id)
            && positive(batch.error_epoch)
            && batch.from_cursor.is_none_or(nonnegative)
            && nonnegative(batch.to_cursor)
            && nonnegative(batch.used_input)
            && nonnegative(batch.used_output)
            && nonnegative(batch.coverage_end)
            && nonnegative(now),
    )?;
    sum(batch.used_input, batch.used_output)?;
    ensure(
        batch.to_cursor != 0
            || (batch.outcomes.is_empty() && batch.used_input == 0 && batch.used_output == 0),
    )?;
    for outcome in &batch.outcomes {
        outcome.validate()?;
        ensure(outcome.observed_at < batch.coverage_end)?;
    }
    if batch.period_id != runtime.period_id || batch.error_epoch != runtime.error_epoch {
        return Ok((runtime.clone(), EvidenceDisposition::Stale));
    }
    if runtime
        .evidence_cursor
        .is_some_and(|cursor| batch.to_cursor <= cursor)
    {
        return Ok((runtime.clone(), EvidenceDisposition::Duplicate));
    }
    if batch.from_cursor != runtime.evidence_cursor
        || batch
            .from_cursor
            .is_some_and(|from| batch.to_cursor <= from)
    {
        return Ok((runtime.clone(), EvidenceDisposition::Gap));
    }
    ensure(
        runtime.period_start <= now
            && now < runtime.next_reset_at
            && batch.coverage_end >= runtime.period_start
            && batch.coverage_end <= runtime.next_reset_at.min(sum(now, 1)?)
            && runtime
                .coverage_end
                .is_none_or(|old| batch.coverage_end >= old),
    )?;
    let mut next = apply_usage(runtime, policy, batch.used_input, batch.used_output)?;
    for outcome in &batch.outcomes {
        next = apply_outcome(&next, policy, outcome)?;
    }
    next.evidence_cursor = Some(batch.to_cursor);
    next.coverage_end = Some(batch.coverage_end);
    next.validate(policy)?;
    Ok((next, EvidenceDisposition::Applied))
}

pub fn clear_error(runtime: &VolcanoRuntime, now: i64) -> Result<VolcanoRuntime> {
    // Step B5: 原因隔离；没有 manual_paused 的所有权，不能顺便恢复来源。
    runtime.validate_shape()?;
    ensure(
        nonnegative(now)
            && runtime.period_start <= now
            && now < runtime.next_reset_at
            && runtime.error_cutoff_at.is_none_or(|cutoff| now >= cutoff),
    )?;
    let mut next = runtime.clone();
    next.error_epoch = increment(runtime.error_epoch)?;
    next.error_exhausted = false;
    next.consecutive_failures = 0;
    next.error_cutoff_at = Some(now);
    next.evidence_cursor = None;
    Ok(next)
}

pub fn replace_policy(
    runtime: &VolcanoRuntime,
    old: &VolcanoPolicy,
    new: &VolcanoPolicy,
    now: i64,
) -> Result<(VolcanoRuntime, Option<PeriodTransition>)> {
    // Step B6: 旧规则先到期；编辑新规则不能直接清空当前消费。
    validate_policy(new)?;
    let (mut next, mut transition) = advance_period(runtime, old, now)?;
    ensure(now >= next.period_start && next.error_cutoff_at.is_none_or(|cutoff| now >= cutoff))?;
    if old.reset_minute != new.reset_minute || old.utc_offset_minutes != new.utc_offset_minutes {
        next.next_reset_at = daily_bounds(now, new)?.1;
    }
    next = apply_usage(&next, new, next.used_input, next.used_output)?;
    next.error_exhausted |= next.consecutive_failures >= new.failure_threshold;
    next.validate(new)?;
    if let Some(t) = &mut transition {
        t.new_next = next.next_reset_at;
    }
    Ok((next, transition))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VolcanoGroupState {
    pub preferred: Option<MembershipId>,
    pub cursor: Option<(i64, MembershipId)>,
}
impl VolcanoGroupState {
    pub(crate) fn validate_shape(&self) -> Result<()> {
        ensure(
            self.cursor.is_none_or(|(order, _)| nonnegative(order))
                && self
                    .preferred
                    .is_none_or(|id| self.cursor.is_some_and(|(_, cursor)| cursor == id)),
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolcanoGroupDecision {
    pub group_id: GroupId,
    pub group_revision: i64,
    pub state: VolcanoGroupState,
    pub eligible: Vec<MembershipId>,
    pub source_revisions: Vec<(SourceId, i64, Option<i64>)>,
    pub priorities: Vec<(MembershipId, Option<i64>)>,
}

pub fn select_volcano_group(
    cfg: &Config,
    group: &GroupView,
    members: &[WorkSnapshot],
    shared_states: &BTreeMap<SourceId, Option<VolcanoRuntime>>,
    previous: &VolcanoGroupState,
) -> Result<VolcanoGroupDecision> {
    // Step B7: 完整身份验证在资格过滤之前；只输出本组成员，不碰渠道。
    if members.len() > MAX_MEMBERSHIPS || shared_states.len() > MAX_SOURCES {
        return Err(PolicyError::Capacity);
    }
    previous.validate_shape()?;
    ensure(
        group.kind == SourceKind::Volcano
            && positive(group.revision)
            && [
                cfg.priority_active,
                cfg.priority_standby,
                cfg.priority_exhausted,
            ]
            .into_iter()
            .all(|p| (-JSON_SAFE..=JSON_SAFE).contains(&p))
            && cfg.priority_active > cfg.priority_standby
            && cfg.priority_standby >= cfg.priority_exhausted,
    )?;
    let mut member_ids = BTreeSet::new();
    let mut active_sources = BTreeSet::new();
    let mut sources = BTreeMap::new();
    let mut retained = Vec::new();
    for w in members {
        ensure(
            w.group == *group
                && w.member.group_id == group.group_id
                && w.member.source_id == w.source.source_id
                && w.source.kind == SourceKind::Volcano
                && positive(w.source.config_revision)
                && nonnegative(w.member.order)
                && w.target.membership_id == w.member.membership_id
                && member_ids.insert(w.member.membership_id),
        )?;
        if let Some(old) = sources.insert(w.source.source_id, &w.source) {
            ensure(old == &w.source)?;
        }
        if sources.len() > MAX_SOURCES {
            return Err(PolicyError::Capacity);
        }
        if w.member.lifecycle != MembershipLifecycle::Retired {
            ensure(active_sources.insert(w.source.source_id))?;
        }
        let state = shared_states
            .get(&w.source.source_id)
            .ok_or(PolicyError::InvalidInput)?;
        if let Some(state) = state {
            ensure(state.source_id == w.source.source_id)?;
            state.validate_shape()?;
        }
        if w.source.retired_at.is_some()
            || w.source_retire_requested
            || w.target.retire_requested
            || w.member.lifecycle == MembershipLifecycle::Retired
            || w.member.retired_at.is_some()
        {
            continue;
        }
        let stopped = w.source.manual_paused
            || state
                .as_ref()
                .is_some_and(|r| r.state(false) == VolcanoState::Exhausted);
        let available = !stopped && w.member.channel_id.is_some() && state.is_some();
        retained.push((w, stopped, available));
    }
    retained.sort_by_key(|(w, _, _)| (w.member.order, w.member.membership_id));
    let candidates: Vec<_> = retained
        .iter()
        .filter(|(_, _, available)| *available)
        .map(|(w, _, _)| (w.member.order, w.member.membership_id))
        .collect();
    let keep = previous
        .preferred
        .is_some_and(|id| candidates.iter().any(|(_, candidate)| *candidate == id));
    let state = if keep {
        previous.clone()
    } else {
        let choice = candidates
            .iter()
            .find(|key| previous.cursor.is_none_or(|cursor| **key > cursor))
            .or_else(|| candidates.first())
            .copied();
        VolcanoGroupState {
            preferred: choice.map(|(_, id)| id),
            cursor: choice.or(previous.cursor),
        }
    };
    let priorities = retained
        .iter()
        .map(|(w, stopped, available)| {
            let id = w.member.membership_id;
            let priority = if *stopped {
                Some(cfg.priority_exhausted)
            } else if !*available {
                None
            } else if state.preferred == Some(id) {
                Some(cfg.priority_active)
            } else {
                Some(cfg.priority_standby)
            };
            (id, priority)
        })
        .collect();
    let retained_source_ids: BTreeSet<_> = retained
        .iter()
        .map(|(w, _, _)| w.source.source_id)
        .collect();
    let source_revisions = sources
        .into_iter()
        .filter(|(id, _)| retained_source_ids.contains(id))
        .map(|(id, source)| {
            (
                id,
                source.config_revision,
                shared_states[&id].as_ref().map(|r| r.revision),
            )
        })
        .collect();
    Ok(VolcanoGroupDecision {
        group_id: group.group_id,
        group_revision: group.revision,
        state,
        eligible: candidates.iter().map(|(_, id)| *id).collect(),
        source_revisions,
        priorities,
    })
}

#[cfg(test)]
mod tests;
