use super::*;
use crate::catalog::ledger::ScanProgress;
use proptest::prelude::*;

fn state(from: i64, now: i64) -> LedgerState {
    assert!(from >= 0 && from < now);
    LedgerState {
        revision: 2,
        initial_requested_from: Some(from),
        scans: [ScanLane::Recent, ScanLane::History].map(|lane| ScanProgress {
            lane,
            generation: 0,
            window: None,
            next_page: 1,
            phase: ScanPhase::Idle,
            last_observed_at: None,
            fault: None,
        }),
    }
}
#[test]
fn plan_preserves_fixed_windows_and_global_faults() {
    let mut s = state(100, 1000);
    assert_eq!(
        plan_scan(&s, ScanLane::Recent, None, None, 1000).unwrap(),
        ScanAction::Begin(ScanWindow {
            from: 700,
            to: 1000
        })
    );
    assert_eq!(
        plan_scan(&s, ScanLane::History, None, None, 1000).unwrap(),
        ScanAction::Begin(ScanWindow {
            from: 100,
            to: 1000
        })
    );
    let p = &mut s.scans[0];
    p.phase = ScanPhase::Scanning;
    p.generation = 2;
    p.window = Some(ScanWindow { from: 100, to: 600 });
    p.next_page = 9;
    p.last_observed_at = Some(700);
    assert_eq!(
        plan_scan(&s, ScanLane::Recent, Some(2), None, 1000).unwrap(),
        ScanAction::Continue
    );
    assert_eq!(
        plan_scan(&s, ScanLane::Recent, Some(1), None, 1000).unwrap(),
        ScanAction::RestartSameWindow
    );
    assert_eq!(
        plan_scan(&s, ScanLane::History, None, None, 699).err(),
        Some(CollectionError::Clock)
    );
    assert_eq!(
        plan_scan(&s, ScanLane::History, None, Some(1001), 1000).err(),
        Some(CollectionError::Clock)
    );
    for (fault, expected) in [
        (ScanFault::Transport, ScanAction::RetrySameWindow),
        (ScanFault::Clock, ScanAction::RetrySameWindow),
        (
            ScanFault::InvalidPage,
            ScanAction::Blocked(ScanFault::InvalidPage),
        ),
        (
            ScanFault::InvalidFact,
            ScanAction::Blocked(ScanFault::InvalidFact),
        ),
        (
            ScanFault::ResourceLimit,
            ScanAction::Blocked(ScanFault::ResourceLimit),
        ),
        (
            ScanFault::HistoryUnavailable,
            ScanAction::Blocked(ScanFault::HistoryUnavailable),
        ),
    ] {
        s.scans[0].phase = ScanPhase::Stalled;
        s.scans[0].fault = Some(fault);
        assert_eq!(
            plan_scan(&s, ScanLane::Recent, None, None, 1000).unwrap(),
            expected
        );
    }
    for fault in [ScanFault::ConflictingIdentity, ScanFault::BindingChanged] {
        s.scans[0].fault = Some(fault);
        assert_eq!(
            plan_scan(&s, ScanLane::History, None, None, 1000).unwrap(),
            ScanAction::Blocked(fault)
        );
    }
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb2_plan_time_restart_and_persistent_history(from in 0i64..1_000_000, elapsed in 2i64..10_000_000, phase in 0u8..4, generation in 1i64..10000, accepted in any::<bool>(), advance in 0i64..10000, recent in any::<bool>(), fault in 0usize..8, observed_delta in 0i64..200, high in any::<bool>()) {
        let from=if high{JSON_SAFE-20_000_000+from}else{from};
        let now=from+elapsed;let mut s=state(from,now);
        let index=usize::from(!recent);let lane=if recent{ScanLane::Recent}else{ScanLane::History};
        let p=&mut s.scans[index];p.generation=generation;p.window=Some(ScanWindow{from,to:now});p.last_observed_at=Some(now+observed_delta);
        p.phase=match phase{0=>ScanPhase::Idle,1=>ScanPhase::Scanning,2=>ScanPhase::Scanned,_=>ScanPhase::Stalled};
        if phase==0 {p.generation=0;p.window=None;p.last_observed_at=None;}
        let reason=[ScanFault::Transport,ScanFault::Clock,ScanFault::InvalidPage,ScanFault::InvalidFact,ScanFault::ResourceLimit,ScanFault::HistoryUnavailable,ScanFault::BindingChanged,ScanFault::ConflictingIdentity][fault];
        if phase==3 {p.fault=Some(reason);}
        let before=s.clone();let actual=plan_scan(&s,lane,accepted.then_some(generation),Some(now),now+advance);
        if phase!=0 && advance<observed_delta {prop_assert_eq!(actual.err(),Some(CollectionError::Clock));}
        else {
            let expected=match phase{1=>if accepted{ScanAction::Continue}else{ScanAction::RestartSameWindow},3 if fault<2=>ScanAction::RetrySameWindow,3=>ScanAction::Blocked(reason),2 if advance==0=>ScanAction::Skip,_=>ScanAction::Begin(ScanWindow{from:if recent && elapsed+advance>300{now+advance-300}else{from},to:now+advance})};
            prop_assert_eq!(actual.unwrap(),expected);
        }
        prop_assert_eq!(&s,&before);
        if phase!=0 {prop_assert_eq!(plan_scan(&s,ScanLane::History,None,None,now-1).err(),Some(CollectionError::Clock));}
        let at=now+advance.max(observed_delta);
        if phase==3 {s.scans[index].fault=Some(ScanFault::Transport);}
        s.scans[1-index]=ScanProgress{lane:if recent{ScanLane::History}else{ScanLane::Recent},generation:5,window:Some(ScanWindow{from,to:now}),next_page:7,phase:ScanPhase::Stalled,last_observed_at:Some(at),fault:Some(ScanFault::BindingChanged)};
        prop_assert_eq!(plan_scan(&s,lane,None,None,at).unwrap(),ScanAction::Blocked(ScanFault::BindingChanged));
        s.scans[1-index].last_observed_at=Some(at+1);
        prop_assert_eq!(plan_scan(&s,lane,None,None,at).err(),Some(CollectionError::Clock));
    }
}
