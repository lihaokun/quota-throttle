# SGM-2 身份绑定与回读确认独立审核

日期：2026-10-15。基线：`a42545f`。工作分支：`research/source-groups-token-management`。路径 B 审核。结论：初审发现的一个阻断项已修复并独立复审关闭；当前无未解决的实现阻断。PBT 组合覆盖仍有明确边界，见 W2；不构成 SGM-2 整体验收。

## 审核方法与独立期待

先阅读 `docs/workflow.md` §6、SGM-2 细化（重点 P14–P16 及“绑定和完成确认”）、主架构“第二子计划共享类型补充”、expectations §1–10，再阅读基线差异。仓库无 `templates/contract-audit/subagent-prompt-template.md`，按 §6.5 等价执行。主 agent 的测试结论及 Step 5 运行记录不作为本报告判断依据。新文件通过 `git diff --no-index /dev/null FILE` 审阅，没有扩读其他未改完整实现。唯一补充的旧实现证据，是为验证本报告 C1 而由主 agent 提供的 `execution_one_prepared` 唯一索引和对应校验约束。

独立提取的本步期待：

| 编号 | 必须成立的契约 |
| --- | --- |
| E1 | 创建认领须有持久、非 NotSent 的唯一创建票，实例命名空间及完整票匹配，完整唯一关联对象的稳定身份与新鲜度正确；0/多匹配、占用 ID、漂移都拒绝。 |
| E2 | 绑定只记录身份及创建效果已物化，不伪造 HTTP 回执，不写 applied/成功收据；已绑定 create 的豁免不能扩展到一般 prepared/uncertain 写。 |
| E3 | 确认在一致事务内检查 member/source/group/execution 四代次、实际字段、身份、新鲜度和最新计划；只接受 Confirm/RetireAbsent。 |
| E4 | 收据按目标身份与代次去重；冻结目标全集完成才成功；再写撤销未完成操作中该成员的临时收据，历史 succeeded/superseded 不变。 |
| E5 | source/member 退休、applied、收据及操作阶段原子提交；最后一个有效成员确认退休后才归档，物理 ID 不删除。 |
| E6 | 暂时失败持久延期至少 30 秒并尊重更晚 Retry-After；需处理错误停止自动调度；显式重试保留原 operation、unknown、暂停和授权消费，不复活 superseded。 |
| E7 | 到期读取最多 64 条，停止/退役优先；不加载全库秘密；本步无网络事务、无生产接线、无 schema 升版、无旧测试弱化。 |

## 不一致与处置

### ✅ C1：已绑定 prepared 创建仍占用唯一 prepared 槽（初审 ❌，复审关闭）

初审位置：`src/catalog/confirmation.rs:83` 的绑定事务仅写映射、Resolved 和 execution；`src/catalog/execution.rs:436` 的 `pending_writes` 随后忽略这一 prepared create。旧 v3 DDL 的约束仍为 `CREATE UNIQUE INDEX execution_one_prepared ON execution_attempts(membership_id) WHERE state = 'prepared'`，`verify_execution` 同样要求每成员最多一个 prepared。

反例：prepare Create → 服务端已提交、回执尚未结算 → 完整唯一新鲜对象绑定 → 不重启直接 prepare Enable/Fields。计划看到 PendingWrite::None，但下一次 prepared 插入触犯唯一索引，正常后续协调被拒绝；需等待回执或额外重开才恢复，与绑定已足以继续协调的契约相违。初审测试 `confirmation_unknown_creation_materializes_without_fabricating_a_response` 先重开再确认，创建 PBT 也先重开再绑定，正好把 prepared 转 uncertain，无法捕获该路径。

复审：`confirmation.rs:90` 在绑定事务将已物化的 prepared create 转 uncertain，保留真实交付未知，不伪造 responded，释放唯一槽；`execution.rs:267` 对已经绑定创建的矛盾 NotSent 返回冲突并回滚。新增 `confirmation_tests.rs:221` 不重开直接 bind→Enable，验证矛盾 NotSent 不改代次、后续真实迟到回执不改历史成功、重开仍有效。该回归由审核者独立运行通过，C1 关闭。细化文档同步说明上述转换，没有修改 v3 DDL。

### ✅ W1：安全谓词在多个位置重复（初审 ⚠️，已消除）

初审 `execution.rs` 的 pending 判断与 `confirmation.rs` 的操作阶段聚合分别复制“create 且已绑定且 resolved 才豁免”条件；Rust `terminal` 集合又在 SQL IN 列表重复。当时集合相符，但只改一份会出现“成员可确认而操作仍 unknown”或“dirty 停止但操作不 failed”。复审已改为唯一 `execution.rs:19` 的 `PENDING_EFFECT_SQL`，两个查询都复用；`confirmation.rs:362` 的 `terminal_error` 遍历有限目标并统一调用同一 `terminal` 分类，删除重复 SQL 枚举。没有引入通用工作流抽象。

### ⚠️ W2：PBT oracle 有独立集合，但状态空间仍有盲区

初审四项 PBT 均配置 128 cases；R6 的 HashSet 确认集合是独立 oracle，有实质价值。以下记录初审盲区及复审处置，不将“128 cases”本身当作覆盖完整性证明：

- 创建 PBT 固定先 reopen，缺少 C1 的连续运行路径。已新增不重开直接绑定、继续 Enable 和迟到真实回执的基本回归，关闭本次具体漏洞。
- 旧收据 PBT 只随机普通 update 次数后投一个旧票；四个 expected 字段被篡改的基本测试不能替代实际四代次迁移。已新增 `confirmation_tests.rs:766` 的 128-case 轨迹，随机执行成员意图、来源模型、组优先级和执行写，逐次拒绝旧收据并保持历史，继而显式重试和新鲜确认；审核者独立运行通过。
- 原不清授权测试只创建未确认 Enable unknown，未核对已消费授权。新增 `confirmation_tests.rs:318` 检查完整 `(enable_epoch, enable_requested_revision, enable_consumed)` 在失败/重试前后相同，并验证普通模型更新后的 preserve 在实际 status=3 上可确认，不再次 Enable；独立运行通过。
- ID 已被另一成员占用、future 观测和绑定各写点回滚已补（`confirmation_tests.rs:162`、`:257`），对应新增回归独立运行通过。

复审又补 `confirmation_glm_consumption_failure_rolls_back_readback_and_receipt`，在 enable_consumed 的 UPDATE 点注入失败，核对完整快照、收据、消费标记及 queued 阶段全部回滚；审核者独立运行通过。

剩余建议：把多成员 partial→同成员再写→其他成员确认→旧 ACK 与延期/重开组合成同一独立状态机；当前这些条件由多项基本测试/PBT 分别覆盖，尚非任意组合证明。这是后续补强建议，没有发现新的实现错误，不阻断本步；完整网络 worker 的迟到写、401/429 和 fence 轨迹仍须在后续阶段验收。

## 逐项对照

| 期待 | 结果 | 差异证据与范围 |
| --- | --- | --- |
| E1 | ✅ | `creation_ticket` 限定唯一非 NotSent 创建，整张票比较隔离实例，`resolve_created` 承接完整唯一稳定身份，额外检查 fresh 和 ID 占用；重复同 ID 零写。 |
| E2 | ✅ | 绑定不写 observed/applied/收据；pending 与操作聚合均只豁免已绑定 resolved create；一般 unknown status 写的匹配 GET 测试仍拒绝。C1 修复后不再占 prepared 槽，交付仍为 uncertain。 |
| E3 | ✅ | `confirm_member` 重装快照并比较完整 Revision，再调用计划器，仅 Confirm/RetireAbsent 可写；额外拒绝 pending_write。四代次、字段、身份、stale 观测均有拒绝测试。 |
| E4 | ✅ | 收据 UPDATE 限定 member revision、NULL 首次写及非历史终态；prepare 同事务清该成员未完成收据；阶段投影先核冻结数量并要求全确认且无 pending。缺目标行回滚及重复/再写/历史保持有回归。 |
| E5 | ✅ | 同一事务更新生命周期、applied、收据、归档和阶段；来源归档有 NOT EXISTS 未退休成员条件，保留 channel_id。随机顺序集合测试与中途回滚测试支持。 |
| E6 | ✅，W2 补强建议 | defer 核完整 Revision、安全时间并取 max(now+30,retry_at)；显式 retry 先查历史、拒绝 superseded/代次失效、只调度未确认行。没有修改创建状态、暂停或授权字段的 SQL。 |
| E7 | ✅（本步） | due 查询 LIMIT 64 且停止/降优先；新增业务模块无网络调用，无 schema/Cargo/旧策略变更。原 `execution_tests.rs` 只增加子测试模块，没有弱化既有断言。完整 worker/真实交付 producer/生产接线仍未实现。 |

## NewAPI 推论边界核实

独立打开固定提交 `6ce7305cd36f16506fb6a2c3c524a5a318539ba7` 源码：`AddChannel` single 分支构造单元素 keys，合法非空 key 因而最多且实际只创建一个渠道；调用 BatchInsertChannels 后只重置代理缓存、记录审计、返回响应。[AddChannel](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/channel.go#L563)

`BatchInsertChannels` 在同一 tx 内 Create 渠道和 AddAbilities，错误回滚，最后 Commit。因此，对同一已核实实例、唯一稳定名称、单次非 NotSent 创建和排除其他写者这些前提，观察到唯一持久对象可以证明该次创建效果已物化。[BatchInsertChannels](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/channel.go)

这是上述代码与前提共同支持的推论。它不证明收到 HTTP 响应，不提供远端 CAS，不证明渠道随后不会被其他写者修改，也不能用于清除一般未知字段/状态写。`BackendReadiness` 输入类型本身不证明生产实例身份与单写者已经落实；后续真实 producer、部署核对和持续对账仍是必要条件。本次没有访问生产目录、生产端口或真实 key。

## 独立验证与退出边界

初审独立执行 `cargo test confirmation_ -- --test-threads=1`：19 passed，0 failed，53.46 秒，其中四项为 128-case PBT。该通过结果没有捕获 C1。复审独立以 `cargo test --locked --offline <test> -- --test-threads=1` 执行新增直接 bind→Enable、绑定占用/回滚、已消费授权重试、四类代次交错 PBT 和 GLM 消费写失败回滚，五项均通过；交错 PBT 为 128 cases，11.94 秒。`git diff --check` 通过。未借用主 agent 的全量测试、变异或原生集成运行结论。

另审到新增隔离原生测试只操作其自建实例和假 key，没有删除原断言。它先强校验固定 binary SHA-256 `90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205`，明确要求该测试构建的 `/api/status` 实报 `v0.0.0`，再由固定构建身份提供 canonical rc.20 readiness。该 test-only 适配没有放宽生产 `BackendReadiness` 的版本检查；它是本地构建身份假设，不是生产预检 producer 已完成的证据。本报告独立核实的是源码事务推论与新增测试差异，没有独立运行该二进制或重复核验其构建元数据，因而不宣称原生集成运行通过。

C1 已关闭，本报告只允许关闭身份绑定/回读/持久调度小步；SGM-2 仍缺 HTTP 交付证据 producer、完整 worker、正式组装和既定全部退出门禁，不能标记整体完成。

## 路径 A 运行记录

本节由主实现者在独立审核后补充，不作为上述独立判定的输入。

- `cargo test --locked --offline`：209 passed，0 failed，1 项原生测试默认 ignored。新增 24 项普通用例，含 5 项 PBT，各 128 cases；保留全部原有断言。
- `bash scripts/contract_audit/sgm2_confirmation.sh`：通过。验证 P14/P15/P16 所在函数、零网络目录事务、共享枚举、真实 queued/partial/failed/succeeded 输出 schema、旧 snapshot 和保护文件差异。
- release 构建通过；Clippy 仅 6 个既有风格警告，本步新增警告已消除。due 测试验证 70 个候选只返回 64 个、停止优先与至少 30 秒延期；未声称完整 worker 的端到端时延已验证。
- 单独原生验收：`QUOTA_THROTTLE_NEWAPI_TEST_BINARY=<固定哈希二进制> cargo test --locked --offline native_managed_channel_contract_on_fresh_database -- --ignored` 通过，32.02 秒。只运行自有 Child、随机非生产端口、新库/假账号/假 key。保留旧禁用创建/字段/状态/分页断言，追加创建 ACK 不落库→目录重开→唯一绑定→真实 GET→退役确认，物理渠道仍存在且 status=2。它不模拟任意网络乱序，也不代表 worker 接线完成。
- 原生初次扩展运行因预期发布版本而实际收到 v0.0.0 失败。随后只读核验二进制内嵌 module=`github.com/QuantumNous/new-api v1.0.0-rc.20`、vcs.revision=`6ce7305cd36f16506fb6a2c3c524a5a318539ba7`、vcs.modified=false，并与既有冻结 SHA-256 一致。测试启动前强校验该哈希；运行版本明确断言为 v0.0.0，canonical readiness 来自固定构建身份。没有将未知版本默认 ready，没有修改生产构建或预检类型。

七项变异分别在独立临时源码与 target 中运行；均出现目标测试失败。正常 worktree 未被变异：

| 故意错误 | 捕获用例 |
| --- | --- |
| 把不完整创建扫描当作完整 | confirmation_binding_rejects_missing_duplicate_drift_stale_and_foreign_ticket |
| 确认忽略 expected revision | confirmation_requires_actual_fields_freshness_identity_and_every_revision |
| 忽略所有一般未决写 | confirmation_unknown_status_write_is_not_resolved_by_matching_get |
| 有一个收据就标整项成功 | confirmation_partial_retire_archives_only_after_last_member |
| 再写不撤销未完成操作的旧收据 | confirmation_rewrite_invalidates_only_unfinished_receipt_and_ack_cannot_finish_it |
| 绑定后仍占 prepared 槽 | confirmation_prepared_create_bind_immediately_enables_without_reopening |
| 未等最后成员退休就归档来源 | confirmation_partial_retire_archives_only_after_last_member |

§5.2 五维度复核：本步接口/设计与实现一致；采用既有事务/安全错误/私有目录风格；确认、绑定及恢复的原子性由故障注入和代次测试支持；工作读取限 64、操作投影限定当前目标且不复制凭据；安全谓词共享、无新服务/数据库版本/通用引擎。W2 更广组合覆盖已在 decisions 中保留为后续 worker 门禁，未降低整体退出要求。
