use super::*;
use crate::catalog::groups::*;
use crate::newapi::managed_channels::NativeRecoveryCheck;
use crate::reconcile::planner::{BackendReadiness, PlanContext};

#[test]
fn t5c_group_ready_versions_noops_and_errors_are_atomic() {
    let (_dir, mut store, group) = fixture();
    let before = image(&store.conn);
    assert_eq!(
        store.set_group_ready(group, 1, true, NOW).unwrap().revision,
        1
    );
    assert_eq!(image(&store.conn), before);
    assert_eq!(
        store
            .set_group_ready(group, 1, false, NOW)
            .unwrap()
            .revision,
        2
    );
    let before = image(&store.conn);
    assert_eq!(
        store.set_group_ready(group, 1, true, NOW).unwrap_err().code,
        Code::RevisionConflict
    );
    assert_eq!(image(&store.conn), before);
    store.conn.execute_batch("CREATE TRIGGER group_ready_abort BEFORE UPDATE ON source_groups BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(store.set_group_ready(group, 2, true, NOW).is_err());
    assert_eq!(image(&store.conn), before);
    store
        .conn
        .execute_batch("DROP TRIGGER group_ready_abort;")
        .unwrap();
    store
        .conn
        .execute("UPDATE source_groups SET revision=?1", [JSON_SAFE])
        .unwrap();
    let before = image(&store.conn);
    assert!(store.set_group_ready(group, JSON_SAFE, true, NOW).is_err());
    assert_eq!(image(&store.conn), before);
}
fn retire_planned(store: &mut CatalogStore, member: MembershipId) {
    let work = store.load_work(member, false).unwrap();
    store
        .submit_operation(
            OperationId::new("retire-reference-member".into()).unwrap(),
            ControlCommand::RetireMember {
                membership_id: member,
                expected_revision: work.target.revision,
            },
            NOW,
        )
        .unwrap();
    let work = store.load_work(member, false).unwrap();
    let context = PlanContext {
        backend: BackendReadiness::from_preflight("", NativeRecoveryCheck::Unknown, false, 0, 0),
        permit: None,
        expected_members: &[],
        peers: &[],
        now: NOW,
    };
    store
        .confirm_member(member, work.revision(), false, None, &context)
        .unwrap();
}
#[test]
fn t5c_group_reference_schema_counts_pending_unmapped_legacy_and_history() {
    let (_dir, mut store, group_id) = fixture();
    let first = store
        .create_source(source("c-reference-a", SourceKind::Glm))
        .unwrap();
    let second = store
        .create_source(source("c-reference-b", SourceKind::Glm))
        .unwrap();
    store.plan_membership(first.source_id, group_id, 0).unwrap();
    let retired = store
        .plan_membership(second.source_id, group_id, 0)
        .unwrap();
    retire_planned(&mut store, retired.membership_id);
    submit(&mut store, group_id, 0, None);
    let live = store
        .register_token(owner(), &native(77, "token-group"), "live".into(), NOW)
        .unwrap();
    let old = store
        .register_token(owner(), &native(88, "token-group"), "old".into(), NOW)
        .unwrap();
    store
        .submit_token_operation(
            op(1),
            TokenCommand::DeleteToken {
                token_id: old.token_id,
                expected_revision: 1,
            },
            NOW,
        )
        .unwrap();
    let ticket = prepare(&mut store, 1, NOW);
    store
        .settle_token_execution(&ticket, ExecutionOutcome::Responded(None), NOW)
        .unwrap();
    let work = store.token_work(&op(1), NOW).unwrap();
    store
        .confirm_token(&work, TokenObservation::Existing(None), NOW)
        .unwrap();
    for (id, name) in [(99, ""), (100, "auto")] {
        store
            .register_token(owner(), &native(id, name), "legacy".into(), NOW)
            .unwrap();
    }
    let future = store
        .register_token(owner(), &native(101, "later-group"), "unmapped".into(), NOW)
        .unwrap();
    assert_eq!(future.group_id, None);
    let later = store
        .create_group(group("later-group", SourceKind::Glm))
        .unwrap();
    let before = image(&store.conn);
    let refs = store.group_references(group_id).unwrap();
    assert_eq!(refs.schema_version, GROUP_REFERENCES_SCHEMA_VERSION);
    assert_eq!(
        (
            refs.active_memberships,
            refs.historical_memberships,
            refs.live_tokens,
            refs.pending_creations,
            refs.historical_tokens,
            refs.unknown_group_tokens
        ),
        (1, 1, 1, 1, 1, 2)
    );
    assert!(refs.known_local_only && !refs.deletion_authorized);
    assert_eq!(
        store.group_references(later.group_id).unwrap().live_tokens,
        1
    );
    assert_eq!(store.token_identity(live.token_id).unwrap().revision, 1);
    assert_eq!(image(&store.conn), before);
    println!(
        "GROUP_REFERENCES_SCHEMA_JSON={}",
        serde_json::to_value(&refs).unwrap()
    );
    assert_eq!(
        serde_json::to_value(refs)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        10
    );
}
#[test]
fn t5c_token_access_uses_current_revision_but_rejects_static_drift() {
    let (_dir, mut store, group) = fixture();
    let id = submit(&mut store, group, 0, None);
    let ticket = prepare(&mut store, 0, NOW);
    store
        .settle_token_execution(&ticket, ExecutionOutcome::Responded(None), NOW)
        .unwrap();
    let view = materialized(&ticket, 77, TokenStatus::Enabled);
    let work = store.token_work(&op(0), NOW).unwrap();
    store
        .confirm_token(
            &work,
            TokenObservation::Creation(CreationLookup::One(Box::new(view.clone()))),
            NOW,
        )
        .unwrap();
    store
        .submit_token_operation(
            op(1),
            TokenCommand::SetTokenEnabled {
                token_id: id,
                expected_revision: 1,
                enabled: false,
            },
            NOW,
        )
        .unwrap();
    let work = store.token_work(&op(1), NOW).unwrap();
    store.validate_token_work(&work, &view, NOW).unwrap();
    assert!(store.validate_token_access(id, 2, &view).is_err());
    let mut wrong = view.clone();
    wrong.cross_group_retry = true;
    assert_eq!(
        store
            .validate_token_work(&work, &wrong, NOW)
            .unwrap_err()
            .code,
        Code::BindingDrift
    );
    let ticket = store.prepare_token_execution(&work, NOW).unwrap();
    store
        .settle_token_execution(&ticket, ExecutionOutcome::Responded(None), NOW)
        .unwrap();
    let mut disabled = view.clone();
    disabled.status = TokenStatus::Disabled;
    let work = store.token_work(&op(1), NOW).unwrap();
    store
        .confirm_token(
            &work,
            TokenObservation::Existing(Some(disabled.clone())),
            NOW,
        )
        .unwrap();
    store.set_group_ready(group, 1, false, NOW).unwrap();
    store.set_group_ready(group, 2, true, NOW).unwrap();
    store.validate_token_access(id, 2, &disabled).unwrap();
    assert!(store.validate_token_access(id, 1, &disabled).is_err());
    assert!(store.validate_token_access(id, 2, &wrong).is_err());
    let mut imported = native(88, "");
    imported.cross_group_retry = true;
    let legacy = store
        .register_token(owner(), &imported, "legacy".into(), NOW)
        .unwrap();
    store
        .validate_token_access(legacy.token_id, 1, &imported)
        .unwrap();
}
#[test]
fn t5c_delay_cannot_refresh_observation_or_clear_unknown() {
    let (_dir, mut store, group) = fixture();
    submit(&mut store, group, 0, None);
    let ticket = prepare(&mut store, 0, NOW);
    store
        .settle_token_execution(&ticket, ExecutionOutcome::Uncertain, NOW)
        .unwrap();
    let old = store.token_work(&op(0), NOW).unwrap();
    let before = image(&store.conn);
    for at in [NOW - 1, NOW + 60] {
        assert!(store.note_token_delay(&old, at).is_err());
    }
    assert_eq!(image(&store.conn), before);
    let deferred = store.note_token_delay(&old, NOW + 61).unwrap();
    assert_eq!(deferred.phase, OperationPhase::Unknown);
    assert_eq!(deferred.updated_at, NOW + 61);
    assert_eq!(store.next_token_attempt_at().unwrap(), Some(NOW + 91));
    assert!(store.pending_token_operations(NOW + 90).unwrap().is_empty());
    assert_eq!(
        store.pending_token_operations(NOW + 91).unwrap(),
        vec![op(0)]
    );
    assert!(store.note_token_delay(&old, NOW + 62).is_err());
    assert!(store
        .confirm_token(
            &old,
            TokenObservation::Creation(CreationLookup::One(Box::new(materialized(
                &ticket,
                77,
                TokenStatus::Enabled
            )))),
            NOW + 61
        )
        .is_err());
    let current = store.token_work(&op(0), NOW + 61).unwrap();
    assert_eq!(current.create_state(), CreateState::MayHaveBeenSent);
    assert!(store.prepare_token_execution(&current, NOW + 61).is_err());
    // 同phase、同error、同处理时间仍有新证据；不能只比较操作投影。
    let same_projection = store.token_work(&op(0), NOW).unwrap();
    store.note_token_delay(&same_projection, NOW + 61).unwrap();
    assert_eq!(
        store
            .note_token_delay(&same_projection, NOW + 61)
            .unwrap_err()
            .code,
        Code::RevisionConflict
    );
}
#[test]
fn t5c_delay_rollback_and_safe_clock_boundary() {
    let (_dir, mut store, group) = fixture();
    submit(&mut store, group, 0, None);
    let work = store.token_work(&op(0), NOW).unwrap();
    assert_eq!(store.next_token_attempt_at().unwrap(), Some(NOW));
    store.conn.execute_batch("CREATE TRIGGER token_delay_abort BEFORE UPDATE ON control_operations BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    let before = image(&store.conn);
    assert!(store.note_token_delay(&work, NOW + 61).is_err());
    assert_eq!(image(&store.conn), before);
    store
        .conn
        .execute_batch("DROP TRIGGER token_delay_abort;")
        .unwrap();
    let report = store.note_token_delay(&work, JSON_SAFE).unwrap();
    assert_eq!(report.phase, OperationPhase::Waiting);
    assert_eq!(store.next_token_attempt_at().unwrap(), None);
    assert!(store
        .pending_token_operations(JSON_SAFE)
        .unwrap()
        .is_empty());
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn t5c_pb3_group_reference_and_ready_changes_match_independent_model(groups in prop::collection::vec(0u8..4,0..20),changes in prop::collection::vec(any::<bool>(),0..12)) {
        let (_dir,mut store,group)=fixture();
        let mut live=0u32; let mut unknown=0u32;
        for (index,kind) in groups.into_iter().enumerate() {
            let name=match kind {0=>"token-group",1=>"",2=>"auto",_=>"elsewhere"};
            if kind==0 {live+=1;}
            if kind==1||kind==2 {unknown+=1;}
            store.register_token(owner(),&native(index as i64+1,name),format!("import-{index}"),NOW).unwrap();
        }
        let mut revision=1; let mut ready=true;
        for next in changes {
            if ready!=next {revision+=1;}
            let prior=store.group(group).unwrap().revision;
            let result=store.set_group_ready(group,prior,next,NOW).unwrap();
            ready=next;
            prop_assert_eq!((result.revision,result.ready),(revision,ready));
            let refs=store.group_references(group).unwrap();
            prop_assert_eq!((refs.live_tokens,refs.unknown_group_tokens,refs.pending_creations),(live,unknown,0));
            prop_assert!(!refs.deletion_authorized && refs.known_local_only);
        }
    }
}
#[test]
#[ignore = "独立串行release性能门禁"]
fn performance_gate_group_references_10k() {
    let (_dir, mut store, group) = fixture();
    let tx = store.conn.transaction().unwrap();
    for id in 1..=10000 {
        tx.execute("INSERT INTO managed_tokens(owner_user_id,owner_scope,native_token_id,explicit_group,display_name,revision,evidence_revision,created_at) VALUES(41,'issuer',?1,?2,'import',1,0,?3)",params![id,if id%2==0 {"token-group"}else{"auto"},NOW]).unwrap();
    }
    tx.commit().unwrap();
    let start = Instant::now();
    let report = store.group_references(group).unwrap();
    assert_eq!(
        (report.live_tokens, report.unknown_group_tokens),
        (5000, 5000)
    );
    println!("GROUP_REFERENCES_10K_MS={}", start.elapsed().as_millis());
    assert!(start.elapsed() < Duration::from_secs(5));
}
