mod config;
mod newapi;
mod orchestrator;
mod quota;

use crate::config::Config;
use crate::orchestrator::Orchestrator;
use anyhow::Result;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".to_string());
    let cfg = Config::load(&path)?;
    let interval = Duration::from_secs(cfg.poll_interval_secs);

    tracing::info!(
        config = %path,
        interval_secs = cfg.poll_interval_secs,
        throttle = cfg.throttle_threshold,
        restore = cfg.restore_threshold,
        dry_run = cfg.dry_run,
        keys = cfg.keys.len(),
        "启动智谱用量守护（priority 单活动 key 模式）"
    );

    let mut orch = Orchestrator::new(cfg);
    let mut ticker = tokio::time::interval(interval);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                orch.tick().await;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("收到中断信号，退出");
                break;
            }
        }
    }
    Ok(())
}
