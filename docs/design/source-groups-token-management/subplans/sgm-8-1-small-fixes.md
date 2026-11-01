# 细化设计 — SGM-8 / 第 1 步：提交被拒就地反馈、轮询 404 提示、维护警告带调用方

> 状态：**细化稿 v2 已确认**（2026-11-01 用户：「好」，与架构 v5 一并确认；§4.4 复审通过）。v1 经 workflow §4.4 独立审查（第 2、3、6 项不通过），修订记录见 §9。
> 确认时一并同意：在本步顺手修 pb2 的弱断言（§6 T10）。
> 架构依据：`sgm-8-cleanup.md` v3（2026-10-28 用户确认）§2.5、§1 第 1 批、§6 第 1 步；本稿对 §2.5 的细化与偏差见 §1 末尾，架构文档已同步（v5）。

## 1. 范围

三项互不依赖，一次提交：

| 编号 | 内容 | 模块 |
|---|---|---|
| F1 | 提交被拒（后端明确拒绝、未记录）→ 确认框保留并就地显示中文原因，不进操作追踪 | `panel_http/ui/{client,core,app}.mjs` |
| F2 | 轮询操作得到 404 → 提示改为「后端暂未找到这条操作记录…」，**照常在 60 秒窗口内继续查询** | `panel_http/ui/app.mjs` |
| F3 | `来源分组维护暂未完成` 警告带上是哪个后台任务（GLM 策略 / 后端检查 / 火山策略） | `runtime.rs` |

**事故复现（2026-10-22）**：删除从未创建成功的 Key（identity #1）→ 后端在事务内以 `not_ready` 拒绝（`catalog/tokens.rs:615-618`，`native_id` 为空）→
面板已在发送前关掉确认框，并把操作记成「结果待核对」→ 轮询 `GET /operations/{id}` 一直得 404 →
显示「请求失败：not_found」，直到 60 秒追踪窗口结束。用户看到的是「点了删除却没反应」。

**不做**：不改后端 HTTP 协议、错误码与状态码；不改 retry / 同 ID 再次提交的路径；不改 60 秒追踪窗口；
不修 `QueueFull` 的根因（第 2 批：后端检查不持锁）。本机 10-27 至 10-28 有约 1034 条该警告（journal 计数，未区分来源），F3 只为给第 2 批留按任务区分的基线。

**对架构 §2.5 的细化与偏差（v2 明示）**：
- §2.5 写「4xx / 503 被拒」；本稿的规则 R 是「收到后端明确错误响应且状态码不是 500、504」，因此 502 等也算被拒——依据是 §2.1 的 I8，并非放宽。
- §2.5 写「202 才关闭确认框」；本稿在**结果不确定**时也关闭确认框（沿用原行为，条目记「结果待核对」），只有明确被拒时保留。
- §2.5 写「轮询 404 → 停止自动轮询」；审查指出这会把晚到落库的提交误报为不存在（§2.1 H9），改为只换提示、不提前停止。
  架构文档 §2.5 已按以上三点同步修订（v5）。

## 2. 关键事实、判定规则与复用点

### 2.1 关键事实（均已查代码）

- **H1 操作类提交在事务提交之前出错就不留记录**：来源控制命令 / 发现模型（`catalog/control.rs:419-561`）与 Key 创建 / 启停 / 删除（`catalog/tokens.rs:561-644`）
  都在同一个 `Immediate` 事务里完成校验与写入。`INSERT INTO control_operations` 之后仍可能出错——成员循环里的 `bump()?`（`control.rs:512`）、
  `SetGroupPriorities` 的检查（`:531`，422 `invalid_input`）、`tokens.rs:639-641` 的后续插入与投影——但这些都发生在提交之前，出错即回滚。
  提交本身失败时：操作类的 `commit(...)?`（`control.rs:441`、`tokens.rs:642`）经 `From<CatalogError>` 一律转成 `storage`（`control.rs:53-57`），返回 **500**；
  目录类写的 busy 已由 RAII 回滚（`store.rs:738-751`），映射为 503 `queue_full`（`admin.rs:131`）。
  **提交成功之后到响应之间，只剩两种 500**：响应编码失败（`response_too_large`，`panel_http/admin/output.rs:53-58`）与任务回复通道被丢弃（任务 panic 或中途退出，`internal`，`bridge.rs:112`）。
- **H2 事务外的拒绝都在调用 store 之前**：桥接满或正在关闭（`admin/bridge.rs:44-104`，503 `queue_full`）、演练模式（403 `dry_run`，`:79-81`）、
  命令槽被占（503 `queue_full`，`reconcile/worker.rs:308-317`、`token_service/execution.rs:106-109`）、发现模型先查重放、网络失败在落库前返回 502
  （`worker/management.rs:14-41`）、Key 操作的 `AuthFailed` 在落库前返回（`token_service.rs:166-170`）、HTTP 层解析错误（400 / 411 / 413 / 415 等，`panel_http.rs:40-56`）。
- **H3 只有三类结果不能证明「未记录」**：
  (a) 桥接等待超时 504 `result_unconfirmed`，以及映射为 504 的 `transport_unknown`——业务任务可能仍在执行（`bridge.rs:105-114`、`:151-179`，进入后不中止）；
  (b) 500（`storage` / `internal` / `response_too_large` / 提交状态未知）；
  (c) 没收到合法错误响应——连接失败、超时、取消（`ClientError.status === null`，`client.mjs:9,133-135`）或协议错误（`ProtocolError`）。
- **H4 retry 例外**：`retry` 先提交、再做网络认证（`worker.rs:355-379`），其 502 / 504 可能发生在提交之后。本次不碰 retry 路径。
- **H5 目录类写**（建来源 / 建组 / 计划成员 / 登记已有 Key）的本地写入都在一次 `mutate` 内完成（`worker.rs:318-340`、`tokens.rs:495-551`）。
  **核验组**把网络错误折叠进 200 的核验报告（`token_service.rs:350-370`），从不返回 502；但注册已在 new-api 完成后，若期间发布代际变化，
  `mutate(Some(epoch))` 会返回 409 `revision_conflict`（`worker.rs:261-262`），此时本地未写入、远端注册已做。核验组可重复执行，重新提交安全。
  登记已有 Key 有同样的 409 路径（`access.rs:54`），本地同样未写入。
- **H6 请求期间不会重复提交**：写请求占用 work 通道期间 `readOnly` 为真（`app.mjs:126`），`submit` 第一行即返回（`app.mjs:353`），`paint` 也把提交按钮置灰（`app.mjs:183`）。
  请求期间唯一能替换确认框的是「恢复操作查询」等用户主动打开的新表单。
- **H7 写请求的回调**：`work` 对写请求只要还持有票据，就会调用 `success`；`success` 抛错（例如 `accept` 校验回执失败，`core.mjs:486-491`）或请求失败时调用 `failure`
  （`app.mjs:226-233`；写票据不受通道作废影响，`core.mjs:280`）。因此可能出现「先进入 success、再进入 failure」：后者按 R 为假处理（错误不是带状态码的 `ClientError`），安全。
  页面销毁时 `lanes.destroy`（`core.mjs:282`）中止写请求但不清空占用者，failure 仍会以 `ClientError('cancelled')`（无状态码，R 为假）被调用。
  写请求只有在**发送之前**（页面不活动 `app.mjs:215`，或取票失败 `:221`）才一个回调都不调用。
- **H8 客户端写截止 40 秒（`core.mjs:274`）**：服务端业务截止为 `min(进入时刻 + 预算, 受理时刻 + 40s)`（`bridge.rs:82`、`panel_http.rs:25-26,295`）。
  通常服务端先回 504；若请求体上传很慢，客户端可能先到期，此时 `status === null`，按 R 为假处理，仍然安全。**H8 不承担安全论证。**
- **H9 404 不能证明「永远没有」**：504 之后任务不中止，可能仍在等 `core` 互斥锁（`worker.rs:251-254`）；查询 `operation()` 抢同一把锁（`worker.rs:301-306`），
  可能先拿到锁得 404，随后提交才落库。F1 之后被拒的提交不再进入轮询，**仍会轮询的只有结果不确定的条目与手动恢复的 ID**，正是这种情况的主体。

### 2.2 判定规则 R 与它依赖的不变量

**判定规则 R**：`rejected(error) ⇔ error 是 ClientError ∧ error.status 是整数 ∧ status ∉ {500, 504}`。

**适用范围**：只用于 `submit()` 的**首次提交**，即 `pending.begin` 刚生成的新操作 ID（或不带 ID 的目录类写）。同 ID 再次提交、retry 不走 R。
新 ID 上不会出现 `idempotency_conflict`，除非随机 ID 碰撞；碰撞时后端那条记录属于别的操作，丢弃本地条目也正确。

**依赖的不变量**：`sgm-8-cleanup.md` §2.6 **I8「首次提交被拒不留记录」**——写请求以新操作 ID 首次提交，若返回 500 / 504 以外的错误，
则后端没有提交任何记录（操作记录与目录行）；核验组例外，只是远端注册可能已做，而它幂等。
由 H1、H2、H5，当前代码满足 I8；T1 按提交族逐类钉住；后续各步必须保持（架构文档已写明第 3 步「建组即注册」的约束）。

R 为真 ⇒ 后端没有记录本次提交，可以保留确认框让用户修改后再提交；再次提交时 `pending.begin` 生成新 ID，不会与已存在的后端记录重复执行。

### 2.3 与已有代码的复用点

| 复用项 | 位置 | 用法 |
|---|---|---|
| `ClientError`（`code` / `status` / `existingSourceId`） | `client.mjs:9,117-120` | R 的输入；不改其构造 |
| `PendingOperations.begin` / `failed` / `accept` / `continue` / `due` / `persist` | `core.mjs:456-510` | 原样复用；只新增 `discard` |
| `PanelApp.work` | `app.mjs:214-237` | 原样复用；回调语义见 H7 |
| `renderer.clearForm` / `renderer.conflict` | `render.mjs:474-502` | 关闭确认框、在确认框内显示一行提示 |
| `PanelApp.message` | `app.mjs:166` | 其他失败的通用文案 |
| `ENUMS.adminError` / `ENUMS.httpError` | `contract.mjs` | T7 校验文案键 |
| `tokens_fixture` / `http()` / `expect()` 夹具 | `panel_http/admin/tests/{tokens,http}.rs` | T1 |
| `Object.create(PanelApp.prototype)` 写法 | `tests/ui/actions.test.mjs` | T3–T6 |

## 3. 错误处理策略

| 结果 | 操作类写（202 回执） | 目录类写（201 / 200） |
|---|---|---|
| 成功 | 关闭确认框（若仍是本次的）、展开操作追踪、原提示 | 关闭确认框、原提示 |
| R 为真 | 丢弃追踪条目；确认框保留并显示原因 | 确认框保留并显示原因 |
| 其他（不确定，含 success 回调内抛错） | **原行为**：条目记「结果待核对」、关闭确认框、展开追踪 | **原行为**：关闭确认框，提示「回执丢失不证明未写入」 |

「确认框仍是本次的」= `this.editor === 提交时捕获的 editor`。用户在请求期间关闭了确认框、或打开了别的表单时，
结果改为写到顶部提示，**绝不清除或改写别人的表单**。

**敏感表单**（添加来源，含凭据）：原先发送前就清空，现在请求期间（最长 40 秒）与被拒之后都保留在 DOM 中。既有的「隐藏页面 / 切组 / 切页即清」规则不变（editor 仍带 `sensitive: true`，`suspend` 照样清除），由 T12 钉住。`duplicate_source` 时重新提交没有意义，但表单照样保留，用户可直接关闭。（代码审核 minor-1）

原因文案统一为 `提交未完成：<中文原因>（<错误码>）`。不写「未执行」，因为核验组在 409 时远端注册可能已经做了（H5）。

## 4. 数据结构

- 确认框对象 `editor` 新增可选字段 `rejection: string | null`（模块私有）：本确认框上一次提交被拒的原因；下一次提交时清空。
- `PendingOperations` 不加字段，新增方法 `discard`（§5.2）。
- `REJECTION_TEXT`：错误码 → 中文原因，**从 `app.mjs` 导出**（T7 需要导入）；所有键必须属于 `ENUMS.adminError ∪ ENUMS.httpError`（T7 校验）。

初版文案：

| 错误码 | 原因 |
|---|---|
| `not_ready` | 对象当前状态不允许此操作（例如尚未创建完成、另有操作正在进行，或组尚未就绪） |
| `queue_full`、`overloaded` | 后端正忙或已达容量上限，请稍后再试 |
| `revision_conflict` | 对象或后端状态已变化，请刷新后重新打开再确认（Key 操作需先刷新本地身份页） |
| `not_found` | 对象不存在或已被移除 |
| `invalid_input`、`bad_request` | 输入不合法，请检查后再提交 |
| `dry_run` | 当前为演练模式，不会写入 |
| `idempotency_conflict` | 操作 ID 冲突，请重新提交 |
| `binding_drift` | 签发账号与已登记的身份不一致 |
| `auth_failed` | new-api 账号认证失败 |
| `remote_rejected` | new-api 拒绝了请求 |
| `invalid_response` | new-api 或上游返回异常 |
| `rate_limited` | new-api 管理接口限流，请稍后再试 |
| `duplicate_group` | 已有使用相同 NewAPI 分组标识的组（`catalog/store.rs:990-995`） |
| `duplicate_membership` | 该来源已在此组中 |
| `type_mismatch` | 来源与组的类型不一致 |
| `retired` | 对象已归档 |
| `channel_conflict` | 渠道绑定冲突 |
| `forbidden` | 请求被拒绝（请求来源校验未通过） |
| 其他 | 后端拒绝了请求 |

`revision_conflict` 的文案特意不写「对象已有新版本」：H5 的 409 来自发布代际变化，对象版本可能没变；
身份类确认框的冲突检测用的是不随快照刷新的本地身份页（`app.mjs:181-182`），不提示「先刷新身份页」用户会陷入反复 409。

`duplicate_source` 单独处理：`该凭据已对应来源 #<id>，没有覆盖配置`（沿用 `existingSourceId`）。

## 5. 模块细化

### 5.1 `client.mjs`

#### 5.1.1 `export function rejected(error) -> boolean`（新）

- **功能**：判定规则 R。
- **实现**：`const UNCERTAIN_STATUS = new Set([500, 504]);`
  返回 `error instanceof ClientError && Number.isInteger(error.status) && !UNCERTAIN_STATUS.has(error.status)`。
- **分支**：不是 `ClientError`（`ProtocolError` / `ActionError` / 回执校验错误 / 其他）→ false；`status === null`（transport / deadline / cancelled）→ false；500 / 504 → false；其余 → true。
- **依据**：`ClientError` 只有在收到 `!response.ok` 且错误体通过 `validateError` 时才带状态码（`client.mjs:117-120`），所以带状态码一定是后端的明确响应。
- **放在 client.mjs 的理由**：`ClientError` 定义在这里；`core.mjs` 被 `client.mjs` 引用，反向引用会成环。
- **正确性论证**：trivial（纯判定、无副作用）。语义正确性依赖 I8（§2.2），由 T1 钉住后端一侧。

### 5.2 `core.mjs` · `PendingOperations`

#### 5.2.1 `discard(id)`（新）

- **功能**：移除一个已确认后端没有记录的条目。与 `dismiss` 的区别：存储写失败时仍然移除内存条目（`dismiss` 会抛错并保留，`core.mjs:511-517`）。
- **实现**：
  1. `items` 里没有 `id` → 返回。
  2. `ids =` 其余 ID（保持原顺序）。
  3. `items.delete(id)`。
  4. 若 `storageError === null`：`try { persist(ids) } catch {}`。`persist` 失败时会自己置位 `storageError`（`core.mjs:459`）。
- **退出**：只有正常返回，从不抛错。
- **副作用**：删除内存条目；sessionStorage 最多写一次；写失败时 `storageError` 被置位，之后 `begin` 拒绝发起新的持久操作（既有规则，`core.mjs:464`）。
- **正确性论证**：
  - 前置：调用方已用 R 确认后端没有这个 ID 的记录（I8）。
  - 删除内存条目不会丢失可恢复的信息，因为后端本来就没有这条记录。
  - 若存储里残留了这个 ID，重新加载后它会被恢复并轮询，得到 404 后按 §5.3.6 显示「暂未找到」，60 秒后停止，不会被误报为成功。
  - 后置：内存中没有该条目；存储要么同步删除，要么 `storageError` 已置位。

### 5.3 `app.mjs`

#### 5.3.1 `submit(action, draft, fields)`（修改）

- **调用关系**：
  - 调用方：`confirm` / `openGroup` / `openSource` / `openRegisterToken` 等表单的 `onSubmit`。
  - 被调用：`buildAction`、`route`、`pending.begin`、`work`、`rejected`、`closeEditor`（新）、`rejectSubmit`（新）。
- **实现**：
  1. `readOnly` 或页面不活动 → 返回（原样，H6）。
  2. `const editor = this.editor`，即本次提交所属的确认框。
  3. 身份版本检查（原样）。失败抛 `ActionError` → `catch` → `notice`，确认框不动。
  4. `buildAction` 与 `route`（原样）。失败 → `catch` → `notice`，确认框不动（同原行为）。
  5. 若 `editor` 存在：`editor.rejection = null`。
  6. **操作类**（`result.kind` 非空）：
     - `item = pending.begin(...)`，原样：发送前先把 ID 持久化，Step U5 不变。`begin` 失败抛错 → `catch` → `notice`，未发送。
     - **不再在发送前关闭确认框。**
     - 调用 `work(descriptor, item.payload, success, failure, 'operation:'+item.id)`：
       - success(value)：`accept`（原样，可能抛错，见 H7）→ `closeEditor(editor)` → `operationsOpen = true` → 原 notice。
       - failure(error)，R 为真：`pending.discard(item.id)` → `rejectSubmit(editor, error)`，不展开追踪。
       - failure(error)，R 为假：`pending.failed(item.id, '结果待核对。…')`（原样）→ `closeEditor(editor)` → `operationsOpen = true`。
  7. **目录类**：调用 `work(descriptor, result.body, success, failure)`：
     - success：`closeEditor(editor)` → 原 notice。
     - failure，R 为真：`rejectSubmit(editor, error)`。
     - failure，R 为假：`closeEditor(editor)` → 原 notice。`duplicate_source` 是 409，必然走 R 分支，所以原 notice 里只保留「回执丢失不证明未写入」那一半。
  8. `work` 没有调用任何回调（只发生在发送之前：页面不活动或取票失败，H7）时：确认框保持打开；若为操作类，条目留在追踪里，
     之后轮询按 §5.3.6 处理（从未发送，60 秒内一直「暂未找到」后停止）。与原行为相比只多了「确认框还开着」，没有其他副作用。
- **退出**：同步返回 `undefined`；异步结果只经由 `work` 的回调送回（H7）。
- **显式假设**：H6（请求期间同一确认框不会重入）；H7（回调语义，含 success 抛错后转 failure）；I8（R 为真即未记录）。
- **正确性论证**：
  - 前置：本次表单值已通过 `buildAction` 校验；操作 ID 是 `begin` 新生成的（R 的适用范围）。
  - (a) R 为真 ⇒ 由 I8，后端没有记录。所以 `discard` 不会丢失可恢复信息；保留确认框后再次提交时，`begin` 会生成新 ID，不会与一条已存在的后端记录重复执行。
  - (b) R 为假 ⇒ 后端可能已经记录。沿用原来的「结果待核对」并关闭确认框；用户只能从追踪面板「同 ID 同载荷再次提交」，后端按幂等重放（`control.rs:433-435`、`tokens.rs:574-576`），不会写两次。
  - (c) success 中 `accept` 抛错 ⇒ `work` 转调 failure，错误不是带状态码的 `ClientError` ⇒ R 为假 ⇒ 走 (b)；此前 success 尚未执行到 `closeEditor`，由 (b) 关闭，最终状态与 (b) 一致。
  - (d) `closeEditor` 与 `rejectSubmit` 只在 `this.editor === editor` 时改动确认框，所以不会清掉用户在请求期间打开的另一个表单。
  - 后置：成功 ⇒ 确认框关闭、条目带回执；R 为真 ⇒ 没有条目、确认框显示原因；其他 ⇒ 条目为「结果待核对」、确认框关闭。
  - 副作用论证：sessionStorage（`begin` 写一次，`discard` 最多再写一次）；内存中的 `pending.items`；一次 HTTP 写；work 通道的占用与释放（`work` 内部）；
    `operationsOpen`；确认框 DOM 与 `notice`。不触碰 secret、usage 与其他通道。

#### 5.3.2 `closeEditor(editor)`（新）

`if (editor && this.editor === editor) { this.renderer.clearForm(); this.editor = null; }`。trivial（一条带身份检查的分支）。

#### 5.3.3 `rejectSubmit(editor, error)`（新）

`text = this.rejectionText(error)`；若 `editor && this.editor === editor`，则 `editor.rejection = text`，否则 `this.notice = text`。
trivial（分支赋值）。重绘由 `work` 的 `finally` 负责（`app.mjs:235`）。

#### 5.3.4 `rejectionText(error)`（新）

`提交未完成：${error.existingSourceId ? `该凭据已对应来源 #${error.existingSourceId}，没有覆盖配置` : Object.hasOwn(REJECTION_TEXT, error.code) ? REJECTION_TEXT[error.code] : '后端拒绝了请求'}（${error.code}）`。用 `Object.hasOwn` 而不是 `??`，避免取到原型上的键。trivial。

#### 5.3.5 `paint()`（修改一行）

确认框提示行由 `conflict ? 原冲突文案 : null` 改为 `conflict ? 原冲突文案 : (this.editor.rejection ?? null)`；禁用条件不变。
冲突优先，因为对象版本变了以后，上一次被拒的原因已经过时。

#### 5.3.6 `pollOperation(id)`（修改失败回调）

- 若 `error instanceof ClientError && error.status === 404`：
  `pending.failed(id, '后端暂未找到这条操作记录。结果未知的提交可能稍后才落库，60 秒内会继续查询；之后可点「继续查询」。')`。
- 否则原样 `pending.failed(id, this.message(error))`。
- **不改追踪状态**：`failed` 只改 `error` 与 `nextPoll`（`core.mjs:498`），条目在 60 秒窗口内照常被 `due()` 返回（`core.mjs:500-510`），窗口结束后停止，用户可点「继续查询」（`core.mjs:499`）。
- **论证**：`GET /operations/{id}` 的 404 只来自 store 的 `NotFound`（`admin/dispatch.rs:87` → `control.rs:385-388`；路由不匹配不会发生，ID 已经过 `operationId` 校验），
  表示后端**此刻**没有这条记录。由 H9，结果未知的提交可能晚于这次查询才落库，所以不提前停止，只把「请求失败：not_found」换成能看懂的说明。
  正确性论证：trivial（一条分支选择文案，不改状态机）。

### 5.4 `runtime.rs`

#### 5.4.1 `fn control<T>(job: &'static str, result: ControlResult<T>) -> RuntimeResult<Option<T>>`（修改）

- **改动**：新增 `job` 参数，警告加字段：`warn!(job, code = ?e.code, "来源分组维护暂未完成")`。其余分支不变：Ok → `Some`；`Storage` → `Err(RuntimeError::Storage)`；其他 → 警告并返回 `Ok(None)`。
- **调用点**（`runtime.rs:72-81` 定义；全文件只有这三处调用，没有其他发出该警告的地方）：
  - `glm_loop` 传 `"glm_policy"`（`runtime.rs:110`）；
  - `maintenance` 传 `"runtime_backend"`（`:136`）与 `"volcano_policy"`（`:152`）。
- **正确性论证**：trivial（只加日志字段）。返回值不变 ⇒ 各循环行为不变。

## 6. 测试

| # | 名称（层） | 内容 | 变异验证 |
|---|---|---|---|
| T1 | `h2_first_submit_rejections_leave_no_record`（Rust HTTP，`panel_http/admin/tests/tokens.rs`；完整路径 `reconcile::worker::tests::token_service_tests::panel_admin::…`，`token_service_tests.rs:16`） | 按提交族逐类钉住 I8，每类都断言：被拒请求的状态码与错误码；操作类 `GET /operations/<该 ID>` 为 404；`worker.snapshot()` 与本地身份列表在请求前后相等（**不用**比较数据库文件字节：catalog 为 WAL 模式，`store.rs:251`，写入可能只在 -wal 文件里）。各类：① 删除从未创建的 Key → 503 `not_ready`（事故复现：核验组 → 提交创建 Key 得 202、不跑 `token_step`）；② 删除 Key 带错误 `expected_revision` → 409；③ 暂停来源带过期版本 → 409 `revision_conflict`；④ 固定一个不可编辑的 GLM 成员（计划中、无渠道）→ `not_ready`；⑤ 建组使用重复的 NewAPI 分组标识 → 409 `duplicate_group`；⑥ 创建 Key 带过期的组版本 → 409 `revision_conflict`；⑦ 用已登记的凭据再建来源 → 409 `duplicate_source`；⑧ 同一来源再次计划进同一组 → 409 `duplicate_membership`（⑥–⑧ 为代码审核 minor-3 补充）。八类都在第一条写语句之前被拒；「INSERT 之后出错也回滚」那一半目前没有 HTTP 路由可达（`SetGroupPriorities`），不在 T1 内 | 契约钉：钉住 I8 的既有行为，不做变异 |
| T2 | `client rejected() treats only definite backend refusals as unrecorded`（node） | null / 500 / 504 / `ProtocolError` → false；403 / 404 / 409 / 422 / 429 / 502 / 503 → true | `UNCERTAIN_STATUS` 去掉 504 → 失败 |
| T3 | `submit rejected keeps the confirm open with the reason and tracks nothing`（node） | 假 renderer + 假 `work` 回调 `ClientError('not_ready', 503)` → 无条目、存储中 `operation_ids` 为空、`editor` 仍在且 `rejection` 含原因、未调用 `clearForm` | 去掉 R 分支 → 失败 |
| T4 | `submit with unknown outcome still records pending check and closes the confirm`（node） | `result_unconfirmed`（504）、`transport`（无状态码）、success 内 `accept` 抛错三种 → 条目保留、`error` 以「结果待核对」开头、调用了 `clearForm`、`editor` 为 null | — |
| T5 | `submit outcome never touches a form opened during the request`（node） | 发送后把 `this.editor` 换成新对象：被拒 → 新对象没有 `rejection`、原因进 `notice`；成功 → 未调用 `clearForm` | `closeEditor` 去掉身份检查 → 失败 |
| T6 | `operation poll 404 explains and keeps polling within the window`（node） | 轮询失败 `ClientError('not_found', 404)` → `error` 含「后端暂未找到这条操作记录」、`tracking` 仍为真、5 秒后 `due()` 返回该条目；60 秒后 `due()` 为 null；`continue` 后再次返回 | 404 分支改为停止轮询（`tracking = false`）→ 失败 |
| T7 | `rejection texts only name contract error codes`（node） | 导入 `REJECTION_TEXT`，键 ⊆ `ENUMS.adminError ∪ ENUMS.httpError` | — |
| T8 | `runtime::tests::control_warning_names_the_maintenance_job`（Rust） | `tracing::subscriber::with_default` 加内存 writer 的 fmt 订阅者；进入后先调用 `tracing::callsite::rebuild_interest_cache()`（消除并行测试先行注册该调用点导致的兴趣缓存竞态），再执行 `control("volcano_policy", Err(QueueFull))`：返回 `Ok(None)`，输出含 `volcano_policy` 与 `QueueFull`；`Storage` 返回 `Err(Storage)` | 警告去掉 `job` → 失败 |
| T9 | `rejected delete keeps confirm open with reason and tracks nothing`（浏览器 × 4） | 拦截 `DELETE /tokens/*` 回 503 `not_ready`：点删除 → 确认提交 → 确认框仍可见、提示含「对象当前状态不允许」、「操作进度（0）」 | — |
| T11 | `pending discard removes the entry even when the storage write fails`（node） | `begin` 之后让 `setItem` 抛错：`discard` 不抛错、条目为空、`storageError` 已置位（与 `dismiss` 的区别；代码审核 minor-2） | `discard` 改为调用 `dismiss` → 失败（已验证，不入永久清单） |
| T12 | `rejected sensitive source form stays only until the page is hidden`（node） | 添加来源被 `duplicate_source` 拒绝后确认框保留并显示「已对应来源 #7」；随后 `suspend` 清除表单 | — |
| T10 | `pb2_admin_dry_run_and_stale_revisions_cannot_write`（既有 Rust 属性测试，`panel_http/admin/tests/http.rs:319`，修改） | 演练分支原先只比较 `catalog.sqlite3` 的字节（`:324,332`），而 catalog 为 WAL 模式，提交可能只写进 `-wal` 文件，断言挡不住写入。改为：请求前后 `worker.snapshot()` 相等，**且**主文件与 `-wal` 文件（不存在记为空）的字节都相等——任何一次提交都会改变两者之一 | 临时在演练分支的请求之后插入一次真实写（例如直接经 worker 建来源）→ 新断言失败、旧断言通过（只做一次，不入永久变异清单） |

**清单同步**：
- `sgm6_b_node_tests.txt` 加 T2–T7、T11、T12（52 → 60，满足 `≥ 47`）；
- `sgm6_b_browser_tests.json` 加 T9（22 × 4 = 88 → 23 × 4 = 92），`tests/ui/reporter.test.mjs:9` 改为 92；
- `sgm7_tests.txt` 加 T8：`sgm7_check.py:40-45` 要求与排序后的测试名逐行一致，T8 按字典序插在 `runtime::tests::r1_…` 之前；
- `sgm6_b_mutations.mjs` 永久加入 T2、T6 的两个变异（「变更前」子串在源文件中唯一）。

**门禁**：
- `cargo test --release` 全量；
- `npm run test:ui`、`npm run check:ui`、`npm run test:browser`（含性能门禁）；
- `python3 scripts/contract_audit/sgm7_check.py`；
- 独立子代理审核。

## 7. 部署与观察

- 本机（3000 / 3001，`quota-throttle-sgm.service`）与 fermat 13001：沿用既有配方（构建 → 替换二进制 → 重启）。
- 部署后在本机复现一次：删除仍处于 never 状态的 identity #1，应看到确认框保留并显示「对象当前状态不允许此操作…（not_ready）」，且不新增追踪条目。
- 按 `job` 统计 `QueueFull` 警告 24 小时，作为第 2 批（后端检查不持锁）的修改前基线。

## 8. 完整性自检

- [x] 推导连续：§5.3.1 逐步列出发送前、成功、R 为真、R 为假、success 抛错、未调用回调六种走向
- [x] 分支覆盖：`rejected` 四个分支；`submit` 操作类与目录类各三个结果；`pollOperation` 两个分支
- [x] 退出覆盖：同步 `return` 与 `catch`；异步只经由回调；`discard` 从不抛错
- [x] 被调用方契约：`begin` / `persist` / `failed` / `due` / `continue` / `accept` / `work` / store 事务均给出行号
- [x] 循环：本次新增代码只有 `discard` 里的一次 `filter`，长度不超过 16（`MAX_PENDING`），必然终止
- [x] 显式假设：H1–H9 均已查代码；R 依赖 I8，由 T1 按提交族钉住；H8 明确不承担安全论证

## 9. 修订记录

**v1 → v2（2026-10-30，workflow §4.4 独立审查：第 2、3、6 项不通过，第 4 项通过，第 5 项通过但需声明偏差；以下均已对照代码核实）**：
1. major：R 依赖跨步骤的后端不变量，只由「删除 Key」一条测试钉住 ⇒ 在架构 §2.6 立为 I8，并写明第 3 步「建组即注册」须保持；T1 扩为按提交族逐类覆盖。
2. minor（实为行为问题）：「轮询 404 即停」会把 504 之后晚到落库的提交误报为不存在（H9）⇒ F2 改为只换提示、窗口内照常查询；T6 与变异随之改。
3. H1 表述不准（插入之后仍有校验；busy 在提交时为 503）⇒ 改写为「提交成功之前出错就回滚，之后只剩编码失败」。
4. H5 把核验组的错误写成 502（实际折叠进 200 报告；真实的提交后错误是 409 `revision_conflict`）⇒ 改正，并改 `revision_conflict` 文案，避免身份页反复 409。
5. 缺模板「复用点」一节 ⇒ 新增 §2.3。
6. `REJECTION_TEXT` 的导出前后矛盾 ⇒ 明确导出。
7. 未声明对架构 §2.5 的偏差 ⇒ §1 末尾列明，架构文档同步。
8. H7 漏了「success 抛错后转 failure」；H8 不承担安全论证；副作用列举不全；`sgm7_tests.txt` 须按排序插入；T1 模块路径；T8 并行下的兴趣缓存竞态 ⇒ 均已补。

**v2 复审（同日，§4.4 第 2–6 项全部通过）后的小修**：H1 中操作类提交失败一律 500（只有目录类 busy 为 503），`bridge.rs:112` 是回复通道被丢弃而非编码失败；
H7 改正：页面销毁时 failure 仍以 `cancelled` 被调用，只有发送前才不调用回调。**确认顺序**：本稿依赖架构 v5 的 I8，需与 v5 一并确认。

**确认（2026-11-01 用户：「好」）**：v2 与架构 v5 一并确认；按建议新增 T10（pb2 的 WAL 弱断言）。

**实现与代码审核（同日，workflow §5.2 独立审核：代码通过，无 blocker / major 代码缺陷）**：
- 审核指出的门禁问题：第一次全量 `cargo test --release` 复用了手工变异时（与工作区共用 `CARGO_TARGET_DIR`）留下的变异二进制，结果作废，已强制重编后重跑；变异运行留下的 `proptest-regressions/panel_http/` 种子文件已删除。
- minor-1 敏感表单在请求期间与被拒后保留 ⇒ §3 补说明，新增 T12；minor-2 `discard` 与 `dismiss` 的区别无测试 ⇒ 新增 T11；minor-3 T1 覆盖比 I8 窄 ⇒ 补 ⑥–⑧ 三类；nit：§5.3.4 同步 `Object.hasOwn`。
- 变异验证：T2、T3、T5（两处身份检查）、T6、T11 在临时副本中均被杀死；T8、T10 在独立编译的副本中被杀死（T10 改用真实写入：SQLite 对 `UPDATE x = x` 不写任何页，最初的变异因此无效）。
