//! 已提交观察的只读展示；不重算策略、不刷新任何路由许可。
#![cfg_attr(not(test), allow(dead_code))]
use super::*;
use crate::catalog::panel::{CommittedGroupAssessment, GroupInputStamp, PanelCatalog};
use crate::quota::{QuotaStatus, WindowStatus};
use crate::source_policy::probe::{ProbeCycle, QuotaReading};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::TryLockError;

pub(crate) const PANEL_SCHEMA_VERSION: i64 = 1;
const DISPLAY_MAX_AGE_MS: i64 = OBSERVATION_MAX_AGE_SECS * 1000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PanelSnapshot {
    pub schema_version: i64,
    pub captured_at_ms: i64,
    pub backend_ready: bool,
    pub catalog: PanelCatalog,
    pub glm: GlmPanelView,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ObservationState {
    Unknown,
    Observed,
    Unavailable,
    Stale,
}
impl ObservationState {
    pub(crate) const VALUES: [Self; 4] = [
        Self::Unknown,
        Self::Observed,
        Self::Unavailable,
        Self::Stale,
    ];
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct WindowView {
    pub percentage: f64,
    pub reset_at_ms: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GlmQuotaView {
    pub source_id: SourceId,
    pub source_revision: i64,
    pub observed_at_ms: Option<i64>,
    pub state: ObservationState,
    pub five_hour: Option<WindowView>,
    pub weekly: Option<WindowView>,
    pub level: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PinReleaseView {
    pub membership_id: MembershipId,
    pub pct: f64,
    pub limit: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GlmGroupAssessment {
    pub group_id: GroupId,
    pub cycle_id: Option<String>,
    pub evaluated_at_ms: Option<i64>,
    pub current: bool,
    pub imminent_members: Option<Vec<MembershipId>>,
    pub pin_release: Option<PinReleaseView>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct GlmPanelView {
    pub cycle_id: Option<String>,
    pub completed_at_ms: Option<i64>,
    pub sources: Vec<GlmQuotaView>,
    pub groups: Vec<GlmGroupAssessment>,
}

struct QuotaData {
    five_hour: Option<WindowView>,
    weekly: Option<WindowView>,
    level: Option<String>,
}
enum DisplayReading {
    Data(QuotaData),
    Unavailable,
    Invalid,
}
struct Sample {
    source_revision: i64,
    cycle_id: u64,
    observed_at_ms: i64,
    reading: DisplayReading,
}
pub(super) struct GlmDisplayCache {
    cycle_id: u64,
    completed_at_ms: i64,
    sources: BTreeMap<SourceId, Sample>,
    groups: BTreeMap<GroupId, CommittedGroupAssessment>,
}
fn valid_number(value: f64) -> bool {
    value.is_finite() && (0.0..=JSON_SAFE as f64).contains(&value)
}
fn window(value: &Option<WindowStatus>) -> Option<WindowView> {
    let value = value.as_ref().filter(|v| valid_number(v.percentage))?;
    Some(WindowView {
        percentage: value.percentage,
        reset_at_ms: (1..=JSON_SAFE)
            .contains(&value.next_reset_time)
            .then_some(value.next_reset_time),
    })
}
fn quota_data(status: &QuotaStatus) -> DisplayReading {
    // Step A1.5: 只清理外部展示数字，不改变原策略输入或吞掉持久存储错误。
    let five_hour = window(&status.five_hour);
    let weekly = window(&status.weekly);
    if five_hour.is_none() && weekly.is_none() {
        return DisplayReading::Invalid;
    }
    let level = status
        .level
        .as_ref()
        .filter(|s| s.chars().take(129).count() <= 128 && !s.trim().is_empty() && !s.contains('\0'))
        .cloned();
    DisplayReading::Data(QuotaData {
        five_hour,
        weekly,
        level,
    })
}
fn recent(observed_at: i64, at: i64) -> bool {
    at.checked_sub(observed_at)
        .is_some_and(|age| (0..=DISPLAY_MAX_AGE_MS).contains(&age))
}
impl GlmDisplayCache {
    pub(super) fn new(cycle: &ProbeCycle, assessments: Vec<CommittedGroupAssessment>) -> Self {
        let sources = cycle
            .observations()
            .iter()
            .map(|o| {
                (
                    o.source_id,
                    Sample {
                        source_revision: o.config_revision,
                        cycle_id: o.cycle_id,
                        observed_at_ms: o.observed_at_ms,
                        reading: match &o.reading {
                            QuotaReading::Available(s) => quota_data(s),
                            QuotaReading::Unavailable => DisplayReading::Unavailable,
                        },
                    },
                )
            })
            .collect();
        Self {
            cycle_id: cycle.cycle_id(),
            completed_at_ms: cycle.completed_at_ms(),
            sources,
            groups: assessments.into_iter().map(|g| (g.group_id, g)).collect(),
        }
    }
    fn sample_current(&self, source: SourceId, revision: i64, at: i64) -> bool {
        self.sources.get(&source).is_some_and(|s| {
            s.source_revision == revision
                && s.cycle_id == self.cycle_id
                && recent(s.observed_at_ms, at)
                && !matches!(s.reading, DisplayReading::Invalid)
        })
    }
    fn group_current(
        &self,
        old: &CommittedGroupAssessment,
        current: &GroupInputStamp,
        at: i64,
    ) -> bool {
        old.stamp == *current
            && old.cycle_id == self.cycle_id
            && recent(old.evaluated_at_ms, at)
            && current
                .relevant_sources()
                .into_iter()
                .all(|(id, rev)| self.sample_current(id, rev, at))
            && old.imminent_members.iter().all(|id| {
                current
                    .source_for(*id)
                    .and_then(|source| self.sources.get(&source))
                    .and_then(|s| match &s.reading {
                        DisplayReading::Data(d) => d.weekly.as_ref(),
                        _ => None,
                    })
                    .and_then(|w| w.reset_at_ms)
                    .is_some_and(|reset| reset > at)
            })
    }
}
fn quota_view(source: &SourceView, cache: Option<&GlmDisplayCache>, at: i64) -> GlmQuotaView {
    let mut result = GlmQuotaView {
        source_id: source.source_id,
        source_revision: source.config_revision,
        observed_at_ms: None,
        state: ObservationState::Unknown,
        five_hour: None,
        weekly: None,
        level: None,
    };
    let Some(cache) = cache else {
        return result;
    };
    let Some(sample) = cache.sources.get(&source.source_id) else {
        return result;
    };
    result.observed_at_ms = Some(sample.observed_at_ms);
    if sample.source_revision != source.config_revision
        || sample.cycle_id != cache.cycle_id
        || !recent(sample.observed_at_ms, at)
    {
        result.state = ObservationState::Stale;
        return result;
    }
    match &sample.reading {
        DisplayReading::Unavailable | DisplayReading::Invalid => {
            result.state = ObservationState::Unavailable
        }
        DisplayReading::Data(data) => {
            result.state = ObservationState::Observed;
            result.five_hour = data.five_hour.clone();
            result.weekly = data.weekly.clone();
            result.level = data.level.clone();
        }
    }
    result
}
pub(super) fn project_glm(
    catalog: &PanelCatalog,
    cache: Option<&GlmDisplayCache>,
    at: i64,
) -> ControlResult<GlmPanelView> {
    let sources = catalog
        .sources
        .iter()
        .filter(|s| s.source.kind == SourceKind::Glm)
        .map(|s| quota_view(&s.source, cache, at))
        .collect();
    let mut groups = Vec::new();
    for group in catalog
        .groups
        .iter()
        .filter(|g| g.group.kind == SourceKind::Glm)
    {
        let old = cache.and_then(|c| c.groups.get(&group.group.group_id));
        let current = match cache.zip(old) {
            Some((c, g)) => c.group_current(g, &GroupInputStamp::from_catalog(group, catalog)?, at),
            None => false,
        };
        let pin_release = old
            .filter(|_| current)
            .and_then(|g| g.pin_release.as_ref())
            .filter(|r| valid_number(r.pct) && valid_number(r.limit))
            .map(|r| PinReleaseView {
                membership_id: r.membership_id,
                pct: r.pct,
                limit: r.limit,
            });
        groups.push(GlmGroupAssessment {
            group_id: group.group.group_id,
            cycle_id: old.map(|g| g.cycle_id.to_string()),
            evaluated_at_ms: old.map(|g| g.evaluated_at_ms),
            current,
            imminent_members: old.filter(|_| current).map(|g| g.imminent_members.clone()),
            pin_release,
        });
    }
    Ok(GlmPanelView {
        cycle_id: cache.map(|c| c.cycle_id.to_string()),
        completed_at_ms: cache.map(|c| c.completed_at_ms),
        sources,
        groups,
    })
}
impl ReconcileWorker {
    pub(crate) fn panel_snapshot(&self) -> ControlResult<PanelSnapshot> {
        self.panel_snapshot_at(policy::current_ms()?)
    }
    pub(super) fn panel_snapshot_at(&self, at: i64) -> ControlResult<PanelSnapshot> {
        // Step A1.4: 原Core中的同一目录/已提交缓存；读取不推进周期、版本或网络。
        if !(0..=JSON_SAFE).contains(&at) {
            return Err(error(ControlErrorCode::InvalidInput));
        }
        let mut core = self.core.try_lock().map_err(|e| {
            error(match e {
                TryLockError::WouldBlock => ControlErrorCode::QueueFull,
                TryLockError::Poisoned(_) => ControlErrorCode::Storage,
            })
        })?;
        let catalog = core.store.panel_catalog()?;
        let glm = project_glm(&catalog, core.panel_glm.as_ref(), at)?;
        Ok(PanelSnapshot {
            schema_version: PANEL_SCHEMA_VERSION,
            captured_at_ms: at,
            backend_ready: core.backend.ready_at(at / 1000),
            catalog,
            glm,
        })
    }
}
