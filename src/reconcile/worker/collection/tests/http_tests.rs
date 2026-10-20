use super::*;
use crate::newapi::transport::{
    tests::{Fake, Reply},
    ManagementBudget, ManagementRequest, RequestSemantics, LIST_BODY_LIMIT,
};

// 仅替代进程证据，实际页请求仍经生产 client/transport/decoder；原生组合另跑。
struct HttpIo {
    client: Arc<NewApiClient>,
    clock: AtomicI64,
}
impl LogIo for HttpIo {
    async fn check(&self) -> Result<LocalLogLease, LogCapabilityIssue> {
        Ok(LocalLogLease::test_fixture())
    }
    async fn recheck(&self, lease: &LocalLogLease) -> Result<(), LogCapabilityIssue> {
        lease.check_time()
    }
    async fn read(
        &self,
        q: LogPageQuery,
        lease: &LocalLogLease,
        class: RequestClass,
    ) -> CollectionResult<CheckedLogPage> {
        self.client.read_log_page(q, lease, class, &|| true).await
    }
    fn now(&self) -> CollectionResult<i64> {
        Ok(self.clock.load(Ordering::SeqCst))
    }
}
fn params(path: &str) -> BTreeMap<String, i64> {
    reqwest::Url::parse(&format!("http://fixture{path}"))
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.parse().unwrap()))
        .collect()
}
#[tokio::test]
async fn collection_http_401_physical_request_counts_and_logical_four_page_budget() {
    let requests = AtomicUsize::new(0);
    let fake = Fake::start(move |r| {
        if r.path == "/api/user/login" {
            return Reply::json(json!({"success":true,"data":{"id":42}}));
        }
        if requests.fetch_add(1, Ordering::SeqCst) == 0 {
            return Reply {
                status: 401,
                ..Reply::json(json!({}))
            };
        }
        let p = params(&r.path)["p"];
        let items: Vec<_> = (1..=100)
            .map(|i| row(1001 - (p - 1) * 100 - i, 800))
            .collect();
        Reply::json(
            json!({"success":true,"data":{"page":p,"page_size":100,"total":1000,"items":items}}),
        )
    })
    .await;
    let (dir, worker) = setup(false);
    let io = HttpIo {
        client: Arc::new(NewApiClient::new(&fake.config(false)).unwrap()),
        clock: AtomicI64::new(1000),
    };
    let report = worker.collect_with(&io).await.unwrap();
    assert_eq!(
        (
            report.attempted,
            report.committed,
            report.inserted,
            report.duplicates
        ),
        (4, 4, 200, 200)
    );
    assert_eq!(persisted(&dir).len(), 200);
    let calls = fake.calls.lock().unwrap();
    assert_eq!(
        calls.iter().filter(|r| r.path == "/api/user/login").count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.path.starts_with("/api/log/"))
            .count(),
        5
    );
    assert_eq!(calls.len(), 7);
}
#[tokio::test]
async fn collection_http_background_read_does_not_occupy_control_slot() {
    let entered = Arc::new(Notify::new());
    let signal = entered.clone();
    let fake = Fake::start(move |r| {
        if r.path == "/api/control" {
            return Reply::json(json!({"success":true,"data":{}}));
        }
        signal.notify_one();
        Reply {
            delay: Duration::from_secs(60),
            ..Reply::json(json!({"success":true}))
        }
    })
    .await;
    let budget = ManagementBudget::new();
    let control_transport =
        crate::newapi::transport::ManagementTransport::new(&fake.config(true), budget.clone())
            .unwrap();
    let client = Arc::new(NewApiClient::with_budget(&fake.config(true), budget).unwrap());
    let (_dir, worker) = setup(false);
    let io = HttpIo {
        client: client.clone(),
        clock: AtomicI64::new(1000),
    };
    let mut collecting = Box::pin(worker.collect_with(&io));
    tokio::select! {_=entered.notified()=>{},result=&mut collecting=>panic!("collector unexpectedly completed: {result:?}")}
    let control = ManagementRequest::new(
        reqwest::Method::GET,
        "/api/control",
        None,
        RequestSemantics::Read,
        LIST_BODY_LIMIT,
    )
    .unwrap();
    control_transport
        .send(control, RequestClass::Control)
        .await
        .unwrap();
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
    drop(collecting);
    assert_eq!(worker.commands.available_permits(), 64);
}
#[tokio::test]
async fn collection_http_429_ends_round_even_when_body_is_not_json() {
    let fake = Fake::start(|_| Reply {
        status: 429,
        body: "not-json-secret".into(),
        headers: "Retry-After: 20\r\n".into(),
        delay: Duration::ZERO,
    })
    .await;
    let (dir, worker) = setup(false);
    let io = HttpIo {
        client: Arc::new(NewApiClient::new(&fake.config(true)).unwrap()),
        clock: AtomicI64::new(1000),
    };
    let report = worker.collect_with(&io).await.unwrap();
    assert_eq!(report.attempted, 1);
    assert_eq!(report.committed, 0);
    assert_eq!(report.state.scans[0].fault, Some(ScanFault::Transport));
    assert_eq!(report.state.scans[1].phase, ScanPhase::Idle);
    assert_eq!(worker.collect_with(&io).await.unwrap().attempted, 0);
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    assert!(persisted(&dir).is_empty());
    assert!(!format!("{:?}", report.issue).contains("not-json-secret"));
}
#[tokio::test]
async fn collection_http_count_find_changes_and_late_rows_converge_without_complete_claim() {
    let records = Arc::new(Mutex::new(
        (1..=230).map(|id| row(id, 800)).collect::<Vec<_>>(),
    ));
    let sent = Arc::new(Mutex::new(BTreeMap::new()));
    let source = records.clone();
    let delivered = sent.clone();
    let calls = AtomicUsize::new(0);
    let fake=Fake::start(move |r| {
        let q=params(&r.path);let mut source=source.lock().unwrap();
        let selected=|source:&Vec<Value>|source.iter().filter(|r|{let t=r["created_at"].as_i64().unwrap();t>=q["start_timestamp"]&&t<=q["end_timestamp"]}).cloned().collect::<Vec<_>>();
        let count=selected(&source).len();
        match calls.fetch_add(1,Ordering::SeqCst) {
            0=>{source.retain(|r|r["id"]!=230);source.push(row(1000,100));}, // 同计数替换，且插入近期窗以外。
            1=>{source.retain(|r|r["id"]!=150);}, // offset 左移。
            2=>{source.push(row(1001,850));}, // offset 右移。
            _=>{},
        }
        let mut rows=selected(&source);rows.sort_by_key(|r|std::cmp::Reverse((r["created_at"].as_i64().unwrap(),r["id"].as_i64().unwrap())));
        let items:Vec<_>=rows.into_iter().skip((q["p"] as usize-1)*100).take(100).collect();
        for r in &items{delivered.lock().unwrap().insert(r["id"].as_i64().unwrap(),r.clone());}
        Reply::json(json!({"success":true,"data":{"page":q["p"],"page_size":100,"total":count,"items":items}}))
    }).await;
    let (dir, worker) = setup(false);
    let io = HttpIo {
        client: Arc::new(NewApiClient::new(&fake.config(true)).unwrap()),
        clock: AtomicI64::new(1000),
    };
    for round in 0..5 {
        if round > 0 {
            tokio::time::pause();
            tokio::time::advance(ROUND_INTERVAL).await;
            tokio::time::resume();
            io.clock.fetch_add(1, Ordering::SeqCst);
        }
        assert!(worker.collect_with(&io).await.unwrap().attempted <= 4);
    }
    let expected: Vec<_> = sent
        .lock()
        .unwrap()
        .iter()
        .map(|(&id, r)| {
            (
                id,
                r["prompt_tokens"].as_i64().unwrap(),
                r["completion_tokens"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(persisted(&dir), expected);
    let ids: BTreeSet<_> = persisted(&dir).into_iter().map(|r| r.0).collect();
    assert!(ids.contains(&1000));
    for r in records.lock().unwrap().iter() {
        assert!(ids.contains(&r["id"].as_i64().unwrap()));
    }
}

#[tokio::test]
async fn collection_http_truncated_body_never_commits_partial_page() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(![3000, 3001].contains(&port));
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut b = [0u8; 4096];
            assert!(socket.read(&mut b).await.unwrap() > 0);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9999\r\nConnection: close\r\n\r\n{\"success\":tr").await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    let mut cfg: crate::config::NewApiConfig = toml::from_str("").unwrap();
    cfg.base_url = format!("http://127.0.0.1:{port}");
    cfg.admin_token = "fake-partial-token".into();
    let (dir, worker) = setup(false);
    let io = HttpIo {
        client: Arc::new(NewApiClient::new(&cfg).unwrap()),
        clock: AtomicI64::new(1000),
    };
    let result = worker.collect_with(&io).await.unwrap();
    assert_eq!((result.attempted, result.committed), (2, 0));
    assert!(result
        .state
        .scans
        .iter()
        .all(|p| p.fault == Some(ScanFault::Transport) && p.next_page == 1));
    assert!(persisted(&dir).is_empty());
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
}
