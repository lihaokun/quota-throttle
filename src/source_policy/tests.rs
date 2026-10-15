use super::glm::*;
use super::probe::*;
use super::{reference, PolicyError};
use crate::catalog::control::{CreateState, MemberTarget, StatusDirective};
use crate::catalog::*;
use crate::config::{Config, HeaderKV, Window};
use crate::orchestrator::{glm_target_priority, Regime, WeeklyInfo};
use crate::quota::{QuotaStatus, WindowStatus};
use crate::reconcile::planner::{ChannelIdentity, PendingWrite, WorkSnapshot};
use proptest::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const NOW: i64 = 1_800_000_000_000;
const SECRET: &str = "fake-only-secret-never-production";
const SELECTOR: &str = "fake-private-selector";

fn cfg() -> Config {
    toml::from_str(
        "poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\nbase_url='http://unused.invalid'",
    )
    .unwrap()
}
fn mid(id: i64) -> MembershipId {
    MembershipId::new(id).unwrap()
}
fn sid(id: i64) -> SourceId {
    SourceId::new(id).unwrap()
}
fn work(id: i64) -> WorkSnapshot {
    let group = GroupView {
        group_id: GroupId::new(1).unwrap(),
        newapi_group: "group-a".into(),
        name: "A".into(),
        kind: SourceKind::Glm,
        revision: 9,
        ready: true,
    };
    WorkSnapshot {
        source: SourceView {
            source_id: sid(id),
            kind: SourceKind::Glm,
            name: format!("s{id}"),
            note: String::new(),
            manual_paused: false,
            retired_at: None,
            config_revision: 7,
        },
        member: MembershipView {
            membership_id: mid(id),
            source_id: sid(id),
            group_id: group.group_id,
            channel_id: Some(ChannelId::new(10000 + id * 7).unwrap()),
            order: id,
            lifecycle: MembershipLifecycle::Ready,
            desired_priority: 10,
            observed_status: Some(1),
            observed_priority: Some(10),
            applied_revision: None,
            retired_at: None,
        },
        target: MemberTarget {
            membership_id: mid(id),
            revision: 5,
            desired_priority: 10,
            status_directive: StatusDirective::Preserve,
            retire_requested: false,
            create_state: CreateState::Resolved,
            create_name: format!("qt-test-m{id}"),
            dirty: false,
            next_attempt_at: 0,
            last_error_code: None,
        },
        identity: ChannelIdentity {
            channel_type: 8,
            group: group.newapi_group.clone(),
            base_url: "https://unused.invalid".into(),
        },
        group,
        models: vec!["model-a".into()],
        managed: true,
        source_retire_requested: false,
        policy_disabled: false,
        pending_write: PendingWrite::None,
        execution_revision: 0,
        enable_consumed: true,
        enable_requested_revision: 5,
    }
}
fn regroup(members: &mut [WorkSnapshot], group_id: i64) {
    for w in members {
        w.group.group_id = GroupId::new(group_id).unwrap();
        w.group.newapi_group = format!("g{group_id}");
        w.member.group_id = w.group.group_id;
        w.identity.group = w.group.newapi_group.clone();
        w.member.membership_id = mid(w.member.membership_id.get() + group_id * 100_000);
        w.target.membership_id = w.member.membership_id;
        w.target.create_name = format!("qt-test-m{}", w.member.membership_id.get());
        w.member.channel_id = w
            .member
            .channel_id
            .map(|id| ChannelId::new(id.get() + group_id * 100_000).unwrap());
    }
}
fn quota(five: f64, weekly: f64, delta_hours: i32) -> QuotaStatus {
    QuotaStatus {
        five_hour: Some(WindowStatus {
            percentage: five,
            next_reset_time: NOW + 3_600_000,
        }),
        weekly: Some(WindowStatus {
            percentage: weekly,
            next_reset_time: NOW + delta_hours as i64 * 3_600_000,
        }),
        level: Some("fake-plan".into()),
    }
}
fn cycle(values: &[Option<f64>]) -> ProbeCycle {
    ProbeCycle {
        cycle_id: 1,
        completed_at_ms: NOW,
        observations: values
            .iter()
            .enumerate()
            .map(|(i, p)| QuotaObservation {
                source_id: sid(i as i64 + 1),
                config_revision: 7,
                cycle_id: 1,
                observed_at_ms: NOW,
                reading: p.map_or(QuotaReading::Unavailable, |p| {
                    QuotaReading::Available(quota(p, p, 48))
                }),
            })
            .collect(),
    }
}
fn decide(members: &[WorkSnapshot], cycle: &ProbeCycle, state: &GlmGroupState) -> GlmGroupDecision {
    decide_glm_group(&cfg(), &work(1).group, members, cycle, state, NOW).unwrap()
}

/// 参考运行原生 channel ID，生产适配器运行 MembershipId；避免相同 ID 掩盖错映射。
fn oracle(
    cfg: &Config,
    members: &[WorkSnapshot],
    cycle: &ProbeCycle,
    previous: &GlmGroupState,
) -> GlmGroupDecision {
    let mut ordered: Vec<_> = members.iter().collect();
    ordered.sort_by_key(|w| (w.member.order, w.member.membership_id));
    let ids: Vec<_> = ordered
        .iter()
        .map(|w| w.member.channel_id.unwrap().get())
        .collect();
    let from_native: HashMap<_, _> = ordered
        .iter()
        .map(|w| (w.member.channel_id.unwrap().get(), w.member.membership_id))
        .collect();
    let to_native: HashMap<_, _> = from_native.iter().map(|(a, b)| (*b, *a)).collect();
    let mut pct = HashMap::new();
    let mut weekly = HashMap::new();
    for w in &ordered {
        let Some(observed) = cycle
            .observations
            .iter()
            .find(|r| r.source_id == w.source.source_id)
        else {
            continue;
        };
        let QuotaReading::Available(q) = &observed.reading else {
            continue;
        };
        let p = cfg
            .watch_windows
            .iter()
            .filter_map(|window| q.window(*window))
            .fold(0.0_f64, |a, w| a.max(w.percentage));
        let id = w.member.channel_id.unwrap().get();
        pct.insert(id, p);
        if let Some(w) = &q.weekly {
            weekly.insert(
                id,
                WeeklyInfo {
                    reset_ms: w.next_reset_time,
                    pct: w.percentage,
                    max_pct: p,
                },
            );
        }
    }
    let active = previous.active.and_then(|id| to_native.get(&id).copied());
    let pinned = previous.pinned.and_then(|id| to_native.get(&id).copied());
    let d = reference::decide(
        &ids,
        &pct,
        &weekly,
        NOW,
        cfg.weekly_reset_lookahead_hours as i64 * 3_600_000,
        active,
        pinned,
        cfg.throttle_threshold,
        cfg.restore_threshold,
        cfg.exhausted_threshold,
    );
    let active = d.active.or(active);
    let priorities = ids
        .iter()
        .map(|id| {
            let p = if Some(*id) == active {
                Some(cfg.priority_active)
            } else if d.eligible.contains(id) {
                Some(cfg.priority_standby)
            } else if pct.contains_key(id) {
                Some(cfg.priority_exhausted)
            } else {
                None
            };
            (from_native[id], p)
        })
        .collect();
    GlmGroupDecision {
        group_id: work(1).group.group_id,
        group_revision: 9,
        cycle_id: cycle.cycle_id,
        state: GlmGroupState {
            active: active.map(|id| from_native[&id]),
            pinned: if d.pin_release.is_some() {
                None
            } else {
                pinned.map(|id| from_native[&id])
            },
            regime: d.regime,
        },
        eligible: d.eligible.iter().map(|id| from_native[id]).collect(),
        imminent: d.imminent.iter().map(|id| from_native[id]).collect(),
        pin_release: d.pin_release.map(|r| GlmPinRelease {
            membership_id: from_native[&r.channel_id],
            pct: r.pct,
            limit: r.limit,
        }),
        priorities,
    }
}

#[test]
fn golden_thresholds_degraded_unknown_and_pin() {
    let members = [work(1), work(2), work(3)];
    let state = GlmGroupState {
        active: Some(mid(1)),
        pinned: Some(mid(1)),
        regime: Regime::Normal,
    };
    let d = decide(&members, &cycle(&[Some(95.), Some(94.), None]), &state);
    assert_eq!(d.state.active, Some(mid(2)));
    assert_eq!(d.state.pinned, None);
    assert_eq!(
        d.pin_release,
        Some(GlmPinRelease {
            membership_id: mid(1),
            pct: 95.,
            limit: 95.
        })
    );
    assert_eq!(
        d.priorities,
        vec![(mid(1), Some(0)), (mid(2), Some(100)), (mid(3), None)]
    );
    let d = decide(
        &members,
        &cycle(&[Some(96.), Some(99.), Some(100.)]),
        &state,
    );
    assert_eq!(d.state.regime, Regime::Degraded);
    assert_eq!(d.state.pinned, Some(mid(1)));
    assert_eq!(
        d.priorities,
        vec![(mid(1), Some(100)), (mid(2), Some(10)), (mid(3), Some(0))]
    );
    let d = decide(&members, &cycle(&[Some(100.); 3]), &state);
    assert_eq!(d.state.active, Some(mid(1)));
    assert_eq!(d.priorities[0].1, Some(100));
    let d = decide(&members, &cycle(&[None; 3]), &state);
    assert_eq!(d.state, state);
    assert_eq!(
        d.priorities,
        vec![(mid(1), Some(100)), (mid(2), None), (mid(3), None)]
    );
}

#[test]
fn golden_imminence_stickiness_and_native_status() {
    let mut members = [work(1), work(2)];
    members[0].member.observed_status = Some(3);
    members[1].member.observed_status = Some(2);
    members[1].member.lifecycle = MembershipLifecycle::CreatedDisabled;
    let mut c = cycle(&[Some(10.), Some(80.)]);
    c.observations[1].reading = QuotaReading::Available(quota(80., 80., 1));
    assert_eq!(
        decide(&members, &c, &GlmGroupState::default()).state.active,
        Some(mid(2))
    );
    let state = GlmGroupState {
        active: Some(mid(1)),
        ..GlmGroupState::default()
    };
    assert_eq!(decide(&members, &c, &state).state.active, Some(mid(1)));
    c.observations[1].reading = QuotaReading::Available(quota(100., 20., 1));
    assert!(decide(&members, &c, &GlmGroupState::default())
        .imminent
        .is_empty());
    let mut cfg = cfg();
    cfg.weekly_reset_lookahead_hours = 0;
    assert!(
        decide_glm_group(&cfg, &members[0].group, &members, &c, &state, NOW)
            .unwrap()
            .imminent
            .is_empty()
    );
}

#[test]
fn paused_retiring_unbound_and_empty_groups_clear_invalid_pin() {
    for mode in 0..7 {
        let mut members = [work(1), work(2)];
        match mode {
            0 => members[0].source.manual_paused = true,
            1 => members[0].source.retired_at = Some(1),
            2 => members[0].source_retire_requested = true,
            3 => members[0].target.retire_requested = true,
            4 => members[0].member.lifecycle = MembershipLifecycle::Retired,
            5 => members[0].member.retired_at = Some(1),
            _ => members[0].member.channel_id = None,
        }
        let state = GlmGroupState {
            active: Some(mid(1)),
            pinned: Some(mid(1)),
            regime: Regime::Normal,
        };
        let d = decide(&members, &cycle(&[None, Some(10.)]), &state);
        assert_eq!(d.state.active, Some(mid(2)), "mode {mode}");
        assert_eq!(d.state.pinned, None);
        assert_eq!(d.priorities.len(), 1);
        assert_eq!(
            decide(&[], &cycle(&[]), &state).state,
            GlmGroupState::default()
        );
    }
}

#[test]
fn invalid_snapshots_are_rejected_before_filtering() {
    for mode in 0..9 {
        let mut members = vec![work(1), work(2)];
        match mode {
            0 => members[1].member.group_id = GroupId::new(2).unwrap(),
            1 => members[1].group.revision += 1,
            2 => members[1].source.kind = SourceKind::Volcano,
            3 => members[1].member.source_id = sid(99),
            4 => members[1].target.membership_id = mid(1),
            5 => members[1] = members[0].clone(),
            6 => {
                members[1].source = members[0].source.clone();
                members[1].member.source_id = sid(1);
            }
            7 => members[1].source.config_revision = 0,
            _ => members[1].group.kind = SourceKind::Volcano,
        }
        let result = decide_glm_group(
            &cfg(),
            &work(1).group,
            &members,
            &cycle(&[Some(10.); 2]),
            &GlmGroupState::default(),
            NOW,
        );
        assert_eq!(result, Err(PolicyError::InvalidInput), "mode {mode}");
    }
    let mut c = cycle(&[None]);
    c.observations.push(c.observations[0].clone());
    assert_eq!(
        decide_glm_group(
            &cfg(),
            &work(1).group,
            &[],
            &c,
            &GlmGroupState::default(),
            NOW
        ),
        Err(PolicyError::InvalidInput)
    );
    assert_eq!(
        decide_glm_group(
            &cfg(),
            &work(1).group,
            &[],
            &cycle(&[]),
            &GlmGroupState::default(),
            -1
        ),
        Err(PolicyError::InvalidInput)
    );
}

#[test]
fn capacity_bounds_and_maximum_snapshot_are_finite() {
    let mut members = Vec::new();
    for i in 0..MAX_MEMBERSHIPS {
        let mut w = work((i % MAX_SOURCES + 1) as i64);
        w.member.membership_id = mid(i as i64 + 1);
        w.target.membership_id = w.member.membership_id;
        w.member.channel_id = Some(ChannelId::new(10000 + i as i64 * 7).unwrap());
        if i >= MAX_SOURCES {
            w.member.lifecycle = MembershipLifecycle::Retired;
            w.member.retired_at = Some(1);
        }
        members.push(w);
    }
    let start = Instant::now();
    assert_eq!(
        decide(
            &members,
            &cycle(&vec![Some(50.); MAX_SOURCES]),
            &GlmGroupState::default()
        )
        .priorities
        .len(),
        MAX_SOURCES
    );
    assert!(start.elapsed() < Duration::from_secs(5));
    members.push(work(99999));
    assert_eq!(
        decide_glm_group(
            &cfg(),
            &work(1).group,
            &members,
            &cycle(&[]),
            &GlmGroupState::default(),
            NOW
        ),
        Err(PolicyError::Capacity)
    );
}

#[test]
fn duplicate_source_group_is_invalid_even_when_paused_unbound_or_retiring() {
    for mode in 0..4 {
        let mut members = vec![work(1), work(2)];
        members[1].source = members[0].source.clone();
        members[1].member.source_id = sid(1);
        match mode {
            0 => {
                for w in &mut members {
                    w.source.manual_paused = true;
                }
            }
            1 => {
                members[1].member.channel_id = None;
                members[1].member.lifecycle = MembershipLifecycle::Planned;
            }
            2 => members[1].target.retire_requested = true,
            _ => {
                members[1].member.lifecycle = MembershipLifecycle::Retired;
                members[1].member.retired_at = Some(1);
            }
        }
        let result = decide_glm_group(
            &cfg(),
            &work(1).group,
            &members,
            &cycle(&[Some(10.)]),
            &GlmGroupState::default(),
            NOW,
        );
        if mode == 3 {
            assert_eq!(result.unwrap().priorities.len(), 1);
        } else {
            assert_eq!(result, Err(PolicyError::InvalidInput), "mode {mode}");
        }
    }
}

#[test]
fn priority_helper_exhaustive_branch_order() {
    let mut cfg = cfg();
    cfg.priority_active = 43;
    cfg.priority_standby = -7;
    cfg.priority_exhausted = -99;
    for active in [false, true] {
        for eligible in [false, true] {
            for known in [false, true] {
                let expected = if active {
                    Some(43)
                } else if eligible {
                    Some(-7)
                } else if known {
                    Some(-99)
                } else {
                    None
                };
                assert_eq!(
                    glm_target_priority(1, active.then_some(1), eligible, known, &cfg),
                    expected
                );
            }
        }
    }
}

#[test]
fn unrepresentable_upstream_reset_is_rejected_without_panicking() {
    let mut c = cycle(&[Some(10.)]);
    let QuotaReading::Available(q) = &mut c.observations[0].reading else {
        unreachable!()
    };
    q.weekly.as_mut().unwrap().next_reset_time = i64::MIN;
    assert_eq!(
        decide_glm_group(
            &cfg(),
            &work(1).group,
            &[work(1)],
            &c,
            &GlmGroupState::default(),
            NOW
        ),
        Err(PolicyError::InvalidInput)
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn a1_complete_glm_chain_matches_frozen_reference(
        values in prop::collection::vec(prop::option::of((0u8..=110, 0u8..=110, -2i32..=30)), 0..20),
        active in 0usize..22, pin in 0usize..22, reverse in any::<bool>(), source_rotation in 0usize..20,
        threshold in 1u8..=100, restore_fraction in 1u8..=100, lookahead in 0u64..=168,
        priorities in (any::<i16>(), any::<i16>(), any::<i16>()), windows in 0u8..4,
    ) {
        let mut cfg = cfg(); cfg.throttle_threshold = threshold as f64;
        cfg.restore_threshold = threshold as f64 * restore_fraction as f64 / 100.;
        cfg.weekly_reset_lookahead_hours = lookahead;
        (cfg.priority_active, cfg.priority_standby, cfg.priority_exhausted) = (priorities.0 as i64, priorities.1 as i64, priorities.2 as i64);
        cfg.watch_windows = match windows { 0 => vec![], 1 => vec![Window::FiveHour], 2 => vec![Window::Weekly], _ => vec![Window::FiveHour, Window::Weekly] };
        let mut members: Vec<_> = (1..=values.len()).map(|id| { let mut w = work(id as i64); w.member.order = (id % 3) as i64; w.member.membership_id = mid(id as i64 * 17 + 501); w.target.membership_id = w.member.membership_id; w.source = work(((id - 1 + source_rotation) % values.len() + 1) as i64).source; w.member.source_id = w.source.source_id; w }).collect();
        if reverse { members.reverse(); }
        let mut c = cycle(&vec![None; values.len()]);
        for (r, v) in c.observations.iter_mut().zip(values) { if let Some((f,w,h)) = v { r.reading = QuotaReading::Available(quota(f as f64,w as f64,h)); } }
        let state = GlmGroupState { active: (active > 0).then(|| mid(active as i64 * 17 + 501)), pinned: (pin > 0).then(|| mid(pin as i64 * 17 + 501)), regime: Regime::Degraded };
        let expected = oracle(&cfg, &members, &c, &state);
        let actual = decide_glm_group(&cfg, &work(1).group, &members, &c, &state, NOW).unwrap();
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn a2_group_isolation_with_shared_source(a in 0u8..=100, b in 0u8..=100, other in 0u8..=100, other_count in 1usize..15) {
        let a_members = vec![work(1), work(2)];
        let mut b_members: Vec<_> = (1..=other_count + 2).map(|id| work(id as i64)).collect();
        regroup(&mut b_members, 2);
        let mut values = vec![Some(other as f64); other_count + 2]; values[0] = Some(a as f64); values[1] = Some(b as f64);
        let mut c = cycle(&values);
        let before = decide(&a_members, &c, &GlmGroupState::default());
        b_members.reverse(); b_members.pop();
        for r in c.observations.iter_mut().skip(2) { r.reading = QuotaReading::Available(quota(100.,100.,1)); }
        let _ = decide_glm_group(&cfg(), &b_members[0].group, &b_members, &c, &GlmGroupState::default(), NOW).unwrap();
        prop_assert_eq!(decide(&a_members, &c, &GlmGroupState::default()), before);
        let mut foreign = a_members.clone(); foreign.push(b_members[0].clone());
        prop_assert_eq!(decide_glm_group(&cfg(), &work(1).group, &foreign, &c, &GlmGroupState::default(), NOW), Err(PolicyError::InvalidInput));
        let b_shared = b_members.iter().find(|w|w.source.source_id == sid(2)).unwrap().member.membership_id;
        prop_assert_ne!(b_shared,mid(2));
        c.observations[1].reading = QuotaReading::Available(quota(20.,20.,48));
        prop_assert!(decide(&a_members,&c,&GlmGroupState::default()).eligible.contains(&mid(2)));
        prop_assert!(decide_glm_group(&cfg(),&b_members[0].group,&b_members,&c,&GlmGroupState::default(),NOW).unwrap().eligible.contains(&b_shared));
        c.observations[1].reading = QuotaReading::Available(quota(100.,100.,48));
        prop_assert!(!decide(&a_members,&c,&GlmGroupState::default()).eligible.contains(&mid(2)));
        prop_assert!(!decide_glm_group(&cfg(),&b_members[0].group,&b_members,&c,&GlmGroupState::default(),NOW).unwrap().eligible.contains(&b_shared));
    }

    #[test]
    fn a4_unknown_and_stale_observations_equal_absence(n in 1usize..20, picked in 0usize..20, mode in 0u8..4, active in any::<bool>()) {
        let members: Vec<_> = (1..=n).map(|id| work(id as i64)).collect(); let i = picked % n;
        let mut c = cycle(&vec![Some(10.); n]);
        match mode { 0 => c.observations[i].config_revision += 1, 1 => c.observations[i].cycle_id += 1, 2 => c.observations[i].reading = QuotaReading::Unavailable, _ => { c.observations.remove(i); } }
        let state = GlmGroupState { active: active.then(|| mid(i as i64 + 1)), ..GlmGroupState::default() };
        let actual = decide(&members, &c, &state);
        c.observations.retain(|r| r.source_id != sid(i as i64 + 1));
        prop_assert_eq!(&actual, &decide(&members, &c, &state));
        prop_assert_eq!(actual.priorities[i].1, if active { Some(100) } else { None });
    }
}

struct FakeQuota {
    url: String,
    calls: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for FakeQuota {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl FakeQuota {
    async fn start(success: Vec<bool>, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(![3000, 3001].contains(&addr.port()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let received = calls.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                let mut buf = [0u8; 1024];
                while !data.ends_with(b"\r\n\r\n") {
                    let n = stream.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    assert!(data.len() <= 16384);
                }
                let request = String::from_utf8(data).unwrap();
                let id: usize = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .split('?')
                    .next()
                    .unwrap()
                    .trim_start_matches("/quota/")
                    .parse()
                    .unwrap();
                received.lock().unwrap().push(request);
                tokio::time::sleep(delay).await;
                let body = if success[id - 1] {
                    let kind = if id.is_multiple_of(2) {
                        "CREDIT_LIMIT"
                    } else {
                        "TOKENS_LIMIT"
                    };
                    format!(
                        r#"{{"success":true,"data":{{"level":"fake-plan","limits":[{{"type":"{kind}","percentage":41,"unit":3,"number":5,"nextResetTime":1800000000000}},{{"type":"{kind}","percentage":72,"unit":6,"number":1,"nextResetTime":1800000000000}},{{"type":"TIME_LIMIT","percentage":100,"unit":5,"number":1}}]}}}}"#
                    )
                } else {
                    format!(r#"{{"success":false,"msg":"{SECRET} {SELECTOR}"}}"#)
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}", body.len(),body);
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self {
            url: format!("http://{addr}"),
            calls,
            task,
        }
    }
    fn input(&self, id: i64) -> ProbeInput {
        ProbeInput::new(
            &work(id).source,
            material(format!("{}/quota/{id}?type=2", self.url)),
        )
        .unwrap()
    }
}
fn material(url: String) -> SourceMaterial {
    SourceMaterial {
        settings: SourceSettings::Glm {
            upstream: UpstreamProfile {
                channel_type: 8,
                base_url: "https://unused.invalid".into(),
                models: vec!["m".into()],
                model_discovery: None,
            },
            quota: GlmQuotaProfile {
                quota_url: url,
                global_headers: vec![HeaderKV {
                    key: "Bigmodel-Organization".into(),
                    value: "global-fake-org".into(),
                }],
                key_headers: vec![HeaderKV {
                    key: "Bigmodel-Project".into(),
                    value: SELECTOR.into(),
                }],
            },
        },
        credential: SecretString::new(SECRET.into()).unwrap(),
    }
}

#[tokio::test]
async fn probe_preserves_auth_windows_and_safe_errors() {
    let server = FakeQuota::start(vec![true, true, false], Duration::ZERO).await;
    let input = server.input(1);
    let debug = format!("{input:?}");
    assert!(!debug.contains(SECRET) && !debug.contains(SELECTOR) && !debug.contains(&server.url));
    let mut probe = SourceProbe::default();
    let c = probe
        .collect_cycle(vec![server.input(3), server.input(2), input])
        .await
        .unwrap();
    assert_eq!(c.cycle_id(), 1);
    assert_eq!(c.observations().len(), 3);
    for r in &c.observations[..2] {
        let QuotaReading::Available(q) = &r.reading else {
            panic!("unexpected unknown")
        };
        assert_eq!(q.five_hour.as_ref().unwrap().percentage, 41.);
        assert_eq!(q.weekly.as_ref().unwrap().percentage, 72.);
        assert!(r.observed_at_ms <= c.completed_at_ms());
    }
    assert!(matches!(
        c.observations[2].reading,
        QuotaReading::Unavailable
    ));
    let debug = format!("{c:?}");
    assert!(!debug.contains(SECRET) && !debug.contains(SELECTOR));
    let calls = server.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    for (i, r) in calls.iter().enumerate() {
        assert!(r.starts_with(&format!("GET /quota/{}?type=2 HTTP/1.1", i + 1)));
        let lower = r.to_ascii_lowercase();
        assert!(lower.contains(&format!("authorization: bearer {SECRET}")));
        assert!(lower.contains("bigmodel-organization: global-fake-org"));
        assert!(lower.contains(&format!("bigmodel-project: {SELECTOR}")));
    }
}

#[tokio::test]
async fn probe_validates_entire_batch_before_network_and_handles_empty_overflow() {
    let server = FakeQuota::start(vec![true], Duration::ZERO).await;
    let mut probe = SourceProbe::default();
    assert_eq!(
        probe
            .collect_cycle(vec![server.input(1), server.input(1)])
            .await
            .unwrap_err(),
        PolicyError::InvalidInput
    );
    let mut bad = server.input(1);
    bad.config_revision = 0;
    assert_eq!(
        probe.collect_cycle(vec![bad]).await.unwrap_err(),
        PolicyError::InvalidInput
    );
    assert_eq!(
        probe
            .collect_cycle((0..=MAX_SOURCES).map(|_| server.input(1)).collect())
            .await
            .unwrap_err(),
        PolicyError::Capacity
    );
    assert!(server.calls.lock().unwrap().is_empty());
    assert_eq!(probe.collect_cycle(vec![]).await.unwrap().cycle_id(), 1);
    probe.last_cycle = u64::MAX;
    assert_eq!(
        probe
            .collect_cycle(vec![server.input(1)])
            .await
            .unwrap_err(),
        PolicyError::SequenceExhausted
    );
    assert!(server.calls.lock().unwrap().is_empty());
}

#[test]
fn probe_input_rejects_paused_retired_and_volcano() {
    for mode in 0..4 {
        let mut s = work(1).source;
        match mode {
            0 => s.manual_paused = true,
            1 => s.retired_at = Some(1),
            2 => s.kind = SourceKind::Volcano,
            _ => s.config_revision = 0,
        }
        assert_eq!(
            ProbeInput::new(&s, material("https://unused.invalid".into())).unwrap_err(),
            PolicyError::InvalidInput
        );
    }
}

#[tokio::test]
async fn cancelled_probe_does_not_publish_partial_cycle_or_reuse_number() {
    let server = FakeQuota::start(vec![true], Duration::from_secs(30)).await;
    let mut probe = SourceProbe::default();
    assert!(tokio::time::timeout(
        Duration::from_millis(100),
        probe.collect_cycle(vec![server.input(1)])
    )
    .await
    .is_err());
    let next = probe.collect_cycle(vec![]).await.unwrap();
    assert_eq!(next.cycle_id(), 2);
    assert!(next.observations().is_empty());
}

#[tokio::test]
async fn quota_timeout_is_bounded_and_does_not_abort_the_cycle() {
    let slow = FakeQuota::start(vec![true], Duration::from_secs(30)).await;
    let fast = FakeQuota::start(vec![true, true], Duration::ZERO).await;
    let start = Instant::now();
    let c = SourceProbe::default()
        .collect_cycle(vec![slow.input(1), fast.input(2)])
        .await
        .unwrap();
    assert!(matches!(
        c.observations[0].reading,
        QuotaReading::Unavailable
    ));
    assert!(matches!(
        c.observations[1].reading,
        QuotaReading::Available(_)
    ));
    assert!(start.elapsed() < Duration::from_secs(15));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn a3_real_http_one_query_per_source_across_groups(success in prop::collection::vec(any::<bool>(),1..6), groups in 1usize..6) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let server = FakeQuota::start(success.clone(),Duration::ZERO).await;
            let mut probe = SourceProbe::default();
            let n = success.len();
            assert_eq!(probe.collect_cycle(vec![server.input(1),server.input(1)]).await.unwrap_err(),PolicyError::InvalidInput);
            assert!(server.calls.lock().unwrap().is_empty());
            let c = probe.collect_cycle((1..=n).rev().map(|id|server.input(id as i64)).collect()).await.unwrap();
            assert_eq!(c.observations.len(),n);
            for (i,r) in c.observations.iter().enumerate() { assert_eq!(r.source_id,sid(i as i64+1)); assert_eq!(matches!(r.reading,QuotaReading::Available(_)),success[i]); }
            for g in 1..=groups {
                let mut members: Vec<_> = (1..=n).map(|id|work(id as i64)).collect(); regroup(&mut members,g as i64);
                let d = decide_glm_group(&cfg(),&members[0].group,&members,&c,&GlmGroupState::default(),NOW).unwrap();
                let expected: Vec<_> = members.iter().filter(|w| success[w.source.source_id.get() as usize - 1]).map(|w|w.member.membership_id).collect();
                assert_eq!(d.eligible,expected);
            }
            assert_eq!(server.calls.lock().unwrap().len(),n);
        });
    }
}
