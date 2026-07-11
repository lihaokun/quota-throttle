# 子计划 statusui-2 — new-api 面板（渠道实况 + 请求流水 + 连接信息）

## 1. 范围与动机

**动机（补盲区）**：现有看板只展示**智谱侧**的额度（桶里剩多少水），完全看不见 **new-api 侧**的两件事：

1. **渠道到底通不通**——本工具只改 `priority`、**从不碰 `status`**。若 new-api 自己把某渠道禁用（如 401 触发 auto-ban），我们把它设成 `priority=100` 也没用：**流量根本不去，而我们现在毫无察觉**。这是本设计的首要价值。
2. **流量是否真的在走 new-api、落在活动 key 上**——链路是否闭环，眼下只能翻日志。

**覆盖**（扩展 status-ui feature，契约变更 = status JSON 新增字段）：
- `newapi.rs`：新增 `list_channel_states()`、`recent_logs(n)`
- `status.rs`：`StatusSnapshot` 新增 `channels` / `recent` / `client_endpoint`；看板新增 new-api 面板
- `orchestrator.rs`：tick 内拉取上述两项并入快照

**不在范围**：opencode 插件（后续 feature，读同一 JSON）。

## 2. 与已有代码的复用点

| 复用项 | 说明 |
|--------|------|
| `NewApiClient`（会话/Bearer 鉴权、apply_headers） | 直接复用，新增两个 GET 方法即可，**零新依赖** |
| 现有 `list_channels()`（name→id） | 保留（sync 用）；新增 `list_channel_states()` 解析完整字段，**不改原方法签名** |
| `status::publish` / 快照整体覆盖机制 | 新字段随快照一起整体覆盖 |
| 看板 CSS 变量与卡片样式 | new-api 面板复用同一套 tokens |

## 3. 错误处理策略

- **两次新增拉取失败不得影响切换**：`list_channel_states` / `recent_logs` 出错 → `debug!` 记录 + 该字段留空（`channels: vec![]` / `recent: vec![]`），**决策与 priority 下发完全不受影响**（它们在此之前已完成）。
- 看板对空数据显示「暂无数据」，不报错。
- 日志条目解析：字段缺失用 `unwrap_or_default()`，单条畸形不影响整批（逐条 `filter_map`）。

## 4. 数据结构定义

### 4.1 `ChannelState`（跨模块：newapi 产出 → status 消费 → JSON 对外）

```
数据结构：ChannelState
字段：
  - id: i64                — 渠道 id
  - name: String           — 渠道名（= key name）
  - enabled: bool          — status == 1
  - status_raw: i64        — 原始 status（1=启用；2=手动禁用；3=自动禁用 …）
  - priority: Option<i64>  — new-api 中的实际 priority（**权威值**，用于与本工具 applied 对账）
  - weight: Option<i64>
  - used_quota: i64        — new-api 累计计费额度（内部单位）
  - auto_ban: Option<i64>  — 是否允许 new-api 自动禁用该渠道
  - models: String
  - group: String

类型不变量：
  - enabled ⟺ status_raw == 1
```

> **计费单位说明**：`used_quota` 是 new-api 内部单位。经实测对照（日志 `quota=316163` ↔ `＄0.632326`），默认 `QuotaPerUnit = 500000`，即 `$ = used_quota / 500000`。看板按此换算并标注「按 new-api 默认计价」——**若用户改过 QuotaPerUnit，此换算会偏**，故同时保留原始值。

### 4.2 `RequestLog`（同上）

```
数据结构：RequestLog
字段：
  - created_at: i64        — epoch **秒**（注意：与快照 updated_at 的毫秒不同单位）
  - channel: i64           — 实际服务的渠道 id  ← 本面板的核心：证明流量落在活动 key 上
  - channel_name: String
  - model_name: String
  - prompt_tokens: i64
  - completion_tokens: i64
  - quota: i64             — 本次计费额度
  - use_time: i64          — 耗时（秒）
  - is_stream: bool
  - token_name: String     — 哪个调用令牌发起的（如 "opencode"）

类型不变量：
  - 仅收录**真实请求**：`model_name` 非空 且 `channel != 0`
    （/api/log/ 混有登录等系统日志，实测 type=7；按上述两字段过滤比依赖 type 枚举更鲁棒）
```

### 4.3 `StatusSnapshot` 扩展（契约变更）

```rust
    // 新增三项
    pub client_endpoint: String,     // B：客户端应连的地址 = new_api_base + "/v1"
    pub channels: Vec<ChannelState>, // C
    pub recent: Vec<RequestLog>,     // A（最近 N 条，N=20）
```
向后兼容：仅**新增**字段，既有消费者不受影响。

## 5. 模块细化

### 5.1 newapi.rs

#### 5.1.1 `list_channel_states(&self) -> Result<Vec<ChannelState>>`
- **调用关系**：callers: `orchestrator::tick`；callees: 已有 `apply_headers` + reqwest。
- **实现思路**：
  1. `GET {base}{channel_path}/?p=0&page_size=100`（与 `list_channels` 同一端点，**独立解析**完整字段）。
  2. 取 `data.items`（新版）或 `data`（旧版数组）—— 复用既有兼容逻辑。
  3. 逐条 `filter_map` 成 `ChannelState`；`id`/`name` 缺失则丢弃该条（无法定位的渠道无展示意义）。
  4. 按 `id` 升序（看板顺序稳定，不跳动）。
- **分支覆盖**：请求失败 → Err；两种 data 形态；单条字段缺失 → 跳过。
- **正确性论证**：non-trivial（跨进程调用）。
  - 前置：已 `authenticate()`（callee 契约：apply_headers 附带会话/Bearer）。
  - 论证：端点与 `list_channels` 同源 ⇒ 鉴权与兼容性已验证；只读 GET ⇒ 无副作用；按 id 排序 ⇒ 满足「顺序稳定」不变量。
  - 后置：返回按 id 升序的渠道状态列表。
  - 副作用：无（纯读）。

#### 5.1.2 `recent_logs(&self, n: usize) -> Result<Vec<RequestLog>>`
- **实现思路**：
  1. `GET {base}/api/log/?p=0&page_size={n}`。
  2. 同上取 items。
  3. **过滤**：`model_name` 非空 且 `channel != 0`（剔除登录等系统日志，见 §4.2 不变量）。
  4. 按 `created_at` **降序**（最新在前）。
- **显式假设**：H1 — `/api/log/` 默认按时间倒序返回；为不依赖该假设，**本函数自行再排序一次**（防御性，成本 O(n log n)，n=20 可忽略）。
- **正确性论证**：non-trivial（跨进程 + 过滤）。
  - 论证：过滤条件基于**字段语义**而非 `type` 枚举值 ⇒ 不因 new-api 版本改动枚举而失效（鲁棒性优于依赖枚举）；自行排序 ⇒ 不依赖 H1。
  - 副作用：无（纯读）。

### 5.2 orchestrator.rs

#### 5.2.1 `tick()` 快照组装处新增两次拉取
- **实现思路**（插在 §5.2 现有「发布快照」之前，**决策与下发已完成**）：
  1. `let channels = self.api.list_channel_states().await.unwrap_or_default();`（失败 → 空 vec + `debug!`）
  2. `let recent = self.api.recent_logs(20).await.unwrap_or_default();`
  3. `client_endpoint = format!("{}/v1", base_url.trim_end_matches('/'))`
  4. 一并写入快照。
- **显式假设**：H2 — 这两次拉取在**决策之后**，故其失败不可能影响 priority 决策（顺序保证）。
- **正确性论证**：
  - 前置：本轮决策与 priority 下发已完成（H2）。
  - 论证：两次拉取均 `unwrap_or_default()` ⇒ 失败退化为空列表，不 propagate 错误、不中断 tick ⇒ 「看板绝不拖垮主循环」原则保持（与 status-ui §3 一致）。
  - 副作用：两次只读 GET（本地 new-api，每 60s 一次，可忽略）。

### 5.3 status.rs

#### 5.3.1 结构体扩展
- `ChannelState` / `RequestLog` 新增；`StatusSnapshot` 加三字段（§4.3）。trivial。

#### 5.3.2 `render_html()` 新增 new-api 面板
- **布局**（接在 key 卡片之后）：
  - **渠道实况表**（C）：每行一个渠道 —— 名称 · **启用状态徽章**（启用 / **已被 new-api 禁用** ← 红色醒目）· new-api 侧 priority（**与我们下发的对账**，不一致时高亮警示）· weight · 累计花费（$，标注「按默认计价」）· auto_ban
  - **最近请求流水**（A）：时间 · 模型 · **落在哪个渠道**（若 == 活动 key 显示绿点，否则黄点提示「掉到了兜底渠道」）· tokens(in/out) · 耗时 · 令牌名
  - **连接 chip**（B）：`客户端连接: {client_endpoint}`，点击复制
- **关键交互语义**：
  - 渠道 `enabled == false` → 整行红底 + 文案「**已被 new-api 禁用，priority 不起作用**」（这正是本子计划的首要动机）
  - 请求落在非活动渠道 → 黄点 + tooltip「未走活动 key（可能活动 key 撞墙后兜底）」
- **正确性**：trivial（纯渲染，无逻辑分支外的状态）。

## 6. 完整性自检 checklist

- [x] 推导连续（两个新方法各 4 步；tick 插入点 4 步）
- [x] 分支覆盖（请求失败 / 两种 data 形态 / 单条畸形 / 过滤条件 / enabled 与否 / 落在活动渠道与否）
- [x] 退出覆盖（Err / Ok(空) / Ok(有数据)；tick 内 unwrap_or_default 退化）
- [x] callee 契约引用（`apply_headers` 需已 authenticate；`list_channels` 的 data 兼容逻辑复用）
- [x] 循环刻画（items 遍历有界；无算法循环）
- [x] 显式假设（H1 日志排序不依赖、H2 拉取在决策之后、QuotaPerUnit 换算假设已标注）

## 7. 验证计划

1. `cargo build` 零 warning。
2. `curl /api/status` → 新增 `channels`（两条，`enabled=true`、priority 与我们下发的 100/10 一致）、`recent`（含刚才 opencode 那次 glm-4.5-air 请求、`channel=2`）、`client_endpoint`。
3. 看板显示 new-api 面板：渠道实况 + 请求流水（那条请求应显示**绿点**=落在活动 key）。
4. **盲区验证（本子计划的核心价值）**：**禁止拿在用的 key 做实验**（禁用活动 key 会真的中断服务）。
   正确做法：临时建一个**假渠道** `zhipu-test`（key 随便填、不进 config.keys）→ 在 new-api 后台禁用它 →
   看板该行应变红并提示「已被 new-api 禁用，priority 不起作用」→ 验证后**删除该假渠道**。
   全程不触碰在用的两把 key。

> **副作用声明**（源码核实，非断言）：本子计划新增的两个拉取均为**纯读**——
> `GetAllChannels` 与 `/api/log/` handler 中**写操作命中数为 0**（只有 `Find` 查询），
> 不触发渠道测试、不消耗额度、不改任何 new-api 状态。成本为每轮两次本地 GET，可忽略。
