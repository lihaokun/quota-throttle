# 子计划 statusui-3 — 实时指标（替换请求流水表）

## 1. 范围与动机

**动机**：statusui-2 的「最近请求流水」列一堆原始请求，**信息密度低、看着怪**，而用户真正要回答的问题是两个：

1. **有没有连上？**（流量到底在不在走我们的网关）
2. **最近消耗多少？**（正在烧多快）

这两个问题 new-api **有现成答案**：`GET /api/log/stat?channel=N` 返回 `{quota, rpm, tpm}`，其中
**`rpm`/`tpm` 统计的是最近 60 秒**（源码 `model/log.go`：`// 只统计最近60秒的rpm和tpm`，
`Where("created_at >= ?", time.Now().Add(-60*time.Second))`），且**支持 `channel_id` 过滤**
（`rpmTpmQuery.Where("channel_id = ?", channel)`）。

> **为什么不做「按渠道累计 tokens」**：new-api 的 `quota_data` 表虽有 `channel_id`，但**所有导出接口都
> GROUP BY 掉了 channel**（`GetAllQuotaDates: Group("model_name, created_at")`），只能直读 SQLite（需新依赖）。
> 而且它**信息冗余**——「每把 key 消耗了多少」由**智谱的 5h/周 百分比**权威回答（更准，连 new-api 之外的
> 消耗都算）。故明确不做。

**覆盖**：
- `newapi.rs`：新增 `channel_rate(channel_id) -> (rpm, tpm)`
- `status.rs`：`KeyStatus` 新增 `rpm`/`tpm`/`last_request_at`/`last_request_model`；看板 key 卡片加实时指标；**删除**请求流水表
- `orchestrator.rs`：**拆分刷新循环**——见 §5.3
- `config.rs`：新增 `panel_interval_secs`（默认 5）

## 2. 与已有代码的复用点

| 复用项 | 说明 |
|--------|------|
| `NewApiClient` 会话鉴权 | 新增方法直接复用 `apply_headers` |
| `recent_logs(1)` | 取最后一条日志 → 「最后一次请求几秒前 / 什么模型」 |
| `status::publish` 快照机制 | 面板循环单独发布 |
| 现有 key 卡片 / 渠道状态表 | 保留不动 |

## 3. 错误处理策略

- `channel_rate` 失败 → 该 key 的 `rpm`/`tpm` 置 `None`（看板显示 `—`），**不影响切换、不影响其它 key**。
- **面板循环与切换循环完全隔离**：面板循环整个 task 挂掉也不影响切换（切换循环独立 tick）。
- 频率：面板 5s × N 把 key（本地回环、纯读、毫秒级）。对网关无影响（已核实所有相关 handler 写操作命中数为 0）。

## 4. 数据结构定义

### 4.1 `LiveMetric`（**独立结构，不塞进 KeyStatus**）

⚠️ **关键设计约束**：实时指标**不能**作为 `KeyStatus` 的字段——因为决策循环每轮会**整体重建** `keys`
（`StatusSnapshot.keys = key_statuses`），会把面板循环写进去的 rpm/tpm **冲掉**。
必须放在**独立字段**，让两个循环写的字段严格**不相交**（§5.3 的并发不变量赖此成立）。

```rust
pub struct LiveMetric {
    pub channel_id: i64,
    /// 最近 60 秒该渠道的请求数（new-api /api/log/stat?channel=N，窗口由服务端固定）
    pub rpm: i64,
    /// 最近 60 秒该渠道的 tokens
    pub tpm: i64,
    /// 该渠道最后一次请求的时间（epoch 秒）。None = 近期日志里无该渠道记录
    pub last_request_at: Option<i64>,
    pub last_request_model: Option<String>,
}
// StatusSnapshot 新增： pub live: Vec<LiveMetric>,   ← 面板循环独占写
```

**语义**：`rpm > 0` ⟺ 最近 60 秒有流量走这把 key（**「有没有连上」的直接答案**）；
`tpm` ⟺ 最近 60 秒烧掉的 tokens（**「烧多快」的直接答案**）。看板按 `channel_id` join 到 key 卡片。

**语义**：
- `rpm > 0` ⟺ 最近 60 秒有流量走这把 key（**「有没有连上」的直接答案**）
- `tpm` ⟺ 最近 60 秒烧掉的 tokens（**「烧多快」的直接答案**）

### 4.2 `StatusSnapshot` 变更

- **删除** `recent: Vec<RequestLog>`（请求流水表随之删除）
- `RequestLog` 结构体保留（`recent_logs` 仍用于取「最后一次请求」），但不再进快照

## 5. 模块细化

### 5.1 newapi.rs

#### 5.1.1 `channel_rate(&self, channel_id: i64) -> Result<(i64, i64)>`
- **功能**：取某渠道最近 60 秒的 (rpm, tpm)。
- **调用关系**：callers: 面板循环；callees: `apply_headers` + reqwest。
- **实现思路**：
  1. `GET {base}/api/log/stat?type=2&channel={id}&start_timestamp=0&end_timestamp=<now>`
     （`type=2` = LogTypeConsume；rpm/tpm 的 60 秒窗口由 new-api 内部固定，start/end 只影响 `quota` 字段，我们不用它）
  2. 解析 `data.rpm` / `data.tpm`，缺失则 0。
- **分支覆盖**：请求失败 → Err；字段缺失 → 0。
- **正确性论证**：non-trivial（跨进程读）。
  - 前置：已 authenticate。
  - 论证：纯 GET（已核实 `SumUsedQuota` 无写操作）；60 秒窗口由服务端保证，客户端无需计时。
  - 副作用：无。

### 5.2 status.rs
- `KeyStatus` 加 4 字段；`StatusSnapshot` 去 `recent`。
- 看板：key 卡片头部加一行**实时指标**——
  `● 3 req/min · 12.4k tok/min · 最后请求 8 秒前 (glm-5.2)`
  - `rpm > 0` → 绿色脉冲圆点（**在跑**）；`rpm == 0` 且最后请求 > 5 分钟 → 灰点（**空闲**）
  - 活动 key 若长时间 `rpm == 0` 而其它 key 有流量 → 说明流量没走活动 key，值得警觉
- **删除**请求流水表整块。

### 5.3 orchestrator.rs —— 拆分双循环（本子计划的结构性改动）

**问题**：现有实现把「智谱用量轮询」和「new-api 面板数据」绑在同一个 60s tick 上。
智谱是**外部 API**（该低频、有速率限制），new-api 是**本地**（可高频）。绑一起导致面板 60 秒才动一次。

**改法**：
- `tick()`（**切换循环**，`poll_interval_secs`=60）：智谱用量 → 决策 → 下发 priority → 发布**决策部分**快照。
- `panel_tick()`（**面板循环**，`panel_interval_secs`=5）：渠道状态 + 每 key 的 rpm/tpm + 最后请求 + 内部余额
  → **只更新快照的面板部分**，**不碰任何决策状态**（`active` / `applied` 只读）。

**并发安全**：两循环都写同一个 `Arc<RwLock<StatusSnapshot>>`。
- **不变量**：面板循环**只写面板字段**（`channels` / `rpm` / `tpm` / `last_request_*` / `newapi_user_quota`），
  **绝不写**决策字段（`active_channel_id` / `tier` / `priority` / 用量百分比）。
- 实现上：面板循环 `read` 出当前快照 → 只改面板字段 → 写回。因两者写的字段**不相交**，
  且 `RwLock` 保证写互斥，故不会丢失对方的更新（**读-改-写的原子性由写锁保证**）。
- **显式假设** H1：面板循环持锁时间极短（仅字段赋值，无 await），故不阻塞切换循环的发布。

**正确性论证**（`panel_tick`）：
- 前置：快照已初始化。
- 论证：面板字段与决策字段**不相交**（见上不变量）⇒ 两循环的读-改-写不会互相覆盖有效数据；
  面板循环失败退化为保留旧面板字段（不清空），不影响决策字段。
- 副作用：一次 `snapshot.write()`；N+2 次本地只读 GET。

### 5.4 config.rs
```rust
/// 状态看板面板数据的刷新间隔（秒）。new-api 是本地服务，可高频；
/// 与智谱用量轮询（poll_interval_secs）**分离**——后者是外部 API，该低频。
#[serde(default = "default_panel_interval")]
pub panel_interval_secs: u64,   // 默认 5
```

## 6. 完整性自检 checklist

- [x] 推导连续（`channel_rate` 2 步；`panel_tick` 读-改-写 3 步）
- [x] 分支覆盖（请求失败 / 字段缺失 / rpm>0 与 ==0 的展示分支）
- [x] 退出覆盖（Err → 该 key 指标置 None；面板循环整体失败不影响切换）
- [x] callee 契约引用（`apply_headers` 需 authenticate；`SumUsedQuota` 的 60 秒窗口语义）
- [x] 循环刻画（面板循环为常驻服务循环，终止=进程退出）
- [x] 显式假设（H1 持锁极短；rpm/tpm 窗口由服务端固定为 60s——**已核实源码**）

## 7. 验证计划

1. `cargo build` 零 warning。
2. 静默时：所有 key `rpm=0`，看板显示灰点「空闲」。
3. 发一发请求 → **5 秒内**看板上活动 key 变绿点、`rpm≥1`、`tpm>0`、「最后请求 X 秒前」开始跳。
4. 切换循环不受影响：`poll_interval_secs=60` 的决策日志频率不变。
5. 面板循环失败（临时停掉 new-api 的管理鉴权）→ 面板字段保留旧值，**切换照常**。
