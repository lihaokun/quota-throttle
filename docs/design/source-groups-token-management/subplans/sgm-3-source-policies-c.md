# SGM-3 C 策略发布接线方案

2026-10-17 更新：C-3 的旧“严格终态”输入要求由用户确认的[观测型熔断](sgm-4-observed-breaker.md)替代；同源正 token 成功观测清计数，不自动解锁耗尽。C-1 的既有验收和事务发布约束不变，O4 持久/运行接线仍需独立实现与验收，不以纯观测合并替代。

日期：2026-10-16。状态：用户已确认 C-1，事务适配（基线 `fce5187`）、批次发布/成员许可（基线 `791bc56`）、共享采集（基线 `8fa37e1`）均已完成测试与独审，C-1 组合验收通过。方案源码基线 `8113696`，A/B 已验收。C-2/C-3 后续范围不由 C-1 代替，不授权改动运行服务。

2026-10-18 增量契约：火山观测事实/历史及发布按 [O4](sgm-4-observed-publication.md)细化。`GlmPolicyCommit` 新增本类全部当前/历史成员 scope，worker 仅替换该 scope 的许可，保留火山原 lease；下文历史“整个许可集合”在混合提供者中指本类集合，不再清空另一类。GLM 计算/授权规则不变。会推进周期的内部管理更新现保存同一周期历史，公开幂等管理命令仍属 C-2，未因此开放 HTTP。

## 1 范围与实施顺序

目标承接架构 G2/G3/G6：策略选择、持久目标和执行许可使用同一版本，来源仍共享、组选择仍独立。复用一个 CatalogStore、一个 ReconcileWorker 和原生 NewAPI 执行链，不增加服务、推理代理、第二份账本或逐 key 上游绑定。

| 小步 | 交付 | 前置/退出边界 |
| --- | --- | --- |
| C-1 已完成验收 | 共享 GLM 采集 → 当前目录/组状态计算 → 同事务保存选择与必要 priority 变化 → 一次发布许可 | 不改 GLM 算法；不新增表/公开命令；原子性、取消、过期、接管边界及组合测试通过 |
| C-2 后续细化 | 组 pin、火山撤错/编辑、来源模型发现等管理动作与幂等收据接线 | 补命令/schema/收据语义；模型按来源、添加/显式同步触发，不进周期轮询 |
| C-3 与 SGM-4 合流 | 火山事实、周期历史、来源 gate、各组目标和许可原子提交/发布 | 真实生产者证明终态/去重/覆盖；不得以原生 error log 条数或外部布尔值签发许可 |

C-1 不接 up/run/sync、现行面板或生产配置。火山 Runtime、组状态和渠道目标本步不变，不执行日重置、人工撤错或 Evidence 消费，也不签发火山启用/晋升许可。暂停/退役仍走既有控制入口。完整火山自动调度只能在 C-3/SGM-4 门禁通过后启用。

## 2 已核实的接线缺口与复用

基线中 `ReconcileWorker::publish_route` 逐成员更新 source_disabled/permit；`save_group_policy` 独立提交组状态；`submit_operation(SetGroupPriorities)` 则每次提升组/成员版本并产生操作。把这些入口顺序调用，既不是同事务，也会在周期性 no-op 时制造新操作。

另一个边界是 `WorkSnapshot.managed`：它来自既有 operation_targets，不是“目录里存在成员”。未接管的 imported/planned 成员不能因自动策略发布就被隐式接管。

| 复用项 | C-1 处理 |
| --- | --- |
| SourceProbe/ProbeInput/ProbeCycle | 保留一源一查、来源版本和单飞轮次；采集不持 Core/目录事务锁 |
| decide_glm_group/GlmGroupState | 不改原 95%/降级/90%/临期/pin/未知行为；读取 B 的持久组状态 |
| save_group_policy_tx | 在当前事务保存组状态，内容相同不增 policy_revision |
| member_control/memberships/旧控制语义 | 仅更新受管、可编辑且 priority 实际变化的成员，保留原 GLM 启用消费规则与历史成功收据 |
| worker mutate/publication/input_epoch | 提交前封旧发送 gate，成功后一次替换完整许可集；未知提交保持关闭 |
| planner/prepare_execution/回读屏障 | 保留先降旧、回读，再提升新首选；不绕过未知执行和实例预检 |

现有 `load_work`/`group_work`、组状态只读 helper 可按需要开放 catalog 内事务使用；不把 Connection 暴露给页面/CLI，也不复制一套 SQL 解码器。

## 3 错误与不变量

复用 ControlResult：非法配置/时刻为 InvalidInput；过期、资格不足为无许可/NotReady；旧轮次拒绝为 RevisionConflict；队列/采集单飞占用为 QueueFull；SQL、解码损坏和版本耗尽为 Storage。quota 单源失败仍是 Unavailable，不转换为 0 或来源耗尽。commit 不确定沿用 Store poisoned 和关闭 publication，不发布成功。

- I-C1：组状态与对应成员 priority 目标要么同事务提交，要么全部回滚。
- I-C2：新许可只能引用已提交的成员/来源/组身份与版本；commit 之前不可见。
- I-C3：同源观测跨组复用，unknown 不被造为 0；原 GLM 纯函数及结果保持不变。
- I-C4：未 managed、暂停、退役、未绑定、组未 ready 或预检不通过的成员不得获启用/晋升许可。
- I-C5：普通 priority 更新不重新授予 GLM 已消费的显式启用权；不改 execution_attempts、旧成功回执或来源 manual_paused。
- I-C6：没有目标变化就不提升持久版本、不新建控制操作；新的合法观测轮次可以更新内存许可时限。仅资格从无效变有效时，可唤醒既有 NotReady 待办，不每轮重置退避。

依赖：现有单写者/私有库锁、SQLite 回滚与 commit 契约、SGM-2 发送 fence/回读规则。外部 NewAPI 不提供远端 CAS，因此“原子发布”仅指本地状态与发送资格；已经发出的旧请求仍按既有未知写/对账机制处理，不承诺即时撤回。

## 4 内部类型与所有权

C-1 不改数据库 v4 和公开 CatalogSnapshot/OperationView wire=1。所有新增输入/结果不实现 Deserialize/公开 Serialize；private material 不进入日志、Report 或 Debug。

| 类型/字段 | 所有者与生命周期 | 约束 |
| --- | --- | --- |
| worker 的 `policy_config: Arc<Config>` | worker 创建时接收现有配置，生命周期内只读 | 不新增另一套配置格式或热重载；new 的测试调用补同一份假配置 |
| worker 的 `probe: AsyncMutex<SourceProbe>` | 一个 worker 一个采集器 | try_lock 单飞；只锁采集器，可 await，不锁 Core |
| Core 的 `last_glm_cycle: Option<u64>` | 本进程成功发布后更新，重启为空 | 轮次不落库；旧轮次不可重放刷新 lease |
| `GlmPolicyCommit` | catalog 原子更新 → worker；只在内部返回 | cycle_id、完整新 GLM permit map、changed_group/member ID 集合和安全计数；所有容量沿用 MAX_GROUPS/MAX_MEMBERSHIPS |
| `PolicyRefreshReport` | worker → 后续组装层 | cycle_id、preview_only、变化组/成员数、获许可/等待数；预览时为预计计数，本地已提交也不等于远端已收敛 |
| RoutingPermit 补 member 身份 | 策略发布层构造，planner 消费 | 增加 membership_id/member_revision，连同现有 source/group/priority/时限一起验证，不只凭相同 source/group/priority 复用旧许可 |

`RoutingPermit::from_policy` 调整为接收当前 WorkSnapshot 和现有优先级/时限参数，只复制已核验字段；`valid_for` 追加成员身份及版本比较。旧执行器测试仅调整构造夹具，不修改原行为断言。直接逐成员 `publish_route` 收窄为测试专用，生产路径只能批次发布；C-1 的生产许可集只包含 GLM，C-3 再扩展可信火山输入。

`ReconcileWorker::new(store, client, Arc<Config>) -> ControlResult<Arc<Self>>` 在构造时复用 Config::validate（只增加 crate 内可见性，不改校验语义），另核安全 priority 次序；错误转为固定 InvalidInput，不透传配置/URL 原文。配置在生命周期内不可变，原调用夹具据此更新。保留 dry_run：策略只返回 preview_only 报告，不保存组/目标、不发布许可；执行器所有远端写路径也在准备票据和发送前检查 dry_run，含创建/禁用，不能靠“没有许可”代替此检查。只读 quota/管理查询仍可用于预览。

## 5 C-1 函数细化

### C1 收集一致的 GLM 采集输入

`CatalogStore::glm_probe_inputs() -> ControlResult<Vec<ProbeInput>>`：usable 后开只读事务，按 SourceId 有界枚举 GLM 来源；排除暂停、退休和 retire_requested。来源视图与 SourceMaterial 从同一事务读取，以现有 ProbeInput::new 校验配置/类型，数量最多 MAX_SOURCES；零来源返回空列表。解码损坏返回 Storage，不跳过损坏行伪称完整采集。读完结束事务再返回临时凭据，绝不在事务内发 HTTP。

论证：单事务确保来源版本和材料同代；去重身份及有界遍历保证一源一个输入、必终止。唯一副作用是短暂持有读事务/私有内存，调用方消费后释放凭据。

### C2 单飞采集并提交一轮

`ReconcileWorker::refresh_glm_policy() -> ControlResult<PolicyRefreshReport>`：

1. try_acquire 既有 commands 容量并 try_lock probe；占用即 QueueFull，不排无界后台任务。
2. Core 锁内调用 C1，随后释放 Core；调用已验证的 SourceProbe::collect_cycle，按其单源超时/Unavailable/取消编号约定执行。取消不发布半轮，已有许可不延期，最终按原时限失效。
3. 整轮完成后，dry_run 分支只在读事务调用 C3 生成状态/目标预览，获真实许可计数为 0，结束事务后返回，不调用写入或许可发布、不写 last_glm_cycle。正常分支进入 worker.publish，复核 cycle_id 严格大于 last_glm_cycle。相同/更旧轮次返回 RevisionConflict，既不落库也不续许可。全失败也是完整新轮次，按未知处理。
4. 以当前 Core.backend、不可变 Config 和当前 UTC 毫秒调用 C3/C4 事务入口；期间没有 await。采集期间发生的暂停、成员变动或配置变动，以事务内重新读取的目录为准；旧来源 revision 的读数由原 GLM adapter 视作未知。
5. commit 成功后仅作无失败路径的整体 permit map 替换、last_glm_cycle 和 input_epoch 更新，最后恢复偶数 publication。返回安全 Report。发生错误保留原缓存；Store poisoned 则 gate 不重新打开。

不在本步新增周期任务。后续组装沿用原 poll_interval，慢轮次不堆积；当前 SourceProbe 仍串行，一轮上界为有限来源数乘原单源时限，不冒称任意来源规模都能 10 秒完成。

论证：网络等待不持写者锁，管理员暂停可先提交；结果落地重新读取身份/版本，故旧读数不能绕过暂停。Core 锁和 publication fence 串行化持久提交与完整内存替换；错误路径没有已发布的新许可。

### C3 当前快照计算和 Keep 适配

`compute_glm_round_tx(tx, cfg, cycle, now_ms) -> ControlResult<GlmRoundDraft>`：内部私有临时结果，不接收前端传入的选择/许可。

校验配置 priority 安全范围与 active>standby≥exhausted、时间安全转换、cycle 的正编号/非重复 SourceId，以及观测时间不晚于完成时间、完成时间不晚于 now_ms。来源/组/成员集合沿目录容量限制。无组是合法空结果，非法整轮拒绝。

在同一事务读取当前 GLM 组、完整 WorkSnapshot 和 StoredGroupPolicy；错误类型/缺必需行是 Storage。逐组调用原 decide_glm_group，不改变其输入筛选、未知、旧 active 保留、自动 pin_release 或阈值算法。所有组共享同一 ProbeCycle，不逐组再采集。

构造完整当前组映射：decision 中 Some(priority) 使用该值，None 或被原 adapter 排除的成员保持现有 desired_priority，不能填 0。自动可写集合只包含 managed 且未退休/退役的成员；未 managed 行和其 status/dirty/applied 全部保持不变，也不创建 operation_targets。它们仍保留在既有执行屏障的可能路由 peer 集中，不因不能自动编辑而假装不存在。

算法可以保留未获授权/未知的旧选择作为本地结果；该结果不自动获得实际路由许可。组未 ready 时也不能据此提升/启用。返回每组新状态、完整目标映射及可信观测引用，供 C4 在同一事务消费。

论证：当前目录只读一次后按组分割，身份和版本已由 catalog/adapter 校验；Keep 回填旧值使未知不引入零优先级。无管理授权不会被自动策略凭空增加。循环受组/成员/来源容量约束。

### C4 同事务保存状态和必要目标

`CatalogStore::commit_glm_round(cfg, cycle, backend, previous_permits, now_ms) -> ControlResult<GlmPolicyCommit>` 只由持有 Core 的 C2 调用；IMMEDIATE 事务内调用 C3、`save_group_policy_tx` 和新 `apply_policy_targets_tx`，最后一次 commit。previous_permits 只用于识别资格恢复，不作为授予新许可的依据；必须来自当前 Core，不接受外部输入。

`apply_policy_targets_tx(tx, group, complete_targets, now)`：先核目标集合、同组归属、当前版本及 managed 标记；不接收稀疏列表冒充全量。对每个自动可写成员比较 priority：相同则不写目标、不清 applied、不动 revision；不同才 checked 增 member_control.revision，更新 memberships.desired_priority、清该成员 applied_revision、标 dirty/安排重试。

变化成员沿用现有 SetGroupPriorities 的 GLM 状态约束：已完成/已消费的 Enabled 归 Preserve，未完成的显式 Enabled 保留；不修改 enable_epoch/enable_requested_revision/enable_consumed，不顺便解除 Disabled/退役。只将与变化成员相关且 phase 不为 succeeded/superseded 的旧操作按既有 superseded 规则处理；旧成功回执及 execution_attempts 不改。普通周期不新增 control_operations/operation_targets，也不增加 source.config_revision 或 group.revision。组策略变化仅由 B helper 增 policy_revision。

所有组共用这一事务：任一 SQL/版本溢出/坏行失败，整轮回滚。由 commit helper 处理未知提交毒化。提交前完成结果集合、完整许可 map 和 Report 的构造/校验；提交后不得再作可能失败的目录查询或单成员 publish。无持久变化仍可提交只读结果并发布新的合法 lease。

许可从缺失/失效变为当前有效时，在同一事务内唤醒该成员已有的 dirty、非退役、last_error_code=NotReady 待办：仅将未来 next_attempt_at 提前到 now，不清错误、不改目标版本/启用 epoch，也不重试终态错误。复用 wake_pending 的筛选原则做有界事务 helper，不在 commit 后另开事务。旧许可仍有效且目标未变时不重复 wake；保证首次恢复不会一直等到新 lease 到期才尝试，又不制造每轮调度写。

论证：状态和目标处于同一 IMMEDIATE 事务，排除“新选择配旧目标”。只对变化行提升版本，保持 no-op 和旧回执；不生成新的显式启用 epoch，所以普通切换不能重开 GLM 自动禁用渠道。安全计数及循环上界沿用既有类型约束。

### C5 构造并消费有界许可

`build_glm_permits(cfg, cycle, backend, final_work, decisions, now_ms)` 在 C4 提交前运行，输入是最终目标版本，而非更新前快照。必须同时满足：managed/已绑定、组 ready、未暂停/退休/退役、decision.eligible 内、同轮同 source revision 的 Available quota，以及 backend.ready_at。否则不产生该成员许可；未知/耗尽不是可用。

lease 只限制启用/晋升权限，不修改 GLM 纯决策或已经服务的原生状态。上限复用现有 30 秒观测预算：截止取来源 observed_at_ms 换算后的时限、backend.expires_at 与本次发布时限三者最早值；所有加法/秒毫秒转换受安全范围检查。到期或未来时间不续期，不用 completed_at 把一轮早期的旧观测伪装成刚采集。慢轮次可能只剩部分来源可获许可，应显示等待并重新采集，不补“健康”默认值。

RoutingPermit 绑定最终 membership/member/source/group revision 和期望 priority；实际发送仍需旧 observation 的 revision/TTL、publication gate、unknown-write 检查及 promotion_barrier。缓存观测不整轮清空：变化成员自然因 revision 失配而要求新 GET，未变成员可复用尚有效观测，避免额外逐渠道轮询。

论证：未知或过期输入没有正向授权，旧成员许可不能套到新目标；时限取交集使任一前提过期都停止新启用/晋升。禁止生产逐成员发布避免中间态；已经发出的写仍按 SGM-2 对账，不扩大保证。

## 6 C-2/C-3 的后续硬边界

- 管理命令需要 operation_id、规范载荷与 expected_revision；重放同一撤错不能再次提升 error_epoch。B 的内部方法不是直接挂 HTTP 的接口。命令 enum/存储约束/公开 schema 的追加迁移要在 C-2 单独确认，不修改冻结 DDL。会推进火山周期的管理动作还依赖 SGM-4 历史写接口；该依赖未实现时不得单独开放 Runtime 更新，不能因为加了幂等收据就丢弃 PeriodTransition，须与 C-3 合流启用。
- `/models` 按来源读取一次，成功同步该来源全部成员，失败保留存量；只由添加或显式同步触发。模型更新与旧许可失效须在一个受控发布中完成。C-1 不改此路径。
- C-3 的火山事务顺序必须为：可信事实检查点/归并 → B 的状态推进及 PeriodTransition 历史持久化 → 全部受影响组目标 → 同一 commit → 一次替换源 gate/许可。不能用 B 单独事务入口拼成多次提交，也不接受另一份统计账本。
- SGM-4 按最新观测契约验证上游错误分类、同源重试去重/成功 token 清零、换源隔离、迟到/乱序/缺页与撤错 fencing；不再要求最终结束证明。纯分类通过不代表请求去重进度和路由发布已持久接线。
- 火山未初始化、事实能力未验证或覆盖缺口时不得新增正向许可；“未知”不写成耗尽原因，也不等于人工暂停。已确定耗尽/暂停的安全停止和周期恢复的具体发布顺序随 C-3 合流验证。

## 7 测试与实施约束

2026-10-16 测试门禁修订：最大目录组装5秒、事务10秒、恢复10秒仍为独立性能预算，由串行release性能入口及CI显式检查；普通并行测试保留全部正确性/容量断言。历史小步受限并行验证不代表默认并行墙钟门槛稳健，见[修复说明](../../../fixes/sgm-fix-parallel-performance-gates.md)。

事务第一小步限定 C3 和 C4 的本地状态/目标部分，新增 `catalog/glm_round`，不提前建立 GlmPolicyCommit/真实许可。`GlmRoundDraft` 持有每个当前 GLM 组的 GroupView、StoredGroupPolicy、原 adapter 决策、完整 WorkSnapshot 和 PolicyTarget 列表；PolicyTarget 为 membership_id、expected_revision、已解析 Keep 的 priority。只在 catalog 内共享，不实现 Debug/前端反序列化或公开序列化。

`apply_glm_round_tx(tx,cfg,cycle,now_ms)` 内部先 compute，再调用组状态保存和目标应用，返回 `AppliedGlmTargets`（更新后的 draft、变化组/成员 ID）。返回不代表 commit 或发布；caller 必须先核 Store usable、持 IMMEDIATE 事务，任何 Err 回滚，成功后仍须 commit。本步仅测试包装提交；后续正式 commit_glm_round 要把许可构造/资格恢复唤醒放入同一事务，不能把此结果提前发布。

辅助契约：validate_round 复用原 Config 校验并核 priority、时刻/容量/来源唯一性；旧但正的 source revision/轮次观测交原 adapter 当未知。editable 同时要求 managed、已绑定、未暂停/退休/退役；其余目标必须 Keep。complete_targets 消费完整 decision 映射并补旧值，重复/陌生 ID 拒绝。apply_policy_targets_tx 独立调用时读取当前组并检查完整集合；整轮路径在同一 IMMEDIATE 事务复用 compute 已读取的本组快照，交模块私有 apply_current_targets_tx 执行同一份集合、revision、priority、Keep 校验和安全版本预计算。两路径都在目标写前完整核验，不把私有快照 helper 暴露成外部入口。其间只写独立组策略表/其他组成员及旧操作 phase，不改变本组输入成员、来源或启用消费状态；所有 operation_targets 保留，故 managed 身份也不改变。应用后回读实际变化成员的 memberships/member_control，取得最终字段和版本；不变的来源、组、模型、管理身份、执行状态复用同一事务输入。未来许可不能拿更新前成员版本。

采集整体验收时发现重复完整回读在全量负载下超出原 10 秒预算，因此收窄为 refresh_target_views_tx：只接收本组已核验快照及本模块实际变更 ID；按原 decoder/member_target 回读两张变化表，缺行为 Storage，任何错误仍整轮回滚。来源/组/配置、operation_targets、member_execution、execution_attempts 未被本轮改写；pending_writes 仅依赖执行记录及 channel_id/create_state（本轮亦不改），故其派生值可复用。返回完整 WorkSnapshot 与直接 load_work 逐字段交叉验证。没有新增跨事务缓存或镜像表，不放宽预算。

事务第一小步不做 C1/C2/C5、轮次重放门禁、NotReady 唤醒或远端 dry_run 门禁。只读 compute 支持 dry_run；低层写 helper 若 cfg.dry_run=true 直接 InvalidInput，供后续预览路径防误调用。ProbeCycle 只增加 cfg(test) 原始输入工厂，生产构造和算法不改。既有目录/组读取 helper 仅扩大 catalog 内可见性，Config::validate 仅扩大 crate 内可见性。

事前清单见 [C-1 expectations](../../../audits/sgm-3-source-policies/batch-c-expectations.md)。按事务适配 → 单写者批次发布/许可绑定 → 共享采集/组合验收三小步实现，各步测试和独审后推进。C-1 完成不代表整个 C 或生产部署完成。

第二小步以 `791bc56` 为基线，实现 C4 的正式 commit/恢复唤醒、C5 许可，以及 C2 中无网络的发布半段。`worker::publish_glm_cycle(cycle,now_ms)` 仅为 worker 内部入口，供后续共享采集调用；不增加接收外部观测/许可的管理 API。Core 仍是唯一写者，last_glm_cycle 只在成功提交并整体替换许可后推进；错误保留旧 map，未知提交保持 fence 关闭。完整 map 仅包含 GLM，火山不获许可。安全 `PolicyRefreshReport` 与 `GlmPolicyCommit` 一起在 catalog 提交前构造，由 worker 返回；报告仅含计数和轮次。只读 preview 复用 compute，报告预计变更及 0 个实际许可，不推进轮次。

本步同时接入不可变 Config 并复用 priority 校验；dry_run 在执行票据准备前和发送 guard 两处拒绝所有远端写，保留读取。来源采集输入、SourceProbe 单飞/取消和 refresh_glm_policy 的网络半段留第三小步；不新增假生产者、周期任务或运行 CLI 接线。许可时限使用秒级窗口，向下取整来源观测毫秒后加既有 30 秒预算，与 backend 到期和发布预算取最小；到期边界不含，不能靠完成时间续旧观测。计数、许可和待唤醒成员在 commit 前完成，提交后不再查询目录。

允许修改范围：catalog 内部事务 helper、worker 的策略刷新/批次发布及 dry_run 发送门禁、RoutingPermit 绑定、Config::validate 的可见性、对应测试与文档。禁止改原 GLM 纯算法、B 状态机、v1–v4 DDL、配置校验语义、管理认证/HTTP 语义、运行 CLI、现行面板或生产文件。不新增依赖或服务；如实现发现需要改业务/持久格式，先回到设计确认。

第三小步以 `8fa37e1` 为基线，完成 C1/C2 并组合验收 C-1。来源读取复用 store 的 decode_source_view/read_material，仅开放后者 catalog 内可见性；同一只读事务至多扫描 MAX_SOURCES+1 行，容量/必需控制行/材料损坏为 Storage，按 SourceId 返回可采集 GLM 来源，不依成员数量重复。读事务和 Core 均在请求开始前释放；MAX_SOURCES 输入组装预算 5 秒，不将其解释为整轮网络预算。

refresh_glm_policy 一次只占一个 commands 槽，并持 probe 单飞锁覆盖采集与发布；发布内部 helper 不二次占槽，原手工批次入口收窄为测试包装。采集中取消直接丢弃 Future，既有 SourceProbe 已消耗的轮次不复用，锁/槽由 RAII 释放；不 spawn 隐藏的后台采集任务。UTC 毫秒取值及 PolicyError 转换只产生固定安全错误码。网络完成后走同一发布 helper，以最新目录核验，不另建提交链路。

验证使用私有临时库、假凭据和随机回环服务，不执行 up/run/down/sync。每项 PBT 至少 128 cases，变异源码与 target 双隔离；运行全量预检期间禁止并行重编译同一 target。真实服务检查和部署须后续另行授权。

## 8 完整性自检

- [x] C-1 的目标、复用、类型所有权、容量和错误路径明确。
- [x] C1–C5 的前置、分支、失败/取消、callee 责任及正确性论证明确。
- [x] 数据库提交先于完整许可替换；未接管/no-op/旧授权/历史回执边界明确。
- [x] C-2/C-3 未细化内容不被本次确认伪装为可编码接口。
- [x] 用户已确认 C-1，按事务适配、发布/许可、采集/组合三小步实施。
- [x] 事务适配小步完成[自动验证、独立契约审和五维度复核](../../../audits/sgm-3-source-policies/c1-tx-audit.md)：新增 15 项测试（2 项各 128 cases PBT），8 项变异被捕获，全量 336 passed/1 ignored；未接运行服务。
- [x] 发布/许可小步完成[自动验证、独立契约审和五维度复核](../../../audits/sgm-3-source-policies/c1-publication-audit.md)：新增 13 项测试（2 项各 128 cases PBT），12 项变异被捕获，全量 349 passed/1 ignored；未接运行服务。
- [x] 采集与组合小步完成[验收](../../../audits/sgm-3-source-policies/c1-acquisition-audit.md)：新增 11 项测试（2 项各 128 cases PBT），最终 10 项变异被捕获，全量 360 passed/1 ignored。
- [x] C-1 自动验证、独立契约审核与五维度复核通过；C-2/C-3 和运行 CLI/生产部署仍待后续。
