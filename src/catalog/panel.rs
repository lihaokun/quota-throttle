//! 面板白名单读模型；只读当前目录，不扫描执行历史或来源凭据表。
#![cfg_attr(not(test), allow(dead_code))]
use super::control::{
    failure, member_target, ControlErrorCode as Code, ControlResult, CreateState, StatusDirective,
};
use super::policy::{read_group_policy, read_runtime, source_record, GroupPolicyState};
use super::store::read_snapshot;
use super::*;
use crate::reconcile::planner::WorkSnapshot;
use crate::source_policy::glm::{GlmGroupDecision, GlmPinRelease};
use crate::source_policy::volcano::VolcanoRuntime;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PanelCatalog {
    pub sources: Vec<PanelSource>,
    pub groups: Vec<PanelGroup>,
    pub memberships: Vec<PanelMembership>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PanelSource {
    pub source: SourceView,
    pub retire_requested: bool,
    pub model_count: u32,
    pub volcano: Option<VolcanoPanel>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct VolcanoPanel {
    pub policy: VolcanoPolicy,
    pub runtime: Option<VolcanoRuntimeView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct VolcanoRuntimeView {
    pub revision: i64,
    pub period_id: i64,
    pub period_start: i64,
    pub next_reset_at: i64,
    pub used_input: i64,
    pub used_output: i64,
    pub consecutive_failures: i64,
    pub error_exhausted: bool,
    pub limit_exhausted: bool,
    pub coverage_end: Option<i64>,
}
impl From<VolcanoRuntime> for VolcanoRuntimeView {
    fn from(r: VolcanoRuntime) -> Self {
        Self {
            revision: r.revision,
            period_id: r.period_id,
            period_start: r.period_start,
            next_reset_at: r.next_reset_at,
            used_input: r.used_input,
            used_output: r.used_output,
            consecutive_failures: r.consecutive_failures,
            error_exhausted: r.error_exhausted,
            limit_exhausted: r.limit_exhausted,
            coverage_end: r.coverage_end,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PanelGroup {
    pub group: GroupView,
    pub policy_revision: i64,
    pub selected_member_id: Option<MembershipId>,
    pub pinned_member_id: Option<MembershipId>,
    pub regime: Option<&'static str>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PanelMembership {
    pub membership: MembershipView,
    pub control_revision: i64,
    pub status_directive: StatusDirective,
    pub retire_requested: bool,
    pub create_state: CreateState,
    pub dirty: bool,
    pub next_attempt_at: i64,
    pub error_code: Option<Code>,
}

impl CatalogStore {
    pub(crate) fn panel_catalog(&mut self) -> ControlResult<PanelCatalog> {
        // Step A1.1: 同一只读事务投影全部当前/历史身份，缺必需行整份失败。
        self.usable()?;
        let tx = self.conn.transaction()?;
        let catalog = read_snapshot(&tx)?;
        let mut sources = Vec::with_capacity(catalog.sources.len());
        for source in catalog.sources {
            let (_, settings, retire_requested) =
                source_record(&tx, source.source_id).map_err(|_| failure(Code::Storage))?;
            let (upstream, volcano) = match settings {
                SourceSettings::Glm { upstream, .. } => (upstream, None),
                SourceSettings::Volcano { upstream, policy } => {
                    let runtime = read_runtime(&tx, source.source_id, &policy)?.map(Into::into);
                    (upstream, Some(VolcanoPanel { policy, runtime }))
                }
            };
            sources.push(PanelSource {
                source,
                retire_requested,
                model_count: upstream.models.len() as u32,
                volcano,
            });
        }
        let mut groups = Vec::with_capacity(catalog.groups.len());
        for group in catalog.groups {
            let policy = read_group_policy(&tx, &group)?;
            let (selected_member_id, pinned_member_id, regime) = match policy.state {
                GroupPolicyState::Glm(s) => (s.active, s.pinned, Some(s.regime.as_str())),
                GroupPolicyState::Volcano(s) => (s.preferred, None, None),
            };
            groups.push(PanelGroup {
                group,
                policy_revision: policy.policy_revision,
                selected_member_id,
                pinned_member_id,
                regime,
            });
        }
        let mut memberships = Vec::with_capacity(catalog.memberships.len());
        for membership in catalog.memberships {
            let target =
                member_target(&tx, membership.membership_id).map_err(|_| failure(Code::Storage))?;
            memberships.push(PanelMembership {
                membership,
                control_revision: target.revision,
                status_directive: target.status_directive,
                retire_requested: target.retire_requested,
                create_state: target.create_state,
                dirty: target.dirty,
                next_attempt_at: target.next_attempt_at,
                error_code: target.last_error_code,
            });
        }
        tx.rollback()?;
        Ok(PanelCatalog {
            sources,
            groups,
            memberships,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct DecisionMemberStamp {
    member_id: MembershipId,
    source_id: SourceId,
    source_revision: i64,
    source_paused: bool,
    source_retired_at: Option<i64>,
    source_retire_requested: bool,
    member_revision: i64,
    order: i64,
    channel_id: Option<ChannelId>,
    lifecycle: MembershipLifecycle,
    retired_at: Option<i64>,
    retire_requested: bool,
}
impl DecisionMemberStamp {
    fn new(
        source: &SourceView,
        source_retire_requested: bool,
        member: &MembershipView,
        revision: i64,
        retire_requested: bool,
    ) -> Self {
        Self {
            member_id: member.membership_id,
            source_id: source.source_id,
            source_revision: source.config_revision,
            source_paused: source.manual_paused,
            source_retired_at: source.retired_at,
            source_retire_requested,
            member_revision: revision,
            order: member.order,
            channel_id: member.channel_id,
            lifecycle: member.lifecycle,
            retired_at: member.retired_at,
            retire_requested,
        }
    }
    fn relevant(&self) -> bool {
        self.channel_id.is_some()
            && self.lifecycle != MembershipLifecycle::Retired
            && self.retired_at.is_none()
            && !self.retire_requested
            && !self.source_paused
            && self.source_retired_at.is_none()
            && !self.source_retire_requested
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct GroupInputStamp {
    group_revision: i64,
    policy_revision: i64,
    members: Vec<DecisionMemberStamp>,
}
impl GroupInputStamp {
    pub(crate) fn from_catalog(group: &PanelGroup, catalog: &PanelCatalog) -> ControlResult<Self> {
        let sources: BTreeMap<_, _> = catalog
            .sources
            .iter()
            .map(|s| (s.source.source_id, s))
            .collect();
        let mut members = Vec::new();
        for row in catalog.memberships.iter().filter(|m| {
            m.membership.group_id == group.group.group_id
                && m.membership.lifecycle != MembershipLifecycle::Retired
        }) {
            let source = sources
                .get(&row.membership.source_id)
                .ok_or_else(|| failure(Code::Storage))?;
            members.push(DecisionMemberStamp::new(
                &source.source,
                source.retire_requested,
                &row.membership,
                row.control_revision,
                row.retire_requested,
            ));
        }
        members.sort_by_key(|m| m.member_id);
        Ok(Self {
            group_revision: group.group.revision,
            policy_revision: group.policy_revision,
            members,
        })
    }
    pub(crate) fn relevant_sources(&self) -> BTreeMap<SourceId, i64> {
        self.members
            .iter()
            .filter(|m| m.relevant())
            .map(|m| (m.source_id, m.source_revision))
            .collect()
    }
    pub(crate) fn source_for(&self, member: MembershipId) -> Option<SourceId> {
        self.members
            .iter()
            .find(|m| m.member_id == member)
            .map(|m| m.source_id)
    }
}
#[derive(Clone)]
pub(crate) struct CommittedGroupAssessment {
    pub group_id: GroupId,
    pub cycle_id: u64,
    pub evaluated_at_ms: i64,
    pub stamp: GroupInputStamp,
    pub imminent_members: Vec<MembershipId>,
    pub pin_release: Option<GlmPinRelease>,
}
impl CommittedGroupAssessment {
    pub(crate) fn capture(
        group: &GroupView,
        policy_revision: i64,
        work: &[WorkSnapshot],
        decision: &GlmGroupDecision,
        at: i64,
    ) -> Self {
        // Step A1.2: 原决策和已保存的版本，在原事务提交之前准备展示证据。
        let mut members: Vec<_> = work
            .iter()
            .map(|w| {
                DecisionMemberStamp::new(
                    &w.source,
                    w.source_retire_requested,
                    &w.member,
                    w.target.revision,
                    w.target.retire_requested,
                )
            })
            .collect();
        members.sort_by_key(|m| m.member_id);
        Self {
            group_id: group.group_id,
            cycle_id: decision.cycle_id,
            evaluated_at_ms: at,
            stamp: GroupInputStamp {
                group_revision: group.revision,
                policy_revision,
                members,
            },
            imminent_members: decision.imminent.clone(),
            pin_release: decision.pin_release.clone(),
        }
    }
}
