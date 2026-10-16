use super::*;
use crate::catalog::control::{
    ControlCommand, ControlErrorCode as Code, ControlResult, OperationId, StatusDirective,
};
use crate::catalog::glm_round::*;
use crate::catalog::policy::{GroupPolicyState, StoredGroupPolicy};
use crate::orchestrator::Regime;
use crate::quota::{QuotaStatus, WindowStatus};
use crate::source_policy::glm::GlmGroupState;
use crate::source_policy::probe::{ProbeCycle, QuotaObservation, QuotaReading};
use rusqlite::types::Value;
use std::time::{Duration, Instant};

#[path = "glm_publication_tests.rs"]
mod publication;

const NOW_MS: i64 = 1_800_000_000_000;
fn cfg() -> Config {
    toml::from_str(
        "poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'",
    )
    .unwrap()
}
fn operation(id: MembershipId) -> OperationId {
    OperationId::new(format!("glm-round-member-{:016}", id.get())).unwrap()
}
struct Fixture {
    dir: TempDir,
    store: CatalogStore,
    sources: Vec<SourceId>,
    groups: Vec<GroupId>,
    members: Vec<Vec<MembershipId>>,
}
fn fixture(n: usize, groups: usize) -> Fixture {
    let (dir, mut store) = setup();
    let volcano = store
        .create_source(source("fake-round-volcano", SourceKind::Volcano))
        .unwrap();
    for i in 0..2 {
        let g = store
            .create_group(group(&format!("round-v{i}"), SourceKind::Volcano))
            .unwrap();
        store
            .plan_membership(volcano.source_id, g.group_id, 31 + i)
            .unwrap();
    }
    let sources: Vec<_> = (0..n)
        .map(|i| {
            store
                .create_source(source(&format!("fake-round-glm-{i}"), SourceKind::Glm))
                .unwrap()
                .source_id
        })
        .collect();
    let mut group_ids = Vec::new();
    let mut members = Vec::new();
    for i in 0..groups {
        let g = store
            .create_group(group(&format!("round-g{i}"), SourceKind::Glm))
            .unwrap();
        let mut rows = Vec::new();
        for id in &sources {
            let m = store
                .plan_membership(*id, g.group_id, cfg().priority_standby)
                .unwrap();
            assert_ne!(m.membership_id.get(), id.get());
            store.conn.execute("UPDATE memberships SET channel_id=?1,lifecycle='ready',observed_status=1,observed_priority=?2 WHERE membership_id=?3",params![10000+m.membership_id.get()*17,cfg().priority_standby,m.membership_id.get()]).unwrap();
            store
                .conn
                .execute(
                    "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
                    [m.membership_id.get()],
                )
                .unwrap();
            store
                .submit_operation(
                    operation(m.membership_id),
                    ControlCommand::EnsureMember {
                        membership_id: m.membership_id,
                        expected_revision: 1,
                        status: StatusDirective::Preserve,
                    },
                    NOW_MS / 1000 - 10,
                )
                .unwrap();
            rows.push(m.membership_id);
        }
        group_ids.push(g.group_id);
        members.push(rows);
    }
    Fixture {
        dir,
        store,
        sources,
        groups: group_ids,
        members,
    }
}
fn cycle(sources: &[SourceId], values: &[Option<u8>]) -> ProbeCycle {
    ProbeCycle::test_input(
        1,
        NOW_MS,
        sources
            .iter()
            .zip(values)
            .map(|(id, p)| QuotaObservation {
                source_id: *id,
                config_revision: 1,
                cycle_id: 1,
                observed_at_ms: NOW_MS - 1,
                reading: p.map_or(QuotaReading::Unavailable, |p| {
                    QuotaReading::Available(QuotaStatus {
                        five_hour: Some(WindowStatus {
                            percentage: f64::from(p),
                            next_reset_time: NOW_MS + 3_600_000,
                        }),
                        weekly: None,
                        level: None,
                    })
                }),
            })
            .collect(),
    )
}
fn apply(
    store: &mut CatalogStore,
    config: &Config,
    cycle: &ProbeCycle,
) -> ControlResult<AppliedGlmTargets> {
    store.usable()?;
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = apply_glm_round_tx(&tx, config, cycle, NOW_MS)?;
    commit(tx, &mut store.poisoned)?;
    Ok(result)
}
fn preview(
    store: &mut CatalogStore,
    config: &Config,
    cycle: &ProbeCycle,
) -> ControlResult<GlmRoundDraft> {
    store.usable()?;
    let tx = store.conn.transaction()?;
    let result = compute_glm_round_tx(&tx, config, cycle, NOW_MS)?;
    tx.rollback()?;
    Ok(result)
}
fn err<T>(result: ControlResult<T>, code: Code) {
    match result {
        Err(e) => assert_eq!(e.code, code),
        Ok(_) => panic!("expected {code:?}"),
    }
}
fn image(conn: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
    [
        "source_secrets",
        "sources",
        "source_groups",
        "memberships",
        "source_control",
        "member_control",
        "member_execution",
        "execution_attempts",
        "control_operations",
        "operation_targets",
        "group_policy_state",
        "volcano_runtime",
    ]
    .iter()
    .map(|table| {
        let mut q = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
            .unwrap();
        let width = q.column_count();
        let rows = q
            .query_map([], |r| {
                (0..width)
                    .map(|i| r.get(i))
                    .collect::<rusqlite::Result<Vec<Value>>>()
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        ((*table).to_string(), rows)
    })
    .collect()
}
fn unmanage(store: &mut CatalogStore, id: MembershipId) {
    let tx = store.conn.transaction().unwrap();
    tx.execute(
        "DELETE FROM operation_targets WHERE operation_id=?1",
        [operation(id).as_str()],
    )
    .unwrap();
    tx.execute(
        "DELETE FROM control_operations WHERE operation_id=?1",
        [operation(id).as_str()],
    )
    .unwrap();
    tx.commit().unwrap();
}

fn assert_round_views(store: &mut CatalogStore, round: &GlmRoundDraft) {
    for w in round.groups.iter().flat_map(|g| &g.work) {
        let actual = store.load_work(w.member.membership_id, false).unwrap();
        assert_eq!(w.source, actual.source);
        assert_eq!(w.group, actual.group);
        assert_eq!(w.member, actual.member);
        assert_eq!(w.target, actual.target);
        assert_eq!(w.identity.channel_type, actual.identity.channel_type);
        assert_eq!(w.identity.group, actual.identity.group);
        assert_eq!(w.identity.base_url, actual.identity.base_url);
        assert_eq!(w.models, actual.models);
        assert_eq!(w.managed, actual.managed);
        assert_eq!(w.source_retire_requested, actual.source_retire_requested);
        assert_eq!(w.policy_disabled, actual.policy_disabled);
        assert_eq!(w.pending_write, actual.pending_write);
        assert_eq!(w.execution_revision, actual.execution_revision);
        assert_eq!(w.enable_consumed, actual.enable_consumed);
        assert_eq!(
            w.enable_requested_revision,
            actual.enable_requested_revision
        );
    }
}
fn unbind(store: &mut CatalogStore, id: MembershipId) {
    store
        .conn
        .execute(
            "UPDATE memberships SET channel_id=NULL,lifecycle='planned' WHERE membership_id=?1",
            [id.get()],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE member_control SET create_state='never' WHERE membership_id=?1",
            [id.get()],
        )
        .unwrap();
}
fn direct(
    store: &mut CatalogStore,
    group: &GroupView,
    targets: &[PolicyTarget],
) -> ControlResult<Vec<MembershipId>> {
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = apply_policy_targets_tx(&tx, group, targets, NOW_MS / 1000)?;
    commit(tx, &mut store.poisoned)?;
    Ok(result)
}

#[test]
fn round_is_internal_and_empty_and_dry_run_are_readonly() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(GlmRoundDraft, serde::Deserialize<'static>);
    assert_not_impl!(AppliedGlmTargets, serde::Serialize);
    assert_not_impl!(GlmRoundDraft, std::fmt::Debug);
    let (_dir, mut store) = setup();
    let empty = cycle(&[], &[]);
    let before = image(&store.conn);
    let r = apply(&mut store, &cfg(), &empty).unwrap();
    assert!(r.round.groups.is_empty());
    assert!(r.changed_groups.is_empty());
    assert!(r.changed_members.is_empty());
    assert_eq!(image(&store.conn), before);
    let mut f = fixture(2, 2);
    let b = cycle(&f.sources, &[Some(2), Some(30)]);
    let before = image(&f.store.conn);
    let mut dry = cfg();
    dry.dry_run = true;
    let r = preview(&mut f.store, &dry, &b).unwrap();
    assert_eq!(r.cycle_id, 1);
    assert_eq!(r.groups.len(), 2);
    assert_eq!(image(&f.store.conn), before);
    err(apply(&mut f.store, &dry, &b), Code::InvalidInput);
    assert_eq!(image(&f.store.conn), before);
}

#[test]
fn round_shared_selection_is_atomic_noop_and_reopen_preserves_targets() {
    let mut f = fixture(3, 2);
    let b = cycle(&f.sources, &[Some(75), Some(15), None]);
    let before = image(&f.store.conn);
    let applied = apply(&mut f.store, &cfg(), &b).unwrap();
    assert_eq!(applied.changed_groups, f.groups);
    assert_eq!(
        applied.changed_members,
        vec![f.members[0][1], f.members[1][1]]
    );
    for (i, g) in applied.round.groups.iter().enumerate() {
        assert_eq!(g.policy.policy_revision, 2);
        assert_eq!(g.decision.state.active, Some(f.members[i][1]));
        assert_eq!(g.work[1].target.revision, 3);
        assert_eq!(g.work[1].target.desired_priority, cfg().priority_active);
        assert_eq!(g.targets[1].expected_revision, 3);
        assert_eq!(g.work[2].target.desired_priority, cfg().priority_standby);
    }
    let after = image(&f.store.conn);
    for table in [
        "sources",
        "source_secrets",
        "source_groups",
        "source_control",
        "member_execution",
        "execution_attempts",
        "operation_targets",
        "volcano_runtime",
    ] {
        assert_eq!(after[table], before[table], "{table}");
    }
    let count = f.store.conn.total_changes();
    let noop = apply(&mut f.store, &cfg(), &b).unwrap();
    assert!(noop.changed_groups.is_empty() && noop.changed_members.is_empty());
    assert_eq!(f.store.conn.total_changes(), count);
    assert_eq!(image(&f.store.conn), after);
    drop(f.store);
    let store = CatalogStore::open(&f.dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&store.conn), after);
}

#[test]
fn round_unmanaged_paused_unbound_retiring_and_unknown_members_keep_all_fields() {
    let mut f = fixture(6, 1);
    let m = f.members[0].clone();
    unmanage(&mut f.store, m[0]);
    f.store
        .conn
        .execute(
            "UPDATE sources SET manual_paused=1 WHERE source_id=?1",
            [f.sources[1].get()],
        )
        .unwrap();
    unbind(&mut f.store, m[2]);
    f.store.conn.execute("UPDATE member_control SET retire_requested=1,status_directive='disabled' WHERE membership_id=?1",[m[3].get()]).unwrap();
    let b = cycle(
        &f.sources,
        &[Some(0), Some(1), Some(2), Some(3), None, Some(100)],
    );
    let before = image(&f.store.conn);
    let out = apply(&mut f.store, &cfg(), &b).unwrap();
    let after = image(&f.store.conn);
    assert_eq!(out.round.groups[0].decision.state.active, Some(m[0])); // 选择不是接管/许可。
    assert_eq!(out.changed_members, vec![m[5]]);
    for id in &m[..5] {
        for table in ["memberships", "member_control", "member_execution"] {
            let old = before[table]
                .iter()
                .find(|r| r[0] == Value::Integer(id.get()))
                .unwrap();
            let new = after[table]
                .iter()
                .find(|r| r[0] == Value::Integer(id.get()))
                .unwrap();
            assert_eq!(old, new, "{table} {id:?}");
        }
    }
    assert_eq!(before["operation_targets"], after["operation_targets"]);
}

#[test]
fn round_enabled_consumption_disabled_and_success_receipts_are_preserved() {
    let mut f = fixture(4, 1);
    let m = f.members[0].clone();
    for id in &m[..3] {
        f.store
            .conn
            .execute(
                "UPDATE member_control SET status_directive='enabled' WHERE membership_id=?1",
                [id.get()],
            )
            .unwrap();
        f.store.conn.execute("UPDATE member_execution SET enable_epoch=1,enable_requested_revision=2 WHERE membership_id=?1",[id.get()]).unwrap();
    }
    f.store
        .conn
        .execute(
            "UPDATE member_execution SET enable_consumed=1 WHERE membership_id=?1",
            [m[0].get()],
        )
        .unwrap();
    f.store
        .conn
        .execute(
            "UPDATE memberships SET applied_revision=2 WHERE membership_id=?1",
            [m[1].get()],
        )
        .unwrap();
    f.store
        .conn
        .execute(
            "UPDATE member_control SET status_directive='disabled' WHERE membership_id=?1",
            [m[3].get()],
        )
        .unwrap();
    f.store
        .conn
        .execute(
            "UPDATE operation_targets SET confirmed_at=1,receipt_json=?1 WHERE operation_id=?2",
            params!["{\"fixture\":\"old-success\"}", operation(m[0]).as_str()],
        )
        .unwrap();
    f.store
        .conn
        .execute(
            "UPDATE control_operations SET phase='succeeded' WHERE operation_id=?1",
            [operation(m[0]).as_str()],
        )
        .unwrap();
    let before = image(&f.store.conn);
    let mut draft = preview(&mut f.store, &cfg(), &cycle(&f.sources, &[Some(1); 4])).unwrap();
    for t in &mut draft.groups[0].targets {
        t.priority = 70;
    }
    direct(
        &mut f.store,
        &draft.groups[0].group,
        &draft.groups[0].targets,
    )
    .unwrap();
    let after = image(&f.store.conn);
    assert_eq!(before["member_execution"], after["member_execution"]);
    assert_eq!(before["operation_targets"], after["operation_targets"]);
    let statuses: Vec<_> = m
        .iter()
        .map(|id| f.store.member_target(*id).unwrap().status_directive)
        .collect();
    assert_eq!(
        statuses,
        vec![
            StatusDirective::Preserve,
            StatusDirective::Preserve,
            StatusDirective::Enabled,
            StatusDirective::Disabled
        ]
    );
    assert_eq!(
        f.store.operation(&operation(m[0])).unwrap().phase,
        crate::catalog::control::OperationPhase::Succeeded
    );
}

#[test]
fn round_target_validation_precedes_all_writes() {
    let mut f = fixture(2, 2);
    let draft = preview(
        &mut f.store,
        &cfg(),
        &cycle(&f.sources, &[Some(1), Some(2)]),
    )
    .unwrap();
    let before = image(&f.store.conn);
    for mode in 0..6 {
        let mut targets = draft.groups[0].targets.clone();
        match mode {
            0 => {
                targets.pop();
            }
            1 => targets[1] = targets[0],
            2 => targets[1].membership_id = f.members[1][0],
            3 => targets[1].expected_revision += 1,
            4 => targets[1].priority = JSON_SAFE + 1,
            _ => targets[1].expected_revision = 0,
        }
        err(
            direct(&mut f.store, &draft.groups[0].group, &targets),
            if mode == 3 {
                Code::RevisionConflict
            } else {
                Code::InvalidInput
            },
        );
        assert_eq!(image(&f.store.conn), before, "mode {mode}");
    }
    let mut group = draft.groups[0].group.clone();
    group.revision += 1;
    err(
        direct(&mut f.store, &group, &draft.groups[0].targets),
        Code::RevisionConflict,
    );
    unmanage(&mut f.store, f.members[0][0]);
    let before = image(&f.store.conn);
    err(
        direct(
            &mut f.store,
            &draft.groups[0].group,
            &draft.groups[0].targets,
        ),
        Code::InvalidInput,
    );
    assert_eq!(image(&f.store.conn), before);
}

#[test]
fn round_cycle_and_config_reject_invalid_data_and_stale_observations_are_unknown() {
    let mut f = fixture(1, 1);
    let b = cycle(&f.sources, &[Some(1)]);
    let before = image(&f.store.conn);
    for mode in 0..7 {
        let mut rows = b.observations().to_vec();
        let (mut id, mut end) = (1, NOW_MS);
        match mode {
            0 => id = 0,
            1 => end = NOW_MS + 1,
            2 => rows.push(rows[0].clone()),
            3 => rows[0].observed_at_ms = NOW_MS + 1,
            4 => rows[0].observed_at_ms = -1,
            5 => rows[0].config_revision = 0,
            _ => rows[0].cycle_id = 0,
        }
        err(
            apply(&mut f.store, &cfg(), &ProbeCycle::test_input(id, end, rows)),
            Code::InvalidInput,
        );
        assert_eq!(image(&f.store.conn), before);
    }
    for mode in 0..4 {
        let mut c = cfg();
        match mode {
            0 => c.throttle_threshold = f64::NAN,
            1 => c.priority_active = c.priority_standby,
            2 => c.priority_exhausted = JSON_SAFE + 1,
            _ => c.weekly_reset_lookahead_hours = 169,
        }
        err(apply(&mut f.store, &c, &b), Code::InvalidInput);
        assert_eq!(image(&f.store.conn), before);
    }
    let mut rows = b.observations().to_vec();
    rows[0].config_revision = 2;
    let out = apply(
        &mut f.store,
        &cfg(),
        &ProbeCycle::test_input(1, NOW_MS, rows),
    )
    .unwrap();
    assert!(out.changed_groups.is_empty() && out.changed_members.is_empty());
    assert_eq!(image(&f.store.conn), before);
}

#[test]
fn round_missing_required_control_rows_are_storage_errors_without_writes() {
    for table in ["member_control", "member_execution", "source_control"] {
        let mut f = fixture(2, 2);
        let (column, id) = if table == "source_control" {
            ("source_id", f.sources[0].get())
        } else {
            ("membership_id", f.members[1][0].get())
        };
        f.store
            .conn
            .execute(&format!("DELETE FROM {table} WHERE {column}=?1"), [id])
            .unwrap();
        let before = image(&f.store.conn);
        err(
            preview(
                &mut f.store,
                &cfg(),
                &cycle(&f.sources, &[Some(1), Some(2)]),
            ),
            Code::Storage,
        );
        err(
            apply(
                &mut f.store,
                &cfg(),
                &cycle(&f.sources, &[Some(1), Some(2)]),
            ),
            Code::Storage,
        );
        assert_eq!(image(&f.store.conn), before);
    }
}

#[test]
fn round_missing_policy_and_revision_exhaustion_roll_back_earlier_groups() {
    for mode in 0..3 {
        let mut f = fixture(2, 2);
        match mode {
            0 => {
                f.store
                    .conn
                    .execute(
                        "DELETE FROM group_policy_state WHERE group_id=?1",
                        [f.groups[1].get()],
                    )
                    .unwrap();
            }
            1 => {
                f.store
                    .conn
                    .execute(
                        "UPDATE group_policy_state SET revision=?1 WHERE group_id=?2",
                        params![JSON_SAFE, f.groups[1].get()],
                    )
                    .unwrap();
            }
            _ => {
                f.store
                    .conn
                    .execute(
                        "UPDATE member_control SET revision=?1 WHERE membership_id=?2",
                        params![JSON_SAFE, f.members[1][0].get()],
                    )
                    .unwrap();
            }
        }
        let before = image(&f.store.conn);
        err(
            apply(
                &mut f.store,
                &cfg(),
                &cycle(&f.sources, &[Some(1), Some(2)]),
            ),
            Code::Storage,
        );
        assert_eq!(image(&f.store.conn), before);
    }
}

#[test]
fn round_pin_degraded_unknown_and_retired_history_keep_glm_semantics() {
    let mut f = fixture(2, 1);
    let g = f.groups[0];
    let m = f.members[0].clone();
    f.store
        .save_group_policy(
            g,
            1,
            1,
            GroupPolicyState::Glm(GlmGroupState {
                active: Some(m[0]),
                pinned: Some(m[0]),
                regime: Regime::Normal,
            }),
        )
        .unwrap();
    let out = apply(
        &mut f.store,
        &cfg(),
        &cycle(&f.sources, &[Some(96), Some(98)]),
    )
    .unwrap();
    assert_eq!(
        out.round.groups[0].decision.state,
        GlmGroupState {
            active: Some(m[0]),
            pinned: Some(m[0]),
            regime: Regime::Degraded
        }
    );
    let out = apply(
        &mut f.store,
        &cfg(),
        &cycle(&f.sources, &[Some(100), Some(30)]),
    )
    .unwrap();
    assert_eq!(
        out.round.groups[0].decision.state,
        GlmGroupState {
            active: Some(m[1]),
            pinned: None,
            regime: Regime::Normal
        }
    );
    assert!(out.round.groups[0].decision.pin_release.is_some());
    let before = image(&f.store.conn);
    let out = apply(&mut f.store, &cfg(), &cycle(&f.sources, &[None, None])).unwrap();
    assert_eq!(out.round.groups[0].decision.state.active, Some(m[1]));
    assert!(out.changed_members.is_empty());
    assert_eq!(image(&f.store.conn), before);
    f.store
        .conn
        .execute(
            "UPDATE memberships SET lifecycle='retired',retired_at=?1 WHERE membership_id=?2",
            params![NOW_MS / 1000, m[0].get()],
        )
        .unwrap();
    let rev = f.store.load_group_policy(g).unwrap().policy_revision;
    f.store
        .save_group_policy(
            g,
            1,
            rev,
            GroupPolicyState::Glm(GlmGroupState {
                active: Some(m[0]),
                pinned: Some(m[0]),
                regime: Regime::Normal,
            }),
        )
        .unwrap();
    let before = image(&f.store.conn);
    let out = apply(
        &mut f.store,
        &cfg(),
        &cycle(&f.sources, &[Some(0), Some(30)]),
    )
    .unwrap();
    assert_eq!(out.round.groups[0].decision.state.active, Some(m[1]));
    assert_eq!(out.round.groups[0].decision.state.pinned, None);
    let after = image(&f.store.conn);
    for table in ["memberships", "member_control", "member_execution"] {
        assert_eq!(
            before[table]
                .iter()
                .find(|r| r[0] == Value::Integer(m[0].get())),
            after[table]
                .iter()
                .find(|r| r[0] == Value::Integer(m[0].get()))
        );
    }
}

fn maximum_fixture() -> (TempDir, CatalogStore, Vec<SourceId>) {
    let (dir, mut store) = setup();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let mut sources = Vec::new();
    for i in 0..MAX_SOURCES {
        let draft =
            validate_source(source(&format!("fake-capacity-glm-{i}"), SourceKind::Glm)).unwrap();
        sources.push(insert_source(&tx, &draft).unwrap().source_id);
    }
    let per_group = MAX_MEMBERSHIPS / MAX_GROUPS;
    for g in 0..MAX_GROUPS {
        let group = insert_group(
            &tx,
            &validate_group(group(&format!("capacity-g{g}"), SourceKind::Glm)).unwrap(),
        )
        .unwrap();
        let op = format!("capacity-op-{g:016}");
        tx.execute("INSERT INTO control_operations VALUES(?1,'set_group_priorities','group',?2,?3,1,?4,'queued',NULL,0,0)",params![op,group.group_id.get(),"a".repeat(64),per_group as i64]).unwrap();
        for i in 0..per_group {
            let index = g * per_group + i;
            let m = insert_membership(
                &tx,
                &NewMembership {
                    source: sources[index % MAX_SOURCES],
                    group: group.group_id,
                    kind: SourceKind::Glm,
                    channel: Some(ChannelId::new(500_000 + index as i64 * 3).unwrap()),
                    position: i as i64,
                    lifecycle: MembershipLifecycle::Imported,
                    priority: 7,
                },
            )
            .unwrap();
            tx.execute(
                "INSERT INTO operation_targets VALUES(?1,?2,1,NULL,NULL)",
                params![op, m.membership_id.get()],
            )
            .unwrap();
        }
    }
    commit(tx, &mut store.poisoned).unwrap();
    (dir, store, sources)
}

#[test]
fn round_maximum_catalog_is_bounded_and_extra_observations_are_rejected() {
    let (_dir, mut store, sources) = maximum_fixture();
    let probe_start = Instant::now();
    assert_eq!(store.glm_probe_inputs().unwrap().len(), MAX_SOURCES);
    println!(
        "GLM_PROBE_INPUTS_MAXIMUM_MS={}",
        probe_start.elapsed().as_millis()
    );
    assert!(probe_start.elapsed() < Duration::from_secs(5));
    store
        .conn
        .execute("UPDATE source_groups SET ready=1", [])
        .unwrap();
    let backend = crate::reconcile::planner::BackendReadiness::from_preflight(
        "v1.0.0-rc.20",
        crate::newapi::managed_channels::NativeRecoveryCheck::Disabled,
        true,
        0,
        NOW_MS / 1000 + 100,
    );
    let b = cycle(&sources, &vec![Some(10); MAX_SOURCES]);
    let start = Instant::now();
    let out = store
        .commit_glm_round(&cfg(), &b, backend, &HashMap::new(), NOW_MS)
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(out.changed_members.len(), MAX_MEMBERSHIPS);
    assert_eq!(out.changed_groups.len(), MAX_GROUPS);
    assert_eq!(out.permits.len(), MAX_MEMBERSHIPS);
    println!("GLM_ROUND_MAXIMUM_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(10));
    let count = store.conn.total_changes();
    let out = store
        .commit_glm_round(&cfg(), &b, backend, &out.permits, NOW_MS)
        .unwrap();
    assert!(out.changed_groups.is_empty() && out.changed_members.is_empty());
    assert_eq!(store.conn.total_changes(), count);
    store
        .conn
        .execute(
            "UPDATE member_control SET dirty=1,last_error_code='not_ready',next_attempt_at=?1",
            [NOW_MS / 1000 + 500],
        )
        .unwrap();
    let waiting = image(&store.conn);
    let count = store.conn.total_changes();
    let start = Instant::now();
    let recovered = store
        .commit_glm_round(&cfg(), &b, backend, &HashMap::new(), NOW_MS)
        .unwrap();
    let elapsed = start.elapsed();
    println!(
        "GLM_PUBLICATION_RECOVERY_MAXIMUM_MS={}",
        elapsed.as_millis()
    );
    assert!(elapsed < Duration::from_secs(10));
    assert!(recovered.changed_members.is_empty());
    assert_eq!(store.conn.total_changes() - count, MAX_MEMBERSHIPS as u64);
    assert_eq!(
        store
            .conn
            .query_row(
                "SELECT count(*) FROM member_control WHERE next_attempt_at=?1",
                [NOW_MS / 1000],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        MAX_MEMBERSHIPS as i64
    );
    store
        .conn
        .execute(
            "UPDATE member_control SET next_attempt_at=?1",
            [NOW_MS / 1000 + 500],
        )
        .unwrap();
    assert_eq!(image(&store.conn), waiting);
    let mut rows = b.observations().to_vec();
    let mut extra = rows[0].clone();
    extra.source_id = SourceId::new(900_000).unwrap();
    rows.push(extra);
    err(
        apply(&mut store, &cfg(), &ProbeCycle::test_input(1, NOW_MS, rows)),
        Code::InvalidInput,
    );
    let tx = store.conn.transaction().unwrap();
    // 测试专用 SQL 绕开正常创建的容量门禁，构造第 1001 条合法材料。
    let extra = validate_source(source("fake-probe-overflow", SourceKind::Glm)).unwrap();
    tx.execute(
        "INSERT INTO source_secrets(raw_key) VALUES(?1)",
        [extra.draft.credential.expose()],
    )
    .unwrap();
    let secret = tx.last_insert_rowid();
    tx.execute("INSERT INTO sources(kind,name,note,secret_id,credential_fingerprint,settings_json,config_revision,manual_paused) VALUES('glm',?1,'',?2,?3,?4,1,0)",params![extra.draft.name,secret,extra.fingerprint,encode_settings(&extra.draft.settings).unwrap()]).unwrap();
    tx.execute(
        "INSERT INTO source_control VALUES(?1,0)",
        [tx.last_insert_rowid()],
    )
    .unwrap();
    commit(tx, &mut store.poisoned).unwrap();
    let before = image(&store.conn);
    err(store.glm_probe_inputs(), Code::Storage);
    assert_eq!(image(&store.conn), before);
}

#[test]
fn round_reused_snapshot_keeps_shared_operation_and_group_versions_consistent() {
    let mut f = fixture(2, 2);
    let shared = OperationId::new("round-shared-source-resume".into()).unwrap();
    f.store
        .submit_operation(
            shared.clone(),
            ControlCommand::SetSourcePaused {
                source_id: f.sources[0],
                expected_revision: 1,
                paused: false,
            },
            NOW_MS / 1000 - 1,
        )
        .unwrap();
    let b = cycle(&f.sources, &[Some(1), Some(70)]);
    let mut rows = b.observations().to_vec();
    rows[0].config_revision = 2;
    let before = image(&f.store.conn);
    let out = apply(
        &mut f.store,
        &cfg(),
        &ProbeCycle::test_input(1, NOW_MS, rows),
    )
    .unwrap();
    assert_eq!(out.changed_members, vec![f.members[0][0], f.members[1][0]]);
    for (g, draft) in out.round.groups.iter().enumerate() {
        assert_eq!(draft.decision.state.active, Some(f.members[g][0]));
        assert_eq!(draft.work[0].target.revision, 4);
        assert_eq!(
            draft.work[0].target.status_directive,
            StatusDirective::Enabled
        );
        assert!(!draft.work[0].enable_consumed);
        assert_eq!(draft.work[0].enable_requested_revision, 3);
    }
    let after = image(&f.store.conn);
    for table in [
        "sources",
        "source_groups",
        "operation_targets",
        "member_execution",
    ] {
        assert_eq!(before[table], after[table], "{table}");
    }
    assert_eq!(
        f.store.operation(&shared).unwrap().phase,
        crate::catalog::control::OperationPhase::Superseded
    );
}

#[test]
fn round_nonempty_unknown_execution_history_survives_target_change_and_reopen() {
    use crate::catalog::execution::ExecutionEffect;
    use crate::reconcile::planner::PendingWrite;
    let mut f = fixture(2, 1);
    let id = f.members[0][0];
    f.store
        .conn
        .execute(
            "UPDATE member_control SET status_directive='enabled' WHERE membership_id=?1",
            [id.get()],
        )
        .unwrap();
    f.store.conn.execute("UPDATE member_execution SET sequence=3,enable_epoch=1,enable_requested_revision=2,enable_consumed=1 WHERE membership_id=?1",[id.get()]).unwrap();
    for (seq, state, error) in [
        (1, "responded", None),
        (2, "uncertain", Some("transport_unknown")),
    ] {
        let effect = serde_json::to_string(&ExecutionEffect::Fields {
            priority: Some(20 + seq),
            models: Some(format!("old-model-{seq}")),
            disable_auto_ban: false,
        })
        .unwrap();
        f.store
            .conn
            .execute(
                "INSERT INTO execution_attempts VALUES(?1,?2,2,1,1,1,'fields',?3,?4,?5,?6,?7)",
                params![
                    id.get(),
                    seq,
                    effect,
                    state,
                    error,
                    NOW_MS / 1000 - 30 + seq,
                    NOW_MS / 1000 - 20 + seq
                ],
            )
            .unwrap();
    }
    f.store.conn.execute("UPDATE control_operations SET phase='unknown',error_code='transport_unknown' WHERE operation_id=?1",[operation(id).as_str()]).unwrap();
    let before = image(&f.store.conn);
    assert_eq!(before["execution_attempts"].len(), 2);
    let out = apply(
        &mut f.store,
        &cfg(),
        &cycle(&f.sources, &[Some(1), Some(70)]),
    )
    .unwrap();
    assert!(out.changed_members.contains(&id));
    assert_round_views(&mut f.store, &out.round);
    assert_eq!(
        out.round.groups[0].work[0].pending_write,
        PendingWrite::Unknown
    );
    assert_eq!(
        out.round.groups[0].work[0].target.status_directive,
        StatusDirective::Preserve
    );
    let after = image(&f.store.conn);
    for table in [
        "member_execution",
        "execution_attempts",
        "operation_targets",
    ] {
        assert_eq!(after[table], before[table], "{table}");
    }
    assert_eq!(
        f.store.operation(&operation(id)).unwrap().phase,
        crate::catalog::control::OperationPhase::Superseded
    );
    drop(f.store);
    let mut store = CatalogStore::open(&f.dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&store.conn), after);
    assert_eq!(
        store.load_work(id, false).unwrap().pending_write,
        PendingWrite::Unknown
    );
}

#[test]
fn round_unknown_commit_poisoning_and_caller_rollback_are_preserved() {
    let mut f = fixture(2, 1);
    let b = cycle(&f.sources, &[Some(1), Some(10)]);
    let before = image(&f.store.conn);
    let tx = f
        .store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    apply_glm_round_tx(&tx, &cfg(), &b, NOW_MS).unwrap();
    tx.rollback().unwrap();
    assert_eq!(image(&f.store.conn), before);
    let tx = f
        .store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.pragma_update(None, "defer_foreign_keys", true).unwrap();
    apply_glm_round_tx(&tx, &cfg(), &b, NOW_MS).unwrap();
    tx.execute("INSERT INTO source_control VALUES(999,0)", [])
        .unwrap();
    assert_eq!(
        commit(tx, &mut f.store.poisoned).unwrap_err().kind,
        ErrorKind::CommitUnknown
    );
    assert_eq!(image(&f.store.conn), before);
    err(apply(&mut f.store, &cfg(), &b), Code::Storage);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn c1_targets_match_independent_model_and_noop(
        rows in prop::collection::vec((prop::option::of(0u8..=100),any::<bool>(),any::<bool>(),any::<bool>(),any::<bool>()),1..7),groups in 1usize..4
    ) {
        let n=rows.len();let mut f=fixture(n,groups);let c=cfg();
        for (i,(_,managed,bound,paused,retiring)) in rows.iter().enumerate() {
            f.store.conn.execute("UPDATE sources SET manual_paused=?1 WHERE source_id=?2",params![paused,f.sources[i].get()]).unwrap();
            for g in 0..groups {
                if !(*managed ^ (g%2==1)) {unmanage(&mut f.store,f.members[g][i]);}
                if !(*bound || g%2==1) {unbind(&mut f.store,f.members[g][i]);}
                if *retiring && g%2==0 {f.store.conn.execute("UPDATE member_control SET retire_requested=1,status_directive='disabled' WHERE membership_id=?1",[f.members[g][i].get()]).unwrap();}
            }
        }
        let b=cycle(&f.sources,&rows.iter().map(|r|r.0).collect::<Vec<_>>());let before=image(&f.store.conn);
        let result=apply(&mut f.store,&c,&b).unwrap();assert_round_views(&mut f.store,&result.round);let after_first=image(&f.store.conn);let mut expected_changed=Vec::new();let mut expected_groups=Vec::new();
        for g in 0..groups {
            let included:Vec<_>=(0..n).filter(|i|!(rows[*i].3 || rows[*i].4 && g%2==0) && (rows[*i].2 || g%2==1)).collect();
            let known:Vec<_>=included.iter().copied().filter(|i|rows[*i].0.is_some()).collect();
            let normal:Vec<_>=known.iter().copied().filter(|i|rows[*i].0.unwrap()<95).collect();
            let degraded=normal.is_empty()&&!known.is_empty();
            let eligible:Vec<_>=if degraded {known.iter().copied().filter(|i|rows[*i].0.unwrap()<100).collect()}else{normal};
            let active=eligible.iter().copied().min_by_key(|i|rows[*i].0.unwrap());
            let expected=GlmGroupState {active:active.map(|i|f.members[g][i]),pinned:None,regime:if degraded {Regime::Degraded}else{Regime::Normal}};
            let changed=expected!=GlmGroupState::default();if changed {expected_groups.push(f.groups[g]);}
            prop_assert_eq!(f.store.load_group_policy(f.groups[g]).unwrap(),StoredGroupPolicy{group_id:f.groups[g],policy_revision:if changed{2}else{1},state:GroupPolicyState::Glm(expected)});
            for (i,row) in rows.iter().enumerate() {
                let managed=row.1 ^ (g%2==1);
                let priority=if !managed || !included.contains(&i) {c.priority_standby}else if active==Some(i) {c.priority_active}else if eligible.contains(&i) {c.priority_standby}else if row.0.is_some() {c.priority_exhausted}else{c.priority_standby};
                let changed=priority!=c.priority_standby;if changed {expected_changed.push(f.members[g][i]);}
                let t=f.store.member_target(f.members[g][i]).unwrap();
                prop_assert_eq!(t.desired_priority,priority);prop_assert_eq!(t.revision,if changed {3}else{2});
                let id=Value::Integer(f.members[g][i].get());
                if !changed {for table in ["memberships","member_control","member_execution"] {prop_assert_eq!(before[table].iter().find(|r|r[0]==id),after_first[table].iter().find(|r|r[0]==id));}}
            }
        }
        prop_assert_eq!(result.changed_groups,expected_groups);prop_assert_eq!(result.changed_members,expected_changed);
        let after=image(&f.store.conn);let count=f.store.conn.total_changes();let second=apply(&mut f.store,&c,&b).unwrap();
        prop_assert!(second.changed_groups.is_empty()&&second.changed_members.is_empty());prop_assert_eq!(image(&f.store.conn),after);prop_assert_eq!(f.store.conn.total_changes(),count);
    }
    #[test]
    fn c1_every_write_failure_rolls_back_all_groups(mode in 0u8..4,n in 2usize..7,groups in 1usize..4,group_index in 0usize..4,member_index in 0usize..7,active in 0usize..7) {
        let mut f=fixture(n,groups);let id=f.members[group_index%groups][member_index%n].get();
        let (table,column,value)=match mode {0=>("group_policy_state","group_id",f.groups[group_index%groups].get()),1=>("member_control","membership_id",id),2=>("memberships","membership_id",id),_=>("control_operations","target_id",id)};
        f.store.conn.execute_batch(&format!("CREATE TRIGGER fail_glm_round BEFORE UPDATE ON {table} WHEN OLD.{column}={value} BEGIN SELECT RAISE(ABORT,'fixture'); END;")).unwrap();
        let before=image(&f.store.conn);let mut values=vec![Some(100);n];values[active%n]=Some(1);let b=cycle(&f.sources,&values);
        err(apply(&mut f.store,&cfg(),&b),Code::Storage);prop_assert_eq!(image(&f.store.conn),before);
    }
}
