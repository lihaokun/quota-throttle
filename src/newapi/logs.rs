//! 全局管理员日志页；不复用旧面板的宽松 recent_logs 解码。
#![allow(dead_code)] // B2 的显式入口在 SGM-7 才接入运行服务。
use super::transport::{ManagementRequest, RequestClass, RequestSemantics, LIST_BODY_LIMIT};
use super::NewApiClient;
use crate::catalog::ledger::{PageEnd, ScanWindow};
use crate::catalog::JSON_SAFE;
use crate::ledger::collection::{CollectionError, CollectionResult, PAGE_SIZE};
use reqwest::Method;
use serde_json::Value;
use std::collections::BTreeSet;

mod capability;
pub(crate) use capability::LocalLogLease;

#[derive(Clone)]
pub(crate) struct LogPageQuery {
    window: ScanWindow,
    page: i64,
}
impl LogPageQuery {
    #[cfg(test)]
    pub(crate) fn test_parts(&self) -> (ScanWindow, i64) {
        (self.window, self.page)
    }
    pub(crate) fn new(window: ScanWindow, page: i64) -> CollectionResult<Self> {
        if window.from < 0
            || window.to < 2
            || window.from >= window.to
            || window.to > JSON_SAFE
            || !(1..=JSON_SAFE).contains(&page)
            || (page - 1).checked_mul(PAGE_SIZE as i64).is_none()
        {
            return Err(CollectionError::InvalidPage);
        }
        Ok(Self { window, page })
    }
    fn path(&self) -> String {
        format!(
            "/api/log/?type=0&p={}&page_size={PAGE_SIZE}&start_timestamp={}&end_timestamp={}",
            self.page,
            self.window.from,
            self.window.to - 1
        )
    }
    pub(crate) fn into_request(self) -> CollectionResult<ManagementRequest> {
        Ok(ManagementRequest::new(
            Method::GET,
            &self.path(),
            None,
            RequestSemantics::Read,
            LIST_BODY_LIMIT,
        )?)
    }
}
pub(crate) struct CheckedLogPage {
    pub(crate) rows: Vec<Value>,
    pub(crate) raw_count: usize,
    pub(crate) total: i64,
    pub(crate) first_key: Option<(i64, i64)>,
    pub(crate) last_key: Option<(i64, i64)>,
    pub(crate) end: PageEnd,
}
#[derive(Clone, Copy)]
pub(crate) struct PageObservation {
    pub(crate) total: i64,
    pub(crate) last_key: Option<(i64, i64)>,
}
impl CheckedLogPage {
    pub(crate) fn observation(&self) -> PageObservation {
        PageObservation {
            total: self.total,
            last_key: self.last_key,
        }
    }
}
pub(crate) fn decode_log_page(
    mut body: Value,
    query: &LogPageQuery,
) -> CollectionResult<CheckedLogPage> {
    // Step B2.1: 全原始页校验；结束由原始条数决定，total 仅作变化提示。
    let bad = || CollectionError::InvalidPage;
    if body.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(bad());
    }
    let data = body
        .get_mut("data")
        .and_then(Value::as_object_mut)
        .ok_or_else(bad)?;
    if data.get("page").and_then(Value::as_i64) != Some(query.page)
        || data.get("page_size").and_then(Value::as_u64) != Some(PAGE_SIZE as u64)
    {
        return Err(bad());
    }
    let total = data
        .get("total")
        .and_then(Value::as_i64)
        .filter(|n| (0..=JSON_SAFE).contains(n))
        .ok_or_else(bad)?;
    let rows = match data.remove("items") {
        Some(Value::Array(rows)) if rows.len() <= PAGE_SIZE => rows,
        _ => return Err(bad()),
    };
    let mut ids = BTreeSet::new();
    let mut first = None;
    let mut last = None;
    for row in &rows {
        let id = row
            .get("id")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0)
            .ok_or_else(bad)?;
        let at = row
            .get("created_at")
            .and_then(Value::as_i64)
            .filter(|n| *n >= query.window.from && *n < query.window.to)
            .ok_or_else(bad)?;
        if row
            .get("type")
            .and_then(Value::as_i64)
            .is_none_or(|n| n < 0)
            || !ids.insert(id)
            || last.is_some_and(|prev| prev <= (at, id))
        {
            return Err(bad());
        }
        first.get_or_insert((at, id));
        last = Some((at, id));
    }
    Ok(CheckedLogPage {
        raw_count: rows.len(),
        end: if rows.len() == PAGE_SIZE {
            PageEnd::More
        } else {
            PageEnd::ReachedEnd
        },
        rows,
        total,
        first_key: first,
        last_key: last,
    })
}
pub(crate) fn observe_page_change(
    previous: Option<PageObservation>,
    page: &CheckedLogPage,
) -> bool {
    previous.is_some_and(|p| {
        p.total != page.total
            || p.last_key
                .zip(page.first_key)
                .is_some_and(|(last, first)| first >= last)
    })
}

impl NewApiClient {
    pub(crate) async fn read_log_page(
        &self,
        query: LogPageQuery,
        lease: &LocalLogLease,
        class: RequestClass,
        guard: &(dyn Fn() -> bool + Send + Sync),
    ) -> CollectionResult<CheckedLogPage> {
        // Step B2.5: 只读共享预算；401 后的最终准入仍核证据期限与取消。
        if !matches!(class, RequestClass::Observe | RequestClass::Backfill) {
            return Err(CollectionError::InvalidPage);
        }
        lease.check_time()?;
        let request = query.clone().into_request()?;
        let check = || lease.check_time().is_ok() && guard();
        let response = self.transport.send_fenced(request, class, &check).await?;
        lease.check_time()?;
        decode_log_page(response.envelope()?, &query)
    }
}

#[cfg(test)]
mod tests;
