# 细化设计 — panel-control（可写看板 + 用量历史 + 调度降级档）

## 0. 调研结论（本次实测/查证得出，后续勿再凭记忆猜）

| # | 结论 | 依据 |
|---|------|------|
| 0.1 | 智谱 `TOKENS_LIMIT` 窗口**只返回整数 `percentage`**，没有 `usage` / `remaining`（那两个字段只出现在被我们过滤掉的 `TIME_LIMIT`／MCP 搜索计数上） | 实测两把真 key 的原始返回 |
| 0.2 | ⇒「真·用尽」只能盯 `percentage`，**分辨率就是 1%**，无法做更细的余量判断 | 由 0.1 推出 |
| 0.3 | new-api 的 `quota_data` 是**小时桶**（`created_at - created_at % 3600`），由 `UpdateQuotaData()` 后台 goroutine **每 `DataExportInterval` 分钟批量刷库**，默认 **5 分钟**，`DataExportEnabled` 默认 `true` | 源码 `model/usedata.go:41-100`、`common/constants.go:68-70`（v1.0.0-rc.20） |
| 0.4 | `quota_data` **没有滞后**：与 `logs`(type=2) 按小时分桶后**逐桶、逐渠道、逐 token 完全一致**（此前「滞后 1.5h」的怀疑是把非消费日志算进来了，**已证伪**） | 实测本机 `.newapi/one-api.db` |
| 0.5 | ⇒ 历史视图的刷新周期定为 **5 分钟**（= 源头落库节奏）。整点刷会让「今天」那根柱最多旧 1 小时；5 秒刷则浪费 16 倍 | 由 0.3 / 0.4 推出 |
| 0.6 | `quota_data` 表**有 `channel_id` 列**，但 new-api 的 `GET /api/data/` 是 `Group by (model_name, created_at)`，**把渠道维度压掉了** | 源码 + 表结构 |
| 0.7 | ⇒ 本次历史视图**不需要按渠道拆分**（沿用现有 24h 图的口径），故继续走 HTTP `/api/data/`，**不引入 rusqlite**。将来若要「哪把 key 烧的」，唯一的路是直读 SQLite | 由 0.6 推出 |

## 1. 范围

五项变更，其中 A 是调度机制修补，B 是基础设施，C/D 建在 B 之上，E 独立。

**A. 调度：合格集分档 + 降级档榨干**（`orchestrator.rs`）
现状漏洞：选择阶梯只有 `<restore` 和 `<throttle` 两级，全员 ≥95% 时两级都空 → `active = None` → 走「保留原活动」分支，**哪怕原活动已经 100% 也继续钉着它**，每个请求先撞一次 429 再靠 priority 阶梯跌走。这正是本项目要消灭的东西。
修法：把「95%」这条线**拆成两条**——
- `throttle` = 95%：**预防线**。还有余量时提前换走（护缓存、避免撞墙）。正常档规则不变。
- `exhausted` = 100%：**真·用尽线**。物理事实。
并引入**合格集**概念（见 §4.1）。

**B. 控制通道：看板从只读变可写**（`status.rs` + `orchestrator.rs` + `main.rs`）
HTTP handler → `mpsc::Sender<Command>` → orchestrator 在 `select!` 里消费并立刻跑一轮 tick。
**保住不变量**：`keys` / `active` / `applied` / `pinned` 仍然只有 orchestrator 一个写者，全程无锁。

**C. pin：合格集内的偏好，不能覆盖自动逻辑**（`orchestrator.rs` + 前端）
pin 只能在**自动逻辑判定为合格**的 key 里表达偏好（覆盖「粘滞 + 挑 pct 最低」这个选择偏好），**不能把不合格的 key 拉上来用**。pin 是优先级，不是安全豁免。

**D. 面板加/删 key**（`status.rs` + `config.rs` + `orchestrator.rs`）
探活 → 建渠道 → `toml_edit` 写回 `config.toml`（保注释保格式）→ 热加载进轮询。

**E. 用量历史范围**（`status.rs` + 前端）
新增 `GET /api/usage?start&end`，前端加「近 30 天」视图与「点某天下钻到小时」。

**不在范围**：
- **pct 时序采样**——智谱无历史接口（0.1），要自建时序库；用户明确只要现有 token 图加范围选项，不做。
- **按渠道拆分用量历史**——new-api HTTP 接口无此维度（0.6），要直读 SQLite；不做。
- **删除 new-api 渠道**——删 key 只「停止调度」，渠道留着（身上挂着历史用量与日志）。

## 2. 与已有代码的复用点

| 复用项 | 说明 |
|--------|------|
| `QuotaProbe::query`（quota.rs） | **加 key 时的探活**直接复用。CLAUDE.md 里那三个坑（缺 `type=2` / 缺 `Bearer` / 缺 org-project selector）全都表现为「limits 为空」或「当前用户不存在 coding plan」，而一把 selector 配错的 key 会被永远当成 0% 用量、**永不切换**。探活把这个错挡在录入口 |
| `NewApiClient::ensure_channel`（newapi.rs） | 加 key 时建渠道，原样复用（已幂等） |
| `NewApiClient::usage_data`（newapi.rs） | 历史接口的唯一数据源，签名不变（`start, end` → `Vec<(model, hour, tokens, count)>`） |
| `status.rs` 手写 HTTP server | 扩展而非重写：加 POST/DELETE + body 读取 + 路由 |
| 快照 `Shared`（status.rs） | 新增字段沿用「决策循环写决策字段、面板循环写面板字段」的分工，不破坏并发约定 |
| **新依赖** | 仅 `toml_edit`（格式保留式 TOML 编辑，Cargo 自己在用）。**不引入** rusqlite（见 0.7）、不引入 web 框架（见 status-ui 设计既有决策） |

## 3. 错误处理策略

- **看板出错绝不影响切换循环**（沿用既有原则）：新增的 POST 路径同理，handler panic / 解析失败只影响该连接。
- **命令通道**：`mpsc` 有界（容量 8）；满了直接对 HTTP 返回 503，**不阻塞** HTTP 线程，也不阻塞 orchestrator。
- **命令回执**：每条 Command 带一个 `oneshot::Sender<Result<T, String>>`，HTTP 侧 `await` 它（带 10s 超时）——这样加 key 的**探活错误原文能同步回显到面板**，而不是让用户去翻日志。orchestrator 侧若提前 drop（不该发生），HTTP 返回 500。
- **加 key 的原子性**：顺序是 ①探活 → ②建渠道 → ③写 config.toml → ④加入内存轮询。任一步失败即中止并返回错误；**已建的渠道不回滚**（幂等，下次同名会复用，且渠道本身无害）。config.toml 写入用**临时文件 + rename** 原子替换，避免崩溃时把用户配置截断。
- **pin 的释放必须基于正面证据**：只有「本轮**查到了** pct **且**它超出当前合格线」才解除 pin。查询失败 → **保持 pin**（与「活动 key 查询失败时保持不变、不因抖动丢缓存」同一原则）。
- **反 CSRF**：看板从此接收智谱 API key 并改变状态，而浏览器里**任何网页都能朝 `127.0.0.1:3001` 发跨域表单 POST**。所有写操作要求请求头 `X-QT-Panel: 1`，缺失 → **403**。跨域请求带不了自定义头，会先发 preflight，而我们不回任何 CORS 头 → 被浏览器拦下。GET 不作要求。
- **绑定地址**：`status_addr` 仍默认 `127.0.0.1:3001`。文档明确警告不要绑 `0.0.0.0`（现在等于把 key 录入接口暴露到局域网）。
- **响应绝不回显 key**：加 key 成功只回 `channel_id` + 探活结果（level + 两个窗口的 pct），不回 key 本身。

## 4. 数据结构定义

### 4.1 合格集 —— 本次设计的核心概念（orchestrator.rs）

```
定义（每轮 tick 现算）：
  已知集  = 本轮成功查到 pct 的 key（查询失败的 key 不在其中，状态未知）
  正常集  = { k ∈ 已知集 | pct[k] < throttle }          // 95%
  合格集  = 若 正常集 非空 → (正常集,        档位=Normal)
            否则           → ({ k ∈ 已知集 | pct[k] < exhausted }, 档位=Degraded)   // 100%

语义：
  · 合格集 = 「自动逻辑现在允许把流量放上去的 key」——安全底线，pin 无权染指
  · 正常档：还有 5% 余量的 key 存在 ⇒ 没必要去碰逼近上限的
  · 降级档：全员都过了预防线 ⇒ 放宽到「只要还有余量（<100%）就能用」，避免明知有余量还去撞 429
  · 查询失败的 key 不进合格集（不会被主动选中），但若它恰好是 current / pinned，按「抖动保护」保持不变
```

### 4.2 选择算法（替换 `orchestrator.rs:117-153`）

```
输入：pct（本轮已知）、current（当前 active）、pinned、throttle / restore / exhausted
输出：新 active、以及可选的「pin 已释放」事件

1. pin 优先（但只在合格集内）：
   若 pinned = Some(p)：
     · pct 里查不到 p（本轮查询失败） → 保持 pin，active = p，结束     ← 抖动保护
     · p ∈ 合格集                     → active = p，结束               ← pin 生效
     · 否则                           → 解除 pin，记录 PinRelease{p, pct[p], 当前合格线}，继续往下
                                                                      ← pin 不得覆盖自动逻辑

2. 自动·粘滞（能不换就不换，护缓存）：
   · current = None                → 进 3
   · pct 里查不到 current          → 保持 current，结束                ← 抖动保护
   · current ∈ 合格集              → 保持 current，结束
   · 否则                          → 进 3

3. 自动·挑选（只在合格集内挑）：
   · Normal 档   → 优先在 { pct < restore } 里取 pct 最低；若为空，在合格集里取 pct 最低
   · Degraded 档 → 直接在合格集里取 pct 最低（restore 在这一档无意义，全员已 ≥95%）

4. 合格集为空（全员 ≥ exhausted，或全部查询失败）：
   · 保留原 current（**不清空**——清空会误把原活动降到 exhausted 档）
   · warn! 一行，交给 new-api 的 429 兜底
```

**降级档的粘滞是有意的**：全员超 95% 时，活动 key 会被继续榨到 100% 才流转到下一把还有余量的。理由与正常档一致——能不换就不换，护 prompt 缓存。

### 4.3 priority 下发规则（统一成一条，替换 `orchestrator.rs:156-167` 的三分支）

```
对每把 key：
  · 是 active            → priority_active    (100)
  · 非 active 且 ∈ 合格集 → priority_standby   (10)
  · 已知但不在合格集      → priority_exhausted (0)
  · 本轮查询失败（未知）  → 不动（continue）
```

**这条统一规则在正常档下的行为与现状逐字节相同**（`<95` → 10，`≥95` → 0），但在降级档下自动变得更聪明：**还有余量的 key 拿 standby(10)、真死的拿 0**，于是万一活动 key 仍撞了 429，new-api 的 priority 阶梯会优先跌到「还有余量」的那把，而不是随机跌到一把已经死掉的。

### 4.4 `Command`（orchestrator.rs，HTTP → 循环）

```
数据结构：Command
  Pin      { channel_id: i64, reply: oneshot::Sender<Result<(), String>> }
  Unpin    { reply: ... }
  AddKey   { req: NewKeyReq, reply: oneshot::Sender<Result<AddKeyOk, String>> }
  RemoveKey{ channel_id: i64, reply: oneshot::Sender<Result<(), String>> }

数据结构：NewKeyReq        { name: String, api_key: String, org: Option<String>, project: Option<String> }
数据结构：AddKeyOk         { channel_id: i64, level: Option<String>, five_hour_pct: Option<f64>, weekly_pct: Option<f64> }
数据结构：PinRelease       { channel_id: i64, pct: f64, limit: f64, at_ms: i64 }   // 供看板显示「pin 因超线自动解除」
```

`Orchestrator` 新增字段：`pinned: Option<i64>`、`last_pin_release: Option<PinRelease>`、`rx: mpsc::Receiver<Command>`。

### 4.5 快照新增字段（status.rs `StatusSnapshot`）

```
pinned:            Option<i64>      // 当前 pin 的渠道
regime:            String           // "normal" | "degraded"，看板据此提示「已进入降级档：全部 key 均已超预防线」
eligible:          Vec<i64>         // 本轮合格集 → 前端据此决定 pin 按钮灰不灰
last_pin_release:  Option<PinRelease>
exhausted_threshold: f64            // 回显，看板画第二条阈值线
```

`KeyStatus.tier` 的取值域扩一个：`active | standby | exhausted | unknown` 不变，但 `standby` 现在按合格集判定（降级档下 96% 的 key 也会是 standby）。

### 4.6 HTTP 接口契约（status.rs）

| 方法 | 路径 | 请求 | 响应 |
|------|------|------|------|
| GET | `/api/status` | — | 现有快照 + §4.5 新增字段 |
| GET | `/api/usage?start=<unix>&end=<unix>` | — | `{ start, end, points: [{ hour, tokens, count }], models: [{ model, tokens, count }] }` |
| POST | `/api/pin` | `{channel_id}` | 200 `{ok:true}` / **409** `{error:"该 key 不在合格集（pct=96 ≥ 预防线 95），自动逻辑不允许钉住"}` |
| DELETE | `/api/pin` | — | 200 |
| POST | `/api/keys` | `{name, api_key, org?, project?}` | 201 `{channel_id, level, five_hour_pct, weekly_pct}` / **400** `{error: 智谱返回的原文}` |
| DELETE | `/api/keys/<channel_id>` | — | 200（停止调度 + 从 config.toml 摘除；**不删 new-api 渠道**） |

所有非 GET 请求须带 `X-QT-Panel: 1`，否则 403（见 §3）。

**`/api/usage` 只返回小时桶，不做日聚合**——日聚合放前端按浏览器本地时区做。理由：日期的「一天」是本地时区概念，后端做就得引时区依赖（项目现在没有 chrono），而前端现有代码本来就在用 `new Date(hour*1000)` 转本地时区。少一个参数、少一个时区坑。代价是 30 天 ≈ 720 小时桶 × 模型数 ≈ 100 KB JSON，走 localhost、每 5 分钟一次，可忽略。

## 5. 模块细化

### 5.1 `orchestrator.rs`（修改）

- **`Orchestrator` 新增字段**（§4.4）。
- **主循环改造**（现在在 `main.rs` 里是 `loop { interval.tick(); orch.tick() }`）：
  ```
  loop {
      select! {
          _ = interval.tick()      => orch.tick().await,
          Some(cmd) = rx.recv()    => { orch.handle(cmd).await; orch.tick().await },  // 命令处理完立刻 tick，priority 秒级下发
      }
  }
  ```
- **`choose_active()`**（新私有函数）：§4.2 的算法，**纯函数**（输入 pct/current/pinned/阈值，输出 active + PinRelease）。纯函数化是为了能写单元测试——本项目目前没有单测，这块逻辑分支多、又是安全核心，值得开这个头（见 §7）。
- **`eligible_set()`**（新私有函数）：§4.1，同样是纯函数。
- **`apply_priorities()`**：改成 §4.3 的单一规则。
- **`handle(cmd)`**：
  - `Pin`：`channel_id` 不在 `keys` 里 → 404 语义的 Err；不在**上一轮**合格集里 → Err（带 pct 与合格线，供前端显示）；否则 `self.pinned = Some(id)`。
  - `Unpin`：`self.pinned = None`。
  - `AddKey`：①`QuotaProbe::query` 探活（失败 → Err(智谱原文)）→ ②`ensure_channel` → ③`config::append_key()` 写回 config.toml → ④`self.keys.push(resolved)`。
  - `RemoveKey`：`self.keys.retain(...)`；`self.applied.remove(id)`；若 `active`/`pinned` 指向它 → 置 `None`（下一 tick 自动重选）；`config::remove_key()`。**不动 new-api 渠道**。

### 5.2 `status.rs`（修改）

- **`handle_conn` 升级**：现在是「`read()` 一次 1 KiB，只解析 path」。改为：读到 `\r\n\r\n` 为止 → 解析 method / path / headers → 若有 `Content-Length` 则继续读满 body（**上界 8 KiB，超限 413**）。
- **路由**：按 (method, path) 分发；写操作先过 `X-QT-Panel` 校验。
- **`/api/usage`**：解析 `start`/`end`（缺失或非法 → 400）→ 调 `NewApiClient::usage_data` → 按 hour 聚合出 `points`、按 model 聚合出 `models` → JSON。**这条路径需要 status 服务持有 `Arc<NewApiClient>`**（现在只持有快照）→ `serve()` 签名加一个参数。
- **`render_html()`**（前端，见 §5.5）。

### 5.3 `config.rs`（修改）

- 新增 `exhausted_threshold: f64`（默认 `100.0`）。
- **加载后校验**：`0 < restore ≤ throttle ≤ exhausted ≤ 100`，不满足直接启动失败（配错阈值是静默灾难，早死早好）。
- **`append_key(path, &NewKeyReq)`** / **`remove_key(path, name)`**：用 `toml_edit::DocumentMut` 增删 `[[keys]]` 条目 —— **注释、空行、排版原样保留**。写入走「临时文件 + fsync + rename」原子替换。
- **`ResolvedKey`** 构造复用现有逻辑（`quota_headers` 从 `org`/`project` 拼 `Bigmodel-Organization` / `Bigmodel-Project` 两条）。

### 5.4 `main.rs`（修改）

`mpsc::channel(8)` → `Sender` 交给 `status::serve()`，`Receiver` 交给 `Orchestrator`。`run`/`up` 两条路径都要接。

### 5.5 前端（`render_html` 内联，无外部依赖）

**图区三态**（顶部一排切换）：

| 视图 | 数据源 | 刷新 |
|------|--------|------|
| **近 24 小时**（默认） | 快照里的 `hourly`（**现状，一行不改**） | 5 秒（随 `/api/status`） |
| **近 30 天** | `/api/usage?start=30d前&end=now` → 前端按本地时区归成日桶 | **5 分钟** + 手动按钮（= 源头落库节奏，见 0.5） |
| **某天 · MM-DD**（从 30 天视图点柱下钻，面包屑可返回） | `/api/usage?start=当天00:00&end=当天24:00` | 若是**今天** → 5 分钟；**过去某天 → 拉一次冻住**（数据已不再变） |

**key 卡片新增**：
- 「📌 钉住」按钮。**不在 `eligible` 里的 key，按钮置灰** + tooltip「已过 95% 预防线，自动逻辑不允许钉住」。灰掉比「点了之后被静默解除」清楚。
- 活动卡若正被 pin → 显示 📌 徽章 + 「取消固定」。
- `last_pin_release` 非空 → 顶部提示条「pin 已因 zhipu-1 达 100% 自动解除，已回到自动选择」。
- `regime == "degraded"` → 顶部黄条「降级档：全部 key 均已超 95% 预防线，正在榨干活动 key 至 100% 后再流转」。

**新增「+ 添加 key」表单**：name / api_key / org / project → 提交 → 成功则显示探活结果（`level` + 5h/周 两个窗口的 pct，让用户确认这把 key 确实是他以为的那把）；失败则**原样显示智谱的错误报文**。

## 6. 完整性自检 checklist

- [ ] 正常档下，A 的改动与现状**行为等价**（未引入回归）——见 §4.3 的论证，需用单测钉住
- [ ] 全员 ≥95% 且活动 key 到 100% → 流转到还有余量的最低 pct 那把（用户提的场景）
- [ ] 全员 = 100% → 保留原活动，不清空，warn，交给 429 兜底
- [ ] pin 一把不合格的 key → 被拒（409），且面板上按钮本来就是灰的
- [ ] pin 的 key 越线 → 自动解除 + 面板提示 + 回到自动选择
- [ ] pin 的 key 本轮查询失败 → **保持 pin**（不因抖动解除）
- [ ] 活动 key 本轮查询失败 → 保持不变（现状行为，勿破坏）
- [ ] 加 key：探活失败 → 不建渠道、不写 config.toml、错误原文回显
- [ ] 加 key：config.toml 的注释/排版**一个字符不掉**
- [ ] 加 key 后无需重启即进入轮询；重启后仍在（config.toml 已持久化）
- [ ] 删 key：停止调度 + 从 config.toml 摘除；new-api 渠道仍在
- [ ] 无 `X-QT-Panel` 头的 POST → 403
- [ ] 命令通道满 → 503，不阻塞 orchestrator
- [ ] 看板挂掉 / bind 失败 → 切换循环照常（现状原则，勿破坏）
- [ ] 近 24 小时视图的实时性**未被削弱**（仍走快照 5 秒刷）

## 7. 验证计划

**单元测试（本项目首次引入，只覆盖纯函数）**：`choose_active()` / `eligible_set()` 的分支矩阵——正常档粘滞、正常档切换、降级档粘滞、降级档流转、全死、pin 生效、pin 越线释放、pin 查询失败保持、current 查询失败保持。这块是安全核心且分支多，端到端测不划算。

**端到端（对真 new-api + 真 key）**：
1. `curl` 打 `/api/usage`，与直接查 `.newapi/one-api.db` 的聚合结果对账（数字必须逐桶一致）。
2. 浏览器：切 30 天 → 点某天下钻 → 面包屑返回；确认过去某天不刷新、今天刷新。
3. pin：钉一把 standby → 观察 60 秒内 priority 下发 + 流量转移（看请求流水的落点变色）；取消 pin → 回到自动选择。
4. pin 越线：把 `throttle` 临时调到低于当前 pct（如 40%）→ 观察 pin 自动解除 + 提示条。
5. 降级档：把 `throttle` 临时调到 1%（制造「全员超线」）→ 观察进入降级档 + priority 按 §4.3 下发（有余量的拿 10，不是 0）。
6. 加 key：故意漏填 org/project → 必须**被拒**并显示智谱原文（这条最关键，是整个功能的价值所在）；补全后成功 → 检查 config.toml 的注释完好 + 新渠道出现在 new-api + 下一轮进入轮询。
7. `X-QT-Panel` 头缺失 → 403。

## 8. 实现顺序（每步独立可测、可回滚）

1. **A 调度降级档**（纯逻辑 + 单测）——不碰 HTTP，风险最低，且立刻修掉一个真实漏洞
2. **E 用量历史**（`/api/usage` + 前端三态）——纯读，不依赖控制通道，能最快看到东西
3. **B 控制通道底座**（POST/body 解析 + `X-QT-Panel` + mpsc + select!）——无用户可见功能，用 curl 验收
4. **C pin**（建在 A 的合格集 + B 的通道上）
5. **D 加/删 key**（`toml_edit` + 探活 + 热加载）
