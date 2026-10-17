# C2 管理操作与来源模型同步

日期：2026-10-18，基线 da35d8e。用户已确认开始 C-2，沿用当前开发分支和连续实施授权。本步完成内部可信管理命令及 worker 入口，不挂 HTTP、不写生产配置、不部署。GLM 算法、火山计量/日格/错误锁存及 NewAPI 推理不变。

## 1 目标与复用

新增 GLM 组固定/取消固定、火山人工撤错/设置、来源模型发现；复用 ControlCommand、control_operations/operation_targets、原策略 helper、worker publish fence 和原渠道执行器。没有第二套任务表或执行状态机，不引入新依赖。

目标分解：目录负责规范命令、幂等、版本和配置/状态同事务；worker 负责单命令槽、模型 GET 的单飞和响应返回后的版本复核；原 C-1/O4 负责新策略输入的目标/许可发布；原执行器负责真正的远端写入与回读。UI 和运行调度由后续模块组装，不能在 C2 偷启后台。

局部配置命令的 Succeeded/0 targets 只表示管理设置已提交，不表示渠道已切换。当前选中成员、目标、实际渠道状态继续分开；未来页面必须按 kind 显示“设置已保存”。模型同步仍产生原成员 targets，只有原执行器回读才能成功。复用同一回执结构，不新增“伪远端成功”阶段。

## 2 命令与公开回执

OperationView 字段不变，wire 从1升为2以显式区分扩展后的 kind；CatalogSnapshot wire仍1。ControlCommand 是进程内可信输入，不增加任意 JSON patch 或 Deserialize。四种新 kind 和所有原 kind 共享 OperationId 名称空间、摘要算法和原错误码。

DB/JSON共享拼写固定为 SetGroupPin→set_group_pin、ClearVolcanoError→clear_volcano_error、UpdateVolcanoPolicy→update_volcano_policy、DiscoverSourceModels→discover_source_models。旧六kind拼写和阶段集合不变；schema须逐项对照共享OperationKind，而不是另建枚举。

| 命令 | 输入 | 后置 |
| --- | --- | --- |
| SetGroupPin | group_id、expected_revision、expected_policy_revision、membership_id:Option | 只接受GLM组；Some 必须是该组已绑定/接管且未暂停/退役的成员；None取消固定。保留 active/regime；组revision增加，原policy revision仅在pinned状态变化时增加，不直接写priority/status/成员接管记录 |
| ClearVolcanoError | source_id、expected_revision、expected_runtime_revision>0 | 只接受已初始化火山源；原 advance_period/clear_error 与历史同事务。先按日格推进周期，再清当前周期错误原因/计数并更新cutoff/epoch；撤错本身不清当前周期token上限原因或人工暂停，跨日重置仍可合法清旧周期上限 |
| UpdateVolcanoPolicy | source_id、expected_revision、expected_runtime_revision≥0、policy | policy 使用原 VolcanoPolicy 值域。Runtime已初始化则复用ReplacePolicy及历史；未初始化仅允许expected_runtime_revision=0，保存设置但不伪造零用量或初始化Runtime |
| DiscoverSourceModels | source_id、expected_revision | 只由专用异步worker入口解析；读取该源配置和材料后一次GET，成功复用原SyncSourceModels目标/版本/收据规则，失败不覆盖存量模型 |

新局部设置命令每个新 OperationId 将目标实体revision增加一次（即便设置值相同），与原显式控制命令一致；重放不增加持久版本、error_epoch、历史或远端副作用。局部命令重放沿原submit维护内存publication/input_epoch同步屏障，不为此重构基础发布机制；发现入口的Replayed直接返回，不发布。ClearError也增加source.config_revision，令旧正向许可失效。UpdatePolicy原helper若已增config revision，不再二次增加。target_revision仍指目标实体的配置版本，不混用Runtime或policy版本。

Pin不新增正向许可，不能绕过95%/耗尽释放规则，也不强制启用原生自动禁用成员。所有新局部命令不新增operation_targets，不接管未授权成员。禁用/暂停/退役沿原命令优先。

## 3 数据库 v7 迁移

冻结v1–v6 SQL，v7仅替换catalog_meta约束与control_operations的kind/目标类型CHECK；字段、旧记录、operation_targets和执行历史保留，不增加长期表。新增本地命令要求expected_targets=0、phase=succeeded，发现模型命令沿原source目标规则。

v6完整校验和私有一致备份后再迁移；早期版本仍保留一份原版本备份、逐版本链推进。未知提交沿poisoned，损坏不自动修复。

control_operations被operation_targets引用。迁移在单一IMMEDIATE事务、foreign_keys=ON下先用两个保留名称的临时迁移表SQL复制这两个表，再按子表→父表顺序删除旧表；按v7父表DDL和冻结v2子表/索引DDL重建，复制回原字段，删除迁移表。不用ALTER改名旧父表，不改sqlite_schema或放宽DDL核验；SQL集合复制不将操作历史全量载入Rust Vec。重建前拒绝这两个表上不属于契约的额外触发器/索引，避免静默丢用户扩展；保留名称冲突、外部FK依赖或任一SQL错误均回滚。

这遵循“重建并复制数据”的迁移思路；SQLite官方明确警告先重命名旧父表可能改坏外键引用。此处按已知父子拓扑一并重建，始终保留外键检查，最后完整检查DDL/FK/旧数据再commit。[SQLite ALTER TABLE](https://www.sqlite.org/lang_altertable.html#making_other_kinds_of_table_schema_changes)

旧版本测试夹具仅补回旧control_operations定义，仍保留全部记录/回执/未决执行和原断言；有新命令的库不做降级。latest DB/wire断言使用相应常量，历史版本验证保持原值。

## 4 函数细化与正确性论证

### C2.0 迁移和验证

`migrate_v6/replace_control_operations` 前置：应用库旧版完整验证和备份已完成；副作用只在当前事务。检测未知索引/触发器、保留表名冲突，SQL复制已知表，先拆子表再替换父表、恢复子表索引和全部记录。最后v7 schema/FK/control核验后一次commit；错误drop事务，未知提交poisoned。DDL来自固定常量，不接受外部SQL。复制循环由SQLite逐行驱动有限旧表；正常退出两张迁移表均不存在，旧数据/实例保留。

`verify_schema/verify_control/operation_projection` 跳过已被v7替代的旧父表DDL，原其余DDL严格等值核验。新局部kind必须零targets且succeeded，source/group kind需匹配；新模型kind只允许source。Projection继续校验冻结expected_targets，不因缺targets伪成功。所有失败拒绝打开/读取，无自动修补。原OperationView shape不变，只更新版本和kind集合。

### C2.1 规范化与幂等

`ControlCommand::target/canonical` 沿原OperationId/安全revision/序列化摘要规则；补expected_policy/runtime的值域，policy.validate共用原方法。模型结果规范化复用原SyncSourceModels，不把模型列表放进DiscoverSourceModels请求摘要。target是无副作用映射；canonical先完整验证再生成摘要，错误无写。

`submit_operation/submit_discovered_models` 共用一个内部提交函数：先校验纯输入并规范化；IMMEDIATE内先查operation_id和digest，完全重放返回旧回执（忽略已经过期的expected revision），冲突返回IdempotencyConflict。新请求才核当前实体、次级版本及生命周期。局部命令走下节；模型发现必须带可信GET结果，普通submit拒绝未解析的发现命令。原六命令行为不变。所有写入和回执同commit，无提交后可失败查询。

论证：摘要只取稳定请求，不取墙钟或变化的上游返回；重放在一切状态变更之前返回，因此撤错epoch和日历史至多推进一次。新请求所有版本检查先于写；任何错误回滚，旧回执不被新调用覆盖。

`canonical_models` 前置为本次输入向量，先核原1000项/非空/逗号和NUL规则，再复用原CSV规范化和排序；不改变旧SyncSourceModels摘要，发现结果错误映射InvalidResponse而显式命令错误仍InvalidInput。`replayed_operation` 前置为规范请求摘要，只读唯一OperationId，缺失返None、相同返严格projection、不同返冲突；不读取材料或改时间。is_policy_setting/is_c2是无副作用的枚举分类，target是固定映射。

### C2.2 局部策略命令

`apply_policy_command_tx` 前置：新命令已过幂等及实体版本核验，tx为当前IMMEDIATE。Pin先读GLM组策略及policy revision；Some成员校验同组、绑定、managed、未暂停/退役，更新pinned并调用save_group_policy_tx，随后bump group.revision；None只清pin。source动作先校验Volcano/非退役及Runtime版本：Clear要求Some，Update允许None/0。Some调用原apply_source_policy_tx（包含PeriodTransition落库），None只验证/保存policy；最后保证source.config_revision为原值+1。插入succeeded/0回执，target_revision引用新实体版本。

后置：只保存配置/策略和必要日历史，不制造渠道目标或新的正向许可；manual_paused和limit_exhausted依原算法，clear_error本身不清当前周期limit，advance_period仍正常重置到期的旧周期。policy/runtime、实体版本、历史和回执原子；循环仅固定成员检查，错误均回滚。可用性变化必须经下一次既有C-1/O4刷新和执行器收敛，不能把局部命令当作远端确认。

### C2.3 模型发现输入与提交

`model_discovery_input` 前置：合法发现命令及OperationId。短只读事务先用同一canonical摘要检查旧回执；重放直接返回旧view且不读取凭据/发GET。新操作查当前source revision、未退役、model_discovery配置存在，再读取该源材料。返回私有Replayed或Fetch，不实现通用Debug/Serialize；事务/Core在任何网络await前释放。

`ModelCatalogClient` 新增有界发现方法，复用现有认证构造和模型解析，不改变旧discover入口。专用客户端禁止重定向；GET总超时10秒（覆盖响应体），响应体最大1MiB、模型结果沿原1000项和CSV规范化校验。失败由worker映射固定InvalidResponse，不泄漏URL/正文/key，不自动重试或调用推理。普通非空成功列表是权威；失败/空列表保留原模型。若未配置发现地址返回NotReady，不猜URL。

成功结果交`submit_discovered_models`在新事务重新核expected_revision/lifecycle，不能把响应贴到更新后的来源。复用SyncSourceModels保存来源及全部非退休成员目标、supersede旧未完成操作、原执行器回读；旧成功回执/执行日志保留。模型排序/去重沿原规范，既有显式接管语义不另扩展。新source添加流程在后续UI组装时调用同一入口；本步不修改创建流程/CLI。

### C2.4 Worker 与版本发布

同步管理命令继续`submit`：沿原commands槽和publish fence同事务提交。新局部命令增加source/group配置版本，故旧permit/observation的版本匹配自然失效，input_epoch在确认提交后推进；不为新设置提前签发许可，不清另一提供者的缓存。失败保留旧数据库/输入代次，未知提交闭锁。新命令dry_run拒绝NotReady，不写状态或发GET；不扩大此步到旧命令dry-run行为重构。

`discover_source_models(id,source,expected_revision)`：dry_run先拒绝；try_acquire commands，再try_lock模型发现单槽，再短Core读取输入。Replayed直接返回，无GET或发布；Fetch在Core外等GET，结束后经原publish调用最终提交。完成、失败、取消都释放命令槽/发现锁；取消GET不落半个操作，不spawn脱离调用者的任务。同一worker发现入口全局单飞，忙返回QueueFull；不建立按key的锁map。最终目录版本检查防暂停/退役/模型更新竞态，旧结果返回RevisionConflict/NotReady而不是覆盖新配置。

局部管理成功只说明设置保存；后续调用既有refresh_glm_policy/refresh_volcano_policy才计算目标和许可，并沿原lower/readback/promote屏障执行。组合测试必须真实走这些入口，不用手造permit掩盖版本失效。

## 5 并发和通信边界

锁顺序：commands→模型发现单槽→短Core/SQLite；同步submit只commands→短Core。网络和超时等待不持Core/事务，不反向获取执行lane。模型GET无会话、每次显式请求复用连接池，断线/超时返回固定错误；可重新提交同OperationId，提交前失败无状态、提交后重放无新GET。最多一个在途发现和原64命令槽，其他请求QueueFull，不新增队列。父进程停止丢弃future释放资源。

来源pause/retire/模型变更与发现响应可交错，靠config revision和原publish fence保证过期结果不能覆盖。自动策略回合与管理命令串行写目录，但允许网络读取并行；其后复核最新配置。依赖同一个CatalogStore单写者和永久实体ID，不尝试跨数据库合并。

## 6 验收与退出

确定性：旧/新命令同ID重放与冲突；Pin跨组/非GLM/未接管/退休拒绝；同值Pin不增加policy revision；原95%释放不变；同周期撤错不清limit/暂停，到期撤错正常重置旧周期且保存历史，重放不改cutoff/epoch；未初始化policy编辑无假用量；坏范围/版本溢出全回滚；模型成功同步多组/失败保留/空列表/超限/重定向/迟到响应/取消/Busy/dry_run/同ID无GET；旧permit失效、新刷新后才收敛。

迁移：v1–v6旧字段/回执/未决执行/instance/备份保留；v7故障回滚、保留名冲突、额外触发器/索引拒绝、不修改FK；原migration fixtures不削弱比较。至少4组各128 cases PBT：局部操作参考模型、迁移保留/重开、模型成功/重放/失败、worker版本/来源隔离。至少8种可编译变异，命令/schema/Step/副作用机械校验，独立契约复审与五维度检查。默认debug/release全量、clippy/build、原性能门禁；新万条操作迁移串行release≤5秒。

不扩大范围：不改原GLM/火山纯算法、事实账本/解析、NewAPI推理/认证/执行器协议，不接key管理/三页面/运行CLI，不读取生产数据，不重启、部署、推送或合并。没有可公开接收任意观测/许可的API。

完整性自检与本地验收：

- [x] 函数实现推导连续，局部保存和远端确认边界明确。
- [x] 新旧命令、初始化/未初始化、重放/冲突及失败分支覆盖。
- [x] 成功、错误、取消及未知提交退出与资源释放明确。
- [x] 原策略、规范化、执行器及数据库 callee 的前后条件承接。
- [x] 迁移有限集合、模型正文/项数、命令槽及循环终止有界。
- [x] 单写者、版本、事务、预检和原策略等上游假设显式。

2026-10-18 C2内部能力验收通过，详见[审核记录](../../../audits/sgm-3-source-policies/c2-audit.md)。这不是完整来源分组功能或在线面板的交付结论。
