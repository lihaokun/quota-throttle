//! 独立来源目录：只有显式本地调用，不接管现有 CLI、调度或 NewAPI。

pub mod control;
pub mod execution;
mod legacy;
mod store;
pub mod types;

pub use legacy::{build_legacy_import, LegacyChannel, LegacyImportPlan};
pub use store::CatalogStore;
pub use types::*;
