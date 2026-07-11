# 细化设计 — status-ui（状态查询接口 + HTML 看板）

## 1. 范围

**目标**：把 quota-throttle 已持有的运行时状态暴露为 JSON 接口，并附一个自刷新的 HTML 看板，显示：
- new-api 健康状态与地址
- 每把 key：5 小时 / 每周 已用百分比、重置时间、档位（active / standby / exhausted）、channel_id、当前 priority
- 当前活动 key

**覆盖模块 / 函数**：
- **新增** `status.rs`：`StatusSnapshot` / `KeyStatus` 数据结构、`serve()` HTTP 服务、`render_html()`
- `orchestrator.rs`：`tick()` 末尾发布快照到共享状态（新增字段 `snapshot: Arc<RwLock<StatusSnapshot>>`）
- `config.rs`：新增 `status_addr`（默认 `127.0.0.1:3001`，空则不启用）
- `main.rs`：`run_loop` 里与 tick 循环并行 spawn 状态服务

**不在范围**：opencode 插件（后续 feature，`server` 类型纯文本输出，读同一 JSON 接口，无需 TUI 渲染）。

## 2. 与已有代码的复用点

| 复用项 | 说明 |
|--------|------|
| `QuotaStatus` / `WindowStatus`（quota.rs） | 直接取 `five_hour` / `weekly` 的 `percentage` + `next_reset_time` 填快照 |
| orchestrator 的 `pct` / `active` / `applied` | 快照的三项核心数据，tick 内已算出，无需重算 |
| `Config` 的阈值与 `dry_run` | 快照里回显，便于看板标注阈值线 |
| tokio runtime | 状态服务与 tick 循环共用，`tokio::select!` 并行 |
| **依赖决策** | **不引入新依赖**：仅 2 个 GET 路由，用 `tokio::net::TcpListener` 手写最小 HTTP/1.1 响应（~70 行）。理由：引入 axum 为 2 个只读路由不划算；且看板 HTML 须**内联** JS/CSS（遵循「self-contained 工具依赖必须 vendor 内联」教训），无模板引擎需求 |

## 3. 错误处理策略

- **状态服务失败不得影响主循环**：服务在独立 task 中 spawn；bind 失败 → `error!` 日志 + 放弃启用状态服务，**主循环照常运行**（切换是核心功能，看板是附属）。
- **单连接处理出错**（读写失败 / 畸形请求）→ 记 `debug!` 并关闭该连接，不影响其它连接与主循环。
- **快照读锁**：用 `std::sync::RwLock`（快照小、无跨 await 持锁），`read()` poison → 用 `unwrap_or_else(|e| e.into_inner())` 恢复（遵循「长驻写入器的锁 panic 后必须可恢复」教训，避免一次 panic 让看板永久黑屏）。
- **查询失败的 key**：`max_pct = None` + `error = Some(msg)`，看板显示为「查询失败」而非 0%（避免误读为「有额度」）。

## 4. 数据结构定义

### 4.1 `KeyStatus`（模块私有；status.rs）

```
数据结构：KeyStatus

字段：
  - name: String          — key 名（= new-api 渠道名）
  - channel_id: i64       — new-api 渠道 id
  - five_hour_pct: Option<f64>   — 5 小时窗口已用%；None = 本轮未取到
  - weekly_pct: Option<f64>      — 每周窗口已用%；None = 本轮未取到
  - five_hour_reset: Option<i64> — 5 小时窗口重置时间（epoch ms）
  - weekly_reset: Option<i64>    — 每周窗口重置时间（epoch ms）
  - max_pct: Option<f64>  — 监控窗口取最大（决策依据）；None = 查询失败
  - tier: String          — "active" | "standby" | "exhausted" | "unknown"
  - priority: Option<i64> — 本工具最近下发的 priority；None = 未下发过
  - error: Option<String> — 查询失败原因

类型不变量：
  - tier == "unknown" ⟺ max_pct.is_none()（查询失败）
  - tier == "active"  ⟹ 该 key 的 channel_id == snapshot.active_channel_id
  - max_pct.is_some() ⟹ max_pct == max(five_hour_pct, weekly_pct) 中被监控的那些窗口
```

### 4.2 `StatusSnapshot`（模块私有；status.rs）

```
数据结构：StatusSnapshot

字段：
  - updated_at: i64            — 本快照生成时刻（epoch ms）
  - dry_run: bool              — 是否空跑（看板高亮提示）
  - throttle_threshold: f64    — 切换阈值（看板画阈值线）
  - restore_threshold: f64     — 挑新活动 key 的余量线
  - new_api_base: String       — new-api 地址
  - new_api_healthy: bool      — 本轮 new-api 健康探测结果
  - active_channel_id: Option<i64> — 当前活动 key 的 channel_id；None = 无可用 key
  - keys: Vec<KeyStatus>

类型不变量：
  - active_channel_id.is_some() ⟹ keys 中恰有一项 tier == "active"
  - keys 顺序与 config.keys 一致（看板显示稳定，不跳动）

生命周期：
  - 创建：Orchestrator::new 时置初值（updated_at=0、keys 空）
  - 修改：每次 tick() 末尾整体覆盖（不做增量合并——避免陈旧字段残留）
  - 删除：随进程结束
```

**跨模块共享性**：orchestrator（写）→ status 服务（读），经 `Arc<RwLock<StatusSnapshot>>` 共享。

## 5. 模块细化

### 5.1 status.rs（新增）

#### 5.1.1 `serve(addr: String, snap: Arc<RwLock<StatusSnapshot>>)`
- **功能描述**：监听 addr，提供 `GET /api/status`（JSON）与 `GET /`（HTML 看板）。
- **调用关系**：callers: `main::run_loop`（tokio::spawn）；callees: `TcpListener::bind/accept`、`handle_conn`。
- **实现思路**：
  1. `TcpListener::bind(addr)` → 失败则 `error!` 并 **return**（不 panic、不影响主循环，见 §3）。
  2. `loop { accept() }`；每个连接 `tokio::spawn(handle_conn(stream, snap.clone()))`。
  3. accept 出错 → `debug!` 后 `continue`（不退出监听循环）。
- **循环刻画**：accept loop 为**有意的无限循环**（服务常驻），终止条件为进程退出 / task 被 drop。非算法循环，无 invariant 义务。
- **正确性论证**：non-trivial（含循环 + 跨 task 共享状态）。
  - 前置：addr 可解析；snap 已初始化。
  - 论证：bind 成功 ⇒ 监听建立；每个 accept 出的连接独立 spawn ⇒ 单连接阻塞/出错不影响其它连接（H1: tokio task 相互隔离）；snap 为 `Arc` clone ⇒ 读侧不阻塞写侧超过一次 `read()` 的时长（快照小、无跨 await 持锁）。
  - 后置：服务持续可用，或 bind 失败时静默降级（主循环不受影响）。
  - 副作用：占用一个 TCP 端口；无其它状态写入。

#### 5.1.2 `handle_conn(stream, snap)`
- **功能描述**：读一个 HTTP 请求行，按路径返回 JSON 或 HTML。
- **实现思路**：
  1. 读取至多 1 KiB（请求行 + 头足够；**上界防止畸形请求撑爆内存**）。
  2. 解析首行 `GET <path> HTTP/1.1`；解析失败 → 400，关闭。
  3. 分支：
     - `/api/status` → `snap.read()` → `serde_json::to_string` → 200 + `application/json`。
     - `/` 或 `/index.html` → `render_html()` → 200 + `text/html`。
     - 其它 → 404。
  4. 每个分支都写完整响应（含 `Content-Length`）后关闭连接（HTTP/1.1 短连接，`Connection: close`）。
- **分支覆盖**：解析失败 / 三条路由 / 写失败（`debug!` 后返回），全覆盖。
- **退出覆盖**：4 条（400 / 200-json / 200-html / 404）+ 读写失败提前返回。
- **正确性论证**：trivial+（纯请求-响应映射，无共享状态写入；唯一 non-trivial 点是 `read()` 的 poison 恢复，已在 §3 说明）。

#### 5.1.3 `render_html() -> String`
- **功能描述**：返回**自包含**的看板页面（内联 CSS + JS，无外部 CDN）。
- **实现思路**：页面 JS 每 5 秒 `fetch('/api/status')` 并重绘表格：每把 key 一行，5h / 周 各画一条进度条（`throttle_threshold` 处画阈值线），档位用颜色区分（active=绿 / standby=灰 / exhausted=红 / unknown=黄）。
- **显式假设**：浏览器支持 `fetch` + ES6（H2；现代浏览器皆满足）。
- **正确性论证**：trivial（纯字符串常量 + 无逻辑分支）。

### 5.2 orchestrator.rs（修改）

#### 5.2.1 `Orchestrator` 新增字段 `snapshot: Arc<RwLock<StatusSnapshot>>`
- 由 `new()` 接收（main 构造后同时交给状态服务），**不在内部创建**——因为 main 需要同一个 Arc 传给 HTTP 服务。

#### 5.2.2 `tick()` 末尾发布快照
- **功能描述**：把本轮算出的 `pct` / windows / `active` / `applied` 组装成 `StatusSnapshot` 整体写入。
- **调用关系**：callers: 主循环；callees: `SystemTime::now`、`RwLock::write`。
- **实现思路**（接在现有 priority 下发之后，**不改任何决策逻辑**）：
  1. 本轮采集阶段已有 `pct: HashMap<i64,f64>`；**额外保留** `windows: HashMap<i64, QuotaStatus>` 与 `errors: HashMap<i64,String>`（新增两个局部 map，采集时顺手填）。
  2. new-api 健康：`GET {base}/api/status`，3s 超时，成功即 healthy（失败不影响本轮决策，仅入快照）。
  3. 逐 key 组装 `KeyStatus`：
     - `tier`：`Some(id)==active` → "active"；`pct<throttle` → "standby"；`pct` 有值 → "exhausted"；否则 "unknown"。
     - `priority`：取 `self.applied.get(&id).copied()`。
  4. 整体覆盖写入 `*self.snapshot.write() = snap`（**覆盖而非合并**，见 §4.2 生命周期）。
- **显式假设**：`applied` 反映本工具最近一次成功下发的 priority（H3；已有幂等机制保证）。
- **正确性论证**：non-trivial（状态写入 + 跨模块共享）。
  - 前置：本轮决策已完成（active / applied 已更新）。
  - 论证：快照在**决策与下发之后**生成 ⇒ 其 `tier` / `priority` 与 new-api 实际状态一致（下发失败的 key 其 `applied` 不更新 ⇒ 快照显示旧 priority，与 new-api 实际一致，不撒谎）；整体覆盖 ⇒ 无陈旧字段残留（§4.2 不变量）。
  - 后置：`StatusSnapshot` 类型不变量成立（active 恰一项、keys 顺序稳定）。
  - 副作用论证：仅一次 `snapshot.write()`；健康探测是只读 GET；无其它写入。

### 5.3 config.rs（修改）

新增：
```rust
/// 状态看板监听地址。留空则不启用看板（仅跑切换循环）。
#[serde(default = "default_status_addr")]
pub status_addr: String,     // 默认 "127.0.0.1:3001"
```
- **正确性**：trivial（纯配置字段）。

### 5.4 main.rs（修改）

#### 5.4.1 `run_loop` 并行 spawn 状态服务
- **实现思路**：
  1. 构造 `snapshot = Arc::new(RwLock::new(StatusSnapshot::default()))`。
  2. `if !cfg.status_addr.is_empty() { tokio::spawn(status::serve(addr, snapshot.clone())) }`。
  3. `Orchestrator::new(cfg, api, keys, snapshot)`；其余 tick / ctrl_c 循环不变。
- **正确性**：trivial（装配，无逻辑）。

## 6. 完整性自检 checklist

- [x] 所有函数实现思路推导连续（`serve` 3 步 / `handle_conn` 4 步 / `tick` 发布 4 步，无跳步）
- [x] 所有分支已覆盖（handle_conn 4 路由分支；tier 4 分支；bind 成功/失败）
- [x] 所有退出点已刻画（bind 失败降级；4 条 HTTP 响应；读写失败提前返回）
- [x] callee 契约引用显式（`RwLock::read` poison 语义、`applied` 幂等语义 H3）
- [x] 循环刻画（accept loop 为常驻服务循环，终止条件=进程退出；非算法循环无 invariant 义务）
- [x] 上游事实显式列出（H1 task 隔离 / H2 浏览器能力 / H3 applied 语义）

## 7. 验证计划

1. `cargo build` 零 warning。
2. `curl localhost:3001/api/status` → JSON 含两把 key 的真实 5h/周 百分比、tier、priority、active_channel_id。
3. 浏览器打开 `localhost:3001` → 看板显示两把 key 的进度条与档位，5 秒自动刷新。
4. **降级验证**：把 `status_addr` 设为已占用端口 → 日志报 error，**切换循环照常运行**（看板是附属，不得拖垮主功能）。
5. **失败 key 验证**：临时把一把 key 的 selector 去掉 → 看板显示「查询失败」而非 0%。
