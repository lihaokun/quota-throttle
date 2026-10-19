use super::*;
use crate::catalog::{GroupDraft, SourceKind};
use crate::panel_http::admin::tests::support::{drive, fixture, until, CoreHold};
use proptest::prelude::*;
use serde_json::Value;
use std::time::Duration;

fn group(n: usize) -> AdminRequest {
    AdminRequest::CreateGroup(GroupDraft {
        name: format!("group-{n}"),
        newapi_group: format!("h2-{n}"),
        kind: SourceKind::Volcano,
    })
}
fn begin(client: &Client, n: usize) -> tokio::task::JoinHandle<Response> {
    let client = client.clone();
    tokio::spawn(async move {
        let entered = Instant::now();
        client
            .admit(group(n), entered, entered + Duration::from_secs(45))
            .await
    })
}
fn code(response: &Response) -> Value {
    serde_json::from_slice::<Value>(&response.body).unwrap()["error"].clone()
}

#[tokio::test]
async fn h2_completed_jobs_keep_all_four_slots_until_supervisor_consumes_result() {
    let f = fixture(false);
    let (client, mut supervisor) = Supervisor::new(f.worker.clone());
    let mut waiting = Vec::new();
    for n in 0..4 {
        waiting.push(begin(&client, n));
        let pending = supervisor.receiver.recv().await.unwrap();
        supervisor.start(pending);
    }
    let mut outcomes = Vec::new();
    for _ in 0..4 {
        outcomes.push(supervisor.jobs.join_next().await.unwrap());
    }
    assert_eq!(client.slots.available_permits(), 0);
    assert_eq!(code(&begin(&client, 99).await.unwrap()), "queue_full");
    assert_eq!(f.worker.snapshot().unwrap().groups.len(), 4);
    for outcome in outcomes {
        assert!(!Supervisor::finish(Some(outcome)));
    }
    for task in waiting {
        assert_eq!(task.await.unwrap().status, "201 Created");
    }
    assert_eq!(client.slots.available_permits(), 4);
}

#[tokio::test(start_paused = true)]
async fn h2_timeouts_do_not_release_actual_jobs_or_forge_rollback() {
    drive(async {
        let f = fixture(false);
        let hold = CoreHold::new(f.worker.clone());
        let (client, mut supervisor) = Supervisor::new(f.worker.clone());
        let mut waiting = Vec::new();
        for n in 0..4 {
            waiting.push(begin(&client, n));
            let pending = supervisor.receiver.recv().await.unwrap();
            supervisor.start(pending);
        }
        until(|| f.worker.admin_test_commands_available() == 60).await;
        tokio::time::advance(Duration::from_secs(11)).await;
        for task in waiting {
            assert_eq!(code(&task.await.unwrap()), "result_unconfirmed");
        }
        assert_eq!(client.slots.available_permits(), 0);
        assert_eq!(code(&begin(&client, 99).await.unwrap()), "queue_full");
        supervisor.close();
        assert_eq!(supervisor.jobs.len(), 4);
        drop(hold);
        while !supervisor.jobs.is_empty() {
            assert!(!Supervisor::finish(supervisor.jobs.join_next().await));
        }
        assert_eq!(client.slots.available_permits(), 4);
        // 已跨过最后准入检查；客户端超时不回滚真实目录提交。
        assert_eq!(f.worker.snapshot().unwrap().groups.len(), 4);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn h2_expired_or_cancelled_handoff_never_calls_worker() {
    drive(async {
        for cancelled in [false, true] {
            let f = fixture(false);
            let (client, mut supervisor) = Supervisor::new(f.worker.clone());
            let waiter = begin(&client, 1);
            let pending = supervisor.receiver.recv().await.unwrap();
            if cancelled {
                waiter.abort();
                let _ = waiter.await;
            } else {
                tokio::time::advance(Duration::from_secs(11)).await;
                assert_eq!(code(&waiter.await.unwrap()), "result_unconfirmed");
            }
            supervisor.start(pending);
            assert!(supervisor.jobs.is_empty());
            assert_eq!(client.slots.available_permits(), 4);
            assert!(f.worker.snapshot().unwrap().groups.is_empty());
        }
    })
    .await;
}

#[tokio::test]
async fn h2_closing_discards_handoff_and_dry_run_rejects_before_admission() {
    for dry_run in [false, true] {
        let f = fixture(dry_run);
        let (client, mut supervisor) = Supervisor::new(f.worker.clone());
        if dry_run {
            assert_eq!(code(&begin(&client, 1).await.unwrap()), "dry_run");
            assert!(supervisor.receiver.try_recv().is_err());
        } else {
            let waiter = begin(&client, 1);
            until(|| supervisor.receiver.len() == 1).await;
            supervisor.close();
            assert_eq!(code(&waiter.await.unwrap()), "internal");
            assert_eq!(code(&begin(&client, 2).await.unwrap()), "queue_full");
        }
        assert_eq!(client.slots.available_permits(), 4);
        assert!(f.worker.snapshot().unwrap().groups.is_empty());
    }
}

#[tokio::test]
async fn h2_usage_idle_barrier_waits_for_real_query_slot_without_closing_it() {
    let f = fixture(false);
    let held = f.worker.admin_test_query_slot().await;
    let worker = f.worker.clone();
    let barrier = tokio::spawn(async move { worker.wait_usage_idle().await });
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(!barrier.is_finished());
    drop(held);
    barrier.await.unwrap().unwrap();
    drop(f.worker.admin_test_query_slot().await);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb6_admin_handoff_completion_cancellation_traces_conserve_actual_slots(actions in prop::collection::vec(0u8..5, 1..40)) {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let f = fixture(false);
            let (client, mut supervisor) = Supervisor::new(f.worker.clone());
            let mut waiting = Vec::new();
            for (n, action) in actions.into_iter().enumerate() {
                match action {
                    0 | 1 => {
                        let available = client.slots.available_permits();
                        let waiter = begin(&client, n);
                        until(|| waiter.is_finished() || client.slots.available_permits() < available).await;
                        if available == 0 { assert_eq!(code(&waiter.await.unwrap()), "queue_full"); }
                        else { waiting.push(waiter); }
                    }
                    2 => {
                        if let Some(waiter) = waiting.pop() { waiter.abort(); let _ = waiter.await; }
                    }
                    3 => {
                        if supervisor.can_start() {
                            if let Ok(pending) = supervisor.receiver.try_recv() { supervisor.start(pending); }
                        }
                    }
                    _ => {
                        if !supervisor.jobs.is_empty() { assert!(!Supervisor::finish(supervisor.jobs.join_next().await)); }
                    }
                }
                assert_eq!(client.slots.available_permits() + supervisor.receiver.len() + supervisor.jobs.len(), MAX_BUSINESS);
            }
            supervisor.close();
            for waiter in waiting { waiter.abort(); let _ = waiter.await; }
            while !supervisor.jobs.is_empty() { assert!(!Supervisor::finish(supervisor.jobs.join_next().await)); }
            assert_eq!(client.slots.available_permits(), MAX_BUSINESS);
        });
    }
}

#[tokio::test(start_paused = true)]
async fn h2_ready_reply_at_exact_deadline_is_still_unconfirmed() {
    drive(async {
        let f = fixture(false);
        let (client, mut supervisor) = Supervisor::new(f.worker.clone());
        let entered = Instant::now();
        let mut waiter =
            Box::pin(client.admit(group(1), entered, entered + Duration::from_secs(45)));
        // 只poll一次到Pending，之后时钟推进不能替我们poll等待者、先消费timeout。
        let pending_once = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(waiter.as_mut(), cx).is_pending())
        })
        .await;
        assert!(pending_once);
        let pending = supervisor.receiver.try_recv().unwrap();
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(pending
            .reply
            .send(Response::error(HttpErrorCode::NotFound))
            .is_ok());
        assert_eq!(code(&waiter.await), "result_unconfirmed");
        drop(pending.permit);
        assert!(f.worker.snapshot().unwrap().groups.is_empty());
    })
    .await;
}

#[test]
fn h2_blocking_pool_queued_work_rechecks_deadline_close_and_cancel() {
    struct Release(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }
    // 单线程blocking池仅故障注入；生产保持Tokio标准池，不推导线程充分性。
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::pause();
        drive(async {
            for mode in 0..3 {
                let f = fixture(false);
                let (client, mut supervisor) = Supervisor::new(f.worker.clone());
                let (tx, rx) = std::sync::mpsc::channel();
                let release = Release(Some(tx));
                let (ready, entered) = oneshot::channel();
                let blocker = tokio::task::spawn_blocking(move || {
                    let _ = ready.send(());
                    let _ = rx.recv();
                });
                entered.await.unwrap();
                let mut waiter = Some(begin(&client, 1));
                let pending = supervisor.receiver.recv().await.unwrap();
                supervisor.start(pending);
                assert_eq!(client.slots.available_permits(), 3);
                if mode == 0 {
                    tokio::time::advance(Duration::from_secs(11)).await;
                }
                if mode == 1 {
                    supervisor.close();
                }
                if mode == 2 {
                    let cancelled = waiter.take().unwrap();
                    cancelled.abort();
                    let _ = cancelled.await;
                }
                drop(release);
                blocker.await.unwrap();
                assert!(!Supervisor::finish(supervisor.jobs.join_next().await));
                if let Some(waiter) = waiter {
                    assert_eq!(
                        code(&waiter.await.unwrap()),
                        if mode == 0 {
                            "result_unconfirmed"
                        } else {
                            "queue_full"
                        }
                    );
                }
                assert_eq!(client.slots.available_permits(), 4);
                assert!(f.worker.snapshot().unwrap().groups.is_empty());
            }
        })
        .await;
    });
}

#[tokio::test]
async fn h2_business_panic_closes_reply_and_other_jobs_still_drain() {
    let f = fixture(false);
    let (client, mut supervisor) = Supervisor::new(f.worker.clone());
    let panicked = begin(&client, 0);
    let pending = supervisor.receiver.recv().await.unwrap();
    supervisor.jobs.spawn_blocking(move || -> JobOutcome {
        let _pending = pending;
        panic!("isolated business panic injection");
    });
    let next = begin(&client, 1);
    let pending = supervisor.receiver.recv().await.unwrap();
    supervisor.start(pending);
    let mut failures = 0;
    while !supervisor.jobs.is_empty() {
        failures += usize::from(Supervisor::finish(supervisor.jobs.join_next().await));
    }
    assert_eq!(failures, 1);
    assert_eq!(code(&panicked.await.unwrap()), "internal");
    assert_eq!(next.await.unwrap().status, "201 Created");
    assert_eq!(client.slots.available_permits(), 4);
    assert_eq!(f.worker.snapshot().unwrap().groups.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn h2_shutdown_cannot_detach_business_even_when_publisher_never_blocks() {
    drive(async {
        use crate::panel_http::{PanelServer, tests::empty, publication::wall_ms};
        use crate::panel_http::admin::tests::support::Running;
        let f = fixture(false);
        let hold = CoreHold::new(f.worker.clone());
        let panel = PanelServer::bind("127.0.0.1:0".parse().unwrap(),None).await.unwrap();
        let addr=panel.listener.local_addr().unwrap();
        let authority=panel.origin.authority.clone();
        let (stop,shutdown)=tokio::sync::watch::channel(false);
        // publisher不读Core：此用例的等待只能由真正业务句柄造成，不能被投影锁掩盖。
        let task=tokio::spawn(panel.serve_with_worker(Arc::new(||Ok(empty(wall_ms().unwrap()))),f.worker.clone(),shutdown));
        let server=Arc::new(Running {addr,authority,stop,task});
        let caller=server.clone();
        let request=tokio::spawn(async move { caller.call("POST","/groups",Some(&serde_json::json!({"schema_version":1,"name":"g","newapi_group":"g","type":"volcano"}))).await });
        until(||f.worker.admin_test_commands_available()==63).await;
        // HTTP等待超时已结束；之后只剩业务closure持有实际任务。
        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(request.await.unwrap().0,504);
        server.stop.send(true).unwrap();
        for _ in 0..1000 {tokio::task::yield_now().await;}
        assert!(!server.task.is_finished());
        drop(hold);
        until(||server.task.is_finished()).await;
        Arc::try_unwrap(server).ok().unwrap().task.await.unwrap().unwrap();
        assert_eq!(f.worker.snapshot().unwrap().groups.len(),1);
    }).await;
}

#[tokio::test(start_paused = true)]
async fn h2_real_serve_shutdown_waits_for_late_commit_and_publication() {
    drive(async {
        use crate::panel_http::admin::tests::support::Running;
        let f = fixture(false);
        let hold = CoreHold::new(f.worker.clone());
        let server = Arc::new(Running::start(f.worker.clone()).await);
        let mut requests = Vec::new();
        for n in 0..4 {
            let server = server.clone();
            requests.push(tokio::spawn(async move { server.call("POST","/groups",Some(&serde_json::json!({"schema_version":1,"name":format!("g{n}"),"newapi_group":format!("g{n}"),"type":"volcano"}))).await }));
        }
        until(|| f.worker.admin_test_commands_available()==60).await;
        tokio::time::advance(Duration::from_secs(11)).await;
        for request in requests { let (status,body) = request.await.unwrap(); assert_eq!(status,504); assert_eq!(body["error"],"result_unconfirmed"); }
        server.stop.send(true).unwrap();
        for _ in 0..100 { tokio::task::yield_now().await; }
        assert!(!server.task.is_finished());
        drop(hold);
        until(|| server.task.is_finished()).await;
        let server = Arc::try_unwrap(server).ok().unwrap();
        server.task.await.unwrap().unwrap();
        assert_eq!(f.worker.snapshot().unwrap().groups.len(),4);
        assert!(tokio::net::TcpStream::connect(server.addr).await.is_err());
    }).await;
}
