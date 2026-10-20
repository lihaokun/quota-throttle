//! B5 只使用本文件创建的私有临时库；历史收据/执行结果为 SQL fixture，不发送 HTTP。
use super::*;
use crate::catalog::control::{ControlCommand, OperationId, StatusDirective};
use crate::catalog::execution::ExecutionEffect;
use rusqlite::types::Value;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[path = "ledger_migration_tests.rs"]
mod ledger;

const CORE: &[&str] = &[
    "source_secrets",
    "sources",
    "source_groups",
    "memberships",
    "legacy_import",
    "sqlite_sequence",
];
const CONTROL: &[&str] = &[
    "source_control",
    "member_control",
    "control_operations",
    "operation_targets",
];
const EXECUTION: &[&str] = &["member_execution", "execution_attempts"];

#[derive(Debug, Clone, PartialEq)]
struct TableImage {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}
type Image = BTreeMap<String, TableImage>;

/// 表名仅来自上面固定的测试清单；读取原始值，不复用被测策略/目录投影。
fn image_of(conn: &Connection, tables: &[&str]) -> Image {
    tables
        .iter()
        .map(|name| {
            let mut query = conn
                .prepare(&format!("SELECT * FROM {name} ORDER BY 1,2"))
                .unwrap();
            let columns = query
                .column_names()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            let rows = query
                .query_map([], |r| {
                    (0..columns.len())
                        .map(|i| r.get(i))
                        .collect::<rusqlite::Result<Vec<Value>>>()
                })
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            ((*name).to_owned(), TableImage { columns, rows })
        })
        .collect()
}
fn history_image(conn: &Connection) -> Image {
    image_of(conn, &[CORE, CONTROL, EXECUTION].concat())
}
fn assert_images(actual: &Image, expected: &Image) {
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    for (name, table) in expected {
        let actual = &actual[name];
        assert_eq!(actual.columns, table.columns, "{name} columns");
        assert_eq!(actual.rows.len(), table.rows.len(), "{name} rows");
        for (index, row) in table.rows.iter().enumerate() {
            assert_eq!(&actual.rows[index], row, "{name} row {index}");
        }
    }
}
fn cell<'a>(table: &'a TableImage, row: &'a [Value], name: &str) -> &'a Value {
    &row[table.columns.iter().position(|s| s == name).unwrap()]
}
fn integer(value: &Value) -> i64 {
    match value {
        Value::Integer(n) => *n,
        _ => panic!("expected integer: {value:?}"),
    }
}
fn row_index(table: &TableImage, column: &str, value: &Value) -> usize {
    table
        .rows
        .iter()
        .position(|r| cell(table, r, column) == value)
        .unwrap()
}
fn set(image: &mut Image, table: &str, row: usize, column: &str, value: Value) {
    let table = image.get_mut(table).unwrap();
    let col = table.columns.iter().position(|s| s == column).unwrap();
    table.rows[row][col] = value;
}
fn epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
fn instance(conn: &Connection) -> String {
    conn.query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
        r.get(0)
    })
    .unwrap()
}

// 只读打开 WAL 备份可能留下 -wal/-shm；它们不是额外的备份副本。
fn backup_files(dir: &TempDir, version: i64) -> Vec<PathBuf> {
    let prefix = format!("catalog-v{version}-before-v{}-", version + 1);
    let mut files: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|r| r.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "sqlite3")
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
        })
        .collect();
    files.sort();
    files
}
fn backups_v3(dir: &TempDir) -> Vec<PathBuf> {
    backup_files(dir, 3)
}

struct History {
    dir: TempDir,
    store: CatalogStore,
    volcano: Vec<SourceId>,
    policies: Vec<VolcanoPolicy>,
    glm_groups: Vec<GroupId>,
    glm_members: Vec<Vec<MembershipId>>,
    volcano_groups: Vec<GroupId>,
    volcano_members: Vec<Vec<MembershipId>>,
    empty_group: GroupId,
}

fn history(n: usize, nv: usize, salt: i64, offset: i64, prepared: bool) -> History {
    let (dir, mut store) = setup();
    let (mut cfg, mut channels) = legacy(n);
    for (i, key) in cfg.keys.iter_mut().enumerate() {
        key.zhipu_api_key = format!("fake-b5-private-{salt}-{i}");
        key.quota_headers.push(HeaderKV {
            key: "X-B5-Selector".into(),
            value: format!("selector-{salt}-{i}"),
        });
        channels[i].models_csv = format!("glm-{salt}-{i},shared");
        channels[i].priority = salt - i as i64 * 13;
    }
    let plan = build_legacy_import(&cfg, &channels).unwrap();
    let receipt = match store.import_legacy(&plan).unwrap() {
        ImportOutcome::Applied(r) => r,
        _ => panic!(),
    };
    let extra = store
        .create_group(group("b5-glm-extra", SourceKind::Glm))
        .unwrap();
    let mut extra_members = Vec::new();
    for source in &receipt.source_ids {
        extra_members.push(
            store
                .plan_membership(*source, extra.group_id, salt + 21)
                .unwrap()
                .membership_id,
        );
    }
    // 同组历史引用合法，且成员身份与来源身份不同。
    store
        .conn
        .execute(
            "UPDATE memberships SET lifecycle='retired',retired_at=?1 WHERE membership_id=?2",
            params![NOW, extra_members[0].get()],
        )
        .unwrap();
    let mut volcano_groups = Vec::new();
    for i in 0..2 {
        volcano_groups.push(
            store
                .create_group(group(&format!("b5-volcano-{i}"), SourceKind::Volcano))
                .unwrap()
                .group_id,
        );
    }
    let empty_group = store
        .create_group(group("b5-empty", SourceKind::Volcano))
        .unwrap()
        .group_id;
    let mut volcano = Vec::new();
    let mut policies = Vec::new();
    let mut volcano_members = vec![Vec::new(), Vec::new()];
    for i in 0..nv {
        let mut draft = source(&format!("fake-b5-volcano-{salt}-{i}"), SourceKind::Volcano);
        let policy = VolcanoPolicy {
            daily_token_limit: Some(300 + salt + i as i64 * 17),
            failure_threshold: 2 + i as i64,
            reset_minute: (salt + i as i64 * 37) % 1440,
            utc_offset_minutes: offset,
        };
        if let SourceSettings::Volcano {
            upstream,
            policy: p,
        } = &mut draft.settings
        {
            *p = policy.clone();
            upstream.base_url = format!("https://source-{i}.invalid/v1");
            upstream.models = vec![format!("volcano-{salt}-{i}"), "shared".into()];
            upstream.model_discovery.as_mut().unwrap().auth = if i % 2 == 0 {
                ModelDiscoveryAuth::Bearer
            } else {
                ModelDiscoveryAuth::XApiKey
            };
        }
        draft.name = format!("volcano-{salt}-{i}");
        draft.note = format!("note-{salt}-{i}");
        let s = store.create_source(draft).unwrap();
        for (gi, gid) in volcano_groups.iter().enumerate() {
            let m = store
                .plan_membership(s.source_id, *gid, salt - i as i64)
                .unwrap();
            assert_ne!(m.membership_id.get(), s.source_id.get());
            if gi == 0 {
                store.conn.execute("UPDATE memberships SET channel_id=?1,lifecycle='ready',observed_status=2,observed_priority=?2 WHERE membership_id=?3",
                    params![10000+m.membership_id.get()*7,salt-i as i64,m.membership_id.get()]).unwrap();
                store
                    .conn
                    .execute(
                        "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
                        [m.membership_id.get()],
                    )
                    .unwrap();
            }
            volcano_members[gi].push(m.membership_id);
        }
        store
            .conn
            .execute(
                "UPDATE sources SET manual_paused=?1 WHERE source_id=?2",
                params![i % 2 == 0, s.source_id.get()],
            )
            .unwrap();
        volcano.push(s.source_id);
        policies.push(policy);
    }
    // 构造有真实字段形状的旧库记录；本测试检验保存/恢复，不冒充远端成功证据。
    for (i, id) in receipt.membership_ids.iter().enumerate() {
        let op = OperationId::new(format!("b5-history-{i:016}")).unwrap();
        store
            .submit_operation(
                op.clone(),
                ControlCommand::EnsureMember {
                    membership_id: *id,
                    expected_revision: 1,
                    status: StatusDirective::Enabled,
                },
                NOW,
            )
            .unwrap();
        let mode = i % 4;
        let state = match mode {
            0 => "responded",
            1 => "not_sent",
            2 => "uncertain",
            _ if prepared => "prepared",
            _ => "responded",
        };
        let error = match state {
            "not_sent" => Some("auth_failed"),
            "uncertain" => Some("transport_unknown"),
            _ => None,
        };
        let sequence = if state == "prepared" { 1 } else { 2 };
        let effect = serde_json::to_string(&ExecutionEffect::Fields {
            priority: Some(salt + i as i64),
            models: Some(format!("effect-{salt}-{i}")),
            disable_auto_ban: false,
        })
        .unwrap();
        store
            .conn
            .execute(
                "UPDATE member_execution SET sequence=?1,enable_consumed=?2 WHERE membership_id=?3",
                params![sequence, mode == 0, id.get()],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO execution_attempts VALUES(?1,1,2,1,1,1,'fields',?2,?3,?4,?5,?6)",
                params![
                    id.get(),
                    effect,
                    state,
                    error,
                    NOW + i as i64,
                    if state == "prepared" {
                        None
                    } else {
                        Some(NOW + 20 + i as i64)
                    }
                ],
            )
            .unwrap();
        let phase = match state {
            "prepared" => "applying",
            "uncertain" => "unknown",
            "not_sent" => "failed",
            _ if mode == 0 => "succeeded",
            _ => "waiting",
        };
        if mode == 0 {
            let native = channels[i].id;
            let json = serde_json::json!({"format_version":1,"membership_id":id.get(),"channel_id":native,"member_revision":2,
                "source_revision":1,"group_revision":1,"execution_revision":2,"status":1,"priority":channels[i].priority,
                "models_digest":format!("{:064x}",salt+i as i64+1),"retired_absent":false}).to_string();
            store.conn.execute("UPDATE operation_targets SET confirmed_at=?1,receipt_json=?2 WHERE operation_id=?3",params![NOW+50,json,op.as_str()]).unwrap();
            store.conn.execute("UPDATE memberships SET applied_revision=2,observed_status=1,observed_priority=desired_priority WHERE membership_id=?1",[id.get()]).unwrap();
        }
        store.conn.execute("UPDATE control_operations SET phase=?1,error_code=?2,updated_at=?3 WHERE operation_id=?4",params![phase,error,NOW+50,op.as_str()]).unwrap();
        store.conn.execute("UPDATE member_control SET dirty=?1,next_attempt_at=?2,last_error_code=?3 WHERE membership_id=?4",params![mode!=0,NOW+90+i as i64,error,id.get()]).unwrap();
    }
    // 新操作不能覆盖较早成功收据；它和旧执行记录一起经过 v4 迁移。
    store
        .submit_operation(
            OperationId::new("b5-new-after-success".into()).unwrap(),
            ControlCommand::EnsureMember {
                membership_id: receipt.membership_ids[0],
                expected_revision: 2,
                status: StatusDirective::Preserve,
            },
            NOW + 100,
        )
        .unwrap();
    as_v3(&mut store);
    verify_schema(&store.conn, 3).unwrap();
    verify_core_data(&store.conn).unwrap();
    crate::catalog::control::verify_control(&store.conn).unwrap();
    crate::catalog::execution::verify_execution(&store.conn).unwrap();
    History {
        dir,
        store,
        volcano,
        policies,
        glm_groups: vec![receipt.group_id, extra.group_id],
        glm_members: vec![receipt.membership_ids, extra_members],
        volcano_groups,
        volcano_members,
        empty_group,
    }
}

/// 只按旧 prepared 行建立允许变化集；不调用生产恢复/投影算法。
fn assert_recovery(before: &Image, after: &Image, low: i64, high: i64) {
    let mut expected = before.clone();
    let mut actual = after.clone();
    for (index, row) in before["execution_attempts"].rows.iter().enumerate() {
        let table = &before["execution_attempts"];
        if cell(table, row, "state") != &Value::Text("prepared".into()) {
            continue;
        }
        let id = cell(table, row, "membership_id").clone();
        let started = integer(cell(table, row, "started_at"));
        let finished = integer(cell(
            &after["execution_attempts"],
            &after["execution_attempts"].rows[index],
            "finished_at",
        ));
        assert!((started.max(low)..=started.max(high)).contains(&finished));
        set(
            &mut expected,
            "execution_attempts",
            index,
            "state",
            Value::Text("uncertain".into()),
        );
        set(
            &mut expected,
            "execution_attempts",
            index,
            "error_code",
            Value::Text("transport_unknown".into()),
        );
        for image in [&mut expected, &mut actual] {
            set(
                image,
                "execution_attempts",
                index,
                "finished_at",
                Value::Integer(-1),
            );
        }
        let e = &before["member_execution"];
        let ei = row_index(e, "membership_id", &id);
        set(
            &mut expected,
            "member_execution",
            ei,
            "sequence",
            Value::Integer(integer(cell(e, &e.rows[ei], "sequence")) + 1),
        );
        let c = &before["member_control"];
        let ci = row_index(c, "membership_id", &id);
        let revision = cell(c, &c.rows[ci], "revision");
        set(
            &mut expected,
            "member_control",
            ci,
            "dirty",
            Value::Integer(1),
        );
        set(
            &mut expected,
            "member_control",
            ci,
            "next_attempt_at",
            Value::Integer(0),
        );
        for target in &before["operation_targets"].rows {
            let t = &before["operation_targets"];
            if cell(t, target, "membership_id") != &id
                || cell(t, target, "target_member_revision") != revision
            {
                continue;
            }
            let o = &before["control_operations"];
            let oi = row_index(o, "operation_id", cell(t, target, "operation_id"));
            if [
                Value::Text("succeeded".into()),
                Value::Text("superseded".into()),
            ]
            .contains(cell(o, &o.rows[oi], "phase"))
            {
                continue;
            }
            let timestamp = integer(cell(
                &after["control_operations"],
                &after["control_operations"].rows[oi],
                "updated_at",
            ));
            assert!((low..=high).contains(&timestamp));
            set(
                &mut expected,
                "control_operations",
                oi,
                "phase",
                Value::Text("unknown".into()),
            );
            set(
                &mut expected,
                "control_operations",
                oi,
                "error_code",
                Value::Text("transport_unknown".into()),
            );
            for image in [&mut expected, &mut actual] {
                set(
                    image,
                    "control_operations",
                    oi,
                    "updated_at",
                    Value::Integer(-1),
                );
            }
        }
    }
    assert_images(&actual, &expected);
}

fn calendar(now: i64, p: &VolcanoPolicy) -> (i64, i64) {
    let mut candidates: Vec<_> = (now / DAY_SECONDS - 2..=now / DAY_SECONDS + 2)
        .map(|day| day * DAY_SECONDS + p.reset_minute * 60 - p.utc_offset_minutes * 60)
        .collect();
    candidates.sort_unstable();
    (
        *candidates.iter().filter(|n| **n <= now).max().unwrap(),
        *candidates.iter().filter(|n| **n > now).min().unwrap(),
    )
}

fn migration_case(
    n: usize,
    nv: usize,
    salt: i64,
    offset: i64,
    prepared: bool,
    corrupt: u8,
) -> Duration {
    let History {
        dir,
        mut store,
        volcano,
        policies,
        glm_groups,
        glm_members,
        volcano_groups,
        volcano_members,
        empty_group,
    } = history(n, nv, salt, offset, prepared);
    let before = history_image(&store.conn);
    let identity = instance(&store.conn);
    let old_snapshot = store.snapshot().unwrap();
    drop(store);
    let low = epoch();
    let started = Instant::now();
    store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let elapsed = started.elapsed();
    let high = epoch();
    let migrated = history_image(&store.conn);
    assert_recovery(&before, &migrated, low, high);
    assert_eq!(store.snapshot().unwrap(), old_snapshot);
    assert_eq!(instance(&store.conn), identity);
    verify_schema(&store.conn, DATABASE_SCHEMA_VERSION).unwrap();
    let files = backups_v3(&dir);
    assert_eq!(files.len(), 1);
    assert_eq!(
        fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let backup = Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    verify_schema(&backup, 3).unwrap();
    assert_eq!(history_image(&backup), before);
    assert_eq!(instance(&backup), identity);
    drop(backup);
    let backup_bytes = fs::read(&files[0]).unwrap();
    for id in &volcano {
        assert_eq!(store.load_source_policy(*id).unwrap(), None);
    }
    for group in &old_snapshot.groups {
        let default = match group.kind {
            SourceKind::Glm => GroupPolicyState::Glm(GlmGroupState::default()),
            SourceKind::Volcano => GroupPolicyState::Volcano(VolcanoGroupState::default()),
        };
        assert_eq!(
            store.load_group_policy(group.group_id).unwrap(),
            StoredGroupPolicy {
                group_id: group.group_id,
                policy_revision: 1,
                state: default
            }
        );
    }
    // 每个来源的字段由独立输入计算，不拿 apply/load 的结果充当 oracle。
    let mut runtimes = Vec::new();
    for (j, (id, p)) in volcano.iter().zip(&policies).enumerate() {
        if j % 4 == 3 {
            runtimes.push(None);
            continue;
        }
        let (start, end) = calendar(NOW, p);
        let mut expected = VolcanoRuntime {
            source_id: *id,
            revision: 1,
            period_id: 1,
            period_start: start,
            next_reset_at: end,
            used_input: 0,
            used_output: 0,
            consecutive_failures: 0,
            error_exhausted: false,
            limit_exhausted: false,
            error_epoch: 1,
            error_cutoff_at: None,
            evidence_cursor: None,
            coverage_end: None,
        };
        assert_eq!(
            store.initialize_source_policy(*id, 1, NOW).unwrap(),
            expected
        );
        let input = if j % 2 == 0 {
            p.daily_token_limit.unwrap()
        } else {
            (j as i64 + 1) * 7
        };
        let output = j as i64 + 2;
        let failures = if j % 3 == 2 { 0 } else { p.failure_threshold };
        let evidence = VolcanoEvidence {
            source_id: *id,
            period_id: 1,
            error_epoch: 1,
            from_cursor: None,
            to_cursor: 200 + j as i64,
            used_input: input,
            used_output: output,
            coverage_end: NOW + 11,
            outcomes: vec![
                OutcomeObservation {
                    first_event_at: NOW + 1,
                    observed_at: NOW + 10,
                    kind: OutcomeKind::Unavailable
                };
                failures as usize
            ],
        };
        expected.revision = 2;
        expected.used_input = input;
        expected.used_output = output;
        expected.consecutive_failures = failures;
        expected.error_exhausted = failures >= p.failure_threshold;
        expected.limit_exhausted = input + output >= p.daily_token_limit.unwrap();
        expected.evidence_cursor = Some(200 + j as i64);
        expected.coverage_end = Some(NOW + 11);
        assert_eq!(
            store
                .apply_source_policy(*id, 1, 1, SourcePolicyUpdate::Evidence(evidence), NOW + 10)
                .unwrap()
                .runtime,
            expected
        );
        if (salt + j as i64) % 2 == 0 {
            expected.revision = 3;
            expected.consecutive_failures = 0;
            expected.error_exhausted = false;
            expected.error_epoch = 2;
            expected.error_cutoff_at = Some(NOW + 11);
            expected.evidence_cursor = None;
            assert_eq!(
                store
                    .apply_source_policy(*id, 1, 2, SourcePolicyUpdate::ClearError, NOW + 11)
                    .unwrap()
                    .runtime,
                expected
            );
        }
        runtimes.push(Some(expected));
    }
    let mut group_states = Vec::new();
    for (i, g) in glm_groups.iter().enumerate() {
        let members = &glm_members[i];
        let index = (salt as usize + i) % members.len();
        let state = GroupPolicyState::Glm(GlmGroupState {
            active: Some(members[index]),
            pinned: Some(
                members[if i == 1 {
                    0
                } else {
                    (index + 1) % members.len()
                }],
            ),
            regime: if (salt + i as i64) % 2 == 0 {
                Regime::Degraded
            } else {
                Regime::Normal
            },
        });
        let expected = StoredGroupPolicy {
            group_id: *g,
            policy_revision: 2,
            state: state.clone(),
        };
        assert_eq!(store.save_group_policy(*g, 1, 1, state).unwrap(), expected);
        group_states.push(expected);
    }
    for (i, g) in volcano_groups.iter().enumerate() {
        let index = (salt as usize + i) % nv;
        let id = volcano_members[i][index];
        let state = GroupPolicyState::Volcano(VolcanoGroupState {
            preferred: if i == 0 { Some(id) } else { None },
            cursor: Some((index as i64, id)),
        });
        let expected = StoredGroupPolicy {
            group_id: *g,
            policy_revision: 2,
            state: state.clone(),
        };
        assert_eq!(store.save_group_policy(*g, 1, 1, state).unwrap(), expected);
        group_states.push(expected);
    }
    group_states.push(StoredGroupPolicy {
        group_id: empty_group,
        policy_revision: 1,
        state: GroupPolicyState::Volcano(VolcanoGroupState::default()),
    });
    assert_eq!(history_image(&store.conn), migrated); // 新策略保存不是旧控制/执行记录重写。
    for _ in 0..2 {
        drop(store);
        store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        assert_eq!(history_image(&store.conn), migrated); // prepared 已恢复，不再提升 sequence/操作时间。
        for (id, expected) in volcano.iter().zip(&runtimes) {
            assert_eq!(&store.load_source_policy(*id).unwrap(), expected);
        }
        for expected in &group_states {
            assert_eq!(
                store.load_group_policy(expected.group_id).unwrap(),
                *expected
            );
        }
        assert_eq!(backups_v3(&dir), files);
        assert_eq!(fs::read(&files[0]).unwrap(), backup_bytes);
    }
    // 各次运行带完整历史；坏状态打开失败后逐字段比较，不能在拒绝时顺便修补。
    match corrupt {
        0 => {
            store
                .conn
                .execute(
                    "DELETE FROM volcano_runtime WHERE source_id=?1",
                    [volcano[0].get()],
                )
                .unwrap();
        }
        1 => {
            store
                .conn
                .execute(
                    "DELETE FROM group_policy_state WHERE group_id=?1",
                    [glm_groups[0].get()],
                )
                .unwrap();
        }
        _ => {
            store.conn.execute("UPDATE group_policy_state SET active_member_id=?1,cursor_member_id=?1,cursor_order=0 WHERE group_id=?2",params![volcano_members[1][0].get(),volcano_groups[0].get()]).unwrap();
        }
    }
    let tables = [
        CORE,
        CONTROL,
        EXECUTION,
        &["catalog_meta", "volcano_runtime", "group_policy_state"],
    ]
    .concat();
    let corrupted = image_of(&store.conn, &tables);
    drop(store);
    assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
    let conn = Connection::open_with_flags(
        dir.path().join("catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(image_of(&conn, &tables), corrupted);
    assert_eq!(backups_v3(&dir), files);
    elapsed
}

#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_complex_history_migration() {
    let elapsed = migration_case(6, 5, 77, 480, true, 2);
    println!("COMPLEX_HISTORY_MIGRATION_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn b5_complex_history_migration_and_policy_reopen(
        n in 4usize..7,nv in 2usize..6,salt in 0i64..10000,offset in -720i64..=840,
        prepared in any::<bool>(),corrupt in 0u8..3
    ) {
        migration_case(n,nv,salt,offset,prepared,corrupt);
    }
}

#[test]
fn b5_boundary_matrix_covers_empty_uninitialized_and_all_corruptions() {
    for (salt, offset, prepared, corrupt) in [
        (0, -720, true, 0),
        (1439, 840, false, 1),
        (77, 480, true, 2),
    ] {
        migration_case(4, 4, salt, offset, prepared, corrupt);
    }
}

#[test]
fn b5_complex_v3_ddl_failure_preserves_every_old_field_and_retry_backup() {
    for collision in ["volcano_runtime", "group_policy_state"] {
        let History { dir, mut store, .. } = history(4, 2, 27, 480, true);
        let before = history_image(&store.conn);
        let identity = instance(&store.conn);
        store.conn.execute_batch(&format!("CREATE TABLE {collision}(marker TEXT); INSERT INTO {collision} VALUES('owned-fixture');")).unwrap();
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
        let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
        verify_schema(&conn, 3).unwrap();
        assert_eq!(history_image(&conn), before);
        assert_eq!(instance(&conn), identity);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row(&format!("SELECT marker FROM {collision}"), [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "owned-fixture"
        );
        let files = backups_v3(&dir);
        assert_eq!(files.len(), 1);
        let bytes = fs::read(&files[0]).unwrap();
        let backup =
            Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(history_image(&backup), before);
        verify_schema(&backup, 3).unwrap();
        drop(backup);
        conn.execute(&format!("DROP TABLE {collision}"), [])
            .unwrap();
        drop(conn);
        let low = epoch();
        store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        let high = epoch();
        assert_recovery(&before, &history_image(&store.conn), low, high);
        assert_eq!(backups_v3(&dir).len(), 2);
        assert_eq!(fs::read(&files[0]).unwrap(), bytes);
    }
}

#[test]
fn b5_final_migration_failure_keeps_original_v1_v2_backup_and_valid_v3() {
    for version in [1, 2] {
        let History { dir, mut store, .. } = history(4, 2, 31, -60, false);
        // 在私有 fixture 中移除高版本侧表，冻结旧 DDL 和原字段不变。
        let tx = store.conn.transaction().unwrap();
        let identity = instance(&tx);
        tx.execute_batch(
            "DROP TABLE execution_attempts; DROP TABLE member_execution; DROP TABLE catalog_meta;",
        )
        .unwrap();
        if version == 1 {
            tx.execute_batch("DROP TABLE operation_targets; DROP TABLE control_operations; DROP TABLE member_control; DROP TABLE source_control;").unwrap();
            tx.execute_batch(SCHEMA.split(';').next().unwrap()).unwrap();
            tx.execute("INSERT INTO catalog_meta VALUES(1,1)", [])
                .unwrap();
        } else {
            tx.execute_batch(SCHEMA_V2.split(';').next().unwrap())
                .unwrap();
            tx.execute("INSERT INTO catalog_meta VALUES(1,2,?1)", [&identity])
                .unwrap();
        }
        tx.pragma_update(None, "user_version", version).unwrap();
        tx.commit().unwrap();
        let tables = if version == 1 {
            CORE.to_vec()
        } else {
            [CORE, CONTROL].concat()
        };
        let before = image_of(&store.conn, &tables);
        store.conn.execute_batch("CREATE TABLE volcano_runtime(marker TEXT); INSERT INTO volcano_runtime VALUES('owned-fixture');").unwrap();
        drop(store);
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
        let conn = Connection::open_with_flags(
            dir.path().join("catalog.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        verify_schema(&conn, 3).unwrap();
        verify_core_data(&conn).unwrap();
        crate::catalog::control::verify_control(&conn).unwrap();
        crate::catalog::execution::verify_execution(&conn).unwrap();
        if version == 2 {
            assert_eq!(instance(&conn), identity);
        }
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(image_of(&conn, &tables), before);
        assert_eq!(
            conn.query_row("SELECT marker FROM volcano_runtime", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "owned-fixture"
        );
        let files = backup_files(&dir, version);
        assert_eq!(files.len(), 1);
        assert!(backups_v3(&dir).is_empty());
        assert_eq!(
            fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let backup =
            Connection::open_with_flags(&files[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        verify_schema(&backup, version).unwrap();
        if version == 2 {
            assert_eq!(instance(&backup), identity);
        }
        assert_eq!(image_of(&backup, &tables), before);
        assert_eq!(
            backup
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            version
        );
    }
}
