//! 控制循环：维护「单把活动 key」，每轮对每把 key 查用量，重排各渠道 priority。
//!
//! 目标——把「选哪把 key」的决策从 new-api 的加权随机收回到本程序，钉住一把活动 key，
//! 让 prompt 缓存能连续命中，只有切换那一下才 miss。
//!
//! 路由杠杆用 priority（不是 weight）：new-api 优先路由最高 priority 的渠道。
//!   - active   (最高档)：所有正常流量都走它。
//!   - standby  (中档)  ：有额度、平时不碰，只作 429 反应式兜底目标。
//!   - exhausted(最低档)：逼近/超阈值，最后手段。
//! 用 priority 而非 weight=0，是为了在活动 key 撞墙时，new-api 仍能沿 priority 阶梯
//! 自动跌到还有额度的 standby 渠道——weight=0 会把渠道从选择集里抹掉，破坏这层兜底。
//!
//! 切换策略（贴合缓存局部性：能不换就不换）：
//!   - 当前活动 key 只要 pct < throttle 就继续钉住。
//!   - 活动 key ≥ throttle 才切走，在「有额度」的其余 key 里挑 pct 最低（剩余最多）的当新活动。
//!   - 本轮查询失败的 key 不参与决策，也不去动它的 priority；活动 key 查询失败时保持不变，
//!     避免一次瞬时抖动就切换、白白丢缓存。

use crate::config::{Config, ResolvedKey};
use crate::newapi::NewApiClient;
use crate::quota::{QuotaProbe, QuotaStatus};
use crate::status::{self, KeyStatus, Shared, StatusSnapshot};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, error, info, warn};

pub struct Orchestrator {
    cfg: Config,
    probe: QuotaProbe,
    api: NewApiClient,
    /// channel_id 已解析好的 key 列表
    keys: Vec<ResolvedKey>,
    /// 当前钉住的活动渠道 id
    active: Option<i64>,
    /// 已下发到 new-api 的 priority（channel_id → priority），幂等用，避免每轮重复 PUT
    applied: HashMap<i64, i64>,
    /// 状态看板共享快照（每轮 tick 末尾整体覆盖）
    snapshot: Shared,
    /// new-api 健康探测用
    http: reqwest::Client,
}

/// 取该 key 在所有监控窗口里的最大使用率（5h 墙和周墙谁高听谁的）。
fn max_watch_pct(cfg: &Config, status: &QuotaStatus) -> f64 {
    let mut max_pct = 0.0f64;
    for w in &cfg.watch_windows {
        if let Some(ws) = status.window(*w) {
            if ws.percentage > max_pct {
                max_pct = ws.percentage;
            }
        }
    }
    max_pct
}

impl Orchestrator {
    pub fn new(cfg: Config, api: NewApiClient, keys: Vec<ResolvedKey>, snapshot: Shared) -> Self {
        let probe = QuotaProbe::new(&cfg.zhipu);
        Self {
            cfg,
            probe,
            api,
            keys,
            active: None,
            applied: HashMap::new(),
            snapshot,
            http: reqwest::Client::new(),
        }
    }

    /// new-api 健康探测（只入快照，不影响本轮决策）
    async fn newapi_healthy(&self) -> bool {
        let url = format!(
            "{}/api/status",
            self.cfg.new_api.base_url.trim_end_matches('/')
        );
        matches!(
            self.http.get(&url).timeout(Duration::from_secs(3)).send().await,
            Ok(r) if r.status().is_success()
        )
    }

    pub async fn tick(&mut self) {
        let keys = self.keys.clone();

        // 1. 采集每把 key 的最大窗口使用率；查询失败的不进 map（不参与本轮决策）。
        //    windows / errors 仅供状态看板，不参与决策。
        let mut pct: HashMap<i64, f64> = HashMap::new();
        let mut windows: HashMap<i64, QuotaStatus> = HashMap::new();
        let mut errors: HashMap<i64, String> = HashMap::new();
        for k in &keys {
            match self.probe.query(&k.zhipu_api_key, &k.quota_headers).await {
                Ok(status) => {
                    let p = max_watch_pct(&self.cfg, &status);
                    info!(name = %k.name, channel_id = k.channel_id, pct = p, "用量");
                    pct.insert(k.channel_id, p);
                    windows.insert(k.channel_id, status);
                }
                Err(e) => {
                    warn!(key = %k.name, error = %e, "查询智谱用量失败，本轮不参与决策");
                    errors.insert(k.channel_id, e.to_string());
                }
            }
        }

        let throttle = self.cfg.throttle_threshold;
        let restore = self.cfg.restore_threshold;

        // 2. 选出活动 key（sticky）
        //    - 当前活动：已知 pct<throttle 才留；本轮没查到（瞬时失败）也留，不因抖动切换。
        let keep = self.active.filter(|id| match pct.get(id) {
            Some(p) => *p < throttle,
            None => true,
        });
        //    - 需要换：在有额度的 key 里挑 pct 最低的。优先要求 pct<restore（多留余量）；
        //      若没有那么宽裕的，退而取 pct<throttle 里最低的。
        let active = keep.or_else(|| {
            let pick = |limit: f64| -> Option<i64> {
                keys.iter()
                    .map(|k| k.channel_id)
                    .filter(|id| pct.get(id).map_or(false, |p| *p < limit))
                    .min_by(|a, b| pct[a].total_cmp(&pct[b]))
            };
            pick(restore).or_else(|| pick(throttle))
        });

        if active != self.active {
            match (self.active, active) {
                (_, Some(id)) => {
                    let name = keys
                        .iter()
                        .find(|k| k.channel_id == id)
                        .map(|k| k.name.as_str())
                        .unwrap_or("?");
                    info!(name, channel_id = id, "切换活动 key");
                }
                (Some(prev), None) => {
                    warn!(prev, "所有 key 都无额度，保留原活动并交给 new-api 429 兜底")
                }
                (None, None) => warn!("暂无任何可用 key"),
            }
            // 全无额度时不要把 active 清空（清空会误把原活动降到 exhausted），保留原值。
            if active.is_some() {
                self.active = active;
            }
        }
        let active = self.active;

        // 3. 计算每个渠道的目标 priority，仅在与已下发值不同时才 PUT（幂等）
        for k in &keys {
            let id = k.channel_id;
            let target = if Some(id) == active {
                self.cfg.priority_active
            } else if pct.get(&id).map_or(false, |p| *p < throttle) {
                self.cfg.priority_standby
            } else if pct.contains_key(&id) {
                self.cfg.priority_exhausted
            } else {
                // 本轮查询失败：状态未知，不动它
                continue;
            };

            if self.applied.get(&id) == Some(&target) {
                continue;
            }

            if self.cfg.dry_run {
                info!(name = %k.name, channel_id = id, priority = target, "dry_run: 将设 priority");
                self.applied.insert(id, target);
            } else {
                match self.api.set_channel_priority(id, target).await {
                    Ok(_) => {
                        self.applied.insert(id, target);
                        info!(name = %k.name, channel_id = id, priority = target, "已设 priority");
                    }
                    Err(e) => error!(name = %k.name, error = %e, "设 priority 失败"),
                }
            }
        }

        // 4. 发布状态快照（在决策与下发**之后**生成 ⇒ 看板与 new-api 实际状态一致；
        //    下发失败的 key 其 applied 未更新，快照如实显示旧 priority，不撒谎）
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let healthy = self.newapi_healthy().await;

        // new-api 面板数据：**纯读**（已核实 GetAllChannels / /api/log/ 无写操作）。
        // 拉取在决策与下发之后 ⇒ 失败也不可能影响切换；退化为空列表，看板显示「暂无数据」。
        let channels = match self.api.list_channel_states().await {
            Ok(v) => v,
            Err(e) => {
                debug!(error = %e, "拉取渠道状态失败（看板降级，不影响切换）");
                Vec::new()
            }
        };
        let recent = match self.api.recent_logs(20).await {
            Ok(v) => v,
            Err(e) => {
                debug!(error = %e, "拉取请求日志失败（看板降级，不影响切换）");
                Vec::new()
            }
        };
        let client_endpoint = format!("{}/v1", self.cfg.new_api.base_url.trim_end_matches('/'));

        let key_statuses = keys
            .iter()
            .map(|k| {
                let id = k.channel_id;
                let w = windows.get(&id);
                let max = pct.get(&id).copied();
                let tier = if Some(id) == active {
                    "active"
                } else if max.is_some_and(|p| p < throttle) {
                    "standby"
                } else if max.is_some() {
                    "exhausted"
                } else {
                    "unknown"
                };
                KeyStatus {
                    name: k.name.clone(),
                    channel_id: id,
                    five_hour_pct: w.and_then(|q| q.five_hour.as_ref().map(|x| x.percentage)),
                    weekly_pct: w.and_then(|q| q.weekly.as_ref().map(|x| x.percentage)),
                    five_hour_reset: w.and_then(|q| q.five_hour.as_ref().map(|x| x.next_reset_time)),
                    weekly_reset: w.and_then(|q| q.weekly.as_ref().map(|x| x.next_reset_time)),
                    max_pct: max,
                    tier: tier.to_string(),
                    priority: self.applied.get(&id).copied(),
                    error: errors.get(&id).cloned(),
                }
            })
            .collect();

        status::publish(
            &self.snapshot,
            StatusSnapshot {
                updated_at: now_ms,
                dry_run: self.cfg.dry_run,
                throttle_threshold: throttle,
                restore_threshold: restore,
                new_api_base: self.cfg.new_api.base_url.clone(),
                new_api_healthy: healthy,
                active_channel_id: active,
                keys: key_statuses,
                client_endpoint,
                channels,
                recent,
            },
        );
    }
}
