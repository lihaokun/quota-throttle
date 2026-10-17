use super::*;
use crate::ledger::analytics::{partition_window, QUERY_PAGE_ROWS};

pub(super) fn source_usage(
    tx: &Transaction<'_>,
    source: SourceId,
    window: ScanWindow,
    budget: &mut ProjectionBudget,
) -> ControlResult<UsageTotals> {
    // Step O4.3: 同事务原投影+秒级边缘；只读有界来源索引，不另存一份用量。
    let partition = partition_window(window).map_err(|_| failure(Code::InvalidInput))?;
    let mut total = UsageTotals::default();
    if let Some(w) = partition.whole {
        sum_window(tx, source, w, true, budget, &mut total)?;
    }
    for w in partition.edges {
        sum_window(tx, source, w, false, budget, &mut total)?;
    }
    Ok(total)
}
fn sum_window(
    tx: &Transaction<'_>,
    source: SourceId,
    window: ScanWindow,
    cells: bool,
    budget: &mut ProjectionBudget,
    total: &mut UsageTotals,
) -> ControlResult<()> {
    let (table, index, time, id, cols, extra) = if cells {
        (
            "ledger_minute_usage",
            "ledger_cell_source_time",
            "minute",
            "rowid",
            "input,output,consumption_rows,minute,rowid",
            "",
        )
    } else {
        (
            "ledger_facts",
            "ledger_fact_source_usage",
            "occurred_at",
            "native_log_id",
            "input,output,1,occurred_at,native_log_id",
            " AND kind='consume'",
        )
    };
    let sql=[format!("SELECT {cols} FROM {table} INDEXED BY {index} WHERE source_id=?1 AND {time}>?2 AND ?3 IS NOT NULL AND {time}<?4{extra} ORDER BY {time},{id} LIMIT ?5"),format!("SELECT {cols} FROM {table} INDEXED BY {index} WHERE source_id=?1 AND {time}=?2 AND {id}>?3 AND {time}<?4{extra} ORDER BY {time},{id} LIMIT ?5")];
    let mut cursor = (window.from - 1, i64::MAX);
    let mut same_time = false;
    loop {
        budget.check()?;
        let mut q = tx.prepare(&sql[usize::from(same_time)])?;
        let mut records = q.query(params![
            source.get(),
            cursor.0,
            cursor.1,
            window.to,
            QUERY_PAGE_ROWS as i64
        ])?;
        let mut count = 0;
        while let Some(r) = records.next()? {
            budget.row()?;
            *total = add_usage(
                *total,
                UsageTotals {
                    input: r.get(0)?,
                    output: r.get(1)?,
                    consumption_rows: r.get(2)?,
                },
            )
            .map_err(|_| failure(Code::Storage))?;
            cursor = (r.get(3)?, r.get(4)?);
            count += 1;
        }
        if !same_time && count < QUERY_PAGE_ROWS {
            return Ok(());
        }
        same_time = count == QUERY_PAGE_ROWS;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observed_usage_exact_edges_budgets_and_overflow_fail_closed() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ledger_facts(native_log_id INTEGER PRIMARY KEY,source_id INTEGER,occurred_at INTEGER,kind TEXT,input INTEGER,output INTEGER);CREATE INDEX ledger_fact_source_usage ON ledger_facts(source_id,occurred_at,native_log_id) WHERE kind='consume';CREATE TABLE ledger_minute_usage(source_id INTEGER,minute INTEGER,input INTEGER,output INTEGER,consumption_rows INTEGER);CREATE INDEX ledger_cell_source_time ON ledger_minute_usage(source_id,minute);INSERT INTO ledger_facts VALUES(1,1,59,'consume',1,2),(2,1,60,'consume',3,4),(3,1,119,'consume',5,6),(4,1,120,'consume',7,8),(5,2,119,'consume',100,100),(6,1,121,'consume',100,100);INSERT INTO ledger_minute_usage VALUES(1,60,8,10,2),(2,60,100,100,1);").unwrap();
        let tx = conn.transaction().unwrap();
        let source = SourceId::new(1).unwrap();
        let window = ScanWindow { from: 59, to: 121 };
        let mut budget = ProjectionBudget::new();
        let got = source_usage(&tx, source, window, &mut budget).unwrap();
        assert_eq!(
            got,
            UsageTotals {
                input: 16,
                output: 20,
                consumption_rows: 4
            }
        );
        assert_eq!(budget.scanned, 3);
        let mut budget = ProjectionBudget::new();
        budget.scanned = crate::ledger::analytics::MAX_QUERY_ROWS - 3;
        assert_eq!(source_usage(&tx, source, window, &mut budget).unwrap(), got);
        assert_eq!(budget.scanned, crate::ledger::analytics::MAX_QUERY_ROWS);
        assert_eq!(
            source_usage(&tx, source, window, &mut budget)
                .unwrap_err()
                .code,
            Code::NotReady
        );
        let mut budget = ProjectionBudget::new();
        budget.start -= Duration::from_secs(11);
        assert_eq!(
            source_usage(&tx, source, window, &mut budget)
                .unwrap_err()
                .code,
            Code::NotReady
        );
        tx.execute(
            "UPDATE ledger_minute_usage SET input=?1 WHERE source_id=1",
            [JSON_SAFE],
        )
        .unwrap();
        assert_eq!(
            source_usage(&tx, source, window, &mut ProjectionBudget::new())
                .unwrap_err()
                .code,
            Code::Storage
        );
        tx.execute_batch("DELETE FROM ledger_facts;WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<2000) INSERT INTO ledger_facts SELECT i,1,59,'consume',1,2 FROM n;").unwrap();
        let mut budget = ProjectionBudget::new();
        assert_eq!(
            source_usage(&tx, source, ScanWindow { from: 59, to: 60 }, &mut budget).unwrap(),
            UsageTotals {
                input: 2000,
                output: 4000,
                consumption_rows: 2000
            }
        );
        assert_eq!(budget.scanned, 2000);
    }
}
