# SGM-2 HTTP 交付证据与 worker 独立审计

日期：2026-10-15。路径：workflow §6.5 B。分支：research/source-groups-token-management；基线：`4c35b786ca76d88a6eac8e1f0c68661042a3ab37`。本文件仅审 HTTP 交付证据与 ReconcileWorker 组装，不替代整个 SGM-2 验收。

最终状态：✅ 本 worker checkpoint 的路径 B 复审通过，W1–W8 及 lookup 刷新延期残留均已关闭；无未解决实现发现。以文末“第三轮与最终 checkpoint 意见”为准，前两轮失败结论保留为修正历史。此结论不表示整个 SGM-2、完整路径 A 或生产发布门禁通过。

## 方法与范围

先读 `subplans/sgm-2-channel-reconcile.md` 全部契约，尤其“HTTP 交付证据”“执行器组装”，再读 architecture 的第二子计划补充与 expectations §1–10；未读 expectations 的 Step 5 主测试记录，未使用主 agent 的测试结论。模板文件不存在，按 §6.5 的同等结构独立抽取期待，再审 `git diff 4c35b78`；三个新增文件用 `git diff --no-index /dev/null FILE` 阅读。既有文件仅看 diff 及必要邻接上下文，未通读未修改实现。

第一轮审阅的 worker 新增文件对象为 `ac98452`，测试为 `aeffdff`，transport 为 `e0323e3`，delivery_tests 为 `ff2b553`。审计期间主 agent 仍在修正实现，以下未关闭项以函数与触发条件定位；行号为本轮记录时位置。

未访问生产配置、数据库、服务或真实 key。独立测试仅使用临时目录、随机回环端口和假凭据。额外只读核对固定上游源码：`GetAllChannels` 明确读取并应用 `group` 查询参数，因此新 group-filtered scan 有实际接口依据。[固定 NewAPI 处理器](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/channel.go#L93)

## 独立期待与对照

| 编号 | 从契约独立抽出的期待 | 对照结果 |
| --- | --- | --- |
| E1 | 每次状态 mutation 先封旧 fence，事务提交/已知回滚后才发布新偶数 epoch；未知提交保持关闭，计数不回绕 | ✅ `mutate` 在 Core 锁内检查 expected epoch，再发布奇数，调用事务后按 Store 可用性发布偶数；耗尽 fail closed |
| E2 | 发送票必须绑定准备事务自己的发布 epoch，不能在解锁后认领另一个命令的 epoch | ✅ `write` 使用 `plan.epoch + 2`，不重新 load 当前 publication 充当准备票；HTTP guard 只读原子和时间 |
| E3 | 网络等待不持 Core/SQLite 锁；单 lane 串行控制工作；命令入口有界，调用者取消不遗失已受理请求 | ✅ 同步 mutation 与网络 await 分离；lane try-lock；命令 64 槽；`reconcile_once` 的独立任务持续结算 |
| E4 | 旧 GET、旧 ACK、旧失败不得改变新发布输入对应的观测、确认或调度状态 | ⚠️ GET 发布与确认使用 expected epoch，旧写仅结算原日志；旧失败延期仍有 W3 |
| E5 | NotSent、可识别应用回执、Uncertain 分开；完整代理错误页不等于应用已结束；写成功仍须回读 | ✅ `DeliveryEvidence` 与 `execute/envelope` 按正文完整性、bool success 和请求语义分类；非 2xx 不可成为成功 |
| E6 | PUT 的前置 GET/解析/patch 失败只能说明 PUT 未发；no-op 不伪造写成功 | ✅ 前置 `get_channel` 错误覆盖为 NotSent；本地解析/patch 错误的默认交付证据为 NotSent；false 结算为未写 |
| E7 | 只有明确未发出的暂时写错误能本轮重试；读 5xx 暂时失败；次数、退避及 Retry-After 有界 | ⚠️ 写重试明确限制 NotSent、三次；长 Retry-After 交持久延期。读 5xx 尚有 W5 |
| E8 | 停止/降低优先，完整组屏障先降后升；源级 gate 影响全部组；缺许可等待必须能让出工作窗口 | ❌ 源 gate 装配共享且 GLM 拒绝供应商停用，但调度有 W1/W2 |
| E9 | 新鲜完整扫描否定旧观察时不得继续用旧缓存确认；缺失/不可读保持未收敛 | ⚠️ epoch/execution stamping 正确；缺失缓存保留有 W4 |
| E10 | run 停止拒绝后续发送并结算已发请求；并发占用 lane 不等于后台永久失败 | ⚠️ stop 原子参与最终 guard；run 对 QueueFull 的处理仍有 W6 |
| E11 | 新 worker 仅显式库入口，不修改现行 CLI/GLM 规则，不升级 schema，不把未来可信 producer 当已实现 | ✅ diff 没有 main/orchestrator/quota、DDL/schema 变更；初始 backend 不 ready，permit 不由 worker 签发 |

## 必须处置的发现

### W1 ❌：NotReady 成员互相占位，有限工作窗口可能饥饿

位置：`src/reconcile/worker.rs:637`，`step` 的 `Wait(NotReady)` 分支，尤其约 645–651 行。

当前只要任意同组 peer 是 dirty 且到期，就 `Ok(false)` 而不延期，没有证明该 peer 能执行解除屏障的动作。两个成员都因缺 permit、permit 过期或组未 ready 而等待，且已有新鲜观测时，会互相当作“下一轮可推进”对象；两者的 next_attempt_at 始终不变。扩大到至少 64 个低 ID 成员时，它们可持续占据 `due_members LIMIT 64`，让更后面的普通任务或内存策略停止任务无法被选中。

契约仅允许“确有同轮可执行阻挡项”时留到下一轮；其他等待必须持久延期。建议仅识别实际可推进的降低/禁用/必要观察，或采用保证公平性的延期规则。需要无 permit 的同组多成员与超过 64 个到期成员的回归，证明无相互等待占位和后续任务饥饿。

### W2 ❌：内存来源停用没有进入停止优先级

位置：`src/reconcile/worker.rs:663` 的 `round`、`src/catalog/confirmation.rs:254` 的 due SQL，以及 `publish_route`。

`publish_route(disabled=true)` 只在 Core 的 `source_disabled` 中存 gate、通过 wake_pending 标 dirty；`round` 直接采用 Store 的 due_members。该 SQL 的停止优先项只看持久的 manual_paused/retire/status_directive，不看 source_disabled。故一个旧低 ID 的创建/提升任务会排在后来的供应商停止前面；配合 W1，还可能把停止挤出前 64 个候选。

源 gate 已正确按 SourceId 共享，但执行资格与调度优先级是两件事。应在截取 64 个成员之前把进程内来源停止纳入排序，不能只对已经截取的 64 项排序。需要跨组、不同成员 ID 和满窗口的实际请求顺序回归。

### W3 ⚠️：旧失败可能覆盖新发布输入的唤醒

位置：`src/reconcile/worker.rs:353` 的 `defer`，约 362–374 行。

该函数调用 `mutate(None)`，只比较 member/source/group revision。`publish_route` 和 `publish_backend` 推进 publication 并唤醒任务，但不一定改变这三项 revision。其后旧在途请求失败，仍可把刚唤醒的同一目标延期 30 秒或更久；终端错误还可能写为 failed。成功 GET 的发布有 expected epoch，这个错误路径没有同等限制。

应避免旧环境的失败覆盖已发布的新输入，或明确证明该错误仍适用。修正时区分“允许结算旧执行日志”和“允许把旧错误写到当前调度状态”。回归应在旧读取等待期间发布新 backend/route，然后交付旧失败，核对新唤醒/新操作状态不被改写。

### W4 ⚠️：新扫描遗漏 required ID 时保留了旧缓存

位置：`src/reconcile/worker.rs:418`，`observe` 的成功发布循环。

成功扫描只插入找到的对象，没有清除该次 required 集合内未返回的成员缓存。若已有仍在 30 秒有效期内的观察，后来完整扫描明确缺少该 ID，旧缓存仍能参加 Confirm 或组提升屏障。新扫描应使该次范围内缺失成员的旧观察失效，并对缺失保留未收敛状态。首次即缺失时虽不会伪造成功，但还需要有界延期，避免每轮重复 Observe。

需要在已有新鲜缓存之后移除一个 fake 渠道，再执行覆盖该 ID 的扫描，断言旧缓存不能确认或作为 barrier 证据。

### W5 ❌：读取 5xx 被映射为永久拒绝（主 agent 已声明正在修正）

位置：`src/newapi/transport.rs:751`。

当前只有 `status.is_server_error() && semantics != Read` 才映射 TransportUnknown；Read 503 落入 RemoteRejected。worker 的 terminal 将 RemoteRejected 视为终端，因此暂时的读 503 不重试而转 failed。应按契约保留暂时故障并延期自动对账，补实际 GET 503→恢复的回归。

### W6 ❌：run 遇另一轮持有 lane 时退出（主 agent 已声明正在修正）

位置：`src/reconcile/worker.rs:714`，`run` 中 task result 的双 `?`。

`round.try_lock` 返回 QueueFull，run 会把它直接传播并结束。并发 `reconcile_once` 是暂时占用，不应永久终止后台协调器。需要后台与显式轮次并发、lane 释放后继续协调的回归。

另外，第一版 lookup 使用 `let _ = self.defer(...)` 丢弃存储失败；审计中已观察主 agent 将扫描错误分支改为传播 `defer` 结果。大扫描后重新 GET 的错误分支仍应一起核对持久延期，不能仅修主扫描分支。

## 独立执行的验证

- `cargo test reconcile::worker::tests -- --test-threads=4`：11 passed，0 failed，含 publication 128-case PBT；14.48 秒。
- `cargo test newapi::transport::delivery_tests -- --test-threads=4`：8 passed，0 failed，含 envelope 128-case PBT；10.01 秒。
- 首次在沙箱内执行 worker 测试，全部在 fake 的回环 bind 处遭 `PermissionDenied`；随后仅申请并获准运行上述隔离测试。此环境失败不计作实现缺陷，也不计作通过。

上述测试是本审计自行运行的证据，覆盖已有测试的场景，不能关闭没有对应回归的 W1–W6。未运行生产实例、未运行真实预检 producer/供应商策略 producer，也未据本次结果声称全部 SGM-2 或发布门禁完成。

## 本轮结论

❌ 第一轮不能关闭 worker 审计：W1/W2/W5/W6 尚须修复并复审；W3/W4 须修复或给出经确认的契约处置。实现已有正确的准备事务 fence、日志交付证据、单 lane、取消后结算和组内回读屏障基础，但这些基础不能代替调度与旧结果边界验证。

即使上述项全部关闭，BackendReadiness 的真实实例/构建预检 producer、供应商策略 producer 及后续完整验收仍未交付；worker 验收通过不代表整个 SGM-2 完成，也不构成生产部署授权。

## 第二轮复审

本轮重新审 diff，未使用主 agent 测试结果。审阅对象：worker `86d9c83b356e864ab69a80fe41f883f82bfe42d0`，worker tests `6e306b71becbc4e006db8ee3935624d79d497edb`，confirmation `3920b2141a854b49584512b058cf64c5ff8e6e6d`。第一轮结论保留为历史，下列状态取代其对应项。

| 项目 | 独立复审结果 |
| --- | --- |
| W1 | ✅ `can_route` 先排除自身许可/就绪/gate 不满足，再仅将实际可执行 Disable 或降低 priority 的 peer 作为可推进阻挡项。缺许可互等回归通过 |
| W2 | ✅ `due_members_with_stops` 在 SQL LIMIT 64 之前加入 source_disabled 的来源集合；65 成员、最后一个成员停止的回归实际确认其先被禁用 |
| W3 | ✅ `input_epoch` 使用最近一次成功外部发布的偶数 publication 值；内部观察/结算保留该值。defer 对照输入 epoch，write 结算后也检查输入 epoch，旧读取失败不覆盖新唤醒的回归通过 |
| W4 | ✅ 原有“新扫描缺失仍保留正缓存”的问题已修正。Found/Unavailable 替换同一成员缓存，缺失/漂移清 observed 字段，负观察绑定四重 revision 与 30 秒窗口；新增立即 retry 问题见 W7 |
| W5 | ✅ 修复位于 worker `code()`：HTTP status≥500 映射可恢复 TransportUnknown，传输原错误 kind 不再导致 worker 永久失败。实际 GET 503 三次后 dirty/waiting/至少 30 秒延期，恢复后收敛的回归通过 |
| W6 | ✅ run 通过 `accept_background_round` 仅吞掉暂时 QueueFull，保留其他错误；lane 占用期间后台未退出、停止可完成的回归通过 |

新 `record_observed` 静态核对：输入集合有界且成员不重复，status/priority 同时有值或同时为空，值域明确；事务内逐成员检查完整 Revision（含 execution），一处失败整笔回滚。SQL 仅更新 observed_status/observed_priority，没有 applied、操作收据或归档写入。身份与新鲜度由 observe 验证，缓存发布在目录事务成功后，HTTP 期间 publication 改变会拒收整个结果。它增加的是诊断和调度证据，不是成功确认。

控制创建包装器保留内层 CatalogError，包括 DuplicateSource 的 source_id；失败不推进 input_epoch，未知提交仍由 Store 可用性阻止偶数 publication。`worker_catalog_wrapper_preserves_duplicate_identity_and_counter_never_wraps` 独立通过。

独立执行 `cargo test reconcile::worker::tests -- --test-threads=4`：18 passed，0 failed，15.72 秒。`git diff --check` 通过。

另外独立运行 `cargo test confirmation -- --test-threads=4`：25 passed，0 failed，23.54 秒，覆盖既有确认/重试/历史收据与 PBT 回归。此前使用过更窄但错误的模块过滤器，得到 0 tests；该空运行不作为验证证据。

### W7 ❌：显式 retry 没有使旧负观察失效

`retry()` 重新安排原操作并推进 input_epoch，但不清理其成员的 CachedObservation；Unavailable 的有效性只核对 Revision/时间，不核对 input_epoch。缺失渠道导致 BindingDrift/failed 后，修复远端并立即重试同一操作时，目标 Revision 不变，下一轮仍直接命中旧负观察并 Reject，未发 GET 就再次 failed。显式重试应重新安排观察，不能要求用户等负缓存 TTL 后再重试。

需要使重试目标的正/负缓存失效，或提供等价保证；回归应在恢复 fake 渠道后立即 retry，证明重新读取且可成功。

### W8 ⚠️：可执行 blocker 查找重复装配完整组

`actionable_blocker` 对每个到期 peer 调用 `assemble`，后者每次都 `load_group_work`。不存在可执行 blocker 时，单次检查会进行 O(n²) 的组装/SQL 读取；成员上限 10000，且每轮最多 64 个成员。应复用已有一致组上下文，或明确记录和验证可接受的容量限制，避免有限输入下仍出现实际不可接受的运行时开销。

### 文档同步

本轮审阅时，细化和 expectations 尚未描述 input_epoch、record_observed、Found/Unavailable 与新的停止排序。尤其 record_observed 是新增非平凡持久写 helper，需要补充上述前置条件、原子性和零确认副作用；input_epoch 应明确成功外部发布与内部 mutation 的区别。此项须在收尾前同步。

第二轮结论：原 W1–W6 已有对应修正证据；新增 W7 未关闭，W8 与文档同步待处置，暂不签署本小步通过。

## 第三轮与最终 checkpoint 意见

最终复核对象：worker `edffe84c3c262d7a94857933420c0acd06cec3af`，worker tests `084c3f9391a8875ca7d806c4ed0aaee445928b6f`，confirmation `3920b2141a854b49584512b058cf64c5ff8e6e6d`，confirmation tests `3aa624d659c056880a78248225c1dc553fc02dbd`。transport/delivery tests 与第一轮独立通过对象相同，分别为 `e0323e3` / `ff2b553`。

- ✅ W7：非 Succeeded 的显式 retry 成功重排后，在同一 Core mutation 中清正/负缓存；未知执行、授权消费和持久收据不被改动。成功历史重试保留缓存且不调用认证恢复。新增“缺失→修复→立即 retry→重新 GET→成功”回归独立通过。
- ✅ W8：actionable_blocker 只持一次 Core 锁并核对 publication，复用已有 plan.group 与缓存，不再逐 peer assemble 或访问 SQLite。诊断性纯计划只接受 Disable/priority 降低；空组屏障不会被当作真实执行许可，实际写仍走完整重新计划和 prepare_execution。遍历相对于现有组输入为线性。
- ✅ 文档：细化新增“执行器审核修正契约”，明确 input_epoch、record_observed、正/负缓存、显式 retry、窗口前停止排序和辅助函数边界；peers_need_observation 的实现现与文档一致，核对 publication。
- ✅ lookup 残留：过期创建匹配对象的刷新 GET 已抽为 refresh_creation_observations；错误携带 Retry-After 进入 defer，延期存储失败优先传播 Storage，不再立即到期重扫。它只刷新匹配且过期的对象，未绑定身份，也不清 unknown。
- ✅ record_observed 新增两项固定回归和 128-case PBT，实际覆盖非法/重复批次拒绝、中途数据库故障整笔回滚、四重 revision 的随机冲突，以及成功观察与清除观察均不改变 applied、意图、操作投影和历史收据。

本轮自行执行的证据：

| 命令/范围 | 结果 |
| --- | --- |
| `cargo test reconcile::worker::tests -- --test-threads=4`（W7/W8 修正后） | 19/19 通过，15.55 秒 |
| 同一 worker suite 加入刷新失败测试后 | 19 passed / 1 failed，25.06 秒；失败在新测试夹具的名称断言，尚未进入待测分支，未计为刷新失败路径通过 |
| `cargo test worker_stale_creation_refresh_failure_defers_and_preserves_storage_errors -- --test-threads=1`（修正夹具后） | 1/1 通过，0.12 秒；同时执行普通延期与注入 Storage 失败两分支 |
| `cargo test record_observed -- --test-threads=4` | 2/2 通过，0.12 秒 |
| `cargo test observation -- --test-threads=4` | 2/2 通过，19.59 秒，含新增 128-case observation PBT 和既有计划器观测回归 |
| `git diff --check` | 通过 |

夹具修正已独立检查：先将 fake 服务对象名称设置成持久 create_name，再 GET 构造真实匹配观察；保留名称断言，没有删除或放宽期望。最后只改该夹具，故重跑目标回归；其他 19 项已有同版本实现的通过结果。

新增原生 worker 测试 diff 已检查：使用独立临时目录、假来源凭据和未注册组，只验证禁用创建、绑定、回读与来源退役，不提供 permit、不启用、不发推理。本审计未独立执行该原生测试，主 agent 的运行结果没有计入这里的独立执行证据。

最终意见：✅ 在本报告限定的 HTTP 交付证据和 ReconcileWorker 组装 checkpoint 内，已发现的正确性、恢复和调度问题均有修正，0 个 critical / unresolved 实现项；可关闭本轮路径 B。真实 BackendReadiness/供应商策略 producer、完整机械检查与变异门禁、后续原生/跨模块验收和部署接线仍须分别满足，不能据此宣布整个 SGM-2 完成或授权生产部署。
