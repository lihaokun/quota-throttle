# 细化设计 — key-rotation / keyrot-1-weekly-reset-first（周窗口临期优先切换）

## 1. 范围

- 本细化覆盖：`config.rs::Config.weekly_reset_lookahead_hours`（新字段）、
  `orchestrator.rs::{WeeklyInfo, imminent, eligible_set, decide, tick}`（决策层）、
  `status.rs::{KeyStatus.imminent, StatusSnapshot.weekly_lookahead_hours}` + 前端 2 行。
- 与架构对应：G1（imminent+decide）、G2（短路等价）、G3（status 展示）。
- `quota.rs` / `newapi.rs` / `main.rs` 零改动（数据在 `QuotaStatus.weekly` 已就位）。

## 2. 与已有代码的复用点

| 复用项 | 是否需适配 |
|---|---|
| `QuotaStatus.weekly`（percentage + next_reset_time） | 原样读取 |
| `max_watch_pct` | 原样调用（tick 已产出） |
| `eligible_set` / `decide` 现有结构与测试 | 扩展，签名加参数 |
| `Regime` 分档 | 不动 |
| `status::update` 快照 | 加字段 |

## 3. 错误处理策略

- 错误模型：所有新逻辑为**纯函数**，无 IO、无错误路径（imminent 对 Option 全分支处理）。
- `weekly` 缺失 / `reset_ms` 异常（≤0 或 > now 但超上限——即异常值）→ 保守视为**非临期**
  （宁可少烧一次也不误判临期导致乱切）。

## 4. 数据结构定义

### 4.1 `WeeklyInfo`（module-private; orchestrator.rs）
```rust
pub struct WeeklyInfo {
    pub reset_ms: i64,   // epoch 毫秒（探针原样）
    pub pct: f64,        // 周窗口已用%
    pub max_pct: f64,    // max_watch_pct（5h 与周取大）
}
```
- **不变量**：`reset_ms > 0`（异常值 → 不构造 WeeklyInfo，视为非临期）。

### 4.2 `StatusSnapshot.weekly_lookahead_hours` / `KeyStatus.imminent`
```
weekly_lookahead_hours: u64 — 配置值 0..=168；前端「策略开关」显示
imminent: bool — 决策层同源（tick 构建 weekly_map 后推导）
```

## 5. 模块细化

### 5.1 config.rs

#### 5.1.1 `Config.weekly_reset_lookahead_hours: u64`
- 功能：周窗口临期时间窗（小时）。0 = 关闭（与旧版等价）；默认 24。
- 调用关系：callers: `orchestrator::tick`（每轮读）；callees: 无。
- 实现：
  ```rust
  /// 周窗口重置进入这个时间窗（小时）内的 key 视为「临期」：切换时优先选它。
  /// 0 = 关闭本策略（行为与未引入本功能时完全一致）。
  #[serde(default = "default_weekly_lookahead")]
  pub weekly_reset_lookahead_hours: u64,
  // default = 24
  ```
- 校验：`validate()` 加 `ensure!(self.weekly_reset_lookahead_hours <= 24*7, ...)`
  （>7 天 = 手误；周窗口最长一周，不存在更长合法值）。
- 正确性：trivial（声明式字段 + 范围校验）。

### 5.2 orchestration.rs（核心）

#### 5.2.1 `fn imminent(weekly: &WeeklyInfo, now_ms: i64, lookahead_ms: i64, exhausted: f64) -> bool`
- 功能：该 key 是否临期（切换时优先选）。
- 前置：`lookahead_ms ≥ 0`；`weekly.pct` / `weekly.max_pct` ∈ [0,100]（探针保证，宽容上界外 → false）。
- 后置：`true ⟺ weekly.pct < exhausted ∧ weekly.max_pct < exhausted ∧ now_ms < weekly.reset_ms ≤ now_ms + lookahead_ms`
- 实现思路：
  1. `weekly.pct >= exhausted || weekly.max_pct >= exhausted` → false（没用完 & 还能服务）
  2. 差量：`reset_ms - now_ms`；`<= 0` → false（已重置/异常）；`> lookahead_ms` → false
  3. 其余 → true
- 分支覆盖：pct 线 / max_pct 线（两个独立 early-return）、reset 过去、reset 超窗、正常 → 5 分支全有。
- 正确性：
  - 前置断言已含；四条件 each → 独立 return；下界 `reset > now`（严格）与上界 `≤ now+lookahead`（闭）
    ——「恰好 24h」算临期（左开右闭）。
  - **left-closed/right-open**：确认 resets 恰好 `now + lookahead` 时刻 ⇒ 真临期吗？
    设定：智谱窗口中 `reset` 到点即清零，此刻仍有一瞬窗口存在 ⇒ 算临期（含端点）；
    而 `reset = now`（刚清零）⇒ 新 window 已经走了，不算。⇒ 左开右闭 = 精确语义。

#### 5.2.2 `fn eligible_set(ids, pct, throttle, exhausted) -> (Vec<i64>, Regime)`
- 功能：合格集完全沿用原逻辑；临期不放宽 95%/100% 门限。
- 实现：
  1. `known_set` = pct 存在的 key（查询失败不在内）— 原样
  2. `normal` = known ∩ pct < throttle；`deg` = known ∩ pct < exhausted
  3. 返回：
     - known 空 → (∅, Normal)
     - `normal` 非空 → (normal, Normal)
     - 否则 → (deg, Degraded)
- 正确性：
  - 与旧版等价性：合格集与档位判定逐字节沿用旧逻辑。

#### 5.2.3 `fn decide(ids, pct, weekly, now_ms, lookahead, current, pinned, throttle, restore, exhausted) -> Decision`
- 功能：选活动 key（三层 1-pin 2-粘滞 3-pick），pick 层优先临期升序。
- 实现思路：
  1. `(eligible, regime)` = eligible_set(...) — 不接入 weekly，保持安全门限
  2. pin：
     - `pinned` 存在 & ∈ ids：
       - pct None → 保持（抖动）
       - eligible ∋ pinned → 保持
       - 越线（pct ≥ 门限）→ pin_release；门限只按 regime 取，临期不豁免
  3. 粘滞：`current ∈ ids`：
     - pct None → 保持
     - eligible ∋ current → 保持
     - 否则落向 4
  4. pick：
     - `imminent_eligible` = eligible ∩ (weekly 存在 && imminent)
     - 非空 → `min_by_key(reset_ms).min_by(max_pct)`（EDF；平手 pct）
     - 空 → 现状 normal/deg 分支（restore 优先 / 最低）
  5. 返回 Decision
- 分支覆盖：pin 四路 / 粘滞三路 / pick 两路 / 空集 — 全部。
- 正确性：见下论证段。

#### 5.2.4 ✓ 验证「pin_release 豁免」推导（写进细化文档）
设 k = pinned，`k ∈ ids`、pct(k) = Some(p)。
- case A：k ∈ imminent_ids ⇒ k ∈ eligible（不变量 I1）⇒ 走「保持」⇒ 不 release。
- case B：k ∉ imminent_ids。k 在 eligible ⟺ (正常档 p<throttle) 或 (降级档 p<exhausted)。
  若 p < 门限 ⇒ 保持；若 p ≥ 门限 ⇒ release 且**门限正确**（因为 k 不临期，无豁免）。
- 结论：**pin 分支不改**，临期豁免由 I1 自动成立。代码加一行注释引用本文档。

#### 5.2.5 `tick()` 适配
- `weekly_map: HashMap<i64, WeeklyInfo>`：探针循环里 `status.weekly` 存在时构造（reset_ms 为
  `next_reset_time`，**原样 ms 单位**）。
- 调用 decide 时传 `&weekly_map`、`now_ms`、`lookahead_ms`（`cfg.weekly_reset_lookahead_hours * 3600_000`）。
- KeyStatus 构造：`imminent = weekly_map.get(id).map_or(false, |w| now_ms < w.reset_ms && ... )`——
  实际上直接用 `d.eligible` 推导：`imminent_ids = eligible ∩ weekly_map 临期`。
  **实现上**把 `imminent_ids` 作为 `decide()` 返回值的一个字段（Decision 加 `imminent: Vec<i64>`）
  更干净——看板/日志与决策一次计算。
- 决策日志：切换时若选的是临期 key，`info!(... imminent=true, reset_in_min=...)`。
- `eligible_set` 的 now_ms/lookahead_ms 参数由 decide 传入，保持单入口。

### 5.3 status.rs（展示层）

- `KeyStatus` + `imminent: bool`（`#[serde(default)]`——旧 JSON 消费方兼容）。
- `StatusSnapshot.weekly_lookahead_hours: u64`（默认 0；tick 每轮写 cfg 值）。
- 前端：
  - 卡片：`k.imminent ? ' <span class="badge b-imminent">⏳ 临期</span>' : ''`（新增 `.b-imminent` 样式）。
  - 顶部 chips：lookahead>0 时显示 `周临期优先 ${lookahead}h`；=0 显示「周临期优先 关」。

## 6. 完整性自检 checklist

- [x] 实现思路推导连续（5.2.1–5.2.5 步接步）
- [x] if/else 分支覆盖（imminent 5 分支、decide 全部）
- [x] 退出点覆盖（Option/Result 无新增错误路径）
- [x] callee 引用（eligible_set 引用 max_watch_pct；decide 引用 eligible_set；无新增外部 API）
- [x] 循环终止（无新增循环）
- [x] 上游事实显式（H1–H5 表 + 单位声明 ms）
