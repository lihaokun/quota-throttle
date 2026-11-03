//! R8 实际 HTTP/虚拟时间组合，R10 私有类型的编译期负能力门禁。
use super::tests::{Fake, Reply};
use super::*;
use proptest::prelude::*;
use std::collections::HashMap;

// 不增加依赖；若类型实现指定 trait，两个候选产生 E0283，防止误加派生。
macro_rules! assert_not_impl {
    ($ty:ty, $bound:path) => {{
        trait AmbiguousIfImpl<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
        struct Implemented;
        impl<T: ?Sized + $bound> AmbiguousIfImpl<Implemented> for T {}
        let _ = <$ty as AmbiguousIfImpl<_>>::check;
    }};
}
pub(crate) use assert_not_impl;

#[test]
fn r10_private_transport_wrappers_cannot_be_debugged_or_serialized() {
    use crate::newapi::managed_channels::*;
    use crate::reconcile::planner::*;
    macro_rules! private {
        ($($ty:ty),+ $(,)?) => {$(
            assert_not_impl!($ty, std::fmt::Debug);
            assert_not_impl!($ty, serde::Serialize);
        )+};
    }
    private!(
        ManagementRequest,
        ManagementResponse,
        ManagementTransport,
        Auth,
        AuthState,
        RawChannelDocument,
        ChannelObservation,
        ChannelPatch,
        DisabledChannelSpec<'static>,
        ChannelScan,
        ManagedChannels<'static>,
        ChannelIdentity,
        WorkSnapshot,
        StampedObservation,
        Peer,
        PlanContext<'static>,
        BackendReadiness,
        RoutingPermit
    );
    assert_not_impl!(crate::catalog::SecretString, serde::Serialize);
    assert_not_impl!(crate::catalog::SourceDraft, serde::Serialize);
    assert_not_impl!(crate::catalog::SourceMaterial, serde::Serialize);
    assert_not_impl!(crate::catalog::execution::WorkTicket, serde::Serialize);
}

fn within_budget(trace: &[(Instant, RequestClass)]) {
    for (i, (start, _)) in trace.iter().enumerate() {
        for (j, (end, _)) in trace.iter().enumerate().skip(i) {
            assert!(
                (j - i + 1) as u128 <= 2 + end.duration_since(*start).as_nanos() / 1_000_000_000,
                "HTTP admission trace exceeds burst/rate bound"
            );
        }
    }
}

async fn http_trace(events: Vec<(u64, u8, u8)>) {
    let attempts = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
    let history = attempts.clone();
    let fake = Fake::start(move |r| {
        if r.path == "/api/user/login" {
            let id = if r.body["username"] == "fake-a" {
                11
            } else {
                22
            };
            return Reply::json(json!({"success":true,"data":{"id":id}}));
        }
        let mode: u8 = r.path.rsplit('/').next().unwrap().parse().unwrap();
        let mut history = history.lock().unwrap();
        let count = history.entry(r.path.clone()).or_default();
        let attempt = *count;
        *count += 1;
        if attempt == 0 && (mode == 1 || mode == 3) {
            return Reply {
                status: 401,
                ..Reply::json(json!({"success":false}))
            };
        }
        if mode == 2 && attempt == 0 || mode == 3 && attempt == 1 {
            return Reply {
                status: 429,
                headers: "Retry-After: 2\r\n".into(),
                ..Reply::json(json!({"success":false}))
            };
        }
        Reply::json(json!({"success":true,"data":{}}))
    })
    .await;
    let budget = ManagementBudget::new();
    let mut clients = Vec::new();
    for (name, id) in [("fake-a", 11), ("fake-b", 22)] {
        let mut cfg = fake.config(false);
        cfg.root_username = name.into();
        let client = ManagementTransport::new(&cfg, budget.clone()).unwrap();
        client.auth.lock().unwrap().auth = Auth::Session(id);
        clients.push(Arc::new(client));
    }
    // 两身份交错；强制前两组包含认证及冷却，随机尾部不能缩成从未碰到故障。
    let mut all = vec![(0, 3, 0), (0, 2, 1)];
    all.extend(events);
    let mut expected_requests = 0;
    let mut expected_cooldowns = 0;
    for (wave, (gap, mode, class)) in all.into_iter().enumerate() {
        tokio::time::advance(Duration::from_millis(gap)).await;
        let mut tasks = tokio::task::JoinSet::new();
        for request_index in 0..4 {
            let identity = request_index % clients.len();
            let client = clients[identity].clone();
            let mode = (mode + (request_index / clients.len()) as u8) % 4;
            let class = [
                RequestClass::Control,
                RequestClass::Observe,
                RequestClass::Backfill,
            ][(class as usize + identity) % 3];
            let path = format!("/api/channel/trace-{wave}-{request_index}/{mode}");
            expected_cooldowns += usize::from(mode == 2 || mode == 3);
            expected_requests += match mode {
                0 => 1,
                1 | 2 => 2,
                _ => 3,
            };
            tasks.spawn(async move {
                for _ in 0..3 {
                    let request = ManagementRequest::new(
                        Method::GET,
                        &path,
                        None,
                        RequestSemantics::Read,
                        SINGLE_BODY_LIMIT,
                    )
                    .unwrap();
                    match client.send(request, class).await {
                        Ok(_) => return,
                        Err(e) if e.kind == ManagementErrorKind::RateLimited => {
                            tokio::time::sleep(e.retry_after.unwrap_or(Duration::from_secs(4)))
                                .await;
                        }
                        Err(e) => panic!("unexpected trace failure: {e:?}"),
                    }
                }
                panic!("bounded rate-limit retry did not converge");
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    }
    let calls = fake.calls.lock().unwrap();
    let logins = calls.iter().filter(|r| r.path == "/api/user/login").count();
    assert!(logins >= 2);
    assert_eq!(calls.len(), expected_requests + logins);
    let state = budget.state.lock().unwrap();
    // 独立服务器计数：若业务、登录或重放绕过 admit，计数不等，不能只看桶自己。
    assert_eq!(state.admissions.len(), calls.len());
    within_budget(&state.admissions);
    assert_eq!(state.cooldowns.len(), expected_cooldowns);
    for (next, start, delay) in &state.cooldowns {
        assert_eq!(*delay, Duration::from_secs(2)); // 由 fake 的固定响应头给出的独立期待。
        for (admitted, _) in &state.admissions[*next..] {
            assert!(
                *admitted >= *start + Duration::from_secs(2),
                "another identity bypassed shared Retry-After"
            );
        }
    }
    assert!(state.queue.is_empty());
    assert_eq!((state.active, state.background), (0, 0));
    assert_eq!(budget.requests.available_permits(), 64);
}

async fn drive_http(events: Vec<(u64, u8, u8)>) {
    let task = tokio::spawn(http_trace(events));
    // 防止 paused runtime 在 OS socket 尚未返回时自动跳到 10s timeout。
    // 驱动始终 runnable；每步让 reactor 处理 IO，再仅推进 10ms。失败有明确上界。
    for _ in 0..50_000 {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        if task.is_finished() {
            task.await.unwrap();
            return;
        }
        tokio::time::advance(Duration::from_millis(10)).await;
    }
    task.abort();
    panic!("HTTP trace did not finish within bounded virtual time");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn r8_http_identities_auth_replay_and_cooldown_share_all_time_windows(events in prop::collection::vec((0u64..2500,0u8..4,0u8..3),0..7)) {
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
        runtime.block_on(drive_http(events));
    }
    #[test]
    fn r10_internal_debug_checks_each_sensitive_field(suffix in "[A-Za-z0-9]{16,40}") {
        use crate::catalog::execution::ExecutionEffect;
        use crate::catalog::SourceKind;
        use crate::newapi::managed_channels::ChannelPatch;
        use crate::reconcile::planner::PlanStep;
        let sentinels:Vec<_>=["NAME","GROUP","URL","MODEL"].iter().map(|s|format!("{s}-{suffix}")).collect();
        let effect=ExecutionEffect::Create {name:sentinels[0].clone(),channel_type:8,group:sentinels[1].clone(),base_url:sentinels[2].clone(),models:vec![sentinels[3].clone()],priority:10,kind:SourceKind::Volcano};
        let fields=ExecutionEffect::Fields {priority:Some(10),models:Some(sentinels[3].clone()),disable_auto_ban:true};
        let step=PlanStep::Patch(ChannelPatch {priority:Some(10),models:Some(sentinels[3].clone()),disable_auto_ban:true});
        for diagnostic in [format!("{effect:?}"),format!("{fields:?}"),format!("{step:?}")] {
            for sentinel in &sentinels {prop_assert!(!diagnostic.contains(sentinel));}
        }
    }
}
