# 火山观测型熔断事前契约

日期：2026-10-17，实施基线 `1872ab3`。从用户最新决定及[细化](../../design/source-groups-token-management/subplans/sgm-4-observed-breaker.md)事前抽取；O1–O3 已完成[本地验收](observed-breaker-audit.md)，下列期待保留。O4 持久/运行接线不提前勾选。

## 1 契约源

用户同意观测型熔断且成功有 token 流清空计数；沿用来源共享、三次防抖、人工撤销/日重置和独立 token 上限。新细化替代旧“必须证明最终失败”的门禁，其余架构和 B 状态机隔离保持。workflow §5/6 要求设计、属性、机械和独审。

## 2 类型

Candidate 的 source/request/occurred_at/kind；State 的 source/request/first_event_at/phase；Decision 的 next/outcome；私有带请求标识类型没有 Debug/Display/serde。OutcomeObservation 的 first_event_at/observed_at/kind 代替旧内部 FinalOutcome/completed_at，无 wire 或 schema 变化。

## 3 枚举与容量

复用 OutcomeKind、FactKind、StreamObservation、FactError、JSON_SAFE/MAX_REQUEST_ID_BYTES/MAX_POLICY_OUTCOMES。RequestObservationPhase 只有 Failed/TokenFlow；absence 为尚无观测。排除 code 和 403 code 表各有单一实现，不从错误正文猜类型。

## 4 步骤

O1 规范校验→已知身份→正 token/错误白名单分类；O2 数字/身份校验→最早时刻→单向状态转移→有界结果；O3 既有代次/周期/游标验证→计量→观测顺序应用，成功清零、锁存不解除。

## 5 行为

成功 token 活动清同源失败计数，非流式也适用；有消费但 0 token、坏元数据、未知流式状态和管理/参数错误不是健康证据。流式 Done/Eof/ClientGone 的正 token 活动可清计数，但不宣称请求完整成功。默认三个不同请求的明确失败观测熔断；同 request/source 多次重试只算一次。跨组共享、跨源隔离。

## 6 时序

同请求失败后成功允许输出 Success；成功后迟到错误不输出；重复成功不再次清除后续新失败。成功始终清有效计数但不解除 error_exhausted/limit_exhausted。旧代次、周期和 cutoff 不被新分类绕过。纯合并不自行存储或宣称重启去重成立；后续 O4 必须同事务发布请求进度和 Runtime/目标。

## 7 PBT

每组≥128 cases：分类的独立表格参考、按 source/request 的随机重复/乱序、串接真实 Runtime 的失败计数与锁存模型。含 F/F/S/F、F/F/F/S、同请求 F/F/S、S/F/replay-S、跨组同源、跨源同 request，以及零 token/未知消费不清。任何错误保留输入，不从实际返回值反推期望。

## 8 性能

单事实有界校验、最多64字节request复制、常数次合并；不新增全历史扫描/全局内存 cache。独立串行release十万次分类合并≤5秒，纳入原性能runner；原有性能门槛保留。

## 9 安全与副作用

仅开发worktree；不访问生产配置、数据、服务或供应商。无新增依赖/DDL/网络/后台任务/推理代理；不改 NewAPI、不改 GLM。真实流式首 chunk 不可从日志实时监听，不作该承诺。源类型由可信catalog保证，不从协议/模型推断。

## 10 一致性与退出

当前规范更新与历史审核分开，保留此前红例。类型调用链只机械更名、不改原测试预期；新测试覆盖行为调整。精确入口、空/全 ignored负例、至少6种可编译变异、debug/release全量、独立性能、clippy/build、独审与五维度检查完成后才称O1–O3验收；整个功能和O4不能由本步替代。
