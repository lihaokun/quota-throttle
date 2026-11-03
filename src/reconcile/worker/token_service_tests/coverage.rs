use super::*;

fn local_id(view: &OperationView) -> ManagedTokenId {
    ManagedTokenId::new(view.target_id.parse().unwrap()).unwrap()
}
pub(super) async fn create_confirmed(f: &TokenFixture, n: usize) -> ManagedTokenId {
    let op = create(f, n);
    token_step(f, &token_op(n)).await.unwrap();
    assert_eq!(
        f.worker.operation(&token_op(n)).unwrap().phase,
        OperationPhase::Waiting
    );
    token_step(f, &token_op(n)).await.unwrap();
    assert_eq!(
        f.worker.operation(&token_op(n)).unwrap().phase,
        OperationPhase::Succeeded
    );
    local_id(&op)
}
fn reconfigure(f: TokenFixture, issuer: bool, legacy: bool) -> TokenFixture {
    let TokenFixture {
        dir,
        fake,
        state,
        worker,
        group,
        members,
    } = f;
    drop(worker);
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let budget = ManagementBudget::new();
    let config = fake.config(true);
    let admin = Arc::new(NewApiClient::with_budget(&config, budget.clone()).unwrap());
    let sign = issuer.then(|| {
        (
            Arc::new(NewApiClient::with_budget(&config, budget).unwrap()),
            41,
        )
    });
    let worker = ReconcileWorker::new_with_token_owners(
        store,
        admin,
        policy_config(),
        TokenOwners::new(sign, legacy.then_some(1)).unwrap(),
    )
    .unwrap();
    worker
        .publish_backend(BackendReadiness::from_preflight(
            SUPPORTED_BACKEND_VERSION,
            NativeRecoveryCheck::Disabled,
            true,
            now() - 1,
            now() + 3600,
        ))
        .unwrap();
    for id in &members {
        publish(&worker, *id, false);
    }
    TokenFixture {
        dir,
        fake,
        state,
        worker,
        group,
        members,
    }
}
async fn wait_for(mut test: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(120), async {
        while !test() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
pub(super) fn native_value(f: &TokenFixture, id: ManagedTokenId) -> i64 {
    f.worker
        .token_identity(id)
        .unwrap()
        .native_token_id
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn t5c_create_replay_uses_persistent_owner_when_issuer_missing() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let original = create(&f, 0);
        let f = reconfigure(f, false, true);
        let before = f.worker.token_identities(None).unwrap();
        assert_eq!(
            f.worker
                .create_token(token_op(0), f.group, 2, "client key".into(), None)
                .unwrap(),
            original
        );
        assert_eq!(
            f.worker
                .create_token(token_op(0), f.group, 2, "different".into(), None)
                .unwrap_err()
                .code,
            ControlErrorCode::IdempotencyConflict
        );
        assert_eq!(
            f.worker
                .create_token(token_op(1), f.group, 2, "client key".into(), None)
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(
            f.worker
                .create_token(operation(0), f.group, 2, "client key".into(), None)
                .unwrap_err()
                .code,
            ControlErrorCode::IdempotencyConflict
        );
        assert_eq!(f.worker.token_identities(None).unwrap(), before);
        assert!(f.fake.calls.lock().unwrap().is_empty());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5c_create_ack_requires_lookup_and_explicit_secret_return() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let view = create(&f, 0);
        let id = local_id(&view);
        token_step(&f, &token_op(0)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Waiting
        );
        assert!(f.worker.reveal_token(id, 1).await.is_err());
        assert_eq!(puts(&f, "/api/token/101/key"), 0);
        token_step(&f, &token_op(0)).await.unwrap();
        let secret = f.worker.reveal_token(id, 1).await.unwrap();
        assert_eq!(secret.expose(), format!("sk-{ACCESS_KEY}"));
        let safe = serde_json::to_string(&f.worker.token_identity(id).unwrap()).unwrap();
        assert!(!safe.contains(ACCESS_KEY));
        assert_eq!(puts(&f, "/api/token/"), 1);
        let replay = create(&f, 0);
        assert_eq!(replay.phase, OperationPhase::Succeeded);
        assert_eq!(puts(&f, "/api/token/"), 1);
        let page = f
            .worker
            .list_tokens(TokenOwnerScope::Issuer, 1)
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert!(!page.items[0].masked_key.contains(ACCESS_KEY));
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_disable_delete_ignore_balance_group_and_source_outage() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Volcano, &[1], true, true).await;
        let id = create_confirmed(&f, 0).await;
        f.state.quota.store(0, Ordering::SeqCst);
        f.state.native.channels.lock().unwrap().get_mut(&1).unwrap()["status"] = json!(2);
        f.worker
            .publish(|core| {
                let g = core.store.group(f.group)?;
                core.store
                    .set_group_ready(f.group, g.revision, false, now())?;
                Ok(())
            })
            .unwrap();
        f.worker
            .set_token_enabled(token_op(1), id, 1, false)
            .unwrap();
        token_step(&f, &token_op(1)).await.unwrap();
        token_step(&f, &token_op(1)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(1)).unwrap().phase,
            OperationPhase::Succeeded
        );
        f.worker.delete_token(token_op(2), id, 2).unwrap();
        token_step(&f, &token_op(2)).await.unwrap();
        token_step(&f, &token_op(2)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(2)).unwrap().phase,
            OperationPhase::Succeeded
        );
        assert!(f.worker.token_identity(id).unwrap().deleted_at.is_some());
        assert_eq!(f.worker.snapshot().unwrap().sources.len(), 1);
        assert_eq!(puts(&f, "/api/token/batch"), 1);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_enable_validates_static_identity_before_put() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let id = create_confirmed(&f, 0).await;
        let native = native_value(&f, id);
        f.state.tokens.lock().unwrap().get_mut(&native).unwrap()["cross_group_retry"] = json!(true);
        f.worker
            .set_token_enabled(token_op(1), id, 1, true)
            .unwrap();
        assert_eq!(
            token_step(&f, &token_op(1)).await.unwrap_err().code,
            ControlErrorCode::BindingDrift
        );
        assert_eq!(puts(&f, "/api/token/?status_only=true"), 0);
        assert_eq!(
            f.worker.operation(&token_op(1)).unwrap().phase,
            OperationPhase::Failed
        );
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_unknown_status_matching_get_retry_and_opposite_command_stay_blocked() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let id = create_confirmed(&f, 0).await;
        f.state.corrupt_status.store(true, Ordering::SeqCst);
        f.worker
            .set_token_enabled(token_op(1), id, 1, false)
            .unwrap();
        assert!(token_step(&f, &token_op(1)).await.is_err());
        token_step(&f, &token_op(1)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(1)).unwrap().phase,
            OperationPhase::Unknown
        );
        assert_eq!(
            f.worker.retry(&token_op(1)).await.unwrap_err().code,
            ControlErrorCode::TransportUnknown
        );
        assert_eq!(
            f.worker
                .set_token_enabled(token_op(2), id, 2, true)
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(puts(&f, "/api/token/?status_only=true"), 1);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_missing_issuer_preserves_replay_and_legacy_management_without_fallback() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let id = create_confirmed(&f, 0).await;
        f.worker
            .set_token_enabled(token_op(1), id, 1, false)
            .unwrap();
        token_step(&f, &token_op(1)).await.unwrap();
        token_step(&f, &token_op(1)).await.unwrap();
        let completed = f.worker.operation(&token_op(1)).unwrap();
        let f = reconfigure(f, false, true);
        let before = f.fake.calls.lock().unwrap().len();
        assert_eq!(
            f.worker
                .set_token_enabled(token_op(1), id, 1, false)
                .unwrap(),
            completed
        );
        assert_eq!(f.worker.retry(&token_op(1)).await.unwrap(), completed);
        assert_eq!(f.fake.calls.lock().unwrap().len(), before);
        assert_eq!(
            f.worker
                .create_token(token_op(9), f.group, 2, "new".into(), None)
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        f.state
            .tokens
            .lock()
            .unwrap()
            .insert(77, token(77, 1, "auto"));
        let old = f
            .worker
            .register_token(TokenOwnerScope::LegacyAdmin, 77, "old".into())
            .await
            .unwrap();
        f.worker
            .set_token_enabled(token_op(2), old.token_id, 1, false)
            .unwrap();
        token_step(&f, &token_op(2)).await.unwrap();
        token_step(&f, &token_op(2)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(2)).unwrap().phase,
            OperationPhase::Succeeded
        );
        assert_eq!(
            f.worker
                .reveal_token(old.token_id, 2)
                .await
                .unwrap()
                .expose(),
            format!("sk-{ACCESS_KEY}")
        );
        assert_eq!(puts(&f, "/api/token/"), 1);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_missing_owner_and_read_failures_are_throttled() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[], true, true).await;
        create(&f, 0);
        let f = reconfigure(f, false, true);
        let first = f.worker.reconcile_once().await.unwrap();
        assert_eq!(first.visited, 1);
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Waiting
        );
        let count = f.fake.calls.lock().unwrap().len();
        let next = f.worker.reconcile_once().await.unwrap();
        assert_eq!(next.visited, 0);
        assert_eq!(f.fake.calls.lock().unwrap().len(), count);
        assert_eq!(puts(&f, "/api/token/"), 0);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_group_scan_checks_tail_unknown_enabled_and_preserves_noop_permits() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let prior = f.worker.core.lock().unwrap().permits[&f.members[0]].expires_at();
        let report = f.worker.check_group(f.group, 2, false).await.unwrap();
        assert!(report.ready);
        assert_eq!(report.revision, 2);
        assert_eq!(
            f.worker.core.lock().unwrap().permits[&f.members[0]].expires_at(),
            prior
        );
        println!(
            "GROUP_CHECK_SCHEMA_JSON={}",
            serde_json::to_value(report).unwrap()
        );
        println!("GROUP_CODE_ENUM_JSON={}", json!(ControlErrorCode::VALUES));
        let template = f.state.native.channels.lock().unwrap()[&1].clone();
        for id in 2..=101 {
            let mut row = template.clone();
            row["id"] = json!(id);
            row["status"] = json!(2);
            f.state.native.channels.lock().unwrap().insert(id, row);
        }
        assert!(f.worker.check_group(f.group, 2, false).await.unwrap().ready);
        f.state
            .native
            .channels
            .lock()
            .unwrap()
            .get_mut(&101)
            .unwrap()["status"] = json!(1);
        let report = f.worker.check_group(f.group, 2, false).await.unwrap();
        assert!(!report.ready);
        assert_eq!(report.error_code, Some(ControlErrorCode::BindingDrift));
        assert!(f
            .fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path.contains("p=2&")));
        assert!(f.worker.core.lock().unwrap().permits.is_empty());
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_reveal_checks_after_secret_response_and_disabled_does_not_enable() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let id = create_confirmed(&f, 0).await;
        let native = native_value(&f, id);
        f.state.tokens.lock().unwrap().get_mut(&native).unwrap()["status"] = json!(2);
        f.worker.reveal_token(id, 1).await.unwrap();
        assert_eq!(puts(&f, "/api/token/?status_only=true"), 0);
        let weak = Arc::downgrade(&f.worker);
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |r| {
            if r.path.ends_with("/key") {
                let worker = weak.upgrade().unwrap();
                worker
                    .publish_backend(BackendReadiness::from_preflight(
                        "",
                        NativeRecoveryCheck::Unknown,
                        false,
                        0,
                        0,
                    ))
                    .unwrap();
            }
        }));
        assert_eq!(
            f.worker.reveal_token(id, 1).await.err().unwrap().code,
            ControlErrorCode::RevisionConflict
        );
        assert_eq!(puts(&f, "/api/token/?status_only=true"), 0);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_prepare_then_failed_check_is_not_sent_and_not_left_applying() {
    drive_case(async {
        let f=tokens_fixture(SourceKind::Glm,&[1],true,true).await;create(&f,0);
        let conn=rusqlite::Connection::open(f.dir.path().join("catalog.sqlite3")).unwrap();
        conn.execute_batch("CREATE TRIGGER invalidate_after_prepare AFTER INSERT ON token_execution_attempts BEGIN UPDATE source_groups SET ready=0; END;").unwrap();
        assert!(token_step(&f,&token_op(0)).await.is_err());
        assert_eq!(puts(&f,"/api/token/"),0);
        assert_eq!(f.fake.calls.lock().unwrap().len(),4);
        assert_eq!(f.worker.operation(&token_op(0)).unwrap().phase,OperationPhase::Failed);
        let state:String=conn.query_row("SELECT state FROM token_execution_attempts",[],|r|r.get(0)).unwrap();assert_eq!(state,"not_sent");
    }).await;
}
#[tokio::test(start_paused = true)]
async fn t5c_drop_cancellation_and_outer_cancellation_preserve_delivery() {
    drive_case(async {
        for direct in [true, false] {
            let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
            create(&f, 0);
            f.state.delay_create.store(true, Ordering::SeqCst);
            let worker = f.worker.clone();
            let task = tokio::spawn(async move {
                if direct {
                    let _lane = worker.lane.lock().await;
                    worker
                        .token_step(&token_op(0), &AtomicBool::new(false))
                        .await
                        .map(|_| ())
                } else {
                    worker.reconcile_once().await.map(|_| ())
                }
            });
            wait_for(|| puts(&f, "/api/token/") == 1).await;
            task.abort();
            let _ = task.await;
            wait_for(|| {
                f.worker.operation(&token_op(0)).unwrap().phase != OperationPhase::Applying
            })
            .await;
            assert_eq!(
                f.worker.operation(&token_op(0)).unwrap().phase,
                if direct {
                    OperationPhase::Unknown
                } else {
                    OperationPhase::Waiting
                }
            );
            token_step(&f, &token_op(0)).await.unwrap();
            assert_eq!(
                f.worker.operation(&token_op(0)).unwrap().phase,
                OperationPhase::Succeeded
            );
            assert_eq!(puts(&f, "/api/token/"), 1);
        }
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_shutdown_settles_dispatched_write_and_leaves_next_queued() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        create(&f, 0);
        create(&f, 1);
        f.state.delay_create.store(true, Ordering::SeqCst);
        let (sender, receiver) = watch::channel(false);
        let worker = f.worker.clone();
        let task = tokio::spawn(async move { worker.run(receiver).await });
        wait_for(|| puts(&f, "/api/token/") == 1).await;
        sender.send(true).unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(puts(&f, "/api/token/"), 1);
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Waiting
        );
        assert_eq!(
            f.worker.operation(&token_op(1)).unwrap().phase,
            OperationPhase::Queued
        );
    })
    .await;
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn t5c_pb4_current_owner_group_and_live_candidate_gate_writes(role_ok in any::<bool>(),quota_ok in any::<bool>(),group_ok in any::<bool>(),live in any::<bool>()) {
        runtime().block_on(drive_case(async move {
            let f=tokens_fixture(SourceKind::Glm,&[2,if live {1}else{2}],true,true).await;create(&f,0);
            f.state.role.store(if role_ok {1}else{100},Ordering::SeqCst);
            f.state.quota.store(if quota_ok {10000}else{0},Ordering::SeqCst);
            if !group_ok {f.state.groups.lock().unwrap().clear();}
            let result=token_step(&f,&token_op(0)).await;
            let allowed=role_ok&&quota_ok&&group_ok&&live;
            assert_eq!(puts(&f,"/api/token/"),usize::from(allowed));
            assert_eq!(result.is_ok(),allowed);
        }));
    }
    #[test]
    fn t5c_pb5_creation_delivery_reopen_and_replay_never_duplicate(uncertain in any::<bool>(),restart in any::<bool>(),expired in any::<bool>()) {
        runtime().block_on(drive_case(async move {
            let f=tokens_fixture(SourceKind::Glm,&[1],true,true).await;let view=create(&f,0);
            f.state.corrupt_create.store(uncertain,Ordering::SeqCst);
            let _=token_step(&f,&token_op(0)).await;
            assert_eq!(puts(&f,"/api/token/"),1);
            assert_eq!(f.worker.operation(&token_op(0)).unwrap().phase,if uncertain {OperationPhase::Unknown}else{OperationPhase::Waiting});
            if expired {f.state.tokens.lock().unwrap().get_mut(&101).unwrap()["status"]=json!(3);}
            let f=if restart {reconfigure(f,true,true)}else{f};
            token_step(&f,&token_op(0)).await.unwrap();
            assert_eq!(f.worker.operation(&token_op(0)).unwrap().phase,OperationPhase::Succeeded);
            assert_eq!(local_id(&view),f.worker.token_identities(None).unwrap()[0].token_id);
            for _ in 0..3 {assert_eq!(create(&f,0).phase,OperationPhase::Succeeded);}
            assert_eq!(puts(&f,"/api/token/"),1);
        }));
    }
    #[test]
    fn t5c_pb6_legacy_management_preserves_group_and_unrelated_catalog(kind in 0u8..3,states in prop::collection::vec(any::<bool>(),1..4),cross in any::<bool>()) {
        runtime().block_on(drive_case(async move {
            let f=tokens_fixture(SourceKind::Glm,&[],false,true).await;
            let group=match kind {0=>"",1=>"auto",_=>"unmanaged"};let mut raw=token(77,1,group);raw["cross_group_retry"]=json!(cross);f.state.tokens.lock().unwrap().insert(77,raw);
            let before=serde_json::to_value(f.worker.snapshot().unwrap()).unwrap();
            let identity=f.worker.register_token(TokenOwnerScope::LegacyAdmin,77,"legacy".into()).await.unwrap();
            let mut revision=1;
            for (index,enabled) in states.iter().enumerate() {
                f.worker.set_token_enabled(token_op(index),identity.token_id,revision,*enabled).unwrap();
                token_step(&f,&token_op(index)).await.unwrap();token_step(&f,&token_op(index)).await.unwrap();revision+=1;
                assert_eq!(f.worker.operation(&token_op(index)).unwrap().phase,OperationPhase::Succeeded);
                let token=f.state.tokens.lock().unwrap()[&77].clone();assert_eq!(token["group"],group);assert_eq!(token["cross_group_retry"],cross);
            }
            assert_eq!(serde_json::to_value(f.worker.snapshot().unwrap()).unwrap(),before);
            assert_eq!(f.worker.token_identity(identity.token_id).unwrap().revision,revision);
            assert_eq!(puts(&f,"/api/token/"),0);
        }));
    }
}
