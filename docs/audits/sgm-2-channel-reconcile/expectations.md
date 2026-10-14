# 渠道协调器契约验收清单

子计划：sgm-2-channel-reconcile。日期：2026-10-15。状态：事前预期已冻结，第一批已验证；不是整体验收通过记录。不能根据实现结果反向删掉验收项。

## 1 契约源

- [架构](../../design/source-groups-token-management/architecture.md)：G1/G2/G3/G6、单来源多成员、单渠道写入口、分布式契约及第二子计划共享类型。
- [sgm-2 细化](../../design/source-groups-token-management/subplans/sgm-2-channel-reconcile.md)：模块、P1–P16、v2 迁移、未知结果与提供者边界。
- [操作 schema](../../design/source-groups-token-management/schemas/control-operation.schema.json)：OperationView 的字段/枚举/类型。
- [sgm-1 修正记录](../../fixes/catalog-fix-capacity-and-pbt.md)：不能仅比较数量；输入按来源区分，直接检查内部敏感类型；保留全部 85 项原测试。
- [workflow](../../workflow.md)：§4.4/§5/§6 与附录 A。

## 2 Schema 字段

OperationView 必须且只包含 schema_version、operation_id、kind、target_type、target_id、phase、target_revision、total_targets、confirmed_targets、error_code、updated_at。类型和组合以 JSON schema 为准；ID 在 Rust 中继续验证 i64 正值，不用 JS number 表示实体身份。

跨字段语义不由 JSON Schema 假装完成：confirmed_targets≤total_targets；succeeded 必须存在全部目标的匹配代次确认（零目标本地操作除外）；重复 ACK 不增加确认数；后来改变目标不抹掉历史已成功操作的收据。只有实现真实输出参与 schema 校验才算通过，手写样例只用于设计检查。

CatalogSnapshot 的 JSON 版本保持 1；数据库 schema 升到 2，两个常量不得混用。v2 私有表不出现在普通快照，原始渠道正文、payload 摘要、key、认证头和服务端错误 message 不能进入 OperationView。

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

- [ ] Rust/SQL/schema 枚举一致；实例目录 DB version=2 与 CatalogSnapshot wire version=1 的区别被测试。
- [ ] 保留 sgm-1 的 85 项 Rust 用例，必要的 v2 fixture 扩充不改变原期望；固定历史审计脚本不被“放宽”以冒充旧提交验证。
- [ ] 新机械脚本以本子计划基线 f9f02aa 校验真实输出、步骤和允许改动范围；旧核心回归与新契约同时通过。
- [ ] 原生 NewAPI 隔离测试验证实际禁用创建、完整分页、回读、双协议普通请求不受组内管理影响。
- [ ] 单独删除发出前/ACK 后 revision 检查、跳过回读、重发 unknown 创建、先 enable 再降级、GLM 强制 enable、首屏截断、绕过预算，均必须被对应测试捕获。

## Step 5 记录

第一批管理传输与渠道接口已验证。P8–P16、目录 v2 和持久操作尚未实现；不得将传输测试通过记为 sgm-2 整体完成。NativeRecoveryCheck 只是 BackendReadiness 的一个前提。

2026-10-15 设计检查：schema metaschema 与设计样例通过；8 个非法样例（未知阶段、过短操作 ID、非法实体 ID、动作/目标类型冲突、成功仍带错误、失败不带错误、零 revision、额外 credential 字段）均被拒绝；本地 Markdown 链接检查通过。未修改业务源码或生产服务。

补充实证：[原生恢复与暂停竞争](../../research/newapi-pause-recovery-race.md)的三个隔离场景通过，验证 R12 的前提确有必要。它验证的是固定原生版本的行为，不是尚未实现的 BackendReadiness/协调器已经通过 R12。

后续路径 A：单元/边界/PBT、实际 JSON Schema 校验、流程/枚举/副作用检查、隔离原生 API 实测及定向变异。路径 B：按 workflow §6.5 给独立 subagent 契约与 diff，不给实现位置列，处理全部阻断项再更新 audit-report/decisions。最终按 §5.2 做五维度复核。

第一批验证记录：120 个普通测试通过；另行运行的原生隔离测试通过。新增 5 项 PBT 各 128 cases，3 项定向变异均被捕获。P1–P7 机械检查、旧目录实际快照 schema、release 构建通过。独立审核及修正见 [审核报告](audit-report.md) 与 [处置](decisions.md)。R8/R10 的完整防退化门禁和 P8–P16 仍未完成，因此本清单整体不勾选通过。
