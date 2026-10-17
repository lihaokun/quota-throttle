use super::*;
use crate::catalog::control::{ControlCommand, OperationId, StatusDirective};
use crate::catalog::ledger::{PageEnd, ScanLane, ScanWindow};
use crate::catalog::policy::{GroupPolicyState, SourcePolicyUpdate};
use crate::newapi::managed_channels::NativeRecoveryCheck;
use crate::reconcile::planner::{BackendReadiness, SUPPORTED_BACKEND_VERSION};
use crate::source_policy::volcano::{VolcanoGroupState, VolcanoRuntime};
use serde_json::{json, Value};
const NOW: i64 = 180_000;
#[path = "observed_publication_properties.rs"]
mod properties;
#[path = "observed_publication_scale.rs"]
mod scale;
struct Fixture {
    dir: TempDir,
    store: CatalogStore,
    sources: Vec<SourceId>,
    groups: Vec<GroupId>,
    members: Vec<MembershipId>,
    cfg: Config,
}
fn fixture() -> Fixture {
    let (dir, mut store) = setup();
    store
        .create_source(source("fake-o4-glm-untouched", SourceKind::Glm))
        .unwrap();
    let sources = (0..2)
        .map(|i| {
            store
                .create_source(source(&format!("fake-o4-source-{i}"), SourceKind::Volcano))
                .unwrap()
                .source_id
        })
        .collect::<Vec<_>>();
    let groups = (0..2)
        .map(|i| {
            store
                .create_group(group(&format!("o4-group-{i}"), SourceKind::Volcano))
                .unwrap()
                .group_id
        })
        .collect::<Vec<_>>();
    let cfg: Config =
        toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\n").unwrap();
    let mut members = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        for (s, source) in sources.iter().enumerate() {
            let m = store
                .plan_membership(
                    *source,
                    *group,
                    if g == s {
                        cfg.priority_active
                    } else {
                        cfg.priority_standby
                    },
                )
                .unwrap()
                .membership_id;
            let channel = 1001 + members.len() as i64;
            store.conn.execute("UPDATE memberships SET channel_id=?1,lifecycle='imported' WHERE membership_id=?2",params![channel,m.get()]).unwrap();
            store
                .conn
                .execute(
                    "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
                    [m.get()],
                )
                .unwrap();
            store
                .submit_operation(
                    OperationId::new(format!("o4-initial-{:016}", m.get())).unwrap(),
                    ControlCommand::EnsureMember {
                        membership_id: m,
                        expected_revision: 1,
                        status: StatusDirective::Enabled,
                    },
                    NOW,
                )
                .unwrap();
            members.push(m);
        }
    }
    store
        .conn
        .execute("UPDATE source_groups SET ready=1", [])
        .unwrap();
    store
        .save_group_policy(
            groups[1],
            1,
            1,
            GroupPolicyState::Volcano(VolcanoGroupState {
                preferred: Some(members[3]),
                cursor: Some((1, members[3])),
            }),
        )
        .unwrap();
    store.initialize_ledger(0, NOW).unwrap();
    ingest(&mut store, &[], NOW);
    let mut f = Fixture {
        dir,
        store,
        sources,
        groups,
        members,
        cfg,
    };
    round(&mut f, NOW);
    f
}
fn backend(at: i64) -> BackendReadiness {
    BackendReadiness::from_preflight(
        SUPPORTED_BACKEND_VERSION,
        NativeRecoveryCheck::Disabled,
        true,
        at,
        at + 100,
    )
}
fn round(f: &mut Fixture, at: i64) -> crate::catalog::volcano_publication::VolcanoPolicyCommit {
    f.store
        .commit_volcano_round(&f.cfg, backend(at), &HashMap::new(), at)
        .unwrap()
}
fn ingest(store: &mut CatalogStore, rows: &[Value], at: i64) {
    let old = store
        .load_ledger_state()
        .unwrap()
        .scans
        .into_iter()
        .find(|s| s.lane == ScanLane::Recent)
        .unwrap();
    let t = store
        .begin_scan(
            ScanLane::Recent,
            old.generation,
            ScanWindow { from: 0, to: at },
            at,
        )
        .unwrap();
    store
        .commit_fact_page(&t, rows, PageEnd::ReachedEnd, at)
        .unwrap();
}
fn event(id: i64, channel: i64, request: &str, success: bool, at: i64) -> Value {
    json!({"id":id,"type":if success{2}else{5},"created_at":at,"channel":channel,"token_id":77,"model_name":"fake-model","group":if channel<=1002{"o4-group-0"}else{"o4-group-1"},"prompt_tokens":if success{5}else{0},"completion_tokens":if success{3}else{0},"is_stream":false,"request_id":request,"other":if success{String::new()}else{json!({"status_code":503,"error_type":"openai_error","error_code":"unavailable"}).to_string()}})
}
fn runtime(f: &Fixture, index: usize) -> VolcanoRuntime {
    f.store
        .load_source_policy(f.sources[index])
        .unwrap()
        .unwrap()
}
fn queue(s: &CatalogStore) -> i64 {
    s.conn
        .query_row("SELECT count(*) FROM volcano_observation_queue", [], |r| {
            r.get(0)
        })
        .unwrap()
}
fn image(conn: &Connection) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    [
        "sources",
        "source_secrets",
        "source_groups",
        "memberships",
        "source_control",
        "member_control",
        "member_execution",
        "execution_attempts",
        "control_operations",
        "operation_targets",
        "volcano_runtime",
        "group_policy_state",
        "ledger_state",
        "ledger_checkpoints",
        "ledger_facts",
        "ledger_minute_usage",
        "volcano_observation_queue",
        "volcano_request_observations",
        "volcano_period_history",
        "sqlite_sequence",
    ]
    .into_iter()
    .map(|table| {
        let mut q = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let width = q.column_count();
        let rows = q
            .query_map([], |r| (0..width).map(|i| r.get(i)).collect())
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        (table.into(), rows)
    })
    .collect()
}

#[test]
fn observed_publication_persistent_success_and_retries_do_not_recount_after_reopen() {
    let mut f = fixture();
    ingest(
        &mut f.store,
        &[
            event(1, 1001, "a", false, NOW - 1),
            event(2, 1003, "b", false, NOW - 1),
        ],
        NOW,
    );
    assert_eq!(queue(&f.store), 2);
    let r = round(&mut f, NOW);
    assert_eq!(r.report.observations, 2);
    assert_eq!(runtime(&f, 0).consecutive_failures, 2);
    ingest(
        &mut f.store,
        &[
            event(3, 1003, "a", true, NOW),
            event(4, 1001, "c", false, NOW),
        ],
        NOW + 1,
    );
    round(&mut f, NOW + 1);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    assert!(!runtime(&f, 0).error_exhausted);
    assert_eq!(runtime(&f, 0).used_input + runtime(&f, 0).used_output, 8);
    let before = image(&f.store.conn);
    let path = f.dir.path().join("catalog.sqlite3");
    drop(f.store);
    f.store = CatalogStore::open(&path).unwrap();
    assert_eq!(image(&f.store.conn), before);
    ingest(
        &mut f.store,
        &[
            event(3, 1003, "a", true, NOW),
            event(4, 1001, "c", false, NOW),
        ],
        NOW + 2,
    );
    assert_eq!(queue(&f.store), 0);
    // 单独验证重复成功，避免随后重复失败错误加一掩盖一次错误清零。
    ingest(&mut f.store, &[event(5, 1003, "a", true, NOW + 1)], NOW + 2);
    round(&mut f, NOW + 2);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    ingest(
        &mut f.store,
        &[
            event(5, 1003, "a", true, NOW + 1),
            event(6, 1001, "c", false, NOW + 1),
        ],
        NOW + 2,
    );
    round(&mut f, NOW + 2);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    assert_eq!(runtime(&f, 0).used_input + runtime(&f, 0).used_output, 16);
    assert_eq!(runtime(&f, 1).consecutive_failures, 0);
}

#[test]
fn observed_publication_shared_source_exhausts_both_groups_without_changing_other_selection() {
    let mut f = fixture();
    let group1 = f.store.load_group_policy(f.groups[1]).unwrap();
    ingest(
        &mut f.store,
        &(1..=3)
            .map(|id| {
                event(
                    id,
                    if id == 2 { 1003 } else { 1001 },
                    &format!("f{id}"),
                    false,
                    NOW - 1,
                )
            })
            .collect::<Vec<_>>(),
        NOW,
    );
    let r = round(&mut f, NOW);
    assert!(runtime(&f, 0).error_exhausted);
    assert_eq!(r.report.permitted, 2);
    assert_eq!(r.report.changed_members, 3);
    for i in [0, 2] {
        let w = f.store.load_work(f.members[i], false).unwrap();
        assert_eq!(w.target.status_directive, StatusDirective::Disabled);
        assert_eq!(w.target.desired_priority, f.cfg.priority_exhausted);
    }
    assert_eq!(f.store.load_group_policy(f.groups[1]).unwrap(), group1);
    assert!(
        matches!(f.store.load_group_policy(f.groups[0]).unwrap().state,GroupPolicyState::Volcano(s) if s.preferred==Some(f.members[1]))
    );
    ingest(
        &mut f.store,
        &[event(4, 1001, "recovered-stream", true, NOW)],
        NOW + 1,
    );
    round(&mut f, NOW + 1);
    assert_eq!(runtime(&f, 0).consecutive_failures, 0);
    assert!(runtime(&f, 0).error_exhausted);
    assert_eq!(
        f.store
            .load_work(f.members[0], false)
            .unwrap()
            .target
            .status_directive,
        StatusDirective::Disabled
    );
}

#[test]
fn observed_publication_historical_usage_zero_tokens_and_boundaries_are_exact() {
    let mut f = fixture();
    let mut old = event(1, 1001, "historical-success", true, NOW - 1000);
    old["prompt_tokens"] = json!(11);
    old["completion_tokens"] = json!(7);
    let mut zero = event(2, 1001, "empty", true, NOW - 1);
    zero["prompt_tokens"] = json!(0);
    zero["completion_tokens"] = json!(0);
    ingest(
        &mut f.store,
        &[
            old,
            zero,
            event(i64::MAX, 1001, "real-failure", false, NOW - 1),
        ],
        NOW,
    );
    assert_eq!(queue(&f.store), 1);
    round(&mut f, NOW);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    assert_eq!(
        (runtime(&f, 0).used_input, runtime(&f, 0).used_output),
        (11, 7)
    );
    let before = image(&f.store.conn);
    round(&mut f, NOW);
    assert_eq!(image(&f.store.conn), before);
    let p = VolcanoPolicy {
        daily_token_limit: Some(18),
        ..Default::default()
    };
    let current = runtime(&f, 0);
    f.store
        .apply_source_policy(
            f.sources[0],
            1,
            current.revision,
            SourcePolicyUpdate::ReplacePolicy(p),
            NOW,
        )
        .unwrap();
    round(&mut f, NOW);
    assert!(runtime(&f, 0).limit_exhausted);
}

#[test]
fn observed_publication_late_source_binding_enqueues_once() {
    let mut f = fixture();
    let mut row = event(1, 9999, "binding-late", false, NOW - 1);
    row["group"] = json!("o4-group-0");
    ingest(&mut f.store, &[row], NOW);
    assert_eq!(queue(&f.store), 0);
    f.store.conn.execute("INSERT INTO memberships(source_id,group_id,kind,channel_id,position,lifecycle,desired_priority,retired_at) VALUES(?1,?2,'volcano',9999,0,'retired',10,10)",params![f.sources[0].get(),f.groups[0].get()]).unwrap();
    let id = f.store.conn.last_insert_rowid();
    let instance: String = f
        .store
        .conn
        .query_row("SELECT catalog_instance_id FROM catalog_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    f.store.conn.execute("INSERT INTO member_control(membership_id,revision,status_directive,retire_requested,create_state,create_name,dirty,next_attempt_at) VALUES(?1,1,'preserve',0,'resolved',?2,0,0)",params![id,format!("qt-{instance}-m{id}")]).unwrap();
    f.store
        .conn
        .execute("INSERT INTO member_execution VALUES(?1,0,0,0,0)", [id])
        .unwrap();
    let pass = f.store.begin_attribution_pass().unwrap();
    let before = image(&f.store.conn);
    f.store.conn.execute_batch("CREATE TRIGGER enqueue_fault BEFORE INSERT ON volcano_observation_queue BEGIN SELECT RAISE(ABORT,'synthetic'); END").unwrap();
    assert!(f.store.reattribute_batch(&pass).is_err());
    assert_eq!(image(&f.store.conn), before);
    f.store
        .conn
        .execute_batch("DROP TRIGGER enqueue_fault")
        .unwrap();
    assert_eq!(f.store.reattribute_batch(&pass).unwrap().changed, 1);
    assert_eq!(queue(&f.store), 1);
    round(&mut f, NOW);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    f.store.reattribute_batch(&pass).unwrap();
    assert_eq!(queue(&f.store), 0);
}

#[test]
fn observed_publication_old_queued_and_unqueued_failures_fence_later_success() {
    for queued in [false, true] {
        let mut f = fixture();
        let old_at = if queued { NOW - 1 } else { NOW - 1000 };
        ingest(&mut f.store, &[event(1, 1001, "old", false, old_at)], NOW);
        assert_eq!(queue(&f.store), i64::from(queued));
        let r = runtime(&f, 0);
        f.store
            .apply_source_policy(
                f.sources[0],
                1,
                r.revision,
                SourcePolicyUpdate::ClearError,
                NOW,
            )
            .unwrap();
        ingest(
            &mut f.store,
            &[
                event(2, 1001, "new", false, NOW + 1),
                event(3, 1001, "old", true, NOW + 2),
            ],
            NOW + 3,
        );
        round(&mut f, NOW + 3);
        assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    }
}

#[test]
fn observed_publication_reset_history_and_old_request_isolation_are_atomic() {
    let mut f = fixture();
    let before = runtime(&f, 0);
    let due = before.next_reset_at;
    ingest(&mut f.store, &[event(1, 1001, "old", false, NOW - 1)], NOW);
    ingest(
        &mut f.store,
        &[
            event(2, 1001, "new", false, due + 1),
            event(3, 1001, "old", true, due + 2),
        ],
        due + 3,
    );
    round(&mut f, due + 3);
    let r = runtime(&f, 0);
    assert_eq!(r.period_id, before.period_id + 1);
    assert_eq!(r.consecutive_failures, 1);
    assert_eq!(r.used_input + r.used_output, 8);
    assert_eq!(
        f.store
            .conn
            .query_row("SELECT count(*) FROM volcano_period_history", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    let image_before = image(&f.store.conn);
    round(&mut f, due + 3);
    assert_eq!(image(&f.store.conn), image_before);
}

#[test]
fn observed_publication_batches_stale_data_and_preview_do_not_grant_permission() {
    let mut f = fixture();
    ingest(
        &mut f.store,
        &(1..=17)
            .map(|id| event(id, 1001, "same", true, NOW - 1))
            .collect::<Vec<_>>(),
        NOW,
    );
    let first = round(&mut f, NOW);
    assert_eq!(first.report.observations, 16);
    assert_eq!(first.report.pending_sources, 1);
    assert!(!first.permits.contains_key(&f.members[0]));
    assert_eq!(queue(&f.store), 1);
    let last = round(&mut f, NOW);
    assert_eq!(last.report.permitted, 4);
    assert_eq!(queue(&f.store), 0);
    assert_eq!(
        runtime(&f, 0).used_input + runtime(&f, 0).used_output,
        17 * 8
    );
    let before = image(&f.store.conn);
    let preview = f.store.preview_volcano_round(&f.cfg, NOW + 86400).unwrap();
    assert!(preview.preview_only);
    assert_eq!(preview.permitted, 0);
    assert_eq!(image(&f.store.conn), before);
    let stale = round(&mut f, NOW + 30);
    assert_eq!(stale.report.permitted, 0);
}

#[test]
fn observed_publication_faults_roll_back_every_semantic_table() {
    for table in [
        "volcano_request_observations",
        "volcano_observation_queue",
        "volcano_runtime",
        "group_policy_state",
        "member_control",
        "volcano_period_history",
    ] {
        let mut f = fixture();
        ingest(
            &mut f.store,
            &(1..=3)
                .map(|id| event(id, 1001, &format!("e{id}"), false, NOW - 1))
                .collect::<Vec<_>>(),
            NOW,
        );
        let before = image(&f.store.conn);
        let action = match table {
            "volcano_request_observations" | "volcano_period_history" => "INSERT",
            "volcano_observation_queue" => "DELETE",
            _ => "UPDATE",
        };
        f.store.conn.execute_batch(&format!("CREATE TRIGGER fault BEFORE {action} ON {table} BEGIN SELECT RAISE(ABORT,'synthetic'); END;")).unwrap();
        let at = if table == "volcano_period_history" {
            runtime(&f, 0).next_reset_at
        } else {
            NOW
        };
        assert!(
            f.store
                .commit_volcano_round(&f.cfg, backend(at), &HashMap::new(), at)
                .is_err(),
            "{table}"
        );
        assert_eq!(image(&f.store.conn), before, "{table}");
        assert!(!f.store.poisoned);
    }
}

#[test]
fn observed_publication_unknown_commit_requires_reopen_and_replays_once() {
    let mut f = fixture();
    ingest(&mut f.store, &[event(1, 1001, "one", false, NOW - 1)], NOW);
    let before = image(&f.store.conn);
    f.store.conn.execute_batch("CREATE TABLE parent_fault(id INTEGER PRIMARY KEY); CREATE TABLE child_fault(id INTEGER REFERENCES parent_fault(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fault AFTER UPDATE ON volcano_runtime BEGIN INSERT INTO child_fault VALUES(1); END;").unwrap();
    assert!(f
        .store
        .commit_volcano_round(&f.cfg, backend(NOW), &HashMap::new(), NOW)
        .is_err());
    assert!(f.store.poisoned);
    assert_eq!(image(&f.store.conn), before);
    f.store
        .conn
        .execute_batch("DROP TRIGGER fault; DROP TABLE child_fault; DROP TABLE parent_fault;")
        .unwrap();
    let path = f.dir.path().join("catalog.sqlite3");
    drop(f.store);
    f.store = CatalogStore::open(&path).unwrap();
    round(&mut f, NOW);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    round(&mut f, NOW);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
}

#[test]
fn observed_publication_sequence_exhaustion_rolls_back_fact_page() {
    let mut f = fixture();
    ingest(
        &mut f.store,
        &[event(1, 1001, "first", false, NOW - 1)],
        NOW,
    );
    round(&mut f, NOW);
    f.store
        .conn
        .execute(
            "UPDATE sqlite_sequence SET seq=?1 WHERE name='volcano_observation_queue'",
            [JSON_SAFE],
        )
        .unwrap();
    let generation = f
        .store
        .load_ledger_state()
        .unwrap()
        .scans
        .into_iter()
        .find(|s| s.lane == ScanLane::Recent)
        .unwrap()
        .generation;
    let t = f
        .store
        .begin_scan(
            ScanLane::Recent,
            generation,
            ScanWindow {
                from: 0,
                to: NOW + 1,
            },
            NOW + 1,
        )
        .unwrap();
    let before = image(&f.store.conn);
    assert!(f
        .store
        .commit_fact_page(
            &t,
            &[event(2, 1001, "second", true, NOW)],
            PageEnd::ReachedEnd,
            NOW + 1
        )
        .is_err());
    assert_eq!(image(&f.store.conn), before);
}

#[test]
fn observed_publication_future_queue_and_ineligible_members_never_authorize() {
    let mut f = fixture();
    ingest(
        &mut f.store,
        &[event(1, 1001, "future", false, NOW + 1)],
        NOW + 2,
    );
    let before = queue(&f.store);
    let r = round(&mut f, NOW);
    assert_eq!(queue(&f.store), before);
    assert_eq!(r.report.permitted, 0); // recent observation itself is future too.
    assert_eq!(runtime(&f, 0).consecutive_failures, 0);
    round(&mut f, NOW + 2);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    f.store
        .conn
        .execute(
            "UPDATE source_groups SET ready=0 WHERE group_id=?1",
            [f.groups[0].get()],
        )
        .unwrap();
    f.store
        .conn
        .execute(
            "UPDATE sources SET manual_paused=1 WHERE source_id=?1",
            [f.sources[0].get()],
        )
        .unwrap();
    f.store
        .conn
        .execute(
            "DELETE FROM operation_targets WHERE membership_id=?1",
            [f.members[3].get()],
        )
        .unwrap();
    // Keep operation bookkeeping valid while removing management ownership.
    f.store
        .conn
        .execute(
            "DELETE FROM control_operations WHERE target_id=?1 AND target_type='membership'",
            [f.members[3].get()],
        )
        .unwrap();
    assert_eq!(round(&mut f, NOW + 2).report.permitted, 0);
}

#[test]
fn observed_publication_unsupported_upstream_is_not_queued_or_enabled() {
    let mut f = fixture();
    f.store.conn.execute("UPDATE sources SET settings_json=json_set(settings_json,'$.settings.upstream.channel_type',1) WHERE source_id=?1",[f.sources[0].get()]).unwrap();
    ingest(
        &mut f.store,
        &[event(1, 1001, "unsupported", false, NOW - 1)],
        NOW,
    );
    assert_eq!(queue(&f.store), 0);
    let r = round(&mut f, NOW);
    assert!(!r.permits.contains_key(&f.members[0]));
    assert!(!r.permits.contains_key(&f.members[2]));
    assert_eq!(runtime(&f, 0).consecutive_failures, 0);
}

#[test]
fn observed_publication_group_only_attribution_does_not_reenqueue() {
    let mut f = fixture();
    let mut row = event(1, 1001, "same", false, NOW - 1);
    row["group"] = json!("late-group");
    ingest(&mut f.store, &[row], NOW);
    assert_eq!(queue(&f.store), 1);
    round(&mut f, NOW);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
    f.store
        .conn
        .execute(
            "UPDATE source_groups SET newapi_group='late-group' WHERE group_id=?1",
            [f.groups[0].get()],
        )
        .unwrap();
    let pass = f.store.begin_attribution_pass().unwrap();
    assert_eq!(f.store.reattribute_batch(&pass).unwrap().changed, 1);
    assert_eq!(queue(&f.store), 0);
    round(&mut f, NOW);
    assert_eq!(runtime(&f, 0).consecutive_failures, 1);
}

#[test]
fn observed_publication_second_group_failure_rolls_back_prior_group_targets() {
    for table in ["memberships", "member_execution"] {
        let mut f = fixture();
        ingest(
            &mut f.store,
            &(1..=3)
                .map(|id| event(id, 1001, &format!("f{id}"), false, NOW - 1))
                .collect::<Vec<_>>(),
            NOW,
        );
        let at = if table == "member_execution" {
            round(&mut f, NOW);
            let due = runtime(&f, 0).next_reset_at;
            ingest(&mut f.store, &[], due);
            due
        } else {
            NOW
        };
        let before = image(&f.store.conn);
        f.store.conn.execute_batch(&format!("CREATE TRIGGER second_group_fault BEFORE UPDATE ON {table} WHEN OLD.membership_id={} BEGIN SELECT RAISE(ABORT,'synthetic'); END",f.members[2].get())).unwrap();
        assert!(
            f.store
                .commit_volcano_round(&f.cfg, backend(at), &HashMap::new(), at)
                .is_err(),
            "{table}"
        );
        assert_eq!(image(&f.store.conn), before);
        f.store
            .conn
            .execute_batch("DROP TRIGGER second_group_fault")
            .unwrap();
        round(&mut f, at);
        assert_ne!(image(&f.store.conn), before);
    }
}
