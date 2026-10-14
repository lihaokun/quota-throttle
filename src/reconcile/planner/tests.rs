use super::*;
use proptest::prelude::*;
use std::time::Duration;

fn fixture(kind: SourceKind) -> WorkSnapshot {
    let source_id = SourceId::new(1).unwrap();
    let group_id = GroupId::new(2).unwrap();
    let membership_id = MembershipId::new(3).unwrap();
    WorkSnapshot {
        source: SourceView {
            source_id,
            kind,
            name: "source-a".into(),
            note: String::new(),
            manual_paused: false,
            retired_at: None,
            config_revision: 7,
        },
        group: GroupView {
            group_id,
            newapi_group: "group-a".into(),
            name: "A".into(),
            kind,
            revision: 9,
            ready: true,
        },
        member: MembershipView {
            membership_id,
            source_id,
            group_id,
            channel_id: Some(ChannelId::new(11).unwrap()),
            order: 0,
            lifecycle: MembershipLifecycle::Ready,
            desired_priority: 100,
            observed_status: None,
            observed_priority: None,
            applied_revision: None,
            retired_at: None,
        },
        target: MemberTarget {
            membership_id,
            revision: 5,
            desired_priority: 100,
            status_directive: StatusDirective::Enabled,
            retire_requested: false,
            create_state: CreateState::Resolved,
            create_name: "qt-instance-m3".into(),
            dirty: true,
            next_attempt_at: 0,
            last_error_code: None,
        },
        identity: ChannelIdentity {
            channel_type: 8,
            group: "group-a".into(),
            base_url: "https://a.invalid/v1/chat/completions".into(),
        },
        models: vec!["model-a".into(), "model-b".into()],
        managed: true,
        source_retire_requested: false,
        policy_disabled: false,
        pending_write: PendingWrite::None,
        execution_revision: 0,
        enable_consumed: false,
        enable_requested_revision: 5,
    }
}
fn observed(snapshot: &WorkSnapshot, status: i64, priority: i64) -> StampedObservation {
    StampedObservation {
        revision: snapshot.revision(),
        channel: ChannelObservation {
            observed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
            channel_id: snapshot.member.channel_id.map(|id| id.get()).unwrap_or(11),
            channel_type: 8,
            name: snapshot.target.create_name.clone(),
            group: snapshot.identity.group.clone(),
            base_url: snapshot.identity.base_url.clone(),
            models: snapshot.models.join(","),
            priority,
            status,
            auto_ban: if snapshot.source.kind == SourceKind::Volcano {
                0
            } else {
                1
            },
            is_multi_key: false,
        },
    }
}
fn peer(id: i64, actual_priority: i64, desired: i64) -> Peer {
    let identity = ChannelIdentity {
        channel_type: 8,
        group: "group-a".into(),
        base_url: format!("https://p{id}.invalid/v1"),
    };
    let revision = Revision {
        member: 2,
        source: 13 + id,
        group: 9,
        execution: 2,
    };
    Peer {
        member_id: MembershipId::new(id).unwrap(),
        group_id: GroupId::new(2).unwrap(),
        revision,
        channel_id: Some(ChannelId::new(100 + id).unwrap()),
        create_state: CreateState::Resolved,
        identity: identity.clone(),
        desired_priority: desired,
        desired_disabled: false,
        pending_write: PendingWrite::None,
        observation: Some(StampedObservation {
            revision,
            channel: ChannelObservation {
                observed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(100),
                channel_id: 100 + id,
                channel_type: 8,
                name: format!("peer-{id}"),
                group: identity.group,
                base_url: identity.base_url,
                models: format!("peer-model-{id}"),
                priority: actual_priority,
                status: 1,
                auto_ban: 0,
                is_multi_key: false,
            },
        }),
    }
}
fn context<'a>(
    snapshot: &WorkSnapshot,
    members: &'a [MembershipId],
    peers: &'a [Peer],
) -> PlanContext<'a> {
    PlanContext {
        backend: BackendReadiness::from_preflight(
            SUPPORTED_BACKEND_VERSION,
            NativeRecoveryCheck::Disabled,
            true,
            90,
            150,
        ),
        permit: Some(RoutingPermit::from_policy(
            &snapshot.source,
            &snapshot.group,
            snapshot.target.desired_priority,
            10,
            true,
            90,
            150,
        )),
        expected_members: members,
        peers,
        now: 100,
    }
}
fn run(
    snapshot: &WorkSnapshot,
    observation: Option<&StampedObservation>,
    peers: &[Peer],
) -> PlanStep {
    let members: Vec<_> = std::iter::once(snapshot.member.membership_id)
        .chain(peers.iter().map(|p| p.member_id))
        .collect();
    plan_next_action(snapshot, observation, &context(snapshot, &members, peers))
}

#[test]
fn stopped_source_is_disabled_before_fields_and_enable_is_not_confirmed() {
    for kind in [SourceKind::Glm, SourceKind::Volcano] {
        let mut snapshot = fixture(kind);
        snapshot.source.manual_paused = true;
        let mut observation = observed(&snapshot, 1, 100);
        assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Disable);
        observation.channel.status = 2;
        assert_eq!(
            run(&snapshot, Some(&observation), &[]),
            PlanStep::Wait(ControlErrorCode::NotReady)
        );
        snapshot.target.status_directive = StatusDirective::Disabled;
        assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Confirm);
    }
}

#[test]
fn stale_observation_identity_drift_and_unmanaged_import_never_write() {
    let snapshot = fixture(SourceKind::Volcano);
    let mut observation = observed(&snapshot, 1, 10);
    observation.revision.source -= 1;
    assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Observe);
    observation.revision = snapshot.revision();
    observation.channel.observed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(69);
    assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Observe);
    observation.channel.observed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(101);
    assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Observe);
    observation.channel.observed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    observation.channel.group = "other".into();
    assert_eq!(
        run(&snapshot, Some(&observation), &[]),
        PlanStep::Reject(ControlErrorCode::BindingDrift)
    );
    let mut unmanaged = snapshot.clone();
    unmanaged.managed = false;
    unmanaged.member.lifecycle = MembershipLifecycle::Imported;
    assert_eq!(
        run(&unmanaged, Some(&observed(&unmanaged, 1, 10)), &[]),
        PlanStep::Wait(ControlErrorCode::NotReady)
    );
}

#[test]
fn missing_expired_wrong_scope_or_unready_permit_never_enables() {
    let snapshot = fixture(SourceKind::Volcano);
    let observation = observed(&snapshot, 2, 10);
    let members = [snapshot.member.membership_id];
    for case in 0..6 {
        let mut ctx = context(&snapshot, &members, &[]);
        match case {
            0 => ctx.permit = None,
            1 => ctx.permit.as_mut().unwrap().expires_at = 100,
            2 => ctx.permit.as_mut().unwrap().source_revision -= 1,
            3 => ctx.permit.as_mut().unwrap().group_id = GroupId::new(99).unwrap(),
            4 => ctx.permit.as_mut().unwrap().allowed = false,
            _ => {
                ctx.backend = BackendReadiness::from_preflight(
                    SUPPORTED_BACKEND_VERSION,
                    NativeRecoveryCheck::Enabled,
                    true,
                    90,
                    150,
                )
            }
        }
        assert_eq!(
            plan_next_action(&snapshot, Some(&observation), &ctx),
            PlanStep::Wait(ControlErrorCode::NotReady)
        );
    }
    let mut unready = snapshot.clone();
    unready.group.ready = false;
    assert_eq!(
        run(&unready, Some(&observation), &[]),
        PlanStep::Wait(ControlErrorCode::NotReady)
    );
}

#[test]
fn volcano_restore_is_standby_then_enable_then_verified_promotion() {
    let snapshot = fixture(SourceKind::Volcano);
    let mut observation = observed(&snapshot, 2, 100);
    let mut peers = vec![peer(4, 100, 100)];
    assert_eq!(
        run(&snapshot, Some(&observation), &peers),
        PlanStep::Patch(ChannelPatch {
            priority: Some(10),
            ..Default::default()
        })
    );
    observation.channel.priority = 10;
    assert_eq!(run(&snapshot, Some(&observation), &peers), PlanStep::Enable);
    observation.channel.status = 1;
    assert_eq!(
        run(&snapshot, Some(&observation), &peers),
        PlanStep::Wait(ControlErrorCode::NotReady)
    );
    peers[0].desired_priority = 10;
    peers[0].revision.member += 1;
    peers[0].observation.as_mut().unwrap().revision = peers[0].revision;
    assert_eq!(
        run(&snapshot, Some(&observation), &peers),
        PlanStep::Wait(ControlErrorCode::NotReady)
    );
    peers[0].observation.as_mut().unwrap().channel.priority = 10;
    assert_eq!(
        run(&snapshot, Some(&observation), &peers),
        PlanStep::Patch(ChannelPatch {
            priority: Some(100),
            ..Default::default()
        })
    );
    observation.channel.priority = 100;
    assert_eq!(
        run(&snapshot, Some(&observation), &peers),
        PlanStep::Confirm
    );
}

#[test]
fn glm_explicit_enable_is_once_and_native_auto_disable_is_preserved() {
    let mut snapshot = fixture(SourceKind::Glm);
    let observation = observed(&snapshot, 3, 100);
    assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Enable);
    snapshot.enable_consumed = true;
    assert_eq!(
        run(&snapshot, Some(&observation), &[]),
        PlanStep::Reject(ControlErrorCode::RemoteRejected)
    );
    snapshot.member.applied_revision = Some(snapshot.target.revision);
    assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Confirm);
    snapshot.member.applied_revision = None;
    snapshot.target.status_directive = StatusDirective::Preserve;
    assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Confirm);
}

#[test]
fn unknown_write_does_not_become_done_and_only_safety_disable_is_allowed() {
    let mut snapshot = fixture(SourceKind::Volcano);
    snapshot.pending_write = PendingWrite::Unknown;
    let mut observation = observed(&snapshot, 1, 100);
    assert_eq!(
        run(&snapshot, Some(&observation), &[]),
        PlanStep::Wait(ControlErrorCode::TransportUnknown)
    );
    snapshot.source.manual_paused = true;
    snapshot.target.status_directive = StatusDirective::Disabled;
    assert_eq!(run(&snapshot, Some(&observation), &[]), PlanStep::Disable);
    observation.channel.status = 2;
    assert_eq!(
        run(&snapshot, Some(&observation), &[]),
        PlanStep::Wait(ControlErrorCode::TransportUnknown)
    );
    snapshot.pending_write = PendingWrite::InFlight;
    observation.channel.status = 1;
    assert_eq!(
        run(&snapshot, Some(&observation), &[]),
        PlanStep::Wait(ControlErrorCode::NotReady)
    );
}

#[test]
fn create_lookup_requires_complete_unique_matching_identity() {
    let mut snapshot = fixture(SourceKind::Volcano);
    snapshot.member.channel_id = None;
    snapshot.member.lifecycle = MembershipLifecycle::Planned;
    snapshot.target.create_state = CreateState::Never;
    assert_eq!(run(&snapshot, None, &[]), PlanStep::CreateDisabled);
    snapshot.source_retire_requested = true;
    assert_eq!(run(&snapshot, None, &[]), PlanStep::RetireAbsent);
    snapshot.target.create_state = CreateState::MayHaveBeenSent;
    assert_eq!(run(&snapshot, None, &[]), PlanStep::LookupCreate);
    assert_eq!(
        resolve_created(&snapshot, true, &[]),
        Ok(CreationMatch::Missing)
    );
    let mut found = observed(&snapshot, 2, 10).channel;
    assert_eq!(
        resolve_created(&snapshot, false, &[found.clone()]),
        Err(ControlErrorCode::InvalidResponse)
    );
    assert_eq!(
        resolve_created(&snapshot, true, &[found.clone()]),
        Ok(CreationMatch::Unique(ChannelId::new(11).unwrap()))
    );
    assert_eq!(
        resolve_created(&snapshot, true, &[found.clone(), found.clone()]),
        Err(ControlErrorCode::AmbiguousCreate)
    );
    found.base_url = "https://different.invalid".into();
    assert_eq!(
        resolve_created(&snapshot, true, &[found]),
        Err(ControlErrorCode::BindingDrift)
    );
}

#[test]
fn barrier_rejects_missing_duplicate_cross_group_and_stale_peer_evidence() {
    let snapshot = fixture(SourceKind::Volcano);
    let observation = observed(&snapshot, 1, 10);
    let members = [snapshot.member.membership_id, MembershipId::new(4).unwrap()];
    let peers = vec![peer(4, 10, 10)];
    let ctx = context(&snapshot, &members, &[]);
    assert_eq!(
        plan_next_action(&snapshot, Some(&observation), &ctx),
        PlanStep::Wait(ControlErrorCode::NotReady)
    );
    for case in 0..5 {
        let mut peers = peers.clone();
        match case {
            0 => peers[0].group_id = GroupId::new(100).unwrap(),
            1 => peers[0].observation.as_mut().unwrap().revision.member -= 1,
            2 => peers[0].pending_write = PendingWrite::Unknown,
            3 => peers[0].observation.as_mut().unwrap().channel.channel_id += 1,
            _ => peers[0].member_id = snapshot.member.membership_id,
        }
        assert!(matches!(
            plan_next_action(
                &snapshot,
                Some(&observation),
                &context(&snapshot, &members, &peers)
            ),
            PlanStep::Wait(_)
        ));
    }
}

#[test]
fn models_update_does_not_enable_glm_or_change_its_auto_ban() {
    let mut snapshot = fixture(SourceKind::Glm);
    snapshot.target.status_directive = StatusDirective::Preserve;
    let mut observation = observed(&snapshot, 3, 100);
    observation.channel.models = "old-model".into();
    assert_eq!(
        run(&snapshot, Some(&observation), &[]),
        PlanStep::Patch(ChannelPatch {
            models: Some("model-a,model-b".into()),
            ..Default::default()
        })
    );
}

#[test]
fn unknown_disabled_only_creation_is_present_but_does_not_block_other_source() {
    let snapshot = fixture(SourceKind::Volcano);
    let observation = observed(&snapshot, 1, 10);
    let members = [snapshot.member.membership_id, MembershipId::new(4).unwrap()];
    let mut p = peer(4, 100, 10);
    p.channel_id = None;
    p.create_state = CreateState::MayHaveBeenSent;
    p.pending_write = PendingWrite::Unknown;
    p.observation = None;
    assert_eq!(
        plan_next_action(
            &snapshot,
            Some(&observation),
            &context(&snapshot, &members, &[p.clone()])
        ),
        PlanStep::Patch(ChannelPatch {
            priority: Some(100),
            ..Default::default()
        })
    );
    p.create_state = CreateState::Resolved;
    assert_eq!(
        plan_next_action(
            &snapshot,
            Some(&observation),
            &context(&snapshot, &members, &[p.clone()])
        ),
        PlanStep::Wait(ControlErrorCode::InvalidResponse)
    );
    p.channel_id = Some(ChannelId::new(104).unwrap());
    assert_eq!(
        plan_next_action(
            &snapshot,
            Some(&observation),
            &context(&snapshot, &members, &[p])
        ),
        PlanStep::Wait(ControlErrorCode::TransportUnknown)
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn r3_stopping_gate_never_emits_enable(paused in any::<bool>(),policy in any::<bool>(),retire in any::<bool>(),status in 1i64..4,volcano in any::<bool>()) {
        let mut snapshot=fixture(if volcano {SourceKind::Volcano}else{SourceKind::Glm});
        snapshot.source.manual_paused=paused;snapshot.policy_disabled=policy;snapshot.target.retire_requested=retire;
        let observation=observed(&snapshot,status,100);let step=run(&snapshot,Some(&observation),&[]);
        if paused||policy||retire {prop_assert_ne!(&step,&PlanStep::Enable);prop_assert_ne!(&step,&PlanStep::Confirm);}
        if (paused||policy||retire)&&status!=2 {prop_assert_eq!(step,PlanStep::Disable);}
    }

    #[test]
    fn r5_maybe_sent_create_never_becomes_a_second_post(paused in any::<bool>(),retired in any::<bool>(),directive in 0usize..3) {
        let mut snapshot=fixture(SourceKind::Volcano);snapshot.member.channel_id=None;snapshot.member.lifecycle=MembershipLifecycle::Planned;
        snapshot.target.create_state=CreateState::MayHaveBeenSent;snapshot.source.manual_paused=paused;snapshot.target.retire_requested=retired;
        snapshot.target.status_directive=[StatusDirective::Preserve,StatusDirective::Enabled,StatusDirective::Disabled][directive];
        prop_assert_eq!(run(&snapshot,None,&[]),PlanStep::LookupCreate);
    }

    #[test]
    fn r7_barrier_matches_independent_peer_rule(values in prop::collection::vec((0i64..130,0i64..130,any::<bool>()),0..8),rotate in 0usize..10) {
        let snapshot=fixture(SourceKind::Volcano);let observation=observed(&snapshot,1,10);
        let mut peers:Vec<_>=values.iter().enumerate().map(|(i,(actual,desired,disabled))|{let mut p=peer(i as i64+4,*actual,*desired);p.desired_disabled = *disabled;p}).collect();
        let members:Vec<_>=std::iter::once(snapshot.member.membership_id).chain(peers.iter().map(|p|p.member_id)).collect();
        if !peers.is_empty() {let length=peers.len();peers.rotate_left(rotate%length);}
        let blocked=values.iter().any(|(actual,desired,disabled)|*disabled||actual>desired||*actual>=100);
        let step=plan_next_action(&snapshot,Some(&observation),&context(&snapshot,&members,&peers));
        if blocked {prop_assert_eq!(step,PlanStep::Wait(ControlErrorCode::NotReady));}
        else {prop_assert_eq!(step,PlanStep::Patch(ChannelPatch {priority:Some(100),..Default::default()}));}
    }

    #[test]
    fn r9_glm_preserve_never_owns_native_status(status in 1i64..4,priority in -20i64..110,auto_ban in 0i64..2) {
        let mut snapshot=fixture(SourceKind::Glm);snapshot.target.status_directive=StatusDirective::Preserve;
        let mut observation=observed(&snapshot,status,priority);observation.channel.auto_ban=auto_ban;
        for _ in 0..2 {
            let step=run(&snapshot,Some(&observation),&[]);
            prop_assert!(!matches!(step,PlanStep::Enable|PlanStep::Disable));
            if let PlanStep::Patch(patch)=step {
                prop_assert!(!patch.disable_auto_ban);
                if let Some(priority)=patch.priority {observation.channel.priority=priority;}
                if let Some(models)=patch.models {observation.channel.models=models;}
                snapshot.execution_revision+=1;observation.revision=snapshot.revision();
            } else {break;}
        }
    }

    #[test]
    fn r12_backend_readiness_requires_all_attested_facts(version in any::<bool>(),recovery in 0usize..3,verified in any::<bool>(),issued in 0i64..200,ttl in 0i64..100,now in 0i64..300) {
        let state=[NativeRecoveryCheck::Disabled,NativeRecoveryCheck::Enabled,NativeRecoveryCheck::Unknown][recovery];
        let ready=BackendReadiness::from_preflight(if version {SUPPORTED_BACKEND_VERSION}else{"unknown"},state,verified,issued,issued+ttl);
        prop_assert_eq!(ready.ready_at(now),version && recovery==0 && verified && now>=issued && now<issued+ttl);
    }

    #[test]
    fn permits_reject_each_independent_scope_or_time_mismatch(delta in 1i64..1000) {
        let snapshot=fixture(SourceKind::Volcano);let observation=observed(&snapshot,2,10);let members=[snapshot.member.membership_id];
        for field in 0..8 {
            let mut ctx=context(&snapshot,&members,&[]);let permit=ctx.permit.as_mut().unwrap();
            match field {
                0=>permit.source_id=SourceId::new(snapshot.source.source_id.get()+delta).unwrap(),
                1=>permit.source_revision+=delta,2=>permit.group_id=GroupId::new(snapshot.group.group_id.get()+delta).unwrap(),
                3=>permit.group_revision+=delta,4=>permit.priority+=delta,5=>permit.issued_at=100+delta,
                6=>permit.expires_at=100, _=>permit.standby_priority=JSON_SAFE+delta,
            }
            prop_assert_eq!(plan_next_action(&snapshot,Some(&observation),&ctx),PlanStep::Wait(ControlErrorCode::NotReady));
        }
    }

    #[test]
    fn peer_evidence_rejects_each_independent_mismatch(delta in 1i64..1000) {
        let snapshot=fixture(SourceKind::Volcano);let observation=observed(&snapshot,1,10);
        let members=[snapshot.member.membership_id,MembershipId::new(4).unwrap()];
        for field in 0..10 {
            let mut p=peer(4,10,10);let observed=p.observation.as_mut().unwrap();
            match field {
                0=>observed.revision.member+=delta,1=>observed.revision.source+=delta,
                2=>{p.revision.group+=delta;observed.revision.group=p.revision.group;},
                3=>observed.channel.channel_id+=delta,4=>observed.channel.channel_type+=delta,
                5=>observed.channel.group=format!("other-{delta}"),6=>observed.channel.base_url=format!("https://other.invalid/{delta}"),
                7=>observed.channel.is_multi_key=true,8=>observed.revision.execution+=delta,
                _=>observed.channel.observed_at=SystemTime::UNIX_EPOCH+Duration::from_secs((100+delta) as u64),
            }
            prop_assert!(matches!(plan_next_action(&snapshot,Some(&observation),&context(&snapshot,&members,&[p])),PlanStep::Wait(_)));
        }
    }

    #[test]
    fn creation_matching_is_complete_unique_and_order_independent(matches in 0usize..4,noise in 0usize..6,wrong_endpoint in any::<bool>(),complete in any::<bool>(),rotation in 0usize..10) {
        let mut snapshot=fixture(SourceKind::Volcano);snapshot.member.channel_id=None;snapshot.member.lifecycle=MembershipLifecycle::Planned;
        snapshot.target.create_state=CreateState::MayHaveBeenSent;
        let mut items=Vec::new();
        for i in 0..matches {let mut c=observed(&snapshot,2,10).channel;c.channel_id=100+i as i64;if wrong_endpoint {c.base_url="https://wrong.invalid".into();}items.push(c);}
        for i in 0..noise {let mut c=observed(&snapshot,1,100).channel;c.channel_id=200+i as i64;c.name=format!("unrelated-{i}");items.push(c);}
        if !items.is_empty() {let count=items.len();items.rotate_left(rotation%count);}
        let expected=if !complete {Err(ControlErrorCode::InvalidResponse)} else if matches>1 {Err(ControlErrorCode::AmbiguousCreate)}
            else if matches==0 {Ok(CreationMatch::Missing)} else if wrong_endpoint {Err(ControlErrorCode::BindingDrift)}
            else {Ok(CreationMatch::Unique(ChannelId::new(100).unwrap()))};
        prop_assert_eq!(resolve_created(&snapshot,complete,&items),expected);
    }
}
