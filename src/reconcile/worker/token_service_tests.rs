use super::*;
use crate::newapi::tokens::TokenOwnerScope;
use crate::newapi::transport::{tests::Recorded, ManagementBudget};
use crate::reconcile::worker::token_service::TokenOwners;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicI64;
#[path = "token_service_tests/coverage.rs"]
mod coverage;
#[path = "token_service_tests/edges.rs"]
mod edges;
#[path = "token_service_tests/native.rs"]
mod native;
#[path = "token_service_tests/runtime_cooperation.rs"]
mod runtime_cooperation;
// 复用原生token假后端，只从新增HTTP入口驱动，避免复制身份/渠道夹具。
#[path = "../../panel_http/admin/tests/tokens.rs"]
mod panel_admin;

const ACCESS_KEY: &str = "PrivateFakeAccessKey0123456789abcdef0123456789ab";
fn ok(value: Value) -> Reply {
    Reply::json(json!({"success":true,"data":value}))
}
fn caller(r: &Recorded) -> i64 {
    r.raw
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("new-api-user"))
                .map(|(_, v)| v.trim().parse().unwrap())
        })
        .unwrap_or(1)
}
fn token(id: i64, owner: i64, group: &str) -> Value {
    json!({"id":id,"user_id":owner,"name":"legacy","key":ACCESS_KEY,"group":group,"status":1,"created_time":100,"expired_time":-1,"remain_quota":0,"used_quota":0,"unlimited_quota":true,"model_limits_enabled":false,"model_limits":"","allow_ips":"","cross_group_retry":false})
}
type RequestHook = Arc<dyn Fn(&Recorded) + Send + Sync>;
struct TokenState {
    native: NativeState,
    tokens: Mutex<BTreeMap<i64, Value>>,
    quota: AtomicI64,
    role: AtomicI64,
    active: AtomicBool,
    groups: Mutex<Vec<String>>,
    sequence: AtomicI64,
    corrupt_create: AtomicBool,
    corrupt_status: AtomicBool,
    delay_create: AtomicBool,
    delay_status: AtomicBool,
    reject_read: AtomicBool,
    unauthorized_create: AtomicBool,
    saw_unauthorized: AtomicBool,
    hook: Mutex<Option<RequestHook>>,
}
impl TokenState {
    fn new() -> Self {
        Self {
            native: NativeState::default(),
            tokens: Mutex::new(BTreeMap::new()),
            quota: AtomicI64::new(10000),
            role: AtomicI64::new(1),
            active: AtomicBool::new(true),
            groups: Mutex::new(vec!["managed".into()]),
            sequence: AtomicI64::new(100),
            corrupt_create: AtomicBool::new(false),
            corrupt_status: AtomicBool::new(false),
            delay_create: AtomicBool::new(false),
            delay_status: AtomicBool::new(false),
            reject_read: AtomicBool::new(false),
            unauthorized_create: AtomicBool::new(false),
            saw_unauthorized: AtomicBool::new(false),
            hook: Mutex::new(None),
        }
    }
    fn reply(&self, r: &Recorded) -> Reply {
        if let Some(hook) = self.hook.lock().unwrap().as_ref().cloned() {
            hook(r);
        }
        let user = caller(r);
        if r.path.starts_with("/api/channel/") {
            return self.native.reply(r);
        }
        if r.path == "/api/user/login" {
            return ok(json!({"id":if r.body["username"]=="issuer" {41}else{1}}));
        }
        if r.path == "/api/user/self" {
            return ok(
                json!({"id":user,"role":if user==41 {self.role.load(Ordering::SeqCst)}else{100},"status":if self.active.load(Ordering::SeqCst){1}else{2},"group":"accounts","quota":self.quota.load(Ordering::SeqCst)}),
            );
        }
        if r.path == "/api/user/self/groups" {
            return ok(Value::Object(
                self.groups
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|name| (name.clone(), json!({"ratio":1})))
                    .collect(),
            ));
        }
        if r.path == "/api/group/" {
            return ok(json!(self.groups.lock().unwrap().clone()));
        }
        if r.path.starts_with("/api/token/") {
            let mut tokens = self.tokens.lock().unwrap();
            if r.method == "GET" && self.reject_read.load(Ordering::SeqCst) {
                return Reply {
                    status: 503,
                    ..Reply::json(json!({"success":false}))
                };
            }
            if r.method == "GET"
                && (r.path.starts_with("/api/token/search?") || r.path.starts_with("/api/token/?"))
            {
                let url = reqwest::Url::parse(&format!("http://fixture{}", r.path)).unwrap();
                let args: HashMap<_, _> = url.query_pairs().into_owned().collect();
                let page = args
                    .get("p")
                    .map(|s| s.parse::<usize>().unwrap())
                    .unwrap_or(1);
                let size = args
                    .get("page_size")
                    .map(|s| s.parse::<usize>().unwrap())
                    .unwrap_or(100);
                let list: Vec<_> = tokens
                    .values()
                    .rev()
                    .filter(|v| {
                        v["user_id"] == user
                            && args
                                .get("keyword")
                                .is_none_or(|tag| v["name"].as_str() == Some(tag))
                    })
                    .cloned()
                    .collect();
                let total = list.len();
                let items: Vec<_> = list
                    .into_iter()
                    .skip((page - 1) * size)
                    .take(size)
                    .collect();
                return ok(json!({"items":items,"total":total,"page":page,"page_size":size}));
            }
            if r.method == "POST" && r.path == "/api/token/" {
                if self.unauthorized_create.swap(false, Ordering::SeqCst) {
                    self.saw_unauthorized.store(true, Ordering::SeqCst);
                    return Reply {
                        status: 401,
                        ..Reply::json(json!({"success":false}))
                    };
                }
                let id = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
                let mut value = token(id, user, r.body["group"].as_str().unwrap());
                for (key, val) in r.body.as_object().unwrap() {
                    value[key] = val.clone();
                }
                tokens.insert(id, value);
                let mut reply = Reply::json(json!({"success":true}));
                if self.corrupt_create.load(Ordering::SeqCst) {
                    reply.body = "{".into();
                }
                if self.delay_create.load(Ordering::SeqCst) {
                    reply.delay = Duration::from_millis(500);
                }
                return reply;
            }
            if r.method == "POST" && r.path == "/api/token/batch" {
                let id = r.body["ids"][0].as_i64().unwrap();
                let count = if tokens.get(&id).is_some_and(|t| t["user_id"] == user) {
                    tokens.remove(&id);
                    1
                } else {
                    0
                };
                return ok(json!(count));
            }
            if r.method == "PUT" && r.path == "/api/token/?status_only=true" {
                let id = r.body["id"].as_i64().unwrap();
                let value = tokens.get_mut(&id).unwrap();
                assert_eq!(value["user_id"], user);
                value["status"] = r.body["status"].clone();
                let mut reply = ok(value.clone());
                if self.corrupt_status.load(Ordering::SeqCst) {
                    reply.body = "{".into();
                }
                if self.delay_status.load(Ordering::SeqCst) {
                    reply.delay = Duration::from_millis(500);
                }
                return reply;
            }
            let id = r.path.split('/').nth(3).unwrap().parse::<i64>().unwrap();
            let value = tokens.get(&id).filter(|v| v["user_id"] == user);
            if r.method == "GET" {
                return value.cloned().map(ok).unwrap_or_else(|| {
                    Reply::json(json!({"success":false,"message":"record not found"}))
                });
            }
            if r.method == "POST" && r.path.ends_with("/key") {
                assert!(value.is_some());
                return ok(json!({"key":ACCESS_KEY}));
            }
        }
        panic!("unexpected token service fake request");
    }
}
struct TokenFixture {
    dir: tempfile::TempDir,
    fake: Fake,
    state: Arc<TokenState>,
    worker: Arc<ReconcileWorker>,
    group: GroupId,
    members: Vec<MembershipId>,
}
async fn tokens_fixture(
    kind: SourceKind,
    statuses: &[i64],
    issuer: bool,
    legacy: bool,
) -> TokenFixture {
    tokens_fixture_with_auth(kind, statuses, issuer, legacy, false).await
}
async fn tokens_fixture_with_auth(
    kind: SourceKind,
    statuses: &[i64],
    issuer: bool,
    legacy: bool,
    session: bool,
) -> TokenFixture {
    let state = Arc::new(TokenState::new());
    let handler = state.clone();
    let fake = Fake::start(move |r| handler.reply(r)).await;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = dir.path().join("catalog.sqlite3");
    let mut store = CatalogStore::open(&path).unwrap();
    let group = store
        .create_group(GroupDraft {
            name: "managed".into(),
            newapi_group: "managed".into(),
            kind,
        })
        .unwrap()
        .group_id;
    let mut members = Vec::new();
    for (i, status) in statuses.iter().enumerate() {
        let upstream = UpstreamProfile {
            channel_type: 8,
            base_url: format!("https://source-{i}.invalid/v1"),
            models: vec![format!("model-{i}")],
            model_discovery: None,
        };
        let settings = if kind == SourceKind::Glm {
            SourceSettings::Glm {
                upstream,
                quota: GlmQuotaProfile {
                    quota_url: "https://quota.invalid".into(),
                    global_headers: vec![],
                    key_headers: vec![],
                },
            }
        } else {
            SourceSettings::Volcano {
                upstream,
                policy: VolcanoPolicy::default(),
            }
        };
        let source = store
            .create_source(SourceDraft {
                name: format!("source-{i}"),
                note: String::new(),
                settings,
                credential: SecretString::new(format!("private-test-source-{i}")).unwrap(),
            })
            .unwrap();
        let member = store
            .plan_membership(source.source_id, group, 100 - i as i64)
            .unwrap();
        members.push(member.membership_id);
        let conn = rusqlite::Connection::open(&path).unwrap();
        let native = i as i64 + 1;
        conn.execute(
            "UPDATE memberships SET channel_id=?1,lifecycle='imported' WHERE membership_id=?2",
            rusqlite::params![native, member.membership_id.get()],
        )
        .unwrap();
        conn.execute(
            "UPDATE member_control SET create_state='resolved' WHERE membership_id=?1",
            [member.membership_id.get()],
        )
        .unwrap();
        state.native.channels.lock().unwrap().insert(native,json!({"id":native,"name":format!("source-{i}"),"type":8,"group":"managed","base_url":format!("https://source-{i}.invalid/v1"),"models":format!("model-{i}"),"priority":100-i as i64,"status":status,"auto_ban":if kind==SourceKind::Glm {1}else{0},"channel_info":{"is_multi_key":false}}));
        store
            .submit_operation(
                operation(i),
                ControlCommand::EnsureMember {
                    membership_id: member.membership_id,
                    expected_revision: 1,
                    status: StatusDirective::Enabled,
                },
                now(),
            )
            .unwrap();
    }
    store.set_group_ready(group, 1, true, now()).unwrap();
    let budget = ManagementBudget::new();
    let config = fake.config(!session);
    let admin = Arc::new(NewApiClient::with_budget(&config, budget.clone()).unwrap());
    let sign = issuer.then(|| {
        let mut config = config.clone();
        config.root_username = "issuer".into();
        (
            Arc::new(NewApiClient::with_budget(&config, budget).unwrap()),
            41,
        )
    });
    let owners = TokenOwners::new(sign, legacy.then_some(1)).unwrap();
    let worker =
        ReconcileWorker::new_with_token_owners(store, admin, policy_config(), owners).unwrap();
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
fn token_op(n: usize) -> OperationId {
    OperationId::new(format!("service-token-{n:016}")).unwrap()
}
fn create(f: &TokenFixture, n: usize) -> OperationView {
    let revision = f.worker.snapshot().unwrap().groups[0].revision;
    f.worker
        .create_token(token_op(n), f.group, revision, "client key".into(), None)
        .unwrap()
}
fn puts(f: &TokenFixture, path: &str) -> usize {
    f.fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.method != "GET" && r.path == path)
        .count()
}
async fn token_step(f: &TokenFixture, id: &OperationId) -> ControlResult<bool> {
    let _lane = f.worker.lane.lock().await;
    f.worker.token_step(id, &AtomicBool::new(false)).await
}
async fn drive_case(future: impl std::future::Future<Output = ()> + Send + 'static) {
    let task = tokio::spawn(future);
    for _ in 0..250_000 {
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
    panic!("bounded token worker scenario did not finish");
}

#[tokio::test(start_paused = true)]
async fn t5c_positive_admission_uses_later_live_candidate() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[2, 1], true, true).await;
        create(&f, 0);
        token_step(&f, &token_op(0)).await.unwrap();
        assert_eq!(puts(&f, "/api/token/"), 1);
        assert_eq!(
            f.worker.operation(&token_op(0)).unwrap().phase,
            OperationPhase::Waiting
        );
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_worker_idempotency_conflict_precedes_missing_target_and_owner() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let old = create(&f, 0);
        let wrong = ManagedTokenId::new(9999).unwrap();
        assert_eq!(
            f.worker
                .delete_token(token_op(0), wrong, 1)
                .unwrap_err()
                .code,
            ControlErrorCode::IdempotencyConflict
        );
        assert_eq!(f.worker.operation(&token_op(0)).unwrap(), old);
        assert_eq!(puts(&f, "/api/token/"), 0);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_group_configuration_does_not_confuse_row_age_with_drift() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Glm, &[1], true, true).await;
        let work = f.worker.work(f.members[0]).unwrap();
        let mut observed = f
            .worker
            .client
            .managed_channels()
            .get_channel(1)
            .await
            .unwrap()
            .observation()
            .unwrap();
        observed.observed_at = SystemTime::now() - Duration::from_secs(31);
        assert!(!fresh(&observed, now()));
        ReconcileWorker::validate_group_configuration(&[work], &HashMap::from([(1, observed)]))
            .unwrap();
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_volcano_auto_ban_is_not_a_ready_configuration() {
    drive_case(async {
        let f = tokens_fixture(SourceKind::Volcano, &[1], true, true).await;
        f.state.native.channels.lock().unwrap().get_mut(&1).unwrap()["auto_ban"] = json!(1);
        let revision = f.worker.snapshot().unwrap().groups[0].revision;
        let result = f
            .worker
            .check_group(f.group, revision, false)
            .await
            .unwrap();
        assert!(!result.ready);
        assert_eq!(result.error_code, Some(ControlErrorCode::BindingDrift));
    })
    .await;
}
