use super::*;
use crate::catalog::ledger::{LedgerState, ScanProgress};
use crate::catalog::{GroupId, SourceId};
use crate::ledger::analytics::{DimensionFilter, UsageGrain, UsageQuery, UsageReport};
use crate::ledger::{add_usage, UsageTotals};

#[derive(Serialize)]
#[serde(tag = "mode", content = "value", rename_all = "snake_case")]
enum Filter<T> {
    Any,
    Unknown,
    Exact(T),
}
fn filter<'a, T, U>(value: &'a DimensionFilter<T>, exact: impl FnOnce(&'a T) -> U) -> Filter<U> {
    match value {
        DimensionFilter::Any => Filter::Any,
        DimensionFilter::Unknown => Filter::Unknown,
        DimensionFilter::Exact(v) => Filter::Exact(exact(v)),
    }
}
#[derive(Serialize)]
struct Filters<'a> {
    source_id: Filter<SourceId>,
    group_id: Filter<GroupId>,
    native_token_id: Filter<String>,
    model: Filter<&'a str>,
}
#[derive(Serialize)]
struct Query<'a> {
    from: i64,
    to: i64,
    utc_offset_minutes: i64,
    grain: &'static str,
    filter: Filters<'a>,
}
fn query(q: &UsageQuery) -> Query<'_> {
    let f = q.filter();
    Query {
        from: q.window().from,
        to: q.window().to,
        utc_offset_minutes: q.utc_offset_minutes(),
        grain: match q.grain() {
            UsageGrain::Hour => "hour",
            UsageGrain::Day => "day",
        },
        filter: Filters {
            source_id: filter(&f.source, |v| *v),
            group_id: filter(&f.group, |v| *v),
            native_token_id: filter(&f.token, i64::to_string),
            model: filter(&f.model, String::as_str),
        },
    }
}
#[derive(Serialize)]
struct Window {
    from: i64,
    to: i64,
}
#[derive(Serialize)]
struct Progress {
    lane: &'static str,
    generation: i64,
    window: Option<Window>,
    next_page: i64,
    phase: &'static str,
    last_observed_at: Option<i64>,
    fault: Option<&'static str>,
}
fn progress(p: &ScanProgress) -> Progress {
    Progress {
        lane: p.lane.as_str(),
        generation: p.generation,
        window: p.window.map(|w| Window {
            from: w.from,
            to: w.to,
        }),
        next_page: p.next_page,
        phase: p.phase.as_str(),
        last_observed_at: p.last_observed_at,
        fault: p.fault.map(|f| f.as_str()),
    }
}
#[derive(Serialize)]
struct Ledger {
    revision: i64,
    initial_requested_from: Option<i64>,
    scans: [Progress; 2],
}
fn ledger(s: &LedgerState) -> Ledger {
    Ledger {
        revision: s.revision,
        initial_requested_from: s.initial_requested_from,
        scans: [progress(&s.scans[0]), progress(&s.scans[1])],
    }
}
#[derive(Serialize)]
struct Totals {
    input: i64,
    output: i64,
    total: i64,
    consumption_rows: i64,
}
fn totals(t: UsageTotals) -> Result<Totals, AdminError> {
    let t = add_usage(t, UsageTotals::default())
        .map_err(|_| AdminError::extra(ExtraCode::ArithmeticOverflow))?;
    Ok(Totals {
        input: t.input,
        output: t.output,
        total: t.input + t.output,
        consumption_rows: t.consumption_rows,
    })
}
#[derive(Serialize)]
struct Model<'a> {
    model: Option<&'a str>,
    usage: Totals,
}
fn models(rows: &[(Option<String>, UsageTotals)]) -> Result<Vec<Model<'_>>, AdminError> {
    rows.iter()
        .map(|(model, t)| {
            Ok(Model {
                model: model.as_deref(),
                usage: totals(*t)?,
            })
        })
        .collect()
}
#[derive(Serialize)]
struct Token {
    native_token_id: Option<String>,
    usage: Totals,
}
#[derive(Serialize)]
struct Bucket<'a> {
    from: i64,
    to: i64,
    total: Totals,
    models: Vec<Model<'a>>,
}
#[derive(Serialize)]
struct Usage<'a> {
    total: Totals,
    models: Vec<Model<'a>>,
    tokens: Vec<Token>,
    buckets: Vec<Bucket<'a>>,
}
fn usage(r: &UsageReport) -> Result<Usage<'_>, AdminError> {
    Ok(Usage {
        total: totals(r.total)?,
        models: models(&r.models)?,
        tokens: r
            .tokens
            .iter()
            .map(|(id, t)| {
                Ok(Token {
                    native_token_id: id.map(|v| v.to_string()),
                    usage: totals(*t)?,
                })
            })
            .collect::<Result<_, AdminError>>()?,
        buckets: r
            .buckets
            .iter()
            .map(|b| {
                Ok(Bucket {
                    from: b.from,
                    to: b.to,
                    total: totals(b.total)?,
                    models: models(&b.models)?,
                })
            })
            .collect::<Result<_, AdminError>>()?,
    })
}
#[derive(Serialize)]
pub(super) struct UsageWire<'a> {
    schema_version: u8,
    query: Query<'a>,
    ledger: Ledger,
    usage: Option<Usage<'a>>,
}
pub(super) fn project(snapshot: &UsageSnapshot) -> Result<UsageWire<'_>, AdminError> {
    // Step H2.6: 同一UsageSnapshot映射全部联合分项；借用模型字符串，未知保持null。
    Ok(UsageWire {
        schema_version: 1,
        query: query(&snapshot.query),
        ledger: ledger(&snapshot.state),
        usage: snapshot.usage.as_ref().map(usage).transpose()?,
    })
}
