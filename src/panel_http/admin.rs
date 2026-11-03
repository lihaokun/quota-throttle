//! 管理业务的白名单适配；不接CLI，不持有第二写连接。
use super::{HttpErrorCode, Response};
use crate::catalog::control::{ControlError, ControlErrorCode};
use crate::catalog::{CatalogError, ErrorKind, SourceId};
use crate::ledger::analytics::AnalyticsError;
use serde::Serialize;

mod bridge;
pub(super) use bridge::{Client, Supervisor};
mod dispatch;
mod input;
mod output;
mod route;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExtraCode {
    DuplicateSource,
    DuplicateGroup,
    DuplicateMembership,
    TypeMismatch,
    Retired,
    ChannelConflict,
    InvalidQuery,
    LimitExceeded,
    ArithmeticOverflow,
    QueryCancelled,
    QueryTimeout,
    DryRun,
    ResultUnconfirmed,
    ResponseTooLarge,
}
impl ExtraCode {
    pub(super) const VALUES: [Self; 14] = [
        Self::DuplicateSource,
        Self::DuplicateGroup,
        Self::DuplicateMembership,
        Self::TypeMismatch,
        Self::Retired,
        Self::ChannelConflict,
        Self::InvalidQuery,
        Self::LimitExceeded,
        Self::ArithmeticOverflow,
        Self::QueryCancelled,
        Self::QueryTimeout,
        Self::DryRun,
        Self::ResultUnconfirmed,
        Self::ResponseTooLarge,
    ];
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(untagged)]
pub(super) enum ErrorCode {
    Control(ControlErrorCode),
    Extra(ExtraCode),
}
#[derive(Debug, Serialize)]
pub(super) struct AdminError {
    schema_version: u8,
    error: ErrorCode,
    existing_source_id: Option<SourceId>,
}
impl AdminError {
    pub(super) fn control(code: ControlErrorCode) -> Self {
        Self {
            schema_version: 1,
            error: ErrorCode::Control(code),
            existing_source_id: None,
        }
    }
    pub(super) fn extra(code: ExtraCode) -> Self {
        Self {
            schema_version: 1,
            error: ErrorCode::Extra(code),
            existing_source_id: None,
        }
    }
    pub(super) fn duplicate(id: SourceId) -> Self {
        Self {
            existing_source_id: Some(id),
            ..Self::extra(ExtraCode::DuplicateSource)
        }
    }
    fn status(&self) -> &'static str {
        use ControlErrorCode as C;
        use ExtraCode as E;
        match self.error {
            ErrorCode::Control(c) => match c {
                C::InvalidInput => "422 Unprocessable Entity",
                C::NotFound => "404 Not Found",
                C::RevisionConflict
                | C::IdempotencyConflict
                | C::BindingDrift
                | C::AmbiguousCreate => "409 Conflict",
                C::QueueFull | C::NotReady => "503 Service Unavailable",
                C::AuthFailed | C::RemoteRejected | C::InvalidResponse => "502 Bad Gateway",
                C::RateLimited => "429 Too Many Requests",
                C::TransportUnknown => "504 Gateway Timeout",
                C::Storage => "500 Internal Server Error",
            },
            ErrorCode::Extra(e) => match e {
                E::DuplicateSource
                | E::DuplicateGroup
                | E::DuplicateMembership
                | E::Retired
                | E::ChannelConflict => "409 Conflict",
                E::TypeMismatch | E::InvalidQuery | E::LimitExceeded | E::ArithmeticOverflow => {
                    "422 Unprocessable Entity"
                }
                E::QueryCancelled => "503 Service Unavailable",
                E::QueryTimeout | E::ResultUnconfirmed => "504 Gateway Timeout",
                E::DryRun => "403 Forbidden",
                E::ResponseTooLarge => "500 Internal Server Error",
            },
        }
    }
}
impl From<ControlError> for AdminError {
    fn from(e: ControlError) -> Self {
        Self::control(e.code)
    }
}
impl From<CatalogError> for AdminError {
    fn from(e: CatalogError) -> Self {
        use ControlErrorCode as C;
        use ErrorKind as K;
        use ExtraCode as E;
        match e.kind {
            K::InvalidInput => Self::control(C::InvalidInput),
            K::NotFound => Self::control(C::NotFound),
            K::Busy => Self::control(C::QueueFull),
            K::DuplicateSource => e
                .source_id
                .map(Self::duplicate)
                .unwrap_or_else(|| Self::control(C::Storage)),
            K::DuplicateGroup => Self::extra(E::DuplicateGroup),
            K::DuplicateMembership => Self::extra(E::DuplicateMembership),
            K::TypeMismatch => Self::extra(E::TypeMismatch),
            K::Retired => Self::extra(E::Retired),
            K::ChannelConflict => Self::extra(E::ChannelConflict),
            _ => Self::control(C::Storage),
        }
    }
}
impl From<AnalyticsError> for AdminError {
    fn from(e: AnalyticsError) -> Self {
        match e {
            AnalyticsError::InvalidQuery => Self::extra(ExtraCode::InvalidQuery),
            AnalyticsError::LimitExceeded => Self::extra(ExtraCode::LimitExceeded),
            AnalyticsError::ArithmeticOverflow => Self::extra(ExtraCode::ArithmeticOverflow),
            AnalyticsError::Cancelled => Self::extra(ExtraCode::QueryCancelled),
            AnalyticsError::TimedOut => Self::extra(ExtraCode::QueryTimeout),
            AnalyticsError::Busy => Self::control(ControlErrorCode::QueueFull),
            AnalyticsError::Control(c) => Self::control(c),
            AnalyticsError::Ledger(_) => Self::control(ControlErrorCode::Storage),
        }
    }
}
#[derive(Debug)]
pub(super) enum AdminFailure {
    Http(HttpErrorCode, Option<&'static str>),
    Business(AdminError),
}
impl AdminFailure {
    pub(super) fn response(self) -> Response {
        match self {
            Self::Http(code, allow) => {
                let mut response = Response::error(code);
                response.allow = allow;
                response
            }
            Self::Business(error) => match serde_json::to_vec(&error) {
                Ok(bytes) => Response {
                    status: error.status(),
                    body: bytes.into(),
                    age_ms: None,
                    allow: None,
                    content_type: super::ContentType::Json,
                },
                Err(_) => Response::error(HttpErrorCode::Internal),
            },
        }
    }
}
impl From<AdminError> for AdminFailure {
    fn from(e: AdminError) -> Self {
        Self::Business(e)
    }
}
impl From<ControlError> for AdminFailure {
    fn from(e: ControlError) -> Self {
        AdminError::from(e).into()
    }
}
impl From<CatalogError> for AdminFailure {
    fn from(e: CatalogError) -> Self {
        AdminError::from(e).into()
    }
}
impl From<AnalyticsError> for AdminFailure {
    fn from(e: AnalyticsError) -> Self {
        AdminError::from(e).into()
    }
}
fn bad_request() -> AdminFailure {
    AdminFailure::Http(HttpErrorCode::BadRequest, None)
}
fn invalid_input() -> AdminFailure {
    AdminError::control(ControlErrorCode::InvalidInput).into()
}

#[cfg(test)]
pub(super) mod tests;
