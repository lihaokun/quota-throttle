use super::*;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::BTreeMap;

pub(super) const FACT_COLUMNS: &str = "native_log_id,request_id,kind,occurred_at,channel_id,token_id,model,native_group,input,output,is_stream,metadata_json,source_id,group_id,attribution_issue";
pub(super) const CELL_COLUMNS: &str = "minute,channel_id,source_id,group_id,native_group,token_id,model,input,output,consumption_rows";
pub(super) const CELL_MATCH: &str = "minute=?1 AND coalesce(channel_id,0)=coalesce(?2,0) AND coalesce(source_id,0)=coalesce(?3,0) AND coalesce(group_id,0)=coalesce(?4,0) AND coalesce(native_group,'')=coalesce(?5,'') AND coalesce(token_id,0)=coalesce(?6,0) AND coalesce(model,'')=coalesce(?7,'')";
pub(super) const DIM_ORDER: &str = "coalesce(channel_id,0),coalesce(source_id,0),coalesce(group_id,0),coalesce(native_group,''),coalesce(token_id,0),coalesce(model,'')";
pub(super) type History = BTreeMap<ChannelId, ChannelAttribution>;

pub(super) fn history(conn: &Connection) -> LedgerResult<History> {
    let mut stmt=conn.prepare("SELECT m.channel_id,m.source_id,m.group_id,g.newapi_group FROM memberships m JOIN source_groups g USING(group_id) WHERE m.channel_id IS NOT NULL ORDER BY m.channel_id LIMIT ?1")?;
    let mut rows = stmt.query([MAX_HISTORY as i64 + 1])?;
    let mut all = Vec::new();
    while let Some(r) = rows.next()? {
        if all.len() == MAX_HISTORY {
            return Err(FactError::LimitExceeded.into());
        }
        all.push(ChannelAttribution {
            channel_id: ChannelId::new(r.get(0)?).map_err(|_| corrupt())?,
            source_id: SourceId::new(r.get(1)?).map_err(|_| corrupt())?,
            group_id: GroupId::new(r.get(2)?).map_err(|_| corrupt())?,
            native_group: r.get(3)?,
        });
    }
    attribute_batch(&[], &all).map_err(|e| match e {
        FactError::LimitExceeded => LedgerError::Fact(e),
        _ => corrupt(),
    })?; // 已存历史损坏不伪装为本次输入错误。
    Ok(all.into_iter().map(|h| (h.channel_id, h)).collect())
}
pub(super) fn check_known(a: &AttributedFact, h: &History) -> LedgerResult<()> {
    validate_attribution(a).map_err(|_| corrupt())?;
    if let Some(source) = a.source_id {
        let row = a
            .native
            .channel_id
            .and_then(|id| h.get(&id))
            .ok_or_else(corrupt)?;
        if row.source_id != source
            || a.group_id.is_some_and(|g| {
                g != row.group_id
                    || a.native.native_group.as_deref() != Some(row.native_group.as_str())
            })
        {
            return Err(corrupt());
        }
    }
    // 旧未知归属允许等待B3，不能在open时自动重投影。
    Ok(())
}
pub(super) fn fact_row(r: &Row<'_>) -> LedgerResult<AttributedFact> {
    let channel: Option<i64> = r.get(4)?;
    let source: Option<i64> = r.get(12)?;
    let group: Option<i64> = r.get(13)?;
    let stream: i64 = r.get(10)?;
    if ![0, 1].contains(&stream) {
        return Err(corrupt());
    }
    let a = AttributedFact {
        native: NativeFact {
            log_id: r.get(0)?,
            request_id: r.get(1)?,
            kind: match r.get::<_, String>(2)?.as_str() {
                "consume" => FactKind::Consume,
                "error" => FactKind::Error,
                _ => return Err(corrupt()),
            },
            occurred_at: r.get(3)?,
            channel_id: channel
                .map(ChannelId::new)
                .transpose()
                .map_err(|_| corrupt())?,
            token_id: r.get(5)?,
            model: r.get(6)?,
            native_group: r.get(7)?,
            input: r.get(8)?,
            output: r.get(9)?,
            is_stream: stream == 1,
            metadata: crate::ledger::storage_codec::decode(&r.get::<_, String>(11)?)
                .map_err(|_| corrupt())?,
        },
        source_id: source
            .map(SourceId::new)
            .transpose()
            .map_err(|_| corrupt())?,
        group_id: group.map(GroupId::new).transpose().map_err(|_| corrupt())?,
        issue: match r.get::<_, String>(14)?.as_str() {
            "none" => AttributionIssue::None,
            "unmapped_channel" => AttributionIssue::UnmappedChannel,
            "group_unresolved" => AttributionIssue::GroupUnresolved,
            _ => return Err(corrupt()),
        },
    };
    validate_attribution(&a).map_err(|_| corrupt())?;
    Ok(a)
}
pub(super) fn read_fact(
    conn: &Connection,
    id: i64,
    h: &History,
) -> LedgerResult<Option<AttributedFact>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FACT_COLUMNS} FROM ledger_facts WHERE native_log_id=?1"
    ))?;
    let mut rows = stmt.query([id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let a = fact_row(row)?;
    check_known(&a, h)?;
    Ok(Some(a))
}
pub(super) fn insert_fact(conn: &Connection, a: &AttributedFact) -> LedgerResult<()> {
    let f = &a.native;
    let encoded = crate::ledger::storage_codec::encode(&f.metadata)?;
    conn.prepare(&format!("INSERT INTO ledger_facts({FACT_COLUMNS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)"))?.execute(params![f.log_id,f.request_id,match f.kind {FactKind::Consume=>"consume",FactKind::Error=>"error"},f.occurred_at,f.channel_id.map(|n|n.get()),f.token_id,f.model,f.native_group,f.input,f.output,i64::from(f.is_stream),encoded,a.source_id.map(|n|n.get()),a.group_id.map(|n|n.get()),match a.issue {AttributionIssue::None=>"none",AttributionIssue::UnmappedChannel=>"unmapped_channel",AttributionIssue::GroupUnresolved=>"group_unresolved"}])?;
    Ok(())
}
pub(super) fn key(a: &AttributedFact) -> UsageCellKey {
    let f = &a.native;
    UsageCellKey {
        minute: f.occurred_at - f.occurred_at % 60,
        channel_id: f.channel_id,
        source_id: a.source_id,
        group_id: a.group_id,
        native_group: f.native_group.clone(),
        token_id: f.token_id,
        model: f.model.clone(),
    }
}
pub(super) fn cell_row(r: &Row<'_>) -> LedgerResult<(UsageCellKey, UsageTotals)> {
    let c: Option<i64> = r.get(1)?;
    let s: Option<i64> = r.get(2)?;
    let g: Option<i64> = r.get(3)?;
    let k = UsageCellKey {
        minute: r.get(0)?,
        channel_id: c.map(ChannelId::new).transpose().map_err(|_| corrupt())?,
        source_id: s.map(SourceId::new).transpose().map_err(|_| corrupt())?,
        group_id: g.map(GroupId::new).transpose().map_err(|_| corrupt())?,
        native_group: r.get(4)?,
        token_id: r.get(5)?,
        model: r.get(6)?,
    };
    let t = UsageTotals {
        input: r.get(7)?,
        output: r.get(8)?,
        consumption_rows: r.get(9)?,
    };
    if !safe(k.minute)
        || k.minute % 60 != 0
        || t.consumption_rows < 1
        || k.token_id.is_some_and(|id| id <= 0)
        || !k.model.as_deref().is_none_or(valid_label)
        || !k.native_group.as_deref().is_none_or(valid_label)
        || (k.group_id.is_some() && (k.source_id.is_none() || k.native_group.is_none()))
        || (k.source_id.is_some() && k.channel_id.is_none())
    {
        return Err(corrupt());
    }
    add_usage(UsageTotals::default(), t).map_err(|_| corrupt())?;
    Ok((k, t))
}
pub(super) fn add_cell(
    conn: &Connection,
    k: &UsageCellKey,
    delta: UsageTotals,
) -> LedgerResult<()> {
    let args = params![
        k.minute,
        k.channel_id.map(|n| n.get()),
        k.source_id.map(|n| n.get()),
        k.group_id.map(|n| n.get()),
        k.native_group,
        k.token_id,
        k.model
    ];
    let mut stmt = conn.prepare(&format!(
        "SELECT input,output,consumption_rows FROM ledger_minute_usage WHERE {CELL_MATCH}"
    ))?;
    let old = stmt
        .query_row(args, |r| {
            Ok(UsageTotals {
                input: r.get(0)?,
                output: r.get(1)?,
                consumption_rows: r.get(2)?,
            })
        })
        .optional()?;
    if let Some(t) = old {
        if t.consumption_rows < 1 {
            return Err(corrupt());
        }
        add_usage(UsageTotals::default(), t).map_err(|_| corrupt())?;
    }
    let t = add_usage(old.unwrap_or_default(), delta)?;
    conn.prepare(&format!("INSERT INTO ledger_minute_usage({CELL_COLUMNS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT DO UPDATE SET input=excluded.input,output=excluded.output,consumption_rows=excluded.consumption_rows"))?.execute(params![k.minute,k.channel_id.map(|n|n.get()),k.source_id.map(|n|n.get()),k.group_id.map(|n|n.get()),k.native_group,k.token_id,k.model,t.input,t.output,t.consumption_rows])?;
    Ok(())
}

pub(super) fn subtract_cell(
    conn: &Connection,
    k: &UsageCellKey,
    delta: UsageTotals,
) -> LedgerResult<()> {
    let args = params![
        k.minute,
        k.channel_id.map(|v| v.get()),
        k.source_id.map(|v| v.get()),
        k.group_id.map(|v| v.get()),
        k.native_group,
        k.token_id,
        k.model
    ];
    let old = conn
        .query_row(
            &format!(
                "SELECT input,output,consumption_rows FROM ledger_minute_usage WHERE {CELL_MATCH}"
            ),
            args,
            |r| {
                Ok(UsageTotals {
                    input: r.get(0)?,
                    output: r.get(1)?,
                    consumption_rows: r.get(2)?,
                })
            },
        )
        .optional()?
        .ok_or_else(corrupt)?;
    add_usage(UsageTotals::default(), old).map_err(|_| corrupt())?;
    add_usage(UsageTotals::default(), delta).map_err(|_| corrupt())?;
    if old.consumption_rows < 1 || delta.consumption_rows < 1 {
        return Err(corrupt());
    }
    let sub = |a: i64, b: i64| a.checked_sub(b).filter(|v| *v >= 0).ok_or_else(corrupt);
    let remaining = UsageTotals {
        input: sub(old.input, delta.input)?,
        output: sub(old.output, delta.output)?,
        consumption_rows: sub(old.consumption_rows, delta.consumption_rows)?,
    };
    let changed = if remaining.consumption_rows == 0 {
        if remaining.input != 0 || remaining.output != 0 {
            return Err(corrupt());
        }
        conn.execute(
            &format!("DELETE FROM ledger_minute_usage WHERE {CELL_MATCH}"),
            args,
        )?
    } else {
        add_usage(UsageTotals::default(), remaining).map_err(|_| corrupt())?;
        conn.execute(&format!("UPDATE ledger_minute_usage SET input=?8,output=?9,consumption_rows=?10 WHERE {CELL_MATCH}"),params![k.minute,k.channel_id.map(|v|v.get()),k.source_id.map(|v|v.get()),k.group_id.map(|v|v.get()),k.native_group,k.token_id,k.model,remaining.input,remaining.output,remaining.consumption_rows])?
    };
    if changed != 1 {
        return Err(corrupt());
    }
    Ok(())
}
