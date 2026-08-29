# 调研报告 — key-rotation（周窗口临期优先切换）

> 由子计划 keyrot-1-weekly-reset-first 引入。目标：切换 key 时按周限额重置时间排序，
> 优先烧「即将刷新且还没用完」的 key，最大化额度使用效率。

## 1. 问题定义

现有调度器（`orchestrator.rs::decide`）在切换活动 key 时：
- 合格集 = `pct < throttle(95%)`（正常档）或 `pct < exhausted(100%)`（全员越线 → 降级档）；
- 在合格集内**挑 `max_pct` 最低的**（护缓存局部性：比例最低 = 余量相对最充足 = 能撑最久 = 切换最少）。

**缺口**：这个选择对「周窗口何时重置」完全盲视。两把 key 都是 30%：
- key A 周窗口**2 小时后**重置——烧掉的 30% 下周一清零，现在烧多少都是在「即将消失的额度」里；
- key B 周窗口**6 天后**重置——额度还有 5 天寿命，剩下 70% 还有价值。

现状 pick 30%/2h 与 30%/6d 完全等同对待（甚至 min_by 随机/顺序输赢），而二者「烧哪个更划算」差别巨大：
烧 A = 赚 2h 内的使用效率；烧 B = 让 A 的 30% 注定被清零浪费。

**本 feature 回答**：切换时按「周窗口距重置的时刻」升序优先——先烧最先清零的额度。
这是经典**单机调度 EDF（Earliest Deadline First）**的直觉：deadline = 窗口重置时刻，
每把 key = 一个「有 deadline 的剩余额度作业」，按 deadline 升序处理可最大化完成量
（对利用率<1 的系统最优；本场景不做严格形式化，够用即可）。

## 2. 已有基础设施（数据早已就位，无需新 API）

| 数据 | 现状 |
|---|---|
| 周窗口 percentage | `quota.rs::QuotaStatus.weekly.percentage`，探针已解析 ✓ |
| 周窗口重置时刻 | `RawLimit.next_reset_time` → `WindowStatus.next_reset_time`（epoch **毫秒**，智谱原始字段 `nextResetTime`）✓ |
| 看板展示 | `status.rs::KeyStatus.weekly_reset` + 前端 `left()` 倒计时，已上屏 ✓ |
| 决策函数 | `decide()` 纯函数（`orchestrator.rs:257`），可单测驱动 ✓ |

**结论**：本 feature 是**纯决策层改动**——`quota.rs`（探针）、`newapi.rs`（下发）、`main.rs`（编排）
**零改动**。周窗口重置时刻已随每次探针响应带回，只差 `decide()` 用它排序。

## 3. 窗口语义核实

- 智谱 `quota/limit` 返回的周窗口 = `unit=6, number=1`（周窗口），`nextResetTime` 为窗口结束时刻
  （epoch ms）；到点后该窗口 percentage 归零。看板同字段倒计时（`left(reset)`，`reset` 单位 ms）。
- **两个窗口并存**：5h（unit=3,number=5）+ 周（unit=6,number=1）。`max_watch_pct` 取两者**较大值**
  作为该 key 的「当前用量」（哪条线先到听哪条）。
- 因此「临期」必须只依**周窗口自己的** `percentage` + `nextResetTime` 判定，且必须核 `max_pct < 100`：
  若 5h 窗口已 100%（max_pct=100），该 key 此刻已接不了流量（选了立即 429），算「不可服务」，排除。

## 4. 方案对比

| 方案 | 描述 | 优劣 |
|---|---|---|
| **A. 合格集内临期优先（推荐）** | 合格集与 95%/100% 门限不变；切换时仅在当前合格候选中按周重置时刻升序 | 保留预防线与降级档，只改变安全候选内部顺序 |
| B. 主动抢切（即使现任<95%也切） | 不依赖「切换」事件，检到临期 key 即提前切 | 破坏粘滞政策（缓存 miss 率上升）；覆盖用户第 1 问拍的「只在切换发生时重排」——**否决** |
| C. threshold-自适应（临期放宽 throttle） | 临期 key 的合格线改尽 exhausted，不调整选择顺序 | 更激进的消耗，但「优先」排序没进选择器——两把都临期时仍挑最低，无法表达「先烧最先重置」；且 95–100 的 key 会进正常档打乱降级档语义——**否决** |
| D. 加权评分（剩余额度/距重置时长） | 合成评分取最大 | 引入新评分公式，语义难讲、调参玄学；两个不同量纲（%与时间）强行合成无理论依据——**否决** |

## 5. 关键假设（显式）

- H1: `nextResetTime`（epoch ms）即该窗口的结束/重置时刻，到点清零。来源：`quota.rs` 注释 + 看板
  已用同字段做倒计时（线上真实数据，可信）。
- H2: 周窗口缺失（只返回 5h 窗口，如个人套餐）的 key = **非临期**，永远按 `pct` 参与常规选择
  （用户 2026-08-25 拍板）。
- H3: 用**差集秒数**算临期（`now < reset <= now + lookahead`），不用绝对时长——因为
  `nextResetTime` 是绝对时刻。
- H4: 5h 窗口「临期」不参与——本 feature 专指周窗口（用户原文「周限额」）。
- H5: lookahead=0 ⇒ 临期集恒空 ⇒ 行为与现状**逐字节等价**（回归保险）。

## 6. 推荐方案

方案 A。实现：`orchestrator.rs` 增 `WeeklyInfo`（`reset_ms: i64, pct: f64, max_pct: f64`）与
`imminent()` 纯函数；`tick()` 构建 `weekly_map`（仅当 `weekly` 存在时）；`eligible_set()` 保持旧逻辑，
`decide()` 只在 eligible 中筛临期候选；`min_by` 改「临期升序 → 平手 max_pct 升序」。配置文件加
`weekly_reset_lookahead_hours = 24`（默认；0 = 关闭策略，行为同旧版）。

**代价与兜底**（明说）：
- 临期优先会覆盖 eligible 内的「restore 余量优先」，但不会把正常档 ≥95% 的 key 拉回候选；
  全员越线进入降级档后，原本就允许继续使用 <100% 的 key。
- 建议 lookahead 6–48h（太大 ≈ 按重置时间硬排，等于全年轮回，缓存 miss 剧增）。
