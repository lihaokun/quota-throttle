use crate::catalog::control::{ControlErrorCode, CreateState, MemberTarget, StatusDirective};
use crate::catalog::{
    ChannelId, GroupId, GroupView, MembershipId, MembershipLifecycle, MembershipView, SourceId,
    SourceKind, SourceView, JSON_SAFE, MAX_MEMBERSHIPS, MAX_SOURCES,
};
use crate::model_catalog::{model_sets_equal, normalize_models_csv};
use crate::newapi::managed_channels::{ChannelObservation, ChannelPatch, NativeRecoveryCheck};
use std::collections::HashSet;
use std::fmt;
use std::time::SystemTime;

pub const OBSERVATION_MAX_AGE_SECS: i64 = 30;
pub const SUPPORTED_BACKEND_VERSION: &str = "v1.0.0-rc.20";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision {
    pub member: i64,
    pub source: i64,
    pub group: i64,
    pub execution: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingWrite {
    None,
    InFlight,
    Unknown,
}

/// 输入由可信预检层核实；此类型不是用户 API，也不负责探测实例。
#[derive(Clone, Copy)]
pub struct BackendReadiness {
    ready: bool,
    checked_at: i64,
    expires_at: i64,
}
impl BackendReadiness {
    pub fn from_preflight(
        version: &str,
        recovery: NativeRecoveryCheck,
        instance_verified: bool,
        checked_at: i64,
        expires_at: i64,
    ) -> Self {
        Self {
            ready: version == SUPPORTED_BACKEND_VERSION
                && recovery == NativeRecoveryCheck::Disabled
                && instance_verified,
            checked_at,
            expires_at,
        }
    }
    pub fn ready_at(self, now: i64) -> bool {
        self.ready && valid_window(self.checked_at, self.expires_at, now)
    }
}
/// 只能由可信策略层产生；没有 Deserialize，不能用面板布尔值代替策略判定。
#[derive(Clone, Copy)]
pub struct RoutingPermit {
    source_id: SourceId,
    source_revision: i64,
    group_id: GroupId,
    group_revision: i64,
    priority: i64,
    standby_priority: i64,
    allowed: bool,
    issued_at: i64,
    expires_at: i64,
}
impl RoutingPermit {
    pub fn from_policy(
        source: &SourceView,
        group: &GroupView,
        priority: i64,
        standby_priority: i64,
        allowed: bool,
        issued_at: i64,
        expires_at: i64,
    ) -> Self {
        Self {
            source_id: source.source_id,
            source_revision: source.config_revision,
            group_id: group.group_id,
            group_revision: group.revision,
            priority,
            standby_priority,
            allowed,
            issued_at,
            expires_at,
        }
    }
    fn valid_for(self, snapshot: &WorkSnapshot, now: i64) -> bool {
        self.allowed
            && valid_window(self.issued_at, self.expires_at, now)
            && self.source_id == snapshot.source.source_id
            && self.source_revision == snapshot.source.config_revision
            && self.group_id == snapshot.group.group_id
            && self.group_revision == snapshot.group.revision
            && self.priority == snapshot.target.desired_priority
            && (-JSON_SAFE..=JSON_SAFE).contains(&self.standby_priority)
    }
}

#[derive(Clone)]
pub struct ChannelIdentity {
    pub channel_type: i64,
    pub group: String,
    pub base_url: String,
}
#[derive(Clone)]
pub struct WorkSnapshot {
    pub source: SourceView,
    pub group: GroupView,
    pub member: MembershipView,
    pub target: MemberTarget,
    pub identity: ChannelIdentity,
    pub models: Vec<String>,
    pub managed: bool,
    pub source_retire_requested: bool,
    pub policy_disabled: bool,
    pub pending_write: PendingWrite,
    pub execution_revision: i64,
    /// 当前显式启用授权已消费；普通目标 revision 更新不得清零，须由持久记录提供。
    pub enable_consumed: bool,
}
impl WorkSnapshot {
    pub fn revision(&self) -> Revision {
        Revision {
            member: self.target.revision,
            source: self.source.config_revision,
            group: self.group.revision,
            execution: self.execution_revision,
        }
    }
    fn retiring(&self) -> bool {
        self.source_retire_requested
            || self.source.retired_at.is_some()
            || self.target.retire_requested
            || self.member.lifecycle == MembershipLifecycle::Retired
    }
    fn stopped(&self) -> bool {
        self.retiring() || self.source.manual_paused || self.policy_disabled
    }
    fn directive(&self) -> StatusDirective {
        if self.source.kind == SourceKind::Glm
            && self.target.status_directive == StatusDirective::Enabled
            && self.member.applied_revision == Some(self.target.revision)
        {
            StatusDirective::Preserve
        } else {
            self.target.status_directive
        }
    }
    fn valid(&self) -> bool {
        self.member.membership_id == self.target.membership_id
            && self.member.source_id == self.source.source_id
            && self.member.group_id == self.group.group_id
            && self.source.kind == self.group.kind
            && self.member.desired_priority == self.target.desired_priority
            && self.identity.group == self.group.newapi_group
            && self.identity.channel_type > 0
            && !self.identity.base_url.is_empty()
            && normalized_models(&self.models).is_some()
            && (-JSON_SAFE..=JSON_SAFE).contains(&self.target.desired_priority)
            && (0..=JSON_SAFE).contains(&self.execution_revision)
            && [
                self.target.revision,
                self.source.config_revision,
                self.group.revision,
            ]
            .iter()
            .all(|v| (1..=JSON_SAFE).contains(v))
            && (self.member.channel_id.is_some()
                == (self.target.create_state == CreateState::Resolved))
    }
}
#[derive(Clone)]
pub struct StampedObservation {
    pub revision: Revision,
    pub channel: ChannelObservation,
}
#[derive(Clone)]
pub struct Peer {
    pub member_id: MembershipId,
    pub group_id: GroupId,
    pub revision: Revision,
    pub channel_id: Option<ChannelId>,
    pub create_state: CreateState,
    pub identity: ChannelIdentity,
    pub desired_priority: i64,
    pub desired_disabled: bool,
    pub pending_write: PendingWrite,
    pub observation: Option<StampedObservation>,
}
pub struct PlanContext<'a> {
    pub backend: BackendReadiness,
    pub permit: Option<RoutingPermit>,
    /// 完整可能路由的成员集，含当前成员与未确认停用的退役中成员。
    /// 不可复用可编辑 priority 集合，也不能从返回的 peers 反推。
    pub expected_members: &'a [MembershipId],
    pub peers: &'a [Peer],
    pub now: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub enum PlanStep {
    Observe,
    CreateDisabled,
    LookupCreate,
    Disable,
    Patch(ChannelPatch),
    Enable,
    Confirm,
    RetireAbsent,
    Wait(ControlErrorCode),
    Reject(ControlErrorCode),
}
impl fmt::Debug for PlanStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Observe => "Observe",
            Self::CreateDisabled => "CreateDisabled",
            Self::LookupCreate => "LookupCreate",
            Self::Disable => "Disable",
            Self::Patch(_) => "Patch([PRIVATE])",
            Self::Enable => "Enable",
            Self::Confirm => "Confirm",
            Self::RetireAbsent => "RetireAbsent",
            Self::Wait(_) => "Wait",
            Self::Reject(_) => "Reject",
        })
    }
}

pub fn plan_next_action(
    snapshot: &WorkSnapshot,
    observation: Option<&StampedObservation>,
    context: &PlanContext<'_>,
) -> PlanStep {
    // Step P11: 先计算来源全局 gate，再核对身份/观测；不从未知创建生成第二个 POST。
    if !snapshot.valid() || !(0..=JSON_SAFE).contains(&context.now) {
        return PlanStep::Reject(ControlErrorCode::InvalidInput);
    }
    let stopped = snapshot.stopped();
    let retiring = snapshot.retiring();
    let Some(channel_id) = snapshot.member.channel_id else {
        if !snapshot.managed {
            return PlanStep::Wait(ControlErrorCode::NotReady);
        }
        if snapshot.target.create_state == CreateState::MayHaveBeenSent {
            return PlanStep::LookupCreate;
        }
        if snapshot.pending_write != PendingWrite::None {
            return waiting_for_write(snapshot.pending_write);
        }
        if retiring {
            return PlanStep::RetireAbsent;
        }
        return if context.backend.ready_at(context.now) && snapshot.identity.channel_type == 8 {
            PlanStep::CreateDisabled
        } else {
            PlanStep::Wait(ControlErrorCode::NotReady)
        };
    };
    let Some(observed) =
        observation.filter(|o| o.revision == snapshot.revision() && fresh(&o.channel, context.now))
    else {
        return PlanStep::Observe;
    };
    if !identity_matches(&snapshot.identity, channel_id, &observed.channel) {
        return PlanStep::Reject(ControlErrorCode::BindingDrift);
    }
    if !snapshot.managed
        || !context.backend.ready_at(context.now)
        || snapshot.identity.channel_type != 8
    {
        return PlanStep::Wait(ControlErrorCode::NotReady);
    }
    let actual = &observed.channel;
    let directive = snapshot.directive();
    if snapshot.pending_write == PendingWrite::InFlight {
        return waiting_for_write(snapshot.pending_write);
    }
    // Step P12: 停止先执行；未知旧写可补偿禁用，但不能因此清除 unknown 或确认完成。
    if (stopped || directive == StatusDirective::Disabled) && actual.status != 2 {
        return PlanStep::Disable;
    }
    if snapshot.pending_write == PendingWrite::Unknown {
        return waiting_for_write(snapshot.pending_write);
    }
    if stopped && directive == StatusDirective::Enabled {
        return PlanStep::Wait(ControlErrorCode::NotReady);
    }
    let permit = context
        .permit
        .filter(|p| p.valid_for(snapshot, context.now) && snapshot.group.ready);
    if directive == StatusDirective::Enabled && permit.is_none() {
        return PlanStep::Wait(ControlErrorCode::NotReady);
    }
    let enable = directive == StatusDirective::Enabled && actual.status != 1;
    if enable && snapshot.source.kind == SourceKind::Glm && snapshot.enable_consumed {
        return PlanStep::Reject(ControlErrorCode::RemoteRejected);
    }
    let desired_priority = if enable && snapshot.source.kind == SourceKind::Volcano {
        snapshot
            .target
            .desired_priority
            .min(permit.unwrap().standby_priority)
    } else {
        snapshot.target.desired_priority
    };
    let routing_raise = actual.status == 1 && desired_priority > actual.priority;
    if routing_raise {
        let Some(permit) = permit else {
            return PlanStep::Wait(ControlErrorCode::NotReady);
        };
        if let Err(code) =
            promotion_barrier(snapshot, context, desired_priority, permit.standby_priority)
        {
            return PlanStep::Wait(code);
        }
    }
    let models = snapshot.models.join(",");
    let patch = ChannelPatch {
        priority: (actual.priority != desired_priority).then_some(desired_priority),
        models: (!model_sets_equal(&actual.models, &models)).then_some(models),
        disable_auto_ban: snapshot.source.kind == SourceKind::Volcano && actual.auto_ban != 0,
    };
    if patch.priority.is_some() || patch.models.is_some() || patch.disable_auto_ban {
        return PlanStep::Patch(patch);
    }
    if enable {
        let permit = permit.unwrap();
        if let Err(code) =
            promotion_barrier(snapshot, context, desired_priority, permit.standby_priority)
        {
            return PlanStep::Wait(code);
        }
        return PlanStep::Enable;
    }
    PlanStep::Confirm
}

fn waiting_for_write(pending: PendingWrite) -> PlanStep {
    PlanStep::Wait(if pending == PendingWrite::Unknown {
        ControlErrorCode::TransportUnknown
    } else {
        ControlErrorCode::NotReady
    })
}
fn valid_window(start: i64, end: i64, now: i64) -> bool {
    (0..=JSON_SAFE).contains(&start) && (0..=JSON_SAFE).contains(&end) && start <= now && now < end
}
fn fresh(observation: &ChannelObservation, now: i64) -> bool {
    let Some(seconds) = observation
        .observed_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
    else {
        return false;
    };
    now.checked_sub(seconds)
        .is_some_and(|age| (0..=OBSERVATION_MAX_AGE_SECS).contains(&age))
}
fn identity_matches(
    identity: &ChannelIdentity,
    id: ChannelId,
    observation: &ChannelObservation,
) -> bool {
    observation.channel_id == id.get()
        && observation.channel_type == identity.channel_type
        && observation.group == identity.group
        && observation.base_url == identity.base_url
        && !observation.is_multi_key
        && (1..=3).contains(&observation.status)
        && (0..=1).contains(&observation.auto_ban)
}

pub fn promotion_barrier(
    snapshot: &WorkSnapshot,
    context: &PlanContext<'_>,
    priority: i64,
    standby: i64,
) -> Result<(), ControlErrorCode> {
    let expected: HashSet<_> = context.expected_members.iter().copied().collect();
    if expected.len() != context.expected_members.len()
        || expected.len() > MAX_SOURCES
        || !expected.contains(&snapshot.member.membership_id)
        || context.peers.len() + 1 != expected.len()
    {
        return Err(ControlErrorCode::NotReady);
    }
    let mut covered = HashSet::from([snapshot.member.membership_id]);
    for peer in context.peers {
        if peer.group_id != snapshot.group.group_id
            || !(0..=JSON_SAFE).contains(&peer.revision.execution)
            || peer.identity.channel_type != 8
            || ![
                peer.revision.member,
                peer.revision.source,
                peer.revision.group,
            ]
            .iter()
            .all(|v| (1..=JSON_SAFE).contains(v))
            || peer.identity.group != snapshot.group.newapi_group
            || peer.revision.group != snapshot.group.revision
            || !expected.contains(&peer.member_id)
            || !covered.insert(peer.member_id)
        {
            return Err(ControlErrorCode::NotReady);
        }
        let Some(id) = peer.channel_id else {
            // 不丢失该 peer 的集合覆盖；从未绑定意味着从未获准 Enable。
            // Never/MayHaveBeenSent 的唯一可能外部效果为禁用创建，不阻挡其它来源。
            if peer.create_state == CreateState::Resolved {
                return Err(ControlErrorCode::InvalidResponse);
            }
            continue;
        };
        if peer.create_state != CreateState::Resolved {
            return Err(ControlErrorCode::InvalidResponse);
        }
        if peer.pending_write != PendingWrite::None {
            return Err(ControlErrorCode::TransportUnknown);
        }
        let Some(observed) = peer
            .observation
            .as_ref()
            .filter(|o| o.revision == peer.revision && fresh(&o.channel, context.now))
        else {
            return Err(ControlErrorCode::NotReady);
        };
        if !identity_matches(&peer.identity, id, &observed.channel) {
            return Err(ControlErrorCode::BindingDrift);
        }
        if observed.channel.status == 1
            && (peer.desired_disabled
                || observed.channel.priority > peer.desired_priority
                || priority > standby && observed.channel.priority >= priority)
        {
            return Err(ControlErrorCode::NotReady);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationMatch {
    Missing,
    Unique(ChannelId),
}
pub fn resolve_created(
    snapshot: &WorkSnapshot,
    complete: bool,
    channels: &[ChannelObservation],
) -> Result<CreationMatch, ControlErrorCode> {
    if !complete || channels.len() > MAX_MEMBERSHIPS {
        return Err(ControlErrorCode::InvalidResponse);
    }
    let matched: Vec<_> = channels
        .iter()
        .filter(|c| c.name == snapshot.target.create_name)
        .collect();
    if matched.len() > 1 {
        return Err(ControlErrorCode::AmbiguousCreate);
    }
    let Some(channel) = matched.first() else {
        return Ok(CreationMatch::Missing);
    };
    let id = ChannelId::new(channel.channel_id).map_err(|_| ControlErrorCode::InvalidResponse)?;
    if !identity_matches(&snapshot.identity, id, channel) {
        return Err(ControlErrorCode::BindingDrift);
    }
    Ok(CreationMatch::Unique(id))
}

/// 对模型输入做轻量有效性复核，worker 仍须从来源成功发现结果取模型。
pub fn normalized_models(models: &[String]) -> Option<Vec<String>> {
    if models.len() > 1000 || models.iter().any(|m| m.contains([',', '\0'])) {
        return None;
    }
    let result = normalize_models_csv(&models.join(","));
    (!result.is_empty()).then_some(result)
}

#[cfg(test)]
mod tests;
