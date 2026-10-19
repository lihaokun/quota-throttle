//! 仅新serve组合使用的额外前提；旧本地预检语义保持不变。
use super::*;

const ENV_LIMIT: u64 = 1024 * 1024;
fn environment_ready(raw: &[u8]) -> bool {
    if raw.len() as u64 > ENV_LIMIT || !raw.ends_with(&[0]) {
        return false;
    }
    let expected: [(&[u8], &[u8]); 3] = [
        (b"SQL_DSN=", b"local"),
        (b"LOG_SQL_DSN=", b"local"),
        (b"ERROR_LOG_ENABLED=", b"true"),
    ];
    expected.iter().all(|(key, value)| {
        let mut values = raw
            .split(|b| *b == 0)
            .filter_map(|entry| entry.strip_prefix(*key));
        values.next() == Some(*value) && values.next().is_none()
    })
}
fn settings_ready(body: &Value) -> bool {
    let Some(rows) = body["data"].as_array().filter(|rows| rows.len() <= 10_000) else {
        return false;
    };
    [
        ("LogConsumeEnabled", "true"),
        ("channel_affinity_setting.enabled", "false"),
    ]
    .iter()
    .all(|(key, value)| {
        let mut values = rows.iter().filter(|row| row["key"] == *key);
        values.next().is_some_and(|row| row["value"] == *value) && values.next().is_none()
    })
}

impl NewApiClient {
    pub(crate) async fn check_runtime_backend(&self, process: &NewApiProcess) -> BackendCheck {
        self.check_runtime_backend_with(process, std::future::ready(()))
            .await
    }
    async fn check_runtime_backend_with<F: std::future::Future<Output = ()> + Send>(
        &self,
        process: &NewApiProcess,
        between: F,
    ) -> BackendCheck {
        // Step R2: 所有证据都只留在检查future；caller的原refresh_from最终一次发布。
        let check = async {
            self.local_target(process)?;
            let pid = read_pid(process).await?;
            let before = process_snapshot(process, pid).await?;
            let backend = self.check_local_backend(process).await;
            if backend.issue().is_some() {
                return Ok(backend);
            }
            between.await;
            if read_pid(process).await? != pid || process_snapshot(process, pid).await? != before {
                return Err(BackendIssue::LocalBinding);
            }
            let raw = tokio::task::spawn_blocking(move || {
                let mut bytes = Vec::new();
                std::fs::File::open(format!("/proc/{pid}/environ"))?
                    .take(ENV_LIMIT + 1)
                    .read_to_end(&mut bytes)?;
                Ok::<_, std::io::Error>(bytes)
            })
            .await
            .map_err(|_| BackendIssue::RuntimePrerequisites)?
            .map_err(|_| BackendIssue::RuntimePrerequisites)?;
            if !environment_ready(&raw) {
                return Err(BackendIssue::RuntimePrerequisites);
            }
            let options = self.preflight_read("/api/option/", LIST_BODY_LIMIT).await?;
            if !settings_ready(&options) {
                return Err(BackendIssue::RuntimePrerequisites);
            }
            if read_pid(process).await? != pid || process_snapshot(process, pid).await? != before {
                return Err(BackendIssue::LocalBinding);
            }
            if !backend.readiness().ready_at(utc_now()?) {
                return Err(BackendIssue::Expired);
            }
            Ok(backend)
        };
        match tokio::time::timeout(CHECK_TIMEOUT, check).await {
            Ok(Ok(check)) => check,
            Ok(Err(issue)) => BackendCheck::failed(issue),
            Err(_) => BackendCheck::failed(BackendIssue::Expired),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn r2_environment_and_route_settings_fail_closed() {
        let valid = b"SQL_DSN=local\0LOG_SQL_DSN=local\0ERROR_LOG_ENABLED=true\0PRIVATE=value\0";
        assert!(environment_ready(valid));
        for invalid in [
            b"SQL_DSN=local\0LOG_SQL_DSN=local\0".as_slice(),
            b"SQL_DSN=local\0LOG_SQL_DSN=local\0ERROR_LOG_ENABLED=false\0",
            b"SQL_DSN=local\0LOG_SQL_DSN=local\0ERROR_LOG_ENABLED=true\0SQL_DSN=remote\0",
            b"SQL_DSN=local\0LOG_SQL_DSN=local\0ERROR_LOG_ENABLED=true",
        ] {
            assert!(!environment_ready(invalid));
        }
        let valid = serde_json::json!({"data":[{"key":"LogConsumeEnabled","value":"true"},
            {"key":"channel_affinity_setting.enabled","value":"false"}]});
        assert!(settings_ready(&valid));
        for (key, value) in [
            ("LogConsumeEnabled", "false"),
            ("channel_affinity_setting.enabled", "true"),
        ] {
            let mut bad = valid.clone();
            for row in bad["data"].as_array_mut().unwrap() {
                if row["key"] == key {
                    row["value"] = value.into();
                }
            }
            assert!(!settings_ready(&bad));
        }
        let mut duplicate = valid.clone();
        duplicate["data"]
            .as_array_mut()
            .unwrap()
            .push(valid["data"][0].clone());
        assert!(!settings_ready(&duplicate));
        assert!(!settings_ready(&serde_json::json!({"data":[]})));
    }

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn start_process(directory: &Path, port: u16, recovery: bool) -> Child {
        // 真正的两个受控进程，只用来注入进程替换；不冒充原生NewAPI组合。
        let program = r#"
import json, os
from http.server import HTTPServer, BaseHTTPRequestHandler
class Server(HTTPServer): allow_reuse_address=True
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args): pass
 def do_GET(self):
  if self.path == '/api/status': data={'version':os.environ['QT_VERSION']}
  elif self.path == '/api/option/': data=[{'key':'AutomaticEnableChannelEnabled','value':os.environ['QT_RECOVERY']},{'key':'LogConsumeEnabled','value':'true'},{'key':'channel_affinity_setting.enabled','value':'false'}]
  else: data={'page':1,'page_size':1,'total':0,'items':[]}
  raw=json.dumps({'success':True,'data':data}).encode()
  self.send_response(200);self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)
Server(('127.0.0.1',int(os.environ['PORT'])),Handler).serve_forever()
"#;
        Child(
            std::process::Command::new("python3")
                .args(["-u", "-c", program])
                .current_dir(directory)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("PORT", port.to_string())
                .env("QT_VERSION", SUPPORTED_BACKEND_VERSION)
                .env("QT_RECOVERY", if recovery { "true" } else { "false" })
                .env("SQL_DSN", "local")
                .env("LOG_SQL_DSN", "local")
                .env("ERROR_LOG_ENABLED", "true")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
    async fn listening(child: &mut Child, port: u16) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                assert!(child.0.try_wait().unwrap().is_none());
                if tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .is_ok()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn r2_two_valid_processes_between_checks_cannot_share_readiness() {
        let dir = tempfile::tempdir().unwrap();
        let reserve = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reserve.local_addr().unwrap().port();
        assert!(![3000, 3001].contains(&port));
        drop(reserve);
        let mut first = start_process(dir.path(), port, false);
        listening(&mut first, port).await;
        let mut cfg: crate::config::ManageConfig = toml::from_str("").unwrap();
        cfg.port = port;
        cfg.data_dir = dir.path().to_string_lossy().into_owned();
        let base = format!("http://127.0.0.1:{port}");
        let mut process = NewApiProcess::new(&cfg, &base).unwrap();
        process.binary = std::fs::read_link(format!("/proc/{}/exe", first.0.id())).unwrap();
        let pid_file = dir.path().join("new-api.pid");
        std::fs::write(&pid_file, first.0.id().to_string()).unwrap();
        let mut api_cfg: crate::config::NewApiConfig = toml::from_str("").unwrap();
        api_cfg.base_url = base;
        api_cfg.admin_token = "synthetic-preflight-token".into();
        let client = NewApiClient::new(&api_cfg).unwrap();
        let mut replacement = None;
        let result = client
            .check_runtime_backend_with(&process, async {
                // A的完整旧预检刚通过；B同exe/cwd/listener且环境正常，但恢复开关不同。
                first.0.kill().unwrap();
                first.0.wait().unwrap();
                let mut second = start_process(dir.path(), port, true);
                listening(&mut second, port).await;
                std::fs::write(&pid_file, second.0.id().to_string()).unwrap();
                replacement = Some(second);
            })
            .await;
        assert_eq!(result.issue(), Some(BackendIssue::LocalBinding));
        assert!(!result.readiness().ready_at(utc_now().unwrap()));
        assert!(replacement.is_some());
    }
}
