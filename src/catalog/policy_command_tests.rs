use super::*;
use crate::catalog::control::{
    ControlCommand, ControlErrorCode as Code, OperationId, OperationKind, OperationPhase,
    StatusDirective,
};
use crate::catalog::policy::{GroupPolicyState, SourcePolicyUpdate};
use crate::source_policy::volcano::{
    OutcomeKind, OutcomeObservation, VolcanoEvidence, VolcanoRuntime,
};
use serde_json::json;
const NOW: i64 = 180_000;
#[path = "policy_command_migration_tests.rs"]
mod migration;

struct Fixture {
    dir: TempDir,
    store: CatalogStore,
    sources: [SourceId; 2],
    groups: Vec<GroupId>,
    members: Vec<MembershipId>,
}
fn fixture() -> Fixture {
    let (dir, mut store) = setup();
    let glm = store
        .create_source(source("fake-c2-glm", SourceKind::Glm))
        .unwrap()
        .source_id;
    let mut draft = source("fake-c2-volcano", SourceKind::Volcano);
    if let SourceSettings::Volcano { policy, .. } = &mut draft.settings {
        policy.daily_token_limit = Some(10);
    }
    let volcano = store.create_source(draft).unwrap().source_id;
    let mut groups = Vec::new();
    let mut members = Vec::new();
    for (i, (source, kind)) in [
        (glm, SourceKind::Glm),
        (glm, SourceKind::Glm),
        (volcano, SourceKind::Volcano),
        (volcano, SourceKind::Volcano),
    ]
    .into_iter()
    .enumerate()
    {
        let g = store
            .create_group(group(&format!("c2-{i}"), kind))
            .unwrap()
            .group_id;
        let m = store.plan_membership(source, g, 100).unwrap().membership_id;
        store
            .conn
            .execute(
                "UPDATE memberships SET channel_id=?1,lifecycle='imported' WHERE membership_id=?2",
                params![1001 + i as i64, m.get()],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
                [m.get()],
            )
            .unwrap();
        store
            .submit_operation(
                op(i),
                ControlCommand::EnsureMember {
                    membership_id: m,
                    expected_revision: 1,
                    status: StatusDirective::Preserve,
                },
                NOW,
            )
            .unwrap();
        groups.push(g);
        members.push(m);
    }
    store
        .conn
        .execute("UPDATE source_groups SET ready=1", [])
        .unwrap();
    Fixture {
        dir,
        store,
        sources: [glm, volcano],
        groups,
        members,
    }
}
fn op(i: usize) -> OperationId {
    OperationId::new(format!("c2-operation-{i:016}")).unwrap()
}
fn image(conn: &Connection) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    [
        "catalog_meta",
        "source_secrets",
        "sources",
        "source_groups",
        "memberships",
        "legacy_import",
        "sqlite_sequence",
        "source_control",
        "member_control",
        "control_operations",
        "operation_targets",
        "member_execution",
        "execution_attempts",
        "group_policy_state",
        "volcano_runtime",
        "ledger_state",
        "ledger_checkpoints",
        "ledger_facts",
        "ledger_minute_usage",
        "volcano_observation_queue",
        "volcano_request_observations",
        "volcano_period_history",
    ]
    .into_iter()
    .map(|name| {
        let mut q = conn
            .prepare(&format!("SELECT * FROM {name} ORDER BY 1,2"))
            .unwrap();
        let n = q.column_count();
        let rows = q
            .query_map([], |r| (0..n).map(|i| r.get(i)).collect())
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        (name.into(), rows)
    })
    .collect()
}
fn source_revision(f: &Fixture, i: usize) -> i64 {
    f.store
        .conn
        .query_row(
            "SELECT config_revision FROM sources WHERE source_id=?1",
            [f.sources[i].get()],
            |r| r.get(0),
        )
        .unwrap()
}
fn runtime(f: &Fixture) -> VolcanoRuntime {
    f.store.load_source_policy(f.sources[1]).unwrap().unwrap()
}
fn initialize_exhausted(f: &mut Fixture) {
    let old = f
        .store
        .initialize_source_policy(f.sources[1], source_revision(f, 1), NOW)
        .unwrap();
    let batch = VolcanoEvidence {
        source_id: f.sources[1],
        period_id: old.period_id,
        error_epoch: old.error_epoch,
        from_cursor: None,
        to_cursor: 1,
        used_input: 20,
        used_output: 0,
        coverage_end: NOW,
        outcomes: (0..3)
            .map(|i| OutcomeObservation {
                first_event_at: NOW - 10,
                observed_at: NOW - 3 + i,
                kind: OutcomeKind::Unavailable,
            })
            .collect(),
    };
    f.store
        .apply_source_policy(
            f.sources[1],
            source_revision(f, 1),
            old.revision,
            SourcePolicyUpdate::Evidence(batch),
            NOW,
        )
        .unwrap();
}
fn pin(f: &Fixture, member: Option<MembershipId>) -> ControlCommand {
    let revision = f
        .store
        .conn
        .query_row(
            "SELECT revision FROM source_groups WHERE group_id=?1",
            [f.groups[0].get()],
            |r| r.get(0),
        )
        .unwrap();
    ControlCommand::SetGroupPin {
        group_id: f.groups[0],
        expected_revision: revision,
        expected_policy_revision: f
            .store
            .load_group_policy(f.groups[0])
            .unwrap()
            .policy_revision,
        membership_id: member,
    }
}
fn clear(f: &Fixture) -> ControlCommand {
    ControlCommand::ClearVolcanoError {
        source_id: f.sources[1],
        expected_revision: source_revision(f, 1),
        expected_runtime_revision: runtime(f).revision,
    }
}
fn assert_error<T>(result: crate::catalog::control::ControlResult<T>, code: Code) {
    assert_eq!(result.err().unwrap().code, code);
}

#[test]
fn c2_pin_scope_noop_replay_and_local_receipt_are_exact() {
    let mut f = fixture();
    let before = image(&f.store.conn);
    let command = pin(&f, Some(f.members[0]));
    let view = f
        .store
        .submit_operation(op(10), command.clone(), NOW)
        .unwrap();
    assert_eq!(
        (
            view.schema_version,
            view.kind,
            view.phase,
            view.total_targets,
            view.target_revision
        ),
        (
            2,
            OperationKind::SetGroupPin,
            OperationPhase::Succeeded,
            0,
            2
        )
    );
    let p = f.store.load_group_policy(f.groups[0]).unwrap();
    assert_eq!(p.policy_revision, 2);
    assert!(
        matches!(p.state,GroupPolicyState::Glm(s) if s.pinned==Some(f.members[0]) && s.active.is_none())
    );
    let after = image(&f.store.conn);
    for table in [
        "memberships",
        "member_control",
        "member_execution",
        "operation_targets",
        "sources",
        "volcano_runtime",
    ] {
        assert_eq!(after[table], before[table]);
    }
    assert_eq!(
        f.store
            .submit_operation(op(10), command, NOW + 86400)
            .unwrap(),
        view
    );
    assert_eq!(image(&f.store.conn), after);
    let same = pin(&f, Some(f.members[0]));
    f.store.submit_operation(op(11), same, NOW + 1).unwrap();
    assert_eq!(
        f.store
            .load_group_policy(f.groups[0])
            .unwrap()
            .policy_revision,
        2
    );
    let other = pin(&f, None);
    assert_error(
        f.store.submit_operation(op(10), other, NOW),
        Code::IdempotencyConflict,
    );
    let command = pin(&f, None);
    f.store.submit_operation(op(12), command, NOW + 2).unwrap();
    assert_eq!(
        f.store
            .load_group_policy(f.groups[0])
            .unwrap()
            .policy_revision,
        3
    );
    assert_eq!(
        f.store
            .load_group_policy(f.groups[1])
            .unwrap()
            .policy_revision,
        1
    );
}

#[test]
fn c2_policy_commands_reject_wrong_scope_stale_versions_and_unmanaged_pin() {
    let mut f = fixture();
    let commands = [
        (pin(&f, Some(f.members[1])), Code::InvalidInput),
        (
            ControlCommand::SetGroupPin {
                group_id: f.groups[2],
                expected_revision: 1,
                expected_policy_revision: 1,
                membership_id: None,
            },
            Code::InvalidInput,
        ),
        (
            ControlCommand::SetGroupPin {
                group_id: f.groups[0],
                expected_revision: 1,
                expected_policy_revision: 2,
                membership_id: None,
            },
            Code::RevisionConflict,
        ),
        (
            ControlCommand::ClearVolcanoError {
                source_id: f.sources[0],
                expected_revision: 1,
                expected_runtime_revision: 1,
            },
            Code::InvalidInput,
        ),
        (
            ControlCommand::ClearVolcanoError {
                source_id: f.sources[1],
                expected_revision: 1,
                expected_runtime_revision: 1,
            },
            Code::NotReady,
        ),
        (
            ControlCommand::UpdateVolcanoPolicy {
                source_id: f.sources[1],
                expected_revision: 1,
                expected_runtime_revision: 1,
                policy: VolcanoPolicy::default(),
            },
            Code::RevisionConflict,
        ),
        (
            ControlCommand::UpdateVolcanoPolicy {
                source_id: f.sources[1],
                expected_revision: 1,
                expected_runtime_revision: 0,
                policy: VolcanoPolicy {
                    daily_token_limit: Some(0),
                    ..Default::default()
                },
            },
            Code::InvalidInput,
        ),
    ];
    for (cmd, error) in commands {
        let before = image(&f.store.conn);
        assert_error(f.store.submit_operation(op(20), cmd, NOW), error);
        assert_eq!(image(&f.store.conn), before);
    }
    f.store
        .conn
        .execute(
            "DELETE FROM operation_targets WHERE membership_id=?1",
            [f.members[0].get()],
        )
        .unwrap();
    f.store
        .conn
        .execute(
            "DELETE FROM control_operations WHERE target_type='membership' AND target_id=?1",
            [f.members[0].get()],
        )
        .unwrap();
    let cmd = pin(&f, Some(f.members[0]));
    let before = image(&f.store.conn);
    assert_error(f.store.submit_operation(op(21), cmd, NOW), Code::NotReady);
    assert_eq!(image(&f.store.conn), before);
}

#[test]
fn c2_clear_keeps_limit_pause_and_replay_does_not_advance_next_day() {
    let mut f = fixture();
    initialize_exhausted(&mut f);
    f.store
        .conn
        .execute(
            "UPDATE sources SET manual_paused=1 WHERE source_id=?1",
            [f.sources[1].get()],
        )
        .unwrap();
    let old = runtime(&f);
    let command = clear(&f);
    let view = f
        .store
        .submit_operation(op(30), command.clone(), NOW + 1)
        .unwrap();
    let next = runtime(&f);
    assert_eq!(
        (
            next.consecutive_failures,
            next.error_exhausted,
            next.limit_exhausted,
            next.used_input
        ),
        (0, false, true, 20)
    );
    assert_eq!(next.error_epoch, old.error_epoch + 1);
    assert_eq!(source_revision(&f, 1), 2);
    assert!(
        f.store
            .snapshot()
            .unwrap()
            .sources
            .iter()
            .find(|s| s.source_id == f.sources[1])
            .unwrap()
            .manual_paused
    );
    let before = image(&f.store.conn);
    assert_eq!(
        f.store
            .submit_operation(op(30), command, old.next_reset_at + 1)
            .unwrap(),
        view
    );
    assert_eq!(image(&f.store.conn), before);
    let command = clear(&f);
    f.store
        .submit_operation(op(31), command, old.next_reset_at + 1)
        .unwrap();
    let new_day = runtime(&f);
    assert!(!new_day.limit_exhausted);
    assert_eq!(new_day.used_input, 0);
    assert_eq!(new_day.period_id, old.period_id + 1);
    assert_eq!(
        f.store
            .conn
            .query_row("SELECT count(*) FROM volcano_period_history", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn c2_edit_uninitialized_policy_does_not_create_fake_runtime_and_invalidates_version() {
    let mut f = fixture();
    let policy = VolcanoPolicy {
        daily_token_limit: Some(250),
        reset_minute: 77,
        utc_offset_minutes: 0,
        ..Default::default()
    };
    let cmd = ControlCommand::UpdateVolcanoPolicy {
        source_id: f.sources[1],
        expected_revision: 1,
        expected_runtime_revision: 0,
        policy: policy.clone(),
    };
    let view = f.store.submit_operation(op(40), cmd.clone(), NOW).unwrap();
    assert_eq!(view.target_revision, 2);
    assert!(f.store.load_source_policy(f.sources[1]).unwrap().is_none());
    let settings = f.store.read_material(f.sources[1], false).unwrap().settings;
    assert!(matches!(settings,SourceSettings::Volcano {policy:p,..} if p==policy));
    let before = image(&f.store.conn);
    assert_eq!(
        f.store.submit_operation(op(40), cmd, NOW + 1).unwrap(),
        view
    );
    assert_eq!(image(&f.store.conn), before);
}

#[test]
fn c2_local_faults_roll_back_configuration_history_and_receipts() {
    for table in [
        "control_operations",
        "source_groups",
        "sources",
        "volcano_period_history",
    ] {
        let mut f = fixture();
        initialize_exhausted(&mut f);
        let (cmd, at) = if table == "source_groups" {
            (pin(&f, Some(f.members[0])), NOW)
        } else {
            (
                clear(&f),
                if table == "volcano_period_history" {
                    runtime(&f).next_reset_at
                } else {
                    NOW
                },
            )
        };
        let before = image(&f.store.conn);
        let verb = if ["control_operations", "volcano_period_history"].contains(&table) {
            "INSERT"
        } else {
            "UPDATE"
        };
        f.store.conn.execute_batch(&format!("CREATE TRIGGER c2_fault BEFORE {verb} ON {table} BEGIN SELECT RAISE(ABORT,'synthetic'); END;")).unwrap();
        assert_error(f.store.submit_operation(op(50), cmd, at), Code::Storage);
        assert_eq!(image(&f.store.conn), before);
        assert!(!f.store.poisoned);
    }
    let mut f = fixture();
    initialize_exhausted(&mut f);
    let cmd = clear(&f);
    let before = image(&f.store.conn);
    f.store.conn.execute_batch("CREATE TABLE c2_parent(id INTEGER PRIMARY KEY);CREATE TABLE c2_child(id INTEGER REFERENCES c2_parent(id) DEFERRABLE INITIALLY DEFERRED);CREATE TRIGGER c2_fault AFTER INSERT ON control_operations BEGIN INSERT INTO c2_child VALUES(1); END").unwrap();
    assert_error(f.store.submit_operation(op(51), cmd, NOW), Code::Storage);
    assert!(f.store.poisoned);
    assert_eq!(image(&f.store.conn), before);
}

#[test]
fn c2_model_results_use_stable_request_identity_and_preserve_other_sources() {
    let mut f = fixture();
    let before = image(&f.store.conn);
    assert_error(
        f.store.submit_operation(
            op(60),
            ControlCommand::DiscoverSourceModels {
                source_id: f.sources[0],
                expected_revision: 1,
            },
            NOW,
        ),
        Code::InvalidInput,
    );
    for models in [vec![], vec!["bad,model".into()], vec!["bad\0model".into()]] {
        assert_error(
            f.store
                .submit_discovered_models(op(60), f.sources[0], 1, models, NOW),
            Code::InvalidResponse,
        );
        assert_eq!(image(&f.store.conn), before);
    }
    let view = f
        .store
        .submit_discovered_models(
            op(60),
            f.sources[0],
            1,
            vec![" z ".into(), "a".into(), "z".into()],
            NOW,
        )
        .unwrap();
    assert_eq!(
        (view.kind, view.total_targets, view.phase),
        (
            OperationKind::DiscoverSourceModels,
            2,
            OperationPhase::Queued
        )
    );
    for i in 0..4 {
        let w = f.store.load_work(f.members[i], false).unwrap();
        if i < 2 {
            assert_eq!(w.models, vec!["a", "z"]);
            assert_eq!(w.target.revision, 3);
        } else {
            assert_eq!(w.models, vec!["glm-example"]);
            assert_eq!(w.target.revision, 2);
        }
    }
    let before = image(&f.store.conn);
    assert_eq!(
        f.store
            .submit_discovered_models(op(60), f.sources[0], 1, vec!["changed".into()], NOW + 1)
            .unwrap(),
        view
    );
    assert_eq!(image(&f.store.conn), before);
    assert!(matches!(
        f.store
            .model_discovery_input(&op(60), f.sources[0], 1)
            .unwrap(),
        crate::catalog::policy_commands::ModelDiscoveryInput::Replayed(_)
    ));
}

#[test]
fn c2_schema_views_and_private_discovery_input() {
    use crate::catalog::policy_commands::ModelDiscoveryInput;
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(ModelDiscoveryInput, std::fmt::Debug);
    assert_not_impl!(ModelDiscoveryInput, serde::Serialize);
    let mut f = fixture();
    let p = pin(&f, None);
    let a = f.store.submit_operation(op(70), p, NOW).unwrap();
    initialize_exhausted(&mut f);
    let c = clear(&f);
    let b = f.store.submit_operation(op(71), c, NOW).unwrap();
    let d = ControlCommand::UpdateVolcanoPolicy {
        source_id: f.sources[1],
        expected_revision: 2,
        expected_runtime_revision: runtime(&f).revision,
        policy: VolcanoPolicy::default(),
    };
    let c = f.store.submit_operation(op(72), d, NOW).unwrap();
    let d = f
        .store
        .submit_discovered_models(op(73), f.sources[0], 1, vec!["model".into()], NOW)
        .unwrap();
    println!(
        "C2_SCHEMA_JSON={}",
        serde_json::to_string(&vec![a, b, c, d]).unwrap()
    );
}

#[test]
fn c2_revision_boundaries_pause_and_retirement_fail_without_partial_changes() {
    for mode in 0..5 {
        let mut f = fixture();
        initialize_exhausted(&mut f);
        let (command, error) = match mode {
            0 => {
                f.store
                    .conn
                    .execute(
                        "UPDATE sources SET config_revision=?1 WHERE source_id=?2",
                        params![JSON_SAFE, f.sources[1].get()],
                    )
                    .unwrap();
                (clear(&f), Code::InvalidInput)
            }
            1 => {
                f.store
                    .conn
                    .execute(
                        "UPDATE source_groups SET revision=?1 WHERE group_id=?2",
                        params![JSON_SAFE, f.groups[0].get()],
                    )
                    .unwrap();
                (pin(&f, Some(f.members[0])), Code::InvalidInput)
            }
            2 => {
                f.store
                    .conn
                    .execute(
                        "UPDATE group_policy_state SET revision=?1 WHERE group_id=?2",
                        params![JSON_SAFE, f.groups[0].get()],
                    )
                    .unwrap();
                (pin(&f, Some(f.members[0])), Code::Storage)
            }
            3 => {
                f.store
                    .conn
                    .execute(
                        "UPDATE volcano_runtime SET revision=?1 WHERE source_id=?2",
                        params![JSON_SAFE, f.sources[1].get()],
                    )
                    .unwrap();
                (clear(&f), Code::Storage)
            }
            _ => {
                f.store
                    .conn
                    .execute(
                        "UPDATE volcano_runtime SET error_epoch=?1 WHERE source_id=?2",
                        params![JSON_SAFE, f.sources[1].get()],
                    )
                    .unwrap();
                (clear(&f), Code::Storage)
            }
        };
        let before = image(&f.store.conn);
        assert_error(f.store.submit_operation(op(80), command, NOW), error);
        assert_eq!(image(&f.store.conn), before);
    }
    for retire in [false, true] {
        let mut f = fixture();
        let command = if retire {
            ControlCommand::RetireMember {
                membership_id: f.members[0],
                expected_revision: 2,
            }
        } else {
            ControlCommand::SetSourcePaused {
                source_id: f.sources[0],
                expected_revision: 1,
                paused: true,
            }
        };
        f.store.submit_operation(op(81), command, NOW).unwrap();
        let before = image(&f.store.conn);
        let command = pin(&f, Some(f.members[0]));
        assert_error(
            f.store.submit_operation(op(82), command, NOW),
            Code::NotReady,
        );
        assert_eq!(image(&f.store.conn), before);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn c2_pb1_local_actions_match_independent_versions_and_replay(events in prop::collection::vec((0u8..4,any::<bool>()),1..18),paused in any::<bool>()) {
        let mut f=fixture();initialize_exhausted(&mut f);f.store.conn.execute("UPDATE sources SET manual_paused=?1 WHERE source_id=?2",params![paused,f.sources[1].get()]).unwrap();
        let mut grev=1;let mut prev=1;let mut pinned=None;let mut srev=1;let mut rrev=2;let mut epoch=1;let mut count=3;let mut locked=true;let mut limited=true;
        for (i,(action,high)) in events.into_iter().enumerate(){
            let cmd=if action<2 {ControlCommand::SetGroupPin {group_id:f.groups[0],expected_revision:grev,expected_policy_revision:prev,membership_id:if action==0{None}else{Some(f.members[0])}}}else if action==2 {ControlCommand::ClearVolcanoError {source_id:f.sources[1],expected_revision:srev,expected_runtime_revision:rrev}}else{ControlCommand::UpdateVolcanoPolicy {source_id:f.sources[1],expected_revision:srev,expected_runtime_revision:rrev,policy:VolcanoPolicy{daily_token_limit:Some(if high{30}else{10}),..Default::default()}}};
            let view=f.store.submit_operation(op(100+i),cmd.clone(),NOW+i as i64).unwrap();
            if action<2 {grev+=1;let next=if action==0{None}else{Some(f.members[0])};if pinned!=next{prev+=1;}pinned=next;}else{srev+=1;if action==2{rrev+=1;epoch+=1;count=0;locked=false;}else{let next=!high;if limited!=next{rrev+=1;}limited=next;}}
            let p=f.store.load_group_policy(f.groups[0]).unwrap();prop_assert_eq!(p.policy_revision,prev);prop_assert!(matches!(p.state,GroupPolicyState::Glm(ref s) if s.pinned==pinned));
            let r=runtime(&f);prop_assert_eq!((r.revision,r.error_epoch,r.consecutive_failures,r.error_exhausted,r.limit_exhausted,r.used_input),(rrev,epoch,count,locked,limited,20));prop_assert_eq!(source_revision(&f,1),srev);
            let before=image(&f.store.conn);prop_assert_eq!(f.store.submit_operation(op(100+i),cmd,NOW+10000).unwrap(),view);prop_assert_eq!(image(&f.store.conn),before);
        }
    }
    #[test]
    fn c2_pb3_discovered_models_match_reference_and_replays_are_readonly(values in prop::collection::vec(0u8..8,1..16), invalid in any::<bool>()) {
        let mut f=fixture();let models=values.iter().map(|v|format!(" m{v} ")).collect::<Vec<_>>();let expected=values.iter().map(|v|format!("m{v}")).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>();
        if invalid {let before=image(&f.store.conn);let mut bad=models.clone();bad.push("bad,model".into());assert_error(f.store.submit_discovered_models(op(200),f.sources[0],1,bad,NOW),Code::InvalidResponse);prop_assert_eq!(image(&f.store.conn),before);}
        let view=f.store.submit_discovered_models(op(200),f.sources[0],1,models,NOW).unwrap();for i in [0,1] {prop_assert_eq!(f.store.load_work(f.members[i],false).unwrap().models,expected.clone());}
        let before=image(&f.store.conn);prop_assert_eq!(f.store.submit_discovered_models(op(200),f.sources[0],1,vec!["other".into()],NOW+1).unwrap(),view);prop_assert_eq!(image(&f.store.conn),before);
    }
}
