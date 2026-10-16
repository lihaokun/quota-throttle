use crate::catalog::CatalogError;
use crate::ledger::FactError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LedgerError {
    Catalog(CatalogError),
    Fact(FactError),
    RevisionConflict,
    InvalidCheckpoint,
    NotInitialized,
}
impl From<CatalogError> for LedgerError {
    fn from(e: CatalogError) -> Self {
        Self::Catalog(e)
    }
}
impl From<rusqlite::Error> for LedgerError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Catalog(e.into())
    }
}
impl From<FactError> for LedgerError {
    fn from(e: FactError) -> Self {
        Self::Fact(e)
    }
}
impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ledger: {self:?}")
    }
}
impl std::error::Error for LedgerError {}
pub(crate) type LedgerResult<T> = std::result::Result<T, LedgerError>;

macro_rules! stored_enum {
    ($name:ident {$($variant:ident => $value:literal),+ $(,)?}) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum $name { $($variant),+ }
        impl $name {
            pub(super) fn as_str(self) -> &'static str { match self { $(Self::$variant => $value),+ } }
            pub(super) fn parse(s: &str) -> LedgerResult<Self> {
                match s { $($value => Ok(Self::$variant)),+, _ => Err(super::corrupt()) }
            }
        }
    };
}
stored_enum!(ScanLane { Recent => "recent", History => "history" });
stored_enum!(ScanPhase { Idle => "idle", Scanning => "scanning", Scanned => "scanned", Stalled => "stalled" });
stored_enum!(ScanFault {
    Transport => "transport", InvalidPage => "invalid_page", InvalidFact => "invalid_fact",
    ConflictingIdentity => "conflicting_identity", BindingChanged => "binding_changed",
    HistoryUnavailable => "history_unavailable", ResourceLimit => "resource_limit", Clock => "clock"
});
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScanWindow {
    pub(crate) from: i64,
    pub(crate) to: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageEnd {
    More,
    ReachedEnd,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanProgress {
    pub(crate) lane: ScanLane,
    pub(crate) generation: i64,
    pub(crate) window: Option<ScanWindow>,
    pub(crate) next_page: i64,
    pub(crate) phase: ScanPhase,
    pub(crate) last_observed_at: Option<i64>,
    pub(crate) fault: Option<ScanFault>,
}
// 私有票据只能从本地持久状态产生，不是原生数据库绑定凭证。
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ScanTicket {
    pub(super) instance: String,
    pub(super) progress: ScanProgress,
}
impl ScanTicket {
    pub(crate) fn progress(&self) -> &ScanProgress {
        &self.progress
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerState {
    pub(crate) revision: i64,
    pub(crate) initial_requested_from: Option<i64>,
    pub(crate) scans: [ScanProgress; 2],
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerCommit {
    pub(crate) inserted: usize,
    pub(crate) duplicates: usize,
    pub(crate) changed_cells: usize,
    pub(crate) ledger_revision: i64,
    pub(crate) progress: ScanProgress,
}
