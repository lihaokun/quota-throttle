use super::*;
use crate::catalog::control::{CreateState, MemberTarget, StatusDirective};
use crate::catalog::{ChannelId, MembershipView, SourceView};
use crate::reconcile::planner::{ChannelIdentity, PendingWrite};
use proptest::prelude::*;
use std::time::{Duration, Instant};

const NOW: i64 = 20 * DAY_SECONDS + 18 * 3600;

#[test]
fn policy_evidence_is_not_a_frontend_deserialization_type() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(VolcanoEvidence, serde::Deserialize<'static>);
    assert_not_impl!(OutcomeObservation, serde::Deserialize<'static>);
    assert_not_impl!(VolcanoRuntime, serde::Serialize);
}
fn sid(n: i64) -> SourceId {
    SourceId::new(n).unwrap()
}
fn mid(n: i64) -> MembershipId {
    MembershipId::new(n).unwrap()
}
fn policy() -> VolcanoPolicy {
    VolcanoPolicy {
        daily_token_limit: Some(100),
        ..Default::default()
    }
}
fn seed(p: &VolcanoPolicy) -> VolcanoRuntime {
    initialize_runtime(sid(7), p, NOW).unwrap()
}
fn outcome(kind: OutcomeKind, at: i64) -> OutcomeObservation {
    OutcomeObservation {
        first_event_at: at,
        observed_at: at,
        kind,
    }
}
fn frame(r: &VolcanoRuntime, cursor: i64, kinds: &[OutcomeKind]) -> VolcanoEvidence {
    VolcanoEvidence {
        source_id: r.source_id,
        period_id: r.period_id,
        error_epoch: r.error_epoch,
        from_cursor: r.evidence_cursor,
        to_cursor: cursor,
        used_input: r.used_input,
        used_output: r.used_output,
        coverage_end: NOW + 101,
        outcomes: kinds.iter().map(|k| outcome(*k, NOW + 1)).collect(),
    }
}
fn cfg() -> Config {
    toml::from_str(
        "poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'",
    )
    .unwrap()
}
fn group(n: i64) -> GroupView {
    GroupView {
        group_id: GroupId::new(n).unwrap(),
        kind: SourceKind::Volcano,
        newapi_group: format!("g{n}"),
        name: format!("g{n}"),
        revision: 3,
        ready: true,
    }
}
fn member(source: i64, g: &GroupView) -> WorkSnapshot {
    let id = mid(g.group_id.get() * 10000 + source * 17);
    WorkSnapshot {
        source: SourceView {
            source_id: sid(source),
            kind: SourceKind::Volcano,
            name: format!("s{source}"),
            note: String::new(),
            manual_paused: false,
            retired_at: None,
            config_revision: 4,
        },
        group: g.clone(),
        member: MembershipView {
            membership_id: id,
            source_id: sid(source),
            group_id: g.group_id,
            channel_id: Some(ChannelId::new(500000 + id.get() * 3).unwrap()),
            order: source * 5,
            lifecycle: MembershipLifecycle::Ready,
            desired_priority: 10,
            observed_status: Some(1),
            observed_priority: Some(10),
            applied_revision: None,
            retired_at: None,
        },
        target: MemberTarget {
            membership_id: id,
            revision: 1,
            desired_priority: 10,
            status_directive: StatusDirective::Enabled,
            retire_requested: false,
            create_state: CreateState::Resolved,
            create_name: format!("qt-fake-m{}", id.get()),
            dirty: false,
            next_attempt_at: 0,
            last_error_code: None,
        },
        identity: ChannelIdentity {
            channel_type: 8,
            group: g.newapi_group.clone(),
            base_url: "https://unused.invalid".into(),
        },
        models: vec!["same-model".into()],
        managed: true,
        source_retire_requested: false,
        policy_disabled: false,
        pending_write: PendingWrite::None,
        execution_revision: 0,
        enable_consumed: false,
        enable_requested_revision: 1,
    }
}
fn states(n: usize) -> BTreeMap<SourceId, Option<VolcanoRuntime>> {
    (1..=n)
        .map(|i| {
            (
                sid(i as i64),
                Some(initialize_runtime(sid(i as i64), &policy(), NOW).unwrap()),
            )
        })
        .collect()
}

#[test]
fn default_boundary_is_one_am_utc8_and_overflow_is_not_wrapped() {
    let p = policy();
    let boundary = 20 * DAY_SECONDS + 17 * 3600;
    assert_eq!(
        daily_bounds(boundary - 1, &p).unwrap(),
        (boundary - DAY_SECONDS, boundary)
    );
    assert_eq!(
        daily_bounds(boundary, &p).unwrap(),
        (boundary, boundary + DAY_SECONDS)
    );
    assert_eq!(daily_bounds(-1, &p), Err(PolicyError::InvalidInput));
    assert_eq!(daily_bounds(0, &p), Err(PolicyError::InvalidInput));
    assert_eq!(
        daily_bounds(JSON_SAFE, &p),
        Err(PolicyError::SequenceExhausted)
    );
    let r = seed(&p);
    assert_eq!(r.coverage_end, None);
    assert_eq!(r.evidence_cursor, None);
    for field in 0..5 {
        let mut invalid = p.clone();
        match field {
            0 => invalid.daily_token_limit = Some(0),
            1 => invalid.failure_threshold = 0,
            2 => invalid.reset_minute = 1440,
            3 => invalid.utc_offset_minutes = 841,
            _ => invalid.utc_offset_minutes = -721,
        }
        assert!(daily_bounds(NOW, &invalid).is_err());
    }
}

#[test]
fn independent_reasons_and_latched_error_survive_success() {
    let p = policy();
    let mut r = seed(&p);
    for _ in 0..2 {
        r = apply_outcome(&r, &p, &outcome(OutcomeKind::Unavailable, NOW)).unwrap();
    }
    assert!(!r.error_exhausted);
    r = apply_outcome(&r, &p, &outcome(OutcomeKind::Ignored, NOW)).unwrap();
    assert_eq!(r.consecutive_failures, 2);
    r = apply_outcome(&r, &p, &outcome(OutcomeKind::Unavailable, NOW)).unwrap();
    r = apply_usage(&r, &p, 70, 30).unwrap();
    assert!(r.error_exhausted && r.limit_exhausted);
    assert_eq!(r.state(true), VolcanoState::Paused);
    r = apply_outcome(&r, &p, &outcome(OutcomeKind::Success, NOW)).unwrap();
    assert_eq!(r.consecutive_failures, 0);
    assert!(r.error_exhausted);
    let cleared = clear_error(&r, NOW).unwrap();
    assert!(!cleared.error_exhausted && cleared.limit_exhausted);
    assert_eq!((cleared.used_input, cleared.used_output), (70, 30));
    assert_eq!(cleared.state(false), VolcanoState::Exhausted);
    let raised = VolcanoPolicy {
        daily_token_limit: Some(101),
        ..p.clone()
    };
    let unlocked = replace_policy(&cleared, &p, &raised, NOW).unwrap().0;
    assert_eq!(unlocked.state(false), VolcanoState::Available);
    let only_error = replace_policy(&r, &p, &raised, NOW).unwrap().0;
    assert!(only_error.error_exhausted && !only_error.limit_exhausted);
    let none = VolcanoPolicy {
        daily_token_limit: None,
        ..p.clone()
    };
    assert!(
        !replace_policy(&r, &p, &none, NOW)
            .unwrap()
            .0
            .limit_exhausted
    );
}

#[test]
fn cutoff_period_and_new_error_epoch_exclude_old_events() {
    let p = policy();
    let r = seed(&p);
    let batch = frame(&r, 1, &[OutcomeKind::Unavailable; 3]);
    let used = apply_evidence(&r, &p, &batch, NOW + 100).unwrap().0;
    let cleared = clear_error(&used, NOW + 10).unwrap();
    assert_eq!(
        apply_evidence(&cleared, &p, &batch, NOW + 100).unwrap(),
        (cleared.clone(), EvidenceDisposition::Stale)
    );
    let mut late = batch.clone();
    for event in &mut late.outcomes {
        event.first_event_at = NOW + 20;
        event.observed_at = NOW + 20;
    }
    assert_eq!(
        apply_evidence(&cleared, &p, &late, NOW + 100).unwrap(),
        (cleared.clone(), EvidenceDisposition::Stale)
    );
    for at in [cleared.period_start - 1, NOW + 9, NOW + 10] {
        assert_eq!(
            apply_outcome(&cleared, &p, &outcome(OutcomeKind::Unavailable, at)).unwrap(),
            cleared
        );
    }
    assert_eq!(
        apply_outcome(&cleared, &p, &outcome(OutcomeKind::Unavailable, NOW + 11))
            .unwrap()
            .consecutive_failures,
        1
    );
    let prior_period = OutcomeObservation {
        first_event_at: cleared.period_start - 1,
        observed_at: NOW + 11,
        kind: OutcomeKind::Unavailable,
    };
    assert_eq!(apply_outcome(&cleared, &p, &prior_period).unwrap(), cleared);
    assert!(clear_error(&cleared, NOW + 9).is_err());
    let mut exhausted_epoch = cleared.clone();
    exhausted_epoch.error_epoch = JSON_SAFE;
    assert_eq!(
        clear_error(&exhausted_epoch, NOW + 11),
        Err(PolicyError::SequenceExhausted)
    );
}

#[test]
fn policy_edit_retains_usage_and_due_edit_preserves_transition() {
    let p = policy();
    let r = apply_usage(&seed(&p), &p, 70, 40).unwrap();
    let new = VolcanoPolicy {
        reset_minute: 120,
        ..p.clone()
    };
    let (edited, t) = replace_policy(&r, &p, &new, NOW).unwrap();
    assert!(t.is_none());
    assert_eq!(edited.period_start, r.period_start);
    assert_eq!((edited.used_input, edited.used_output), (70, 40));
    assert!(edited.limit_exhausted);
    assert!(edited.next_reset_at > NOW);
    let (reset, t) = replace_policy(&r, &p, &new, r.next_reset_at).unwrap();
    let t = t.unwrap();
    assert_eq!((t.old_start, t.old_next), (r.period_start, r.next_reset_at));
    assert_eq!(
        (t.new_start, t.new_next),
        (reset.period_start, reset.next_reset_at)
    );
    assert_eq!(
        (t.reset_minute, t.utc_offset_minutes),
        (p.reset_minute, p.utc_offset_minutes)
    );
    assert_eq!(reset.used_input + reset.used_output, 0);
    assert_eq!(reset.state(true), VolcanoState::Paused);
    assert_eq!(
        advance_period(&reset, &new, reset.period_start - 1).unwrap(),
        (reset.clone(), None)
    );
    let mut partial = seed(&p);
    partial.consecutive_failures = 2;
    let low = VolcanoPolicy {
        failure_threshold: 2,
        ..p.clone()
    };
    let latched = replace_policy(&partial, &p, &low, NOW).unwrap().0;
    assert!(latched.error_exhausted);
    assert!(
        replace_policy(&latched, &low, &p, NOW)
            .unwrap()
            .0
            .error_exhausted
    );
}

#[test]
fn evidence_is_atomic_absolute_and_cursor_guarded() {
    let p = policy();
    let r = seed(&p);
    let mut batch = frame(&r, 1, &[OutcomeKind::Unavailable]);
    batch.used_input = 50;
    batch.used_output = 10;
    let (next, d) = apply_evidence(&r, &p, &batch, NOW + 100).unwrap();
    assert_eq!(d, EvidenceDisposition::Applied);
    assert_eq!(
        apply_evidence(&next, &p, &batch, NOW + 100).unwrap(),
        (next.clone(), EvidenceDisposition::Duplicate)
    );
    let mut gap = frame(&next, 3, &[]);
    gap.from_cursor = Some(2);
    assert_eq!(
        apply_evidence(&next, &p, &gap, NOW + 100).unwrap(),
        (next.clone(), EvidenceDisposition::Gap)
    );
    let mut malformed = frame(&next, 2, &[OutcomeKind::Unavailable; 2]);
    malformed.used_input = 80;
    malformed.outcomes[1].observed_at = malformed.outcomes[1].first_event_at - 1;
    assert_eq!(
        apply_evidence(&next, &p, &malformed, NOW + 100),
        Err(PolicyError::InvalidInput)
    );
    assert_eq!(next.consecutive_failures, 1);
    assert_eq!(next.used_input, 50);
    let mut lower = frame(&next, 2, &[]);
    lower.used_output = 9;
    assert!(apply_evidence(&next, &p, &lower, NOW + 100).is_err());
    let mut future = frame(&next, 2, &[]);
    future.coverage_end = NOW + 102;
    assert!(apply_evidence(&next, &p, &future, NOW + 100).is_err());
    let mut other = frame(&next, 2, &[]);
    other.source_id = sid(99);
    assert!(apply_evidence(&next, &p, &other, NOW + 100).is_err());
    assert_eq!(
        apply_usage(&r, &p, JSON_SAFE, 1),
        Err(PolicyError::SequenceExhausted)
    );
}

#[test]
fn empty_maximum_batches_and_invalid_runtime_fail_safely() {
    let p = policy();
    let r = seed(&p);
    let empty = frame(&r, 0, &[]);
    let (zero, _) = apply_evidence(&r, &p, &empty, NOW + 100).unwrap();
    assert_eq!(zero.evidence_cursor, Some(0));
    let max = frame(&r, 1, &vec![OutcomeKind::Ignored; MAX_POLICY_OUTCOMES]);
    assert!(apply_evidence(&r, &p, &max, NOW + 100).is_ok());
    let too_many = frame(&r, 1, &vec![OutcomeKind::Ignored; MAX_POLICY_OUTCOMES + 1]);
    assert_eq!(
        apply_evidence(&r, &p, &too_many, NOW + 100),
        Err(PolicyError::Capacity)
    );
    for mode in 0..7 {
        let mut bad = r.clone();
        match mode {
            0 => bad.revision = 0,
            1 => bad.period_id = 0,
            2 => bad.next_reset_at += 1,
            3 => bad.limit_exhausted = true,
            4 => bad.consecutive_failures = p.failure_threshold,
            5 => bad.evidence_cursor = Some(1),
            _ => bad.error_cutoff_at = Some(bad.next_reset_at),
        }
        assert!(advance_period(&bad, &p, NOW).is_err(), "mode {mode}");
    }
    let mut overflow = r.clone();
    overflow.error_exhausted = true;
    overflow.consecutive_failures = JSON_SAFE;
    assert_eq!(
        apply_outcome(&overflow, &p, &outcome(OutcomeKind::Unavailable, NOW)),
        Err(PolicyError::SequenceExhausted)
    );
    overflow.period_id = JSON_SAFE;
    assert_eq!(
        advance_period(&overflow, &p, overflow.next_reset_at),
        Err(PolicyError::SequenceExhausted)
    );
}

#[test]
fn stable_rotation_and_unknown_members_do_not_steal_preferred() {
    let g = group(1);
    let mut rows: Vec<_> = (1..=3).map(|i| member(i, &g)).collect();
    let mut shared = states(3);
    let first =
        select_volcano_group(&cfg(), &g, &rows, &shared, &VolcanoGroupState::default()).unwrap();
    assert_eq!(first.state.preferred, Some(rows[0].member.membership_id));
    shared
        .get_mut(&sid(1))
        .unwrap()
        .as_mut()
        .unwrap()
        .error_exhausted = true;
    let second = select_volcano_group(&cfg(), &g, &rows, &shared, &first.state).unwrap();
    assert_eq!(second.state.preferred, Some(rows[1].member.membership_id));
    shared
        .get_mut(&sid(1))
        .unwrap()
        .as_mut()
        .unwrap()
        .error_exhausted = false;
    assert_eq!(
        select_volcano_group(&cfg(), &g, &rows, &shared, &second.state)
            .unwrap()
            .state,
        second.state
    );
    rows.retain(|w| w.source.source_id != sid(2));
    let third = select_volcano_group(&cfg(), &g, &rows, &shared, &second.state).unwrap();
    assert_eq!(
        third.state.preferred,
        Some(member(3, &g).member.membership_id)
    );
    for w in &mut rows {
        w.source.manual_paused = true;
    }
    let none = select_volcano_group(&cfg(), &g, &rows, &shared, &third.state).unwrap();
    assert_eq!(none.state.preferred, None);
    assert_eq!(none.state.cursor, third.state.cursor);
    for w in &mut rows {
        w.source.manual_paused = false;
    }
    shared.insert(sid(3), None);
    let wrap = select_volcano_group(&cfg(), &g, &rows, &shared, &none.state).unwrap();
    assert_eq!(
        wrap.state.preferred,
        Some(member(1, &g).member.membership_id)
    );
    assert_eq!(wrap.priorities[1].1, None);
    rows[0].member.channel_id = None;
    assert!(
        select_volcano_group(&cfg(), &g, &rows, &shared, &wrap.state)
            .unwrap()
            .eligible
            .is_empty()
    );
}

#[test]
fn group_validation_precedes_filtering_and_capacity_is_bounded() {
    maximum_selection_case();
}

#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_volcano_maximum_selection() {
    let elapsed = maximum_selection_case();
    println!("VOLCANO_MAXIMUM_SELECTION_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(5));
}

fn maximum_selection_case() -> Duration {
    let g = group(1);
    let shared = states(2);
    for mode in 0..6 {
        let mut rows = vec![member(1, &g), member(2, &g)];
        match mode {
            0 => rows[1].group = group(2),
            1 => rows[1].target.membership_id = rows[0].member.membership_id,
            2 => rows[1].source.kind = SourceKind::Glm,
            3 => rows[1].source.config_revision = 0,
            _ => {
                rows[1].source = rows[0].source.clone();
                rows[1].member.source_id = sid(1);
                if mode == 4 {
                    for w in &mut rows {
                        w.source.manual_paused = true;
                    }
                } else {
                    rows[1].member.channel_id = None;
                    rows[1].member.lifecycle = MembershipLifecycle::Planned;
                }
            }
        }
        assert!(
            select_volcano_group(&cfg(), &g, &rows, &shared, &VolcanoGroupState::default())
                .is_err(),
            "mode {mode}"
        );
    }
    let rows = vec![member(1, &g)];
    assert!(select_volcano_group(
        &cfg(),
        &g,
        &rows,
        &BTreeMap::new(),
        &VolcanoGroupState::default()
    )
    .is_err());
    let invalid = VolcanoGroupState {
        preferred: Some(rows[0].member.membership_id),
        cursor: None,
    };
    assert!(select_volcano_group(&cfg(), &g, &rows, &shared, &invalid).is_err());
    let mut wrong = shared.clone();
    wrong.get_mut(&sid(1)).unwrap().as_mut().unwrap().source_id = sid(2);
    assert!(
        select_volcano_group(&cfg(), &g, &rows, &wrong, &VolcanoGroupState::default()).is_err()
    );
    let mut bad_cfg = cfg();
    bad_cfg.priority_standby = bad_cfg.priority_active;
    assert!(
        select_volcano_group(&bad_cfg, &g, &rows, &shared, &VolcanoGroupState::default()).is_err()
    );
    let mut many = Vec::new();
    for i in 0..MAX_MEMBERSHIPS {
        let mut w = member((i % MAX_SOURCES + 1) as i64, &g);
        w.member.membership_id = mid(1_000_000 + i as i64);
        w.target.membership_id = w.member.membership_id;
        w.member.channel_id = Some(ChannelId::new(2_000_000 + i as i64).unwrap());
        if i >= MAX_SOURCES {
            w.member.lifecycle = MembershipLifecycle::Retired;
            w.member.retired_at = Some(1);
        }
        many.push(w);
    }
    let all = states(MAX_SOURCES);
    let start = Instant::now();
    assert_eq!(
        select_volcano_group(&cfg(), &g, &many, &all, &VolcanoGroupState::default())
            .unwrap()
            .eligible
            .len(),
        MAX_SOURCES
    );
    let elapsed = start.elapsed();
    many.push(member(1, &g));
    assert_eq!(
        select_volcano_group(&cfg(), &g, &many, &all, &VolcanoGroupState::default()),
        Err(PolicyError::Capacity)
    );
    elapsed
}

#[test]
fn retirement_filters_versions_but_does_not_bypass_identity_checks() {
    let g = group(1);
    let shared = states(2);
    for mode in 0..5 {
        let mut rows = vec![member(1, &g), member(2, &g)];
        match mode {
            0 => rows[1].source.retired_at = Some(1),
            1 => rows[1].source_retire_requested = true,
            2 => rows[1].target.retire_requested = true,
            3 => rows[1].member.lifecycle = MembershipLifecycle::Retired,
            _ => rows[1].member.retired_at = Some(1),
        }
        let d = select_volcano_group(&cfg(), &g, &rows, &shared, &VolcanoGroupState::default())
            .unwrap();
        assert_eq!(d.source_revisions, vec![(sid(1), 4, Some(1))]);
        assert_eq!(d.eligible, vec![rows[0].member.membership_id]);
        assert_eq!(d.priorities.len(), 1);
        rows[1].source.kind = SourceKind::Glm;
        assert!(select_volcano_group(&cfg(), &g, &rows, &shared, &d.state).is_err());
    }
}

/// 独立按 UTC 日期枚举附近边界，不调用生产 daily_bounds。
fn calendar_reference(now: i64, p: &VolcanoPolicy) -> (i64, i64) {
    let mut boundaries: Vec<_> = ((now / DAY_SECONDS) - 2..=(now / DAY_SECONDS) + 2)
        .map(|day| day * DAY_SECONDS - p.utc_offset_minutes * 60 + p.reset_minute * 60)
        .collect();
    boundaries.sort();
    (
        *boundaries.iter().filter(|t| **t <= now).max().unwrap(),
        *boundaries.iter().find(|t| **t > now).unwrap(),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn b1_independent_reasons_match_reference(actions in prop::collection::vec((0u8..9,0u16..200),1..80)) {
        let mut p=policy();let mut r=seed(&p);let mut now=NOW;
        let (mut input,mut output,mut streak,mut locked,mut paused)=(0i64,0i64,0i64,false,false);
        for (action,value) in actions {
            now+=1;let value=value as i64;
            match action {
                0=>{streak+=1;locked|=streak>=p.failure_threshold;r=apply_outcome(&r,&p,&outcome(OutcomeKind::Unavailable,now)).unwrap();}
                1=>{streak=0;r=apply_outcome(&r,&p,&outcome(OutcomeKind::Success,now)).unwrap();}
                2=>r=apply_outcome(&r,&p,&outcome(OutcomeKind::Ignored,now)).unwrap(),
                3=>{input+=value;output+=value/2;r=apply_usage(&r,&p,input,output).unwrap();}
                4=>{streak=0;locked=false;r=clear_error(&r,now).unwrap();}
                5=>{let mut new=p.clone();new.daily_token_limit=if value%5==0 {None}else{Some(value+1)};r=replace_policy(&r,&p,&new,now).unwrap().0;p=new;}
                6=>paused= !paused,
                7=>{let mut new=p.clone();new.failure_threshold=value%10+1;locked|=streak>=new.failure_threshold;r=replace_policy(&r,&p,&new,now).unwrap().0;p=new;}
                _=>{now=r.next_reset_at;input=0;output=0;streak=0;locked=false;r=advance_period(&r,&p,now).unwrap().0;}
            }
            let capped=p.daily_token_limit.is_some_and(|limit|input+output>=limit);
            prop_assert_eq!((r.used_input,r.used_output,r.consecutive_failures,r.error_exhausted,r.limit_exhausted),(input,output,streak,locked,capped));
            let expected=if paused {VolcanoState::Paused}else if locked||capped {VolcanoState::Exhausted}else{VolcanoState::Available};
            prop_assert_eq!(r.state(paused),expected);prop_assert_eq!(r.revision,1);
        }
    }
    #[test]
    fn b2_calendar_reset_and_edit_match_independent_dates(
        now in 3*DAY_SECONDS..2_500_000_000i64,minute in 0i64..1440,offset in -720i64..=840,
        new_minute in 0i64..1440,new_offset in -720i64..=840,days in 0i64..3651,tail in 0i64..DAY_SECONDS
    ) {
        let p=VolcanoPolicy {reset_minute:minute,utc_offset_minutes:offset,..policy()};
        let r=initialize_runtime(sid(7),&p,now).unwrap();
        prop_assert_eq!((r.period_start,r.next_reset_at),calendar_reference(now,&p));
        let used=apply_usage(&r,&p,70,40).unwrap();
        let new=VolcanoPolicy {reset_minute:new_minute,utc_offset_minutes:new_offset,..p.clone()};
        let (edited,t)=replace_policy(&used,&p,&new,now).unwrap();
        prop_assert!(t.is_none());prop_assert_eq!(edited.period_start,r.period_start);
        prop_assert_eq!(edited.next_reset_at,calendar_reference(now,&new).1);
        prop_assert_eq!((edited.used_input,edited.used_output,edited.limit_exhausted),(70,40,true));
        let later=r.next_reset_at+days*DAY_SECONDS+tail;
        let (reset,t)=advance_period(&used,&p,later).unwrap();
        prop_assert_eq!((reset.period_start,reset.next_reset_at),calendar_reference(later,&p));
        prop_assert_eq!((reset.period_id,reset.error_epoch,reset.revision),(2,2,1));
        prop_assert_eq!(reset.state(true),VolcanoState::Paused);
        prop_assert_eq!(reset.used_input+reset.used_output,0);
        prop_assert_eq!(t.unwrap().old_next,r.next_reset_at);
        prop_assert_eq!(advance_period(&reset,&p,later).unwrap(),(reset.clone(),None));
        prop_assert_eq!(advance_period(&reset,&p,now).unwrap(),(reset,None));
    }
    #[test]
    fn b3_split_replay_gap_and_epoch_match_reference(
        values in prop::collection::vec((0u8..3,0u8..12,0u8..6),1..50),chunk in 1usize..9,
        controls in prop::collection::vec((0u8..3,0u16..10,0u8..5),1..20)
    ) {
        let p=policy();let mut actual=seed(&p);let mut now=NOW;let mut sequence=0;
        // 仅共享数据表示；初值、游标、代次、覆盖和状态转移均不读取 actual。
        let (start,end)=calendar_reference(now,&p);
        let mut expected=VolcanoRuntime {
            source_id:sid(7),revision:1,period_id:1,period_start:start,next_reset_at:end,
            used_input:0,used_output:0,consecutive_failures:0,error_exhausted:false,
            limit_exhausted:false,error_epoch:1,error_cutoff_at:None,evidence_cursor:None,coverage_end:None,
        };
        prop_assert_eq!(&actual,&expected);
        for (i,part) in values.chunks(chunk).enumerate() {
            let mut batch=VolcanoEvidence {
                source_id:sid(7),period_id:expected.period_id,error_epoch:expected.error_epoch,
                from_cursor:expected.evidence_cursor,to_cursor:0,used_input:expected.used_input,
                used_output:expected.used_output,coverage_end:0,outcomes:Vec::new(),
            };
            for (kind,inc,out) in part {
                now+=1;sequence+=1;
                expected.used_input+=*inc as i64;expected.used_output+=*out as i64;
                let kind=match kind {0=>OutcomeKind::Success,1=>OutcomeKind::Unavailable,_=>OutcomeKind::Ignored};
                match kind {
                    OutcomeKind::Success=>expected.consecutive_failures=0,
                    OutcomeKind::Unavailable=>{expected.consecutive_failures+=1;expected.error_exhausted|=expected.consecutive_failures>=3;},
                    OutcomeKind::Ignored=>{}
                }
                batch.outcomes.push(outcome(kind,now));
            }
            expected.limit_exhausted=expected.used_input+expected.used_output>=100;
            expected.evidence_cursor=Some(sequence);expected.coverage_end=Some(now+1);
            batch.to_cursor=sequence;batch.used_input=expected.used_input;batch.used_output=expected.used_output;batch.coverage_end=now+1;
            let (next,d)=apply_evidence(&actual,&p,&batch,now).unwrap();actual=next;
            prop_assert_eq!(d,EvidenceDisposition::Applied);prop_assert_eq!(&actual,&expected);
            prop_assert_eq!(apply_evidence(&actual,&p,&batch,now).unwrap(),(expected.clone(),EvidenceDisposition::Duplicate));
            let mut fresh=batch.clone();fresh.from_cursor=Some(sequence);fresh.to_cursor=sequence+1;
            let mut gap=fresh.clone();gap.from_cursor=None;
            prop_assert_eq!(apply_evidence(&actual,&p,&gap,now).unwrap(),(expected.clone(),EvidenceDisposition::Gap));
            let mut wrong_period=fresh.clone();wrong_period.period_id+=1;
            prop_assert_eq!(apply_evidence(&actual,&p,&wrong_period,now).unwrap(),(expected.clone(),EvidenceDisposition::Stale));

            let (control,days,invalid_kind)=controls[i%controls.len()];
            let mut invalid=fresh.clone();
            match invalid_kind {
                0=>invalid.used_input= -1,
                1=>invalid.outcomes.push(OutcomeObservation{first_event_at:now+1,observed_at:now,kind:OutcomeKind::Unavailable}),
                2=>{invalid.outcomes.clear();invalid.coverage_end=expected.period_start-1;},
                3=>invalid.source_id=sid(8),
                _=>invalid.to_cursor=0,
            }
            prop_assert!(apply_evidence(&actual,&p,&invalid,now).is_err());
            prop_assert_eq!(&actual,&expected);
            if control==0 {
                actual=clear_error(&actual,now).unwrap();
                expected.error_epoch+=1;expected.error_exhausted=false;expected.consecutive_failures=0;
                expected.error_cutoff_at=Some(now);expected.evidence_cursor=None;
                prop_assert_eq!(&actual,&expected);
                // 撤错前的代次即使包含 cutoff 后的结果也不得接受。
                let mut old=fresh.clone();old.from_cursor=None;old.coverage_end=now+2;
                old.outcomes=vec![outcome(OutcomeKind::Unavailable,now+1);3];
                prop_assert_eq!(apply_evidence(&actual,&p,&old,now+1).unwrap(),(expected.clone(),EvidenceDisposition::Stale));
                // 新投影保留用量，但同秒/迟到的旧结果不重建错误计数。
                let mut rebuilt=fresh.clone();rebuilt.error_epoch=expected.error_epoch;rebuilt.from_cursor=None;
                rebuilt.outcomes=vec![outcome(OutcomeKind::Unavailable,now);3];
                sequence+=1;rebuilt.to_cursor=sequence;expected.evidence_cursor=Some(sequence);
                let (next,d)=apply_evidence(&actual,&p,&rebuilt,now).unwrap();actual=next;
                prop_assert_eq!(d,EvidenceDisposition::Applied);prop_assert_eq!(&actual,&expected);
            } else if control==1 {
                now=expected.next_reset_at+i64::from(days)*DAY_SECONDS;
                let (start,end)=calendar_reference(now,&p);
                expected.period_id+=1;expected.error_epoch+=1;expected.period_start=start;expected.next_reset_at=end;
                expected.used_input=0;expected.used_output=0;expected.consecutive_failures=0;
                expected.error_exhausted=false;expected.limit_exhausted=false;
                expected.error_cutoff_at=None;expected.evidence_cursor=None;expected.coverage_end=None;
                actual=advance_period(&actual,&p,now).unwrap().0;prop_assert_eq!(&actual,&expected);
                prop_assert_eq!(apply_evidence(&actual,&p,&fresh,now).unwrap(),(expected.clone(),EvidenceDisposition::Stale));
                // 单独检验周期门禁，不能依赖 epoch 同时改变而蒙混通过。
                let mut old_period=fresh.clone();old_period.error_epoch=expected.error_epoch;
                prop_assert_eq!(apply_evidence(&actual,&p,&old_period,now).unwrap(),(expected.clone(),EvidenceDisposition::Stale));
                // 旧周期迟到结果在新投影里不计失败，覆盖/游标仍可推进。
                let mut late=VolcanoEvidence {source_id:sid(7),period_id:expected.period_id,error_epoch:expected.error_epoch,
                    from_cursor:None,to_cursor:sequence+1,used_input:0,used_output:0,coverage_end:now+1,
                    outcomes:vec![outcome(OutcomeKind::Unavailable,start-1);3]};
                sequence+=1;late.to_cursor=sequence;expected.evidence_cursor=Some(sequence);expected.coverage_end=Some(now+1);
                let (next,d)=apply_evidence(&actual,&p,&late,now).unwrap();actual=next;
                prop_assert_eq!(d,EvidenceDisposition::Applied);prop_assert_eq!(&actual,&expected);
            }
        }
    }
    #[test]
    fn b4_two_groups_follow_independent_sticky_rings(n in 1usize..12,actions in prop::collection::vec((0usize..12,0u8..3),1..65)) {
        let groups=[group(1),group(2)];
        let mut rows:Vec<Vec<_>>=groups.iter().map(|g|(1..=n).map(|i|member(i as i64,g)).collect()).collect();
        let mut shared=states(n);let mut failed=vec![false;n];let mut paused=vec![false;n];let mut alive=vec![true;n];
        let mut previous=[VolcanoGroupState::default(),VolcanoGroupState::default()];
        let mut current=[None::<usize>;2];let mut cursor=[None::<usize>;2];
        for (index,action) in actions {
            let i=index%n;
            match action {
                0=>{failed[i]= !failed[i];let r=shared.get_mut(&sid(i as i64+1)).unwrap().as_mut().unwrap();r.error_exhausted=failed[i];r.revision+=1;}
                1=>{paused[i]= !paused[i];for group_rows in &mut rows {for w in group_rows {if w.source.source_id==sid(i as i64+1){w.source.manual_paused=paused[i];w.source.config_revision+=1;}}}}
                _=>{alive[i]=false;rows[0].retain(|w|w.source.source_id!=sid(i as i64+1));}
            }
            let saved=shared.clone();
            for g in 0..2 {
                let eligible:Vec<_>=(0..n).filter(|i|(g==1||alive[*i])&&!failed[*i]&&!paused[*i]).collect();
                if !current[g].is_some_and(|i|eligible.contains(&i)) {
                    current[g]=(0..n).map(|offset|cursor[g].map_or(offset,|last|(last+1+offset)%n)).find(|i|eligible.contains(i));
                    if let Some(chosen)=current[g]{cursor[g]=Some(chosen);}
                }
                let d=select_volcano_group(&cfg(),&groups[g],&rows[g],&shared,&previous[g]).unwrap();
                prop_assert_eq!((d.group_id,d.group_revision),(groups[g].group_id,groups[g].revision));
                let expected_versions:Vec<_>=rows[g].iter().map(|w|(w.source.source_id,w.source.config_revision,shared[&w.source.source_id].as_ref().map(|r|r.revision))).collect();
                prop_assert_eq!(&d.source_revisions,&expected_versions);
                let expected=current[g].map(|i|member(i as i64+1,&groups[g]).member.membership_id);
                prop_assert_eq!(d.state.preferred,expected);
                prop_assert_eq!(d.state.cursor,cursor[g].map(|i|{let w=member(i as i64+1,&groups[g]);(w.member.order,w.member.membership_id)}));
                prop_assert_eq!(&d.eligible,&eligible.iter().map(|i|member(*i as i64+1,&groups[g]).member.membership_id).collect::<Vec<_>>());
                for (id,priority) in &d.priorities {
                    let w=rows[g].iter().find(|w|w.member.membership_id==*id).unwrap();let i=w.source.source_id.get() as usize-1;
                    let expected=if failed[i]||paused[i]{0}else if Some(*id)==expected{100}else{10};
                    prop_assert_eq!(*priority,Some(expected));
                }
                previous[g]=d.state;
            }
            prop_assert_eq!(&shared,&saved);
        }
    }
}
