//! 显式来源分组运行入口；只组装已验收worker，不启动旧来源写者或旧面板。
use crate::boot::NewApiProcess;
use crate::catalog::control::ControlErrorCode;
use crate::catalog::CatalogStore;
use crate::newapi::transport::ManagementBudget;
use crate::newapi::NewApiClient;
use crate::panel_http::PanelServer;
use crate::reconcile::worker::{token_service::TokenOwners, ReconcileWorker};
use std::fmt;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::{info, warn};

mod config;
#[cfg(test)]
mod tests;

const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5);
const BACKEND_INTERVAL: Duration = Duration::from_secs(30);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeError {
    Config,
    PrivatePath,
    Panel,
    ManagedProcess,
    Prerequisites,
    Storage,
    Background,
    Signal,
}
impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "source-groups runtime: {self:?}")
    }
}
impl std::error::Error for RuntimeError {}
type RuntimeResult<T> = Result<T, RuntimeError>;

async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() || shutdown.changed().await.is_err() {
            return;
        }
    }
}
async fn delay(shutdown: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    tokio::select! { biased; _ = stopped(shutdown) => false, _ = tokio::time::sleep(duration) => true }
}
async fn owned_job<T, F>(mut shutdown: watch::Receiver<bool>, job: F) -> RuntimeResult<Option<T>>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    if *shutdown.borrow() {
        return Ok(None);
    }
    let runtime = tokio::runtime::Handle::current();
    // 实际同步事务在阻塞线程上执行，stop只能在await处取消；永远等待真实句柄。
    tokio::task::spawn_blocking(move || {
        runtime.block_on(async move {
        tokio::select! { biased; _ = stopped(&mut shutdown) => None, result = job => Some(result) }
    })
    })
    .await
    .map_err(|_| RuntimeError::Background)
}
fn control<T>(result: crate::catalog::control::ControlResult<T>) -> RuntimeResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.code == ControlErrorCode::Storage => Err(RuntimeError::Storage),
        Err(e) => {
            warn!(code = ?e.code, "来源分组维护暂未完成");
            Ok(None)
        }
    }
}
fn repair_outcome<T>(result: crate::ledger::analytics::AnalyticsResult<T>) -> RuntimeResult<()> {
    use crate::catalog::ledger::LedgerError;
    use crate::ledger::analytics::AnalyticsError;
    match result {
        Ok(_)
        | Err(AnalyticsError::Busy | AnalyticsError::Cancelled | AnalyticsError::TimedOut)
        | Err(AnalyticsError::Ledger(LedgerError::NotInitialized))
        | Err(AnalyticsError::Control(
            ControlErrorCode::QueueFull | ControlErrorCode::RevisionConflict,
        )) => Ok(()),
        Err(_) => Err(RuntimeError::Storage),
    }
}
async fn glm_loop(
    worker: Arc<ReconcileWorker>,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> RuntimeResult<()> {
    // Step R4: 单轮来源采样，与火山/预检独立；每轮结束再等待，绝不积压tick。
    loop {
        let current = worker.clone();
        let Some(result) = owned_job(shutdown.clone(), async move {
            current.refresh_glm_policy().await
        })
        .await?
        else {
            return Ok(());
        };
        control(result)?;
        if !delay(&mut shutdown, interval).await {
            return Ok(());
        }
    }
}
async fn maintenance(
    worker: Arc<ReconcileWorker>,
    process: Arc<NewApiProcess>,
    mut shutdown: watch::Receiver<bool>,
) -> RuntimeResult<()> {
    // Step R5: 合并预检完成后才发布ready；每个真实job前后均响应停止。
    let mut backend_due = Instant::now() + BACKEND_INTERVAL;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        if Instant::now() >= backend_due {
            let (current, native) = (worker.clone(), process.clone());
            let Some(result) = owned_job(shutdown.clone(), async move {
                current.refresh_runtime_backend(&native).await
            })
            .await?
            else {
                return Ok(());
            };
            let ready = control(result)?.is_some_and(|check| check.issue().is_none());
            backend_due = Instant::now()
                + if ready {
                    BACKEND_INTERVAL
                } else {
                    MAINTENANCE_INTERVAL
                };
        }
        let current = worker.clone();
        let Some(result) = owned_job(shutdown.clone(), async move {
            current.refresh_volcano_policy().await
        })
        .await?
        else {
            return Ok(());
        };
        control(result)?;
        let current = worker.clone();
        let Some(result) = owned_job(shutdown.clone(), async move {
            current.repair_attribution_once().await
        })
        .await?
        else {
            return Ok(());
        };
        repair_outcome(result)?;
        if !delay(&mut shutdown, MAINTENANCE_INTERVAL).await {
            return Ok(());
        }
    }
}

async fn start(
    loaded: config::Loaded,
    shutdown: watch::Receiver<bool>,
) -> RuntimeResult<Option<JoinSet<RuntimeResult<()>>>> {
    // Step R2: 先取得唯一面板端口和catalog锁，再进入本次托管运行组合。
    if *shutdown.borrow() {
        return Ok(None);
    }
    let panel = PanelServer::bind(loaded.addr, loaded.options.panel_origin.as_deref())
        .await
        .map_err(|_| RuntimeError::Panel)?;
    let manage = loaded
        .policy
        .new_api
        .manage
        .as_ref()
        .ok_or(RuntimeError::Config)?
        .clone();
    let catalog_path = loaded.options.catalog_path.clone();
    let native_dir = std::path::PathBuf::from(&manage.data_dir);
    let Some(store) = owned_job(shutdown.clone(), async move {
        config::private_directory(&native_dir)?;
        CatalogStore::open(&catalog_path).map_err(|_| RuntimeError::Storage)
    })
    .await?
    else {
        return Ok(None);
    };
    let store = store?;
    let process = Arc::new(
        NewApiProcess::new(&manage, &loaded.policy.new_api.base_url)
            .map_err(|_| RuntimeError::Config)?,
    );
    if *shutdown.borrow() {
        return Ok(None);
    }
    let (native, stop) = (process.clone(), shutdown.clone());
    let Some(started) = owned_job(shutdown.clone(), async move {
        tokio::time::timeout(Duration::from_secs(180), native.ensure_group_running(&stop)).await
    })
    .await?
    else {
        return Ok(None);
    };
    started
        .map_err(|_| RuntimeError::ManagedProcess)?
        .map_err(|_| RuntimeError::ManagedProcess)?;
    if *shutdown.borrow() {
        return Ok(None);
    }
    let budget = ManagementBudget::new();
    let admin = Arc::new(
        NewApiClient::with_budget(&loaded.policy.new_api, budget.clone())
            .map_err(|_| RuntimeError::Config)?,
    );
    let issuer = loaded
        .options
        .issuer
        .map(|issuer| {
            let mut cfg = loaded.policy.new_api.clone();
            cfg.admin_token.clear();
            cfg.extra_headers.clear();
            cfg.manage = None;
            cfg.channel_template = None;
            cfg.root_username = issuer.username;
            cfg.root_password = issuer.password;
            NewApiClient::with_budget(&cfg, budget)
                .map(|client| (Arc::new(client), issuer.user_id))
                .map_err(|_| RuntimeError::Config)
        })
        .transpose()?;
    let worker = if issuer.is_some() || loaded.options.legacy_admin_user_id.is_some() {
        let owners = TokenOwners::new(issuer, loaded.options.legacy_admin_user_id)
            .map_err(|_| RuntimeError::Config)?;
        ReconcileWorker::new_with_token_owners(store, admin, loaded.policy.clone(), owners)
    } else {
        ReconcileWorker::new(store, admin, loaded.policy.clone())
    }
    .map_err(|_| RuntimeError::Storage)?;
    let (current, native) = (worker.clone(), process.clone());
    let Some(check) = owned_job(shutdown.clone(), async move {
        current.refresh_runtime_backend(&native).await
    })
    .await?
    else {
        return Ok(None);
    };
    if check.map_err(|_| RuntimeError::Storage)?.issue().is_some() {
        return Err(RuntimeError::Prerequisites);
    }
    if *shutdown.borrow() {
        return Ok(None);
    }
    let mut tasks = JoinSet::new();
    let (current, stop) = (worker.clone(), shutdown.clone());
    tasks.spawn(async move { current.run(stop).await.map_err(|_| RuntimeError::Storage) });
    let (current, native, stop) = (worker.clone(), process.clone(), shutdown.clone());
    tasks.spawn(async move {
        current
            .run_fact_collection(&native, stop)
            .await
            .map_err(|_| RuntimeError::Storage)
    });
    tasks.spawn(glm_loop(
        worker.clone(),
        Duration::from_secs(loaded.policy.poll_interval_secs),
        shutdown.clone(),
    ));
    tasks.spawn(maintenance(worker.clone(), process, shutdown.clone()));
    tasks.spawn(async move {
        panel
            .serve_ui(worker, loaded.ui, shutdown)
            .await
            .map_err(|_| RuntimeError::Panel)
    });
    info!(address = %loaded.addr, dry_run = loaded.policy.dry_run, "来源分组服务已启动；上游与访问Key由三页面管理");
    Ok(Some(tasks))
}

async fn supervise<F>(
    mut tasks: JoinSet<RuntimeResult<()>>,
    stop: watch::Sender<bool>,
    signal: F,
) -> RuntimeResult<()>
where
    F: Future<Output = RuntimeResult<()>>,
{
    // Step R3: 任何任务提前结束都广播停止，然后等待其余真实工作；不abort丢句柄。
    let mut result = tokio::select! {
        biased;
        result = signal => result,
        task = tasks.join_next() => match task {
            Some(Ok(Err(error))) => Err(error),
            _ => Err(RuntimeError::Background),
        },
    };
    stop.send_replace(true);
    while let Some(task) = tasks.join_next().await {
        if result.is_ok() {
            result = task.unwrap_or(Err(RuntimeError::Background));
        }
    }
    result
}
pub(crate) async fn run_path(path: &str) -> RuntimeResult<()> {
    let loaded = config::load(Path::new(path))?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| RuntimeError::Signal)?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|_| RuntimeError::Signal)?;
    let signal = async move {
        tokio::select! { _=terminate.recv()=>{}, _=interrupt.recv()=>{} };
        Ok(())
    };
    let (stop, shutdown) = watch::channel(false);
    supervise_startup(start(loaded, shutdown), stop, signal).await
}
async fn supervise_startup<F, S>(
    startup: F,
    stop: watch::Sender<bool>,
    signal: S,
) -> RuntimeResult<()>
where
    F: Future<Output = RuntimeResult<Option<JoinSet<RuntimeResult<()>>>>>,
    S: Future<Output = RuntimeResult<()>>,
{
    tokio::pin!(signal);
    tokio::pin!(startup);
    let started = tokio::select! {
        biased;
        result = &mut signal => {
            stop.send_replace(true);
            return match startup.await? {
                Some(tasks) => supervise(tasks, stop, std::future::ready(result)).await,
                None => result,
            };
        },
        result = &mut startup => result?,
    };
    if let Some(tasks) = started {
        supervise(tasks, stop, signal).await
    } else {
        Ok(())
    }
}
