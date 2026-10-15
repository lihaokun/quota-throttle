use super::PolicyError;
use crate::catalog::{
    GlmQuotaProfile, SecretString, SourceId, SourceKind, SourceMaterial, SourceSettings,
    SourceView, JSON_SAFE, MAX_SOURCES,
};
use crate::config::ZhipuConfig;
use crate::quota::{QuotaProbe, QuotaStatus};
use std::collections::HashSet;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ProbeInput {
    pub(super) source_id: SourceId,
    pub(super) config_revision: i64,
    quota: GlmQuotaProfile,
    credential: SecretString,
}
impl ProbeInput {
    /// 仅接受当前可采集的 GLM 来源；配置与凭据由目录同一快照提供。
    pub fn new(source: &SourceView, material: SourceMaterial) -> Result<Self, PolicyError> {
        if source.kind != SourceKind::Glm
            || source.manual_paused
            || source.retired_at.is_some()
            || !(1..=JSON_SAFE).contains(&source.config_revision)
        {
            return Err(PolicyError::InvalidInput);
        }
        let SourceSettings::Glm { quota, .. } = material.settings else {
            return Err(PolicyError::InvalidInput);
        };
        Ok(Self {
            source_id: source.source_id,
            config_revision: source.config_revision,
            quota,
            credential: material.credential,
        })
    }
}
impl fmt::Debug for ProbeInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeInput")
            .field("source_id", &self.source_id)
            .field("config_revision", &self.config_revision)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum QuotaReading {
    Available(QuotaStatus),
    Unavailable,
}
#[derive(Debug, Clone)]
pub struct QuotaObservation {
    pub source_id: SourceId,
    pub config_revision: i64,
    pub cycle_id: u64,
    pub observed_at_ms: i64,
    pub reading: QuotaReading,
}
#[derive(Debug, Clone)]
pub struct ProbeCycle {
    pub(super) cycle_id: u64,
    pub(super) completed_at_ms: i64,
    pub(super) observations: Vec<QuotaObservation>,
}
impl ProbeCycle {
    pub fn cycle_id(&self) -> u64 {
        self.cycle_id
    }
    pub fn completed_at_ms(&self) -> i64 {
        self.completed_at_ms
    }
    pub fn observations(&self) -> &[QuotaObservation] {
        &self.observations
    }
}

#[derive(Default)]
pub struct SourceProbe {
    pub(super) last_cycle: u64,
}
impl SourceProbe {
    pub async fn collect_cycle(
        &mut self,
        mut inputs: Vec<ProbeInput>,
    ) -> Result<ProbeCycle, PolicyError> {
        // Step P1: 全量校验早于任何请求；取消的轮次也不再复用编号。
        if inputs.len() > MAX_SOURCES {
            return Err(PolicyError::Capacity);
        }
        let mut unique = HashSet::new();
        for input in &inputs {
            if !(1..=JSON_SAFE).contains(&input.config_revision) || !unique.insert(input.source_id)
            {
                return Err(PolicyError::InvalidInput);
            }
        }
        inputs.sort_by_key(|input| input.source_id);
        let cycle_id = self
            .last_cycle
            .checked_add(1)
            .ok_or(PolicyError::SequenceExhausted)?;
        now_ms()?;
        self.last_cycle = cycle_id;
        let mut observations = Vec::with_capacity(inputs.len());
        // Step P2: 以来源而非成员为单位查询，不重试，不持有目录或执行器锁。
        for input in inputs {
            let probe = QuotaProbe::new(&ZhipuConfig {
                quota_url: input.quota.quota_url,
                extra_headers: input.quota.global_headers,
            });
            let reading = match probe
                .query(input.credential.expose(), &input.quota.key_headers)
                .await
            {
                Ok(status) => QuotaReading::Available(status),
                Err(_) => QuotaReading::Unavailable,
            };
            observations.push(QuotaObservation {
                source_id: input.source_id,
                config_revision: input.config_revision,
                cycle_id,
                observed_at_ms: now_ms()?,
                reading,
            });
        }
        // Step P3: 只返回整轮结果；失败来源也保留明确的未知记录。
        Ok(ProbeCycle {
            cycle_id,
            completed_at_ms: now_ms()?,
            observations,
        })
    }
}

fn now_ms() -> Result<i64, PolicyError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .ok_or(PolicyError::InvalidInput)
}
