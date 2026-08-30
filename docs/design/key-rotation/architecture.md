# 架构设计 — key-rotation（周窗口临期优先切换）

> 调研：`docs/research/key-rotation-research.md`。本文档为架构层（§4.2 格式），
> 函数级细化由 `docs/design/key-rotation/detailed-design.md` 承载。

## 1. 核心流程描述

```
tick()（每 poll_interval_secs 一轮）
  └─ 对每把 key: QuotaProbe::query() → QuotaStatus{ five_hour, weekly }
       ├─ weekly 存在 → 构建 WeeklyInfo{reset_ms, pct, max_pct}（决策用）
       └─ weekly 缺失 → 该 key 无 WeeklyInfo（非临期）
     ↓
  decide()（纯函数）
     ├─ eligible_set(): 原合格集不变（正常档 <95%；全员越线后降级档 <100%）
     ├─ 粘滞：现任合格即不换（不变）
     ├─ pin：门限仍由档位决定（正常档 95%；降级档 100%）
     └─ pick：当前合格集内的临期集合非空 → 升序 reset，平手 pct 低者；否则照旧
     ↓
  priority 三档下发（active/standby/exhausted）→ 快照发布
```

## 2. 核心数据结构定义

### 2.1 `WeeklyInfo`（module-private，orchestrator）
```
字段：
  - reset_ms: i64  — 周窗口重置时刻（epoch **毫秒**，探针原样）
  - pct: f64       — 周窗口已用%（decision-used：临期判定用「周窗口自己的 pct」）
  - max_pct: f64   — max_watch_pct 的当前值（5h 与周取大；**不可服务性判定**用）
不变量：
  - WeeklyInfo 存在 ⇒ 该 key 本轮查到周窗口
  - max_pct ≥ 0
生命周期：每轮 tick 重建（探针数据是瞬时快照）
跨模块共享性：本模块私有（不进 status 快照字段结构）
```

### 2.2 `imminent(key, weekly: Option<&WeeklyInfo>, now_secs, lookahead_hours) -> bool`
```
前置: lookahead_hours ≥ 0
后置: 返回 true ⟺ weekly 存在 ∧ reset_ms ∈ (now, now+lookahead]（毫秒换算）
      ∧ weekly.pct < exhausted ∧ weekly.max_pct < exhausted
    返回 false ⟺ 不满足上列任一
```
- `now + lookahead` 用**毫秒**（`now_secs*1000` 精度低但相差<1s 可忽略；为避免单位混乱，
  实现在 ms 域做 `reset_ms > now_ms` 与 `reset_ms <= now_ms + lookahead_hours*3600_000`）。

### 2.3 `imminent() 判定术语`
- **临期集**（per-tick，决策层）：全部满足上列 4 条件的 key 集合。
- 非临期 = 无周窗口 / 周窗口不临期 / 周 pct ≥ exhausted / max_pct ≥ exhausted 任一满足。

## 3. 模块划分与功能规约

### 3.1 config.rs — `weekly_reset_lookahead_hours: u64`
```
功能：新配置字段（cfg 顶层）。0 = 关闭临期优先策略（行为与旧版等价）。
保证：默认 24；load 时注入 default = 24。
```
**不变式**：`0 ≤ weekly_reset_lookahead_hours ≤ 168`（7 天 = 周窗口周期封顶——超过周窗口无意义，
留 168 上限是防手误；若用户想关是 0，不存在 >168 的合法场景）。

### 3.2 orchestrator.rs — `decide` / `eligible_set` / `imminent` / `tick`（本 feature 主体）
```
decide(ids, pct, weekly_map, now, lookahead, current, pinned, throttle, restore, exhausted) -> Decision
  pre: 全部参数合法（lookahead ≥ 0；weekly_map 键 ⊆ ids）
  post:
    (a) pinned 且该 key ∈ 合格集 ⇒ active = pinned（pin 优先级）
    (b) pinned 越线（门限按档位取——normal=throttle, degraded=exhausted；临期不放宽门限）⇒
        pin_release=Some，回归自动逻辑
    (c) current 且 ∈ 合格集 ⇒ active = current（粘滞；抖动保护：current 查询失败 ⇒ 保持）
    (d) 否则在合格集内：
        临期集非空 ⇒ active = 临期集中 reset_ms 最小；平手取 max_pct 最小
        临期集空 ⇒ 现状：正常档 min_by pct（restore 优先）；降级档 min_by pct
    (e) 无合格者 ⇒ active = None（调用方保留原值）

eligible_set(ids, pct, throttle, exhausted) -> (Vec<i64>, Regime)
  pre: 阈值合法
  post: 正常运行：
    正常档合格集 = {known_ids : max_pct < throttle}
    否则降级档合格集 = {known_ids : max_pct < exhausted}
    全部查询失败 ⇒ (空, Normal)【不变】

imminent(weekly: Option<&WeeklyInfo>, now_ms, lookahead_ms, exhausted) -> bool
  （纯函数，如上）
```

### 3.3 status.rs — 展示层
```
KeyStatus.imminent: bool         — 是否在临期集（决策同源）
StatusSnapshot.weekly_lookahead_hours: u64 — 传递给看板，显示策略开关
前端：卡片「⏳ 临期 · N 小时后重置」徽标（仅 imminent=true）
顶部说明行：weekly_reset_lookahead_hours 的值（0 = 关闭）
```

## 4. 模块间接口规约

| 接口 | 输入 | 输出 | 约定 |
|---|---|---|---|
| `quota::QuotaStatus.weekly` → `orchestrator::WeeklyInfo` | `QuotaStatus` | `Option<WeeklyInfo>` | weekly 缺失 = None；ms 单位原样透传 |
| `orchestrator::Decision` → `status::update` | `Decision` + `weekly_map` | `status::KeyStatus` | `imminent` 由 weekly_map 推导，与 decision 同源 |
| `config` → `orchestrator` | `Config.weekly_reset_lookahead_hours` | 同 | 每轮从 cfg 读（不变，config 不可变） |

## 5. 关键设计决策

| 决策 | 理由 |
|---|---|
| **临期优先只在切换发生时应用**（不主动抢先切） | 保住「能不换就不换」的缓存局部性决策路径；用户拍板 |
| **临期判定只看周窗口**（5h 不参与） | 用户明确「周限额」；5h 是滑动窗口，reset 频繁，引入会疯狂切换 |
| **临期只在当前合格集内排序** | 95% 预防线不变；只有全员都 ≥95% 进入降级档后，95–100% 的 key 才可继续使用 |
| **临期 key 的其他 key 采用「周窗口 pct < exhausted 且 max_pct < exhausted」** | 5h 若 100% = 不可服务，排除（不可服务 = 选了立即 429） |
| **pin 门限不因临期改变** | 正常档仍取 throttle，降级档仍取 exhausted；临期只是选择偏好，不是安全豁免 |
| **不加第 4 档 priority** | 三档已覆盖：临期非 active ⇒ standby(10)；active ⇒ 100。加档引入新不变量，收益低 |
| **lookahead 配置化且 0=关闭** | 用户要的「强/中/弱」调节空间；0 时全路径短路，与旧版逐字节一致（回归保险）；上限 168h（7 天 = 周窗口周期）防手误 |
| **EDF（按 reset_ms 升序）** | 经典单机调度直觉；无需引入评分公式 |

## 6. 架构正确性论证

### goal → 模块映射
- **G1 最大化额度使用效率**（用户目标）→ `orchestrator::imminent` + `decide`（判定+排序主体）
- **G2 不影响非临期选择的旧行为** → `eligible_set`（临期集恒空时全路径短路）+ 回归测试
- **G3 用户可见性** → `status.rs`（卡片徽标 + lookahead 显示）

### 模块协作论证
- G1: `imminent()` 在 tick 纯计算每 key 是否临期（依据 H1/H2/H3/H4）；`decide()` 仅在
  切换发生时（粘滞已排除 current）把临期集按 reset_ms 升序选出 active。组合 ⟹ 选的总是
  「最先清零的、还有余量的、现在能服务的」key。
- G2: `weekly_map` 每轮独立构建；`lookahead=0 ⟹ imminent() = false ∀key`（恒 False 因
  `reset_ms ≤ now + 0` 与 `reset_ms > now` 互斥）⟹ 合格集 = 原式、decide 走旧 min_by ⟹ 旧行为。
- G3: `Decision.eligible` + `weekly_map` → `KeyStatus.imminent`（同源）；前端读取显示。

### 关键假设
- H1（reset 语义）、H2（无周窗口=非临期）、H3（now 与 reset 差比）、H4（只周窗口）、
  H5（lookahead=0 等价旧版）——见研究文档 §5。

### 模块级 invariant
- I1：`Decision.imminent` ⊆ `eligible`（只从当前合格集筛临期候选）— 维护方：decide。
- I2：`Decision.active ∈ eligible ∪ ∅`（活动 key 恒在合格集或空）— 维护方：decide。
- I3：决策身份 key = 主 channel_id（现有）不变 — 维护方：各入口归一化。

## 7. 引用与边界

- 本文档定义架构；`detailed-design.md` 逐函数 6 条 + 论证段。
- **范围外**：5h 窗口临期、priority 第 4 档、探针改动、newapi.rs、main.rs。
