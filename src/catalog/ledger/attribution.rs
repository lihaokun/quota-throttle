//! 有界历史补归属；仅派生归属、分钟分项和账本 revision 可以改变。
use super::*;

pub(crate) const ATTRIBUTION_BATCH_ROWS: usize = 256;
const MAX_ID_SQL: &str = "SELECT max(native_log_id) FROM ledger_facts";
fn page_sql() -> String {
    format!("SELECT {} FROM ledger_facts WHERE native_log_id>?1 AND native_log_id<=?2 ORDER BY native_log_id LIMIT ?3", rows::FACT_COLUMNS)
}
#[cfg(test)]
pub(crate) fn attribution_test_sql() -> (&'static str, String) {
    (MAX_ID_SQL, page_sql())
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AttributionCursor {
    instance: String,
    after_id: i64,
    through_id: i64,
}
pub(crate) struct AttributionBatch {
    pub(crate) checked: usize,
    pub(crate) changed: usize,
    pub(crate) ledger_revision: i64,
    pub(crate) next: Option<AttributionCursor>,
}
impl CatalogStore {
    pub(crate) fn begin_attribution_pass(&self) -> LedgerResult<AttributionCursor> {
        self.usable()?;
        let tx = self.conn.unchecked_transaction()?;
        initialized(&tx)?;
        let through_id = tx
            .query_row(MAX_ID_SQL, [], |r| r.get::<_, Option<i64>>(0))?
            .unwrap_or(0);
        check(through_id >= 0)?;
        let cursor = AttributionCursor {
            instance: instance(&tx)?,
            after_id: 0,
            through_id,
        };
        tx.commit()?;
        Ok(cursor)
    }
    pub(crate) fn reattribute_batch(
        &mut self,
        cursor: &AttributionCursor,
    ) -> LedgerResult<AttributionBatch> {
        // Step B3.5: 冻结上界与主键分页；未知或已知行都推进本遍，不卡在首条未知项。
        self.usable()?;
        check(0 <= cursor.after_id && cursor.after_id <= cursor.through_id)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = initialized(&tx)?;
        if instance(&tx)? != cursor.instance {
            return Err(LedgerError::RevisionConflict);
        }
        let facts = {
            let mut stmt = tx.prepare(&page_sql())?;
            let mut rows = stmt.query(params![
                cursor.after_id,
                cursor.through_id,
                ATTRIBUTION_BATCH_ROWS as i64
            ])?;
            let mut facts = Vec::new();
            while let Some(row) = rows.next()? {
                facts.push(rows::fact_row(row)?);
            }
            facts
        }; // SELECT 在任何 UPDATE 前结束，避免同连接迭代修改未定义行为。
        let history = rows::history(&tx)?;
        for fact in &facts {
            rows::check_known(fact, &history)?;
        }
        let pending: Vec<_> = facts
            .iter()
            .filter(|f| f.source_id.is_none() || f.group_id.is_none())
            .collect();
        let native: Vec<_> = pending.iter().map(|f| f.native.clone()).collect();
        let attributed = attribute_batch(&native, &history.values().cloned().collect::<Vec<_>>())?;
        let mut changed = 0;
        for (old, new) in pending.into_iter().zip(attributed) {
            if old.native != new.native
                || old.source_id.is_some_and(|id| new.source_id != Some(id))
                || old.group_id.is_some_and(|id| new.group_id != Some(id))
            {
                return Err(corrupt());
            }
            if old.source_id == new.source_id
                && old.group_id == new.group_id
                && old.issue == new.issue
            {
                continue;
            }
            // Step B3.6: 撤旧、加新、只改派生三列；整批与 revision 同一提交点。
            if old.native.kind == FactKind::Consume {
                let delta = UsageTotals {
                    input: old.native.input,
                    output: old.native.output,
                    consumption_rows: 1,
                };
                rows::subtract_cell(&tx, &rows::key(old), delta)?;
                rows::add_cell(&tx, &rows::key(&new), delta)?;
            }
            let count=tx.execute("UPDATE ledger_facts SET source_id=?2,group_id=?3,attribution_issue=?4 WHERE native_log_id=?1",params![new.native.log_id,new.source_id.map(|v|v.get()),new.group_id.map(|v|v.get()),match new.issue {AttributionIssue::None=>"none",AttributionIssue::UnmappedChannel=>"unmapped_channel",AttributionIssue::GroupUnresolved=>"group_unresolved"}])?;
            if count != 1 {
                return Err(corrupt());
            }
            changed += 1;
        }
        let ledger_revision = if changed > 0 {
            bump(&tx, state.revision)?
        } else {
            state.revision
        };
        let next = facts
            .last()
            .filter(|f| f.native.log_id < cursor.through_id)
            .map(|f| AttributionCursor {
                instance: cursor.instance.clone(),
                after_id: f.native.log_id,
                through_id: cursor.through_id,
            });
        let result = AttributionBatch {
            checked: facts.len(),
            changed,
            ledger_revision,
            next,
        };
        commit(tx, &mut self.poisoned)?;
        Ok(result)
    }
}
