# 火山首选因观测新鲜度抖动而反复轮换

2026-10-20，fermat 全新部署运行中发现。本记录在修改生产代码前建立；经用户确认后才改代码。基线 `c0e7c50`。

## 一 现象与复现

**现象（fermat 现网 `quota-throttle serve`）**：一个火山组两成员 m1/m2 都健康——未耗尽、连续异常 0、`auto_ban=0`、`status=1`、用量约每日上界 1%、`volcano_observation_queue` 为空。但 new-api admin 日志里 `Updated channel` 成对出现，间隔几分钟、最短 3 分钟；priority 100/10 在 m1/m2 之间来回换，opencode 流量随之在两把 key 之间切换；成员 observed 字段持续变化，面板频繁重渲。必现，频率取决于采集节奏。部署前基线（2026-10-20 03:22–04:22）：60 分钟内 `Updated channel` 8 条即 4 对，时刻 04:02/04:07/04:13/04:18，约每 5.4 分钟换一次 active。

**最小复现**：固化为 4 条回归测试（`src/catalog/observed_publication_tests.rs`）。在未修改生产代码的 `c0e7c50` 上运行 `cargo test --release --bin quota-throttle observed_publication`：新增 4 条全红，原有 20 条全绿（2 条性能门禁按设计 ignored）。日志 `red-observed-publication.log` 保留。

| 用例 | 输入（两源都健康） | 预期 | 修复前实际 |
| --- | --- | --- | --- |
| ① `observed_publication_stale_ledger_window_keeps_volcano_preferred` | 账本最后观测跨过 `OBSERVATION_MAX_AGE_SECS` 后恢复 | 首选 `[1,4]` 不变，目标不变 | `[2,3]`，两组都被换 |
| ② `observed_publication_recent_scan_gap_keeps_volcano_preferred` | 采集 `begin_scan` 后、页提交前跑一轮，再提交 | 同上 | `[2,3]` |
| ③ `observed_publication_pending_backlog_keeps_volcano_preferred` | 首选源积压 17 条成功观测（单轮只消 16） | 首选不变，积压期间该源无正向许可 | `[2,4]`，积压组被换 |
| ④ `observed_publication_pause_during_stale_window_still_rotates_preferred` | 过期窗口内经 `SetSourcePaused` 暂停首选来源 | 立即换到健康成员 `[2,4]`；暂停源成员目标由控制意图负责，过期轮次不覆盖；0 许可 | `[None, None]`，两组首选都被清空 |

**出错路径**：采集器 `begin_scan` 把 recent 车道 `last_observed_at` 置空，页提交后才恢复（`ledger.rs`，校验见 :63）→ `ledger_ready` 要求 Scanning/Scanned、无 fault、`last_observed_at` 非空且小于 30 秒（`observed.rs:283-296`）→ `fresh=false` → `project_source` 的 `ready = initialized && usable_time && fresh && !pending && type8 && !disabled`（`observed.rs:271-276`）为假 → `volcano_publication.rs:166-182` 把该源 runtime 映射为 `None` → `select_volcano_group` 中 `available` 要求 `state.is_some()`（`source_policy/volcano.rs:507`），候选为空、`keep=false`、首选存为 `None`、游标保留（:516-531）→ 下一轮新鲜时选游标之后的下一个成员（:522-526），两成员时必然换人 → `targets()` 写 `desired_priority` 并清 `applied_revision`（`volcano_publication.rs:63-87`）→ worker 降旧、回读、升新 → `Updated channel`。

**预期 vs 实际**：预期来源只是"观测暂未就绪/有待处理观测"时，只挡正向许可，首选保持、目标不变；实际首选被清空，恢复时换人。

## 二 根因

- **症状**：`Updated channel` 成对抖动；流量在两把 key 之间切换。
- **根因**：`volcano_publication.rs:166-182` 用 `ready` 决定来源状态是否进入组选择器。`ready` 含 `fresh` 与 `!pending` 两个**观测处理瞬态**，于是"日志采集暂未就绪/还有待处理观测"被当成"来源不可用/未知"。设计中 Available 只由来源状态机决定（`sgm-3-source-policies-b.md:44`："不证明供应商健康或日志采集已就绪"）；新鲜度和队列只是正向许可前提（`sgm-4-observed-publication.md:59`），且"ready 门禁失败只跳过正向许可"（:109）。实现偏离设计，属 §7 第二类"算法内部逻辑错误"。
- **主触发**：采集器每轮开新 recent 窗口都会 `begin_scan`，形成页提交前的空窗；火山策略每 5 秒一轮，时常落进空窗（②）。次触发：采集失败或提交被拒导致 10 秒以上空窗（①），以及单轮超过 16 条的积压（③）。
- **措辞诱因**：`sgm-4-observed-publication.md:61`"未知状态传 None"没有定义"未知"。B 设计里 None 只表示 Runtime 未初始化（`b.md:143`）。
- **更正此前判断**：`WARN 来源分组维护暂未完成 code=QueueFull` 不是本 bug 的原因，也不由本 bug 引起。管理请求预算为 1 req/s（突发 2），稳态约 0.4 req/s，一次切换 7~9 个请求，每 2~3 分钟一次远不足以填满队列。该警告指向锁竞争：`refresh_runtime_backend` 持 `automatic_updates` 写锁做网络检查期间，其它 try 锁失败（`worker.rs:314/358/405/994` 均映射 QueueFull）。另行处理，见第六部分"举一反三"。
- 本修复消除根因，不是绕过。

## 三 参考实现对照

- **参考 1：本仓 main 现网调度 `src/orchestrator.rs:28-29, 289-290, 326-327`**。"抖动保护：本轮查询失败的 key 不进合格集（不会被主动选中），也不动它的 priority；但若它恰好是 current 或 pinned，则保持不变——一次瞬时失败不该丢缓存，也不该抖掉用户的 pin。"
  - 同一组输入（现任健康，本轮缺证据）：参考实现保持现任、priority 不动；本实现清空首选，恢复时换人。差异点即根因。
- **参考 2：本仓 GLM 策略 `source_policy/glm.rs:178`** 用量未知时 `.or(active)` 保持现任。GLM 不受本 bug 影响。
- **CLAUDE.md 外部参考**（new-api、opencode、models.dev）不涉及首选黏性：new-api 只按 priority 路由。故以本仓既有实现和设计文档作对照：`architecture.md:144`"火山正常保持首选，只在源不可用/移除或组首次选择时按稳定成员顺序轮转"；`sgm-3-source-policies.md:25`"首选可用则保持"；`sgm-4-observed-publication.md:41`"安全停止不依赖日志新鲜"。

## 四 修复方案

1. `src/catalog/ledger/observed.rs`：`SourceProjection` 新增 `selectable: bool`。`project_source` 中 `selectable = initialized && usable_time && upstream.channel_type == 8`；`ready` 仍为 `selectable && fresh && !pending && !disabled`，语义逐位不变。两处早返回（退休/退役、未初始化 runtime）置 `selectable: false`。
2. `src/catalog/volcano_publication.rs:166-183`：选择器映射由 `s.ready || exhausted` 改为 `s.selectable || exhausted`。
3. **不改** `ready` 及其全部使用点：许可闸 :247、观测截止 :255-258（缺 `observed_at` 会报 Storage 并终止服务）、`targets()` :66 的启用指令。（行号为修复后。）

**为什么这样修**：把观测处理瞬态从"来源可用性"中摘除，回到设计定义；它们仍完整地挡正向许可。不采用"runtime 存在即可选"：非 type8 或周期未开始的来源会被选为永远拿不到许可的首选。

**修改后用复现走一遍**：空窗/过期/积压轮次中来源 `selectable`，成员可用 → `keep=true` → 首选不变，priority 为与原值相同的 Some → `targets()` 比较相等不写 → `changed_* = 0`；`ready` 为假，许可仍为 0。恢复轮同一首选，正常发许可。暂停或耗尽时 `stopped` 由 `manual_paused`/Exhausted 决定，与新鲜度无关，组选择立即换到健康成员（④）；耗尽源成员由 `targets()` 按 `source.disabled` 下发 Disabled，暂停源成员由 `SetSourcePaused` 控制意图负责（`writable()` 排除之）。

**`pending` 一并摘除**：用户指示"先完成紧急修复"，按本节推荐形态实施（用户未逐字回复此项，记录在此）。理由：设计把队列放在许可一侧；首选源流量最大，最容易积压。代价：若积压中含失败事件，该源最长延迟"积压/16 × 约 5.5 秒"才因耗尽被换下，期间不发新许可，属 `architecture.md:154` 已接受的异步观测风险。若只摘 `fresh`，③改为断言"积压期间允许换选"（不推荐）。

## 五 正确性论证

- **根因消除**：选择器输入不再含 `fresh`/`pending`。复现路径上首选成员始终 selectable 且未 stopped，`keep` 恒真，游标不推进，不产生目标变更；不是靠调大窗口掩盖。
- **不变量保持**：
  1. 正向许可只在 `ready`（含新鲜、无积压、backend、组 ready）时发放：:247 未改；`decision.eligible` 只在 :251 与 `source.ready` 合取使用，扩大 eligible 不授予任何许可。
  2. 安全停止不依赖日志新鲜（O4:41）：`stopped` 在选择器内由 `manual_paused`/Exhausted 计算，与 `fresh` 无关（④验证暂停时组选择立即换走）；耗尽源的 Disabled 来自 `targets()` 中的 `source.disabled`，暂停源的 Disabled 来自 `SetSourcePaused`。
  3. 未初始化、非 type8、`now < period_start` 的来源仍不可选：selectable 保留三项条件，runtime 为 None 时仍映射 None。
  4. 恢复来源不抢占现首选：`keep`/游标逻辑未改。
  5. 过期轮次不触发 Storage：`ready` 未改，:255 的 `observed_at` 截止只在通过 :247 `ready` 检查后计算。
  6. 过期时的首次选择：可以选出首选但无许可；Enabled 指令仍要求 `ready`，不 bump `enable_epoch`。
  7. 不产生双 active：过期窗口里可能提前写入新首选的目标，但 planner 无有效许可时对 Enabled 成员返回 `Wait(NotReady)`（`planner.rs:306-311`），升级还需 promotion barrier（:449-454）。
- **无回归**：4 条回归测试；observed_publication 原有 20 条；火山选择器、worker、scenario 测试；默认满核 `cargo test --release` 全量。变异：选择闸改回 `s.ready`，恰好①–④红。
- **已知残余（不阻断，既有模式）**：新成员初始指令为 Preserve（`store.rs:1087`）。若组的首次选择恰逢过期窗口，高于 standby 的 Preserve 成员可无许可被降级，而新首选要等许可，期间可能出现同优先级分流，损失缓存局部性但不涉及安全；`group.ready=false` 时今天已有同样模式。

## 六 测试用例清单

| 类型 | 用例 | 状态 |
| --- | --- | --- |
| 回归 | ① 过期 30 秒窗口 | 已加；修复前红，修复后绿 |
| 回归 | ② `begin_scan` 空窗（生产主触发） | 已加；修复前红，修复后绿 |
| 回归 | ③ 首选源 pending 积压（按推荐一并摘除 pending，见第四部分） | 已加；修复前红，修复后绿 |
| 新增 | ④ 过期窗口内暂停仍换首选 | 已加；修复前红，修复后绿 |
| 保持 | observed_publication 原有 20 条 | 修复后 24/24 绿 |
| 变异 | 选择闸改回 `s.ready`（修复前口径） | 恰好 ①–④ 4 条红、其余 20 条绿，已还原 |
| 保持 | 默认满核 `cargo test --release` 全量 | 见第七部分提交说明 |

**测试编写更正**：④初稿用直接 SQL 改 `manual_paused` 并断言暂停源成员变 Disabled，修复后在该断言失败。核对 `writable()`（`volcano_publication.rs:37-45`，排除 `manual_paused`）与 O4:61"暂停/退役由原控制意图优先"后确认：禁用暂停源成员是 `SetSourcePaused` 控制命令的职责，策略轮次本就不写，原断言属预期错误，不是本修复引入的回归。终稿改走真实控制命令，断言首选立即换走、暂停源成员目标不被过期轮次覆盖、0 许可。

**举一反三（同类风险，不在本次修复内，需另立并回 §4 定设计）**：

1. `begin_scan` 清空 `last_observed_at`，使所有火山正向许可与访问 Key 正向准入（`planner.rs:309`、sgm-5 C `positive_admission`）周期性闪断。需决定是否跨 `begin_scan` 保留上一代页的观测时间。
2. `refresh_runtime_backend` 持 `automatic_updates` 写锁跨网络检查（约 2~4 秒，每约 30 秒）。导致 GLM 循环 try 锁报 QueueFull，并使采集器提交被拒、整页丢弃，形成 10 秒以上的不新鲜空窗。`sgm-7-runtime-release.md:57` 规定持锁，需改设计。
3. `来源分组维护暂未完成` 警告不含调用方，无法区分来源。可作独立小修。
4. GLM 不受影响（用量未知保持现任），已核对。

## 七 代码更新清单

| 文件 | 位置 | 改动 | 状态 |
| --- | --- | --- | --- |
| `src/catalog/ledger/observed.rs` | `SourceProjection`；`project_source` 三处构造 | 新增 `selectable`，`ready` 逐位等价 | 已改 |
| `src/catalog/volcano_publication.rs` | :166-183 | 选择映射改用 `selectable` | 已改 |
| `src/catalog/observed_publication_tests.rs` | 新增 helper + 4 条测试 | 回归与新增用例 | 已加 |
| `scripts/contract_audit/sgm4_observed_publication.sh` | 精确测试清单 | 不改：该脚本钉在 O4 基线 `ce23dc7`、只允许 O4 路径，自 `f7e54a2` 后从未维护，后续单元在当前 HEAD 本就不满足其路径白名单；本修复回归证据记录于本文件与全量运行 | 不适用 |

## 八 文档更新清单

| 文档 | 更新 | 状态 |
| --- | --- | --- |
| `subplans/sgm-4-observed-publication.md:61` | 明确只有来源不可承载首选才传 None；新鲜度与队列只限正向许可，不得折叠成 None | 已改 |
| `subplans/sgm-3-source-policies-b.md:139` | 补"调用方不得把观测新鲜度、待处理队列等采集瞬态折叠成 None" | 已改 |
| `implementation-plan.md` | 进度记录本修复 | 随提交更新 |
| `architecture.md` | 原文正确（:144），无需改 | — |
| `docs/audits/sgm-4-facts-analytics/observed-publication-expectations.md` | 已核对 :15/:19 只写"原组选择/许可"，无旧口径，无需改 | — |
