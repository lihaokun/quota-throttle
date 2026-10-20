# 细化设计 — SGM-6 / 面板改回 main 视觉 + 组为主页（panel-main-style）

> 状态：草案，待用户确认（2026-10-20）。确认前不写代码。
> 取代：`e230333` / `42cae58` / `c0e7c50` 三版前端视觉（用户：「全部视觉效果都不满意，还是喜欢 main 版本」）。

## 1. 范围

用户原话要点（2026-10-20）：
1. 视觉**全部**改回 main 版（`main:src/status.rs` 内嵌看板），不是「参考 main 调颜色」。
2. **默认主页是「组」**；每次选一个组；能看到该组**全部来源的用量**和**当前选择**。
3. 加一个 GLM 组（zhipu-6、zhipu-7），和火山组同屏看统一效果。
4. 火山切换逻辑梳理、QueueFull 锁问题**放到本次重构之后**。

**只改前端**：`src/panel_http/ui/{index.html, panel.css, render.mjs, app.mjs, charts.mjs}`。
**不改**：后端任何 Rust 代码、`/state` 与 `/usage/query` 契约、`core.mjs` / `client.mjs` / `contract.mjs`
（契约投影、写入 revision/operation 流程、刷新节奏全部原样）、CSP、资产白名单。

## 2. 视觉基准：搬 main 的什么

逐项照搬 `main:src/status.rs:597-715` 的样式与 DOM 形态：

| main 元素 | 用途 | 本次落点 |
|---|---|---|
| `:root` 色板 `--bg #0b0d12 / --card #151922 / --line / --ok / --warn / --bad / --accent`、径向渐变背景、`.wrap` 1100px | 整体底色与版心 | 全站 |
| `header h1 + .tag`、`.sub` 一行说明 | 顶栏 | 全站 |
| `.chips / .chip / .dot / .copy` | 顶部状态条（后端健康、当前选择、客户端连接可点击复制） | 全站 |
| `.card` + `.act`（活动绿框）/ `.dead`（耗尽半透明）/ `.off`（被禁用红框） | 一个来源一张卡 | 组页、来源页 |
| `.chead .name .tier(.t-active/.t-standby/.t-exhausted/.t-unknown) .cid .badge(.b-on/.b-off)` | 卡头：名字 + 档位药丸 + 渠道号 + 启用徽章 | 同上 |
| `.win` 两列 + `.wlab .l/.p` + `.bar .fill` + `.rst` | 用量窗口：标签、大号百分比、进度条、「x 小时 y 分后重置」 | 同上 |
| `.meta`（卡底一行灰字）+ `.del` | priority（期望/实际不一致标黄）、渠道信息、危险操作 | 同上 |
| `.pbtn` / `.pbtn.on` | 「📌 固定到这把」按钮（不合格置灰 + title 说明原因） | GLM 成员卡 |
| `.ban .ban-warn / .ban-info` | 降级档、pin 自动解除、组未就绪 | 组页 |
| `.hrow + .seg` | 小节标题右侧的分段切换（近 24 小时 / 近 30 天；组切换；页切换） | 全站 |
| `.chart .cb`（div 柱状图，悬停浮层）+ `.xax` + `.cnote` | 用量柱状图 | 组页、数据页 |
| `.tbl` | 按模型表等 | 组页、数据页 |
| `.frow input` + `.sbtn` + `.fhint` | 「添加 key」式表单卡 | 添加来源 / 生成访问 Key |
| `.toast` | 操作失败提示 | 全站 |

**CSP 适配（main 是内联的，这里不能照抄写法，只照抄结果）**：
- main 大量用 `style="width:…"` 与 `onclick=` —— CSP `style-src 'self'` + 禁内联脚本 ⇒ 宽度/颜色一律走 CSSOM（`el.style.width = …`），事件一律 `addEventListener`。
- main 用 `innerHTML` 模板 —— `check:ui` 禁 `innerHTML` ⇒ 继续用 `render.mjs` 的 `node()` 建 DOM。
- 结论：**观感与 main 一致，写法沿用本分支约束**。

## 3. 信息架构

```
┌ quota-throttle  [来源分组]                       [ 组 | 来源 | 数据 ]   ← .seg 页切换，默认「组」
│ 每 5 秒刷新 · 快照 2 秒前                                              ← .sub
│ ● 后端 就绪 │ 火山 → volcano-2 │ GLM → zhipu-6 📌 │ 客户端连接 http://127.0.0.1:13000/v1 (点击复制) │ 操作进度 0
```

- 页面三个：**组（默认）/ 来源 / 数据**。hash 仍是 `#groups / #sources / #data`，无 hash 时落到 `#groups`。
- 「访问 Key」不再单独占一大块，归到**组页里该组下面**（访问 Key 本来就是按组签发的）。

### 3.1 组页（默认主页）

```
组  [ 火山 · 2 ] [ GLM · 2 ]                                   [＋ 创建组]
⚠ 降级档：本组全部来源都越过预防线……（仅 GLM regime=degraded 时）
📌 zhipu-7 的固定已自动解除：用量 96% 越过合格线 95%。          [知道了]

┌ volcano-2  活动  渠道 #2  启用 ─────────────────── ⋯ ┐ ┌ volcano-1  备用  渠道 #1  启用 ───── ⋯ ┐
│ 今日用量                                       37% │ │ 今日用量                         12% │
│ ▓▓▓▓▓▓▓░░░░░░░░░░░░░                               │ │ ▓▓░░░░░░░░░░░░░░░░░░                 │
│ 7.4亿 / 20亿 tokens · 3 小时 12 分后重置            │ │ …                                    │
│ priority 100 · 18 个模型 · 连续失败 0               │ │ priority 10 · …                      │
└────────────────────────────────────────────────────┘ └──────────────────────────────────────┘

┌ zhipu-6  活动  渠道 #3  启用        [📌 固定到这把] ⋯ ┐
│ 5 小时窗口          0%  │ 每周窗口            95%  │
│ ▓░░░░░░░░░░             │ ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓░     │
│ 2 小时 5 分后重置        │ 3 天 4 小时后重置         │
│ priority 100 · max 套餐 · 采样 3 秒前               │
└─────────────────────────────────────────────────────┘

近 24 小时用量（本组）                         [ 近 24 小时 | 近 30 天 ]
┌ ▂▃▅▇▆▃▂▁ …（main 柱状图，悬停显示该小时 token） ┐
按模型（本组）    glm-5.3-flash  ▓▓▓▓▓▓  12.3M …
访问 Key（本组） 名称 · 状态 · 复制 · 启用/停用 · 删除           [生成访问 Key]
＋ 把来源加入本组（同类型来源下拉 + 确认）
```

- **组切换**：`.seg` 一排按钮，一次只看一个组；选中组记在 `this.selectedGroup`（已有字段），刷新不丢。
- **成员卡 = 来源卡**：卡里显示的是「这个成员背后那个来源的用量 + 这个成员在本组的状态」。
- **当前选择**：`selected_member_id` 那张卡是 `.act` 绿框 + 「活动」药丸；其余「备用」。
  顶部 chip 也写「火山 → volcano-2」。
- 用量图/按模型：一次 `POST /usage/query`，`filter.group_id = exact(本组)`（现成能力，单请求）。
  **不做每个来源各一条历史线**：后端只有 1 个查询槽，逐源查 = N 次串行请求，违背「别引入逐渠道轮询」的血泪。

### 3.2 来源页

- 同一套卡片组件，列出**全部来源**（不分组），卡头不显示「活动/备用」（那是组内概念），改显示「属于：火山」。
- 顶部 `.seg` 类型筛选（全部 / GLM / 火山）+ 名称搜索。
- 「添加来源」= main「添加 key」那样的表单卡（`.frow` 两列输入 + `.sbtn` + `.fhint` 说明），按类型切字段。
- 次要操作（编辑模型、重新发现模型、每日策略、暂停/恢复、撤销错误耗尽、归档）放卡片右上角 **⋯ 分类菜单**
  （这是你上一轮在选项里选定的方案，保留；只把菜单样式改成 main 的按钮风格）。

### 3.3 数据页

- main 的 `.hrow`「近 24 小时用量」+ `.seg [近 24 小时 | 近 30 天 | 某一天]` + `.chart` 柱状图 + `.cnote`。
- 下面 `.frow` 一行筛选（来源 / 组 / 访问 Key / 模型），三个 `.tbl` 表（按模型 / 按 Key / 按小时×模型），分页保留。

### 3.4 全局件

- 操作进度、编辑表单、访问 Key 明文槽、通知：功能不动，样式换成 main 的 `.card / .ban / .toast`。
- 刷新节奏不动：状态 5 秒、用量 60 秒、快照 10 秒算陈旧转只读。

### 3.5 修「页面不停刷新」（用户 2026-10-20 追加）

根因（读码确认）：`app.mjs` 的 pump 每 **1 秒** `paint()` 一次；`render.mjs` 里只有来源页有
`data-src-stamp` 防护，**组页、操作区每秒 `replaceChildren()` 整块重建** ⇒ 打开的 ⋯ 菜单自己关掉、
悬停浮层消失、滚动位置/焦点抖动，看起来就是「不停刷新」。

改法：
- **每个区块都加内容戳**（组页、来源页、操作区、顶部 chips、用量区）：戳只含「会改变显示内容的数据」
  （目录、观测值、选中组、身份页、只读/忙碌状态等），戳不变就不碰 DOM。
- **每秒都变的文字不进戳**：「采样 N 秒前」「x 小时 y 分后重置」用 `data-` 标记，
  每次 paint 只改这些节点的 `textContent`，不重建卡片。
- 用量图容器跨重建复用（同来源页现有 `source-usage-content` 的做法），图只在新查询结果到达时重画。
- 验收：打开 ⋯ 菜单停 15 秒不自动关闭；悬停柱状图浮层不闪；数据变化（如 priority 切换）在 ≤5 秒内反映。

## 4. 卡片字段映射（全部来自现有 `/state`，不加后端字段）

| 卡片位置 | GLM 来源 | 火山来源 |
|---|---|---|
| 档位药丸 | 组页：`selected_member_id`→活动，其余→备用；任一窗口 `≥100`→耗尽；`manual_paused`→暂停；观测 `unknown/stale/unavailable`→**未知**（黄） | 组页：`selected_member_id`→活动；`error_exhausted‖limit_exhausted`→耗尽；`manual_paused`→暂停；`runtime=null`→**未初始化**（黄） |
| 用量窗口 | `glm.sources[].five_hour / weekly`：`percentage` + `reset_at_ms` | `(used_input+used_output)/daily_token_limit`，`next_reset_at` |
| 「未知」规则 | 观测非 current ⇒ 显示「查询失败 / 未知」，**绝不显示 0%**（main 同款 `win()` 的 `pct==null` 分支） | `daily_token_limit=null` 或 `runtime=null` ⇒「未知」；`coverage_end=null` ⇒ 标「无覆盖」不是 0 |
| 启用徽章 | `membership.observed_status`：1→启用（绿）；2/3→已被 new-api 禁用（红框 `.off`） | 同左 |
| priority | `desired_priority` 与 `observed_priority` 不一致时标黄「new-api 侧是 X，不一致！」（main 同款） | 同左 |
| 额外 | 临期（`imminent_members`）徽章、套餐 level、采样年龄、📌 固定按钮 | 连续失败次数、模型数 |

阈值红线（main 进度条上的 `.thr`）：GLM 的切换阈值**不在 `/state` 里**，本次**不画**；
要画需要后端把阈值放进 `/state` —— 属于契约变更，不在本次范围。

## 5. 模块改动

| 文件 | 改动 |
|---|---|
| `panel.css` | 整份重写为 main 的样式表（§2 表格），外加 ⋯ 菜单、表单、分页等本分支独有件的 main 风格样式 |
| `index.html` | 顶栏改 main 结构（h1+tag、sub、chips、页切换 seg）；三个 section 顺序改为 组/来源/数据 |
| `render.mjs` | 重写渲染：`card()` / `win()` / `chip()` / `seg()` / `ban()` / `bars()` 等 main 组件的 DOM 版；组页、来源页按 §3 组装。保留 `node` 导出（`secret.test.mjs` 依赖） |
| `app.mjs` | 默认页 `sources`→`groups`；组页挂用量查询（group 过滤）；**不碰** tick/work/operation/secret 等逻辑代码（`sgm6_b_mutations.mjs` 钉了其中原文） |
| `charts.mjs` | canvas 折线 → main 的 div 柱状图（高度经 CSSOM），`modelColor` 保留（测试自引用） |
| `core.mjs` `client.mjs` `contract.mjs` | **不动** |

## 6. 测试计划

- `npm run test:ui`（Node 逻辑测试）：预期不受影响，必须全绿。
- `npm run check:ui`（契约审计）：保留 `Step U1…U10` 标记、禁 `innerHTML/setInterval` 等规则；`--freeze` 基线需更新到本次提交。
- `npm run test:browser`（playwright，chromium/firefox × 1280/390）：**按新结构改写断言**——
  默认页、按钮可访问名（尽量沿用旧名以减少改动）、⋯ 菜单内操作先点开菜单再点；
  `performance.spec.mjs` 的 `canvas.chart 320px` 改为对新柱状图的等价断言。
  另：`c0e7c50` 已使部分断言失效（未跑过），本次一并修正。
  需要先装 playwright 1.64.1 对应浏览器（chromium 1248 / firefox 1555，本机只有 1208/1217，要下载）。
- `scripts/tests/groups-browser.mjs`（真端到端）：组按钮名等标签同步更新。
- `cargo test --release`：UI 资产是 `include_bytes!`，跑全量确认资产预算（≤512KiB/个，≤1MiB 总）与白名单测试通过。
- **人工验收**：本地预览（UI 本地、数据代理 fermat）在 1280 / 390 两个宽度截图给你看，满意后才提交部署。

## 7. GLM 组接入（运维步骤，不改代码，走面板/管理 API）

1. 已实测（2026-10-20）：zhipu-6、zhipu-7 用各自 selector 查用量均成功；**二者与 zhipu-1～5 是同一团队**，
   查到同一份团队额度（5 小时 0%、每周 **95%**）⇒ 组里两张卡数字相同、周额度已近满，可能直接进降级档。
2. `POST /sources` ×2：type=glm，凭据 + `key_headers`（Bigmodel-Organization/Project），模型走 discover。
   注意：**创建时不探活**，用量要等下一轮后台探测才出现（期间显示「未知」）。
3. `POST /groups`：name=`GLM`，newapi_group=`glm`（与火山组的 `volcano` 分开 ⇒ opencode 的 Key 属火山组，流量不会串到 GLM 渠道）。
4. `POST /memberships` ×2 → `/memberships/{id}/ensure` → `POST /groups/{id}/check`（register）。
5. 暂不生成 GLM 组访问 Key（没有客户端要用；需要时在组页一键生成）。

## 8. 用户决定（2026-10-20 已确认）

1. §3 页面结构与组页示意图：**按此实现**。
2. 次要操作：**保留 ⋯ 分类菜单**，只换成 main 风格。
3. 用量图：**换成 main 的 div 柱状图**。
4. 顺序：**先做前端**；预览沿用现在的方式（界面本地起、数据代理到 fermat 13001），**不重启 fermat 后端**。
   GLM 组在前端完成之后再加。

## 9. 实施记录（2026-10-20）

- 落地文件：`index.html`、`panel.css`（整份改为 main 样式表）、`render.mjs`（重写）、`app.mjs`（默认组页、组用量查询、
  新柱状图报告；`tick/work/operation/SecretSlot` 等逻辑与变异测试钉住的原文未动）、`charts.mjs`（canvas → 纯函数 `barModel`）。
  `core.mjs` / `client.mjs` / `contract.mjs` / 后端零改动。
- 独立审核（子代理，只读）发现并已修复：
  1. 目录无组时 `ensureSelectedGroup` 每次读状态都重置用量（数据页每 5 秒重查、慢查询永不出结果）→ 只在选择真变化且处于组页时重置；
  2. 用量查询被其他工作（刷新本地身份等）作废后停在「正在查询」→ 回调都未运行时 1 秒后重排；
  3. 不属于当前组 / 未归组的访问 Key 无处操作、无组时 Key 区消失 → 折叠表「本页其他组或未归组的访问 Key」+ 无组时列全部；
  4. 表格内 ⋯ 菜单被滚动容器裁切、变暗卡片的菜单被下一张卡盖住 → `.tbl-wrap:has(.menu)` 不裁、`.card:has(.menu.open)` 抬层；
  5. 火山来源无覆盖记录时显示绿色 0% → 显示「本周期尚无覆盖记录，累计数不能视为零用量」+ 档位「无覆盖」；
     GLM 窗口过了自身重置时刻不再当作当前值；活动成员用量未知时加「用量未知」徽章；菜单项先关菜单再执行动作。
- 新增浏览器用例 2 个（清单 `sgm6_b_browser_tests.json` 同步）：组页按组过滤查询 + 打开的菜单跨状态刷新不关；
  未知用量不画成 0 + 无组时不反复重查（撤回修复 1 做变异验证：查询 3 次 ≠ 1，用例确实能抓到）。
- 测试：`test:ui` 52/52；`check:ui` 通过；`cargo test --release` 772/0/35（终版资产另跑 `panel_http::ui` 5/0）；浏览器门禁 80/80（20 用例 × chromium/firefox × 1280/390）；性能门禁 4/4。

## 11. 追加：组卡片排行 + 用量「按组」分项（2026-10-20，草案待确认）

### 11.1 用户决定（原话要点）

- 组页顶部改为**一排组卡片**；组要非常醒目，下面内容明确属于哪个组。
- 排序：**钉住的组（最多 4 个）→ 当前在看的组（若未钉住）→ 其余按「上一个整点小时」token 用量降序**；
  当前组一定出现在这一排。最多显示 **5** 张，第 6 张是「更多 / 新建」：可搜索的菜单 + 「＋ 新建组」。
- 钉住**存浏览器**；浏览器**记住上次看的组**，下次默认打开它（组还在的话）。
- 上小时用量走**方案 A**：后端在用量结果里加「按组」分项，一次查询拿到全部组（不逐组查询）。

### 11.2 后端：用量结果加 `groups[]`（契约变更）

依据（已读码）：账本分钟单元 `ledger_minute_usage` 本就带 `group_id` 维度（`src/catalog/ledger/rows.rs:6`），
`UsageAccumulator::push` 已收到 `dims.group`（`src/ledger/analytics.rs:134`），只是没汇总。

| 位置 | 改动 |
|---|---|
| `src/ledger/analytics.rs` | `Working` 加 `groups: BTreeMap<Option<GroupId>, UsageTotals>`；`push_inner` 同 `tokens` 一样做容量预检（`MAX_USAGE_GROUPS = MAX_GROUPS + 1`，含「未归组」null）与精确加法；`finish` 用 `finish_parts` 产出并校验边际和 = total；`UsageReport` 加 `groups: Vec<(Option<GroupId>, UsageTotals)>` |
| `src/panel_http/admin/output/usage.rs` | `Usage` 加 `groups: Vec<Group>`，`Group { group_id: Option<String>, usage: Totals }`（id 按现有惯例序列化为字符串） |
| `schemas/panel-admin-responses.schema.json` | `UsageReport.required` 加 `groups`；`$defs.GroupUsage {group_id: Id|null, usage: Totals}`，`maxItems 101` |
| `src/panel_http/ui/contract.mjs` | 由 `sgm6_b_contract.mjs` 重新生成（不手改） |
| `src/panel_http/ui/core.mjs` | `usageReport` 记录加 `groups`；`validateUsage` 对 `groups` 做与 `tokens` 相同的唯一性 + 边际和校验（不一致整份拒收） |

归属语义与现有「按组过滤」完全相同（同一列）；未归组的消费落在 `group_id: null`。
不新增端点、不改查询请求格式、不增加查询次数。

### 11.3 前端

- **上小时排行**：`app.mjs` 新增独立状态 `rank = {hour, totals: Map<group_id, total>}`；
  查询区间 `[floor(采样时刻/3600)·3600 − 3600, floor(采样时刻/3600)·3600)`，不过滤、`grain: hour`；
  **整点变化才重查**（每小时 1 次），失败 60 秒后重试；走现有工作槽排队，不与组用量图共享结果状态。
  排行未到时：按目录顺序排，卡片显示「上小时 —」（未知不当 0）。
- **排序**：`pins(≤4, 仍存在的) → selected(若未钉) → 其余按上小时 total 降序，平手按目录顺序`；取前 5 张，余下进「更多」。
- **浏览器记忆**（`localStorage`，键 `qt.panel.groups.v1`，值 `{pins: [id…], last: id}`，读写均 try/catch，失败当作空）：
  选组时写 `last`；钉/取消钉写 `pins`；钉满 4 个时其余卡片的 📌 置灰并提示「最多钉住 4 个」。
  打开页面：`last` 仍在目录 → 选它；否则选排序第一的组。
- **组卡片**：组名（大号）· 类型 · 成员数 · 「→ 当前选中来源」· 「上小时 X tokens」· 📌；当前组高亮边框。
- **「更多」菜单**（用户 2026-10-20 修订）：自上而下
  ① 第一行「＋ 新建组」；② 第二行搜索框（按组名 / new-api 分组即时过滤，回到第 1 页）；
  ③ 其余组的**分页列表**（每行：组名 · 类型 · 成员数 · 上小时用量），底部「上一页 · 第 x / y 页 · 下一页」。
  **每页条数按可用高度动态计算**：`max(5, floor((视口高度 − 菜单顶部位置 − 新建/搜索/分页三行高度) / 行高))`，
  窗口尺寸变化时重算（不丢搜索词与当前页的组）；手机窄屏同样最少 5 条（菜单内部滚动兜底）。
  区块重建时保留菜单打开状态、搜索框内容 / 焦点与页码。
- **组标题栏**：卡片下方大号组名 + 类型 + new-api 分组 + 核验状态 + 当前选择，右侧 ⋯（核验 / 加入来源 / 取消固定 / 引用）；
  其下各小节标题都带组名（「火山 · 成员」「火山 · 近 24 小时用量」「火山 · 访问 Key」）。

### 11.4 测试

- Rust：accumulator 按组分项（多组 + 未归组 null + 容量上限 + 边际和）；HTTP 输出含 `groups`；现有 4 处 `UsageReport {…}` 构造补字段。
- Node：`validateUsage` 拒收按组边际不一致（扩充现有 u8 用例，不改用例清单）；fixtures 的 `usageSnapshot` 生成 `groups`。
- 浏览器（新增用例并登记清单）：排序（钉住 → 当前 → 用量）、5 张 + 更多、搜索过滤、钉满 4 个置灰、
  刷新后记住上次的组、排行查询只发 1 次（不逐组）。
- 全量 `cargo test --release`、浏览器门禁、性能门禁；本地预览截图验收后再提交部署。

### 11.5 实施记录（2026-10-20，用户确认「A，存浏览器… 一起改 开始吧」后）

- 后端：`analytics.rs` 按组分项（`MAX_USAGE_GROUPS = 101`）、`output/usage.rs` 输出 `groups`、schema `GroupUsage`、`contract.mjs` 重新生成、
  `core.mjs` 校验（唯一 + 边际和）。两处「归属修复前后报告完全相等」的既有断言（`attribution_tests.rs`、`ledger_attribution_tests.rs`）
  改为：总量 / 按模型 / 按 Key / 按时间不变，按组分项从「未归组」挪进真实组——这是加了按组分项后的正确语义，不是放宽。
- 前端：组卡片排 + 「更多」菜单（新建 → 搜索 → 分页，每页条数仅在打开 / 窗口尺寸变化时按高度算，≥5）、组标题栏、
  状态条只显示当前组、`localStorage` 偏好（钉住 ≤4、上次的组，失效钉住自动清理）、上小时排行（每整点 1 次、不逐组、无组时不查）。
- 快照闪动根治：`readState` 按 `X-QT-Snapshot-Age-Ms` 把下次读取排在下一次发布后约 300ms（≥1s，异常回退 5s）。
  实测 120 秒最大快照年龄 6 秒、从未过期（修前约每 60 秒越过 10 秒线一次）。
- 独立审核第二轮发现并已修：另一处 Rust 断言漏改（会挂）；「更多」翻页点一下菜单就关（全局点击改按派发路径判断，
  已做变异验证）；失效钉住占名额；组卡片排随工作槽忙闲重建打断拼音输入（卡片排改独立内容戳，写按钮可用态原地同步）；
  补 HTTP 输出 `groups` 序列化测试。
- 预览：fermat 仍是旧后端（无 `groups`），本地预览代理临时按组各查一次拼出 `groups`（仅预览用，部署新后端后不需要）。
- 测试：`cargo test --release` 775/0/35；`test:ui` 52/52；`check:ui` 通过；浏览器门禁 88/88（22 用例 × 4）；性能门禁 4/4；
  新增用例对两处关键修复（无组反复重查、翻页关菜单）各做过变异验证。

## 10. 完整性自检

- [ ] 旧版每个功能在新结构里都有位置（§3 逐页对照子代理功能清单）
- [ ] 任何「查不到」路径显示「未知」，不显示 0
- [ ] 无内联 style / 脚本 / innerHTML / setInterval
- [ ] 不新增任何逐来源轮询或额外后端请求
- [ ] 后端与契约零改动（`git diff --stat` 只含 `src/panel_http/ui/**`、测试、本文档）
