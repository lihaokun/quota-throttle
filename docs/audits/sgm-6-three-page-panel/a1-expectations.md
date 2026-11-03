# SGM6 A1 事前契约清单

2026-10-19，基线`999ca41`。本清单在A1编码前由原架构/子计划独立提取，再根据已独审的函数细化锁定范围；事前预期不以代码倒推，最终验收单列在§10。

## 1 契约源

[架构](../../design/source-groups-token-management/architecture.md)、[A1细化](../../design/source-groups-token-management/subplans/sgm-6-panel-state-a1.md)、SGM1目录、SGM2控制、C1原GLM发布、C2局部操作、O4火山运行状态、SGM5 C就绪层次。独立审核先读这些契约再读实现，4项初审问题已修订，无新增产品选择。

## 2 Schema

新增PanelSnapshot wire1；旧Catalog/Operation/Token wire和DB v8不变。精确字段/nullable/秒毫秒语义以A1 §2及panel-snapshot.schema.json为准。源/组/成员子视图复用原定义；ID为字符串、cycle为u64字符串，所有公开整数JSON安全。只支持输出，不接受反序列化为许可。

## 3 枚举

复用SourceKind、MembershipLifecycle、StatusDirective、CreateState、ControlErrorCode与Regime原值；仅新增面板ObservationState四值unknown/observed/unavailable/stale，不当作新的来源业务状态。schema集合与实际序列化机械相等。

## 4 步骤

A1.1单事务安全目录；A1.2原已提交决策展示提取；A1.3原worker成功路径整批缓存；A1.4只读合成/失效；A1.5外部数字展示清理。代码Step注释逐项机械核验。

## 5 行为

来源共用状态、组策略版本/首选/固定、成员控制版本/退休意图可读；不把ready/dirty/applied当远端操作成功。具体未知操作留按需OperationView，不扫描历史执行表。保留历史身份，GLM不出现火山数据，火山不出现临期。周期只展示、不推进；没有模型/配额探测或NewAPI请求。

## 6 时序状态

目录同一只读事务，Core try_lock忙与poison分开；零写/零publication推进。只有原GLM成功commit后整体更新显示，失败/preview/重放/取消不更新。源以当前config_revision及原observed_at分类；group current必须满足完整stamp、评估自身年龄及相关来源样本的明确谓词，不能用completed_at续期。空组不能永远current，全失败不是零额度。

## 7 PBT不变量

完整stamp的集合域严格等于原group_work的全部未退休成员，而非目录全历史；暂停/请求退休/无channel仍在域内。当前成员退休使旧判定失效，退休后新判定可以current；已经退休且无当前引用的历史字段不使判定失效，目录历史不能因此删除。该边界先按RCA修正契约，再修实现/矩阵预期。

至少3组各128 cases：混合来源/控制/历史的独立白名单投影；来源版本/时间/失败/窗口数值分类；完整成员集合变化导致组失效，而无关observed/applied/重试字段不使判定过期。预期从输入fixture与契约独立构造，不能调用生产投影/stamp函数作为expected。基本回归覆盖提交/回滚/重开、读失败零副作用、周期越界和事件清空。

## 8 性能

最多1000来源/100组/10000成员，稳定排序，不截断。常规刷新不读execution_attempts、操作历史或来源凭据表，不依赖历史条数。最大目录各1短模型、无执行历史的独立release投影≤5秒；普通容量用例保留功能断言。保持原23项release及旧大组debug门禁，新预算独立串行，不承诺任意配置文本规模。

全量额外暴露旧导入/组快照计时遗漏，系统扫描还找到GLM纯容量一项，按独立批准[RCA](../../fixes/catalog-fix-import-performance-gate.md)保留所有功能/计时窗口/阈值并补齐隔离：最终release27入口、debug4入口（五个原预算）。此为测试编排修正，不改变本批事前性能目标或业务测试数量。

## 9 安全与副作用

上述最大性能夹具无GLM观察缓存，预算只约束该完整未知状态投影；有缓存的stamp/临期路径通过组合和PBT验证，不由此推导其最大规模耗时。

输出类型不含endpoint、headers、selector、secret_ref、fingerprint、credential、create_name、票据或供应商错误正文。已有私有设置只用于白名单提取，不能整体serde。持久坏值/缺行整体Storage，外部quota显示清理不能掩盖存储损坏。旧算法/DDL/依赖/HTTP/CLI不变；fermat只用于隔离测试，不部署、不读生产数据。

## 10 双路径与退出

设计独审通过后才编码；基本/边界/3组PBT、至少6类有效编译变异、schema/枚举/步骤/冻结范围、默认debug/release、独立性能、clippy/build及独审五维度。机械入口固定实际测试集合并拒绝空跑/全ignored，变异须编译后断言失败且还原。A1退出不等于HTTP/三页面/fermat部署完成。

Step 5实测（2026-10-19）：精确19入口（18普通/1性能）、3组各128 cases PBT、8类A1有效编译变异及恢复通过；另4类旧预算零预算变异分别捕获并恢复。最终默认debug/release各663 passed/0 failed/30 ignored，27项release性能与4个debug入口通过，A1最大目录103ms。schema/枚举/5步骤、核心冻结和三个旧容量case逐字对照、24项runner、clippy/build通过。初轮夹具/退休红例、两轮旧计时失败及恢复工具mtime问题完整保留于[audit](a1-audit.md)，不冒充本轮无失败过程。最终独立日志复核与五维度通过，0个未关闭实质问题。
