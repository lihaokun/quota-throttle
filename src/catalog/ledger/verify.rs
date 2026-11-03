use super::*;
use rows::{CELL_COLUMNS, DIM_ORDER, FACT_COLUMNS};

pub(crate) fn verify_ledger(conn: &Connection) -> std::result::Result<(), CatalogError> {
    verify(conn).map_err(|e| match e {
        LedgerError::Catalog(e) => e,
        _ => error(ErrorKind::CorruptData),
    })
}
fn verify(conn: &Connection) -> LedgerResult<()> {
    state(conn)?;
    let h = rows::history(conn)?;
    // 两个流都使用持久有序索引，不把历史或聚合单元装进全量map。
    let mut all = conn.prepare(&format!(
        "SELECT {FACT_COLUMNS} FROM ledger_facts ORDER BY native_log_id"
    ))?;
    let mut facts = all.query([])?;
    while let Some(r) = facts.next()? {
        rows::check_known(&rows::fact_row(r)?, &h)?;
    }
    drop(facts);
    let mut ordered=conn.prepare(&format!("SELECT {FACT_COLUMNS} FROM ledger_facts INDEXED BY ledger_fact_projection WHERE kind='consume' ORDER BY occurred_at-occurred_at%60,{DIM_ORDER},native_log_id"))?;
    let mut facts = ordered.query([])?;
    let mut stmt=conn.prepare(&format!("SELECT {CELL_COLUMNS} FROM ledger_minute_usage INDEXED BY ledger_cell_key ORDER BY minute,{DIM_ORDER}"))?;
    let mut cells = stmt.query([])?;
    let mut current: Option<(UsageCellKey, UsageTotals)> = None;
    while let Some(r) = facts.next()? {
        let a = rows::fact_row(r)?;
        let k = rows::key(&a);
        if current.as_ref().is_some_and(|(old, _)| old != &k) {
            compare_cell(current.take().unwrap(), cells.next()?)?;
        }
        let entry = current.get_or_insert_with(|| (k, UsageTotals::default()));
        entry.1 = add_usage(
            entry.1,
            UsageTotals {
                input: a.native.input,
                output: a.native.output,
                consumption_rows: 1,
            },
        )
        .map_err(|_| corrupt())?;
    }
    if let Some(last) = current {
        compare_cell(last, cells.next()?)?;
    }
    if cells.next()?.is_some() {
        return Err(corrupt());
    }
    Ok(())
}
fn compare_cell(
    expected: (UsageCellKey, UsageTotals),
    actual: Option<&rusqlite::Row<'_>>,
) -> LedgerResult<()> {
    let actual = rows::cell_row(actual.ok_or_else(corrupt)?)?;
    if actual != expected {
        return Err(corrupt());
    }
    Ok(())
}
