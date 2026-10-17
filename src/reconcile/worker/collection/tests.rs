use super::*;
use crate::newapi::logs::decode_log_page;
use proptest::prelude::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicUsize};
mod http_tests;

fn row(id: i64, at: i64) -> Value {
    json!({"id":id,"type":if id%4==0{5}else{2},"created_at":at,"channel":0,"token_id":id%3,"model_name":format!("m{}",id%2),"group":"unmapped","prompt_tokens":if id%4==0{0}else{id%10+1},"completion_tokens":if id%4==0{0}else{2},"is_stream":false,"other":""})
}
struct ModelIo {
    rows: Mutex<Vec<Value>>,
    moves: Mutex<VecDeque<(i64, Value)>>,
    delivered: Mutex<BTreeMap<i64, Value>>,
    calls: Mutex<Vec<(RequestClass, i64, ScanWindow)>>,
    now: AtomicI64,
    block: AtomicUsize,
    entered: AtomicUsize,
    rechecks: AtomicUsize,
    reject_post: AtomicBool,
    error: Mutex<Option<CollectionError>>,
    release: Notify,
    block_after: AtomicUsize,
}
impl ModelIo {
    fn new(rows: Vec<Value>) -> Self {
        Self {
            rows: Mutex::new(rows),
            moves: Mutex::new(VecDeque::new()),
            delivered: Mutex::new(BTreeMap::new()),
            calls: Mutex::new(Vec::new()),
            now: AtomicI64::new(1000),
            block: AtomicUsize::new(0),
            entered: AtomicUsize::new(0),
            rechecks: AtomicUsize::new(0),
            reject_post: AtomicBool::new(false),
            error: Mutex::new(None),
            release: Notify::new(),
            block_after: AtomicUsize::new(0),
        }
    }
    async fn gate(&self, stage: usize) {
        let calls = self.calls.lock().unwrap().len();
        let prior = self.block_after.load(Ordering::SeqCst);
        let ready = stage == 1
            || if stage == 2 {
                calls >= prior
            } else {
                calls > prior
            };
        if self.block.load(Ordering::SeqCst) == stage && ready {
            self.entered.store(stage, Ordering::SeqCst);
            self.release.notified().await;
        }
    }
}
impl LogIo for ModelIo {
    async fn check(&self) -> Result<LocalLogLease, LogCapabilityIssue> {
        self.gate(1).await;
        Ok(LocalLogLease::test_fixture())
    }
    async fn recheck(&self, _: &LocalLogLease) -> Result<(), LogCapabilityIssue> {
        let n = self.rechecks.fetch_add(1, Ordering::SeqCst);
        self.gate(if n.is_multiple_of(2) { 2 } else { 4 }).await;
        if n % 2 == 1 && self.reject_post.load(Ordering::SeqCst) {
            Err(LogCapabilityIssue::LocalBinding)
        } else {
            Ok(())
        }
    }
    async fn read(
        &self,
        query: LogPageQuery,
        _: &LocalLogLease,
        class: RequestClass,
    ) -> CollectionResult<CheckedLogPage> {
        let (window, page) = query.test_parts();
        self.calls.lock().unwrap().push((class, page, window));
        self.gate(3).await;
        if let Some(error) = self.error.lock().unwrap().take() {
            return Err(error);
        }
        let mut rows = self.rows.lock().unwrap();
        let total = rows
            .iter()
            .filter(|r| {
                let at = r["created_at"].as_i64().unwrap();
                at >= window.from && at < window.to
            })
            .count();
        if let Some((remove, add)) = self.moves.lock().unwrap().pop_front() {
            rows.retain(|r| r["id"] != remove);
            rows.push(add);
        }
        let mut ordered: Vec<_> = rows
            .iter()
            .filter(|r| {
                let at = r["created_at"].as_i64().unwrap();
                at >= window.from && at < window.to
            })
            .cloned()
            .collect();
        ordered.sort_by_key(|r| {
            std::cmp::Reverse((r["created_at"].as_i64().unwrap(), r["id"].as_i64().unwrap()))
        });
        let selected: Vec<_> = ordered
            .into_iter()
            .skip((page as usize - 1) * 100)
            .take(100)
            .collect();
        for r in &selected {
            self.delivered
                .lock()
                .unwrap()
                .entry(r["id"].as_i64().unwrap())
                .or_insert_with(|| r.clone());
        }
        decode_log_page(
            json!({"success":true,"data":{"page":page,"page_size":100,"total":total,"items":selected}}),
            &query,
        )
    }
    fn now(&self) -> CollectionResult<i64> {
        Ok(self.now.load(Ordering::SeqCst))
    }
}
fn setup(dry_run: bool) -> (tempfile::TempDir, Arc<ReconcileWorker>) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let mut config: Config =
        toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\n").unwrap();
    config.new_api.base_url = "http://127.0.0.1:9".into();
    config.new_api.admin_token = "fake-test-token".into();
    config.dry_run = dry_run;
    let client = Arc::new(NewApiClient::new(&config.new_api).unwrap());
    (
        dir,
        ReconcileWorker::new(store, client, Arc::new(config)).unwrap(),
    )
}
fn persisted(dir: &tempfile::TempDir) -> Vec<(i64, i64, i64)> {
    let conn = rusqlite::Connection::open_with_flags(
        dir.path().join("catalog.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = conn
        .prepare("SELECT native_log_id,input,output FROM ledger_facts ORDER BY native_log_id")
        .unwrap();
    statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}
async fn next(worker: &ReconcileWorker, io: &ModelIo) -> CollectionReport {
    tokio::time::advance(ROUND_INTERVAL).await;
    io.now.fetch_add(1, Ordering::SeqCst);
    worker.collect_with(io).await.unwrap()
}
#[tokio::test(start_paused = true)]
async fn collection_interleaves_fixed_windows_and_deduplicates_across_lanes() {
    let (dir, worker) = setup(false);
    let io = ModelIo::new((1..=250).map(|id| row(id, 800)).collect());
    let before = serde_json::to_value(worker.snapshot().unwrap()).unwrap();
    let first = worker.collect_with(&io).await.unwrap();
    assert_eq!(
        (first.attempted, first.committed, first.inserted),
        (4, 4, 200)
    );
    assert_eq!(first.duplicates, 200);
    assert_eq!(
        io.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| (c.0, c.1))
            .collect::<Vec<_>>(),
        vec![
            (RequestClass::Observe, 1),
            (RequestClass::Backfill, 1),
            (RequestClass::Observe, 2),
            (RequestClass::Backfill, 2)
        ]
    );
    let again = worker.collect_with(&io).await.unwrap();
    assert_eq!(again.attempted, 0);
    io.now.store(1100, Ordering::SeqCst);
    let second = next(&worker, &io).await;
    assert_eq!((second.attempted, second.inserted), (2, 50));
    assert!(second
        .state
        .scans
        .iter()
        .all(|p| p.window.unwrap().to == 1000 && p.phase == ScanPhase::Scanned));
    assert_eq!(persisted(&dir).len(), 250);
    assert_eq!(
        serde_json::to_value(worker.snapshot().unwrap()).unwrap(),
        before
    );
    let core = worker.core.lock().unwrap();
    assert!(core.permits.is_empty());
    assert!(core.last_glm_cycle.is_none());
    assert_eq!(core.input_epoch, 0);
}
#[tokio::test(start_paused = true)]
async fn collection_post_binding_check_discards_page_and_clock_failure_never_forges_time() {
    let (dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    io.reject_post.store(true, Ordering::SeqCst);
    assert_eq!(
        worker.collect_with(&io).await.err(),
        Some(CollectionError::Capability(
            LogCapabilityIssue::LocalBinding
        ))
    );
    assert!(persisted(&dir).is_empty());
    let old = worker.ledger_state().unwrap();
    io.reject_post.store(false, Ordering::SeqCst);
    io.now.store(999, Ordering::SeqCst);
    tokio::time::advance(ROUND_INTERVAL).await;
    assert_eq!(
        worker.collect_with(&io).await.err(),
        Some(CollectionError::Clock)
    );
    assert_eq!(worker.ledger_state().unwrap(), old);
    io.now.store(1001, Ordering::SeqCst);
    next(&worker, &io).await;
    assert_eq!(persisted(&dir).len(), 1);
}
#[tokio::test(start_paused = true)]
async fn collection_conflict_stops_both_lanes_and_bad_pages_never_skip() {
    let (dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    worker.collect_with(&io).await.unwrap();
    io.rows.lock().unwrap()[0]["prompt_tokens"] = json!(9);
    let result = next(&worker, &io).await;
    assert_eq!(result.attempted, 1);
    assert!(result
        .state
        .scans
        .iter()
        .any(|p| p.fault == Some(ScanFault::ConflictingIdentity)));
    let calls = io.calls.lock().unwrap().len();
    let stalled = next(&worker, &io).await;
    assert_eq!(stalled.attempted, 0);
    assert_eq!(io.calls.lock().unwrap().len(), calls);
    assert_eq!(persisted(&dir), vec![(1, 2, 2)]);
    let (_dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    *io.error.lock().unwrap() = Some(CollectionError::InvalidPage);
    let result = worker.collect_with(&io).await.unwrap();
    assert_eq!(result.state.scans[0].fault, Some(ScanFault::InvalidPage));
    assert_eq!(result.state.scans[0].next_page, 1);
    assert_eq!(result.state.scans[1].phase, ScanPhase::Scanned);
    let result = next(&worker, &io).await;
    assert_eq!(result.state.scans[0].generation, 1);
    assert_eq!(result.state.scans[0].next_page, 1);
}
#[tokio::test(start_paused = true)]
async fn collection_dry_run_queue_capacity_and_429_do_not_add_writes_or_retry_loops() {
    let (_dir, worker) = setup(true);
    let io = ModelIo::new(vec![row(1, 800)]);
    let before = worker.ledger_state().unwrap();
    assert!(worker.collect_with(&io).await.unwrap().preview_only);
    assert!(io.calls.lock().unwrap().is_empty());
    assert_eq!(worker.ledger_state().unwrap(), before);
    let (_dir, worker) = setup(false);
    let slots = worker.commands.acquire_many(64).await.unwrap();
    assert_eq!(
        worker.collect_with(&io).await.err(),
        Some(CollectionError::Busy)
    );
    drop(slots);
    *io.error.lock().unwrap() = Some(CollectionError::Management(ManagementError {
        kind: ManagementErrorKind::InvalidResponse,
        status: Some(429),
        retry_after: Some(Duration::from_secs(60)),
        delivery: DeliveryEvidence::Uncertain,
    }));
    let result = worker.collect_with(&io).await.unwrap();
    assert_eq!(result.attempted, 1);
    assert_eq!(result.state.scans[0].fault, Some(ScanFault::Transport));
    assert_eq!(result.state.scans[1].phase, ScanPhase::Idle);
    assert_eq!(worker.collect_with(&io).await.unwrap().attempted, 0);
}
#[tokio::test(start_paused = true)]
async fn collection_total_deadline_and_shutdown_release_resources_without_losing_confirmed_state() {
    let (dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    io.block.store(2, Ordering::SeqCst);
    let mut pending = Box::pin(worker.collect_with(&io));
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    tokio::time::advance(ROUND_TIMEOUT).await;
    assert!(pending.await.is_err());
    assert_eq!(worker.commands.available_permits(), 64);
    assert!(persisted(&dir).is_empty());
    let (tx, rx) = watch::channel(true);
    drop(tx);
    let cfg: crate::config::ManageConfig = toml::from_str("").unwrap();
    let process = NewApiProcess::new(&cfg, "http://127.0.0.1:9").unwrap();
    worker.run_fact_collection(&process, rx).await.unwrap();
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb4_collection_moving_offsets_and_late_history_match_observed_reference(n in 110i64..241, mutations in prop::collection::vec((1i64..241,0i64..1000),0..10)) {
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            tokio::time::pause();let (dir,worker)=setup(false);
            let io=ModelIo::new((1..=n).map(|id|row(id,if id%3==0{500}else{800})).collect());
            *io.moves.lock().unwrap()=mutations.iter().enumerate().map(|(i,(remove,at))|(*remove,row(1000+i as i64,*at))).collect();
            for _ in 0..18 {let r=next(&worker,&io).await;assert!(r.attempted<=4);}
            assert!(io.moves.lock().unwrap().is_empty());
            let expected:Vec<_>=io.delivered.lock().unwrap().iter().map(|(&id,r)|(id,r["prompt_tokens"].as_i64().unwrap(),r["completion_tokens"].as_i64().unwrap())).collect();
            assert_eq!(persisted(&dir),expected);
            let actual:BTreeSet<_>=persisted(&dir).into_iter().map(|r|r.0).collect();
            for r in io.rows.lock().unwrap().iter(){assert!(actual.contains(&r["id"].as_i64().unwrap()));}
            assert_eq!(worker.ledger_state().unwrap().initial_requested_from,Some(0));
        });
    }
    #[test]
    fn pb5_collection_singleflight_cancellation_and_fair_retry(stage in 1usize..5, count in 1i64..30, partial in any::<bool>(), failure in 0u8..5) {
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            tokio::time::pause();let (dir,worker)=setup(false);let io=ModelIo::new((1..=count).map(|id|row(id,800)).collect());io.block.store(stage,Ordering::SeqCst);
            let partial=partial&&stage!=1;io.block_after.store(usize::from(partial),Ordering::SeqCst);
            let mut pending=Box::pin(worker.collect_with(&io));
            std::future::poll_fn(|cx|{assert!(pending.as_mut().poll(cx).is_pending());std::task::Poll::Ready(())}).await;
            assert_eq!(io.entered.load(Ordering::SeqCst),stage);assert_eq!(worker.commands.available_permits(),63);
            assert_eq!(worker.collect_with(&io).await.err(),Some(CollectionError::Busy));
            drop(pending);assert_eq!(worker.commands.available_permits(),64);assert_eq!(persisted(&dir).len(),if partial{count as usize}else{0});
            let before=worker.ledger_state().unwrap();assert_eq!(worker.collect_with(&io).await.unwrap().attempted,0);
            io.block.store(0,Ordering::SeqCst);io.rechecks.store(0,Ordering::SeqCst);io.calls.lock().unwrap().clear();
            let result=next(&worker,&io).await;assert!(result.attempted<=4);assert_eq!(persisted(&dir).len(),count as usize);
            if stage!=1 {let index=usize::from(partial);assert_eq!(io.calls.lock().unwrap()[0].0,if partial{RequestClass::Observe}else{RequestClass::Backfill});assert_eq!(result.state.scans[index].generation,before.scans[index].generation+1);}
            let error=if failure==4{CollectionError::InvalidPage}else{CollectionError::Management(ManagementError{kind:[ManagementErrorKind::TransportUnknown,ManagementErrorKind::RateLimited,ManagementErrorKind::AuthFailed,ManagementErrorKind::QueueFull][failure as usize],status:(failure==1).then_some(429),retry_after:None,delivery:DeliveryEvidence::Uncertain})};
            *io.error.lock().unwrap()=Some(error.clone());io.calls.lock().unwrap().clear();
            let failed=next(&worker,&io).await;assert_eq!(failed.issue,Some(error));assert_eq!(persisted(&dir).len(),count as usize);
            let lane=if io.calls.lock().unwrap()[0].0==RequestClass::Observe{0}else{1};
            assert_eq!(failed.state.scans[lane].fault,match failure{0|1=>Some(ScanFault::Transport),4=>Some(ScanFault::InvalidPage),_=>None});
            if failure==1||failure==2{assert_eq!(failed.attempted,1);}
            let slots=worker.commands.acquire_many(64).await.unwrap();assert_eq!(worker.collect_with(&io).await.err(),Some(CollectionError::Busy));drop(slots);
        });
    }
}

#[tokio::test(start_paused = true)]
async fn collection_background_sleep_stop_restart_and_multiple_runners_preserve_progress() {
    for cancel in [false, true] {
        let (dir, worker) = setup(false);
        let io = ModelIo::new((1..=250).map(|id| row(id, 800)).collect());
        let (tx, rx) = watch::channel(false);
        let mut run = Box::pin(worker.run_collection_with(&io, rx));
        std::future::poll_fn(|cx| {
            assert!(run.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        let prior = worker.ledger_state().unwrap();
        assert!(prior
            .scans
            .iter()
            .all(|p| p.next_page == 3 && p.phase == ScanPhase::Scanning));
        let (_tx2, rx2) = watch::channel(false);
        assert_eq!(
            worker.run_collection_with(&io, rx2).await.err(),
            Some(CollectionError::Busy)
        );
        if cancel {
            drop(run);
        } else {
            tx.send(true).unwrap();
            run.await.unwrap();
        }
        assert!(!worker.collection_running.load(Ordering::SeqCst));
        assert_eq!(persisted(&dir).len(), 200);
        let calls = io.calls.lock().unwrap().len();
        let (tx, rx) = watch::channel(false);
        let mut run = Box::pin(worker.run_collection_with(&io, rx));
        std::future::poll_fn(|cx| {
            assert!(run.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(io.calls.lock().unwrap().len(), calls);
        tokio::time::advance(ROUND_INTERVAL).await;
        io.now.store(1100, Ordering::SeqCst);
        std::future::poll_fn(|cx| {
            assert!(run.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        let after = worker.ledger_state().unwrap();
        for (old, new) in prior.scans.iter().zip(&after.scans) {
            assert_eq!(new.generation, old.generation + 1);
            assert_eq!(new.window, old.window);
            assert_eq!(new.next_page, 3);
        }
        assert_eq!(
            io.calls.lock().unwrap()[calls..]
                .iter()
                .map(|c| c.1)
                .collect::<Vec<_>>(),
            vec![1, 1, 2, 2]
        );
        assert_eq!(persisted(&dir).len(), 200);
        tx.send(true).unwrap();
        run.await.unwrap();
    }
}
#[tokio::test(start_paused = true)]
async fn collection_background_inflight_shutdown_and_dry_run_wait_without_spinning() {
    for dry_run in [false, true] {
        let (dir, worker) = setup(dry_run);
        let io = ModelIo::new(vec![row(1, 800)]);
        io.block.store(3, Ordering::SeqCst);
        let (tx, rx) = watch::channel(false);
        let mut run = Box::pin(worker.run_collection_with(&io, rx));
        std::future::poll_fn(|cx| {
            assert!(run.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(io.calls.lock().unwrap().len(), usize::from(!dry_run));
        tx.send(true).unwrap();
        run.await.unwrap();
        assert!(persisted(&dir).is_empty());
        assert_eq!(worker.commands.available_permits(), 64);
        assert!(!worker.collection_running.load(Ordering::SeqCst));
    }
}

#[tokio::test(start_paused = true)]
async fn collection_page_timeouts_retry_same_window_and_explicit_retry_does_not_skip() {
    let (dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    io.block.store(3, Ordering::SeqCst);
    let mut pending = Box::pin(worker.collect_with(&io));
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    tokio::time::advance(PAGE_TIMEOUT).await;
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(
        worker.ledger_state().unwrap().scans[0].fault,
        Some(ScanFault::Transport)
    );
    tokio::time::advance(PAGE_TIMEOUT).await;
    let timed = pending.await.unwrap();
    assert_eq!((timed.attempted, timed.committed), (2, 0));
    assert!(persisted(&dir).is_empty());
    io.block.store(0, Ordering::SeqCst);
    let recovered = next(&worker, &io).await;
    assert_eq!(
        recovered.state.scans[0].window,
        Some(ScanWindow {
            from: 700,
            to: 1000
        })
    );
    assert_eq!(recovered.state.scans[0].generation, 2);
    let (_dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    *io.error.lock().unwrap() = Some(CollectionError::InvalidPage);
    let result = worker.collect_with(&io).await.unwrap();
    let gen = result.state.scans[0].generation;
    assert!(worker
        .retry_fact_with(ScanLane::Recent, gen + 1, &io)
        .await
        .is_err());
    let retried = worker
        .retry_fact_with(ScanLane::Recent, gen, &io)
        .await
        .unwrap();
    assert_eq!(retried.generation, gen + 1);
    assert_eq!(retried.next_page, 1);
    let done = next(&worker, &io).await;
    assert_eq!(done.state.scans[0].generation, gen + 1);
}
#[tokio::test(start_paused = true)]
async fn collection_waiting_page_does_not_hold_core_or_fence_unrelated_control() {
    let (dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    io.block.store(3, Ordering::SeqCst);
    let mut pending = Box::pin(worker.collect_with(&io));
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    worker
        .create_group(GroupDraft {
            name: "control-during-log-read".into(),
            newapi_group: "control-during-log-read".into(),
            kind: SourceKind::Glm,
        })
        .unwrap();
    io.block.store(0, Ordering::SeqCst);
    io.release.notify_one();
    assert_eq!(pending.await.unwrap().inserted, 1);
    assert_eq!(persisted(&dir).len(), 1);
}
#[tokio::test(start_paused = true)]
async fn collection_unknown_commit_keeps_writer_poisoned_and_requires_reopen() {
    let (dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    worker
        .ledger_mutation(|s| s.initialize_ledger(500, 1000))
        .unwrap();
    let path = dir.path().join("catalog.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE test_parent(id INTEGER PRIMARY KEY); CREATE TABLE test_child(id INTEGER REFERENCES test_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER test_fault AFTER INSERT ON ledger_facts BEGIN INSERT INTO test_child VALUES(1); END;").unwrap();
    assert!(worker.collect_with(&io).await.is_err());
    assert!(worker.core.lock().unwrap().store.usable().is_err());
    assert!(!worker.publication.load(Ordering::SeqCst).is_multiple_of(2));
    assert!(persisted(&dir).is_empty());
    assert_eq!(io.calls.lock().unwrap().len(), 1);
    db.execute_batch("DROP TRIGGER test_fault;").unwrap();
    drop(db);
    let client = worker.client.clone();
    let cfg = worker.policy_config.clone();
    drop(worker);
    let reopened = ReconcileWorker::new(CatalogStore::open(&path).unwrap(), client, cfg).unwrap();
    assert_eq!(reopened.collect_with(&io).await.unwrap().inserted, 1);
    assert_eq!(
        reopened.ledger_state().unwrap().initial_requested_from,
        Some(500)
    );
}

#[tokio::test(start_paused = true)]
async fn collection_unencodable_saved_window_stalls_only_its_lane() {
    let (dir, worker) = setup(false);
    let io = ModelIo::new(vec![row(1, 800)]);
    worker
        .ledger_mutation(|s| s.initialize_ledger(0, 1000))
        .unwrap();
    worker
        .ledger_mutation(|s| s.begin_scan(ScanLane::Recent, 0, ScanWindow { from: 0, to: 1 }, 1000))
        .unwrap();
    let report = worker.collect_with(&io).await.unwrap();
    assert_eq!(report.state.scans[0].fault, Some(ScanFault::InvalidPage));
    assert_eq!(
        report.state.scans[0].window,
        Some(ScanWindow { from: 0, to: 1 })
    );
    assert_eq!(report.state.scans[0].next_page, 1);
    assert_eq!(report.state.scans[1].phase, ScanPhase::Scanned);
    assert_eq!((report.attempted, report.inserted), (1, 1));
    assert_eq!(io.calls.lock().unwrap()[0].0, RequestClass::Backfill);
    let generation = report.state.scans[0].generation;
    let again = next(&worker, &io).await;
    assert_eq!(again.state.scans[0].generation, generation);
    assert_eq!(persisted(&dir), vec![(1, 2, 2)]);
}
