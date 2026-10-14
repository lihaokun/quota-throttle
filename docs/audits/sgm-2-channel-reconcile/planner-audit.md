# 协调计划与发送门禁审核

日期：2026-10-15。基线：c71a9fc。范围：P11/P12 的纯计划、P13 的最终发送准入门禁，以及已 applied 的 GLM 启用目标继承规则。结论：当前小步已验证，第三批及整个 SGM-2 尚未完成。

## 独立审核与处置

独立审核按契约和限定 diff 执行，没有把输入类型或测试名称当作 worker/真实预检已接线的证明。修正后无本小步未解决的阻断项。

审核发现的限流错误已修复：先取得 token、后执行 guard、拒绝再退款，可能在 guard 暂停期间桶补满后重复返还额度，突破 burst。现改为在最终准入临界区检查认证熔断和业务 guard，guard 返回后重新取时间补桶，通过才扣额度；不存在退款路径。普通响应等待不持预算/认证锁，锁序为 Budget→auth，guard 禁止 IO、数据库、等待及预算重入。

新增长 guard 回归记录本地准入点时间，检查 true/false 长暂停后的所有子时间窗。它不是多线程 socket 发包窗口测试；本实现不宣称操作系统调度下的物理发包硬实时保证，也不宣称远端 CAS。

| 契约 | 结果 |
| --- | --- |
| 来源暂停/退役/策略停用覆盖启用，绑定漂移不写错资源 | ✅ gate、身份、代次和观测时效均进入纯计划 |
| 火山恢复先备用，再启用，再按组条件提升 | ✅ 完整轨迹及定向变异覆盖；不直接恢复旧高优先级 |
| GLM preserve 不拥有 native status | ✅ 属性测试推进字段更新后的下一步，仍不 Enable/Disable |
| 显式启用一次性 | ✅ 已 applied 后 preserve；有消费证据但回读再次禁用时不重启用；持久证据仍待执行器 |
| 完整 peer 集和真实降低/禁用屏障 | ✅ 缺失、重复、跨组、过期、身份漂移、未决已绑定写均不提升 |
| 未知创建只对账 | ✅ 完整扫描、精确名称、0/1/多匹配，未确认创建不重复 POST |
| 禁用创建不无谓阻塞旧组切换 | ✅ 无绑定 Never/MayHaveBeenSent 仍参与集合覆盖，按 disabled-only 前提豁免；Resolved 无绑定拒绝，已绑定 Unknown 继续阻挡 |
| 排队和 401 重放后的最终门禁 | ✅ 新鲜业务 guard 与 auth.failed 均在最终准入复核，拒绝无该次 HTTP 效果 |

## 测试证据

- 普通完整测试：164 通过、0 失败；既有原生隔离测试 1 项默认忽略。本小步不以普通测试替代完整协调器的原生验收。
- 新增 23 项测试：18 项计划器、4 项发送门禁、1 项目录继承回归；计划器含 8 项 PBT，各 128 cases。
- 独立字段矩阵逐项扰动 permit 的 source/group 身份、generation、priority、时间窗，以及 peer 的 source/member/group/execution generation、ID、group/type/endpoint/multi-key/观测时间；随机创建集合覆盖 0/1/多匹配、顺序、无关项、完整性与漂移。
- `bash scripts/contract_audit/sgm2_planner.sh` 通过：步骤定位、纯计划零 IO、共享枚举、无前端能力反序列化、保护模块 diff、旧目录及 OperationView 实际输出 schema。
- release 构建通过；Clippy 仅 6 个既有风格警告。没有改 GLM 选择算法、生产配置或现有运行进程。

变异只在独立临时源码与 target 执行，正常 worktree 未被变异：

| 故意错误 | 捕获测试 |
| --- | --- |
| 绕过最终准入 check | stale_intent_after_budget_wait_is_not_sent_or_charged |
| 忽略 peer 实际降低/禁用条件 | r7_barrier_matches_independent_peer_rule |
| 把 GLM preserve 变为 enabled | r9_glm_preserve_never_owns_native_status |
| 未知创建再次 POST | r5_maybe_sent_create_never_becomes_a_second_post |
| 火山恢复跳过 standby | volcano_restore_is_standby_then_enable_then_verified_promotion |
| guard 返回后不重新补桶/更新时间 | slow_guard_never_creates_late_refund_or_stale_admission_burst |

六项均被对应测试捕获。正常代码全量回归仍通过。

## 执行器接线前的硬约束

1. 真正的 BackendReadiness/策略许可生产者尚未实现；输入构造器不是预检证明，不能接收前端自报的许可布尔值。
2. 完整屏障集必须包含尚未确认停用的退役中成员，不能复用排除 retire_requested 的 priority 编辑集。
3. 无绑定创建豁免依赖恒禁用创建、先持久绑定再启用、无其他写者等前提；映射遗失/恢复无法证明这些条件时不得豁免。
4. enable_consumed 必须持久化并跨普通 priority/model revision 保留；已消费但未 applied 的 GLM 启用也不能被普通更新重新授权。只有新显式启用/恢复可重新授权。
5. execution revision 必须在发起写时推进，使同一意图内的旧 GET 失效；同组路由动作串行到回读屏障，或提供等价组级 fencing。
6. Store 提交与 fence 发布不能留旧票放行窗口；需要预先失效、提交后发布、失败重载。发送后仍须数据库代次核对与逐目标回读。
7. 持久执行记录、未知相反写的恢复、真实 WorkTicket、重试/熔断恢复及 worker 均未实现；不得把内存 bool 当作跨重启保障，也不得直接改写已经提交的 v2 DDL。

当前属性覆盖的是纯计划、局部轨迹和不一致输入，不是完整 R3/R5/R7/ACK 状态机。整个 SGM-2 的其余 PBT、变异、原生双协议与恢复验收，以及第一批 R8/R10 完整防退化待办继续保留。
