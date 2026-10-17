//! 统一事实解析及消费投影纯函数；尚未接采集、持久化或调度。
#![allow(dead_code)] // B/C 接线之前，不用假生产调用消除未使用警告。
pub(crate) mod analytics;
pub(crate) mod collection;
mod decode;
pub(crate) mod observations;
pub(crate) mod storage_codec;
mod types;

use crate::catalog::{ChannelId, JSON_SAFE};
pub(crate) use decode::decode_native_row;
pub(crate) use decode::valid_label;
use std::collections::{BTreeMap, BTreeSet};
pub(crate) use types::*;

fn ensure(ok: bool, error: FactError) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(error)
    }
}
fn bounded(len: usize, max: usize) -> Result<()> {
    ensure(len <= max, FactError::LimitExceeded)
}
fn valid_totals(t: UsageTotals) -> bool {
    [t.input, t.output, t.consumption_rows]
        .into_iter()
        .all(|n| (0..=JSON_SAFE).contains(&n))
        && t.input
            .checked_add(t.output)
            .is_some_and(|n| n <= JSON_SAFE)
}

pub(crate) fn decode_native_batch(rows: &[serde_json::Value]) -> Result<Vec<NativeFact>> {
    // Step A2: 完整批内规范去重；身份冲突不得覆盖，排序不是终态游标。
    bounded(rows.len(), MAX_FACTS)?;
    let mut facts: BTreeMap<i64, NativeFact> = BTreeMap::new();
    for row in rows {
        if let Some(fact) = decode_native_row(row)? {
            if let Some(previous) = facts.get(&fact.log_id) {
                ensure(previous == &fact, FactError::ConflictingIdentity)?;
            } else {
                facts.insert(fact.log_id, fact);
            }
        }
    }
    Ok(facts.into_values().collect())
}

pub(crate) fn attribute_batch(
    facts: &[NativeFact],
    history: &[ChannelAttribution],
) -> Result<Vec<AttributedFact>> {
    // Step A3: 全映射先验证，不因没有对应消费而跳过坏/重复历史。
    bounded(facts.len(), MAX_FACTS)?;
    bounded(history.len(), MAX_HISTORY)?;
    let mut mapping = BTreeMap::new();
    for row in history {
        ensure(
            row.channel_id.get() > 0
                && row.source_id.get() > 0
                && row.group_id.get() > 0
                && decode::valid_label(&row.native_group)
                && mapping.insert(row.channel_id, row).is_none(),
            FactError::InvalidAttribution,
        )?;
    }
    let mut ids = BTreeSet::new();
    let mut attributed = Vec::with_capacity(facts.len());
    for fact in facts {
        decode::validate_fact(fact)?;
        ensure(ids.insert(fact.log_id), FactError::ConflictingIdentity)?;
        let (source_id, group_id, issue) = match fact.channel_id.and_then(|id| mapping.get(&id)) {
            None => (None, None, AttributionIssue::UnmappedChannel),
            Some(row) if fact.native_group.as_deref() == Some(row.native_group.as_str()) => (
                Some(row.source_id),
                Some(row.group_id),
                AttributionIssue::None,
            ),
            Some(row) => (Some(row.source_id), None, AttributionIssue::GroupUnresolved),
        };
        attributed.push(AttributedFact {
            native: fact.clone(),
            source_id,
            group_id,
            issue,
        });
    }
    Ok(attributed)
}

pub(crate) fn validate_attribution(f: &AttributedFact) -> Result<()> {
    decode::validate_fact(&f.native)?;
    let valid = match f.issue {
        AttributionIssue::None => {
            f.source_id.is_some() && f.group_id.is_some() && f.native.native_group.is_some()
        }
        AttributionIssue::UnmappedChannel => f.source_id.is_none() && f.group_id.is_none(),
        AttributionIssue::GroupUnresolved => f.source_id.is_some() && f.group_id.is_none(),
    };
    ensure(
        valid && (f.source_id.is_none() || f.native.channel_id.is_some()),
        FactError::InvalidAttribution,
    )
}

pub(crate) fn usage_delta(facts: &[AttributedFact]) -> Result<UsageDelta> {
    // Step A4: 每条新增消费进入唯一联合单元；错误/取消不能变成独立失败计数。
    bounded(facts.len(), MAX_FACTS)?;
    let mut ids = BTreeSet::new();
    for fact in facts {
        validate_attribution(fact)?;
        ensure(
            ids.insert(fact.native.log_id),
            FactError::ConflictingIdentity,
        )?;
    }
    let mut delta = UsageDelta::default();
    for fact in facts {
        let f = &fact.native;
        if f.kind != FactKind::Consume {
            continue;
        }
        let key = UsageCellKey {
            minute: f.occurred_at - f.occurred_at % 60,
            channel_id: f.channel_id,
            source_id: fact.source_id,
            group_id: fact.group_id,
            native_group: f.native_group.clone(),
            token_id: f.token_id,
            model: f.model.clone(),
        };
        let contribution = UsageTotals {
            input: f.input,
            output: f.output,
            consumption_rows: 1,
        };
        let previous = delta.cells.get(&key).copied().unwrap_or_default();
        delta.cells.insert(key, add_usage(previous, contribution)?);
        delta.total = add_usage(delta.total, contribution)?;
    }
    Ok(delta)
}

pub(crate) fn add_usage(left: UsageTotals, right: UsageTotals) -> Result<UsageTotals> {
    // Step A5: 输入、逐字段加法与合计都精确校验，不饱和/不部分更新。
    ensure(
        valid_totals(left) && valid_totals(right),
        FactError::ArithmeticOverflow,
    )?;
    let checked = |a: i64, b: i64| a.checked_add(b).ok_or(FactError::ArithmeticOverflow);
    let result = UsageTotals {
        input: checked(left.input, right.input)?,
        output: checked(left.output, right.output)?,
        consumption_rows: checked(left.consumption_rows, right.consumption_rows)?,
    };
    ensure(valid_totals(result), FactError::ArithmeticOverflow)?;
    Ok(result)
}

#[cfg(test)]
mod tests;
