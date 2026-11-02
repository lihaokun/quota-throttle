# 细化设计 — SGM-8 / 第 5 批：签发账号内部余额

> 状态（2026-11-03）：**v3/C1已确认，B1–B8已实施，最终代码契约独审通过、未关闭生产问题0。** F1已修复并独立56例关闭；修后最后Node68/0、check:ui、full浏览器112/112、余额8/8及全部JS25/25变异通过。仅最终固定SHA完整Rust/C2汇总仍pending。用户最新授权B3–B8连续完成，取消逐模块中间停点，最终统一交付；部署确认保留。证据时点见§10.11、[实施汇总](../../../audits/sgm-8-5-balance/b3-b8-implementation-review.md)、[最终代码独审](../../../audits/sgm-8-5-balance/final-contract-review.md)。代码独审不代替固定SHA C2。
> 架构依据：`sgm-8-cleanup.md` v6 §2.9（2026-11-01 用户确认；§4.4 第 1 项三轮审查后通过）、§2.6 I5 / I8 / I9 / I10、§6 第 1b 步。
> 本稿对 §2.9 的细化与偏差见 §1 末尾。所有 file:line 以提交 `7e5cf97` 为准。

## 1. 范围

| 编号 | 内容 | 主要模块 |
|---|---|---|
| B1 | 目录库 v9：`balance_policy` 单行表、v8→v9 迁移、读 / 写 / 校验 | `catalog/schema_v9.sql`（新）、`catalog/balance.rs`（新）、`catalog/store.rs`、`catalog/types.rs` |
| B2 | new-api调用：有界准入/认证等待、观测资料、换算信息、add_quota | `newapi/transport.rs`、`newapi/tokens.rs`、`newapi/users.rs`（新） |
| B3 | 余额槽位、观测轮、补足函数（7 步）、自动补足、改设置、快照视图 | `reconcile/worker/balance.rs`（新）、`reconcile/worker.rs`、`reconcile/worker/token_service.rs` |
| B4 | 独立后台任务 `balance_loop` | `runtime.rs` |
| B5 | 两个同步写路由：`POST /balance/refill`、`PUT /balance/policy`；路由截止传给 worker；已批准60/70/75配套 | `panel_http.rs`、`panel_http/admin/{route,input,bridge,dispatch,output}.rs` |
| B6 | 快照 v2：新增根字段 `balance` | `reconcile/worker/panel.rs`、`panel_http/tests.rs`、三份 schema |
| B7 | 前端：契约、校验、两个动作、余额栏、状态栏 chip | `panel_http/ui/{contract,core,client,app,render}.mjs`、`scripts/contract_audit/sgm6_b_contract.mjs` |
| B8 | 文档与探针：更正 CLAUDE.md、rc.20 `add_quota` 探针 | `CLAUDE.md`、`scripts/probes/newapi_balance_probe.py`（新） |

**不做**：不改模型倍率、不改初始化脚本（架构 §2.9「明示接受 / 不做」）；不新增错误码（复用 `ControlErrorCode` / `ExtraCode`，`AdminError` 的 28 个码、通用 `REJECTION_TEXT` 不变；补足动作的提示由 §5.8.4 单独覆盖）；不维护已冻结的历史审计脚本（§7）。

**对架构 §2.9 的细化与偏差（明示，2026-11-02 用户确认）**：
1. 快照不是并列加三个根字段，而是加**一个**根字段 `balance = {policy, unit, accounts}`：根宽度与 schema 改动最小，且三者总是一起读写。语义与 §2.9 相同。
2. `dispatch` 增加 `deadline` 参数：bridge 现在不把路由截止传给业务（`panel_http/admin/bridge.rs:166`），§2.9 第 5 步要求手动补足的 guard 并入路由截止，只能改签名。
3. 周期观测用 `RequestClass::Observe`（后台类、同时最多 1 个在途，`newapi/transport.rs:346-352`），补足函数第 3 步的新鲜读数仍用 `Control`：现有 `profile()` 固定 `Control`（`newapi/tokens.rs:357-364`），因此新增观测变体。
4. 补足的 guard 为假时传输层返回 `RevisionConflict`/`NotSent`（`transport.rs:557-558`）；对外改报 `not_ready`（「后端未就绪或请求已超时，未发送」），避免面板把它说成「对象版本已变化」。

**另两项已确认的实现细节**：改余额设置时，开事务或提交遇到 Busy，确认未提交并回滚后返回 503 `queue_full`；所有者处于封锁 / 自动退避期时的拒绝，不覆盖原来的「结果不确定」记录（§3、§5.4.3）。

**C1 回到架构原文**：换算不可用时禁用金额输入，只展示原始额度（F-M1）；不确定补足由 HTTP 层返回 504 `result_unconfirmed`（B-m3），不新增偏差。**D1 已确认**：用户提出「加大预算到 60s」后，批准 §5.6 仅补足路由的 60s 业务 / 70s 客户端 / 75s 连接配套，架构 §2.9 已同步；原「仅改措辞、保留现有时限」建议未采纳。

**本稿的确认与检查点（最新用户覆盖）**：v1/I10/D1与C1均已完成，已改为subagent独立审核。用户随后授权B3–B8一次性连续完成，取消逐模块中间停点；本批实现、修正、验证和独审连续收敛后统一交付，不把旧§10历史停点作为再次等待的依据。最终固定SHA审核/门禁与部署前明确确认仍需完成，PR #5范围不变；本机及远端首轮原生写没有提前获部署许可。

## 2. 关键事实与复用点

### 2.1 关键事实（均已查代码或实测）

- **E1 投递证据**：`DeliveryEvidence::{NotSent, ResponseReceived, Uncertain}`（`transport.rs:44-50`）。HTTP 200 且 `success:false` 为 `ResponseReceived` + `RemoteRejected`（`:214-215`）；非 JSON 的 429、超时、连接中断后为 `Uncertain`（`:812-813,872-873,879-884`）；本地预算冷却超过 60s 立即 `NotSent`（`:332-345`）。`code()` 把任何 ≥500 的状态映射为 `TransportUnknown`（`reconcile/worker.rs:169-183`）。
- **E2 guard 只检查一次**：`send_fenced` 在预算准入、即将扣费时检查 guard，准入后请求必定发出，执行期间不再检查；401 重登后的重放会重新检查（`transport.rs:552-561,357-359`）。
- **E3 写请求语义**：POST 只能是 `IdempotentWrite` 或 `Create`（`transport.rs:128-136`）；`add_quota` 不幂等，用 `Create`。401 时请求在鉴权中间件就被拒，重登后重放不会叠加。
- **E4 账号资料**：`parse_profile` 核对 `id == owner.user_id`、`status == 1`、角色，并要求 |quota| ≤ 2^53−1（`newapi/tokens.rs:154-177`）；`token_client(scope)` 给出所有者的客户端（`reconcile/worker/token_service.rs:147-165`，目前模块私有）。
- **E5 后端租约**：`BackendReadiness::ready_at(now_s)` / `expires_at()`（`reconcile/planner.rs:30-58`），以秒计；`token_frame` 的取法见 `token_service.rs:179-193`。
- **E6 路由截止**：`deadline = min(entered_at + budget, connection_end − 5s)`（`bridge.rs:82`），到期回 504 `result_unconfirmed` 而任务继续（`:105-114`）；`Pending.deadline` 只用于拒绝检查，不传给 `dispatch`（`:151-169`）。
- **E7 目录错误映射**：返回 `ControlResult` 的路径里，所有 `CatalogError` / `rusqlite::Error` 都变成 `Storage`（`catalog/control.rs:53-62`，HTTP 500）。开 IMMEDIATE 事务或提交时的 Busy 未写入任何东西，必须单独映射为 `QueueFull`（503），否则面板会误记「结果待核对」。
- **E8 迁移机制**：版本链在 `store.rs:182-248`；`verify_schema` 只接受 `CREATE` 语句（`:880-885`），默认行必须在 Rust 迁移代码里插入；`migrate_v5`（`:377-394`）是不重建操作表的模板；`migrate_v4` 示范插入单行默认值（`:369`）。
- **E9 快照**：`panel_snapshot_at` 持 `core.try_lock()` 组装（`reconcile/worker/panel.rs:281-301`），全仓库只有它与 `panel_http/tests.rs:24-41` 的 `empty()` 两处构造 `PanelSnapshot`；`panel_http/tests.rs:572-580` 禁止快照文本含 `settings` 等子串。
- **E10 前端契约是生成的**：`contract.mjs` 由 `scripts/contract_audit/sgm6_b_contract.mjs` 从 schema 生成，CI 的 `check:ui` 会 `--check`（`sgm6_b_check.mjs:25`）；新 record 族要在生成器的 `shapes`（`:42-54`）登记，新枚举在 `:13-29` 登记；`tests/ui/core.test.mjs:9-12` 要求每个 `record()` 的字段集合等于 `WIRE_FIELDS`。
- **E11 前端缓存键**：`chips()` 的 stamp（`render.mjs:416`）与 `groups()` 的 stamp（`:338`）不含 `balance`，必须并入，否则余额变化不重画。`draftConflict` 只认 source / group / membership（`core.mjs:529-540`），新 entity 需在 `currentEntity` 加分支。`decimal` 只接受整数串（`core.mjs:11-12`），美元金额要新解析器。
- **E12 rc.20**：`POST /api/user/manage {id, action:"add_quota", mode:"add", value}`；`/api/status` 公开返回 `quota_per_unit`（float）与 `quota_display_type`（架构 §2.9 调研，临时实例实测）。
- **E13 明确拒绝的无状态码分支**：`envelope()` 的 2xx + `success:false` 产生 `ResponseReceived/RemoteRejected`，`status=None`（`transport.rs:208-219,576-583`）；非 2xx + 合法 JSON 的 500 产生 `ResponseReceived/status=Some(500)`（`:854-875`）。分类须区分这两条，不能只看 `code()`，因为后者已把 ≥500 折叠为 `TransportUnknown`（C1 B-M1、B-M3）。
- **E14 请求计数的时点**：`Fake` 在 handler 返回后才保存 `Recorded`（`newapi/transport/tests.rs:82-83`）；未知路径会使 `TokenState::reply` panic。余额测试必须先扩展路径，并用入口 hook 计数，不能把未知路径 panic 后的空列表当作「没有发送」（C1 B-M2）。

### 2.2 复用点

| 复用项 | 位置 | 用法 |
|---|---|---|
| `migrate_v5` 模板、`backup_to`、`verify_schema` 链 | `catalog/store.rs:377-394,577-664,829-901` | v8→v9 |
| `save_group_policy_tx` 的「比较修订号 → 不变即返回 → bump」 | `catalog/policy.rs:438-485` | `save_balance_policy` |
| `bump`、`positive_revision` | `catalog/control.rs:565-570`、`catalog/types.rs:347` | 修订号 |
| `ManagementRequest::new`、`send_fenced`、`envelope` | `newapi/transport.rs:106-140,525-589,185-220` | `add_user_quota`、`quota_unit` |
| `parse_profile`、`ManagedTokens::read` | `newapi/tokens.rs:154-177,335-342` | 观测变体 |
| `token_client`、`code`、`error`、`now` | `reconcile/worker/token_service.rs:147-165`、`reconcile/worker.rs:149-183` | 补足函数 |
| `owned_job`、`delay`、`control` | `runtime.rs:44-84` | `balance_loop` |
| check_group 的同步写链路 | `route.rs:103,143`、`input.rs:271-274,383-390`、`dispatch.rs:85`、`output.rs:36,97` | 两个新路由 |
| `tokens_fixture` / `TokenState` / `Fake` | `reconcile/worker/token_service_tests.rs:37-340`、`newapi/transport/tests.rs:7-114` | 后端测试 |
| `submitApp`、`richState` | `tests/ui/actions.test.mjs`、`tests/ui/fixtures.mjs` | 前端测试 |

## 3. 错误处理策略

| 路径 | 结果 | 对外 | 槽位记录 | 封锁 / 退避 |
|---|---|---|---|---|
| 观测：读资料失败 | 未知 | 快照 `state:"unknown"` + `unknown_reason` | 保留上次成功读数与时间，记失败原因 | 无 |
| 观测：读换算失败 | 保留上次换算 | — | — | 无 |
| 第 1 步：所有者未配置 / 自动互斥被占 / 手动等锁到截止 | 未发送 | `not_ready` / `queue_full`（规则 R 为真）；bridge 截止也可能先回 504 | 不记录：未取得所有者互斥，不能覆盖其他在途操作 | 无 |
| 第 1b 步：封锁、自动退避或 dry_run | 未发送 | `not_ready`（dry_run 在 HTTP 层先为 403） | 不记录，保留原结果 | 原样保留 |
| 第 1b 步已持互斥、未封锁，但手动截止已过 | 未发送 | `not_ready`；bridge 可能已回 504 | 记 `not_sent/not_ready`，金额 null | 无 |
| 第 2–4 步失败，或第 5 步 `DeliveryEvidence::NotSent`（含 guard 假、本地冷却、连接建立失败） | 未发送 | `not_ready` 503 / `invalid_input` 422 / `queue_full` 503 等；存储或锁中毒为 500 | 能安全写槽位时记 `not_sent`；自动无需补足 / 开关关闭的 `Ok(None)` 不记录 | 无 |
| `ResponseReceived` 且 status 为 None 或小于 500 | 明确拒绝；None 是 2xx 的 `success:false`（E13） | `remote_rejected` 等 502 / 429 | 记 `rejected`、尝试金额及错误码 | 自动退避 10 分钟，不封锁手动 |
| 其余已准入结果（`Uncertain` 或收到 ≥500，含 500 + `success:false` JSON） | 结果不确定 | worker `TransportUnknown`；dispatch 映射 504 `result_unconfirmed`（规则 R 为假） | 记 `uncertain`、尝试金额、`transport_unknown` | 封锁 10 分钟（自动与手动）+ 自动退避 |
| `success:true` | 已补足 | 200 + 回执 | 记 `refilled` + 金额；第 7 步回读成功则刷新读数 | 无 |
| 路由截止先到 | bridge 回 504 `result_unconfirmed`；任务继续，guard 保证截止后不发送 | — | 按上面各行记录 | 同上 |
| 改设置：校验失败 | `invalid_input` 422 | — | — | — |
| 改设置：修订号不符 | `revision_conflict` 409 | — | — | — |
| 改设置：开事务 / 提交时 Busy | `queue_full` 503（E7） | — | — | — |
| 改设置：其他存储错误 | `storage` 500（结果不确定） | — | 保留旧缓存；若 store 不可用，关闭发布栅栏（§5.4.4） | — |

规则 R / I8 自洽：表中所有「非 500 / 504」的对外错误都发生在 add 发送之前，或是 new-api 的明确拒绝，二者都没有改变余额；改设置的 409 / 422 / 503 都在提交之前回滚。`last_refill` 是内存尝试结果，不是已提交的操作记录或目录行；T1 要分别断言持久状态不变与 §5.4.3 允许的精确内存变化（§6），不把一次 `not_sent` 内存记录误判成落库。

## 4. 数据结构

### 4.1 SQL（`catalog/schema_v9.sql`，新）

```sql
CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 9),
    catalog_instance_id TEXT NOT NULL UNIQUE CHECK(length(catalog_instance_id) = 32 AND catalog_instance_id NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE balance_policy (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    auto_refill INTEGER NOT NULL CHECK(auto_refill IN (0, 1)),
    warn_below INTEGER NOT NULL CHECK(warn_below >= 1),
    refill_to INTEGER NOT NULL CHECK(refill_to <= 4000000000000000),
    CHECK(warn_below < refill_to)
) STRICT;
```

默认行由 `migrate_v8` 插入：`(1, 1, 1, 100000000000000, 1000000000000000)`。值是确定的，新建库与迁移库的镜像一致（`token_migration_tests.rs:53,56` 依赖这一点）。

### 4.2 Rust 类型

```rust
// catalog/balance.rs
pub const BALANCE_CAP: i64 = 4_000_000_000_000_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BalancePolicy { pub revision: i64, pub auto_refill: bool, pub warn_below: i64, pub refill_to: i64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BalancePolicyDraft { pub auto_refill: bool, pub warn_below: i64, pub refill_to: i64 }

// newapi/users.rs
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaUnit { pub quota_per_unit: f64, pub display_type: String }   // 有限且 > 0；display_type 为 1..=16 字节 ASCII 字母数字

// reconcile/worker/balance.rs
pub(crate) struct Balance {                       // ReconcileWorker 的新字段
    slot: std::sync::Mutex<BalanceSlot>,
    issuer_lock: tokio::sync::Mutex<()>,          // 按所有者的异步互斥
    legacy_lock: tokio::sync::Mutex<()>,
    failed: std::sync::atomic::AtomicBool,       // 默认false；无法安全结算时单向故障关闭
}
struct BalanceSlot { policy: BalancePolicy, unit: Option<QuotaUnit>, accounts: Vec<AccountSlot> }  // 顺序固定：issuer 在前
struct AccountSlot {
    owner: TokenOwner,
    reading: Option<(i64 /*quota*/, i64 /*at_ms*/)>,
    failure: Option<(ControlErrorCode, i64 /*at_ms*/)>,
    locked_until_ms: i64,                         // 结果不确定后的封锁（自动 + 手动）
    backoff_until_ms: i64,                        // 仅自动
    last_refill: Option<RefillRecord>,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefillSource { Auto, Manual }
#[derive(Clone, Copy)]
pub(crate) enum RefillOutcome { Refilled, NotSent(ControlErrorCode), Rejected(ControlErrorCode), Uncertain }
#[derive(Clone, Copy)]
struct RefillRecord { at_ms: i64, source: RefillSource, amount: Option<i64>, outcome: RefillOutcome }
pub(crate) enum RefillAmount { ToTarget, Exact(i64) }
// 内部返回值，无 Serialize；HTTP 层构造下列字符串 ID 的线上类型。
pub(crate) struct RefillReport { pub owner_scope: TokenOwnerScope, pub user_id: i64, pub before: i64, pub amount: i64, pub after: Option<i64> }

// worker 快照与 HTTP 输出共用的安全类型；均 Serialize，不省略 null 字段。
pub(crate) struct BalanceView { pub policy: BalancePolicy, pub unit: Option<QuotaUnit>, pub accounts: Vec<BalanceAccountView> }
pub(crate) struct BalanceAccountView {
    pub owner_scope: TokenOwnerScope, pub user_id: String, pub state: BalanceState,
    pub quota: Option<i64>, pub observed_at_ms: Option<i64>, pub unknown_reason: Option<ControlErrorCode>,
    pub locked_until_ms: Option<i64>, pub backoff_until_ms: Option<i64>,
    pub last_refill: Option<BalanceRefillView>,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BalanceState { Sufficient, Low, Exhausted, Unknown }
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BalanceRefillOutcome { Refilled, NotSent, Rejected, Uncertain }
pub(crate) struct BalanceRefillView {
    pub at_ms: i64, pub source: RefillSource, pub outcome: BalanceRefillOutcome,
    pub amount: Option<i64>, pub error_code: Option<ControlErrorCode>,
}
pub(crate) struct BalanceRefilled {
    pub schema_version: i64, pub owner_scope: TokenOwnerScope, pub user_id: String,
    pub before: i64, pub amount: i64, pub after: Option<i64>,
}
pub(crate) struct BalancePolicySaved {
    pub schema_version: i64, pub revision: i64, pub auto_refill: bool,
    pub warn_below: i64, pub refill_to: i64,
}
```

常量：BALANCE_INTERVAL=60s、READING_FRESH_MS=180000、BLOCK_MS=BACKOFF_MS=600000、BALANCE_READ_BUDGET=20s、AUTO_REFILL_BUDGET=60s。后两者只约束余额新增入口，不改变其它管理调用的默认行为。

新增私有RefillAttemptGuard：持有本所有者的tokio互斥guard、槽位引用、source与已校验amount，及settled=false。它只在第5步潜在投递前创建；Drop须在仍持所有者锁时同步结算，字段解锁在Drop逻辑之后发生。没有新数据库表或持久任务；原架构已接受的重启丢封锁边界不变。 私有槽位无法结算时仅置balance.failed，不绕过Core写全局路由栅栏。

`Balance` / `BalanceSlot` / `AccountSlot` / `RefillRecord` 为 worker 私有内存状态，不落库、不直接序列化；两类所有者必须对应不同正 ID。`BalancePolicy/Draft` 为 catalog → worker → HTTP 共用，`QuotaUnit` 为 newapi → worker → 前端共用，其余 View 为快照 / HTTP → 浏览器共用。公开到 crate 的签名所引用的枚举同样为 `pub(crate)`。

上述 View 的字段始终输出，禁止 `skip_serializing_if`；所有 `user_id` 由已校验的内部正 i64 显式 `to_string()`，按既有字符串 ID 规则校验，不以 JSON 数字或 JS `Number` 表示。额度为 ±JSON_SAFE 内的整数，尝试金额为 1..=BALANCE_CAP，时间为 0..=JSON_SAFE 毫秒；`accounts` 至多两项、范围和 ID 均唯一，顺序 issuer、legacy_admin。

### 4.3 线上格式

**快照根字段 `balance`**（`panel-snapshot.schema.json`，`schema_version` 1 → 2，`$id` 后缀与 title 同步为 2）：

```json
"balance": {
  "policy": {"revision": 1, "auto_refill": true, "warn_below": 100000000000000, "refill_to": 1000000000000000},
  "unit": {"quota_per_unit": 500000, "display_type": "USD"},          // 可为 null
  "accounts": [{
    "owner_scope": "issuer", "user_id": "2",
    "state": "sufficient",                                            // sufficient | low | exhausted | unknown
    "quota": 999999976493170, "observed_at_ms": 1761997000000,        // 都可为 null（从未读到）
    "unknown_reason": null,                                           // state=unknown 时的 controlError 码，可为 null
    "locked_until_ms": null, "backoff_until_ms": null,                // 已过期时为 null
    "last_refill": {"at_ms": 0, "source": "auto", "outcome": "refilled", "amount": 1, "error_code": null}   // 可为 null
  }]
}
```

`state` 在组装快照时按 `at` 计算：存在尚未被后续成功读取清除的 `failure`，或读数缺失、超过 3 分钟 → `unknown`；否则读数 ≤ 0 → `exhausted`；< `warn_below` → `low`；否则 `sufficient`。读取失败时保留上次成功读数及时间作历史展示，不因旧读数仍在 3 分钟内就继续显示充足或触发自动补足（I9）；成功读取同时更新读数、时间并清除 `failure`。字段名都不含 `settings`（E9）。

新鲜度统一为 `0 ≤ at_ms - observed_at_ms ≤ 180_000`；负年龄或严格 `> 180_000` 为未知。失败原因来自 `failure`，仅缺读数 / 过期 / 时钟回拨时可为 null。`quota` 与 `observed_at_ms` 同时为 null 或同时非 null；`unknown_reason != null` 必须对应 unknown。封锁 / 退避只有截止晚于本次 `captured_at_ms` 时输出非 null。迟到的周期观测可能覆盖补足后的回读，只影响显示，至多到下一轮成功观测（正常轮次间隔 60s，网络等待可延长），明示接受；每次实际补足仍重新读资料，不用缓存作金额依据。

`RefillRecord` → `BalanceRefillView` 映射如下（所有列必有，含 null）：

| 内部结论 | outcome | amount | error_code |
|---|---|---|---|
| `Refilled` | `refilled` | 本次已校验的尝试金额，非 null | null |
| `NotSent(c)` | `not_sent` | 第 4 步金额校验通过后为该金额；此前拒绝为 null | c，非 null |
| `Rejected(c)` | `rejected` | 本次已校验的尝试金额，非 null | c，非 null |
| `Uncertain` | `uncertain` | 本次已校验的尝试金额，非 null | `transport_unknown` |

`source` 的完整枚举为 `auto/manual`，`outcome` 为 `refilled/not_sent/rejected/uncertain`，`state` 为 `sufficient/low/exhausted/unknown`。成功结论只由 add 响应产生；回读失败使账号未知、成功回执 `after=null`，不把旧余额冒充回读结果。

`panel-snapshot.schema.json` 新 `$defs` 固定为 `balance`、`balance_account`、`balance_refill`、`balance_policy`、`quota_unit`；根字段引用 `balance`。各对象 `additionalProperties:false`，属性全列入 required，nullable 用明确联合类型；以条件约束补足结论的 amount/error_code 组合。错误码、所有者范围沿用既有 enum 定义，不另抄一套。

**请求**（`panel-admin-requests.schema.json` 新增两个 def 并加入 `anyOf`）：
- `RefillBalance`：`{"schema_version":1, "owner_scope":"issuer"|"legacy_admin", "amount": 1..4000000000000000}`。
- `SetBalancePolicy`：`{"schema_version":1, "expected_revision": revision, "auto_refill": bool, "warn_below": 1..JSON_SAFE, "refill_to": 1..4000000000000000}`。

**响应**（`panel-admin-responses.schema.json` 新增两个本地 def 并加入 `anyOf`）：
- `BalanceRefilled`：`{"schema_version":1, "owner_scope", "user_id": id, "before": signed, "amount", "after": signed|null}`。
- `BalancePolicySaved`：`{"schema_version":1, "revision", "auto_refill", "warn_below", "refill_to"}`。

响应中 `id` 是字符串；`BalanceRefilled.amount` 必须等于原请求金额、scope 等于请求范围。`BalancePolicySaved` 三项值等于原请求，revision 只能为 `expected_revision`（无变化）或 `expected_revision+1`（有变化，仍须 ≤JSON_SAFE）。响应的两个 def 是完整严格对象：`BalancePolicySaved` 中策略四字段的约束与快照 `balance_policy` 一致，`BalanceRefilled` 的 signed / id / amount 复用对应既有定义或上界；`sgm6_b_wires` 同时校验两种载体。

## 5. 模块细化

### 5.1 `catalog/balance.rs`（新）

#### 5.1.1 `fn valid(d: &BalancePolicyDraft) -> bool`
`0 < warn_below < refill_to ≤ BALANCE_CAP`。trivial。

#### 5.1.2 `impl CatalogStore { pub(crate) fn balance_policy(&self) -> ControlResult<BalancePolicy> }`
- 读 `SELECT revision, auto_refill, warn_below, refill_to FROM balance_policy WHERE singleton = 1`；不存在或越界 → `Storage`（`verify_balance` 已在打开时保证，这里只是防御）。
- trivial（单条只读查询）。

#### 5.1.3 `impl CatalogStore { pub(crate) fn save_balance_policy(&mut self, expected_revision: i64, draft: BalancePolicyDraft) -> ControlResult<BalancePolicy> }`
- **实现**：
  1. `self.usable()?`（`CommitUnknown` → `Storage`）。
  2. `ensure(positive_revision(expected_revision) && valid(&draft), InvalidInput)`。
  3. 开 IMMEDIATE 事务；`Busy` → `QueueFull`，其他错误 → `Storage`（E7）。
  4. 读旧值；`old.revision != expected_revision` → `RevisionConflict`（事务回滚）。
  5. 三项都相同 → 返回旧值，不递增（与 `save_group_policy_tx:460-462` 一致）。
  6. `revision = bump(old.revision)?`；`UPDATE … WHERE singleton = 1 AND revision = ?old`，要求影响 1 行，否则 `Storage`。
  7. `commit(tx, &mut self.poisoned)`：`Busy` → `QueueFull`（未提交，RAII 回滚），其他错误 → `Storage`。
- **退出**：Ok(新值) / 上述错误。
- 修订号到达 JSON_SAFE 时，`bump` 的 `InvalidInput` 直接返回 422，事务回滚；此处有意不照搬 `save_group_policy_tx` 把 bump 失败改为 Storage 的映射。复用的是比较 / 不变不递增 / bump 的模式，不是全部错误映射。
- **正确性论证**：前置 = 输入已校验、调用方持 `Core` 锁。第 2–4 步的失败都在任何 UPDATE 之前，事务回滚 ⇒ 什么都没写（I8）；第 6 步后才可能写入，唯一的写入在同一事务内提交，提交 Busy 时 SQLite 不落库 ⇒ `QueueFull` 也是「什么都没写」；只有提交返回其他错误时结果未知 ⇒ `Storage` 500，面板按 R 为假处理。并发：IMMEDIATE 事务串行化写者，修订号比较保证「先读后写」不丢更新。

#### 5.1.4 `pub(super) fn verify_balance(conn: &Connection) -> Result<()>`
恰好 1 行、`singleton = 1`、取值满足 `valid` 且 `revision ≥ 1` → Ok；否则 `CorruptData`，不补行。trivial。

### 5.2 `catalog/store.rs` / `types.rs`：v8 → v9

按 E8 逐点改动（与 v7→v8 同构）：
1. `types.rs:8` `DATABASE_SCHEMA_VERSION = 9`；`store.rs:20` 后加 `const SCHEMA_V9`。
2. 打开旧库的探测（`:144-161`）后加 `if version >= 9 { verify_balance(&probe)?; }`；总校验（`:268-272`）加 `verify_balance`。
3. 迁移链（`:248` 后）：`if version <= 8 { if !new_file && version == 8 { backup_to("catalog-v8-before-v9-{suffix}.sqlite3") } store.migrate_v8()?; }`。
4. `fn migrate_v8(&mut self) -> Result<()>`：照 `migrate_v5`：IMMEDIATE；取 `catalog_instance_id`；`DROP TABLE catalog_meta`；执行 `SCHEMA_V9`；`INSERT INTO catalog_meta VALUES(1,9,?id)`；`INSERT INTO balance_policy VALUES(1,1,1,100000000000000,1000000000000000)`；`user_version = 9`；`verify_schema(&tx, 9)`、`verify_core_data`、`verify_balance`；`commit`。
5. `verify_schema`：V8 段在 `expected_version >= 9` 时排除 `CREATE TABLE catalog_meta`，追加 V9 段（`expected_version >= 9`）。
- **正确性论证**：与已有各版本迁移同构；失败时事务回滚、旧库与备份都保留（由 T-B2 钉住）。新建库也走完整迁移链（`version` 从 1 开始，`:126`），因此同样得到默认行。

### 5.3 new-api 调用

#### 5.3.1 `newapi/tokens.rs`：有截止的余额资料读取
- 余额调用新增 `observe_profile_until(deadline: tokio::time::Instant)`（Observe）与 `profile_until(deadline)`（Control），共用私有profile_with(class, optional_deadline)。原profile()仍用Control/None，行为不变。
- 调用方balance_round/refill → 固定owner核验 → 既有read/parse_profile；期限为Some时将同一绝对deadline传至§5.3.4，不能只改外层签名后仍走无截止send。request.for_user、重登后的principal核对和错误证据保持原样。
- 论证：身份/角色/数值由同一parse_profile验证；队列与认证等待由§5.3.4有界，已准入HTTP仍沿原10s执行上限，不用可取消外层timeout包登录。失败交caller记未知，逻辑GET至多一次401恢复重放、无余额写入；在既有同步短临界正常返回前提下，调用到deadline后至多再等一个已准入HTTP执行窗口。

#### 5.3.2 `newapi/users.rs`（新）：`quota_unit_until(deadline: tokio::time::Instant) -> transport::Result<QuotaUnit>`
- **功能 / 调用关系**：balance_round → quota_unit_until → §5.3.4有界入口GET /api/status（Read、Observe、SINGLE_BODY_LIMIT）→ envelope。
- **实现**：从 data 取 `quota_per_unit`（数值，有限且 >0）与 `quota_display_type`（1..=16 字节 ASCII 字母数字），成功构造 QuotaUnit；缺失 / 不合法为 InvalidResponse，网络 / envelope 失败按原证据返回。
- **正确性论证**：固定backend、合法envelope和逐字段校验后才产生换算值；失败不猜比例。队列和认证等待由绝对deadline限制，HTTP另有10s/体积上限，有限字段校验终止；无余额写入，保留旧换算由caller决定。

#### 5.3.3 `add_user_quota_until(user_id: i64, amount: i64, guard: &(dyn Fn() -> bool + Send + Sync), deadline: tokio::time::Instant) -> transport::Result<()>`
- 前置校验：`user_id > 0`、`1 ≤ amount ≤ BALANCE_CAP`，否则 `InvalidInput`/`NotSent`（防 `id = 0` 命中 root 的陷阱）。
- ManagementRequest固定POST /api/user/manage、id/action:add_quota/mode:add/value、Create与SINGLE_BODY_LIMIT → send_fenced_until(request,Control,guard,deadline)。Ok仍仅表示2xx/success:true；deadline不意味着已发请求回滚。
- 由管理员客户端（`worker.client`）调用；旧管理员所有者就是管理员本人，root 可给自己加（架构 §2.9）。
- **正确性论证**：非正ID/越界金额在构造前NotSent；每次准入均查guard及deadline，只有明确401允许一次恢复后重放（原请求在鉴权前被拒），其它失败不重发。准入截止由§5.3.4保证，已准入执行≤原HTTP上限；交付仍按原证据，不能把超时当撤销。副作用为一次逻辑add及有限认证恢复/预算消耗，不写目录库。

#### 5.3.4 `newapi/transport.rs`：仅余额调用使用的有界准入入口（IR-M2）

- **接口**：新增send_fenced_until(request,class,guard,deadline)。与原send_fenced共用send_inner(optional_deadline)；旧调用传None，队列64槽、Control优先、2在途/1后台、速率与HTTP_TIMEOUT均不改。不是给整个请求套timeout。
- **未发送等待**：新增admit_checked_until(class,check,deadline)，只对budget.admit_checked future用timeout_at；Admission的既有Drop移除未准入waiter。最终check同时要求Instant::now()<deadline；即使future在边界先Ready，返回后也再查，过期则drop admission、QueueFull/NotSent、零原请求执行。不重置截止，不因队列通知/429续冷却而延长。已过期/64槽满均立即明确失败。
- **认证锁与登录预算**：refresh_session_until把同一deadline传给等待refresh锁、登录预算的准入；等待锁超时只取消本等待者，不取消持锁的其他登录。取得锁后沿原generation/auth判定；登录尚未准入的超时不设置RefreshCancellation熔断，因为没有发登录。登录已准入则等原execute完成/10s超时，正常落定认证状态；普通业务deadline不取消这个执行阶段。随后若已过deadline，不再发原资料/add或401重放，返回QueueFull/NotSent。原request与重放、初始Pending认证和401恢复均携带同一绝对截止；不得漏回无截止refresh_session_locked里的admit。
- **已准入请求**：只受原execute的10s/body上限约束，不被普通deadline中途取消；若deadline到时它还在执行，逻辑调用可晚于deadline结束，保留真实delivery分类。401的登录与重放仍至多各一次，截止后不能再准入新HTTP。余额sender对显式认证熔断不主动retry_authentication。
- **stop/异常**：运行树stop仍可取消future；认证中保留原RefreshCancellation行为。若取消的是潜在add，§5.4.3的RefillAttemptGuard负责不确定结算，不能将deadline的明确NotSent与任意取消混同。
- **论证**：每个等待段共享有限deadline，取消未准入等待只清自己的队列/锁waiter，不干扰已有认证；任何截止时刻至多有一个该调用已准入HTTP，故正常deadline之后只可能等其剩余≤10s执行，不会继续新等待或重放。持续Control/重复冷却可使Observe超时，但不能使余额调用永不返回。同步锁/SQLite工作仍服从既有短临界正常返回的运行前提，不声称硬实时结束；不提前释放仍在执行的业务/预算槽。

### 5.4 `reconcile/worker/balance.rs`（新）

#### 5.4.1 构造与所有者
- `ReconcileWorker` 增加字段 `balance: Balance`。`new()`（`worker.rs:201-245`）在把 `store` 移入 `Core` 之前读 `store.balance_policy()?`，以空 `accounts` 构造。
- `new_with_token_owners`（`token_service.rs:127-146`）在写入 `token_owners` 后，按 `TokenOwners` 生成 `accounts`（issuer 在前、legacy 在后）。
- `token_client` 改为 `pub(in crate::reconcile::worker)`，供本模块取所有者客户端。
- 所有者集合从构造完成的不可变 `token_owners` 生成，后续不动态增删；按此集合调用 `token_client(scope)` 不应失败。若仍失败，记该账号 failure 并继续观测其他账号，不用 `?` 中止整轮；不记录补足尝试。构造前读策略失败则不产生半初始化 worker。
- **正确性论证**：前置为store已通过v9校验、TokenOwners沿原构造器核验同实例/同预算及不同正ID；先读取单行策略再移动store，成功后槽位与DB一致，账号只由固定owners生成，不按远端响应改变身份。失败时沿原RAII回收资源，余额任务尚未启动，无半初始化后台写。

#### 5.4.2 `pub(crate) async fn balance_round(&self) -> ControlResult<()>`
- **实现**：
  1. `accounts` 为空 → Ok。
  2. `quota_unit_until(Instant::now()+BALANCE_READ_BUDGET)`：成功更新unit，失败保留旧值。等待Observe即使被Control饿住也在自己的截止结束。
  3. 各所有者依次取固定客户端；取不到按§5.4.1记failure继续。`observe_profile_until(Instant::now()+BALANCE_READ_BUDGET)`：成功写reading/清failure，失败记failure保留旧读数；仅短暂持槽位锁。
  4. 自动预筛沿原开关/读数新鲜/阈值/封锁规则；命中调用refill(Auto,None)，**仅忽略非Storage错误**，Storage立即向上返回，不能吞成Ok。
- **退出**：槽位中毒或refill的Storage向上报致命故障，其它读取/业务错误记结果后继续。至多3个读调用、2个自动补足；新增异步等待都依§5.3.4/attempt_end有截止，已准入HTTP至多延后10s，不再以单次HTTP超时替代排队有界证明；同步短临界正常返回是既有前提。
- **正确性论证**：本轮的观测路径不取Core、不经mutate、不递增栅栏；自动补足调用另依§5.4.3短暂读Core。读失败只写failure、保留旧读数，快照因失败或过期判未知，旧低读数不触发自动补足（I9）。预筛不是授权：封锁/退避以1b互斥内复核为准，开关/金额以第4步当时策略与新鲜读数为准。

#### 5.4.3 `pub(crate) async fn refill(&self, scope: TokenOwnerScope, amount: RefillAmount, source: RefillSource, route_deadline: Option<Instant>) -> ControlResult<Option<RefillReport>>`
- **调用关系**：调用方 `balance_round`（自动）、`dispatch`（手动）；被调用 `token_client`、`profile`、`add_user_quota`、`code`。
- **实现**（与架构 §2.9 补足函数各步一一对应）：
  1. 入口固定attempt_end：Manual要求route_deadline=Some并使用它，缺失InvalidInput；Auto用当前tokio Instant+AUTO_REFILL_BUDGET。所有者未配置NotReady。Auto try_lock被占QueueFull，Manual timeout_at(attempt_end,lock)；未取得锁的失败不记last_refill。所有者互斥持续到结算/回读结束，潜在投递后由RefillAttemptGuard持有。
  1b. 持互斥读槽位，按此顺序：`now < locked_until`（两者）或 `Auto && now < backoff_until` → `NotReady`（**不记录**，以免覆盖之前的「结果不确定」记录）；`Manual` 且 `route_deadline` 已过 → `NotReady`（记录 `not_sent`，让已收到 504 的用户看到「未发送」）；dry_run → `NotReady`（不记录）。
     **进入第 2 步前释放槽位锁；持槽位锁时绝不取 Core。**记录未发送时 amount=null。锁中毒无法安全记记录时返回 Storage，不伪造槽位结果。
  2. 短暂锁 Core 并核 store 可用，失败为 Storage；`backend.ready_at(now_s)` 为假 → NotReady；否则记 `start_s=now_s` 与 `lease_end_s=backend.expires_at()`。释放 Core。
  3. 取固定客户端，调用profile_until(min(attempt_end,now+BALANCE_READ_BUDGET))。失败更新failure、保留旧读数，返回NotReady，不封锁/退避；成功把before保存在栈上并更新reading/清failure。此时取消还无潜在add，不创建不确定补足记录。
  4. `ToTarget` 在本步短暂读槽位的**当时策略**：若自动开关已关，或 `before ≥ warn_below`，返回 Ok(None)、不记录；否则用 checked arithmetic 得 `amount=min(refill_to-before,BALANCE_CAP)`。`Exact(a)` 用 a，不受自动开关影响。校验 `1 ≤ amount ≤ BALANCE_CAP` 且 `before.checked_add(amount)` 存在并 ≤BALANCE_CAP，否则 InvalidInput、记录 not_sent/amount=null；仅校验通过后保存 Some(amount) 供后续记录。释放槽位锁后才进入网络步骤。
  5. 无await地把所有者互斥移入RefillAttemptGuard（settled=false），随后调用管理员add_user_quota_until(owner.id,amount,guard,attempt_end)。guard要求start_s≤now_s<lease_end_s且tokio Instant<attempt_end，包括401重放的每次新准入都复核；不比较正常publication。截止后零发送指不再准入新请求；已准入请求可晚于截止传输/返回，仍按真实证据结算。stop在排队期取消时也可保守封锁一个实际上未发出的尝试，不能把未知取消误报NotSent。
  6. 严格按以下顺序分类（E1/E13）：Ok → Refilled；Err 且 delivery=NotSent → NotSent（RevisionConflict / 连接建立失败的 TransportUnknown 改为 NotReady，其余沿 code）；否则若 `e.delivery == ResponseReceived && e.status.is_none_or(|s| s < 500)` → Rejected(code(&e))，设自动 backoff_until；否则 → Uncertain，**在释放所有者互斥前**设 `locked_until=backoff_until=now+600_000ms`。None 对应 2xx + success:false；HTTP 500 + success:false JSON 必须走 Uncertain，即使 delivery 为 ResponseReceived。
  7. 第6步成功记录终态后，置RefillAttemptGuard.settled=true；Refilled再用profile_until(min(attempt_end,now+BALANCE_READ_BUDGET))回读，成功更新reading/after，失败仅记failure、after=None。截止已过不新发回读，按读取失败记未知；回读阶段取消不推翻已落定Refilled。所有者锁由guard保留到本函数退出。
  - 从第 2 步起，每个终态都写 `last_refill={now, source, amount, outcome}`；第 4 步 Ok(None) 不记录，第 1 / 1b 的例外以本节为准。第 6 步先记录补足结论再作回读，回读只改观测；失败不会将 refilled 改成 uncertain。若锁中毒等故障不能安全记录，返回 Storage 而不掩盖故障。封锁、退避与终态都在同一所有者互斥内写入。
- **退出**：Refilled → Ok(Some(report))；自动不需补 / 已关开关 → Ok(None)；NotSent(c) / Rejected(c) → Err(c)；Uncertain → Err(TransportUnknown)，由 dispatch 映射 ExtraCode::ResultUnconfirmed/504。Manual 若意外得到 Ok(None)，映射 Internal/500（规则 R 为假），不伪报成功。
- **显式假设**：E1、E2、E3；架构 §5「服务端处理在封锁期内结束」。
- 第5步guard还须原子检查balance.failed=false，1b/round/view也先检查该位，true时Storage/零add。这是无法结算后的不可恢复故障关闭，不是将普通publication代际重新纳入余额准入。
- **取消退出（IR-M1）**：第5步前drop只释放锁（无潜在add）。第5步起，RefillAttemptGuard的Drop若settled=false，在其持有的所有者锁仍有效时同步锁槽位，记uncertain/source/Some(amount)、locked_until=backoff_until=取消结算时刻+600000ms，之后才让owner_guard字段析构解锁；不创建脱离监督树的结算/补发任务。正常第6步也先写记录/封锁，再置settled。槽位中毒或无法安全结算时Drop不得panic，解锁前只设置balance.failed=true，不写全局publication；round/view/1b检查failed返回Storage，add的guard也检查此不可恢复故障位（不是正常发布代际），后续零add。此故障保护的状态在Balance中有明确定义，不依赖伪造Mutex中毒。已确定结论后取消回读保持原结论。独立Native仍可晚到，但已有手动job会在1b看到封锁而非读旧余额再发第二笔。 真正catalog CommitUnknown仍仅由§5.4.4在Core内封全局栅栏；私有余额槽位故障不破坏并发mutate/publish协议，Storage仍沿原监督树停止控制器。
- **正确性论证**：
  - 前置：所有者已配置；互斥与槽位锁顺序为「所有者互斥 → Core（短暂）→ 槽位」，且 `Core` 与槽位都不跨 `.await` 持有 ⇒ 无死锁。
  - (a) 第 5 步之前的任何返回都没有发出 add ⇒ 对外错误码都是「什么都没做」（规则 R 为真成立）。
  - (b) guard 由 E2 保证只在准入时检查：租约到期或路由截止后不会发出（I5、N6）；准入后发出的请求，其结论由第 6 步按投递证据分类，≥500 一律不确定 ⇒ 不会把可能生效的补足报成「明确拒绝」（I8）。
  - (c) 正常结算和取消Drop都在同一所有者互斥内先封锁再解锁，1b复核封锁，保证本进程内顺序。第4步仅证明每笔amount及before+amount有界，不推出实际余额≤cap；退款反例合法，用户已确认I10收窄，超JSON_SAFE资料仍按I9未知处理。
  - (d) 第 3 步的资料读取核对 id（E4），排除「不存在的 id 回成功」；`add_user_quota` 拒绝 `user_id ≤ 0`，排除 `id = 0` 命中 root。
  - 副作用：至多两次逻辑资料读、一次逻辑add及既有401恢复，槽位/日志写；不是exactly-once。§5.3.4限制新增队列/认证锁等待，已发HTTP有10s上限；普通响应超时不释放真实业务槽。stop可取消auto future，但Drop先结算再解锁，join blocking句柄不等于HTTP已完成。重启丢封锁为既有边界，不作跨重启去重保证。

#### 5.4.4 `pub(crate) fn set_balance_policy(&self, expected_revision: i64, draft: BalancePolicyDraft) -> ControlResult<BalancePolicy>`
- dry_run → `NotReady`（bridge 已先拒，双保险）。
- `let mut core=self.core.lock()`，不经 mutate → 保存得到 result → **无论成功或失败，先检查 store.usable()**。若不可用（尤其 CommitUnknown），在仍持 Core 时 `publication.store(u64::MAX, SeqCst)`（参照 worker.rs:258）并返回 Storage，不再更新缓存。若仍可用且 result=Ok，持同一 Core 临界区再锁槽位更新 policy；可用且 Err 则原样返回，不改缓存。
- **正确性论证**：Core → 槽位的固定锁序保证提交 / 缓存顺序一致。成功与明确回滚（Busy、409、422）都不递增 publication/input_epoch，不打断其他帧；提交未知时不能保持原偶数 gate，单向封为 u64::MAX 使 Key / lane 的旧帧不能再发。该失效保护不是正常策略发布，属于保持存储故障既有失败关闭性质（T-B9）。不允许为了「不动栅栏」而忽略 store 中毒。

#### 5.4.5 `pub(super) fn balance_view(&self, at_ms: i64) -> ControlResult<BalanceView>`
- **功能 / 调用关系**：`panel_snapshot_at` 持 Core → balance_view → 短暂锁槽位，按 §4.3 的纯映射构造 BalanceView；锁中毒为 Storage，不返回半份视图。
- **正确性论证**：先按缺失 / failure / 负年龄 / 超龄分支判未知，再对有效读数作穷尽分类，故负余额不是哨兵、失败不变零；封锁 / 退避按 captured_at_ms 投影，记录逐结论展开且保留字符串 ID。账号有界至多 2 项、无网络或存储副作用，锁序只有 Core → 槽位，构造完成才发布快照。

### 5.5 `runtime.rs`：`async fn balance_loop(worker: Arc<ReconcileWorker>, mut shutdown: watch::Receiver<bool>) -> RuntimeResult<()>`
- **功能 / 调用关系**：start 将 balance_loop 放进既有 JoinSet；循环调用 owned_job → balance_round → control("balance", result) → delay。与 glm_loop 并列，首轮启动后立即执行。
- **分支 / 退出**：每个 job 前检查 stop；owned_job 返回 None / delay 被 stop 中断 → Ok 退出；非 Storage 业务错误记录后继续；Storage / join panic 沿原 supervisor 统一停机，不静默吞损坏。不得另套可取消 timeout 包住 profile / 登录，也不新建脱离监督树的任务。
- **正确性论证**：轮次串行、至多两个所有者，不积压tick；每个新增异步等待由§5.3.4/attempt_end约束，不假定Control会留空隙。Storage从refill贯穿round上报；stop不开始新轮，owned_job取消未来时由RefillAttemptGuard同步先结算后解锁，再等待实际blocking句柄。没有脱离监督的任务；正常同步短临界返回前提下受控关闭，T-B14须用真实refill链验证，不能只模拟round错误。

### 5.6 管理 API

- `route.rs`：`Route::RefillBalance`（`POST ["balance","refill"]`）、`Route::SetBalancePolicy`（`PUT ["balance","policy"]`），允许方法表与强类型匹配两处同时加（否则落到 `:185` 返回 500）。
- `input.rs`：两个请求 DTO 按 §4.3，amount 用 `safe(v,1)` 且 ≤BALANCE_CAP；非法 owner_scope 的 string_enum 解码为 400 bad_request，数值越界为 422 invalid_input。`AdminRequest` 加两个变体，writes 默认算写、dry_run 为 403。`budget()` 的补足独立分支拟为 60s（见本节待定方案），改设置仍为 10s，其他预算不变。
- `bridge.rs`：Supervisor::start 原样传入 `pending.deadline: tokio::time::Instant`；`dispatch(worker, request, deadline: tokio::time::Instant)` 与 refill / timeout_at / guard 全程同一种时钟，不转 std。
- `dispatch.rs`：RefillBalance → refill(Exact(amount), Manual, Some(deadline))。Ok(Some) 构造 BalanceRefilled；Err(TransportUnknown) 映射 **ExtraCode::ResultUnconfirmed**；其它 ControlError 按原映射。意外 Ok(None) 为 Internal/500。SetBalancePolicy 交原同步保存，输出 BalancePolicySaved，不发原生请求。
- `output.rs`：两个成功回复固定 200；内部 user_id 用 to_string 构造 wire，全部字段包括 after=null 均输出。金额不以回读差值重算，保持原请求绑定。
- **正确性论证**：路由固定方法 / 路径与强类型解码把非法数据挡在 worker 前；writes 与预算表同步防止落入默认路径。有效请求才进入原四槽 bridge，deadline 随实际工作句柄传递；HTTP 超时不释放仍在运行的业务槽、不宣称回滚。dispatch 只在 worker 确认补足后回 200，未知统一 Extra 504，所以前端 R 不会将未知误作拒绝；响应字符串 ID 与严格 schema 保持跨语言一致，无新增错误码。

**路由预算方案（2026-11-02 用户确认 60/70/75；B-m9）**：

| 环节 | 当前值 | 仅 refillBalance 的已批准配套 |
|---|---|---|
| AdminRequest 业务预算 | v1 拟30s | 60s |
| 浏览器 work ticket | 40s | 70s，保留响应读取余量 |
| HTTP 单连接上限 | 45s，减5s响应余量后约40s | 75s，仍保留5s响应余量 |

只改业务预算会被另外两层截断，三层必须同批调整；其余管理路由的 10/30s、浏览器 state 12s / work 40s、连接45s、16连接及4业务槽保持原值。有效业务截止仍为 `min(entered_at+60s, connection_end-5s)`，慢上传可能缩短可用时间；60s 是最多等待预算，不保证限流冷却 / 排队必在其中结束。超过业务截止仍返回504，由按账号和时间显示的 last_refill 消解；不能自动重发 add，也不能把一条早于本次提交的历史记录当作本次成功。

`panel_http.rs`拟移走accept统一45s外层timeout；read_head仍使用原头部超时。授权通过、POST、精确refill路径且无query才选75s，其它45s，**connection_end=accepted_at+selected_timeout**，绝不从解析完成重新起算。body/business/respond剩余阶段整体timeout_at(connection_end)，错误响应仍沿原45s和5s写超时。不能只改内部值而留旧外层45s。客户端固定refill ticket70s，其它不变、时限不从表单取；慢头/body时客户端仍可能先超时，不能保证读回所有504。T-B10额外锁住accepted_at锚点、慢头及准确方法/路径/授权，16/4槽归属不变。

T-B10 增加有界时钟测试：在原40s之后、60s之前完成的补足得到200；过业务截止得504且未发送的 add 被 guard 挡住；75s连接上限、其它路由45s及四槽所有权均保留；客户端相应在40s后仍可接收本动作、70s到期拒绝迟到结果。**用户已确认具体配套，架构 §2.9 已同步；新增边界已由独立 subagent 技术复核，实施门禁仍须验证。**

### 5.7 快照（`reconcile/worker/panel.rs`）
`PANEL_SCHEMA_VERSION = 2`；`PanelSnapshot` 加 `pub balance: BalanceView`；`panel_snapshot_at` 在组装时调用 `self.balance_view(at)`。`panel_http/tests.rs:24-41` 的 `empty()` 补 `balance`（空 accounts、默认策略、`unit: null`）。容量测试（`panel_http/tests/capacity.rs:302-306`）把 `balance` 的最大宽度（2 个账号）计入根估算。

### 5.8 前端

#### 5.8.1 schema 与契约生成
- **功能 / 调用关系**：构建时从三份 §4.3 schema → `sgm6_b_contract.mjs` → contract.mjs，运行时代码只 import 生成的字段、枚举、上限。
- `shapes` 固定映射：balance → `pd.balance`，balanceAccount → `pd.balance_account`，balanceRefill → `pd.balance_refill`，balancePolicy → `pd.balance_policy`，quotaUnit → `pd.quota_unit`，balanceRefilled → `rd.BalanceRefilled`，balancePolicySaved → `rd.BalancePolicySaved`。enums 加 balanceState、refillSource、refillOutcome，来源分别为三个对应属性的完整 enum。`LIMITS.balanceCap` 从 `pd.balance_policy.properties.refill_to.maximum` 生成，不在 JS 再手写4e15。
- **正确性论证**：生成器只读 schema，统一导出七族字段与三枚举；先变更 schema，再重生成且 `--check` 精确比较，可以发现漏族 / 漏字段 / 漏版本。无运行时业务写入；实际正反报文与 Rust → JS 校验另由 §7 wires 验证，不能只凭生成器通过宣称契约正确。

#### 5.8.2 `core.mjs`

**`usd(unit) -> bool`**：功能为判定美元输入是否可用；供格式化、表单、buildAction 调用。输入的非 null QuotaUnit 已通过严格字段校验，判定 `unit !== null && unit.display_type === 'USD'`（比例仍须正、有限）；trivial。null 或非 USD 只显示原始额度，**禁用金额输入**，不开放整数原始单位输入替代（F-M1，回到架构）。

**`validateState(value) -> state`**：功能为沿原 record 校验新增 balance，供客户端 state 回复使用。stateFields 加 balance；七族字段集合均取 WIRE_FIELDS，时间 / 金额 / ID / 枚举取原校验器与 LIMITS。除 §4.3 的可空规则外，须校验：
- 账号范围、ID 唯一且顺序 issuer 在前；非 unknown 的 state 与 quota / warn_below 一致，读数及时间非空且年龄在0..=180000ms。
- `unknown_reason !== null ⇒ state==='unknown'`；quota 与 observed_at_ms 同空；failure 或读数缺失 / 过期 / 负年龄不冒充充足。
- locked_until_ms / backoff_until_ms 非 null 时必须晚于根 captured_at_ms；last_refill 的 outcome / amount / error_code 符合 §4.3 映射；policy 的三值满足不等式。
- 失败抛 ProtocolError（对服务器回包的拒绝），不把坏数据降成零。论证：完整结构、关系和数值校验都通过才发布前端 state；同份快照与 captured_at 比较，不拼接不同时刻的余额；账号至多2项、有限字段验证终止、无写入副作用。

**`validateReply(family, value, request)`**：新增两族，供 requestJson 调用。balanceRefilled 校验 scope 与 amount 分别等于原请求，user_id 为正整数字符串，before/after 为允许范围内的整数 / null；balancePolicySaved 校验三项等于原请求，revision 为 expected 或 expected+1（后者还需未超过 JSON_SAFE）。不要求 after=before+amount，因为有并发扣费 / 退款；不把 after=null 当失败。论证：结构校验 + 请求绑定阻止错账号 / 错金额 / 错设置成功回执；有限比较无副作用。

**`decimalRatio(number) -> {p: bigint, q: bigint}`**（私有）：把已验证正有限比例的`Number.toString()`规范十进制串转为整数分子p、分母q。支持该规范串的科学计数法，按小数位/指数移动10的幂，p/q>0；数字至多17位有效数、指数在IEEE有限数范围，转换规模有界。比例语义明确取规范十进制值，不解释成二进制尾差，也不读取未经验证的用户比例串。仅纯换算，无网络/存储副作用。

**`quotaFromDollars(text, unit) -> integer`**：buildAction调用；先ensureAction(usd)，校验 `/^(0|[1-9][0-9]{0,349})(\.[0-9]{1,2})?$/`（最多350位整数+2位小数，总长≤353），把美元文本直接转整数分c，不先转Number。用decimalRatio得p/q，N=c*p、D=100*q，半上舍入为整数商 `(2*N+D)/(2*D)`；要求1≤结果≤BigInt(LIMITS.balanceCap)后才转Number。所有失败为ActionError。禁止Math.round(Number(text)*比例)及通用epsilon。论证：商余数精确表达十进制半上舍入，界限检查前无Number精度丢失，结果≤cap<JSON_SAFE可安全转换。向量：0.29×50→15、0.57×50→29、2.01×500000→1005000、0.01×7.3→0而拒绝；8000000000.00×500000恰cap，再加一分拒绝。BigInt与有界字符串处理为现代浏览器能力，bootstrap能力检测同步登记，缺失时沿原只读兼容提示，不用BigInt字面量造成旧解析器整模块无法加载。 规范最小比例5e-324下cap对应339位美元整数，350位域覆盖全部合法预填而不限制比例；输入/BigInt运算仍有统一上限。unit=50的默认target=1e15对应20000000000000.00必须可提交，最小比例及超长度反例入同一测试。

**`formatQuota(quota, unit) -> string`**：null返回「未知」；USD用同一p/q把原始整数绝对值精确换算到美元分后半上舍入，按字符串分组为en-US千分位/两位小数，负号在$前（−10955000/500000→-$21.91）。非USD显示原始额度，不猜汇率。不得用可能溢出/丢精度的Number除法作为格式化依据；正有限比例的规范指数和安全整数输入使BigInt/输出字符串规模有界，覆盖负数/极小比例/零/null，纯展示不反用于用户金额文本。

**`balanceObservations(snapshot, observation) -> private view`**：每次既有1s tick调用paint时，在core的observations中计算余额投影。对各账号，展示年龄=`captured_at_ms-observed_at_ms+observation.age`（该age已含保守服务端/客户端延迟）；负年龄、严格>180000ms、根快照不fresh或原账号已unknown，都只降为unknown，不能把后端unknown本地升级。保留原quota/时间为历史，输出effective_state/reason/fresh，不改原wire。chip、余额栏、预填、近似上限与错误提示共用此投影；渲染戳纳入effective状态/原因/新鲜度，避免只依赖根fresh。无需逐毫秒重建DOM，只有投影类别变化才触发guard重绘。论证：所有使用点以同一保守年龄降级，179999/180000有效、180001未知，根快照仍fresh也不能保留绿点。

**`currentEntity` / `buildAction`**：签名追加私有可选第六参数balanceContext（其它动作不使用）；两个余额动作由app.submit传入本次balanceObservations及冻结换算上下文，缺失为ActionError。设置动作先ensureAction(draft.entity==='balance')；currentEntity独立比较policy.revision，不落入control_revision。refillBalance校验范围，按投影新鲜度作可选近似金额预校验，生成原DTO、kind=null；setBalancePolicy生成原expected_revision及设置、kind=null。此context不进入HTTP请求或wire。
金额可输入时，对设置中的每个金额字段分别比较用户文本与冻结原始值对应的原预填文本：未编辑（包括原值小于一美分导致的空初值）保留冻结raw，只有修改字段才用上述精确换算，避免仅改开关或no-op时悄悄改变合法的非美分网格额度。换算不可用时设置动作只读取checkbox，warn_below/refill_to从冻结策略原样带回，忽略禁用文本。打开时冻结换算上下文，提交时比例变化则ActionError重新确认。最后检查设置不等式。金额近似预校验须由caller传入当前balanceObservations的新鲜性，不能直接用wire中的旧state；读数投影未知时跳过近似上限，仍由后端新鲜读取决定。论证：版本戳防旧覆盖，未编辑/单位缺失均不改原金额，所有用户校验先于发送。

#### 5.8.3 `client.mjs`
- **功能 / 调用关系**：buildAction → route → requestJson → validateReply。refillBalance：POST /balance/refill、family balanceRefilled、write；setBalancePolicy：PUT /balance/policy、family balancePolicySaved、write；均status200。
- **关键接线**：requestJson 把 usage、refillBalance、setBalancePolicy 三个动作的**原请求 body**传给 validateReply，其它动作仍用 descriptor.query；两新路由不依赖空 query 中的 owner_scope。body 来自 buildAction 的不可变提交载荷，发送与校验使用同一份，不能在 await 后从当前表单重新取金额。
- **正确性论证**：固定路由不接受任意 URL / 方法，保留原 body / 响应上限、鉴权边界与 R 分类；成功回复先校验200再校验请求绑定，因此不会因 query 缺字段把一次成功误当协议错误。过期与取消仍按原 ticket 规则返回未知，不自动重发。§5.6 的70s是仅本动作的已批准配套，原其它时限不变。

#### 5.8.4 `app.mjs`

**`action` / `openRefill` / `openBalancePolicy`**：新写动作分支必须在既有 `if (this.readOnly) return` 后；供相应按钮调用。无账号则没有余额栏、没有入口；scope找不到时ActionError。说明完整写为「所有组的访问 Key 共用这个账号的 new-api 内部余额；这里的设置对所有组及已配置的两个账号生效，与智谱 / 火山套餐额度无关。预警线应远大于约 10 分钟的消耗。」
- openRefill捕获scope，!usd时禁用金额及提交并解释；quota=null或balanceObservations已判未知时不预填。否则用BigInt求R=min(max(target−quota,0),cap)，预填美元分=`R*100*q/p`的整数商（向下截），输出无$和千分位的纯数字，0时空白；不经二进制除法再floor。预填只是建议，后端仍重新读取。
- openBalancePolicy 使用 freezeDraft('balance', state.balance.policy)。无美元换算时禁用两个金额字段，只允许改自动开关；renderer.form 增加静态 field.disabled 支持，submit取值后buildAction仍按 §5.8.2 保留原金额，不能相信被禁用的DOM值。
- 论证：入口服从原只读 / 过期 / dry_run规则，冻结范围和设置版本防止后续切组改错账号；预填受每笔上限限制且不向上舍入，不承诺快照就是当前余额；无账号 / null / 单位不可用都显式处理，只有用户确认才提交。

**`submit` / `rejectSubmit` / `rejectionText`**：两新动作仍走kind=null同步分支，不进操作ID追踪。捕获本次action、editor、scope、冻结载荷和提交时间；成功按 §5.8.3 绑定后关闭本次确认框，分别显示 `已补足 ${formatQuota(value.amount, unit)}`、`余额设置已保存`。非R（含504、连接未知、协议错误）关闭本次确认框，分别提示「结果待核对：请看该账号的上次补足结果及其时间」/「结果待核对：请看当前余额设置及修订号」，不重发。
- rejected为真时保留editor身份检查，rejectionText收原动作/scope；not_ready为「本次补足未发送；后端未就绪、请求截止或账号处于封锁期」。当前余额投影新鲜且有locked_until_ms才计算约等待秒数，否则提示等待信息未知，不捏造0。额度近似值同样依赖余额投影新鲜，而非仅根快照fresh。
- refillBalance 的 invalid_input 覆盖为金额 / 上限说明；若有新鲜读数，附「按上次观察，最多还能补约 formatQuota(min(LIMITS.balanceCap,max(LIMITS.balanceCap−quota,0)),unit)，以服务端新鲜读数为准」。读数null/快照旧时不显示最大数值。
- buildAction 在本地抛出的 ActionError（金额 / 超额 / 单位 / 版本）对这两个动作写入**本次**editor.rejection并重绘，不仅写顶部notice；editor已换则只写notice。通用REJECTION_TEXT与其他动作保持不变；超时不能写成「未发送」。
- 论证：通过原R才可作明确拒绝提示；区分前置本地校验、业务拒绝、未知、成功，且保留editor身份，避免改写用户另开的表单。记录时间避免将历史last_refill冒认本次；所有可选读数都有缺失 / 旧快照分支，最多额度明确只是近似。

#### 5.8.5 `render.mjs`
- **功能 / 调用关系**：既有tick→paint→observations（新增balance投影）→frame→chips/groups/identities/balanceStrip。戳加入balance、effective状态/原因/新鲜度及vm.readOnly；accounts为空隐藏。使用原每秒paint即可跨180秒边界降级，不能只等下一份服务器快照。
- chip取 `exhausted > unknown > low > sufficient`，同状态按issuer、legacy顺序展示原因；快照过期时覆盖为「未知（快照过期，以下是上次观察）」，不得出现绿点。状态与原因表如下：

| 状态 | 颜色 | 文案 / 原因来源 |
|---|---|---|
| sufficient | dot ok，仅新鲜快照 | 「内部余额充足」；读数≥预警线 |
| low | dot warn | 「内部余额偏低」；追加自动补足已关闭（policy）、封锁 / 自动退避（截止字段）、上次补足失败及错误码中文原因（last_refill）；均无则「等待下一轮自动补足」 |
| exhausted | dot bad | 「内部余额已用完，转发会被 new-api 挡住，与智谱 / 火山额度无关」；同样附可用的关开关 / 失败 / 退避信息 |
| unknown | dot warn | unknown_reason非null按既有中文码说明；否则按缺读数 / 负年龄 / >180000ms给「尚未读到 / 读取时间异常 / 读数已过期」 |
| 快照过期 | dot warn | 「内部余额未知（快照过期）」；保留上次金额和读取时间但不声称当前状态 |

- balanceStrip按账号一行显示范围、user_id、金额/未知、读取时间、上次补足的来源/结论/尝试金额/错误码与时间，封锁 / 退避显示「采样时约剩N秒」，**静态显示**，随新快照刷新，不新增updateLive倒计时类型。两账号按钮无障碍名分别含范围，例如「补足签发账号余额」「补足旧管理员余额」，id仍传scope；按钮disabled均服从vm.readOnly。
- 补足未知结果不因余额数值变化而自动改为成功，保留服务端last_refill结论。设置说明与 §5.8.4 完全一致。复用现有dot.ok/warn/bad、toolbar等类（panel.css:29-31已有），本稿不需新增CSS类；若实现确需新增则先补清单与设计，不临时换视觉风格。
- **正确性论证**：只消费同一已验证快照，最差排序不会以low遮住unknown；新鲜度进入stamp，余额更新或过期都会重绘，旧数据不能保留绿点。账号与模型字段均按文本渲染，最多两行、无网络或业务写入；所有按钮只派发原动作，不能由展示直接补足。

## 6. 测试

| # | 名称（层） | 内容 | 变异验证 |
|---|---|---|---|
| T-B1 | `balance_policy_defaults_validation_and_revision`（Rust catalog） | 新库默认行；warn≥target、target>cap、0→422；版本冲突409；不变不递增 / 有变化递增；revision=JSON_SAFE且有变化时bump失败422、全部回滚 | 去掉修订号比较 → 失败 |
| T-B2 | `v8_migration_adds_default_balance_policy_and_backup`（Rust catalog） | v8 库迁移：镜像不变 + 默认行、实例 id 不变、`catalog-v8-before-v9-` 备份 0600 且通过 `verify_schema(8)`；冲突表 → 打开失败、原库不变；`verify_balance` 拒绝损坏行 | — |
| T-B3 | `balance_policy_busy_is_queue_full_not_storage`（Rust catalog） | 另一连接持写锁时保存 → `QueueFull`，库不变 | 映射回 `Storage` → 失败 |
| T-B4 | `add_user_quota_request_shape_and_delivery`（Rust newapi，`Fake`） | POST体精确；200+success:false→RemoteRejected/ResponseReceived/**status=None**；HTTP500+success:false JSON→ResponseReceived/status=Some(500)；非JSON429/超时→Uncertain；连接建立失败→NotSent；guard假/非正ID→入口计数0；401仅重放一次且重放前重查guard | 去掉 guard → 失败 |
| T-B5 | `quota_unit_parses_status_and_rejects_invalid` | 正常；缺字段 / 非有限 / ≤0 / 超长类型 → `InvalidResponse` | — |
| T-B6 | `refill_steps_and_outcomes`（Rust worker，`tokens_fixture`） | 1/1b/2–7每条退出；手动等锁超时不留新记录且零POST、取得互斥后已截止记not_sent；封锁拒绝保留旧uncertain；200+false/None只退避不封锁；**500+false JSON**与超时都为uncertain、尝试金额保留、封锁/退避截止=结算时刻+600000ms，再次手动在1b拒绝且POST累计仍1；不能只断言HTTP504。成功后回读失败仍refilled/after=null；路由过期与墙钟回拨guard均零发送 | 去掉1b检查、将500+JSON错误当Rejected → 对封锁/第二次发送的断言失败 |
| T-B7 | `auto_refill_amount_and_skips`（Rust worker） | 负读数且目标=cap时金额=cap；≥预警线不补；关开关/dry_run/退避/互斥被占跳过；第3步资料读取阻塞期间关开关，返回后第4步重读策略不得发送；两所有者并发各自有界、同一所有者串行 | 去掉min上限、去掉发送前开关重查 → 失败 |
| T-B8 | `balance_round_unknown_never_zero`（Rust worker） | 读失败 → 快照 `unknown` + 原因，`quota` 保持上次值或 null，从不为 0；即使旧低余额仍新鲜也不自动补足；成功读取后清失败并恢复按读数判定；3 分钟后变 `unknown`；补足成功后回读失败仍记已补足、`after=null` 且余额状态未知 | — |
| T-B9 | `balance_policy_write_does_not_bump_publication`（Rust worker） | 成功及明确回滚不动publication/input_epoch，库与槽位一致；注入CommitUnknown后store不可用、publication=u64::MAX，捕获的Key/lane gate不能再发，缓存不伪更新 | 成功改经mutate、去掉中毒后封栅栏 → 各失败 |
| T-B10 | `balance_routes_shape_budget_and_dry_run`（Rust HTTP） | 路由矩阵26→28、两族200及user_id字符串；dry_run403；非法scope400 bad_request、金额越界422；Uncertain对外为504 result_unconfirmed；tokio虚拟时钟验证等锁截止、截止后POST零、四槽不提前释放；§5.6待定超时链另有原路由不变与新路由跨40s成功案例 | dispatch不给请求上下文/截止、未知错误改502 → 失败 |
| T1 扩充 | `h2_first_submit_rejections_leave_no_record`（既有） | 按下方专门说明扩充⑨过期设置409、⑩金额合法但余额+金额超cap在worker第4步422，直接读取策略/last_refill；管理入口真实计数，不能用未知路径panic作零发送证据 | ⑨误写策略、⑩错误发送一次可识别add → 断言失败 |
| T-B11 | Node测试组（下表给出精确名称） | 完整状态/回复/请求绑定、金额/格式向量、版本冲突、单位禁用、app成功/拒绝/未知、客户端状态矩阵 | 永久变异清单见§7 |
| T-B12 | 四浏览器测试组（下表） | 每项×4项目；逐项断言状态原因、成功路径、拒绝原框提示、dry_run、单位null及无账号，不能只验证DOM出现 | — |
| T-B13 | 手动真实rc.20探针 | 下方列10项预期，显式binary+SHA、私有临时实例与随机端口，退出停止自建进程；不进CI | — |
| T-B14 | `runtime::tests::balance_loop_runs_immediately_survives_business_errors_and_stops` | 首轮立即、非Storage错误继续、stop等待实际句柄；**真实refill第2步store不可用→balance_round→loop**必须传播Storage，不能只模拟round错误 | 吞Storage、首轮推迟、业务错误退出 → 失败 |
| T-B15 | `balance_transport_deadlines_bound_waits_without_cancelling_login`（Rust transport） | 持续Control/重复续冷却令Observe在20s截止明确结束、waiter/64槽归还；refresh锁被占的等待者到期不取消持有者；自己登录已准入再过deadline仍落定认证、不会熔断/发原业务；401恢复保留原截止、过期不重放；未选中的等待者也会按时醒来退出 | 漏掉队列/refresh锁/登录预算任一截止 → 对应受控用例失败 |
| T-B16 | `cancelled_refill_settles_before_unlocking_waiting_manual`（Rust worker/runtime） | 专用异步测试服务器先收首笔add、**实际quota写入等Notify**；手动已进入bridge等同owner；stop取消auto后先见uncertain/600000ms封锁再解锁，第二笔入口计数仍0。另测准入前取消的保守封锁、明确NotSent不误封、确定Refilled后取消回读不改结论、槽位故障仅failed关闭，并与真实mutate/publish交错：发布完成后余额仍零准入、原路由协议不受影响；catalog中毒另在T-B9 | 去掉Drop结算、提前释放owner_guard、回读取消重写结论 → 各失败；不能只用Reply.delay |
| T-B17 | `refund_does_not_change_pre_refill_bounds_contract`（Rust worker） | C=4e15，profile给C−100、amount100，add前退款50；允许实际C+50，断言仅有1笔SGM add、amount与before+amount有界，不把合法退款判Storage。资料超JSON_SAFE仍未知且不自动加 | 去掉发送前计算界限 → 生成的超额输入失败；不能把退款后实际cap当旧断言保留 |

**B-M2 / B-m7 的夹具与 T1 非恒真说明**：先扩展 TokenState：`/api/status` 能返回合法/非法比例；`/api/user/manage` 精确识别id/action/mode/value，支持成功并实际增加quota、200+false、500+false JSON、超过10s延迟（是否已加由具体用例明确），均返回合法Reply而非panic。入口 hook 在任何分支之前按路径 / 方法计数；通过一条正常add正例证明计数和原生quota确会变化，再使用零发送反例。

`observed()` 扩为目录快照、本地身份、**直接从Core.store读取的balance_policy**、**直接锁槽位读取的各账号last_refill**（需测试辅助时仅加 `#[cfg(test)]` 读取接口，不经发布缓存）。⑨先成功更新一次策略产生真实旧revision，捕获基线后用旧值改另一项，断言409、全部直接状态不变、无持久提交。⑩先设假后端余额为正（例如10000），amount=BALANCE_CAP本身合法，但before+amount>cap：确认资料GET已发生，worker第4步422；add入口计数0、原生quota不变、目录/身份/策略不变。

⑩不能盲目要求完整observed相等：按 §5.4.3，此拒绝**应**把目标账号内存last_refill更新为manual/not_sent、error=invalid_input、amount=null（读取时间也可能刷新），其他账号记录不变；要断言这个精确差异，且持久数据不变，不能允许任意槽位变化。I8约束的是落库记录 / 目录行；此处仍保留已确认的未发送尝试记录规则。这样同时响应B-M2与B-m2，不以修测试为由暗改语义。金额>cap的输入层422另留T-B10，不替代⑩。

**T-B11 精确新增Node名称与向量（实现时逐名登记）**：

| 测试名 | 必测内容 |
|---|---|
| `balance state and replies validate` | 七族字段 / 字符串ID，wrong scope/amount/策略值/越级revision拒绝；非unknown状态关系、配对null、未来封锁、last_refill四结论、issuer顺序；未知与快照旧分开 |
| `quotaFromDollars rounds and bounds` | 保留原向量，新增0.29×50→15、0.57×50→29；8000000000.00×500000恰cap，再加一分拒绝；比例科学计数法、下溢舍入0及超限均受控；使用独立整数商余数参考生成正例；unit50的默认目标14位美元文本、最小比例339位cap文本、350位/353字符边界及超长度拒绝 |
| `formatQuota preserves negatives units and grouping` | 原负数/分组/非USD/null向量；精确十进制预填不向上、极小正比例不格式化为Infinity；格式文本不能反用于输入 |
| `balance actions build exact bodies` | 精确DTO、金额disabled时开关可改且旧金额保留、错draft.entity/错scope拒绝 |
| `balance policy draft conflicts on revision` | revision变化ActionError，不允许undefined===undefined漏过 |
| `balance submit success binds body and shows result` | submitApp+真实route/requestJson/validateReply接线，两新动作200成功、正确提示、不进pending；错scope/amount/值为协议错误；真实openBalancePolicy→submit验证USD非美分网格合法raw策略的no-op/仅开关/单字段修改保持未编辑字段，含raw1/2空初值、单位/版本变化及主动清空拒绝 |
| `balance submit refusals stay inline and unknown stays pending check` | not_ready等候提示、超额近似值、设置409、金额本地ActionError就地；504/transport/协议错误为相应未知文案；editor换后不改新表单 |
| `balance forms disable unavailable currency and bound prefills` | unit=null/非USD金额禁用，开关可改；读数null空白；负数目标=cap预填截顶、美元向下到分、无格式符 |

`tests/ui/client.test.mjs`矩阵同步两个200路由及原body上下文。余额观察投影并入`balance state and replies validate`：同一根快照持续fresh、余额年龄179999/180000/180001ms，所有使用点随投影降级；后端unknown不本地升级。bootstrap缺BigInt只读回退、balanceContext缺失/跨版本/单位改变也列入相应用例，不增加隐形测试名。

**T-B12 四个新增浏览器用例（每个×4，共16项）**：
1. `balance states explain causes and keep rejected refill inline`：原状态/原因及原框断言外，固定同一根快照并推进浏览器观察年龄跨180000ms，根仍fresh时chip/余额栏也必须变未知，近似上限与预填不再使用旧读数。
2. `balance refill and policy save succeed with request binding`：真实前端提交两新动作到固定夹具路由，200后成功提示、确认框关闭；验证字符串ID、金额与设置。
3. `balance dry run disables writes and unavailable units disable amounts`：dry_run全写入口禁用；非dry_run下unit=null/非USD禁用金额，仍能仅改开关。
4. `balance empty owners hide controls and prefills preserve bounds`：无账号无栏 / chip；两个账号按钮名字不同；null读数不预填，纯数字、负余额/上限与截分边界。

**T-B13 手动探针预期（不进CI）**：沿 `newapi_token_probe.py`，只接收显式 `--binary`、核锁定rc.20 SHA256，随机未占用端口且禁止3000/3001、私有临时目录、所有者/监听证明后才初始化；仅假账号/余额，无供应商或生产数据。验证：①正常正add为200/success=true；②value≤0为200/false；③已登录普通用户以正确New-Api-User调用manage，**200/false、message为非空权限拒绝说明**，账号余额不变（锁定源码[AdminAuth/authHelper](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/middleware/auth.go#L128)）；④不存在正id为200/true但不改任何账号；⑤id0为200/true且root增加指定量；⑥/api/status合法换算字段；⑦root给自己加为200/true；⑧add ACK后立刻profile看见before+amount（无并发扣费）；⑨超int64的JSON整数被绑定拒绝、200/false；⑩非法mode为200/false。异常与成功都只回收自己持有的进程句柄，临时凭据0600、用后shred，报告只含脱敏结果；源码预期与实际二进制若有差异应报告失败，不把探针调成全接受。

既有测试需同步（E8、E9）：所有降版夹具加删 `balance_policy`（`tests.rs:30-36`、`token_migration_tests.rs:13`、`policy_command_migration_tests.rs:115`、`observed_publication_properties.rs:28`），`observed_publication_properties.rs:4` 的排除清单加 `balance_policy`，`token_migration_tests.rs:34` 的字面量 8 → 9 并加默认行，`:132` 的「未来版本」改为 `DATABASE_SCHEMA_VERSION + 1`；`tests/ui/fixtures.mjs` 的 `emptyState` 升 2 并加 `balance`；`tests/ui/core.test.mjs:28` 的变异改为 3；`panel_http/tests.rs:26` 的字面量schema_version改用PANEL_SCHEMA_VERSION，补默认balance。

时间测试不睡10分钟：route deadline / 等锁 / 请求延迟用tokio暂停时钟；余额视图传显式at_ms，封锁测试断言实际记录的墙钟截止差为600000ms并立即重提验证零发送，边界过期用受控夹具设定 / 纯判定输入。无需新增生产时钟服务，不把std Instant用于虚拟时钟断言。所有变异在独立源码副本与独立CARGO_TARGET_DIR执行，编译失败、未知路径panic或零测试不算捕获。

## 7. 清单与审计同步

- CI会检查的：先改三份schema，再生成contract.mjs，核七族 / 三枚举 / LIMITS.balanceCap；check:ui不能跳过。按§6计划Node **60→68**（8个新名称；client矩阵扩充不额外计名）、浏览器 **24→28个名称，96→112项**（B2 C2修正已新增一个交互回归，原23/92基线与历史审查数字保留）。同步sgm6_b_node_tests.txt、sgm6_b_browser_tests.json与reporter.test.mjs；如实现按真实行为合理拆分，必须同批列出新名与精确数量，不用宽松下界掩盖漏跑。
- T-B14采用文中精确runtime测试名，按字典序加入sgm7_tests.txt；所有fixture / schema版本 / capacity根宽度同步，不改并发度以绕过门禁。
- `record_wire`登记两新请求/回复；实际Rust成功样本用测试专用emit_balance_pair一次预留pair序号，输出同pid/序号的request_<族>-pid-id.json与response_<族>-pid-id.json，并在**balance-context子目录**写同response文件名的sidecar，内容只指向精确request_file（basename）。不改变其它族emit；子目录不进入既有顶层*.json schema扫描。mjs消费新回复时必须存在sidecar、验证basename/pid/序号/请求族对应，再读取**原请求body**交validateReply，缺失即失败，禁止从回复反造scope/amount/设置。错误样本沿原AdminError处理。补同一回复配错scope、amount、三设置、expected_revision的拒绝反例，证明配对有意义。
- 维护sgm6_b_wires.mjs/.py：新增两请求/回复、字符串ID/null分支；实际Rust回复族13→15，UI schema族21→25、正例至少36，并有数字ID/缺null/错误组合反例。sidecar只是测试上下文，不新增产品wire，不含凭据。主architecture.md同步§4共享类型/consumer（本轮已补设计概述），handoff测试范围补至T-B17。
- 永久JS变异改为：精确半上舍入改截断或退回二进制浮点乘法（0.29×50捕获，替代v2的删除Math.round）；去掉余额年龄投影；balance版本戳落回control_revision；漏传回复body上下文；拒绝不留原框。Rust独立编译变异：漏取消结算/提前解锁、漏准入/认证等待截止、吞Storage、漏guard/封锁、500+JSON当Rejected、漏CommitUnknown封gate、漏发送前开关复核。before子串必须唯一，精确语义失败并恢复复验。
- 前端文件清单包括client/core/app/render、fixtures、core/actions/client/reporter测试、四个浏览器用例、wires两脚本、生成器和清单；复用现有样式，不新增CSS类，故panel.css本稿零修改。若后续新增类，先更新清单并交复审。
- 历史冻结审计（`sgm6_a1.sh:78`、`sgm6_a21.sh:13`、`sgm6_a22*`、`sgm6_b_check --freeze`、`sgm7_check --freeze`）钉在旧 base，CI 不跑，HEAD 上已不全通过；本批不维护，只在 PR 说明中列出。
- `CLAUDE.md`「new-api 管理面没有『调用户余额』的 API」一条更正为：rc.20 有 `POST /api/user/manage add_quota`（陷阱：不存在的 id 回成功、`id = 0` 命中 root），自动化只走 API。

## 8. 部署与观察

- v8→v9打开时自动迁移、先备份；C3须同时确认迁移、回滚路径和首次原生加额。**本机首轮也会写new-api**：按交接读数预计qtadmin约1e8→1e15、qtsigner已约1e15则不补；实际仍以部署时的新鲜profile为准。本机与fermat都必须先得到部署确认，不能把v1功能确认当部署许可。
- fermat预计签发账号约1e12→1e15，旧管理员低于预警线也会补；这是两类真实所有者的原生写。明确同意前不读改受保护config/state/native，不启动迁移、补足或替换服务。
- 回滚方案A：停控制器、确认目录锁已释放，保留一份当前v9一致备份，再换回旧二进制并从catalog-v8-before-v9备份恢复。**备份时点之后新增/修改的目录数据会丢失**，须先核对原生新增渠道/Key及归属，不能盲复原。处理原库及其-wal/-shm为同一恢复集合，不能在运行期间删侧文件；恢复文件保持0600、目录0700、所有者正确，重新开库前检查版本与完整性。
- 无损于既有v8业务表的替代方案B（仅用户批准的人工降级）：在停服务且备份完整v9后，一个受控事务内删balance_policy、重建schema_v8的catalog_meta并保留instance_id、置user_version=8，再用旧版verify_schema/core完整校验。必须删balance_policy，否则再次升级建表冲突；此方案只丢本批余额设置、不回退其它业务表，不作为本批自动功能。fermat恢复或降级均须用户明确同意。
- 回滚目录不撤销已经成功的原生add_quota，不自动subtract或重发；由new-api审计核对。部署后核对日志、原生审计、余额栏/chip、balance快照及普通转发，金额都以原始单位核验。
- CI groups-runtime运行的全新临时rc.20中，初始化器仍给issuer1e12；本批启动首轮会自动补到1e15，旧管理员按读数同理。现有真实组合探针扩充断言：balance根/版本/默认策略正确、首次补足结果和账户读数达到预期（无并发消费时）、两种协议与后续原流程仍过。这里只碰测试实例，和正式部署许可分开；T-B13语义专用探针仍手动运行、不入CI。

## 9. 完整性自检

**Step0与实施对照（IR-m3）**：`docs/audits/sgm-8-5-balance/expectations.md` 十节预期已在实施前独立核对，现回填实际位置。机械项覆盖schema字段/enum/字符串ID、B步骤注释、共享常量/生成字段、旧GLM/火山策略零改动和精确清单。B3/B7规定的PBT均已写入并在对应门禁运行，精确名称/128例生成域/独立参考见§10.12；这不构成无穷全域证明。F1修复与代码独审已通过，最后UI及固定SHA退出条件仍以§10.11为准。

以下为v3作者文本核对，独立审查结论另记；不代表实现/测试通过：

- [x] IR-M1：潜在add前装入持owner锁的Drop保护，结算先于解锁。
- [x] IR-M2：预算、认证锁、登录预算共享绝对截止；不取消普通已发HTTP。
- [x] IR-M3/M4：精确十进制BigInt及随浏览器推进的余额观察投影，相关反例入测试计划。
- [x] IR-M5：用户同意每笔/补足前计算界限，架构I10已同步，退款不再违反未承诺的硬cap。
- [x] IR-m1–m5：Storage传播、请求sidecar、Step0草稿、主架构共享类型、accepted_at锚点已补。
- [x] 路由预算决策：用户已确认 §5.6 的 60/70/75 配套，架构 §2.9 已同步，新增边界已通过技术复审。
- [x] v3 独立技术复审及 Step0 文本独立核对通过，用户批准 D1 后进入逐模块实现；测试门禁仍待执行。

## 10. 修订记录

v1（2026-11-01）：初稿。

v1 接手整理（同日，仍待用户确认）：集中列明四处偏差、两项细节及 C1 / C2 / C3 停点；按已确认架构 I9 补清失败后旧读数仍新鲜时的未知判定与自动补足预筛，并明确成功补足后回读失败的 `after=null`。只修改细化文本，不代表已完成 Claude 的 C1 设计审查；未写代码、未提交或部署。

v1 用户确认（2026-11-02 用户：「好」）：确认以上四处偏差、两项细节与 I9 补充；进入 C1 等待 Claude 设计审查，尚未进入实现。

### v1 → v2：C1逐条处置（2026-11-02，文本已修，待Claude复审）

审查原文：`/tmp/quota-throttle-sgm8-5-c1-review.md`，结论不通过、无blocker；代码基线仍7e5cf97。本轮仅修改细化文本，未写实现、运行实现门禁、提交、推送或部署。

**编号核对**：用户称major 9条，但报告正文实际列出8个major编号（B-M1–B-M3、F-M1–F-M5）。以下全部逐一处置；预算D1独立列出，不虚构第9个major或宣称未提供的条目已关闭。minor为后端10条、前端8条；nit按原顺序编号N1–N11。

| Major | 处置与复审位置 |
|---|---|
| B-M1 | §6 T-B4固定500+success:false JSON证据；T-B6断言uncertain、金额、10分钟封锁与再次手动零发送，不能只凭同样的504判断变异被杀。 |
| B-M2 | §6扩假后端正常add与入口计数，observed直接读设置和last_refill；⑩用合法amount但before+amount超cap，明确只有not_sent内存记录可变、持久状态与原生quota不变，解决恒真及与记录规则的交叉。 |
| B-M3 | §3、§5.4.3精确使用ResponseReceived且status.is_none_or(<500)，说明None来自2xx/false，新增None明确拒绝反例。 |
| F-M1 | §5.8.2/.4回到架构：非USD/null单位禁用金额输入，设置只改开关、原金额带回；用户校验为ActionError，无新增偏差。 |
| F-M2 | §5.8.2/.3 requestJson给两新族传原body，校验scope/amount及设置/修订号；§6 app成功链与浏览器成功用例覆盖。 |
| F-M3 | §5.8.5增加状态/颜色/完整文案/原因来源表，T-B12逐项断言。 |
| F-M4 | §1/.3/.5.8.4通用REJECTION_TEXT不变但按原动作覆盖not_ready/invalid_input，等待时间与近似最大量有新鲜度/空值分支，本地ActionError也写editor.rejection。 |
| F-M5 | §4.2补完整View/内部RefillReport边界，wire ID均字符串、null不省；§4.3固定defs/枚举/金额及错误码组合，拒绝与未知保留尝试金额；§7维护跨实现wires。 |

| Minor | 处置与复审位置 |
|---|---|
| B-m1 | §5.4.3/.6全链路tokio Instant，移除into_std，暂停时钟用于截止测试。 |
| B-m2 | §3按1、1b、2–4、5分别记录，不把guard/冷却误列为前4步；§6说明拒绝的内存尝试记录不等于持久提交。 |
| B-m3 | §3/.5.4.3/.5.6未知由dispatch映射ExtraCode::ResultUnconfirmed/504，回到架构，不新增偏差。 |
| B-m4 | §5.4.4提交后核store可用性，中毒则publication=u64::MAX；成功/明确回滚不递增，T-B9含去掉封gate变异。 |
| B-m5 | §5.6/T-B10非法scope400 bad_request，数值越界422。 |
| B-m6 | §5.4.3手动等锁timeout_at；未拿到锁的超时不改记录，避免占槽无限等待 / 覆盖在途结果。 |
| B-m7 | §6先列status/manage可执行假后端分支与入口计数，正例证明计数有效，再做零发送断言。 |
| B-m8 | §5.3.2/.3、§5.5补前后置、调用、交付/401限制与循环终止论证，不再标trivial。 |
| B-m9 | §5.6写出原30/40/45限制；用户提出60s后改列60/70/75配套草案、慢上传/冷却与504取舍，作为D1单独待确认，未默认采纳原措辞建议。 |
| B-m10 | §6 T-B14首轮/业务错误/停止/致命故障，§7同步精确runtime名字。 |
| F-m1 | §5.8.5 exhausted>unknown>low>sufficient，过期快照不绿，测试相同。 |
| F-m2 | §5.8.4预填截cap、向下到分、纯数字、null空白；测试负余额/目标cap边界。 |
| F-m3 | §5.8.2/T-B11明确2.01×500000舍入向量与0.01×7.3拒绝向量，永久Math.round变异。 |
| F-m4 | §4.3/.5.8.2补状态关系、null、未来截止、last_refill组合及issuer顺序，balance独立版本戳与draft.entity硬校验。 |
| F-m5 | §5.8.4/.5新分支在readOnly后，按钮禁用、无账号隐藏、完整说明、静态封锁剩余、两个无障碍名；form支持disabled。 |
| F-m6 | §6给出8个新增Node名称、app成功/未知/409、格式向量和client矩阵、4个浏览器用例；§7列永久变异。 |
| F-m7 | §7明确计划68 Node / 108浏览器，维护wires两脚本及覆盖族/反例，fixture使用PANEL_SCHEMA_VERSION；复用现有CSS，无新类。 |
| F-m8 | §6 T-B13固定10项预期与权限拒绝源码依据、SHA/私有临时实例/清理，补root自加/立即读可见/int64越界；手动、不进CI。 |

| Nit（按报告顺序） | 处置 |
|---|---|
| N1 token_client与循环退出 | §5.4.1/.2从不变owners生成，异常仍记failure继续，不用?中止整轮。 |
| N2 回拨/年龄 | §5.4.3 guard加start_s≤now；§4.3负年龄未知、超龄统一严格>180000ms。 |
| N3 锁释放 | §5.4.3明确离开1b释放槽位再取Core，Core/槽位不跨await。 |
| N4 自动策略重查 | §5.4.3第4步读当前策略，profile等待期间关闭开关则Ok(None)，T-B7覆盖。 |
| N5 可见性/Manual None | §4.2签名类型pub(crate)，§5.6 Manual异常None为Internal/500。 |
| N6 bump映射 | §5.1.3保留直接422并明确不是逐项复制save_group_policy_tx，T-B1校验无写。 |
| N7 连接失败 | §3/.5.4.3尊重NotSent，映射not_ready、不封锁。 |
| N8 迟到观测 | §4.3明确接受显示被覆盖至下次成功观测，不引入新序号，不用于实际补足金额依据。 |
| N9 格式/类型/常量 | §5.8.4 formatQuota补unit；§4.2/.5.3统一display_type约束、年龄边界；§5.8.1从schema生成balanceCap。 |
| N10 回滚 | §8补数据丢失、侧文件/权限、原生已加额度不回退，以及需批准的删balance_policy/重建v8元数据降级替代方案。 |
| N11 部署/CI | §8本机与fermat均先确认原生写，CI临时实例首轮补足与balance快照断言，专用探针仍手动。 |

**D1 路由预算（用户决定记录）**：原审查建议只修措辞、保持旧时限。用户先要求解释，随后提出「那不应该加大预算到60s？」；因此不视为同意旧建议。v2明确提出60s业务、70s浏览器、75s该路由连接配套，列全受影响函数与原路由不变测试；具体配套尚待用户确认，架构原文未改。如采纳，须同步§2.9并让Claude复核预算增量的架构第1项。此待定项不影响本轮对其余意见的文本修订，不构成实现许可。

### v2 subagent独立复审（2026-11-02）

用户要求「按workflow用subagent独立审核，不用等Claude」。三路独立审查先读架构再读v2/必要代码；去重后blocker 0、major 5、minor 5，整体不通过。完整证据见[独审报告](../../../audits/sgm-8-5-balance/c1-v2-independent-review.md)，冻结SHA256为87f4fae48338a4dfe480cadd71c70a060a752cfc67f65c6edd2f3db659ac2aec。**下表保留v2审查时的待修状态，后续用户决定及v3处置以下一节为准**，不回写历史报告为通过。

| 新编号 | 状态 / 下一步 |
|---|---|
| IR-M1 | 待修：补足潜在投递阶段的取消结算/封锁须先于同所有者解锁，测试延后实际原生写而非只延迟回包 |
| IR-M2 | 待修：余额调用的预算准入须有可证明的有界/进展规则，区分认证与已发请求，不用外层取消式timeout掩盖 |
| IR-M3 | 待修：用明确十进制整数分子/分母处理半单位舍入，补0.29×50→15等向量 |
| IR-M4 | 待修：余额展示年龄加入observation.age，根快照仍fresh时也能过期降级，相关控件共用投影 |
| IR-M5 | 待用户决定：维持现有add而只约束每笔/加额前计算，或坚持实际余额硬上限并调整远端能力；架构I10未改 |
| IR-m1 | 待修：仅忽略非Storage，补真实refill→round→loop致命错误传播 |
| IR-m2 | 待修：Rust wire样本须带可配对的原请求上下文，禁止从回复反造期望造成恒真 |
| IR-m3 | 待补：C1通过后、实现前独立产出workflow §6 Step0的十节expectations、机械项和PBT不变量 |
| IR-m4 | 待补：主architecture.md共享类型及consumer同步，handoff清单更新到T-B14 |
| IR-m5 | 待补：选定连接截止明确锚定accepted_at，测试头部时间计入75/45s总上限 |

三方确认旧B-M1/B-M2/B-M3已实质修订；字符串ID/null、运行时原body绑定、就地错误、状态排序、迁移回滚未发现新增问题。60/70/75草案在时限链方向上可行，但不能证明任务自动有界，也不保证慢上传下客户端读完504；具体数值仍待确认。

本轮仅记录独立审查结果，没有技术方案修订被冒认成审查通过；未写业务代码、实现测试、提交、推送、部署或读运行数据。当前停C1，先确定IR-M5架构方向，再补细化与独立复审。

### v2 → v3：独审问题修订（2026-11-02，待复审）

用户理解I10后答复「ok 没问题」，确认只保证每笔/补足前计算及进程内串行封锁；已同步清理架构I10、§2.9和假设，移除退款后实际余额硬cap及“两笔仍保证JSON安全”的推论。原v2独审报告和冻结副本保持，不回写成通过。

| 项目 | v3文本处置 |
|---|---|
| IR-M1 | §4.2/5.4.3 RefillAttemptGuard在潜在投递前持有owner锁，正常/取消均先结算后解锁；故障位失败关闭，T-B16实际写延后且已有手动job等待的反例 |
| IR-M2 | §5.3.4有界send/准入/refresh等待，初始认证和401沿同截止、已发HTTP不被普通deadline取消；20s读取/60s自动attempt、T-B15持续Control/续冷却/登录阶段测试 |
| IR-M3 | §5.8.2按规范十进制p/q+整数分BigInt半上舍入；预填/格式化也避免浮点尾差；原Math.round变异改为截断/浮点回退，补0.29/0.57及cap边界 |
| IR-M4 | §5.8.2/.4/.5共用balanceObservations有效年龄、只降级不升级，stamp含投影类别；同根fresh的180秒边界纳入Node和浏览器 |
| IR-M5 | 按本次用户确认收窄架构I10；T-B17允许退款反例的实际C+50，但每笔及before+amount依然约束，越JSON_SAFE未知/不自动补 |
| IR-m1 | §5.4.2不吞Storage，T-B14要求真实refill→round→loop传播 |
| IR-m2 | §7成功Rust样本请求/回复同pair序号、子目录sidecar明确配对原请求；错误上下文反例，不从回包反造 |
| IR-m3 | 本节前Step0安排及十节expectations草稿，机械项和至少128例的生成式不变量；尚未实施 |
| IR-m4 | 主architecture已加共享类型/consumer设计段，§7及handoff同步T-B1–T-B17 |
| IR-m5 | §5.6明确accepted_at+选定时限，不从解析完成重计；测试慢头/精确路由/权限选择 |

本轮只修文档，不新增持久补足表、不改Native接口或供应商上限；D1具体60/70/75配套仍待确认。复审必须同时核上位I10增量、截止/取消与前端推论，不能因条目已写就判通过。C1通过且Step0核对后才可实现；C2/C3停点保留，仍PR #5。

### v3最终独审结论（2026-11-02）

后端/前端/架构三路独立技术复审收敛为blocker 0、major 0、minor 0；Step0文本层补项也独立核对通过，完整证据与限制见[最终报告](../../../audits/sgm-8-5-balance/c1-v3-independent-review.md)。最终技术输入SHA9626ec15af838d1fc8b7604d09efc6381ba197047a890384665a3a07c8bb2614，冻结副本路径在报告；此后仅附审查状态记录。

小修记录：BR3-M1发现Drop无Core写MAX会与mutate竞态，已改为只关闭balance.failed，真实catalog中毒仍持Core关闭全局；T-B16含真实发布交错，后端定向复核关闭。F3-m1发现13位金额域不足，扩到350整数位+2小数、353字符，不缩单位域，前端定向复核关闭。Step0再补字段/类型/来源和enum/常量归属表、余额局部故障与全局gate区分，架构审查者确认文本层关闭。

本次用户已同意I10保证范围修订；D1具体60/70/75配套仍待确认，架构预算原文未改，条件技术通过不代替批准。没有实现/schema/门禁执行结果，没有代码提交或部署；C2/C3停点保持，PR #5范围不变。

### 10.5 D1 批准与实施入口（2026-11-02）

用户对“补足业务 60s、客户端 70s、该路由连接 75s，其它路由不变”答复“好”，批准具体配套。架构 §2.9 已同步；上文 D1 待确认与未实现描述为各次审查时的历史记录。C1 技术独审与用户确认均已完成，按 B1–B8 顺序逐模块实现、测试和独审；按 workflow 禁止跨模块大提交，本地模块提交后保留 C2 停点。尚无实现门禁通过或部署结论。

### 10.6 B1 实施与模块审查（2026-11-02）

已实现余额策略持久化、schema v9 与 v8 备份迁移；按 T-B1–B3 扩充真实提交失败、损坏旧库及独立状态 PBT。两路独立模块审查均剩余问题 0；全量 Rust 787/0、Node 60/0、浏览器复跑 92/0，4 项独立性能门禁通过，三项变异均编译后语义失败且恢复复验 10/0。完整故障、门禁、变异、偏差证据见 `docs/audits/sgm-8-5-balance/b1-implementation-review.md`。

B1 本地提交后停 C2；上述是工作树模块审查，不代替该 SHA 的提交后独审。B2–B8 尚未实现，不推送或部署，PR #5 范围不变。

### 10.7 B1 提交后 C2 独立审核（2026-11-02）

按用户要求，以三个无会话历史的 subagent 对固定提交 `94362c46b1f7eeb9951f0562f1cbcbc9b295eb34` 分别执行契约重新提取/diff 对照、正确性与测试审核、机械检查及独立门禁。B1 局部 C2 通过，blocker/major/minor 均 0，无代码修复或接受偏离。Rust 787/0（35 既有 ignored）、Node 60/0、浏览器 full 一次 92/0；189 个输入文件前后哈希完全一致。报告与处置见 `docs/audits/sgm-8-5-balance/{audit-report,decisions}.md`。

下一实施模块为 B2；本轮仅审核和同步文档，不宣称 B2–B8 已实现或整个子计划已满足 §6.6 退出，不推送或部署。

### 10.8 B2 实施与模块审核（2026-11-02）

按 §5.3 完成有截止的额度/资料读取、原子 add 调用，以及队列、refresh 锁、登录准入的统一绝对截止；旧 None 路径与已准入 HTTP/交付证据不变。同步更正 CLAUDE.md 的旧 API 结论。两路模块独审剩余问题0；T-B4/T-B5/T-B15细分14测试，含128例PBT。全量Rust801/0（35既有ignored）、Node60/0、浏览器full92/0；五项独立变异均编译后语义失败、恢复14/0。首轮测试期望错误及覆盖修正全部保留于 `docs/audits/sgm-8-5-balance/b2-implementation-review.md`。

B2 随本次本地提交交 C2，不能把模块审查等同该 SHA 的提交后独审。B3–B8 未实现；不推送或部署，运行服务未改动。

### 10.9 B2 提交后 C2 / PBT 专项（2026-11-02）

固定提交 `961972902d929f004dda46f6be476b79326ffed9` 的独立审核暂不通过：当前产品实现未发现确认缺陷，但1 major/2 minor测试证据缺口未闭环。独立副本确认四项错误变异（profile丢deadline、Pending延长截止、`>=`改`>`、amount窄化i32）均编译成功且原14项仍14/0；基线/恢复也14/0，179原始文件哈希一致。完整矩阵和处置见 `docs/audits/sgm-8-5-balance/b2-c2-audit-report.md`、`b2-c2-pbt-review.md`、`decisions.md`。

要求补最小入口/时刻/等号/大金额断言并杀死新增逃逸后复审，不强推大型状态机。现有128例PBT只覆盖固定双槽阻塞时的未选中waiter，不宣称B2或整个第5批的PBT完整。B3/B7尚待实施的四组性质不计为B2缺陷。本轮仅审核，未改被审源码，不进入B3、不推送或部署。

### 10.10 B2 C2意见修正（2026-11-02）

用户已要求修正。F1–F3以公开入口/精确原end/空闲等号/大整数JSON补齐，16项测试通过；原四逃逸及Observe/Control独立漏传、ID窄化共七项变异均编译后被捕获，恢复16/0。原128例限定PBT保持，不称全域或整批性质已完成。

V1确定复现为手势中途重绘替换按钮导致click未到达；单按钮引用保护相关DOM，现有pump让周期work让位，最新授权守卫、state/expiry和在途work规则保留。独立专项复审通过，永久回归杀死DOM/yield回退。新完整清单24×4=96，最终Rust默认并行803/0（35既有ignored）、Node60/0、check:ui/SGM7通过、浏览器full一次96/96。修正期间Node fixture接口与CDP清理失败均保留，不放宽原断言/时限。

方案 `docs/fixes/sgm8-b2-fix-verification-gaps.md`、执行及复审证据 `docs/audits/sgm-8-5-balance/b2-c2-fix-review.md` 随本次本地提交保存。提交后停C2固定SHA复核，B3–B8未开始，不推送或部署。旧C2报告作为历史保留。

### 10.11 B3–B8连续实施与当前最终验证（2026-11-03）

最新用户授权覆盖旧逐模块停点：B3–B8一次性连续完成，模块间不再等待；最终统一交付和部署前确认保留。实现位置如下，历史B1/B2失败/修正记录不删除：

| 需求 | 已完成位置与验证 |
|---|---|
| B3 / T-B6–9、16、17 | `reconcile/worker/balance.rs`及构造/owner接线；`token_service_tests/balance.rs`10项真实Store/传输集成；`balance/tests.rs`4项取消/私有故障定例与3项128例PBT。T-B16已串真实owned_job/watch stop、HTTP bridge中的同owner手动等待者和实际延后Native写；G1/G2经独立静态与执行复核关闭 |
| B4 / T-B14 | `runtime.rs`第六受管余额任务，立即首轮、轮末等60s；两项runtime测试含真实deferred-FK store不可用→refill→round→loop→监督器Storage链。3项错误变异独立编译后语义捕获、恢复2/0 |
| B5 / T-B10、T1⑨⑩ | HTTP两路由、60/70/75绝对截止接线，旧45s/16连接/4业务槽保留；真实慢头4s+body9s在accepted+70s得到504/零发送。旧45s外层回退与调用处重置锚点两项变异均基线1/0→语义0/1→恢复1/0；T1直接读策略/槽位记录并精确断言允许的not_sent差异 |
| B6 | `worker/panel.rs`快照v2、三份schema、生成字段/枚举与capacity；Rust成功wire同pid/pair原请求sidecar，mjs/py按原请求核scope/amount/设置/版本；Python单位尾换行漏检已补约束和负例 |
| B7 / T-B11、12 | `ui/{core,client,app,render}.mjs`；8个指定Node名称集中`tests/ui/balance.test.mjs`，4个指定浏览器名称×4。精确十进制、年龄只降级、请求绑定、静态封锁余量及F1未编辑raw金额保真均已实施；F1真实open→submit回归、独立56例及新变异已关闭，修后最后full112/112通过 |
| B8 / T-B13 | `newapi_balance_probe.py`严格10项已真实10/10；`groups_runtime_probe.py`16项含首轮自动补足与原生回读；原双协议及GLM/火山策略流程通过。固定binary、新tmp/随机端口/假账号、清理与脱敏阶段诊断已验证 |

已取得门禁：Rust默认并发831/0（35既有ignored），F1修后Node68/0及check:ui；旧full浏览器112/112、UI43正例/25族/76余额反例、JS核2050实际Rust回复/15族、Python核3386报文/20请求族/15回复族；Native专用10/10，真实组合16/16+4/4无API mocks浏览器，测试进程均清理。B3关键8/8、B4关键3/3、HTTP两项反向验证闭环；F1修后余额变异子集**8/8**及全部JS最后整轮**25/25**已实际通过，日志`/tmp/sgm8-balance-js-mutations-unrestricted.log`，原沙箱子进程无输出失败保留。

**输入时点与未完成项**：Rust主full编译早于`balance/tests.rs`先join再断言publication及`runtime/tests.rs`有界接收/先释放Condvar再断言两项测试harness修正，生产Rust未改且独立恢复已过；最终固定SHA完整Rust/C2汇总仍待安排。旧full112/wires/组合早于F1，其历史时点保留；F1修后最后full112/112已有新日志`/tmp/sgm8-balance-browser-final.log`及`/tmp/quota-throttle-sgm6-b-browser-ZPlYy1/report.json`。F1真实open→submit永久回归、最后Node68/0/check:ui、8余额/25全部JS变异及原审查者独立56/56复核均已通过；[八节修正报告](../../../fixes/sgm8-balance-policy-raw-amount.md)与[最终代码独审](../../../audits/sgm-8-5-balance/final-contract-review.md)已归档，未关闭生产问题0。本机与远端stable仍7e5cf97，未部署本轮工作树。

原始日志、各失败原因、模块独审、变异输入/恢复哈希与资源清理见[实施汇总](../../../audits/sgm-8-5-balance/b3-b8-implementation-review.md)和[处置记录](../../../audits/sgm-8-5-balance/decisions.md)。

### 10.12 最终Named PBT覆盖矩阵

每组128例；名称与生成域取已实现测试，参考模型独立于生产函数。下列“最终矩阵”指本批已声明性质的位置/范围，不表示pending退出项已完成。

| 模块/精确测试名 | 实现位置 | 生成范围与保证边界 |
|---|---|---|
| B1 `balance_pb_policy_trace_matches_independent_revision_model` | `catalog/balance_tests.rs` | 1..23步，小金额0..11/修订模式/重开；独立策略版本模型。是窄状态轨迹，迁移/真实失败另靠定例 |
| B2 `balance_deadline_pbt_unselected_waiter_never_leaks_or_calls_guard` | `newapi/transport/balance_tests.rs` | class三种×1..499ms，固定两槽占满、waiter未被选；只证资源归还/截止/guard未调用，入口接线等由补测覆盖 |
| B3 `balance_projection_pbt_age_failure_and_signed_values` | `worker/balance/tests.rs` | quota±JSON_SAFE、age=-10..180010、failure/missing；独立分类模型核未知/历史读数/ID |
| B3 `refill_bounds_pbt_actual_http_matches_wide_integer_reference` | 同上 | before±JSON_SAFE、exact=-1..cap+1、target2..cap、refund0..999、auto/manual；独立i128 oracle与真实HTTP/add计数，只约束单笔及before+amount，允许退款后实际越cap |
| B3 `cancelled_attempt_pbt_seals_one_owner_before_release` | 同上 | 双owner/source/合法金额/五种取消或交付终态/后续auto-manual；独立封锁模型后调用真实refill验证同owner与另一owner准入。不是任意长度并发trace穷举；晚写/回读取消另有T-B16确定性集成 |
| B7 `quotaFromDollars rounds and bounds` 内fast-check | `tests/ui/balance.test.mjs` | cents1..1e8、tenths1..1e6；独立整数商余数半上舍入。极小比例、科学记数/350位域/cap等由固定向量补足 |
| B7 `balance state and replies validate` 内fast-check | 同上 | baseAge0..190000、至多30个0..3000ms增量、failure；独立累计年龄的unknown单向模型，根fresh边界另有定例/浏览器 |

## 11. C1 独立审查及后续复审材料

本节原C1输入基线/待定描述为当时审查材料，设计历史保留；当前实现/终审状态见§10.11，不再以“B3–B8未开始”作为下一步。

- 工作区：`/home/haokun/projects/quota-throttle/.worktrees/source-groups-research`；分支 `research/source-groups-token-management`，后续仍归入 PR #5。
- 代码基线：`7e5cf97`。本稿尚未提交，请读取工作区文件，不用 `git show` 代替；本次只记录确认及准备材料，没有代码改动或新提交。
- 输入：`docs/workflow.md` §4.3.1–4.3.3、§4.4；`sgm-8-cleanup.md` v6 §2.9、I5 / I8 / I9 / I10、§5；本稿全部；必要时只读核对 E1–E12 引用的代码。
- 审查范围：v3按workflow §4.4逐项独立判断，不采信作者§10为通过证据；原v2已不通过，核所有IR处置与相关推论。用户已确认I10收窄，须复核第1项；D1仍单列用户确认，不代批准。
- 重点：补足 1 → 1b → 7 的退出与副作用、所有者互斥 / Core / 槽位锁序、截止后的零发送、投递证据与 ≥500 不确定分类、10 分钟封锁、失败立即未知及回读失败不改补足结论、Busy 与 I8、v9 迁移 / 回滚、前端换算及契约清单、T-B1–T-B13 与变异覆盖。
- 输出：逐项通过/不通过，问题级别、file:line依据、最小方案；由主代理记录与处置，独立审查者不修改业务代码。
- 当前阶段：**B1–B8已实施，F1修复、最终代码契约独审与最后UI门禁通过；最终固定SHA完整Rust/C2汇总pending**。取消模块间等待，连续完成已授权工作；不把工作树代码独审等同固定SHA C2，不迁移运行库或部署。
