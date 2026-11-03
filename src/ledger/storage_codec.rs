//! 私有持久格式；不是NativeFact的通用serde能力。
use super::*;
use serde_json::{json, Value};

fn invalid() -> FactError {
    FactError::InvalidShape
}
fn stream_name(s: StreamObservation) -> &'static str {
    match s {
        StreamObservation::Missing => "missing",
        StreamObservation::Done => "done",
        StreamObservation::Eof => "eof",
        StreamObservation::ClientGone => "client_gone",
        StreamObservation::Other => "other",
    }
}
fn value(m: &RelayMetadataState) -> Value {
    match m {
        RelayMetadataState::Absent => json!({"version":1,"state":"absent"}),
        RelayMetadataState::Unusable => json!({"version":1,"state":"unusable"}),
        RelayMetadataState::Parsed(m) => json!({"version":1,"state":"parsed",
            "status_code":m.status_code,"error_type":m.error_type,"error_code":m.error_code,
            "attempted_channels":m.attempted_channels.as_ref().map(|v|v.iter().map(|id|id.get()).collect::<Vec<_>>()),
            "stream":stream_name(m.stream)}),
    }
}
pub(crate) fn encode(m: &RelayMetadataState) -> Result<String> {
    // Step B1.6: 只输出固定白名单；读取端再核闭合字段集合和完整规范不变量。
    super::decode::validate_metadata(m)?;
    let encoded = value(m).to_string();
    ensure(encoded.len() <= MAX_OTHER_BYTES, FactError::LimitExceeded)?;
    Ok(encoded)
}
fn text(v: &Value) -> Result<Option<String>> {
    if v.is_null() {
        Ok(None)
    } else {
        Ok(Some(v.as_str().ok_or_else(invalid)?.to_owned()))
    }
}
pub(crate) fn decode(raw: &str) -> Result<RelayMetadataState> {
    ensure(raw.len() <= MAX_OTHER_BYTES, FactError::LimitExceeded)?;
    let v: Value = serde_json::from_str(raw).map_err(|_| invalid())?;
    let m = match v.get("state").and_then(Value::as_str) {
        Some("absent") => RelayMetadataState::Absent,
        Some("unusable") => RelayMetadataState::Unusable,
        Some("parsed") => {
            let field = |name| v.get(name).ok_or_else(invalid);
            let status = field("status_code")?;
            let status_code = if status.is_null() {
                None
            } else {
                Some(
                    status
                        .as_u64()
                        .filter(|n| (100..=599).contains(n))
                        .ok_or_else(invalid)? as u16,
                )
            };
            let attempts = field("attempted_channels")?;
            let attempted_channels = if attempts.is_null() {
                None
            } else {
                let a = attempts
                    .as_array()
                    .filter(|a| a.len() <= MAX_ATTEMPTS)
                    .ok_or_else(invalid)?;
                Some(
                    a.iter()
                        .map(|id| {
                            ChannelId::new(id.as_i64().ok_or_else(invalid)?).map_err(|_| invalid())
                        })
                        .collect::<Result<Vec<_>>>()?,
                )
            };
            let stream = match field("stream")?.as_str() {
                Some("missing") => StreamObservation::Missing,
                Some("done") => StreamObservation::Done,
                Some("eof") => StreamObservation::Eof,
                Some("client_gone") => StreamObservation::ClientGone,
                Some("other") => StreamObservation::Other,
                _ => return Err(invalid()),
            };
            RelayMetadataState::Parsed(RelayMetadata {
                status_code,
                attempted_channels,
                stream,
                error_type: text(field("error_type")?)?,
                error_code: text(field("error_code")?)?,
            })
        }
        _ => return Err(invalid()),
    };
    ensure(v == value(&m), invalid())?; // version、缺字段及未知字段不被静默忽略。
    super::decode::validate_metadata(&m)?;
    Ok(m)
}
