use super::*;

// 受控假进程仅测竞态；真实二进制证据另由 native 门禁取得。
struct LocalFixture {
    dir: tempfile::TempDir,
    child: std::process::Child,
    process: NewApiProcess,
    config: crate::config::NewApiConfig,
}
impl Drop for LocalFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl LocalFixture {
    async fn start(sqlite: bool) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(![3000, 3001].contains(&port));
        drop(listener);
        let program = r#"
import json, os, time
from pathlib import Path
from http.server import HTTPServer, BaseHTTPRequestHandler
class H(BaseHTTPRequestHandler):
 def log_message(self,*args): pass
 def do_GET(self):
  if self.path == '/api/status':
   Path('status-seen').touch()
   while not Path('release').exists(): time.sleep(.01)
  raw=json.dumps({'success':True,'data':{'version':os.environ['QT_VERSION']}}).encode()
  self.send_response(429 if self.path == '/api/cooldown' else 200)
  if self.path == '/api/cooldown': self.send_header('Retry-After','60')
  self.send_header('Content-Length',str(len(raw)));self.end_headers()
  try: self.wfile.write(raw)
  except (BrokenPipeError,ConnectionResetError): pass
HTTPServer(('127.0.0.1',int(os.environ['PORT'])),H).serve_forever()
"#;
        let mut command = std::process::Command::new("python3");
        command
            .args(["-u", "-c", program])
            .current_dir(dir.path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("PORT", port.to_string())
            .env("QT_VERSION", SUPPORTED_BACKEND_VERSION)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if sqlite {
            command.env("SQL_DSN", "local").env("LOG_SQL_DSN", "local");
        }
        let child = command.spawn().unwrap();
        let mut cfg: crate::config::ManageConfig = toml::from_str("").unwrap();
        cfg.port = port;
        cfg.data_dir = dir.path().to_string_lossy().into();
        let mut config: crate::config::NewApiConfig = toml::from_str("").unwrap();
        config.base_url = format!("http://127.0.0.1:{port}");
        config.admin_token = "fake-token".into();
        let process = NewApiProcess::new(&cfg, &config.base_url).unwrap();
        let mut result = Self {
            dir,
            child,
            process,
            config,
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                assert!(result.child.try_wait().unwrap().is_none());
                if tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        result.process.binary =
            std::fs::read_link(format!("/proc/{}/exe", result.child.id())).unwrap();
        std::fs::write(
            result.dir.path().join("new-api.pid"),
            result.child.id().to_string(),
        )
        .unwrap();
        result
    }
}
async fn wait_for(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn log_capability_real_producer_rejects_missing_environment_and_mid_read_pid_change() {
    let f = LocalFixture::start(false).await;
    let client = NewApiClient::new(&f.config).unwrap();
    assert_eq!(
        client.check_local_logs(&f.process).await.err(),
        Some(LogCapabilityIssue::SqliteUnverified)
    );
    assert!(!f.dir.path().join("status-seen").exists());
    drop(f);
    let f = LocalFixture::start(true).await;
    let client = NewApiClient::new(&f.config).unwrap();
    let change = async {
        wait_for(|| f.dir.path().join("status-seen").exists()).await;
        std::fs::write(f.dir.path().join("new-api.pid"), u32::MAX.to_string()).unwrap();
        std::fs::write(f.dir.path().join("release"), b"").unwrap();
    };
    let (checked, ()) = tokio::join!(client.check_local_logs(&f.process), change);
    assert_eq!(checked.err(), Some(LogCapabilityIssue::LocalBinding));
}
#[tokio::test]
async fn log_capability_total_deadline_cancels_budget_wait_before_status_send() {
    use crate::newapi::transport::{tests::queued_requests, ManagementBudget};
    let f = LocalFixture::start(true).await;
    let budget = ManagementBudget::new();
    let client = NewApiClient::with_budget(&f.config, budget.clone()).unwrap();
    let request = ManagementRequest::new(
        Method::GET,
        "/api/cooldown",
        None,
        RequestSemantics::Read,
        SINGLE_BODY_LIMIT,
    )
    .unwrap();
    assert_eq!(
        client
            .transport
            .send(request, RequestClass::Observe)
            .await
            .err()
            .unwrap()
            .status,
        Some(429)
    );
    let clock = async {
        wait_for(|| queued_requests(&budget) > 0).await;
        tokio::time::pause();
        tokio::time::advance(CHECK_TIMEOUT).await;
        tokio::time::resume();
    };
    let (checked, ()) = tokio::join!(client.check_local_logs(&f.process), clock);
    assert_eq!(checked.err(), Some(LogCapabilityIssue::Expired));
    assert_eq!(queued_requests(&budget), 0);
    assert!(!f.dir.path().join("status-seen").exists());
}
