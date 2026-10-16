use super::*;
use serde_json::{Map, Value};

fn required<'a>(row: &'a Map<String, Value>, field: &str) -> Result<&'a Value> {
    row.get(field).ok_or(FactError::InvalidShape)
}
fn number(value: &Value, min: i64, max: i64) -> Result<i64> {
    value
        .as_i64()
        .filter(|v| (min..=max).contains(v))
        .ok_or(FactError::InvalidNumber)
}
pub(super) fn valid_text(text: &str, limit: usize) -> bool {
    text.len() <= limit && !text.chars().any(char::is_control)
}
pub(super) fn valid_label(text: &str) -> bool {
    !text.is_empty() && valid_text(text, MAX_LABEL_BYTES)
}
fn valid_request_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_REQUEST_ID_BYTES
        && text
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn label(value: Option<&Value>, valid: fn(&str) -> bool) -> (Option<String>, bool) {
    match value {
        None | Some(Value::Null) => (None, true),
        Some(Value::String(s)) if s.is_empty() => (None, true),
        Some(Value::String(s)) if valid(s) => (Some(s.clone()), true),
        _ => (None, false),
    }
}
fn optional<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|v| !v.is_null())
}
fn tag(map: &Map<String, Value>, key: &str) -> std::result::Result<Option<String>, ()> {
    match optional(map, key) {
        None => Ok(None),
        Some(Value::String(s)) if valid_text(s, MAX_TAG_BYTES) => Ok(Some(s.clone())),
        _ => Err(()),
    }
}
fn metadata_object(map: &Map<String, Value>) -> std::result::Result<RelayMetadata, ()> {
    let status_code = optional(map, "status_code")
        .map(|v| number(v, 100, 599).map(|v| v as u16).map_err(|_| ()))
        .transpose()?;
    let attempted_channels = match optional(map, "admin_info") {
        None => None,
        Some(value) => {
            let admin = value.as_object().ok_or(())?;
            optional(admin, "use_channel")
                .map(|v| {
                    let rows = v.as_array().filter(|v| v.len() <= MAX_ATTEMPTS).ok_or(())?;
                    rows.iter()
                        .map(|row| {
                            let s = row
                                .as_str()
                                .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                                .ok_or(())?;
                            ChannelId::new(s.parse::<i64>().map_err(|_| ())?).map_err(|_| ())
                        })
                        .collect::<std::result::Result<Vec<_>, ()>>()
                })
                .transpose()?
        }
    };
    let stream = match optional(map, "stream_status") {
        None => StreamObservation::Missing,
        Some(value) => {
            let stream = value.as_object().ok_or(())?;
            let status = tag(stream, "status")?;
            let reason = tag(stream, "end_reason")?;
            match (status.as_deref(), reason.as_deref()) {
                (Some("ok"), Some("done")) => StreamObservation::Done,
                (Some("ok"), Some("eof")) => StreamObservation::Eof,
                (Some("error"), Some("client_gone")) => StreamObservation::ClientGone,
                _ => StreamObservation::Other,
            }
        }
    };
    Ok(RelayMetadata {
        status_code,
        error_type: tag(map, "error_type")?,
        error_code: tag(map, "error_code")?,
        attempted_channels,
        stream,
    })
}
fn metadata(value: Option<&Value>) -> RelayMetadataState {
    let raw = match value {
        None | Some(Value::Null) => return RelayMetadataState::Absent,
        Some(Value::String(s)) if s.is_empty() => return RelayMetadataState::Absent,
        Some(Value::String(s)) if s.len() <= MAX_OTHER_BYTES => s,
        _ => return RelayMetadataState::Unusable,
    };
    let parsed = serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_object().and_then(|map| metadata_object(map).ok()));
    parsed
        .map(RelayMetadataState::Parsed)
        .unwrap_or(RelayMetadataState::Unusable)
}

pub(crate) fn decode_native_row(row: &Value) -> Result<Option<NativeFact>> {
    // Step A1: 核心计量严格验证，可选标签/元数据异常保留消费并降级。
    let row = row.as_object().ok_or(FactError::InvalidShape)?;
    let kind = match number(required(row, "type")?, 0, i64::MAX)? {
        2 => FactKind::Consume,
        5 => FactKind::Error,
        _ => return Ok(None),
    };
    let log_id = number(required(row, "id")?, 1, i64::MAX)?;
    let occurred_at = number(required(row, "created_at")?, 0, JSON_SAFE)?;
    let channel = number(required(row, "channel")?, 0, i64::MAX)?;
    let token = number(required(row, "token_id")?, 0, i64::MAX)?;
    let input = number(required(row, "prompt_tokens")?, 0, JSON_SAFE)?;
    let output = number(required(row, "completion_tokens")?, 0, JSON_SAFE)?;
    let is_stream = required(row, "is_stream")?
        .as_bool()
        .ok_or(FactError::InvalidShape)?;
    let (model, model_valid) = label(row.get("model_name"), valid_label);
    let (native_group, group_valid) = label(row.get("group"), valid_label);
    let (request_id, request_valid) = label(row.get("request_id"), valid_request_id);
    let fact = NativeFact {
        log_id,
        kind,
        occurred_at,
        channel_id: if channel == 0 {
            None
        } else {
            Some(ChannelId::new(channel).map_err(|_| FactError::InvalidNumber)?)
        },
        token_id: if token == 0 { None } else { Some(token) },
        model,
        native_group,
        input,
        output,
        is_stream,
        request_id,
        metadata: if model_valid && group_valid && request_valid {
            metadata(row.get("other"))
        } else {
            RelayMetadataState::Unusable
        },
    };
    validate_fact(&fact)?;
    Ok(Some(fact))
}

pub(super) fn validate_fact(f: &NativeFact) -> Result<()> {
    ensure(
        f.log_id > 0
            && (0..=JSON_SAFE).contains(&f.occurred_at)
            && f.channel_id.is_none_or(|id| id.get() > 0)
            && f.token_id.is_none_or(|id| id > 0)
            && valid_totals(UsageTotals {
                input: f.input,
                output: f.output,
                consumption_rows: 0,
            })
            && (f.kind != FactKind::Error || (f.input == 0 && f.output == 0)),
        FactError::InvalidNumber,
    )?;
    ensure(
        f.model.as_deref().is_none_or(valid_label)
            && f.native_group.as_deref().is_none_or(valid_label)
            && f.request_id.as_deref().is_none_or(valid_request_id),
        FactError::InvalidShape,
    )?;
    if let RelayMetadataState::Parsed(m) = &f.metadata {
        ensure(
            m.status_code.is_none_or(|n| (100..=599).contains(&n))
                && [&m.error_type, &m.error_code]
                    .into_iter()
                    .all(|v| v.as_deref().is_none_or(|s| valid_text(s, MAX_TAG_BYTES)))
                && m.attempted_channels
                    .as_ref()
                    .is_none_or(|v| v.len() <= MAX_ATTEMPTS && v.iter().all(|id| id.get() > 0)),
            FactError::InvalidShape,
        )?;
    }
    Ok(())
}
