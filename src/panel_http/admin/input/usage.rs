use super::*;
use crate::catalog::ledger::ScanWindow;
use crate::ledger::analytics::{DimensionFilter, UsageFilter, UsageGrain};

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum Filter {
    Any {},
    Unknown {},
    Exact { value: String },
}
impl Default for Filter {
    fn default() -> Self {
        Self::Any {}
    }
}
impl Filter {
    fn map<T>(
        self,
        f: impl FnOnce(String) -> Result<T, AdminFailure>,
    ) -> Result<DimensionFilter<T>, AdminFailure> {
        Ok(match self {
            Self::Any {} => DimensionFilter::Any,
            Self::Unknown {} => DimensionFilter::Unknown,
            Self::Exact { value } => DimensionFilter::Exact(f(value)?),
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filters {
    #[serde(default, deserialize_with = "object")]
    source_id: Filter,
    #[serde(default, deserialize_with = "object")]
    group_id: Filter,
    #[serde(default, deserialize_with = "object")]
    native_token_id: Filter,
    #[serde(default, deserialize_with = "object")]
    model: Filter,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Grain {
    Hour,
    Day,
}
body!(Query {
    from: i64,
    to: i64,
    utc_offset_minutes: i64,
    #[serde(deserialize_with = "string_enum")]
    grain: Grain,
    #[serde(default, deserialize_with = "object")]
    filter: Filters
});
pub(super) fn decode(bytes: &[u8]) -> Result<UsageQuery, AdminFailure> {
    let d: Query = json(bytes)?;
    let filter = UsageFilter {
        source: d
            .filter
            .source_id
            .map(|v| Ok(SourceId::new(route::id(&v)?)?))?,
        group: d
            .filter
            .group_id
            .map(|v| Ok(GroupId::new(route::id(&v)?)?))?,
        token: d.filter.native_token_id.map(|v| route::id(&v))?,
        model: d.filter.model.map(Ok)?,
    };
    UsageQuery::new(
        ScanWindow {
            from: d.from,
            to: d.to,
        },
        d.utc_offset_minutes,
        match d.grain {
            Grain::Hour => UsageGrain::Hour,
            Grain::Day => UsageGrain::Day,
        },
        filter,
    )
    .map_err(Into::into)
}
