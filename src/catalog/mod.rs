//! 独立来源目录：只有显式本地调用，不接管现有 CLI、调度或 NewAPI。

pub mod confirmation;
pub mod control;
pub mod execution;
pub(crate) mod glm_publication;
pub(crate) mod glm_round;
mod legacy;
pub(crate) mod policy;
mod store;
pub mod types;

pub use legacy::{build_legacy_import, LegacyChannel, LegacyImportPlan};
pub use store::CatalogStore;
pub use types::*;
