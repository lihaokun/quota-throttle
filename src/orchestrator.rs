//! 控制循环：维护「单把活动 key」，每轮对每把 key 查用量，重排各渠道 priority。
//!
//! 目标——把「选哪把 key」的决策从 new-api 的加权随机收回到本程序，钉住一把活动 key，
//! 让 prompt 缓存能连续命中，只有切换那一下才 miss。
//!
//! 路由杠杆用 priority（不是 weight）：new-api 优先路由最高 priority 的渠道。
//!   - active   (最高档)：所有正常流量都走它。
//!   - standby  (中档)  ：合格但非活动，平时不碰，只作 429 反应式兜底目标。
//!   - exhausted(最低档)：不合格，最后手段。
//! 用 priority 而非 weight=0，是为了在活动 key 撞墙时，new-api 仍能沿 priority 阶梯
//! 自动跌到还有额度的 standby 渠道——weight=0 会把渠道从选择集里抹掉，破坏这层兜底。
//!
//! ## 两条线，不是一条
//!
//! throttle(95%) 是**预防线**——还有余量，但该提前换走了（护缓存 + 别撞墙）。
//! exhausted(100%) 是**真·用尽线**——物理上没了。
//! 二者据此分出两档「合格集」（= 自动逻辑允许把流量放上去的 key）：
//!   - **正常档**：有 key < throttle ⇒ 只在这些宽裕的里选。
//!   - **降级档**：全员 ≥ throttle ⇒ 放宽到「只要 < exhausted 就能用」。
//!     否则会出现「明知 96% 那把还有余量，却继续钉着已经 100% 的活动 key 撞 429」。
//!
//! ## 选择策略（贴合缓存局部性：能不换就不换）
//!
//!   - **pin**（用户从看板手动指定）只在合格集内生效：它是**优先级**，不是安全豁免——
//!     能覆盖「粘滞 + 挑最低」的选择偏好，但**不能把不合格的 key 拉上来用**，越线即自动解除。
//!   - **粘滞**：现任只要还在合格集里就不换（两档皆然；降级档下就是「榨干到 100% 再流转」）。
//!   - **挑选**：只在合格集内挑 pct 最低的（正常档优先要求 < restore，多留余量 → 切换更少）。
//!   - **抖动保护**：本轮查询失败的 key 不进合格集（不会被主动选中），也不动它的 priority；
//!     但若它恰好是现任或被 pin 的，则保持不变——一次瞬时失败不该丢缓存、也不该抖掉 pin。

use crate::config::{Config, NewKeySpec, ResolvedKey};
use crate::newapi::NewApiClient;
use crate::quota::{QuotaProbe, QuotaStatus};
use crate::status::{self, KeyStatus, LiveMetric, Shared};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

pub struct Orchestrator {
    cfg: Config,
    probe: QuotaProbe,
    api: Arc<NewApiClient>,
    /// channel_id 已解析好的 key 列表
    keys: Vec<ResolvedKey>,
    /// 当前钉住的活动渠道 id
    active: Option<i64>,
    /// 用户从看板手动 pin 的渠道（只在合格集内生效，见 decide）
    pinned: Option<i64>,
    /// 上一次 pin 被自动解除的事件（供看板提示）
    last_pin_release: Option<PinRelease>,
    /// 上一轮的档位（判 pin 是否合法时要用它换算合格线）
    regime: Regime,
    /// 上一轮的合格集与 pct —— pin 命令据此**当场**判定能不能钉，不必等下一轮
    last_eligible: Vec<i64>,
    last_pct: HashMap<i64, f64>,
    /// 已下发到 new-api 的 priority（channel_id → priority），幂等用，避免每轮重复 PUT
    applied: HashMap<i64, i64>,
    /// 状态看板共享快照（**只写决策字段**，面板字段由 Panel 循环独占）
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

/// 合格集是在哪一档算出来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    /// 还有 key 低于预防线 ⇒ 只在这些宽裕的 key 里选
    Normal,
    /// 全员都过了预防线 ⇒ 放宽到「只要还有余量（< exhausted）就能用」
    Degraded,
}

impl Regime {
    pub fn as_str(self) -> &'static str {
        match self {
            Regime::Normal => "normal",
            Regime::Degraded => "degraded",
        }
    }
}

/// pin 因越出合格线被**自动解除**的事件（供看板提示）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PinRelease {
    pub channel_id: i64,
    /// 解除时该 key 的 pct
    pub pct: f64,
    /// 当时的合格线（正常档=throttle，降级档=exhausted）
    pub limit: f64,
}

/// 看板（HTTP）→ 控制循环的命令。
///
/// **为什么走命令通道而不是给状态加锁**：keys / active / pinned / applied 现在只有
/// orchestrator 一个写者，无锁、无重入，并发安全性是**显然**的。让 HTTP 直接改这些状态
/// 会毁掉这个不变量。命令通道把「谁能写」这件事继续锁死在一个地方。
///
/// 每条命令带一个 oneshot 回执 ⇒ HTTP 侧能**同步拿到成败**（比如加 key 时智谱的错误原文
/// 要回显到面板上，而不是让用户去翻日志）。
#[derive(Debug)]
pub enum Command {
    /// 手动钉住某把 key（只在合格集内生效，见 decide）
    Pin {
        channel_id: i64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// 取消手动 pin，回到自动选择
    Unpin {
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// 从看板加一把 key：探活 → 建渠道 → 写回 config.toml → 热加载
    AddKey {
        spec: NewKeySpec,
        reply: oneshot::Sender<Result<AddKeyOk, String>>,
    },
    /// 停止调度某把 key（priority 压到 0 + 从 config.toml 摘除；**不删 new-api 渠道**）
    RemoveKey {
        channel_id: i64,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// 加 key 成功后回给面板的东西：探活结果原样奉上，让用户当场确认
/// 「这把 key 确实是我以为的那把」（套餐档位 + 两个窗口的真实用量）。
#[derive(Debug, Clone, Serialize)]
pub struct AddKeyOk {
    pub channel_id: i64,
    pub level: Option<String>,
    pub five_hour_pct: Option<f64>,
    pub weekly_pct: Option<f64>,
}

/// 命令回执：**主循环在 tick 之后才发出**，于是「HTTP 200 返回」⇔「状态已落地且已发布」。
pub enum Ack {
    Unit(oneshot::Sender<Result<(), String>>, Result<(), String>),
    Key(
        oneshot::Sender<Result<AddKeyOk, String>>,
        Result<AddKeyOk, String>,
    ),
}

impl Ack {
    /// 命令是否真的改了状态。失败的命令什么也没改 ⇒ 不必再跑一轮 tick
    /// （tick 会去查智谱，不该被无效命令白白触发）。
    pub fn changed(&self) -> bool {
        match self {
            Ack::Unit(_, r) => r.is_ok(),
            Ack::Key(_, r) => r.is_ok(),
        }
    }
    pub fn send(self) {
        match self {
            Ack::Unit(tx, r) => {
                let _ = tx.send(r);
            }
            Ack::Key(tx, r) => {
                let _ = tx.send(r);
            }
        }
    }
}

/// 一轮决策的完整结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// 本轮活动 key。`None` = 无人合格 ⇒ 调用方**保留原活动**（不清空）
    pub active: Option<i64>,
    /// 合格集：自动逻辑允许把流量放上去的 key
    pub eligible: Vec<i64>,
    pub regime: Regime,
    /// 非空则调用方须把 `pinned` 置 None
    pub pin_release: Option<PinRelease>,
}

/// 算合格集 —— **自动逻辑的地盘，pin 无权染指**。
///
/// - 已知集 = 本轮成功查到 pct 的 key（查询失败的不在其中，状态未知）
/// - 正常集 = 已知集里 pct < throttle 的；非空 ⇒ 正常档，合格集 = 正常集
/// - 否则   ⇒ 降级档，合格集 = 已知集里 pct < exhausted 的（还有余量就能用）
///
/// 全部查询失败（已知集为空）⇒ 返回空集 + 正常档：没有任何证据表明降级了。
fn eligible_set(
    ids: &[i64],
    pct: &HashMap<i64, f64>,
    throttle: f64,
    exhausted: f64,
) -> (Vec<i64>, Regime) {
    let under = |limit: f64| -> Vec<i64> {
        ids.iter()
            .copied()
            .filter(|id| pct.get(id).is_some_and(|p| *p < limit))
            .collect()
    };
    if !ids.iter().any(|id| pct.contains_key(id)) {
        return (Vec::new(), Regime::Normal);
    }
    let normal = under(throttle);
    if !normal.is_empty() {
        return (normal, Regime::Normal);
    }
    (under(exhausted), Regime::Degraded)
}

/// 选活动 key。**纯函数**（分支多、又是安全核心，必须可单测）。
///
/// 三层，从强到弱：
///   1. pin —— 只在合格集内生效。pin 是**优先级**，不是安全豁免：它能覆盖「粘滞 + 挑最低」
///      这个选择偏好，但**不能把自动逻辑判定为不合格的 key 拉上来用**。越线即自动解除。
///   2. 粘滞 —— 现任只要还合格就不换（护 prompt 缓存：能不换就不换）。
///   3. 挑选 —— 只在合格集内挑 pct 最低的（正常档优先要求 < restore，多留余量）。
///
/// **抖动保护**：本轮查询失败的 key 不进合格集（不会被主动选中），但若它恰好是 current
/// 或 pinned，则**保持不变** —— 一次瞬时失败不该丢缓存，也不该抖掉用户的 pin。
/// 反过来说，解除 pin 必须基于「查到了 **且** 确实超线」这个正面证据。
fn decide(
    ids: &[i64],
    pct: &HashMap<i64, f64>,
    current: Option<i64>,
    pinned: Option<i64>,
    throttle: f64,
    restore: f64,
    exhausted: f64,
) -> Decision {
    let (eligible, regime) = eligible_set(ids, pct, throttle, exhausted);
    let mut pin_release = None;
    let done = |active, eligible, pin_release| Decision {
        active,
        eligible,
        regime,
        pin_release,
    };

    // 1. pin（只在合格集内）
    if let Some(p) = pinned.filter(|p| ids.contains(p)) {
        match pct.get(&p) {
            // 查询失败 → 保持 pin（抖动保护）
            None => return done(Some(p), eligible, None),
            Some(_) if eligible.contains(&p) => return done(Some(p), eligible, None),
            // 越线 → 解除 pin，落到自动逻辑
            Some(&pp) => {
                let limit = match regime {
                    Regime::Normal => throttle,
                    Regime::Degraded => exhausted,
                };
                pin_release = Some(PinRelease {
                    channel_id: p,
                    pct: pp,
                    limit,
                });
            }
        }
    }

    // 2. 粘滞
    if let Some(c) = current.filter(|c| ids.contains(c)) {
        match pct.get(&c) {
            // 查询失败 → 保持（抖动保护）
            None => return done(Some(c), eligible, pin_release),
            Some(_) if eligible.contains(&c) => return done(Some(c), eligible, pin_release),
            Some(_) => {}
        }
    }

    // 3. 在合格集内挑 pct 最低
    let lowest = |cands: &[i64]| -> Option<i64> {
        cands
            .iter()
            .copied()
            .min_by(|a, b| pct[a].total_cmp(&pct[b]))
    };
    let active = match regime {
        Regime::Normal => {
            // 优先挑余量更足的（< restore），让新活动 key 撑更久 → 切换更少
            let roomy: Vec<i64> = eligible
                .iter()
                .copied()
                .filter(|id| pct[id] < restore)
                .collect();
            lowest(&roomy).or_else(|| lowest(&eligible))
        }
        // 降级档全员已 ≥ throttle，restore 在这一档没有意义
        Regime::Degraded => lowest(&eligible),
    };

    // 4. active == None ⇒ 合格集为空（全员 ≥ exhausted 或全查询失败）⇒ 调用方保留原活动
    done(active, eligible, pin_release)
}

impl Orchestrator {
    pub fn new(
        cfg: Config,
        api: Arc<NewApiClient>,
        keys: Vec<ResolvedKey>,
        snapshot: Shared,
    ) -> Self {
        let probe = QuotaProbe::new(&cfg.zhipu);
        Self {
            cfg,
            probe,
            api,
            keys,
            active: None,
            pinned: None,
            last_pin_release: None,
            regime: Regime::Normal,
            last_eligible: Vec::new(),
            last_pct: HashMap::new(),
            applied: HashMap::new(),
            snapshot,
            http: reqwest::Client::new(),
        }
    }

    /// 处理一条看板命令，返回**尚未发出**的回执。
    ///
    /// 调用方须：改了状态 → 先跑一轮 tick（priority 秒级下发）→ 再 `ack.send()`。
    /// 顺序很重要：回执一旦发出，HTTP 就 200 了；若此时 tick 还没跑，前端立刻拉
    /// `/api/status` 会看到**旧快照**，像是按钮没生效。
    pub async fn handle(&mut self, cmd: Command) -> Ack {
        match cmd {
            Command::Pin { channel_id, reply } => Ack::Unit(reply, self.pin(channel_id)),
            Command::Unpin { reply } => {
                if self.pinned.is_some() {
                    info!("已取消 pin，回到自动选择");
                }
                self.pinned = None;
                self.last_pin_release = None;
                Ack::Unit(reply, Ok(()))
            }
            Command::AddKey { spec, reply } => {
                let r = self.add_key(spec).await;
                Ack::Key(reply, r)
            }
            Command::RemoveKey { channel_id, reply } => {
                let r = self.remove_key(channel_id).await;
                Ack::Unit(reply, r)
            }
        }
    }

    /// 加一把 key：**探活 → 建渠道 → 写回 config.toml → 热加载**。顺序是有讲究的。
    ///
    /// ① 探活先行，是这个功能最值钱的地方。CLAUDE.md 里那三个坑（url 缺 `?type=2`、缺
    ///    `Bearer`、缺 org/project selector）**都不会报错**——它们表现为「查得通但 limits 为空」，
    ///    而空 limits 会被当成 0% 用量，于是这把 key **永远不会被切走**，直到某天线上撞墙才发现。
    ///    把这个错挡在录入口，比事后 debug 便宜一万倍。探活不过 ⇒ 什么都不改。
    /// ② 探活过了才建渠道、才动 config.toml。
    async fn add_key(&mut self, spec: NewKeySpec) -> Result<AddKeyOk, String> {
        let name = spec.name.trim().to_string();
        if name.is_empty() || spec.api_key.trim().is_empty() {
            return Err("名称和 API key 都不能为空".into());
        }
        if self.keys.iter().any(|k| k.name == name) {
            return Err(format!("已存在同名 key：{name}"));
        }
        let spec = NewKeySpec {
            name: name.clone(),
            api_key: spec.api_key.trim().to_string(),
            ..spec
        };

        // ① 探活
        let headers = spec.headers();
        let status = self
            .probe
            .query(&spec.api_key, &headers)
            .await
            .map_err(|e| format!("探活失败，未做任何改动：{e}"))?;

        // ② 建渠道（新 key 一律以 standby 入场；要不要转正交给下一轮自动决策）
        let tpl = self
            .cfg
            .new_api
            .channel_template
            .as_ref()
            .ok_or("配置里没有 [new_api.channel_template]，无法自动建渠道")?;
        self.api
            .create_channel(tpl, &name, &spec.api_key, self.cfg.priority_standby)
            .await
            .map_err(|e| format!("建渠道失败：{e}"))?;
        let channel_id = self
            .api
            .list_channels()
            .await
            .ok()
            .and_then(|m| m.get(&name).copied())
            .ok_or_else(|| format!("渠道已建好，但在 new-api 里解析不到它的 id：{name}"))?;

        // ③ 写回 config.toml（唯一数据源 ⇒ 重启后仍在）
        if let Err(e) = crate::config::append_key(&self.cfg.source_path, &spec) {
            error!(name = %name, error = %e, "写回 config.toml 失败");
            return Err(format!(
                "渠道已建好（#{channel_id}），但写回 config.toml 失败：{e}。\
                 本次运行内它不会生效，请手动补一条 [[keys]] 后重启"
            ));
        }

        // ④ 热加载：下一轮 tick 就会查它的用量并纳入调度
        self.keys.push(ResolvedKey {
            name: name.clone(),
            zhipu_api_key: spec.api_key.clone(),
            channel_id,
            quota_headers: headers,
        });
        info!(name = %name, channel_id, level = ?status.level, "已加入新 key（探活通过）");

        Ok(AddKeyOk {
            channel_id,
            level: status.level,
            five_hour_pct: status.five_hour.as_ref().map(|w| w.percentage),
            weekly_pct: status.weekly.as_ref().map(|w| w.percentage),
        })
    }

    /// 停止调度某把 key。**不删 new-api 渠道**（它身上挂着历史用量和日志）。
    ///
    /// ⚠️ 但必须先把它的 priority 压到最低档再放手——否则一把 priority=100 的活动渠道被移出
    /// 管辖后仍会**继续吃下全部流量**，而我们已经不再盯它的用量了。那是最坏的结果。
    async fn remove_key(&mut self, id: i64) -> Result<(), String> {
        let name = self
            .keys
            .iter()
            .find(|k| k.channel_id == id)
            .map(|k| k.name.clone())
            .ok_or_else(|| format!("渠道 #{id} 不在管辖的 key 列表里"))?;
        if self.keys.len() <= 1 {
            return Err("这是最后一把 key，移除后就没有可路由的渠道了".into());
        }

        if !self.cfg.dry_run {
            self.api
                .set_channel_priority(id, self.cfg.priority_exhausted)
                .await
                .map_err(|e| format!("把 {name} 的 priority 压到最低失败，未做任何改动：{e}"))?;
        }
        crate::config::remove_key(&self.cfg.source_path, &name)
            .map_err(|e| format!("从 config.toml 移除失败：{e}"))?;

        self.keys.retain(|k| k.channel_id != id);
        self.applied.remove(&id);
        if self.active == Some(id) {
            self.active = None; // 下一轮自动重选
        }
        if self.pinned == Some(id) {
            self.pinned = None;
        }
        info!(name = %name, channel_id = id, "已停止调度（new-api 渠道保留，priority 已压到最低）");
        Ok(())
    }

    /// 钉住某把 key。**只能钉合格集内的** —— pin 是优先级，不是安全豁免：
    /// 它能覆盖「粘滞 + 挑最低」的选择偏好，但不能把自动逻辑判定为不合格的 key 拉上来用。
    fn pin(&mut self, id: i64) -> Result<(), String> {
        let name = self
            .keys
            .iter()
            .find(|k| k.channel_id == id)
            .map(|k| k.name.clone())
            .ok_or_else(|| format!("渠道 #{id} 不在管辖的 key 列表里"))?;

        // 判据用**上一轮**的合格集：命令是异步来的，此刻没有更新的证据
        if !self.last_eligible.contains(&id) {
            let limit = match self.regime {
                Regime::Normal => self.cfg.throttle_threshold,
                Regime::Degraded => self.cfg.exhausted_threshold,
            };
            return Err(match self.last_pct.get(&id) {
                Some(p) => format!(
                    "{name} 用量 {p}% 已越过合格线 {limit}%，自动逻辑不允许钉住（pin 是优先级，不是安全豁免）"
                ),
                None => format!("{name} 本轮用量查询失败，状态未知，暂不能钉住"),
            });
        }

        self.pinned = Some(id);
        self.last_pin_release = None; // 手动重新 pin ⇒ 清掉上一次的「自动解除」提示
        info!(name = %name, channel_id = id, "已手动钉住 key");
        Ok(())
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
        let exhausted = self.cfg.exhausted_threshold;

        // 2. 选出活动 key（合格集 + pin + 粘滞，见 decide 的文档）
        let ids: Vec<i64> = keys.iter().map(|k| k.channel_id).collect();
        let name_of = |id: i64| -> &str {
            keys.iter()
                .find(|k| k.channel_id == id)
                .map_or("?", |k| k.name.as_str())
        };
        let d = decide(
            &ids,
            &pct,
            self.active,
            self.pinned,
            throttle,
            restore,
            exhausted,
        );

        // pin 越线 → 自动解除（pin 不得覆盖自动逻辑）
        if let Some(rel) = d.pin_release {
            warn!(
                name = name_of(rel.channel_id),
                channel_id = rel.channel_id,
                pct = rel.pct,
                limit = rel.limit,
                "pin 的 key 已越出合格线，自动解除 pin，回到自动选择"
            );
            self.pinned = None;
            self.last_pin_release = Some(rel);
        }

        // 记下本轮判据，供 pin 命令**当场**判合法性（命令是异步来的，不能等下一轮）
        self.last_eligible = d.eligible.clone();
        self.last_pct = pct.clone();

        if d.regime != self.regime {
            match d.regime {
                Regime::Degraded => warn!(
                    throttle,
                    exhausted, "进入降级档：全部 key 均已越过预防线，将榨干活动 key 至用尽再流转"
                ),
                Regime::Normal => info!("回到正常档：已有 key 低于预防线"),
            }
            self.regime = d.regime;
        }

        if d.active != self.active {
            match (self.active, d.active) {
                (_, Some(id)) => info!(name = name_of(id), channel_id = id, "切换活动 key"),
                (Some(prev), None) => {
                    warn!(prev, "所有 key 都无额度，保留原活动并交给 new-api 429 兜底")
                }
                (None, None) => warn!("暂无任何可用 key"),
            }
            // 全无额度时不要把 active 清空（清空会误把原活动降到 exhausted），保留原值。
            if d.active.is_some() {
                self.active = d.active;
            }
        }
        let active = self.active;

        // 3. 计算每个渠道的目标 priority，仅在与已下发值不同时才 PUT（幂等）。
        //    单一规则（正常档下与旧的三分支逐字节等价；降级档下自动变准——**还有余量**的
        //    key 拿 standby 而非 exhausted，于是万一活动 key 仍撞 429，new-api 的 priority
        //    阶梯会优先跌到还有余量的那把，而不是随机跌到一把已经死透的）。
        for k in &keys {
            let id = k.channel_id;
            let target = if Some(id) == active {
                self.cfg.priority_active
            } else if d.eligible.contains(&id) {
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

        // 4. 发布**决策部分**快照（在决策与下发之后生成 ⇒ 与 new-api 实际状态一致；
        //    下发失败的 key 其 applied 未更新，快照如实显示旧 priority，不撒谎）。
        //    面板字段（channels/live/hourly/model_usage/内部余额）由 Panel 循环独占写，此处不碰。
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let healthy = self.newapi_healthy().await;
        let client_endpoint = format!("{}/v1", self.cfg.new_api.base_url.trim_end_matches('/'));

        let key_statuses = keys
            .iter()
            .map(|k| {
                let id = k.channel_id;
                let w = windows.get(&id);
                let max = pct.get(&id).copied();
                // 档位与 priority 下发规则同源（合格集），两者永不打架
                let tier = if Some(id) == active {
                    "active"
                } else if d.eligible.contains(&id) {
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

        // 只写决策字段（面板字段不动）
        status::update(&self.snapshot, |s| {
            s.updated_at = now_ms;
            s.dry_run = self.cfg.dry_run;
            s.throttle_threshold = throttle;
            s.restore_threshold = restore;
            s.exhausted_threshold = exhausted;
            s.new_api_base = self.cfg.new_api.base_url.clone();
            s.new_api_healthy = healthy;
            s.active_channel_id = active;
            s.pinned_channel_id = self.pinned;
            s.regime = d.regime.as_str().to_string();
            s.eligible = d.eligible.clone();
            s.last_pin_release = self.last_pin_release.map(|r| status::PinReleaseInfo {
                channel_id: r.channel_id,
                pct: r.pct,
                limit: r.limit,
            });
            s.keys = key_statuses;
            s.client_endpoint = client_endpoint;
        });
    }
}

/// 面板循环：只刷**看板数据**，与切换循环完全隔离。
///
/// 为什么独立：智谱是**外部 API**（该低频，60s），new-api 是**本地**服务（毫秒级纯读，可 5s 高频）。
/// 绑在一起会让看板 60 秒才动一次，看不到实时流量。
///
/// **并发安全**：本循环只写面板字段（channels / live / hourly / model_usage / newapi_user_quota），
/// 决策循环只写决策字段，两者严格不相交 + 写锁互斥 ⇒ 不会互相覆盖。
/// 本循环整个挂掉也不影响切换。
pub struct Panel {
    pub api: Arc<NewApiClient>,
    pub snapshot: Shared,
}

impl Panel {
    pub async fn run(self, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            self.refresh().await;
        }
    }

    async fn refresh(&self) {
        // 渠道实况（含「被 new-api 自动禁用」这个盲区）
        let channels = self.api.list_channel_states().await.unwrap_or_else(|e| {
            debug!(error = %e, "面板：拉取渠道状态失败");
            Vec::new()
        });

        // new-api 内部虚拟余额（见底会直接挡住转发）
        let newapi_user_quota = self.api.user_quota().await.unwrap_or(-1);

        // 每把 key 的实时速率（最近 60 秒，服务端固定窗口）。
        // 渠道列表**从快照读**（决策循环发布的），而不是自己攥一份副本 ——
        // 这样面板加/删 key 之后无需重启，实时指标就能跟上。
        let mut live: Vec<LiveMetric> = Vec::new();
        for channel_id in status::tracked_channels(&self.snapshot) {
            let (rpm, tpm) = self.api.channel_rate(channel_id).await.unwrap_or((0, 0));
            live.push(LiveMetric {
                channel_id,
                rpm,
                tpm,
                last_request_at: None,
                last_request_model: None,
            });
        }
        // 最后一次请求：日志按时间倒序，每个渠道首次出现即最新
        if let Ok(logs) = self.api.recent_logs(50).await {
            for l in &logs {
                if let Some(m) = live
                    .iter_mut()
                    .find(|m| m.channel_id == l.channel && m.last_request_at.is_none())
                {
                    m.last_request_at = Some(l.created_at);
                    m.last_request_model = Some(l.model_name.clone());
                }
            }
        }

        // 用量统计（new-api 自己按小时聚合的 quota_data）：近 24h 时序 + 按模型汇总
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (mut hourly, mut model_usage) = (Vec::new(), Vec::new());
        if let Ok(rows) = self.api.usage_data(now - 24 * 3600, now + 3600).await {
            // 与 /api/usage 共用聚合函数 ⇒ 近 24h 图和历史图口径必然一致
            (hourly, model_usage) = status::aggregate_usage(rows);
            model_usage.sort_by(|a, b| b.tokens.cmp(&a.tokens)); // 用量降序
        }

        // 只写面板字段
        status::update(&self.snapshot, |s| {
            s.channels = channels;
            s.newapi_user_quota = newapi_user_quota;
            s.live = live;
            s.hourly = hourly;
            s.model_usage = model_usage;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THROTTLE: f64 = 95.0;
    const RESTORE: f64 = 90.0;
    const EXHAUSTED: f64 = 100.0;

    /// pct 列表 → (ids, map)。`None` 表示该 key 本轮**查询失败**（状态未知）。
    fn setup(pcts: &[(i64, Option<f64>)]) -> (Vec<i64>, HashMap<i64, f64>) {
        let ids = pcts.iter().map(|(id, _)| *id).collect();
        let map = pcts
            .iter()
            .filter_map(|(id, p)| p.map(|p| (*id, p)))
            .collect();
        (ids, map)
    }

    fn decide_with(
        pcts: &[(i64, Option<f64>)],
        current: Option<i64>,
        pinned: Option<i64>,
    ) -> Decision {
        let (ids, pct) = setup(pcts);
        decide(
            &ids, &pct, current, pinned, THROTTLE, RESTORE, EXHAUSTED,
        )
    }

    // ——— 正常档：与改造前的行为必须逐条等价（防回归）———

    #[test]
    fn 正常档_粘滞_现任未越预防线就不换() {
        // 现任 94%（快到线了但没过），另一把才 10% —— 依然不换，护 prompt 缓存
        let d = decide_with(&[(1, Some(94.0)), (2, Some(10.0))], Some(1), None);
        assert_eq!(d.active, Some(1));
        assert_eq!(d.regime, Regime::Normal);
    }

    #[test]
    fn 正常档_现任越线_换到余量最足的() {
        let d = decide_with(
            &[(1, Some(95.0)), (2, Some(60.0)), (3, Some(30.0))],
            Some(1),
            None,
        );
        assert_eq!(d.active, Some(3), "该挑 pct 最低的");
        assert!(!d.eligible.contains(&1), "越线的不该进合格集");
    }

    #[test]
    fn 正常档_没有低于restore的_退而取低于throttle里最低的() {
        // 都在 90~95 之间：没有「宽裕」的候选，但它们仍然合格
        let d = decide_with(&[(1, Some(96.0)), (2, Some(93.0)), (3, Some(91.0))], Some(1), None);
        assert_eq!(d.active, Some(3));
        assert_eq!(d.regime, Regime::Normal, "只要有人 < throttle 就还是正常档");
    }

    #[test]
    fn 正常档_首次选择_无现任() {
        let d = decide_with(&[(1, Some(60.0)), (2, Some(30.0))], None, None);
        assert_eq!(d.active, Some(2));
    }

    // ——— 降级档：本次要修的漏洞 ———

    #[test]
    fn 降级档_全员越预防线但现任还有余量_继续榨干不换() {
        // 用户提的场景的前半段：全员 >95%，现任 97% 还有余量 → 不换（护缓存）
        let d = decide_with(&[(1, Some(97.0)), (2, Some(96.0))], Some(1), None);
        assert_eq!(d.regime, Regime::Degraded);
        assert_eq!(d.active, Some(1), "降级档同样粘滞：能不换就不换");
        assert_eq!(d.eligible, vec![1, 2], "有余量的都合格");
    }

    #[test]
    fn 降级档_现任真用尽_流转到还有余量的() {
        // 用户提的场景：全员 >95%，现任到了 100% → 必须流转到 96% 那把，而不是继续钉着死 key
        let d = decide_with(&[(1, Some(100.0)), (2, Some(96.0)), (3, Some(98.0))], Some(1), None);
        assert_eq!(d.regime, Regime::Degraded);
        assert_eq!(d.active, Some(2), "流转到还有余量里 pct 最低的");
        assert!(!d.eligible.contains(&1), "100% 的不合格");
    }

    #[test]
    fn 全员真用尽_保留原活动交给429兜底() {
        let d = decide_with(&[(1, Some(100.0)), (2, Some(100.0))], Some(1), None);
        assert_eq!(d.active, None, "None ⇒ 调用方保留原活动，不清空");
        assert!(d.eligible.is_empty());
    }

    #[test]
    fn 有key低于预防线就该回到正常档() {
        let d = decide_with(&[(1, Some(99.0)), (2, Some(50.0))], Some(1), None);
        assert_eq!(d.regime, Regime::Normal);
        assert_eq!(d.active, Some(2), "现任 99% 已越预防线 → 换到 50% 那把");
    }

    // ——— 抖动保护 ———

    #[test]
    fn 现任查询失败_保持不变() {
        let d = decide_with(&[(1, None), (2, Some(10.0))], Some(1), None);
        assert_eq!(d.active, Some(1), "一次瞬时失败不该丢缓存");
        assert!(!d.eligible.contains(&1), "但它不进合格集（状态未知）");
    }

    #[test]
    fn 全部查询失败_不算降级档() {
        let d = decide_with(&[(1, None), (2, None)], Some(1), None);
        assert_eq!(d.regime, Regime::Normal, "没有任何证据表明降级了");
        assert_eq!(d.active, Some(1));
    }

    // ——— pin：合格集内的偏好，不能覆盖自动逻辑 ———

    #[test]
    fn pin_覆盖挑最低的偏好() {
        // 自动逻辑本会选 3（最低），但用户 pin 了 2 → 听用户的
        let d = decide_with(&[(1, Some(96.0)), (2, Some(60.0)), (3, Some(30.0))], Some(1), Some(2));
        assert_eq!(d.active, Some(2));
        assert_eq!(d.pin_release, None);
    }

    #[test]
    fn pin_覆盖粘滞() {
        // 现任 1 还合格（粘滞本会留它），但用户 pin 了 2
        let d = decide_with(&[(1, Some(50.0)), (2, Some(60.0))], Some(1), Some(2));
        assert_eq!(d.active, Some(2));
    }

    #[test]
    fn pin_不能把不合格的key拉上来用() {
        // 正常档下 pin 一把 96% 的 → 不合格 → 自动解除，回到自动选择
        let d = decide_with(&[(1, Some(50.0)), (2, Some(96.0))], Some(1), Some(2));
        assert_eq!(
            d.pin_release,
            Some(PinRelease { channel_id: 2, pct: 96.0, limit: THROTTLE })
        );
        assert_eq!(d.active, Some(1), "回到自动逻辑（粘滞留现任）");
    }

    #[test]
    fn pin_在降级档下的合格线是exhausted而非throttle() {
        // 全员 >95%：96% 的 key 在降级档里是**合格**的 → pin 得以保持
        let d = decide_with(&[(1, Some(97.0)), (2, Some(96.0))], Some(1), Some(2));
        assert_eq!(d.regime, Regime::Degraded);
        assert_eq!(d.active, Some(2), "降级档下 96% 合格，pin 生效");
        assert_eq!(d.pin_release, None);
    }

    #[test]
    fn pin_的key真用尽_自动解除() {
        let d = decide_with(&[(1, Some(96.0)), (2, Some(100.0))], Some(1), Some(2));
        assert_eq!(
            d.pin_release,
            Some(PinRelease { channel_id: 2, pct: 100.0, limit: EXHAUSTED })
        );
        assert_eq!(d.active, Some(1));
    }

    #[test]
    fn pin_的key查询失败_保持pin不解除() {
        // 解除 pin 必须基于「查到了且确实超线」的正面证据，不能被一次抖动抖掉
        let d = decide_with(&[(1, Some(50.0)), (2, None)], Some(1), Some(2));
        assert_eq!(d.active, Some(2));
        assert_eq!(d.pin_release, None);
    }

    #[test]
    fn pin_指向已被删除的key_忽略() {
        let d = decide_with(&[(1, Some(50.0))], Some(1), Some(99));
        assert_eq!(d.active, Some(1));
        assert_eq!(d.pin_release, None, "不该为不存在的 key 报解除事件");
    }
}
