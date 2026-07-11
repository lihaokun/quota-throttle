mod boot;
mod config;
mod newapi;
mod orchestrator;
mod quota;
mod status;

use crate::boot::NewApiProcess;
use crate::config::{Config, ResolvedKey};
use crate::newapi::NewApiClient;
use crate::orchestrator::Orchestrator;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
用法: quota-throttle <子命令> [config.toml]
  up     下载/启动 new-api（若配了 manage）→ sync 建渠道 → 进入切换循环
  sync   （确保 new-api 起着）按 key 列表建/对齐渠道并打印 name→channel_id，不进循环
  run    假设 new-api 已在跑，只解析渠道并进入切换循环
  down   停掉本工具托管的 new-api
省略子命令时按 run 处理。";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let mut args = std::env::args().skip(1);
    let first = args.next();
    let (cmd, cfg_path) = match first.as_deref() {
        Some("up") | Some("down") | Some("sync") | Some("run") => (
            first.clone().unwrap(),
            args.next().unwrap_or_else(|| "config.toml".to_string()),
        ),
        Some("-h") | Some("--help") => {
            println!("{USAGE}");
            return Ok(());
        }
        Some(path) => ("run".to_string(), path.to_string()),
        None => ("run".to_string(), "config.toml".to_string()),
    };

    let cfg = Config::load(&cfg_path).with_context(|| format!("加载配置失败: {cfg_path}"))?;

    match cmd.as_str() {
        "down" => cmd_down(&cfg),
        "sync" => cmd_sync(cfg).await,
        "up" => cmd_up(cfg).await,
        "run" => cmd_run(cfg).await,
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

/// 若配了 manage，就确保 new-api 原生进程在跑（不在则下载+启动）。
async fn ensure_newapi_up(cfg: &Config) -> Result<()> {
    match &cfg.new_api.manage {
        Some(m) => {
            let proc = NewApiProcess::new(m, &cfg.new_api.base_url)?;
            proc.ensure_running().await
        }
        None => {
            // 没配托管：只健康检查，起不起来是用户自己的事
            let url = format!("{}/api/status", cfg.new_api.base_url.trim_end_matches('/'));
            if reqwest::Client::new()
                .get(&url)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
            {
                Ok(())
            } else {
                bail!(
                    "new-api 在 {} 上不可达，且未配 [new_api.manage] 让本工具托管——\
                     请自行启动 new-api，或配置 manage 让本工具下载运行",
                    cfg.new_api.base_url
                )
            }
        }
    }
}

fn cmd_down(cfg: &Config) -> Result<()> {
    match &cfg.new_api.manage {
        Some(m) => {
            let proc = NewApiProcess::new(m, &cfg.new_api.base_url)?;
            proc.stop()
        }
        None => {
            warn!("未配 [new_api.manage]，没有本工具托管的 new-api 可停");
            Ok(())
        }
    }
}

async fn cmd_sync(cfg: Config) -> Result<()> {
    ensure_newapi_up(&cfg).await?;
    let mut api = NewApiClient::new(&cfg.new_api)?;
    api.authenticate().await?;
    let map = api
        .sync_channels(
            &cfg.keys,
            cfg.new_api.channel_template.as_ref(),
            cfg.priority_standby,
        )
        .await?;
    print_mapping(&cfg, &map);
    Ok(())
}

async fn cmd_up(cfg: Config) -> Result<()> {
    ensure_newapi_up(&cfg).await?;
    let mut api = NewApiClient::new(&cfg.new_api)?;
    api.authenticate().await?;
    let map = api
        .sync_channels(
            &cfg.keys,
            cfg.new_api.channel_template.as_ref(),
            cfg.priority_standby,
        )
        .await?;
    let keys = resolve_keys(&cfg, &map);
    run_loop(cfg, api, keys).await
}

async fn cmd_run(cfg: Config) -> Result<()> {
    ensure_newapi_up(&cfg).await?;
    let mut api = NewApiClient::new(&cfg.new_api)?;
    api.authenticate().await?;
    // run 不建渠道，只列出已有的来解析 id
    let map = api.list_channels().await.unwrap_or_default();
    let keys = resolve_keys(&cfg, &map);
    run_loop(cfg, api, keys).await
}

/// 把 config.keys + (name→id 映射) 解析成 orchestrator 用的 ResolvedKey。
/// 优先用 config 里显式写的 channel_id，否则按 name 从映射里取。
fn resolve_keys(cfg: &Config, map: &HashMap<String, i64>) -> Vec<ResolvedKey> {
    let mut out = Vec::new();
    for k in &cfg.keys {
        match k.channel_id.or_else(|| map.get(&k.name).copied()) {
            Some(id) => out.push(ResolvedKey {
                name: k.name.clone(),
                zhipu_api_key: k.zhipu_api_key.clone(),
                channel_id: id,
                quota_headers: k.quota_headers.clone(),
            }),
            None => warn!(name = %k.name, "解析不到 channel_id（既无显式配置也无同名渠道），本 key 跳过"),
        }
    }
    out
}

fn print_mapping(cfg: &Config, map: &HashMap<String, i64>) {
    info!("渠道映射 name → channel_id：");
    for k in &cfg.keys {
        match map.get(&k.name) {
            Some(id) => info!("  {} → {}", k.name, id),
            None => warn!("  {} → (未找到)", k.name),
        }
    }
}

async fn run_loop(cfg: Config, api: NewApiClient, keys: Vec<ResolvedKey>) -> Result<()> {
    if keys.is_empty() {
        bail!("没有可用的 key（channel_id 都解析不到），无法进入切换循环");
    }
    let interval = Duration::from_secs(cfg.poll_interval_secs);
    info!(
        interval_secs = cfg.poll_interval_secs,
        throttle = cfg.throttle_threshold,
        restore = cfg.restore_threshold,
        dry_run = cfg.dry_run,
        keys = keys.len(),
        "进入切换循环（priority 单活动 key 模式）"
    );

    // 状态看板：独立 task，bind 失败只降级（切换循环照常跑）
    let snapshot: status::Shared = Default::default();
    if !cfg.status_addr.trim().is_empty() {
        tokio::spawn(status::serve(cfg.status_addr.clone(), snapshot.clone()));
    }

    let mut orch = Orchestrator::new(cfg, api, keys, snapshot);
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = ticker.tick() => orch.tick().await,
            _ = tokio::signal::ctrl_c() => {
                info!("收到中断信号，退出（托管的 new-api 仍在跑，用 down 停）");
                break;
            }
        }
    }
    Ok(())
}
