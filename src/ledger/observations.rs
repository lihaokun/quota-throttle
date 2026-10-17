//! 来源级观测分类和单请求去重；没有第二份计数器、存储、网络或终态推断。
use super::*;
use crate::catalog::SourceId;
use crate::source_policy::volcano::{OutcomeKind, OutcomeObservation};

const EXCLUDED_CODES: &[&str] = &[
    "invalid_request_error",
    "invalid_request",
    "invalid_parameter",
    "context_length_exceeded",
    "content_filter",
    "content_policy_violation",
    "context_canceled",
    "context_cancelled",
    "client_gone",
    "request_cancelled",
];
const FORBIDDEN_AUTH_CODES: &[&str] = &[
    "invalid_api_key",
    "authentication_error",
    "permission_denied",
];

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ObservationCandidate {
    pub(crate) source_id: SourceId,
    pub(crate) request_id: String,
    pub(crate) occurred_at: i64,
    pub(crate) kind: OutcomeKind,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestObservationPhase {
    Failed,
    TokenFlow,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RequestObservationState {
    pub(crate) source_id: SourceId,
    pub(crate) request_id: String,
    pub(crate) first_event_at: i64,
    pub(crate) phase: RequestObservationPhase,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ObservationDecision {
    pub(crate) next: RequestObservationState,
    pub(crate) outcome: Option<OutcomeObservation>,
}

fn successful_tokens(fact: &NativeFact) -> bool {
    if fact.input == 0 && fact.output == 0 {
        return false;
    }
    match &fact.metadata {
        RelayMetadataState::Absent => !fact.is_stream,
        RelayMetadataState::Unusable => false,
        RelayMetadataState::Parsed(m) => {
            let no_failure = m.status_code.is_none_or(|code| code < 400)
                && m.error_type.as_deref().is_none_or(str::is_empty)
                && m.error_code.as_deref().is_none_or(str::is_empty);
            no_failure
                && if fact.is_stream {
                    matches!(
                        m.stream,
                        StreamObservation::Done
                            | StreamObservation::Eof
                            | StreamObservation::ClientGone
                    )
                } else {
                    m.stream == StreamObservation::Missing
                }
        }
    }
}
fn unavailable(metadata: &RelayMetadataState) -> bool {
    let RelayMetadataState::Parsed(m) = metadata else {
        return false;
    };
    if m.error_type.as_deref() != Some("openai_error")
        || m.error_code
            .as_deref()
            .is_some_and(|code| EXCLUDED_CODES.contains(&code))
        || matches!(m.stream, StreamObservation::ClientGone)
    {
        return false;
    }
    match m.status_code {
        Some(401 | 429 | 500 | 502 | 503 | 504) => true,
        Some(403) => m
            .error_code
            .as_deref()
            .is_some_and(|code| FORBIDDEN_AUTH_CODES.contains(&code)),
        _ => false,
    }
}

pub(crate) fn classify_observation(fact: &AttributedFact) -> Result<Option<ObservationCandidate>> {
    // Step O1: 规范事实、来源/请求身份先校验；未知不是失败，也不能伪造健康。
    validate_attribution(fact)?;
    let (Some(source_id), Some(request_id)) = (fact.source_id, &fact.native.request_id) else {
        return Ok(None);
    };
    let kind = match fact.native.kind {
        FactKind::Consume if successful_tokens(&fact.native) => OutcomeKind::Success,
        FactKind::Error if unavailable(&fact.native.metadata) => OutcomeKind::Unavailable,
        _ => return Ok(None),
    };
    Ok(Some(ObservationCandidate {
        source_id,
        request_id: request_id.clone(),
        occurred_at: fact.native.occurred_at,
        kind,
    }))
}

pub(crate) fn observe_request(
    previous: Option<&RequestObservationState>,
    candidate: &ObservationCandidate,
) -> Result<ObservationDecision> {
    // Step O2: 状态绑定 request/source，只允许尚无观测→失败/活动→活动的单向转移。
    ensure(
        candidate.source_id.get() > 0
            && decode::valid_request_id(&candidate.request_id)
            && (0..=JSON_SAFE).contains(&candidate.occurred_at)
            && candidate.kind != OutcomeKind::Ignored,
        FactError::InvalidShape,
    )?;
    if let Some(old) = previous {
        ensure(
            (0..=JSON_SAFE).contains(&old.first_event_at)
                && decode::valid_request_id(&old.request_id),
            FactError::InvalidShape,
        )?;
        ensure(
            old.source_id == candidate.source_id && old.request_id == candidate.request_id,
            FactError::InvalidAttribution,
        )?;
    }
    let first_event_at = previous.map_or(candidate.occurred_at, |old| {
        old.first_event_at.min(candidate.occurred_at)
    });
    let (phase, emit) = match (previous.map(|old| old.phase), candidate.kind) {
        (Some(RequestObservationPhase::TokenFlow), _) => {
            (RequestObservationPhase::TokenFlow, false)
        }
        (Some(RequestObservationPhase::Failed), OutcomeKind::Unavailable) => {
            (RequestObservationPhase::Failed, false)
        }
        (_, OutcomeKind::Success) => (RequestObservationPhase::TokenFlow, true),
        (_, OutcomeKind::Unavailable) => (RequestObservationPhase::Failed, true),
        (_, OutcomeKind::Ignored) => unreachable!("candidate kind checked above"),
    };
    Ok(ObservationDecision {
        next: RequestObservationState {
            source_id: candidate.source_id,
            request_id: candidate.request_id.clone(),
            first_event_at,
            phase,
        },
        outcome: emit.then_some(OutcomeObservation {
            first_event_at,
            observed_at: candidate.occurred_at,
            kind: candidate.kind,
        }),
    })
}

#[cfg(test)]
mod tests;
