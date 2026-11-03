use super::*;
use crate::panel_http::AdminTestServer;

#[tokio::test]
async fn h2_http_usage_and_shutdown_wait_for_timed_out_inner_sqlite_reader() {
    let (dir, worker) = setup();
    initialize(&worker);
    add(&worker, 1);
    let server = AdminTestServer::start(worker.clone()).await;
    let request = json!({"schema_version":1,"from":0,"to":300,"utc_offset_minutes":480,"grain":"hour","filter":{"native_token_id":{"mode":"exact","value":"1"}}});
    let (status, report) = server.call("POST", "/usage/query", Some(&request)).await;
    assert_eq!(status, 200);
    assert_eq!(
        report["usage"]["total"],
        json!({"input":5,"output":3,"total":8,"consumption_rows":1})
    );
    assert_eq!(report["usage"]["models"][0]["model"], "fake-model");
    assert_eq!(report["usage"]["tokens"][0]["native_token_id"], "1");
    assert_eq!(
        report["usage"]["buckets"][0]["models"][0]["usage"],
        report["usage"]["total"]
    );
    let (query, entered, release) = blocked_query(&worker, Duration::from_secs(60));
    entered.await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(query.await.unwrap().err(), Some(AnalyticsError::TimedOut));
    tokio::time::resume();
    assert_eq!(worker.query_slots.available_permits(), 0);
    // 外部查询调用方也已停止；它的真实SQLite尾部仍持有原槽和目录guard。
    server.stop.send(true).unwrap();
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(!server.task.is_finished());
    release.send(()).unwrap();
    server.task.await.unwrap().unwrap();
    assert_eq!(worker.query_slots.available_permits(), 1);
    assert!(worker.query_usage(super::query()).await.is_ok());
    drop(worker);
    assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_ok());
}
