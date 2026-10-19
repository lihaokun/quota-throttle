//! 连接只持等待端；唯一serve supervisor拥有handoff和全部实际业务句柄。
use super::input::AdminRequest;
use super::{dispatch, input, output, route, AdminError, AdminFailure, ExtraCode};
use crate::catalog::control::ControlErrorCode;
use crate::panel_http::request::RequestHead;
use crate::panel_http::{HttpErrorCode, Response, WRITE_TIMEOUT};
use crate::reconcile::worker::ReconcileWorker;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};
use tokio::task::{JoinError, JoinSet};
use tokio::time::{timeout_at, Instant};

pub(in crate::panel_http) const MAX_BUSINESS: usize = 4;

#[derive(Clone)]
pub(in crate::panel_http) struct Client {
    sender: mpsc::Sender<Pending>,
    slots: Arc<Semaphore>,
    closing: Arc<AtomicBool>,
    dry_run: bool,
}
pub(in crate::panel_http) struct Pending {
    request: AdminRequest,
    reply: oneshot::Sender<Response>,
    permit: OwnedSemaphorePermit,
    deadline: Instant,
}
pub(in crate::panel_http) struct JobOutcome {
    reply: oneshot::Sender<Response>,
    response: Response,
    _permit: OwnedSemaphorePermit,
}
pub(in crate::panel_http) struct Supervisor {
    worker: Arc<ReconcileWorker>,
    closing: Arc<AtomicBool>,
    pub receiver: mpsc::Receiver<Pending>,
    pub jobs: JoinSet<JobOutcome>,
}
fn unconfirmed() -> Response {
    AdminFailure::Business(AdminError::extra(ExtraCode::ResultUnconfirmed)).response()
}
fn full() -> Response {
    AdminFailure::Business(AdminError::control(ControlErrorCode::QueueFull)).response()
}
fn refusal(closing: &AtomicBool, deadline: Instant) -> Option<Response> {
    if Instant::now() >= deadline {
        Some(unconfirmed())
    } else if closing.load(Ordering::Acquire) {
        Some(full())
    } else {
        None
    }
}
impl Client {
    pub async fn call(
        &self,
        head: &RequestHead,
        body: Vec<u8>,
        connection_end: Instant,
    ) -> Response {
        let entered_at = Instant::now();
        let decoded = route::parse(&head.method, &head.path, head.query.as_deref())
            .and_then(|route| input::decode(route, &body));
        drop(body);
        match decoded {
            Err(error) => error.response(),
            Ok(request) => self.admit(request, entered_at, connection_end).await,
        }
    }
    async fn admit(
        &self,
        request: AdminRequest,
        entered_at: Instant,
        connection_end: Instant,
    ) -> Response {
        // Step H2.2: 一个permit跨handoff、blocking排队/执行/编码至结果回收；超时只丢等待端。
        if self.dry_run && request.writes() {
            return AdminFailure::Business(dispatch::dry_run_error()).response();
        }
        let deadline = (entered_at + request.budget()).min(connection_end - WRITE_TIMEOUT);
        if let Some(response) = refusal(&self.closing, deadline) {
            return response;
        }
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            return full();
        };
        let (reply, receiver) = oneshot::channel();
        if let Some(response) = refusal(&self.closing, deadline) {
            return response;
        }
        if self
            .sender
            .try_send(Pending {
                request,
                reply,
                permit,
                deadline,
            })
            .is_err()
        {
            return full();
        }
        let result = timeout_at(deadline, receiver).await;
        // Ready恰好在deadline也不能回成功；关闭sender不等同业务回滚。
        if Instant::now() >= deadline {
            return unconfirmed();
        }
        match result {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Response::error(HttpErrorCode::Internal),
            Err(_) => unconfirmed(),
        }
    }
}
impl Supervisor {
    pub fn new(worker: Arc<ReconcileWorker>) -> (Client, Self) {
        let (sender, receiver) = mpsc::channel(MAX_BUSINESS);
        let closing = Arc::new(AtomicBool::new(false));
        let client = Client {
            sender,
            slots: Arc::new(Semaphore::new(MAX_BUSINESS)),
            closing: closing.clone(),
            dry_run: worker.is_dry_run(),
        };
        (
            client,
            Self {
                worker,
                closing,
                receiver,
                jobs: JoinSet::new(),
            },
        )
    }
    pub fn can_start(&self) -> bool {
        self.jobs.len() < MAX_BUSINESS
    }
    pub fn start(&mut self, pending: Pending) {
        if pending.reply.is_closed() {
            return;
        }
        if let Some(response) = refusal(&self.closing, pending.deadline) {
            let _ = pending.reply.send(response);
            return;
        }
        let worker = self.worker.clone();
        let closing = self.closing.clone();
        let runtime = Handle::current();
        self.jobs.spawn_blocking(move || {
            let Pending {
                request,
                reply,
                permit,
                deadline,
            } = pending;
            // 进入原runtime上下文后检查（包括测试虚拟时钟），紧邻真正callee；其后不abort。
            let response = runtime.block_on(async {
                if reply.is_closed() {
                    return unconfirmed();
                }
                if let Some(response) = refusal(&closing, deadline) {
                    return response;
                }
                match dispatch::dispatch(&worker, request)
                    .await
                    .and_then(output::encode)
                {
                    Ok(response) => response,
                    Err(error) => AdminFailure::Business(error).response(),
                }
            });
            JobOutcome {
                reply,
                response,
                _permit: permit,
            }
        });
    }
    pub fn finish(result: Option<Result<JobOutcome, JoinError>>) -> bool {
        match result {
            Some(Ok(outcome)) => {
                let _ = outcome.reply.send(outcome.response);
                false
            }
            Some(Err(_)) => {
                tracing::debug!("panel_business_failed");
                true
            }
            None => false,
        }
    }
    pub fn close(&mut self) {
        self.closing.store(true, Ordering::Release);
        self.receiver.close();
        while self.receiver.try_recv().is_ok() {}
    }
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
