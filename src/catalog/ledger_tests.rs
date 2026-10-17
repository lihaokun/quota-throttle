use super::*;
use crate::catalog::ledger::*;
use crate::ledger::{FactError, MAX_FACTS};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[path = "ledger_properties.rs"]
mod properties;
#[path = "ledger_scan_tests.rs"]
mod scans;

fn error_is<T>(r: LedgerResult<T>, expected: LedgerError) {
    assert_eq!(r.err(), Some(expected));
}
fn image(conn: &Connection) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    [
        "ledger_state",
        "ledger_checkpoints",
        "ledger_facts",
        "ledger_minute_usage",
    ]
    .iter()
    .map(|table| {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let n = stmt.column_count();
        stmt.query_map([], |r| (0..n).map(|i| r.get(i)).collect())
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    })
    .collect()
}

fn full_facts(conn: &Connection) -> Vec<Value> {
    conn.prepare("SELECT native_log_id,request_id,kind,occurred_at,channel_id,token_id,model,native_group,input,output,is_stream,metadata_json,source_id,group_id,attribution_issue FROM ledger_facts ORDER BY native_log_id").unwrap().query_map([],|r|Ok(json!({
        "id":r.get::<_,i64>(0)?,"request":r.get::<_,Option<String>>(1)?,"kind":r.get::<_,String>(2)?,"at":r.get::<_,i64>(3)?,"channel":r.get::<_,Option<i64>>(4)?,"token":r.get::<_,Option<i64>>(5)?,"model":r.get::<_,Option<String>>(6)?,"group":r.get::<_,Option<String>>(7)?,"input":r.get::<_,i64>(8)?,"output":r.get::<_,i64>(9)?,"stream":r.get::<_,i64>(10)?,"metadata":serde_json::from_str::<Value>(&r.get::<_,String>(11)?).unwrap(),"source":r.get::<_,Option<i64>>(12)?,"group_id":r.get::<_,Option<i64>>(13)?,"issue":r.get::<_,String>(14)?
    }))).unwrap().map(|r|r.unwrap()).collect()
}
fn total(conn: &Connection) -> (i64, i64, i64) {
    let mut stmt = conn
        .prepare("SELECT input,output,consumption_rows FROM ledger_minute_usage")
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })
    .unwrap()
    .map(|r| r.unwrap())
    .fold((0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2))
}
fn mapped(n: usize) -> (TempDir, CatalogStore, SourceId, GroupId) {
    let (dir, mut store) = setup();
    let s = store
        .create_source(source("fake-ledger-source", SourceKind::Glm))
        .unwrap()
        .source_id;
    let g = store
        .create_group(group("ledger-g", SourceKind::Glm))
        .unwrap()
        .group_id;
    // 有意用退休历史，且source/group身份不同于channel；侧表完整，重开仍需通过原核验。
    let tx = store.conn.transaction().unwrap();
    for i in 0..n {
        tx.execute("INSERT INTO memberships(source_id,group_id,kind,channel_id,position,lifecycle,desired_priority,retired_at) VALUES(?1,?2,'glm',?3,0,'retired',10,10)",params![s.get(),g.get(),1001+i as i64]).unwrap();
    }
    tx.execute("INSERT INTO member_control(membership_id,revision,status_directive,retire_requested,create_state,create_name,dirty,next_attempt_at) SELECT membership_id,1,'preserve',0,'resolved','qt-'||(SELECT catalog_instance_id FROM catalog_meta)||'-m'||membership_id,0,0 FROM memberships",[]).unwrap();
    tx.execute(
        "INSERT INTO member_execution SELECT membership_id,0,0,0,0 FROM memberships",
        [],
    )
    .unwrap();
    tx.commit().unwrap();
    (dir, store, s, g)
}

fn row(id: i64) -> Value {
    json!({"id":id,"type":2,"created_at":125,"channel":0,"token_id":0,"model_name":null,"group":null,"prompt_tokens":5,"completion_tokens":3,"is_stream":false,"other":""})
}
fn begin(store: &mut CatalogStore) -> ScanTicket {
    store.initialize_ledger(0, 300).unwrap();
    let state = store.load_ledger_state().unwrap();
    store
        .begin_scan(
            ScanLane::Recent,
            state.scans[0].generation,
            ScanWindow { from: 0, to: 300 },
            300,
        )
        .unwrap()
}
#[test]
fn smoke_ingest_reopen_and_replay() {
    let (dir, mut store) = setup();
    let t = begin(&mut store);
    let c = store
        .commit_fact_page(&t, &[row(1), row(1)], PageEnd::ReachedEnd, 300)
        .unwrap();
    assert_eq!((c.inserted, c.duplicates, c.changed_cells), (1, 1, 1));
    drop(store);
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let t = begin(&mut store);
    let c = store
        .commit_fact_page(&t, &[row(1), row(2)], PageEnd::ReachedEnd, 300)
        .unwrap();
    assert_eq!((c.inserted, c.duplicates), (1, 1));
    let got: (i64, i64, i64) = store
        .conn
        .query_row(
            "SELECT input,output,consumption_rows FROM ledger_minute_usage",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(got, (10, 6, 2));
}

#[test]
fn checkpoints_reject_stale_foreign_and_backwards_events() {
    let (_dir, mut store) = setup();
    error_is(
        store.load_scan_ticket(ScanLane::Recent),
        LedgerError::NotInitialized,
    );
    error_is(
        store.initialize_ledger(300, 300),
        LedgerError::InvalidCheckpoint,
    );
    let t = begin(&mut store);
    let before = image(&store.conn);
    let t2 = begin(&mut store);
    assert!(t == t2);
    assert_eq!(image(&store.conn), before);
    error_is(
        store.initialize_ledger(1, 300),
        LedgerError::RevisionConflict,
    );
    let (_other, mut foreign) = setup();
    let other = begin(&mut foreign);
    error_is(
        store.commit_fact_page(&other, &[row(1)], PageEnd::More, 300),
        LedgerError::RevisionConflict,
    );
    store.commit_fact_page(&t, &[], PageEnd::More, 400).unwrap();
    let current = store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
    assert_eq!(current.progress().next_page, 2);
    error_is(
        store.commit_fact_page(&t, &[row(1)], PageEnd::More, 400),
        LedgerError::RevisionConflict,
    );
    error_is(
        store.commit_fact_page(&current, &[], PageEnd::More, 399),
        LedgerError::InvalidCheckpoint,
    );
    let old = image(&store.conn);
    error_is(
        store.mark_scan_fault(&current, ScanFault::Clock, 399),
        LedgerError::InvalidCheckpoint,
    );
    assert_eq!(old, image(&store.conn));
    let fault = store
        .mark_scan_fault(&current, ScanFault::Transport, 400)
        .unwrap();
    assert_eq!((fault.next_page, fault.phase), (2, ScanPhase::Stalled));
    error_is(
        store.mark_scan_fault(&current, ScanFault::Transport, 400),
        LedgerError::RevisionConflict,
    );
    error_is(
        store.begin_scan(
            ScanLane::Recent,
            fault.generation,
            ScanWindow { from: 1, to: 300 },
            300,
        ),
        LedgerError::RevisionConflict,
    );
    let next = store
        .begin_scan(
            ScanLane::Recent,
            fault.generation,
            ScanWindow { from: 0, to: 300 },
            300,
        )
        .unwrap();
    assert_eq!(next.progress().generation, fault.generation + 1);
    assert_eq!(next.progress().next_page, 1);
    error_is(
        store.commit_fact_page(&current, &[], PageEnd::More, 400),
        LedgerError::RevisionConflict,
    );
    store
        .commit_fact_page(&next, &[json!({"type":7})], PageEnd::ReachedEnd, 400)
        .unwrap();
    assert!(store.load_scan_ticket(ScanLane::Recent).unwrap().is_none());
    assert_eq!(total(&store.conn), (0, 0, 0));
}

#[test]
fn same_identity_conflicts_in_every_field_without_partial_writes() {
    let (_dir, mut store) = setup();
    let t = begin(&mut store);
    store
        .commit_fact_page(&t, &[row(7)], PageEnd::ReachedEnd, 300)
        .unwrap();
    for (field, value) in [
        ("created_at", json!(126)),
        ("channel", json!(8)),
        ("token_id", json!(9)),
        ("model_name", json!("changed")),
        ("group", json!("historical")),
        ("prompt_tokens", json!(6)),
        ("completion_tokens", json!(4)),
        ("is_stream", json!(true)),
        ("request_id", json!("new-id")),
        ("other", json!("{}")),
    ] {
        let t = begin(&mut store);
        let old = image(&store.conn);
        let mut changed = row(7);
        changed[field] = value;
        error_is(
            store.commit_fact_page(&t, &[row(1), changed], PageEnd::ReachedEnd, 300),
            LedgerError::Fact(FactError::ConflictingIdentity),
        );
        assert_eq!(image(&store.conn), old);
    }
    let t = begin(&mut store);
    let old = image(&store.conn);
    let mut bad = row(9);
    bad["created_at"] = json!(300);
    error_is(
        store.commit_fact_page(&t, &[bad], PageEnd::More, 300),
        LedgerError::InvalidCheckpoint,
    );
    error_is(
        store.commit_fact_page(&t, &vec![row(1); MAX_FACTS + 1], PageEnd::More, 300),
        LedgerError::Fact(FactError::LimitExceeded),
    );
    assert_eq!(image(&store.conn), old);
}

#[test]
fn late_overflow_and_sql_failure_roll_back_facts_cells_and_checkpoint() {
    for fault in 0..4 {
        let (_dir, mut store) = setup();
        let t = begin(&mut store);
        let mut full = row(10);
        full["model_name"] = json!("z");
        full["prompt_tokens"] = json!(JSON_SAFE);
        full["completion_tokens"] = json!(0);
        if fault == 0 {
            store
                .commit_fact_page(&t, &[full], PageEnd::ReachedEnd, 300)
                .unwrap();
        }
        let t = begin(&mut store);
        let old = image(&store.conn);
        let mut a = row(1);
        a["model_name"] = json!("a");
        let mut z = row(2);
        z["model_name"] = json!("z");
        match fault {
            1=>store.conn.execute_batch("CREATE TRIGGER fail_fact BEFORE INSERT ON ledger_facts WHEN NEW.native_log_id=2 BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap(),
            2=>store.conn.execute_batch("CREATE TRIGGER fail_cell BEFORE INSERT ON ledger_minute_usage WHEN NEW.model='z' BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap(),
            3=>store.conn.execute_batch("CREATE TRIGGER fail_checkpoint BEFORE UPDATE ON ledger_checkpoints BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap(),_=>{},
        }
        let result = store.commit_fact_page(&t, &[a, z], PageEnd::ReachedEnd, 300);
        if fault == 0 {
            error_is(result, LedgerError::Fact(FactError::ArithmeticOverflow));
        } else {
            assert!(result.is_err());
        }
        assert_eq!(image(&store.conn), old);
        assert!(!store.poisoned);
    }
}

#[test]
fn deferred_commit_failure_poisoning_requires_reopen() {
    let (dir, mut store) = setup();
    let t = begin(&mut store);
    let old = image(&store.conn);
    store.conn.execute_batch("CREATE TABLE ledger_test_parent(id INTEGER PRIMARY KEY); CREATE TABLE ledger_test_child(id INTEGER REFERENCES ledger_test_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER deferred_fault AFTER INSERT ON ledger_facts BEGIN INSERT INTO ledger_test_child VALUES(1); END;").unwrap();
    assert!(matches!(
        store.commit_fact_page(&t, &[row(1)], PageEnd::ReachedEnd, 300),
        Err(LedgerError::Catalog(CatalogError {
            kind: ErrorKind::CommitUnknown,
            ..
        }))
    ));
    assert!(store.poisoned);
    assert_eq!(image(&store.conn), old);
    assert!(store
        .mark_scan_fault(&t, ScanFault::Transport, 300)
        .is_err());
    store.conn.execute_batch("DROP TRIGGER deferred_fault; DROP TABLE ledger_test_child; DROP TABLE ledger_test_parent;").unwrap();
    drop(store);
    let mut reopened = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let t = reopened
        .load_scan_ticket(ScanLane::Recent)
        .unwrap()
        .unwrap();
    assert_eq!(
        reopened
            .commit_fact_page(&t, &[row(1)], PageEnd::ReachedEnd, 300)
            .unwrap()
            .inserted,
        1
    );
}

#[test]
fn retired_mapping_group_mismatch_and_newly_resolvable_unknown_survive_open() {
    let (dir, mut store, s, g) = mapped(2);
    let t = begin(&mut store);
    let mut a = row(1);
    a["channel"] = json!(1001);
    a["group"] = json!("ledger-g");
    let mut b = row(2);
    b["channel"] = json!(1002);
    b["group"] = json!("foreign");
    let mut c = row(3);
    c["channel"] = json!(5000);
    c["group"] = json!("ledger-g");
    store
        .commit_fact_page(&t, &[a, b, c.clone()], PageEnd::ReachedEnd, 300)
        .unwrap();
    let got: Vec<(Option<i64>, Option<i64>)> = store
        .conn
        .prepare("SELECT source_id,group_id FROM ledger_facts ORDER BY native_log_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        got,
        vec![
            (Some(s.get()), Some(g.get())),
            (Some(s.get()), None),
            (None, None)
        ]
    );
    // 之后才绑定新历史渠道；旧unknown不得被open偷偷计第二次。
    let m = store.plan_membership(s, g, 10).unwrap();
    store
        .conn
        .execute(
            "UPDATE memberships SET channel_id=5000,lifecycle='ready' WHERE membership_id=?1",
            [m.membership_id.get()],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
            [m.membership_id.get()],
        )
        .unwrap();
    let old = image(&store.conn);
    drop(store);
    let mut reopened = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&reopened.conn), old);
    let t = begin(&mut reopened);
    assert_eq!(
        reopened
            .commit_fact_page(&t, &[c], PageEnd::ReachedEnd, 300)
            .unwrap()
            .inserted,
        0
    );
    assert_eq!(total(&reopened.conn), (15, 9, 3));
}

#[test]
fn null_cell_unique_constraint_and_metadata_redaction_are_enforced() {
    let (_dir, mut store) = setup();
    let t = begin(&mut store);
    let mut a = row(1);
    a["request_id"] = json!("same");
    a["other"]=json!(json!({"content":"PRIVATE_SENTINEL","cache_tokens":999,"admin_info":{"use_channel":["3","3"]},"stream_status":{"status":"error","end_reason":"client_gone"}}).to_string());
    let mut b = a.clone();
    b["id"] = json!(2);
    let mut err = a.clone();
    err["id"] = json!(3);
    err["type"] = json!(5);
    err["prompt_tokens"] = json!(0);
    err["completion_tokens"] = json!(0);
    store
        .commit_fact_page(&t, &[a, b, err], PageEnd::ReachedEnd, 300)
        .unwrap();
    assert_eq!(total(&store.conn), (10, 6, 2));
    let stored: String = store
        .conn
        .query_row(
            "SELECT metadata_json FROM ledger_facts WHERE native_log_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!stored.contains("PRIVATE_SENTINEL") && !stored.contains("cache_tokens"));
    assert!(store
        .conn
        .execute(
            "INSERT INTO ledger_minute_usage SELECT * FROM ledger_minute_usage",
            []
        )
        .is_err());
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(ScanTicket, std::fmt::Debug);
    assert_not_impl!(ScanTicket, serde::Serialize);
    assert_not_impl!(ScanTicket, serde::Deserialize<'static>);
}

#[test]
fn corrupt_rows_missing_state_and_projection_mismatch_are_not_repaired() {
    for sql in ["DELETE FROM ledger_state", "DELETE FROM ledger_checkpoints WHERE lane='recent'", "UPDATE ledger_facts SET metadata_json='{\"version\":2,\"state\":\"absent\"}'", "UPDATE ledger_facts SET metadata_json='{\"version\":1,\"state\":\"absent\",\"secret\":\"PRIVATE_SENTINEL\"}'", "UPDATE ledger_minute_usage SET input=input+1", "DELETE FROM ledger_minute_usage", "UPDATE ledger_facts SET model='bad'||char(10)", "UPDATE ledger_facts SET metadata_json='{\"version\":1,\"state\":\"parsed\",\"status_code\":null,\"error_type\":null,\"error_code\":null,\"attempted_channels\":null,\"stream\":\"new\"}'"] {
        let (dir,mut store)=setup();let t=begin(&mut store);store.commit_fact_page(&t,&[row(1)],PageEnd::ReachedEnd,300).unwrap();
        store.conn.execute_batch(sql).unwrap();let old=image(&store.conn);drop(store);
        let err=CatalogStore::open(&dir.path().join("catalog.sqlite3")).err().unwrap();assert!(!format!("{err:?}").contains("PRIVATE_SENTINEL"));
        let conn=Connection::open_with_flags(dir.path().join("catalog.sqlite3"),OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();assert_eq!(image(&conn),old);
    }
}

#[test]
fn known_attribution_corruption_is_rejected_even_when_projection_matches() {
    for change_source in [false, true] {
        let (dir, mut store, _, _) = mapped(1);
        let other = store
            .create_source(source("fake-wrong-ledger-source", SourceKind::Glm))
            .unwrap()
            .source_id;
        let other_group = store
            .create_group(group("wrong-ledger-group", SourceKind::Glm))
            .unwrap()
            .group_id;
        let t = begin(&mut store);
        let mut r = row(1);
        r["channel"] = json!(1001);
        r["group"] = json!("ledger-g");
        store
            .commit_fact_page(&t, &[r], PageEnd::ReachedEnd, 300)
            .unwrap();
        let (column, id) = if change_source {
            ("source_id", other.get())
        } else {
            ("group_id", other_group.get())
        };
        store
            .conn
            .execute(&format!("UPDATE ledger_facts SET {column}=?1"), [id])
            .unwrap();
        store
            .conn
            .execute(&format!("UPDATE ledger_minute_usage SET {column}=?1"), [id])
            .unwrap();
        let before = image(&store.conn);
        drop(store);
        let e = CatalogStore::open(&dir.path().join("catalog.sqlite3"))
            .err()
            .unwrap();
        assert_eq!(e.kind, ErrorKind::CorruptData);
        let conn = Connection::open_with_flags(
            dir.path().join("catalog.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(image(&conn), before);
    }
}

#[test]
fn unused_invalid_history_is_storage_corruption() {
    let (_dir, mut store, _, _) = mapped(1);
    let t = begin(&mut store);
    store
        .conn
        .execute("UPDATE source_groups SET newapi_group=char(10)", [])
        .unwrap();
    let old = image(&store.conn);
    assert!(matches!(
        store.commit_fact_page(&t, &[], PageEnd::ReachedEnd, 300),
        Err(LedgerError::Catalog(CatalogError {
            kind: ErrorKind::CorruptData,
            ..
        }))
    ));
    assert_eq!(image(&store.conn), old);
}

#[test]
fn checkpoint_number_and_missing_table_corruption_are_rejected() {
    for field in ["revision", "generation", "next_page"] {
        let (_dir, mut store) = setup();
        let t = begin(&mut store);
        if field == "generation" {
            store
                .commit_fact_page(&t, &[], PageEnd::ReachedEnd, 300)
                .unwrap();
        }
        let table = if field == "revision" {
            "ledger_state"
        } else {
            "ledger_checkpoints"
        };
        store
            .conn
            .execute(
                &format!(
                    "UPDATE {table} SET {field}=?1 WHERE {}",
                    if field == "revision" {
                        "singleton=1"
                    } else {
                        "lane='recent'"
                    }
                ),
                [JSON_SAFE],
            )
            .unwrap();
        let old = image(&store.conn);
        if field == "generation" {
            error_is(
                store.begin_scan(
                    ScanLane::Recent,
                    JSON_SAFE,
                    ScanWindow { from: 0, to: 300 },
                    300,
                ),
                LedgerError::InvalidCheckpoint,
            );
        } else {
            let t = store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
            error_is(
                store.commit_fact_page(&t, &[row(1)], PageEnd::More, 300),
                LedgerError::InvalidCheckpoint,
            );
        }
        assert_eq!(image(&store.conn), old);
    }
    for damage in [
        "DROP TABLE ledger_checkpoints",
        "PRAGMA ignore_check_constraints=ON; UPDATE ledger_checkpoints SET next_page=0;",
        "PRAGMA ignore_check_constraints=ON; UPDATE ledger_checkpoints SET generation=-1;",
    ] {
        let (dir, mut store) = setup();
        let t = begin(&mut store);
        store
            .commit_fact_page(&t, &[row(1)], PageEnd::ReachedEnd, 300)
            .unwrap();
        let expected = full_facts(&store.conn);
        store.conn.execute_batch(damage).unwrap();
        drop(store);
        assert_eq!(
            CatalogStore::open(&dir.path().join("catalog.sqlite3"))
                .err()
                .unwrap()
                .kind,
            ErrorKind::CorruptData
        );
        let conn = Connection::open_with_flags(
            dir.path().join("catalog.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(full_facts(&conn), expected);
    }
}

#[test]
fn stored_codec_roundtrip_boundaries_and_late_row_corruption() {
    let (dir, mut store) = setup();
    let t = begin(&mut store);
    let mut input = Vec::new();
    let mut expected = Vec::new();
    for (i, (raw_stream, stream)) in [
        (Value::Null, "missing"),
        (json!({"status":"ok","end_reason":"done"}), "done"),
        (json!({"status":"ok","end_reason":"eof"}), "eof"),
        (
            json!({"status":"error","end_reason":"client_gone"}),
            "client_gone",
        ),
        (json!({"status":"future","end_reason":"future"}), "other"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = i as i64 + 1;
        let mut r = row(id);
        r["request_id"] = json!("r".repeat(64));
        r["token_id"] = json!(i64::MAX);
        r["model_name"] = json!("m".repeat(256));
        r["group"] = json!("g".repeat(256));
        r["is_stream"] = json!(true);
        r["other"]=json!(json!({"status_code":599,"error_type":"é".repeat(64),"error_code":"c".repeat(128),"admin_info":{"use_channel":vec![i64::MAX.to_string();128]},"stream_status":raw_stream,"content":"PRIVATE_IGNORED"}).to_string());
        expected.push(json!({"id":id,"request":"r".repeat(64),"kind":"consume","at":125,"channel":null,"token":i64::MAX,"model":"m".repeat(256),"group":"g".repeat(256),"input":5,"output":3,"stream":1,"metadata":{"version":1,"state":"parsed","status_code":599,"error_type":"é".repeat(64),"error_code":"c".repeat(128),"attempted_channels":vec![i64::MAX;128],"stream":stream},"source":null,"group_id":null,"issue":"unmapped_channel"}));
        input.push(r);
    }
    let mut max = row(i64::MAX);
    max["other"] = json!("invalid");
    input.push(max);
    expected.push(json!({"id":i64::MAX,"request":null,"kind":"consume","at":125,"channel":null,"token":null,"model":null,"group":null,"input":5,"output":3,"stream":0,"metadata":{"version":1,"state":"unusable"},"source":null,"group_id":null,"issue":"unmapped_channel"}));
    store
        .commit_fact_page(&t, &input, PageEnd::ReachedEnd, 300)
        .unwrap();
    assert_eq!(full_facts(&store.conn), expected);
    drop(store);
    let mut store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(full_facts(&store.conn), expected);
    let valid = expected[0]["metadata"].clone();
    for (name, value) in [
        ("error_code", json!("x".repeat(129))),
        ("error_type", json!("\n")),
        ("status_code", json!(600)),
        ("attempted_channels", json!(vec![1; 129])),
        ("version", json!(2)),
        ("extra", json!("PRIVATE_SENTINEL")),
    ] {
        let mut bad = valid.clone();
        bad[name] = value;
        assert!(crate::ledger::storage_codec::decode(&bad.to_string()).is_err());
    }
    let raw = "{\"version\":1,\"state\":\"absent\"}";
    let boundary = format!("{raw}{}", " ".repeat(16384 - raw.len()));
    assert!(crate::ledger::storage_codec::decode(&boundary).is_ok());
    assert_eq!(
        crate::ledger::storage_codec::decode(&(boundary + " ")).err(),
        Some(FactError::LimitExceeded)
    );
    for part in (100..1600).map(row).collect::<Vec<_>>().chunks(1000) {
        let t = begin(&mut store);
        store
            .commit_fact_page(&t, part, PageEnd::ReachedEnd, 300)
            .unwrap();
    }
    store.conn.execute("UPDATE ledger_facts SET metadata_json='{\"version\":2,\"state\":\"absent\"}' WHERE native_log_id=?1",[i64::MAX]).unwrap();
    drop(store);
    assert_eq!(
        CatalogStore::open(&dir.path().join("catalog.sqlite3"))
            .err()
            .unwrap()
            .kind,
        ErrorKind::CorruptData
    );
    let (edge_dir, mut edge) = setup();
    edge.initialize_ledger(JSON_SAFE - 1, JSON_SAFE).unwrap();
    let t = edge
        .begin_scan(
            ScanLane::Recent,
            0,
            ScanWindow {
                from: JSON_SAFE - 1,
                to: JSON_SAFE,
            },
            JSON_SAFE,
        )
        .unwrap();
    let mut r = row(i64::MAX);
    r["created_at"] = json!(JSON_SAFE - 1);
    r["prompt_tokens"] = json!(0);
    r["completion_tokens"] = json!(0);
    edge.commit_fact_page(&t, &[r], PageEnd::ReachedEnd, JSON_SAFE)
        .unwrap();
    drop(edge);
    let edge = CatalogStore::open(&edge_dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(
        edge.conn
            .query_row("SELECT minute FROM ledger_minute_usage", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        ((JSON_SAFE - 1) / 60) * 60
    );
    assert_eq!(total(&edge.conn), (0, 0, 1));
}

fn maximum_ingestion() -> Duration {
    let (_dir, mut store, _, _) = mapped(10_000);
    let t = begin(&mut store);
    let rows:Vec<_>=(1..=1024).map(|id|{let mut r=row(id);r["channel"]=json!(1000+id);r["group"]=json!("ledger-g");r["token_id"]=json!(id+10000);r["model_name"]=json!(format!("m{id:04}{}","x".repeat(251)));r["other"]=json!(json!({"error_code":"e".repeat(128),"error_type":"t".repeat(128),"admin_info":{"use_channel":vec![i64::MAX.to_string();128]}}).to_string());r}).collect();
    let start = Instant::now();
    let c = store
        .commit_fact_page(&t, &rows, PageEnd::ReachedEnd, 300)
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!((c.inserted, c.changed_cells), (1024, 1024));
    assert_eq!(total(&store.conn), (5120, 3072, 1024));
    // 超界历史不能因当前没有对应流量而绕过。
    store.conn.execute("INSERT INTO memberships(source_id,group_id,kind,channel_id,position,lifecycle,desired_priority,retired_at) SELECT source_id,group_id,kind,999999,0,'retired',10,10 FROM memberships LIMIT 1",[]).unwrap();
    let t = begin(&mut store);
    let old = image(&store.conn);
    error_is(
        store.commit_fact_page(&t, &[], PageEnd::ReachedEnd, 300),
        LedgerError::Fact(FactError::LimitExceeded),
    );
    assert_eq!(old, image(&store.conn));
    elapsed
}
#[test]
fn maximum_ingestion_capacity_without_wall_clock_budget() {
    maximum_ingestion();
}
#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_ledger_ingestion() {
    let t = maximum_ingestion();
    println!("LEDGER_INGESTION_MS={}", t.as_millis());
    assert!(t < Duration::from_secs(5));
}

#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_ledger_reopen_100k() {
    let (dir, mut store) = setup();
    let t = begin(&mut store);
    store
        .commit_fact_page(&t, &[row(1)], PageEnd::ReachedEnd, 300)
        .unwrap();
    // 正确性模型：每分钟恰有一条5+3消费，100000个单元；不是复制单个总量掩盖遍历成本。
    store.conn.execute_batch("BEGIN; DELETE FROM ledger_facts; DELETE FROM ledger_minute_usage; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO ledger_facts(native_log_id,kind,occurred_at,input,output,is_stream,metadata_json,attribution_issue) SELECT x,'consume',x*60,5,3,0,'{\"version\":1,\"state\":\"absent\"}','unmapped_channel' FROM n; INSERT INTO ledger_minute_usage(minute,input,output,consumption_rows) SELECT occurred_at,5,3,1 FROM ledger_facts; COMMIT;").unwrap();
    let plans:Vec<String>=store.conn.prepare("EXPLAIN QUERY PLAN SELECT * FROM ledger_facts INDEXED BY ledger_fact_projection WHERE kind='consume' ORDER BY occurred_at-occurred_at%60,coalesce(channel_id,0),coalesce(source_id,0),coalesce(group_id,0),coalesce(native_group,''),coalesce(token_id,0),coalesce(model,''),native_log_id").unwrap().query_map([],|r|r.get(3)).unwrap().map(|r|r.unwrap()).collect();
    assert!(plans.iter().any(|p| p.contains("ledger_fact_projection")));
    assert!(plans.iter().all(|p| !p.contains("TEMP B-TREE")));
    drop(store);
    let start = Instant::now();
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(total(&store.conn), (500000, 300000, 100000));
    println!("LEDGER_REOPEN_100K_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(10));
}
