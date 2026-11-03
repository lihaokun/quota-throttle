use super::*;
use crate::ledger::analytics::AnalyticsError;
use std::future::Future;
use std::task::{Context, Poll, Waker};

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("held automatic gate must reject before network or commit"),
    }
}

#[tokio::test(start_paused = true)]
async fn r7_actual_group_check_excludes_background_publication_without_changing_epoch() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Volcano, &[2], true, true).await;
        let worker = f.worker.clone();
        let seen = Arc::new(AtomicUsize::new(0));
        let observed = seen.clone();
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |request| {
            if request.path != "/api/user/self" {
                return;
            }
            assert!(worker.automatic_updates.try_read().is_err());
            let epoch = worker.publication.load(Ordering::SeqCst);
            assert_eq!(
                ready(worker.refresh_volcano_policy()).unwrap_err().code,
                ControlErrorCode::QueueFull
            );
            assert_eq!(
                ready(worker.refresh_glm_policy()).unwrap_err().code,
                ControlErrorCode::QueueFull
            );
            assert_eq!(
                ready(worker.repair_attribution_once()).unwrap_err(),
                AnalyticsError::Busy
            );
            assert_eq!(worker.publication.load(Ordering::SeqCst), epoch);
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(f.worker.check_group(f.group, 2, false).await.unwrap().ready);
        assert!(seen.load(Ordering::SeqCst) > 0);
        assert!(f.worker.automatic_updates.try_write().is_ok());
        f.worker.refresh_glm_policy().await.unwrap();
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn r7_manual_pause_still_revokes_group_check_and_releases_background_gate() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Volcano, &[2], true, true).await;
        let worker = f.worker.clone();
        let work = worker.work(f.members[0]).unwrap();
        let once = Arc::new(AtomicBool::new(false));
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |request| {
            if request.path == "/api/user/self" && !once.swap(true, Ordering::SeqCst) {
                assert!(worker.automatic_updates.try_read().is_err());
                worker
                    .submit(
                        operation(99),
                        ControlCommand::SetSourcePaused {
                            source_id: work.source.source_id,
                            expected_revision: work.source.config_revision,
                            paused: true,
                        },
                    )
                    .unwrap();
            }
        }));
        assert_eq!(
            f.worker
                .check_group(f.group, 2, false)
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::RevisionConflict
        );
        assert!(f.worker.work(f.members[0]).unwrap().source.manual_paused);
        assert!(f.worker.automatic_updates.try_write().is_ok());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn r7_registration_short_lease_rejects_before_http_but_verification_keeps_original_rule() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Volcano, &[2], true, true).await;
        f.worker
            .publish_backend(BackendReadiness::from_preflight(
                SUPPORTED_BACKEND_VERSION,
                NativeRecoveryCheck::Disabled,
                true,
                now(),
                now() + 20,
            ))
            .unwrap();
        assert_eq!(
            f.worker
                .check_group(f.group, 2, true)
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert!(f.fake.calls.lock().unwrap().is_empty());
        assert!(f.worker.check_group(f.group, 2, false).await.unwrap().ready);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn r7_cancelled_group_check_releases_automatic_updates() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Volcano, &[2], true, true).await;
        let (worker, group) = (f.worker.clone(), f.group);
        let task = tokio::spawn(async move { worker.check_group(group, 2, false).await });
        until(|| f.worker.automatic_updates.try_read().is_err()).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(f.worker.automatic_updates.try_write().is_ok());
        f.worker.refresh_glm_policy().await.unwrap();
    })
    .await;
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn r7_pbt_automatic_gate_never_blocks_manual_invalidation(events in prop::collection::vec(0u8..5, 0..16)) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let f = tokens_fixture(SourceKind::Volcano, &[2], true, true).await;
            let mut held = Some(f.worker.automatic_updates.try_write().unwrap());
            for (index, event) in events.into_iter().enumerate() {
                let before = f.worker.publication.load(Ordering::SeqCst);
                match event {
                    0 => assert_eq!(f.worker.refresh_volcano_policy().await.unwrap_err().code, ControlErrorCode::QueueFull),
                    1 => assert_eq!(f.worker.refresh_glm_policy().await.unwrap_err().code, ControlErrorCode::QueueFull),
                    2 => assert_eq!(f.worker.repair_attribution_once().await.unwrap_err(), AnalyticsError::Busy),
                    3 => {
                        let work = f.worker.work(f.members[0]).unwrap();
                        f.worker.submit(operation(100 + index), ControlCommand::SetSourcePaused {
                            source_id: work.source.source_id, expected_revision: work.source.config_revision, paused: index % 2 == 0,
                        }).unwrap();
                    }
                    _ => {
                        drop(held.take());
                        f.worker.refresh_glm_policy().await.unwrap();
                        held = Some(f.worker.automatic_updates.try_write().unwrap());
                    }
                }
                let after = f.worker.publication.load(Ordering::SeqCst);
                if event < 3 { assert_eq!(after, before); } else { assert!(after > before); }
            }
            drop(held);
            assert!(f.worker.automatic_updates.try_read().is_ok());
        });
    }
}

#[tokio::test(start_paused = true)]
async fn r7_token_step_and_reveal_exclude_automatic_publication_and_release_guard() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let worker = f.worker.clone();
        let seen = Arc::new(AtomicUsize::new(0));
        let observed = seen.clone();
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |_| {
            assert!(worker.automatic_updates.try_read().is_err());
            assert!(worker.lane.try_lock().is_err());
            let before = worker.publication.load(Ordering::SeqCst);
            assert_eq!(
                ready(worker.refresh_volcano_policy()).unwrap_err().code,
                ControlErrorCode::QueueFull
            );
            assert_eq!(
                ready(worker.repair_attribution_once()).unwrap_err(),
                AnalyticsError::Busy
            );
            assert_eq!(worker.publication.load(Ordering::SeqCst), before);
            observed.fetch_add(1, Ordering::SeqCst);
        }));
        let id = coverage::create_confirmed(&f, 0).await;
        assert!(seen.load(Ordering::SeqCst) > 0);
        assert!(f.worker.automatic_updates.try_write().is_ok());
        seen.store(0, Ordering::SeqCst);
        assert_eq!(
            f.worker.reveal_token(id, 1).await.unwrap().expose(),
            format!("sk-{ACCESS_KEY}")
        );
        assert!(seen.load(Ordering::SeqCst) >= 4);
        assert!(f.worker.automatic_updates.try_write().is_ok());
        assert!(f.worker.lane.try_lock().is_ok());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn r7_busy_token_step_leaves_queued_untouched_and_manual_pause_still_revokes() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let original = create(&f, 0);
        let before = f.worker.publication.load(Ordering::SeqCst);
        let held = f.worker.automatic_updates.try_write().unwrap();
        assert_eq!(
            token_step(&f, &token_op(0)).await.unwrap_err().code,
            ControlErrorCode::QueueFull
        );
        assert_eq!(f.worker.operation(&token_op(0)).unwrap(), original);
        assert_eq!(f.worker.publication.load(Ordering::SeqCst), before);
        assert!(f.fake.calls.lock().unwrap().is_empty());
        drop(held);
        let worker = f.worker.clone();
        let work = worker.work(f.members[0]).unwrap();
        let once = Arc::new(AtomicBool::new(false));
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |request| {
            if request.path == "/api/user/self" && !once.swap(true, Ordering::SeqCst) {
                worker
                    .submit(
                        operation(199),
                        ControlCommand::SetSourcePaused {
                            source_id: work.source.source_id,
                            expected_revision: work.source.config_revision,
                            paused: true,
                        },
                    )
                    .unwrap();
            }
        }));
        assert_eq!(
            token_step(&f, &token_op(0)).await.unwrap_err().code,
            ControlErrorCode::RevisionConflict
        );
        assert_eq!(puts(&f, "/api/token/"), 0);
        assert!(f.worker.work(f.members[0]).unwrap().source.manual_paused);
        assert!(f.worker.automatic_updates.try_write().is_ok());
    })
    .await;
}
