use super::*;
use crate::catalog::ledger::{LedgerError, PageEnd, ScanLane, ScanWindow};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc;
mod attribution_tests;

fn setup() -> (tempfile::TempDir, Arc<ReconcileWorker>) {
    setup_with(false)
}
fn setup_with(dry_run: bool) -> (tempfile::TempDir, Arc<ReconcileWorker>) {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    let mut config: Config =
        toml::from_str("poll_interval_secs=10\nkeys=[]\n[zhipu]\n[new_api]\n").unwrap();
    config.new_api.base_url = "http://127.0.0.1:9".into();
    config.new_api.admin_token = "fake-query-only".into();
    config.dry_run = dry_run;
    let client = Arc::new(NewApiClient::new(&config.new_api).unwrap());
    (
        dir,
        ReconcileWorker::new(store, client, Arc::new(config)).unwrap(),
    )
}
fn query() -> UsageQuery {
    UsageQuery::new(
        ScanWindow { from: 0, to: 300 },
        480,
        UsageGrain::Hour,
        UsageFilter::default(),
    )
    .unwrap()
}
fn initialize(worker: &ReconcileWorker) {
    let mut core = worker.core.lock().unwrap();
    core.store.initialize_ledger(0, 300).unwrap();
    core.store
        .begin_scan(ScanLane::Recent, 0, ScanWindow { from: 0, to: 300 }, 300)
        .unwrap();
}
fn add(worker: &ReconcileWorker, id: i64) {
    worker.mutate(None,|core|{let ticket=core.store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();Ok(core.store.commit_fact_page(&ticket,&[json!({"id":id,"type":2,"created_at":125,"channel":0,"token_id":1,"model_name":"fake-model","group":null,"prompt_tokens":5,"completion_tokens":3,"is_stream":false,"other":""})],PageEnd::More,300))}).unwrap().unwrap();
}
async fn released(slots: &Semaphore) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while slots.available_permits() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
// 一次真实读快照冻结后阻塞；控制任务可独立写库，取消不能伪装成工作已经退出。
fn blocked_query(
    worker: &Arc<ReconcileWorker>,
    timeout: Duration,
) -> (
    tokio::task::JoinHandle<AnalyticsResult<UsageSnapshot>>,
    tokio::sync::oneshot::Receiver<()>,
    mpsc::Sender<()>,
) {
    let (entered_tx, entered) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    let worker = worker.clone();
    let task = tokio::spawn(async move {
        worker
            .query_usage_with(query(), timeout, move |target, q, control| {
                let mut entered = Some(entered_tx);
                target.test_query(q, &control, MAX_QUERY_ROWS, || {
                    if let Some(tx) = entered.take() {
                        let _ = tx.send(());
                        let _ = wait.recv_timeout(Duration::from_secs(10));
                    }
                })
            })
            .await
    });
    (task, entered, release)
}

#[tokio::test]
async fn query_worker_reads_without_command_or_routing_locks_and_keeps_state() {
    let (_dir, worker) = setup();
    assert!(worker.query_usage(query()).await.unwrap().usage.is_none());
    initialize(&worker);
    add(&worker, 1);
    let before = serde_json::to_value(worker.snapshot().unwrap()).unwrap();
    let state = worker
        .core
        .lock()
        .unwrap()
        .store
        .load_ledger_state()
        .unwrap();
    let epoch = worker.publication.load(Ordering::SeqCst);
    let input = worker.core.lock().unwrap().input_epoch;
    let _commands = worker.commands.try_acquire_many(64).unwrap();
    let _lane = worker.lane.lock().await;
    let _collector = worker.collector.lock().await;
    let snapshot = worker.query_usage(query()).await.unwrap();
    assert_eq!(snapshot.state, state);
    assert_eq!(snapshot.usage.unwrap().total.consumption_rows, 1);
    assert_eq!(
        serde_json::to_value(worker.snapshot().unwrap()).unwrap(),
        before
    );
    assert_eq!(worker.publication.load(Ordering::SeqCst), epoch);
    assert_eq!(worker.core.lock().unwrap().input_epoch, input);
}
#[tokio::test]
async fn query_worker_allows_control_writes_while_snapshot_is_open() {
    let (_dir, worker) = setup();
    initialize(&worker);
    add(&worker, 1);
    let old = worker
        .core
        .lock()
        .unwrap()
        .store
        .load_ledger_state()
        .unwrap();
    let (task, entered, release) = blocked_query(&worker, Duration::from_secs(60));
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        worker.query_usage(query()).await.err(),
        Some(AnalyticsError::Busy)
    );
    assert_eq!(worker.commands.available_permits(), 64);
    add(&worker, 2);
    release.send(()).unwrap();
    let observed = task.await.unwrap().unwrap();
    assert_eq!(observed.state, old);
    assert_eq!(observed.usage.unwrap().total.consumption_rows, 1);
    assert_eq!(
        worker
            .query_usage(query())
            .await
            .unwrap()
            .usage
            .unwrap()
            .total
            .consumption_rows,
        2
    );
}
#[tokio::test]
async fn query_worker_cancel_keeps_slot_and_directory_guard_until_actual_exit() {
    let (dir, worker) = setup();
    initialize(&worker);
    add(&worker, 1);
    let slots = worker.query_slots.clone();
    let (task, entered, release) = blocked_query(&worker, Duration::from_secs(60));
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.err().unwrap().is_cancelled());
    assert_eq!(
        worker.query_usage(query()).await.err(),
        Some(AnalyticsError::Busy)
    );
    drop(worker);
    assert_eq!(
        CatalogStore::open(&dir.path().join("catalog.sqlite3"))
            .err()
            .unwrap()
            .kind,
        ErrorKind::AlreadyOpen
    );
    release.send(()).unwrap();
    released(&slots).await;
    assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_ok());
}
#[tokio::test]
async fn query_worker_timeout_does_not_release_running_slot_early() {
    let (_dir, worker) = setup();
    initialize(&worker);
    add(&worker, 1);
    let (task, entered, release) = blocked_query(&worker, Duration::from_secs(60));
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(task.await.unwrap().err(), Some(AnalyticsError::TimedOut));
    tokio::time::resume();
    assert_eq!(
        worker.query_usage(query()).await.err(),
        Some(AnalyticsError::Busy)
    );
    release.send(()).unwrap();
    released(&worker.query_slots).await;
    assert_eq!(
        worker
            .query_usage(query())
            .await
            .unwrap()
            .usage
            .unwrap()
            .total
            .consumption_rows,
        1
    );
}
#[tokio::test]
async fn query_worker_poisoned_writer_blocks_new_and_completed_results() {
    let (dir, worker) = setup();
    initialize(&worker);
    add(&worker, 1);
    let db = rusqlite::Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE test_parent(id INTEGER PRIMARY KEY); CREATE TABLE test_child(id INTEGER REFERENCES test_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER test_fault AFTER INSERT ON ledger_facts BEGIN INSERT INTO test_child VALUES(1); END;").unwrap();
    let (task, entered, release) = blocked_query(&worker, Duration::from_secs(60));
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .unwrap()
        .unwrap();
    let failure=worker.mutate(None,|core| {
        let ticket=core.store.load_scan_ticket(ScanLane::Recent).unwrap().unwrap();
        Ok(core.store.commit_fact_page(&ticket,&[json!({"id":2,"type":2,"created_at":125,"channel":0,"token_id":1,"model_name":"fake-model","group":null,"prompt_tokens":5,"completion_tokens":3,"is_stream":false,"other":""})],PageEnd::More,300))
    }).unwrap().err().unwrap();
    assert!(matches!(failure,LedgerError::Catalog(e) if e.kind==ErrorKind::CommitUnknown));
    release.send(()).unwrap();
    assert!(
        matches!(task.await.unwrap().err(),Some(AnalyticsError::Ledger(LedgerError::Catalog(e))) if e.kind==ErrorKind::CommitUnknown)
    );
    assert!(
        matches!(worker.query_usage(query()).await.err(),Some(AnalyticsError::Ledger(LedgerError::Catalog(e))) if e.kind==ErrorKind::CommitUnknown)
    );
}
#[tokio::test]
async fn query_worker_core_contention_is_cancellable_before_dispatch() {
    let (_dir, worker) = setup();
    let w = worker.clone();
    let (ready_tx, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _core = w.core.lock().unwrap();
        ready_tx.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    ready.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), worker.query_usage(query()))
            .await
            .is_err()
    );
    assert_eq!(worker.query_slots.available_permits(), 1);
    release.send(()).unwrap();
    holder.join().unwrap();
    assert!(worker.query_usage(query()).await.unwrap().usage.is_none());
}

#[test]
fn query_worker_queued_cancellation_keeps_guard_and_never_runs_query() {
    use std::future::Future;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let (ready_tx, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let blocker = rt.spawn_blocking(move || {
        ready_tx.send(()).unwrap();
        let _ = wait.recv_timeout(Duration::from_secs(10));
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    rt.block_on(async {
        let (dir, worker) = setup();
        initialize(&worker);
        let slots = worker.query_slots.clone();
        let ran = Arc::new(AtomicBool::new(false));
        let mark = ran.clone();
        let mut pending = Box::pin(worker.query_usage_with(
            query(),
            Duration::from_secs(60),
            move |target, q, c| {
                mark.store(true, Ordering::SeqCst);
                target.query(q, &c)
            },
        ));
        std::future::poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(slots.available_permits(), 0);
        drop(pending);
        drop(worker);
        assert_eq!(slots.available_permits(), 0);
        assert_eq!(
            CatalogStore::open(&dir.path().join("catalog.sqlite3"))
                .err()
                .unwrap()
                .kind,
            ErrorKind::AlreadyOpen
        );
        release.send(()).unwrap();
        blocker.await.unwrap();
        released(&slots).await;
        assert!(!ran.load(Ordering::SeqCst));
        assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_ok());
    });
}
