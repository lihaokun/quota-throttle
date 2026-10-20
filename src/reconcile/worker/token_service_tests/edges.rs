use super::coverage::{create_confirmed, native_value};
use super::*;

#[tokio::test(start_paused = true)]
async fn t5c_volcano_positive_gate_requires_auto_ban_off_but_glm_keeps_it() {
    drive_case(async {
        for kind in [SourceKind::Glm, SourceKind::Volcano] {
            let f = tokens_fixture(kind, &[1], true, true).await;
            f.state.native.channels.lock().unwrap().get_mut(&1).unwrap()["auto_ban"] = json!(1);
            create(&f, 0);
            let result = token_step(&f, &token_op(0)).await;
            if kind == SourceKind::Glm {
                result.unwrap();
                assert_eq!(puts(&f, "/api/token/"), 1);
            } else {
                assert_eq!(result.unwrap_err().code, ControlErrorCode::BindingDrift);
                assert_eq!(puts(&f, "/api/token/"), 0);
            }
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5c_owners_are_private_validate_scope_and_allow_legacy_only() {
    drive_case(async {
        use crate::newapi::transport::assurance_tests::assert_not_impl;
        assert_not_impl!(TokenOwners, std::fmt::Debug);
        assert_not_impl!(TokenOwners, serde::Serialize);
        let f = tokens_fixture(SourceKind::Glm, &[], false, true).await;
        assert!(TokenOwners::new(None, None).is_err());
        assert!(TokenOwners::new(None, Some(0)).is_err());
        let client = Arc::new(NewApiClient::new(&f.fake.config(true)).unwrap());
        assert!(TokenOwners::new(Some((client.clone(), 0)), None).is_err());
        assert!(TokenOwners::new(Some((client.clone(), 1)), Some(1)).is_err());
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
        let owners = TokenOwners::new(Some((client, 41)), Some(1)).unwrap();
        assert!(ReconcileWorker::new_with_token_owners(
            store,
            f.worker.client.clone(),
            policy_config(),
            owners
        )
        .is_err());
        assert!(f.fake.calls.lock().unwrap().is_empty());
        assert_eq!(
            f.worker
                .create_token(token_op(0), f.group, 2, "new".into(), None)
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_dry_run_and_backpressure_do_not_publish_or_send() {
    drive_case(async {
        let mut f = tokens_fixture(SourceKind::Glm, &[], true, true).await;
        {
            let held = f.worker.commands.try_acquire_many(64).unwrap();
            assert_eq!(
                f.worker
                    .create_token(token_op(0), f.group, 2, "new".into(), None)
                    .unwrap_err()
                    .code,
                ControlErrorCode::QueueFull
            );
            drop(held);
            let held = f.worker.lane.try_lock().unwrap();
            assert_eq!(
                f.worker
                    .check_group(f.group, 2, false)
                    .await
                    .unwrap_err()
                    .code,
                ControlErrorCode::QueueFull
            );
            drop(held);
        }
        let mut cfg = (*policy_config()).clone();
        cfg.dry_run = true;
        Arc::get_mut(&mut f.worker).unwrap().policy_config = Arc::new(cfg);
        assert_eq!(
            f.worker
                .check_group(f.group, 2, false)
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(
            f.worker
                .create_token(token_op(0), f.group, 2, "new".into(), None)
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert_eq!(
            f.worker
                .register_token(TokenOwnerScope::LegacyAdmin, 77, "old".into())
                .await
                .unwrap_err()
                .code,
            ControlErrorCode::NotReady
        );
        assert!(f.worker.token_identities(None).unwrap().is_empty());
        assert!(f.fake.calls.lock().unwrap().is_empty());
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_final_gate_rejects_post_after_last_read_changes_publication() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        create(&f, 0);
        let groups = Arc::new(AtomicUsize::new(0));
        let seen = groups.clone();
        let weak = Arc::downgrade(&f.worker);
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |r| {
            if r.path == "/api/user/self/groups" && seen.fetch_add(1, Ordering::SeqCst) == 1 {
                weak.upgrade()
                    .unwrap()
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
        assert!(token_step(&f, &token_op(0)).await.is_err());
        assert_eq!(groups.load(Ordering::SeqCst), 2);
        assert_eq!(puts(&f, "/api/token/"), 0);
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Failed
        );
        let work = f
            .worker
            .core
            .lock()
            .unwrap()
            .store
            .token_work(&token_op(0), now())
            .unwrap();
        assert_eq!(
            work.create_state(),
            crate::catalog::control::CreateState::Never
        );
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_401_recovery_rechecks_worker_gate_before_create_replay() {
    drive_case(async {
        let f = tokens_fixture_with_auth(SourceKind::Glm, &[1], true, true, true).await;
        create(&f, 0);
        f.state.unauthorized_create.store(true, Ordering::SeqCst);
        let weak = Arc::downgrade(&f.worker);
        let state = Arc::downgrade(&f.state);
        *f.state.hook.lock().unwrap() = Some(Arc::new(move |r| {
            if r.path == "/api/user/login"
                && state
                    .upgrade()
                    .unwrap()
                    .saw_unauthorized
                    .load(Ordering::SeqCst)
            {
                weak.upgrade()
                    .unwrap()
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
        assert!(token_step(&f, &token_op(0)).await.is_err());
        assert_eq!(puts(&f, "/api/token/"), 1);
        assert!(f.state.tokens.lock().unwrap().is_empty());
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Failed
        );
        let logins = f
            .fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == "/api/user/login" && r.body["username"] == "issuer")
            .count();
        assert_eq!(logins, 2);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_missing_ambiguous_and_expired_creation_readback_never_resends() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        create(&f, 0);
        f.state.corrupt_create.store(true, Ordering::SeqCst);
        assert!(token_step(&f, &token_op(0)).await.is_err());
        let original = f.state.tokens.lock().unwrap().remove(&101).unwrap();
        token_step(&f, &token_op(0)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Unknown
        );
        let mut second = original.clone();
        second["id"] = json!(102);
        f.state
            .tokens
            .lock()
            .unwrap()
            .extend([(101, original), (102, second)]);
        token_step(&f, &token_op(0)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().error_code,
            Some(ControlErrorCode::AmbiguousCreate)
        );
        f.state.tokens.lock().unwrap().remove(&102);
        f.state.tokens.lock().unwrap().get_mut(&101).unwrap()["status"] = json!(3);
        token_step(&f, &token_op(0)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Succeeded
        );
        let id = f.worker.token_identities(None).unwrap()[0].token_id;
        assert!(f.worker.reveal_token(id, 1).await.is_err());
        assert_eq!(puts(&f, "/api/token/"), 1);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_current_group_revision_and_lost_query_do_not_rewrite_receipts() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let id = create_confirmed(&f, 0).await;
        let receipt = f.worker.operation(&token_op(0)).unwrap();
        f.worker
            .publish(|core| {
                let g = core.store.group(f.group)?;
                core.store
                    .set_group_ready(f.group, g.revision, false, now())?;
                let g = core.store.group(f.group)?;
                core.store
                    .set_group_ready(f.group, g.revision, true, now())?;
                Ok(())
            })
            .unwrap();
        publish(&f.worker, f.members[0], false);
        f.worker.reveal_token(id, 1).await.unwrap();
        let native = native_value(&f, id);
        assert_eq!(native, 101);
        f.worker
            .set_token_enabled(token_op(1), id, 1, false)
            .unwrap();
        token_step(&f, &token_op(1)).await.unwrap();
        f.state.reject_read.store(true, Ordering::SeqCst);
        assert!(token_step(&f, &token_op(1)).await.is_err());
        let query_count = f.fake.calls.lock().unwrap().len();
        let report = f.worker.reconcile_once().await.unwrap();
        assert!(report.visited <= 1); // 旧成员自身可能仍有一次到期观测，不是token热循环。
        let token_reads = f.fake.calls.lock().unwrap()[query_count..]
            .iter()
            .filter(|r| r.path.starts_with("/api/token/"))
            .count();
        assert_eq!(token_reads, 0);
        assert_eq!(f.worker.operation(&token_op(0)).unwrap(), receipt);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5c_actual_past_creation_expiry_recovers_without_current_routing() {
    drive_case(async {
        use crate::catalog::tokens::TokenCommand;
        use crate::newapi::tokens::TokenOwner;
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let started = now() - 120;
        let expiry = now() - 60;
        let spec = f
            .worker
            .publish(|core| {
                core.store.submit_token_operation(
                    token_op(0),
                    TokenCommand::CreateToken {
                        owner: TokenOwner {
                            user_id: 41,
                            scope: TokenOwnerScope::Issuer,
                        },
                        group_id: f.group,
                        expected_group_revision: 2,
                        display_name: "expired history".into(),
                        expires_at: Some(expiry),
                    },
                    started,
                )?;
                let work = core.store.token_work(&token_op(0), started)?;
                let ticket = core.store.prepare_token_execution(&work, started)?;
                core.store
                    .settle_token_execution(&ticket, ExecutionOutcome::Uncertain, started)?;
                core.store.set_group_ready(f.group, 2, false, now())?;
                ticket.create_spec()
            })
            .unwrap();
        let mut value = token(101, 41, "managed");
        value["name"] = json!(spec.correlation_name);
        value["expired_time"] = json!(expiry);
        value["status"] = json!(3);
        f.state.tokens.lock().unwrap().insert(101, value);
        f.state.groups.lock().unwrap().clear();
        f.state.quota.store(0, Ordering::SeqCst);
        token_step(&f, &token_op(0)).await.unwrap();
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Succeeded
        );
        assert_eq!(puts(&f, "/api/token/"), 0);
        let id = f.worker.token_identities(None).unwrap()[0].token_id;
        assert!(f.worker.reveal_token(id, 1).await.is_err());
    })
    .await;
}
fn old_sql_image(f: &TokenFixture) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    let conn = rusqlite::Connection::open(f.dir.path().join("catalog.sqlite3")).unwrap();
    let names=conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT IN ('managed_tokens','token_operation_targets','token_execution_attempts') ORDER BY name").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let filter = if name == "control_operations" {
                " WHERE target_type!='token'"
            } else {
                ""
            };
            let mut statement = conn
                .prepare(&format!(
                    "SELECT * FROM \"{}\"{filter} ORDER BY rowid",
                    name.replace('"', "\"\"")
                ))
                .unwrap();
            let count = statement.column_count();
            let rows = statement
                .query_map([], |r| (0..count).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}
#[tokio::test(start_paused = true)]
async fn t5c_token_management_preserves_nonempty_ledger_and_all_old_sql_rows() {
    drive_case(async {
        use crate::catalog::ledger::{ScanLane,ScanWindow,PageEnd};
        let f=tokens_fixture(SourceKind::Glm,&[1],false,true).await;
        f.worker.publish(|core| {
            core.store.initialize_ledger(0,300).unwrap();
            let state=core.store.load_ledger_state().unwrap();
            let ticket=core.store.begin_scan(ScanLane::Recent,state.scans[0].generation,ScanWindow{from:0,to:300},300).unwrap();
            let row=json!({"id":500,"type":2,"created_at":125,"channel":1,"token_id":77,"model_name":"historical-model","group":"managed","prompt_tokens":17,"completion_tokens":5,"is_stream":false,"other":""});
            core.store.commit_fact_page(&ticket,&[row],PageEnd::ReachedEnd,300).unwrap();Ok(())
        }).unwrap();
        let before=old_sql_image(&f);assert!(!before["ledger_facts"].is_empty());
        f.state.tokens.lock().unwrap().insert(77,token(77,1,"auto"));
        let identity=f.worker.register_token(TokenOwnerScope::LegacyAdmin,77,"historical key".into()).await.unwrap();
        f.worker.set_token_enabled(token_op(0),identity.token_id,1,false).unwrap();token_step(&f,&token_op(0)).await.unwrap();token_step(&f,&token_op(0)).await.unwrap();
        f.worker.delete_token(token_op(1),identity.token_id,2).unwrap();token_step(&f,&token_op(1)).await.unwrap();token_step(&f,&token_op(1)).await.unwrap();
        assert_eq!(old_sql_image(&f),before);
    }).await;
}
