//! new-api 原生进程托管：按平台下载 release 二进制、sha256 校验、detached 启动、健康检查、停止。
//!
//! 不用容器：new-api 就是一个 Go 单二进制 + 内置 SQLite。我们把它下到 data_dir 里跑，
//! 进程放进独立进程组（不与本守护父子耦合），PID 落盘，便于 `down` 停。

use crate::config::ManageConfig;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tracing::{info, warn};

/// 平台对应的 release 资产名与 checksums 文件名。
fn asset_names(version: &str) -> Result<(String, &'static str)> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let (asset, checksums) = match (os, arch) {
        ("linux", "x86_64") => (format!("new-api-{version}"), "checksums-linux.txt"),
        ("linux", "aarch64") => (format!("new-api-arm64-{version}"), "checksums-linux.txt"),
        ("macos", _) => (format!("new-api-macos-{version}"), "checksums-macos.txt"),
        _ => bail!("不支持的平台 {os}/{arch}：请手动部署 new-api 或改用 run 子命令"),
    };
    Ok((asset, checksums))
}

pub struct NewApiProcess {
    pub data_dir: PathBuf,
    pub binary: PathBuf,
    pub port: u16,
    pub base_url: String,
    repo: String,
    version: String,
    client: reqwest::Client,
}

impl NewApiProcess {
    pub fn new(cfg: &ManageConfig, base_url: &str) -> Result<Self> {
        let (asset, _) = asset_names(&cfg.version)?;
        let data_dir = PathBuf::from(&cfg.data_dir);
        Ok(Self {
            binary: data_dir.join(&asset),
            data_dir,
            port: cfg.port,
            base_url: base_url.trim_end_matches('/').to_string(),
            repo: cfg.repo.clone(),
            version: cfg.version.clone(),
            client: reqwest::Client::new(),
        })
    }

    fn pid_file(&self) -> PathBuf {
        self.data_dir.join("new-api.pid")
    }

    /// 确保二进制存在：不在就下载并校验 sha256。
    pub async fn ensure_binary(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("创建 data_dir 失败: {}", self.data_dir.display()))?;
        if self.binary.exists() {
            info!(binary = %self.binary.display(), "new-api 二进制已存在，跳过下载");
            return Ok(());
        }

        let (asset, checksums_file) = asset_names(&self.version)?;
        let base = format!(
            "https://github.com/{}/releases/download/{}",
            self.repo, self.version
        );

        // 先取期望 sha256
        let checksums_url = format!("{base}/{checksums_file}");
        info!(url = %checksums_url, "下载 checksums");
        let checksums = self
            .client
            .get(&checksums_url)
            .send()
            .await
            .context("下载 checksums 失败")?
            .error_for_status()
            .context("checksums 响应非 2xx")?
            .text()
            .await?;
        let want = checksums
            .lines()
            .find_map(|l| {
                let mut it = l.split_whitespace();
                let sha = it.next()?;
                let name = it.next()?;
                (name == asset).then(|| sha.to_string())
            })
            .with_context(|| format!("checksums 里找不到 {asset}"))?;

        // 下载二进制（约 130MB，一次性 buffer）
        let bin_url = format!("{base}/{asset}");
        info!(url = %bin_url, "下载 new-api 二进制（约 130MB，稍等）");
        let bytes = self
            .client
            .get(&bin_url)
            .send()
            .await
            .context("下载 new-api 二进制失败")?
            .error_for_status()
            .context("二进制响应非 2xx")?
            .bytes()
            .await
            .context("读取二进制响应体失败")?;

        let got = hex(&Sha256::digest(&bytes));
        if got != want {
            bail!("sha256 校验不通过: 期望 {want} 实得 {got}");
        }
        info!("sha256 校验通过");

        let tmp = self.binary.with_extension("part");
        std::fs::write(&tmp, &bytes).context("写入二进制失败")?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .context("chmod 失败")?;
        std::fs::rename(&tmp, &self.binary).context("重命名二进制失败")?;
        info!(binary = %self.binary.display(), "new-api 就绪");
        Ok(())
    }

    /// GET {base}/api/status，200 即视为健康。
    pub async fn is_healthy(&self) -> bool {
        let url = format!("{}/api/status", self.base_url);
        matches!(
            self.client
                .get(&url)
                .timeout(Duration::from_secs(3))
                .send()
                .await,
            Ok(r) if r.status().is_success()
        )
    }

    /// 确保 new-api 在跑：已健康则直接返回；否则 detached 启动并等到健康。
    pub async fn ensure_running(&self) -> Result<()> {
        if self.is_healthy().await {
            info!(base = %self.base_url, "new-api 已在运行");
            return Ok(());
        }
        self.ensure_binary().await?;

        let log_path = self.data_dir.join("new-api.log");
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .context("打开 new-api 日志失败")?;
        let log_err = log.try_clone()?;

        // 绝对路径启动，工作目录设为 data_dir，让 SQLite/日志都落这里。
        let binary_abs = std::fs::canonicalize(&self.binary).context("解析二进制绝对路径失败")?;
        let data_abs = std::fs::canonicalize(&self.data_dir)?;

        let child = Command::new(&binary_abs)
            .current_dir(&data_abs)
            .env("PORT", self.port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .process_group(0) // 独立进程组，免得本守护收 Ctrl-C 把它带走
            .spawn()
            .with_context(|| format!("启动 new-api 失败: {}", binary_abs.display()))?;

        let pid = child.id();
        std::fs::write(self.pid_file(), pid.to_string()).ok();
        info!(pid, port = self.port, log = %log_path.display(), "已拉起 new-api，等待健康…");

        for i in 1..=60 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if self.is_healthy().await {
                info!(base = %self.base_url, secs = i, "new-api 健康");
                return Ok(());
            }
        }
        bail!(
            "new-api 启动后 60s 内未就绪，看看 {} 里的日志",
            log_path.display()
        );
    }

    /// 停止托管的 new-api（读 PID 文件发 SIGTERM）。
    pub fn stop(&self) -> Result<()> {
        let pf = self.pid_file();
        let pid: i32 = match std::fs::read_to_string(&pf) {
            Ok(s) => s.trim().parse().context("PID 文件内容非法")?,
            Err(_) => {
                warn!("没有 PID 文件，new-api 可能不是本工具起的，跳过");
                return Ok(());
            }
        };
        // SIGTERM（用 /bin/kill，省得引入 libc crate）
        let ok = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            info!(pid, "已向 new-api 发送 SIGTERM");
        } else {
            warn!(pid, "kill 失败，可能进程已退出");
        }
        std::fs::remove_file(&pf).ok();
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
