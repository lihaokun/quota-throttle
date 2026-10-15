# 渠道协调器契约验收清单

子计划：sgm-2-channel-reconcile。日期：2026-10-15。状态：前两批、第三批计划器、持久日志、绑定/回读、HTTP worker、R8/R10 及多组组合轨迹已验证；不是整体验收通过记录。不能根据实现结果反向删掉验收项。

## 1 契约源

- [架构](../../design/source-groups-token-management/architecture.md)：G1/G2/G3/G6、单来源多成员、单渠道写入口、分布式契约及第二子计划共享类型。
- [sgm-2 细化](../../design/source-groups-token-management/subplans/sgm-2-channel-reconcile.md)：模块、P1–P16、v2 迁移、未知结果与提供者边界。
- [操作 schema](../../design/source-groups-token-management/schemas/control-operation.schema.json)：OperationView 的字段/枚举/类型。
- [sgm-1 修正记录](../../fixes/catalog-fix-capacity-and-pbt.md)：不能仅比较数量；输入按来源区分，直接检查内部敏感类型；保留全部 85 项原测试。
- [workflow](../../workflow.md)：§4.4/§5/§6 与附录 A。
- [认证重试修正记录](../../fixes/newapi-fix-healthy-auth-retry.md)：外部复审 F1 已在 3b1bbae 修复并验证；原先测试未覆盖的健康会话重复登录已有红绿回归。

## 2 Schema 字段

OperationView 必须且只包含 schema_version、operation_id、kind、target_type、target_id、phase、target_revision、total_targets、confirmed_targets、error_code、updated_at。类型和组合以 JSON schema 为准；ID 在 Rust 中继续验证 i64 正值，不用 JS number 表示实体身份。

跨字段语义不由 JSON Schema 假装完成：confirmed_targets≤total_targets；succeeded 必须存在全部目标的匹配代次确认（零目标本地操作除外）；重复 ACK 不增加确认数；后来改变目标不抹掉历史已成功操作的收据。只有实现真实输出参与 schema 校验才算通过，手写样例只用于设计检查。

CatalogSnapshot 的 JSON 版本保持 1；数据库依次迁移到 v2 意图表、v3 执行记录，版本常量不得混用。私有表不出现在普通快照，原始渠道正文、payload 摘要、key、认证头和服务端错误 message 不能进入 OperationView。

## 3 枚举值

| 类型 | 精确集合 | 机械检查 |
| --- | --- | --- |
| OperationKind | ensure_member, set_source_paused, retire_member, retire_source, set_group_priorities, sync_source_models | 一个 Rust 定义，输出与 schema/SQL CHECK 一致 |
| OperationPhase | queued, applying, waiting, partial, unknown, succeeded, failed, superseded | 同上；未知枚举拒绝，不默认成功 |
| TargetType | source, group, membership | 与 OperationKind 的合法组合一致 |
| StatusDirective | preserve, enabled, disabled | enabled=1，disabled=2，preserve 不发状态写；不和来源三态混用 |
| CreateState | never, may_have_been_sent, resolved | 不允许从未知结果无条件回到 never |
| RequestClass | control, observe, backfill | 同实例共用预算，控制优先 |

错误代码集合由操作 schema 固化。传输错误不能映射为供应商额度耗尽。已有 SourceKind、MembershipLifecycle、ModelDiscoveryAuth 继续复用，不能按不同模块再定义一套相似枚举。

## 4 流程步骤

实现使用 // Step Pn:，机械检查须定位到对应函数，不只检查文件里出现过字符串。

| 步骤 | 验收要求 |
| --- | --- |
| P1 | 请求路径/上限校验与有界排队 |
| P2 | 每次发送获取预算，auth 快照后释放状态锁 |
| P3 | 401 generation 单飞重登，第二次失败明确退出 |
| P4 | 按 ID 从第 1 页有界扫描，严格解析页字段 |
| P5 | 覆盖范围与完整性标记，不能首屏缺失即宣称不存在 |
| P6 | 新成员单组、禁用创建，凭据只在内部请求体 |
| P7 | status 专用接口及有限批量，回执不等于确认 |
| P8 | v1 校验/备份/v2 事务迁移，imported 不自动接管 |
| P9 | 幂等 ID 先于当前 revision 检查，同 ID 不改载荷 |
| P10 | 意图、成员目标/代次、操作目标关系同事务 |
| P11 | 全局源 gate、绑定状态、未知创建优先检查 |
| P12 | 降级/禁用先于提升/启用，GLM 保留原状态所有权 |
| P13 | 发出前重新核对；创建先持久记 may_have_been_sent |
| P14 | 回执后再次核对代次，不让旧 ACK 完成新操作 |
| P15 | 逐目标回读，保存唯一代次收据后才确认 |
| P16 | 重启恢复、暂时故障自动有界对账、无忙轮询 |

## 5 行为契约

- [ ] EnsureMember 创建 status=2；绑定持久化且最新 gate 允许后才启用；不调用 CopyChannel。
- [ ] 无匹配创建结果不再 POST，多匹配返回 ambiguous_create，不取首个/最新 ID。
- [ ] 绑定 ID 不迁移到别的 Source/Group；缺失/属性漂移不偷偷新建替代。
- [ ] 同源暂停/退役覆盖全部成员；移除成员只影响一个组；未知创建也不能在暂停之后自发启用。
- [ ] 只有全部应停渠道回读确认后才归档；保留历史映射及禁用的物理渠道。
- [ ] 同组完整 priority 计划先降后升，其他组 priority 不变；不更换请求模型。
- [ ] GLM 正常 preserve 与 auto_ban 保留；不是 quota 100% 就写 status=2，也不把 status=3 自动拉起。
- [ ] BackendReadiness 只读验证原生自动启用关闭；开启/未知时阻止接管，不自动更改全局 option 或 GLM auto_ban。
- [ ] 火山恢复前设 standby，保留其他组有效首选；不能因 resume 清掉耗尽原因。
- [ ] 模型成功发现才生成同步，失败不写空列表；模型字段更新保留其他原生字段。
- [ ] HTTP 200 缺 success=true、非 JSON 429、缺字段、缺失 ID 都不能伪装成功/空库。
- [ ] 相同 operation_id 同参数重放返回原收据，即使当前实体 revision 已推进；不同参数无副作用地拒绝。
- [ ] retry_operation 是显式恢复，不改变原命令；不恢复 superseded 意图，不重发 unknown 创建。
- [ ] v1→v2 保留来源/组/成员/secret/导入标记；新实例元 ID 只生成一次，旧 ID 不复用。

## 6 时序和状态契约

- [ ] SQLite 事务内不发网络请求；普通 HTTP 等待不持有 auth 状态锁。
- [ ] 身份/目标持久化先于发送；停止进程后重开不重复已可能发送的创建。
- [ ] Request/ACK 两侧均检查 revision；仅一侧检查不能过关。
- [ ] 新成员在来源暂停、退役及组未就绪时保持禁用；imported 未接管时不自动改旧路由。
- [ ] 降旧首选的 ACK/回读未完成前不提升新首选；恢复的 enable 不越过 priority 安全屏障。
- [ ] 暂时错误每轮最多三次，之后自动延期而非丢失 dirty；认证失败熔断，不反复打登录限流。
- [x] F1 局部修复已验证：未熔断的显式 retry 不登录、不改变认证代次；真正熔断才显式恢复，Token 不 root 登录；判断与恢复沿用单飞锁，取消/失败保持熔断，逻辑 64 槽与 HTTP 共享预算不绕过。新增 11 项测试含 128 cases PBT，4 项变异捕获；全量 288 passed/1 ignored。此项完成不代表整个 SGM-2 退出。
- [ ] 每个实际网络请求，包括登录/401 重放/重试，都计入共享预算；退避不占普通执行槽。
- [ ] 401 在等待登录前释放在途槽；两条并发 401 不会占满槽位使刷新死锁。
- [ ] unknown 与 partial 不混同成功；确认收据的历史和当前 generation 分开。
- [ ] NewAPI 不支持 CAS 的限制写入测试假设；不把本地拒收旧 ACK 当成远端迟到写已消失。

## 7 属性测试

每项至少 128 cases，使用独立状态机/集合/时间轨迹作为 oracle；生成不同来源/组的不同参数。共享管理 API fake 记录实际 method/path/body/发送次序，但诊断输出必须脱敏。

| 编号 | 不变量 | 检验方式 |
| --- | --- | --- |
| R1 | 重复操作不增实体/请求效果，不同载荷不改状态 | 随机 ID 重放、代次推进、成功后重放，对照独立操作收据模型 |
| R2 | 单组 priority 变更不写其他组 | 同源至少两个组、不同优先级和模型；比较全部实际请求及最终字段，不只比较数量 |
| R3 | 全局暂停/退役 gate 高于任何组启用意图 | 随机加入/暂停/恢复/退役/回执交错；检查每次启用的发起许可和最终静止状态；未决旧写单独标 unknown |
| R4 | 过期 ACK 不能修改新 generation 的 applied/终态 | 打乱重复 ACK 顺序、跨源/组错投；逐字段对比参考状态机 |
| R5 | 未知创建不重复 POST、不误认历史 | 在发送意图前后、响应前后、映射保存前后重启；0/1/多关联匹配；明确区分认证拒绝与可能产生副作用的请求 |
| R6 | partial 不可冒充全量暂停/退休成功 | 批量接口成功但遗漏 ID、changed=0、部分超时；确认集合按 ID/代次去重 |
| R7 | 提升/恢复不能越过组安全屏障 | 随机旧首选、禁用状态、降级失败及迟到回执；验证所有提升的前置证据 |
| R8 | 任意时间窗的发送次数不超过预算 | 虚拟时间、401/429/重试/多客户端共享桶、64 队列边界；事件轨迹独立计算许可上界 |
| R9 | GLM preserve 不写状态，火山规则不污染 GLM | 混合提供者、native status=3、相同模型名字；按类型及显式人工动作校验写集合 |
| R10 | 无敏感值进入 Debug/错误/OperationView | 直接检查所有新内部 wrapper；不同 key/header/指纹哨兵，不只格式化外层固定脱敏容器 |
| R11 | 迁移/恢复不改历史身份且不发送无授权请求 | 随机合法 v1 目录、回滚点、imported/retired 组合；比较完整元组与来源私有材料 |
| R12 | 原生自动恢复冲突不能被误判为 ready | 随机配置/读取失败/迟到原生状态变化；冲突时零 option 写入、零未经许可的接管动作 |

## 8 性能与流控

- [ ] 默认 1 请求/秒、burst=2；在途≤2 且背景≤1，发送队列上限 64，分页最多 101 页/10000 条，单批 status≤100。
- [ ] 总 HTTP 10 秒包含读正文；connect 3 秒；测试不用真实长 sleep 验证所有超时分支。
- [ ] 同源配额/模型探测不由渠道数重复触发；多个面板消费者共用后台结果。
- [ ] 新模型/priority 与远端一致时零 PUT；暂停回读可用批量观测，但不能降低逐 ID 确认要求。
- [ ] 不为每次管理命令复制全部源凭据；原始渠道 JSON 只保留单次安全更新所需窗口。
- [ ] 临时错误延期后自动恢复；等待只有具体队列/时钟/任务，不能 busy-loop 反复请求。

## 9 安全与副作用

- [ ] 不访问生产配置/数据库/服务，不使用真实 key；隔离 NewAPI 探针只管理自建进程。
- [ ] 目录迁移先核对 application_id/schema，再备份；外库/更高版本不修改。
- [ ] management 禁止未知重定向；源 API key 不经管理请求头或错误正文回显。
- [ ] 普通 token 签发和管理会话将分离；此步不生成管理员下游 token 作为临时替代。
- [ ] NewAPI 渠道物理删除、标签全量操作、CopyChannel 不在执行集合。
- [ ] 未纳管渠道不被改写；group/type/endpoint 漂移需要显式处理，不自动侵占。
- [ ] main 仅注册模块，未接线旧循环；orchestrator/quota 决策不改，正式新旧写者互斥在 sgm-7 验收。

## 10 跨实现一致性与变异门禁

- [ ] Rust/SQL/schema 枚举一致；实例目录 DB version=3 与 CatalogSnapshot wire version=1 的区别被测试。
- [ ] 保留 sgm-1 的 85 项 Rust 用例，必要的 v2 fixture 扩充不改变原期望；固定历史审计脚本不被“放宽”以冒充旧提交验证。
- [ ] 新机械脚本以本子计划基线 f9f02aa 校验真实输出、步骤和允许改动范围；旧核心回归与新契约同时通过。
- [ ] 原生 NewAPI 隔离测试验证实际禁用创建、完整分页、回读、双协议普通请求不受组内管理影响。
- [ ] 单独删除发出前/ACK 后 revision 检查、跳过回读、重发 unknown 创建、先 enable 再降级、GLM 强制 enable、首屏截断、绕过预算，均必须被对应测试捕获。

## Step 5 记录

用户已确认沿用本地托管架构，历史记录中的“范围待选择”不再是当前阻塞。本地预检复用 boot 配置/PID 和原管理预算，不增加远程支持、实例登记库或数据库 inode 机制。事前要求：真实进程/端口及版本检查，v0.0.0 仅冻结运行构建可映射，原生自动启用必须明确 false；失败不写 option、不启动/重启服务。worker 先撤销旧 ready、迟到检查不能覆盖新输入；超时和有期限许可必须验证。独立原生测试改为实际调用 producer，不能继续用手工 true 作为其完成证据。

执行器组合事前门禁：至少 128 cases 的真实 HTTP 多组随机命令/部分拒绝/重开/重试，与独立字段数组、成功收据历史及不可变归属核对；混合 GLM/火山同名模型不能交叉改写。另测未知创建的 0/多/漂移查找与最新暂停/退役、迟到启用 ACK 与新意图、一般未知写经匹配 GET/重开/重试仍未确认。所有计时驱动和故障仅限测试，不改生产参数、不篡改持久未决状态或延期时间。原生行为与真实预检继续单列，不因本组 PBT 通过而默认就绪。

R8/R10 补齐事前门禁：真实 HTTP 两身份共享桶的随机 401/登录/重放、429/冷却/重试轨迹，至少 128 cases，独立检查任意时间窗上界并与实际请求数量对照；既有并发、取消和 64 槽边界保留。私有 wrapper 以编译期负 Debug/Serialize 断言防误加能力，允许 Debug 的内部类型用不同字段哨兵直接检查；至少删除预算调用和误加 Debug/Serialize 的变异须被捕获。测试不接触生产，不能借此勾选真实预检或完整 SGM-2 退出项。

执行器审核门禁：成功外部发布推进 input_epoch，内部观察/结算不推进；旧错误不得覆盖新发布。Found/Unavailable 缓存绑定四重 revision 和 30 秒时效，缺失/漂移清旧正结果，显式非成功 retry 清缓存后重新观察。record_observed 仅更新诊断/排序字段，批次四重 revision 检查且失败整笔回滚，零 applied/收据副作用。due_members_with_stops 在 LIMIT 64 前排共享来源停用，actionable_blocker 复用组快照线性检查，不能逐 peer 重载完整组；无自身许可先延期，lane 忙不能结束后台任务。目录错误需保留原类型及重复来源 ID。新增回归、性质测试和独审关闭后才记录本小步通过。

HTTP 交付事前门禁：真实业务写区分明确未发出、可识别完整应用回执、未知；前置 GET 失败不记 PUT 未决，完整代理错误页不能伪造应用终结；429 冷却在头到达即共享，正文失败不丢 Retry-After。固定错误码与交付证据同时测试，成功回执不得直接设置 applied。公开诊断无请求/响应原文。

绑定/回读小步的事前门禁：只有完整唯一且新鲜的创建对象可登记 ID；已绑定 create 的效果与未知 HTTP 交付历史分离，不能推广为一般 unknown 写可被 GET 清除。确认须四重 revision、新鲜身份/目标字段和零未决写；逐目标首次收据、全部目标完成才 succeeded，最后退役成员后才源归档。延期/重试不复制操作、不重置启用授权或未知创建。验证覆盖实际 partial/unknown/failed/succeeded 投影、跨组/乱序/重复/重开 PBT 与定向变异；本步不冒充网络 worker 已上线。

持久执行小步的事前验收：v2→v3 保留完整目录、私有材料、实例 ID 及历史操作；失败回滚到迁移起始版本。prepared 重开后为 uncertain，不重复创建；完整响应仅终结尝试，不确认目标。重复/迟到回执仅结算原票，不能写 observed/applied/成功收据；新显式授权与普通目标更新分别验证。准备和结算均使旧观测代次失效。补至少 128 cases 的旧票/授权交错与崩溃恢复 PBT，并定向移除代次/授权/unknown 门禁确认测试有效。本小步不包含 worker、真实交付证据生产者或回读完成入口，这些仍是 SGM-2 退出条件。

P1–P10、P11/P12 纯计划、P13 准入门禁与持久准备、P14 日志结算/绑定、P15 回读确认、P16 持久重开/延期/显式重试，以及 HTTP 交付、worker 组合链路、R8/R10 和本地 BackendReadiness producer 已验证。来源策略、组权限与完整应用接线仍由后续子计划提供；不能用受控 fixture 的 RoutingPermit 宣称供应商策略已实现，worker 尚未接现行 up/run。

2026-10-15 设计检查：schema metaschema 与设计样例通过；8 个非法样例（未知阶段、过短操作 ID、非法实体 ID、动作/目标类型冲突、成功仍带错误、失败不带错误、零 revision、额外 credential 字段）均被拒绝；本地 Markdown 链接检查通过。未修改业务源码或生产服务。

补充实证：[原生恢复与暂停竞争](../../research/newapi-pause-recovery-race.md)的三个隔离场景通过，验证 R12 的前提确有必要。它验证的是固定原生版本的行为，不是尚未实现的 BackendReadiness/协调器已经通过 R12。

后续路径 A：单元/边界/PBT、实际 JSON Schema 校验、流程/枚举/副作用检查、隔离原生 API 实测及定向变异。路径 B：按 workflow §6.5 给独立 subagent 契约与 diff，不给实现位置列，处理全部阻断项再更新 audit-report/decisions。最终按 §5.2 做五维度复核。

第一批验证记录：120 个普通测试通过；另行运行的原生隔离测试通过。新增 5 项 PBT 各 128 cases，3 项定向变异均被捕获。P1–P7 机械检查、旧目录实际快照 schema、release 构建通过。独立审核及修正见 [审核报告](audit-report.md) 与 [处置](decisions.md)。R8/R10 的完整防退化门禁和 P8–P16 仍未完成，因此本清单整体不勾选通过。

第二批验证记录：141 个普通测试通过（含新增 21 项，其中 4 项 PBT 各 128 cases），3 项定向变异均被捕获；第二批机械检查、OperationView 实际输出及枚举/SQL/schema、旧快照 v1、release 构建通过。独审发现并修复“目标子行丢失仍可显示成功”：新增冻结 expected_targets 与读取/重开集合核对。详见[第二批审核](batch-2-audit.md)。当前尚欠 P11–P16、完整 ACK/permit/worker PBT 与第一批 R8/R10 防退化门禁，整体仍不勾选退出通过。

第三批计划器小步：164 个普通测试通过（新增 23 项，其中 8 项 PBT 各 128 cases），6 项定向变异被捕获；机械检查、旧真实输出 schema、release 通过。独审发现的迟到退款缺陷已消除，详见[当前审核](planner-audit.md)。当前未完成项以该报告“执行器接线前的硬约束”为准；完整 ACK/恢复状态机、真实预检及其余整体验收仍未完成，整体不勾选退出通过。

第三批持久日志小步：185 个普通测试通过（新增 21 项，其中 4 项 PBT 各 128 cases），6 项定向变异被捕获；机械检查、实际输出 schema、release 通过。独审的多票阶段聚合与迁移 oracle 两项已关闭，见[日志审核](execution-journal-audit.md)。P13 持久准备、P14 结算及 P16 prepared 重开已实现，但绑定、真实交付证据、回读确认、重试与 worker 未实现；完整 SGM-2 退出项继续保持未勾选。

第三批绑定/回读小步：209 个普通测试通过（新增 24 项，其中 5 项 PBT 各 128 cases），7 项定向变异被捕获；实际 partial/failed/succeeded 输出 schema、机械检查和 release 通过。扩展的原生隔离测试验证丢弃创建回执→重开→唯一绑定→真实 GET→退役确认；不等价于完整 worker 或断连网络状态机。独审 C1 唯一 prepared 槽冲突已关闭，重复谓词已集中，剩余组合覆盖纳入 worker 门禁，见[绑定/回读审核](confirmation-audit.md)。整体退出条件仍不勾选。

第三批 HTTP 交付与 worker 小步：240 个普通测试通过（新增 31 项：20 项 worker、8 项交付、3 项观察持久化；其中 3 项 PBT 各 128 cases），8 项定向变异全部被对应断言捕获。隔离原生测试扩展到实际 worker 禁用创建→绑定→回读→退役，单独运行通过；未注册组、无 permit、不启用、不发推理。机械检查、真实输出 schema、release 通过，clippy 仅 6 条原有警告。独审 W1–W8 和刷新 GET 延期残留已关闭，见[worker 审核](worker-audit.md)及[处置](decisions.md)。新增用例夹具的首次失败及修正保留记录，不以删断言通过。整体退出项仍未勾选。

R8/R10 补齐：245 个普通测试通过，新增 5 项（含真实 HTTP 轨迹及内部 Debug 两项各 128 cases PBT）；机械脚本显式运行原有 key/header/指纹、WorkTicket 和 OperationView 隐私门禁。6 项仓库变异被捕获，其中 4 项运行时断言失败，误加 Debug/Serialize 两项在编译时准确触发负 trait 断言 E0283；不是把任意编译失败当测试有效。release 通过，clippy 无新警告。本轮未修改生产行为，未重跑默认忽略的原生测试。独审[补齐报告](assurance-audit.md)通过；准入时刻与网络到达时间、有限随机轨迹与全状态证明的区别保留。实例预检范围尚待用户选择，整体仍未验收。

执行器组合轨迹：249 个普通测试通过（368.51 秒，另 1 项原生测试仍默认忽略且本轮未重跑），新增 4 项 PBT 各 128 cases；机械检查、release 和 diff 检查通过，clippy 无新警告。多组字段/归属、全部写方法目标集合、成功历史逐目标收据、部分失败/重开/重试、50→10 未完成降低、回读屏障、一般字段/状态未知写及未知创建的最新停止目标均取得组合证据。5 项独立临时副本变异被运行断言捕获。新增 fields 夹具曾遗漏 source revision 更新后的许可，修正前提且增加故障已消费断言，原 Unknown 断言及回归种子保留。详见[独审报告](scenarios-audit.md)和[处置](decisions.md)。这是对既有 R1–R7/R9/R11 的增量组合覆盖，不能替代旧迁移/错投回执/批量接口等门禁，也不证明真实预检或部署互斥。

本地预检实现：用户已确认沿用本地架构，历史范围阻塞撤回。本轮新增 11 项普通测试，其中 2 项 PBT 各 128 cases；最终全量 260 passed / 1 ignored（260.08 秒），忽略的原生测试单独 1/1 通过（52.11 秒），实际调用 producer/worker 刷新，包含自动恢复冲突和检查期间 PID 记录变化。6 项变异均被运行断言捕获，机械脚本、实际输出 schema、release 通过，clippy 无新警告。独审 F1 内部维护拒收合法预检已修复：复用 input_epoch，不新增计数器；保留外部输入与新检查的迟到隔离。见[本地预检审核](local-preflight-audit.md)。不读取生产配置/DB/key，不引入数据库指纹或远程注册；部署、来源策略和组权限仍不是本结果的证明范围。
