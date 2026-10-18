use crate::config::{HeaderKV, ModelDiscoveryConfig};
use crate::model_catalog::normalize_models_csv;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;

pub const CATALOG_SNAPSHOT_VERSION: i64 = 1;
pub const DATABASE_SCHEMA_VERSION: i64 = 8;
pub const JSON_SAFE: i64 = 9_007_199_254_740_991;
pub const MAX_SOURCES: usize = 1000;
pub const MAX_GROUPS: usize = 100;
pub const MAX_MEMBERSHIPS: usize = 10000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    DuplicateSource,
    DuplicateGroup,
    DuplicateMembership,
    ChannelConflict,
    NotFound,
    TypeMismatch,
    Retired,
    ImportConflict,
    ImportRequiresEmpty,
    ForeignDatabase,
    UnsupportedSchema,
    UnsafePath,
    AlreadyOpen,
    UnsupportedPlatform,
    Busy,
    Storage,
    CorruptData,
    CommitUnknown,
    BackupFailed,
    BackupExists,
}

/// 不保留底层错误、SQL 参数或用户文本，防止错误链泄露凭据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError {
    pub kind: ErrorKind,
    pub source_id: Option<SourceId>,
    /// 失败备份可能仍留在目标路径，调用方必须提示人工核对。
    pub incomplete_backup: bool,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "catalog: {:?}", self.kind)?;
        if self.incomplete_backup {
            f.write_str(" (incomplete backup requires inspection)")?;
        }
        Ok(())
    }
}
impl std::error::Error for CatalogError {}
pub type Result<T> = std::result::Result<T, CatalogError>;
pub(super) fn error(kind: ErrorKind) -> CatalogError {
    CatalogError {
        kind,
        source_id: None,
        incomplete_backup: false,
    }
}
pub(super) fn require(ok: bool, kind: ErrorKind) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(error(kind))
    }
}

fn parse_positive_id(text: &str) -> Result<i64> {
    require(
        !text.is_empty() && !text.starts_with('0') && text.bytes().all(|b| b.is_ascii_digit()),
        ErrorKind::InvalidInput,
    )?;
    let value = text
        .parse::<i64>()
        .map_err(|_| error(ErrorKind::InvalidInput))?;
    require(value > 0, ErrorKind::InvalidInput)?;
    Ok(value)
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(i64);
        impl $name {
            pub fn new(value: i64) -> Result<Self> {
                require(value > 0, ErrorKind::InvalidInput)?;
                Ok(Self(value))
            }
            pub fn get(self) -> i64 {
                self.0
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                parse_positive_id(&text)
                    .map(Self)
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}
id_type!(SourceId);
id_type!(GroupId);
id_type!(MembershipId);
id_type!(ChannelId);
id_type!(ManagedTokenId);

#[derive(Clone, Copy)]
pub(super) struct SecretRef(pub(super) i64);

macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $value)] $variant),+ }
        impl $name {
            pub const VALUES: &'static [&'static str] = &[$($value),+];
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $value),+ } }
            pub(super) fn parse(value: &str) -> Result<Self> {
                match value { $($value => Ok(Self::$variant)),+, _ => Err(error(ErrorKind::CorruptData)) }
            }
        }
    };
}
wire_enum!(SourceKind { Glm => "glm", Volcano => "volcano" });
wire_enum!(MembershipLifecycle {
    Planned => "planned", Imported => "imported", CreatedDisabled => "created_disabled",
    Ready => "ready", Retired => "retired"
});

#[derive(Clone)]
pub struct SecretString(String);
impl SecretString {
    pub fn new(value: String) -> Result<Self> {
        require(
            !value.is_empty() && value.trim() == value && !value.contains(['\r', '\n', '\0']),
            ErrorKind::InvalidInput,
        )?;
        Ok(Self(value))
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamProfile {
    pub channel_type: i64,
    pub base_url: String,
    pub models: Vec<String>,
    pub model_discovery: Option<ModelDiscoveryConfig>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlmQuotaProfile {
    pub quota_url: String,
    pub global_headers: Vec<HeaderKV>,
    pub key_headers: Vec<HeaderKV>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VolcanoPolicy {
    pub daily_token_limit: Option<i64>,
    pub failure_threshold: i64,
    pub reset_minute: i64,
    pub utc_offset_minutes: i64,
}
impl Default for VolcanoPolicy {
    fn default() -> Self {
        Self {
            daily_token_limit: None,
            failure_threshold: 3,
            reset_minute: 60,
            utc_offset_minutes: 480,
        }
    }
}
impl VolcanoPolicy {
    pub(crate) fn validate(&self) -> Result<()> {
        require(
            self.daily_token_limit.is_none_or(positive_revision)
                && (1..=100).contains(&self.failure_threshold)
                && (0..1440).contains(&self.reset_minute)
                && (-720..=840).contains(&self.utc_offset_minutes),
            ErrorKind::InvalidInput,
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceSettings {
    Glm {
        upstream: UpstreamProfile,
        quota: GlmQuotaProfile,
    },
    Volcano {
        upstream: UpstreamProfile,
        policy: VolcanoPolicy,
    },
}
impl SourceSettings {
    pub fn kind(&self) -> SourceKind {
        match self {
            Self::Glm { .. } => SourceKind::Glm,
            Self::Volcano { .. } => SourceKind::Volcano,
        }
    }
}

pub struct SourceDraft {
    pub name: String,
    pub note: String,
    pub settings: SourceSettings,
    pub credential: SecretString,
}
pub struct SourceMaterial {
    pub settings: SourceSettings,
    pub credential: SecretString,
}
pub(super) struct ValidatedSource {
    pub(super) draft: SourceDraft,
    pub(super) fingerprint: String,
}
// 私有设置可能含 header；连同容器 Debug 一起脱敏。
macro_rules! redacted_debug {
    ($($t:ty),+) => { $(impl fmt::Debug for $t {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("[REDACTED]") }
    })+ };
}
redacted_debug!(
    UpstreamProfile,
    GlmQuotaProfile,
    SourceSettings,
    SourceDraft,
    SourceMaterial,
    ValidatedSource
);

#[derive(Debug)]
pub struct GroupDraft {
    pub name: String,
    pub newapi_group: String,
    pub kind: SourceKind,
}
pub(super) struct ValidatedGroup(pub(super) GroupDraft);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceView {
    pub source_id: SourceId,
    #[serde(rename = "type")]
    pub kind: SourceKind,
    pub name: String,
    pub note: String,
    pub manual_paused: bool,
    pub retired_at: Option<i64>,
    pub config_revision: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupView {
    pub group_id: GroupId,
    pub newapi_group: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: SourceKind,
    pub revision: i64,
    pub ready: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipView {
    pub membership_id: MembershipId,
    pub source_id: SourceId,
    pub group_id: GroupId,
    pub channel_id: Option<ChannelId>,
    pub order: i64,
    pub lifecycle: MembershipLifecycle,
    pub desired_priority: i64,
    pub observed_status: Option<i64>,
    pub observed_priority: Option<i64>,
    pub applied_revision: Option<i64>,
    pub retired_at: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSnapshot {
    pub schema_version: i64,
    pub sources: Vec<SourceView>,
    pub groups: Vec<GroupView>,
    pub memberships: Vec<MembershipView>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportReceipt {
    pub group_id: GroupId,
    pub source_ids: Vec<SourceId>,
    pub membership_ids: Vec<MembershipId>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportOutcome {
    Applied(ImportReceipt),
    AlreadyApplied(ImportReceipt),
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && name.chars().count() <= 128 && !name.contains('\0')
}
pub(crate) fn valid_group(group: &str) -> bool {
    !group.is_empty()
        && group.len() <= 64
        && group != "auto"
        && group
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
pub(super) fn safe_number(value: i64) -> bool {
    (-JSON_SAFE..=JSON_SAFE).contains(&value)
}
pub(super) fn positive_revision(value: i64) -> bool {
    (1..=JSON_SAFE).contains(&value)
}
pub(super) fn valid_timestamp(value: Option<i64>) -> bool {
    value.is_none_or(|v| (0..=JSON_SAFE).contains(&v))
}

fn validate_url(text: &str, allow_query: bool) -> Result<()> {
    let url = reqwest::Url::parse(text).map_err(|_| error(ErrorKind::InvalidInput))?;
    require(
        text.trim() == text
            && matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && (allow_query || url.query().is_none()),
        ErrorKind::InvalidInput,
    )
}
pub(super) fn validate_settings(settings: &mut SourceSettings) -> Result<()> {
    let upstream = match settings {
        SourceSettings::Glm { upstream, quota } => {
            validate_url(&quota.quota_url, true)?;
            for headers in [&quota.global_headers, &quota.key_headers] {
                require(headers.len() <= 64, ErrorKind::InvalidInput)?;
                for header in headers {
                    reqwest::header::HeaderName::from_bytes(header.key.as_bytes())
                        .map_err(|_| error(ErrorKind::InvalidInput))?;
                    reqwest::header::HeaderValue::from_str(&header.value)
                        .map_err(|_| error(ErrorKind::InvalidInput))?;
                }
            }
            upstream
        }
        SourceSettings::Volcano { upstream, policy } => {
            policy.validate()?;
            upstream
        }
    };
    require(
        upstream.channel_type > 0
            && upstream.models.len() <= 1000
            && upstream.models.iter().all(|m| !m.contains([',', '\0'])),
        ErrorKind::InvalidInput,
    )?;
    validate_url(&upstream.base_url, false)?;
    if let Some(discovery) = &upstream.model_discovery {
        validate_url(&discovery.url, false)?;
    }
    upstream.models = normalize_models_csv(&upstream.models.join(","));
    require(!upstream.models.is_empty(), ErrorKind::InvalidInput)
}
pub(super) fn fingerprint(key: &SecretString) -> String {
    let mut hash = Sha256::new();
    hash.update(b"quota-throttle/source-key/v1\0");
    hash.update(key.expose().as_bytes());
    format!("{:x}", hash.finalize())
}
pub(super) fn validate_source(mut draft: SourceDraft) -> Result<ValidatedSource> {
    draft.name = draft.name.trim().to_owned();
    require(
        valid_name(&draft.name) && draft.note.chars().count() <= 2048 && !draft.note.contains('\0'),
        ErrorKind::InvalidInput,
    )?;
    validate_settings(&mut draft.settings)?;
    Ok(ValidatedSource {
        fingerprint: fingerprint(&draft.credential),
        draft,
    })
}
pub(super) fn validate_group(mut draft: GroupDraft) -> Result<ValidatedGroup> {
    draft.name = draft.name.trim().to_owned();
    require(
        valid_name(&draft.name) && valid_group(&draft.newapi_group),
        ErrorKind::InvalidInput,
    )?;
    Ok(ValidatedGroup(draft))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSettings {
    version: i64,
    settings: SourceSettings,
}
pub(super) fn encode_settings(settings: &SourceSettings) -> Result<String> {
    serde_json::to_string(&StoredSettings {
        version: 1,
        settings: settings.clone(),
    })
    .map_err(|_| error(ErrorKind::Storage))
}
pub(super) fn decode_settings(text: &str) -> Result<SourceSettings> {
    // 复用旧配置类型，但私有存储不继承它们“忽略未知字段”的解析宽容。
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| error(ErrorKind::CorruptData))?;
    let mut stored: StoredSettings =
        serde_json::from_str(text).map_err(|_| error(ErrorKind::CorruptData))?;
    require(stored.version == 1, ErrorKind::CorruptData)?;
    validate_settings(&mut stored.settings).map_err(|_| error(ErrorKind::CorruptData))?;
    // 与原始对象比较，禁止静默丢弃未知字段、补默认鉴权或修补非法模型目录。
    let canonical = serde_json::to_value(&stored).map_err(|_| error(ErrorKind::CorruptData))?;
    require(canonical == value, ErrorKind::CorruptData)?;
    Ok(stored.settings)
}
