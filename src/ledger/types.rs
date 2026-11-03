use crate::catalog::{ChannelId, GroupId, SourceId};
use std::collections::BTreeMap;

pub(crate) const MAX_FACTS: usize = 1024;
pub(crate) const MAX_HISTORY: usize = 10_000;
pub(crate) const MAX_LABEL_BYTES: usize = 256;
pub(crate) const MAX_REQUEST_ID_BYTES: usize = 64;
pub(crate) const MAX_OTHER_BYTES: usize = 16 * 1024;
pub(crate) const MAX_TAG_BYTES: usize = 128;
pub(crate) const MAX_ATTEMPTS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FactError {
    InvalidShape,
    InvalidNumber,
    LimitExceeded,
    ConflictingIdentity,
    InvalidAttribution,
    ArithmeticOverflow,
}
impl std::fmt::Display for FactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "facts: {self:?}")
    }
}
impl std::error::Error for FactError {}
pub(crate) type Result<T> = std::result::Result<T, FactError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FactKind {
    Consume,
    Error,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamObservation {
    Missing,
    Done,
    Eof,
    ClientGone,
    Other,
}

// 不派生Debug/Serialize，元数据不是普通日志或前端载荷。
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum RelayMetadataState {
    Absent,
    Parsed(RelayMetadata),
    Unusable,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RelayMetadata {
    pub(crate) status_code: Option<u16>,
    pub(crate) error_type: Option<String>,
    pub(crate) error_code: Option<String>,
    pub(crate) attempted_channels: Option<Vec<ChannelId>>,
    pub(crate) stream: StreamObservation,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct NativeFact {
    pub(crate) log_id: i64,
    pub(crate) kind: FactKind,
    pub(crate) occurred_at: i64,
    pub(crate) channel_id: Option<ChannelId>,
    pub(crate) token_id: Option<i64>,
    pub(crate) model: Option<String>,
    pub(crate) native_group: Option<String>,
    pub(crate) input: i64,
    pub(crate) output: i64,
    pub(crate) is_stream: bool,
    pub(crate) request_id: Option<String>,
    pub(crate) metadata: RelayMetadataState,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ChannelAttribution {
    pub(crate) channel_id: ChannelId,
    pub(crate) source_id: SourceId,
    pub(crate) group_id: GroupId,
    pub(crate) native_group: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttributionIssue {
    None,
    UnmappedChannel,
    GroupUnresolved,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AttributedFact {
    pub(crate) native: NativeFact,
    pub(crate) source_id: Option<SourceId>,
    pub(crate) group_id: Option<GroupId>,
    pub(crate) issue: AttributionIssue,
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct UsageCellKey {
    pub(crate) minute: i64,
    pub(crate) channel_id: Option<ChannelId>,
    pub(crate) source_id: Option<SourceId>,
    pub(crate) group_id: Option<GroupId>,
    pub(crate) native_group: Option<String>,
    pub(crate) token_id: Option<i64>,
    pub(crate) model: Option<String>,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UsageTotals {
    pub(crate) input: i64,
    pub(crate) output: i64,
    pub(crate) consumption_rows: i64,
}
#[derive(Default, PartialEq, Eq)]
pub(crate) struct UsageDelta {
    pub(crate) cells: BTreeMap<UsageCellKey, UsageTotals>,
    pub(crate) total: UsageTotals,
}
