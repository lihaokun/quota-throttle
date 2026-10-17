//! 有界查询计算和取消上下文；不打开数据库或启动网络、任务与业务策略。
use super::{add_usage, valid_label, UsageTotals};
use crate::catalog::control::ControlErrorCode;
use crate::catalog::ledger::{LedgerError, LedgerState, ScanWindow};
use crate::catalog::{GroupId, SourceId, JSON_SAFE};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod query;
pub(crate) use query::*;

pub(crate) const MAX_QUERY_SECONDS: i64 = 31 * 86400;
pub(crate) const MAX_MODELS: usize = 1024;
pub(crate) const MAX_TOKENS: usize = 4096;
pub(crate) const MAX_BUCKET_MODELS: usize = 65536;
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const QUERY_PAGE_ROWS: usize = 256;
pub(crate) const MAX_QUERY_ROWS: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnalyticsError {
    Ledger(LedgerError),
    Control(ControlErrorCode),
    InvalidQuery,
    LimitExceeded,
    ArithmeticOverflow,
    Cancelled,
    TimedOut,
    Busy,
}
pub(crate) type AnalyticsResult<T> = std::result::Result<T, AnalyticsError>;
impl From<LedgerError> for AnalyticsError {
    fn from(e: LedgerError) -> Self {
        if matches!(&e, LedgerError::Catalog(c) if c.kind == crate::catalog::ErrorKind::Busy) {
            Self::Busy
        } else {
            Self::Ledger(e)
        }
    }
}
impl From<crate::catalog::CatalogError> for AnalyticsError {
    fn from(e: crate::catalog::CatalogError) -> Self {
        LedgerError::Catalog(e).into()
    }
}
impl From<rusqlite::Error> for AnalyticsError {
    fn from(e: rusqlite::Error) -> Self {
        LedgerError::from(e).into()
    }
}

pub(crate) struct QueryControl {
    cancelled: AtomicBool,
    deadline: Instant,
    interrupt: Mutex<Option<Arc<rusqlite::InterruptHandle>>>,
}
impl QueryControl {
    pub(crate) fn new(deadline: Instant) -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            deadline,
            interrupt: Mutex::new(None),
        }
    }
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let handle = self
            .interrupt
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(handle) = handle {
            handle.interrupt();
        }
    }
    pub(crate) fn register_interrupt(
        &self,
        handle: rusqlite::InterruptHandle,
    ) -> AnalyticsResult<()> {
        let handle = Arc::new(handle);
        *self
            .interrupt
            .lock()
            .map_err(|_| AnalyticsError::Control(ControlErrorCode::Storage))? =
            Some(handle.clone());
        // 取消可能发生在注册之前；此时 interrupt 可能是 no-op，标志仍阻止下一条 SQL。
        let result = self.check();
        if result.is_err() {
            handle.interrupt();
        }
        result
    }
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(crate) fn check(&self) -> AnalyticsResult<()> {
        self.check_at(Instant::now())
    }
    fn check_at(&self, now: Instant) -> AnalyticsResult<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(AnalyticsError::Cancelled)
        } else if now >= self.deadline {
            Err(AnalyticsError::TimedOut)
        } else {
            Ok(())
        }
    }
}

#[derive(PartialEq, Eq)]
pub(crate) struct UsageSnapshot {
    pub(crate) query: UsageQuery,
    pub(crate) state: LedgerState,
    pub(crate) usage: Option<UsageReport>,
}
#[derive(PartialEq, Eq)]
pub(crate) struct UsageReport {
    pub(crate) total: UsageTotals,
    pub(crate) models: Vec<(Option<String>, UsageTotals)>,
    pub(crate) tokens: Vec<(Option<i64>, UsageTotals)>,
    pub(crate) buckets: Vec<UsageBucket>,
}
#[derive(PartialEq, Eq)]
pub(crate) struct UsageBucket {
    pub(crate) from: i64,
    pub(crate) to: i64,
    pub(crate) total: UsageTotals,
    pub(crate) models: Vec<(Option<String>, UsageTotals)>,
}
// 由解码后的事实/分钟单元短暂借用，调用方只传消费；不复制原始元数据。
#[derive(Clone, Copy)]
pub(crate) struct UsageDimensions<'a> {
    pub(crate) source: Option<SourceId>,
    pub(crate) group: Option<GroupId>,
    pub(crate) token: Option<i64>,
    pub(crate) model: Option<&'a str>,
}
#[derive(Default)]
struct BucketWork {
    total: UsageTotals,
    models: BTreeMap<Option<String>, UsageTotals>,
}
struct Working {
    total: UsageTotals,
    models: BTreeMap<Option<String>, UsageTotals>,
    tokens: BTreeMap<Option<i64>, UsageTotals>,
    buckets: Vec<BucketWork>,
    ranges: Vec<ScanWindow>,
    bucket_models: usize,
}
pub(crate) struct UsageAccumulator {
    query: UsageQuery,
    work: AnalyticsResult<Working>,
}
fn sum(a: UsageTotals, b: UsageTotals) -> AnalyticsResult<UsageTotals> {
    add_usage(a, b).map_err(|_| AnalyticsError::ArithmeticOverflow)
}
fn capacity(new: bool, len: usize, limit: usize) -> AnalyticsResult<()> {
    if new && len >= limit {
        Err(AnalyticsError::LimitExceeded)
    } else {
        Ok(())
    }
}

impl UsageAccumulator {
    pub(crate) fn new(query: UsageQuery, control: &QueryControl) -> AnalyticsResult<Self> {
        control.check()?;
        let ranges = query.bucket_ranges();
        let mut buckets = Vec::new();
        for _ in &ranges {
            control.check()?;
            buckets.push(BucketWork::default());
        }
        Ok(Self {
            query,
            work: Ok(Working {
                total: UsageTotals::default(),
                models: BTreeMap::new(),
                tokens: BTreeMap::new(),
                buckets,
                ranges,
                bucket_models: 0,
            }),
        })
    }
    pub(crate) fn push(
        &mut self,
        dims: UsageDimensions<'_>,
        at: i64,
        totals: UsageTotals,
        control: &QueryControl,
    ) -> AnalyticsResult<()> {
        // Step B3.2: 容量和精确加法先验；错误销毁工作集，禁止随后发布部分结果。
        let result = match &mut self.work {
            Ok(work) => Self::push_inner(&self.query, work, dims, at, totals, control),
            Err(e) => return Err(e.clone()),
        };
        if let Err(e) = &result {
            self.work = Err(e.clone());
        }
        result
    }
    fn push_inner(
        query: &UsageQuery,
        work: &mut Working,
        dims: UsageDimensions<'_>,
        at: i64,
        totals: UsageTotals,
        control: &QueryControl,
    ) -> AnalyticsResult<()> {
        control.check()?;
        let index = query.bucket_index(at)?;
        if dims.token.is_some_and(|n| n <= 0)
            || dims.model.is_some_and(|s| !valid_label(s))
            || (dims.group.is_some() && dims.source.is_none())
            || totals.consumption_rows <= 0
        {
            return Err(AnalyticsError::InvalidQuery);
        }
        sum(UsageTotals::default(), totals)?;
        if !query.matches(dims) {
            return Ok(());
        }
        // 标签已按 A 验证上界；只在这一有界入口复制，不打印标签。
        let model = dims.model.map(str::to_owned);
        let bucket = &work.buckets[index];
        capacity(
            !work.models.contains_key(&model),
            work.models.len(),
            MAX_MODELS,
        )?;
        capacity(
            !work.tokens.contains_key(&dims.token),
            work.tokens.len(),
            MAX_TOKENS,
        )?;
        let new_bucket_model = !bucket.models.contains_key(&model);
        capacity(new_bucket_model, work.bucket_models, MAX_BUCKET_MODELS)?;
        let total = sum(work.total, totals)?;
        let model_total = sum(work.models.get(&model).copied().unwrap_or_default(), totals)?;
        let token_total = sum(
            work.tokens.get(&dims.token).copied().unwrap_or_default(),
            totals,
        )?;
        let bucket_total = sum(bucket.total, totals)?;
        let bucket_model = sum(
            bucket.models.get(&model).copied().unwrap_or_default(),
            totals,
        )?;
        control.check()?;
        work.total = total;
        work.models.insert(model.clone(), model_total);
        work.tokens.insert(dims.token, token_total);
        let bucket = &mut work.buckets[index];
        bucket.total = bucket_total;
        bucket.models.insert(model, bucket_model);
        work.bucket_models += usize::from(new_bucket_model);
        Ok(())
    }
    pub(crate) fn finish(
        self,
        state: LedgerState,
        control: &QueryControl,
    ) -> AnalyticsResult<UsageSnapshot> {
        let work = self.work?;
        control.check()?;
        if state.initial_requested_from.is_none() {
            return Err(AnalyticsError::Ledger(LedgerError::NotInitialized));
        }
        let models = finish_parts(work.models, work.total, control)?;
        let tokens = finish_parts(work.tokens, work.total, control)?;
        let mut buckets = Vec::new();
        let mut bucket_sum = UsageTotals::default();
        for (bucket, range) in work.buckets.into_iter().zip(work.ranges) {
            control.check()?;
            bucket_sum = sum(bucket_sum, bucket.total)?;
            buckets.push(UsageBucket {
                from: range.from,
                to: range.to,
                total: bucket.total,
                models: finish_parts(bucket.models, bucket.total, control)?,
            });
        }
        if bucket_sum != work.total {
            return Err(AnalyticsError::ArithmeticOverflow);
        }
        control.check()?;
        Ok(UsageSnapshot {
            query: self.query,
            state,
            usage: Some(UsageReport {
                total: work.total,
                models,
                tokens,
                buckets,
            }),
        })
    }
}
fn finish_parts<K: Ord>(
    map: BTreeMap<K, UsageTotals>,
    expected: UsageTotals,
    control: &QueryControl,
) -> AnalyticsResult<Vec<(K, UsageTotals)>> {
    let mut total = UsageTotals::default();
    let mut rows = Vec::new();
    for (key, value) in map {
        control.check()?;
        total = sum(total, value)?;
        rows.push((key, value));
    }
    if total != expected {
        return Err(AnalyticsError::ArithmeticOverflow);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
