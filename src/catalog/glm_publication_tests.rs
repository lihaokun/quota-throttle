use super::*;
use crate::catalog::glm_publication::*;
use crate::newapi::managed_channels::NativeRecoveryCheck;
use crate::reconcile::planner::{BackendReadiness, RoutingPermit};
use std::collections::HashMap;

fn backend(end: i64) -> BackendReadiness {
    BackendReadiness::from_preflight("v1.0.0-rc.20", NativeRecoveryCheck::Disabled, true, 0, end)
}
fn ready(f: &mut Fixture) {
    f.store
        .conn
        .execute("UPDATE source_groups SET ready=1", [])
        .unwrap();
}
fn publish(
    f: &mut Fixture,
    b: &ProbeCycle,
    previous: &HashMap<MembershipId, RoutingPermit>,
) -> GlmPolicyCommit {
    f.store
        .commit_glm_round(&cfg(), b, backend(NOW_MS / 1000 + 100), previous, NOW_MS)
        .unwrap()
}

#[test]
fn publication_final_versions_history_preview_and_reopen() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(GlmPolicyCommit, std::fmt::Debug);
    assert_not_impl!(GlmPolicyCommit, serde::Serialize);
    assert_not_impl!(RoutingPermit, serde::de::DeserializeOwned);
    let mut f = fixture(2, 2);
    ready(&mut f);
    let b = cycle(&f.sources, &[Some(1), Some(10)]);
    let before = image(&f.store.conn);
    let preview = f.store.preview_glm_round(&cfg(), &b, NOW_MS).unwrap();
    assert_eq!(preview.permitted, 0);
    assert!(preview.preview_only);
    assert_eq!(image(&f.store.conn), before);
    let old = f.store.load_work(f.members[0][0], false).unwrap();
    let out = publish(&mut f, &b, &HashMap::new());
    assert_eq!(out.report.changed_groups, preview.changed_groups);
    assert_eq!(out.report.changed_members, preview.changed_members);
    assert_eq!(out.report.permitted, 4);
    assert_eq!(out.report.waiting, 0);
    assert_eq!(out.changed_members.len(), 2);
    assert_eq!(out.changed_groups.len(), 2);
    let permit = out.permits[&old.member.membership_id];
    assert!(!permit.valid_for(&old, NOW_MS / 1000));
    for (&id, &p) in &out.permits {
        let w = f.store.load_work(id, false).unwrap();
        assert!(p.valid_for(&w, NOW_MS / 1000));
        let mut changed = w.clone();
        changed.target.revision += 1;
        assert!(!p.valid_for(&changed, NOW_MS / 1000));
        changed = w.clone();
        changed.member.membership_id = MembershipId::new(900_000).unwrap();
        assert!(!p.valid_for(&changed, NOW_MS / 1000));
        assert_eq!(w.source.config_revision, 1);
        assert_eq!(w.group.revision, 1);
        assert!(!w.enable_consumed);
    }
    let count = f.store.conn.total_changes();
    let stable = publish(&mut f, &b, &out.permits);
    assert_eq!(stable.report.changed_members, 0);
    assert_eq!(stable.report.changed_groups, 0);
    assert_eq!(f.store.conn.total_changes(), count);
    let after = image(&f.store.conn);
    drop(f.store);
    let reopened = CatalogStore::open(&f.dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&reopened.conn), after);
}

#[test]
fn publication_wakes_only_recovered_not_ready_and_preserves_every_other_field() {
    let mut f = fixture(4, 2);
    ready(&mut f);
    let b = cycle(&f.sources, &[Some(1); 4]);
    let out = publish(&mut f, &b, &HashMap::new());
    let now = NOW_MS / 1000;
    for (g, ids) in f.members.iter().enumerate() {
        for (i, id) in ids.iter().enumerate() {
            f.store.conn.execute("UPDATE member_control SET dirty=?1,next_attempt_at=?2,last_error_code=?3 WHERE membership_id=?4",params![i!=2,now+500,if i==1 {"auth_failed"} else {"not_ready"},id.get()]).unwrap();
            assert!(g < 2);
        }
    }
    // 第一组的旧许可仍有效；第二组资格恢复，只能唤醒 dirty + NotReady。
    let previous: HashMap<_, _> = out
        .permits
        .into_iter()
        .filter(|(id, _)| f.members[0].contains(id))
        .collect();
    let before = image(&f.store.conn);
    let changes = f.store.conn.total_changes();
    let new = publish(&mut f, &b, &previous);
    assert_eq!(f.store.conn.total_changes() - changes, 2);
    for (g, ids) in f.members.iter().enumerate() {
        for (i, id) in ids.iter().enumerate() {
            let target = f.store.load_work(*id, false).unwrap().target;
            assert_eq!(
                target.next_attempt_at,
                if g == 1 && (i == 0 || i == 3) {
                    now
                } else {
                    now + 500
                }
            );
            assert_eq!(
                target.last_error_code,
                Some(if i == 1 {
                    Code::AuthFailed
                } else {
                    Code::NotReady
                })
            );
        }
    }
    // 还原仅允许改变的时间字段，整库必须与之前完全一致。
    f.store
        .conn
        .execute(
            "UPDATE member_control SET next_attempt_at=?1 WHERE membership_id IN (?2,?3)",
            params![now + 500, f.members[1][0].get(), f.members[1][3].get()],
        )
        .unwrap();
    assert_eq!(image(&f.store.conn), before);
    let changes = f.store.conn.total_changes();
    publish(&mut f, &b, &new.permits);
    assert_eq!(f.store.conn.total_changes(), changes);
    // 旧许可过期也属于资格恢复；沿用目标时不应重置启用消费等字段。
    let later = NOW_MS + 31_000;
    let rows = b
        .observations()
        .iter()
        .cloned()
        .map(|mut o| {
            o.cycle_id = 2;
            o.observed_at_ms = later;
            o
        })
        .collect();
    let next = ProbeCycle::test_input(2, later, rows);
    let out = f
        .store
        .commit_glm_round(
            &cfg(),
            &next,
            backend(later / 1000 + 100),
            &new.permits,
            later,
        )
        .unwrap();
    assert_eq!(out.report.changed_members, 0);
    assert_eq!(
        f.store
            .load_work(f.members[0][0], false)
            .unwrap()
            .target
            .next_attempt_at,
        later / 1000
    );
}

#[test]
fn publication_stale_unknown_ineligible_and_future_observations_never_get_permits() {
    let mut f = fixture(3, 2);
    ready(&mut f);
    let first = cycle(&f.sources, &[Some(1), Some(2), Some(3)]);
    let mut rows = first.observations().to_vec();
    rows[0].config_revision += 1;
    rows[1].cycle_id += 1;
    rows[2].reading = QuotaReading::Unavailable;
    let b = ProbeCycle::test_input(1, NOW_MS, rows);
    assert!(publish(&mut f, &b, &HashMap::new()).permits.is_empty());
    let b = cycle(&f.sources, &[Some(100); 3]);
    assert!(publish(&mut f, &b, &HashMap::new()).permits.is_empty());
    let before = image(&f.store.conn);
    let mut rows = first.observations().to_vec();
    rows[0].observed_at_ms = NOW_MS + 1;
    err(
        f.store.commit_glm_round(
            &cfg(),
            &ProbeCycle::test_input(2, NOW_MS, rows),
            backend(NOW_MS / 1000 + 100),
            &HashMap::new(),
            NOW_MS,
        ),
        Code::InvalidInput,
    );
    assert_eq!(image(&f.store.conn), before);
}

#[test]
fn publication_lease_boundaries_do_not_use_cycle_completion() {
    let mut f = fixture(2, 1);
    ready(&mut f);
    let at = NOW_MS + 999;
    let base = cycle(&f.sources, &[Some(1), Some(1)]);
    let mut rows = base.observations().to_vec();
    rows[0].observed_at_ms = at - 31_000;
    rows[1].observed_at_ms = at - 1000;
    let batch = ProbeCycle::test_input(1, at, rows);
    let out = f
        .store
        .commit_glm_round(
            &cfg(),
            &batch,
            backend(at / 1000 + 100),
            &HashMap::new(),
            at,
        )
        .unwrap();
    assert_eq!(out.permits.len(), 1);
    assert!(!out.permits.contains_key(&f.members[0][0]));
    let id = f.members[0][1];
    let p = out.permits[&id];
    let w = f.store.load_work(id, false).unwrap();
    assert_eq!(p.expires_at(), at / 1000 + 29);
    assert!(p.valid_for(&w, at / 1000 + 28));
    assert!(!p.valid_for(&w, at / 1000 + 29));
    let clipped = f
        .store
        .commit_glm_round(&cfg(), &batch, backend(at / 1000 + 2), &out.permits, at)
        .unwrap();
    assert_eq!(clipped.permits[&id].expires_at(), at / 1000 + 2);
    let expired = f
        .store
        .commit_glm_round(
            &cfg(),
            &batch,
            backend(at / 1000 + 100),
            &out.permits,
            at + 29_000,
        )
        .unwrap();
    assert!(expired.permits.is_empty());
    let future = BackendReadiness::from_preflight(
        "v1.0.0-rc.20",
        NativeRecoveryCheck::Disabled,
        true,
        at / 1000 + 1,
        at / 1000 + 100,
    );
    assert!(f
        .store
        .commit_glm_round(&cfg(), &batch, future, &HashMap::new(), at)
        .unwrap()
        .permits
        .is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn publication_pbt_lease_intersection_and_member_scope(
        age_ms in 0i64..40_001, backend_remaining in -2i64..45,
        offset in 0i64..1000, managed in any::<bool>(), bound in any::<bool>(),
        paused in any::<bool>(), retiring in any::<bool>(), registered in any::<bool>()
    ) {
        let mut f=fixture(1,2);let at=NOW_MS+offset;
        f.store.conn.execute("UPDATE source_groups SET ready=?1",[registered]).unwrap();
        if !managed {f.store.conn.execute("DELETE FROM operation_targets",[]).unwrap();}
        if !bound {
            for ids in &f.members {unbind(&mut f.store,ids[0]);}
        }
        f.store.conn.execute("UPDATE sources SET manual_paused=?1 WHERE source_id=?2",params![paused,f.sources[0].get()]).unwrap();
        f.store.conn.execute("UPDATE source_control SET retire_requested=?1 WHERE source_id=?2",params![retiring,f.sources[0].get()]).unwrap();
        let base=cycle(&f.sources,&[Some(1)]);let mut obs=base.observations().to_vec();obs[0].observed_at_ms=at-age_ms;
        let b=ProbeCycle::test_input(1,at,obs);
        let end=((at-age_ms)/1000+30).min(at/1000+backend_remaining).min(at/1000+30);
        let eligible=managed&&bound&&!paused&&!retiring&&registered&&end>at/1000;
        let out=f.store.commit_glm_round(&cfg(),&b,backend(at/1000+backend_remaining),&HashMap::new(),at).unwrap();
        prop_assert_eq!(out.permits.len(),if eligible {2}else{0});
        for (id,p) in out.permits {
            let w=f.store.load_work(id,false).unwrap();
            prop_assert_eq!(p.expires_at(),end);prop_assert!(p.valid_for(&w,at/1000));
            prop_assert!(!p.valid_for(&w,end));
            let mut changed=w.clone();changed.target.revision+=1;prop_assert!(!p.valid_for(&changed,at/1000));
            changed=w.clone();changed.member.membership_id=MembershipId::new(900_000).unwrap();prop_assert!(!p.valid_for(&changed,at/1000));
        }
    }
}
