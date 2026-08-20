mod boot;
mod config;
mod newapi;
mod orchestrator;
mod quota;
mod status;

use crate::boot::NewApiProcess;
use crate::config::{Config, ResolvedKey};
use crate::newapi::{NewApiClient, SyncOutcome};
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
    let outcome = api
        .sync_channels(
            &cfg.keys,
            cfg.new_api.channel_template.as_ref(),
            cfg.new_api.channel_template_claude.as_ref(),
            cfg.priority_standby,
        )
        .await?;
    setup_claude(&api, &cfg).await;
    print_mapping(&cfg, &outcome);
    Ok(())
}

async fn cmd_up(cfg: Config) -> Result<()> {
    ensure_newapi_up(&cfg).await?;
    let mut api = NewApiClient::new(&cfg.new_api)?;
    api.authenticate().await?;
    let outcome = api
        .sync_channels(
            &cfg.keys,
            cfg.new_api.channel_template.as_ref(),
            cfg.new_api.channel_template_claude.as_ref(),
            cfg.priority_standby,
        )
        .await?;
    setup_claude(&api, &cfg).await;
    let keys = resolve_keys(&cfg, &outcome.primary, &outcome.claude);
    run_loop(cfg, api, keys).await
}

async fn cmd_run(cfg: Config) -> Result<()> {
    ensure_newapi_up(&cfg).await?;
    let mut api = NewApiClient::new(&cfg.new_api)?;
    api.authenticate().await?;
    // run 不建渠道，只列出已有的来解析 id
    let map = api.list_channels().await.unwrap_or_default();
    let claude = claude_map(&cfg, &map);
    let keys = resolve_keys(&cfg, &map, &claude);
    run_loop(cfg, api, keys).await
}

/// claude 铺设（配了模板才有）：注册分组 + 确保 CC 专用令牌 + 打印接入说明。
/// **所有失败 warn 降级，绝不阻断**（细化设计 §3）——最坏情况 CC 侧 401，
/// opencode 侧与切换循环完全无感。
async fn setup_claude(api: &NewApiClient, cfg: &Config) {
    let Some(tpl) = cfg.new_api.channel_template_claude.as_ref() else {
        return;
    };
    if let Err(e) = api.ensure_group(&tpl.group).await {
        warn!(
            group = %tpl.group,
            error = %e,
            "注册分组失败：⚠️ CC 请求会 403「无权访问分组」。请到 new-api UI（设置→分组）\
             手动把该分组加入「用户可用分组」和「分组倍率」后重新 sync"
        );
    }
    let base = cfg.new_api.base_url.trim_end_matches('/');
    match api.ensure_claude_token(&tpl.token_name, &tpl.group).await {
        Ok(key) => {
            info!("Claude Code 接入（只改这两个 env；其余如 ANTHROPIC_DEFAULT_*_MODEL 与你现在的直连配置完全一致）：");
            info!("  ANTHROPIC_BASE_URL={base}");
            info!("  ANTHROPIC_AUTH_TOKEN={key}");
        }
        Err(e) => {
            warn!(error = %e, "获取 Claude Code 专用令牌失败");
            info!("Claude Code 接入：");
            info!("  ANTHROPIC_BASE_URL={base}");
            info!("  ANTHROPIC_AUTH_TOKEN=<请到 new-api UI 手动创建/复制令牌 {}（分组 {}）>",
                tpl.token_name, tpl.group);
        }
    }
}

/// 由「渠道名→id」全集 + claude 模板，挑出各 key 的 -cc 渠道映射（key 名 → id）。
/// run 子命令用（up/sync 用 SyncOutcome，不重复解析）。
fn claude_map(cfg: &Config, all: &HashMap<String, i64>) -> HashMap<String, i64> {
    let Some(tpl) = cfg.new_api.channel_template_claude.as_ref() else {
        return HashMap::new();
    };
    cfg.keys
        .iter()
        .filter_map(|k| {
            let cc = tpl.channel_name(&k.name);
            match all.get(&cc) {
                Some(id) => Some((k.name.clone(), *id)),
                None => {
                    warn!(name = %k.name, channel = %cc, "claude 渠道未建（先跑 sync），该 key 的 Claude Code 侧缺席");
                    None
                }
            }
        })
        .collect()
}

/// 把 config.keys + 两侧 (name→id 映射) 解析成 orchestrator 用的 ResolvedKey。
/// 优先用 config 里显式写的 channel_id，否则按 name 从映射里取；
/// claude 侧缺项静默 None（sync/claude_map 阶段已 warn 过，不刷屏）。
fn resolve_keys(
    cfg: &Config,
    primary: &HashMap<String, i64>,
    claude: &HashMap<String, i64>,
) -> Vec<ResolvedKey> {
    let mut out = Vec::new();
    for k in &cfg.keys {
        match k.channel_id.or_else(|| primary.get(&k.name).copied()) {
            Some(id) => out.push(ResolvedKey {
                name: k.name.clone(),
                zhipu_api_key: k.zhipu_api_key.clone(),
                channel_id: id,
                claude_channel_id: claude.get(&k.name).copied(),
                quota_headers: k.quota_headers.clone(),
            }),
            None => warn!(name = %k.name, "解析不到 channel_id（既无显式配置也无同名渠道），本 key 跳过"),
        }
    }
    out
}

fn print_mapping(cfg: &Config, outcome: &SyncOutcome) {
    info!("渠道映射 name → channel_id（cc = Claude Code 侧）：");
    for k in &cfg.keys {
        match outcome.primary.get(&k.name) {
            Some(id) => {
                let cc = outcome
                    .claude
                    .get(&k.name)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "未建".to_string());
                info!("  {} → {} (cc: {})", k.name, id, cc);
            }
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

    let api = std::sync::Arc::new(api);

    // 看板 → 控制循环的命令通道（pin 等写操作）。有界(8)：满了 HTTP 侧直接 503，
    // 既不阻塞看板也不阻塞控制循环。
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(8);

    // 状态看板：独立 task，bind 失败只降级（切换循环照常跑）。
    // 持有 api 是为了 /api/usage 能按需查任意区间的用量（不进 5 秒快照）。
    let snapshot: status::Shared = Default::default();
    if !cfg.status_addr.trim().is_empty() {
        tokio::spawn(status::serve(
            cfg.status_addr.clone(),
            snapshot.clone(),
            api.clone(),
            cmd_tx,
        ));
    }

    // 面板循环：独立 task、独立频率（本地 new-api 可高频；智谱是外部 API 该低频）。
    // 只写面板字段，与切换循环的决策字段严格不相交。
    tokio::spawn(
        orchestrator::Panel {
            api: api.clone(),
            snapshot: snapshot.clone(),
        }
        .run(Duration::from_secs(cfg.panel_interval_secs.max(1))),
    );

    let mut orch = Orchestrator::new(cfg, api, keys, snapshot);
    let mut ticker = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = ticker.tick() => orch.tick().await,
            // 看板命令：先落地（tick 下发 priority + 发布快照），**再回执**。
            // 于是「HTTP 200 返回」⇔「/api/status 已能看到这次改动」，前端不会读到旧快照。
            Some(cmd) = cmd_rx.recv() => {
                let ack = orch.handle(cmd).await;
                if ack.changed() { orch.tick().await; }
                ack.send();
            }
            _ = tokio::signal::ctrl_c() => {
                info!("收到中断信号，退出（托管的 new-api 仍在跑，用 down 停）");
                break;
            }
        }
    }
    Ok(())
}
