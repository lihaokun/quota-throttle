# C-1 事务适配审核决策

## B1 缺必需行的错误分类

独审发现已有 membership 缺 member_control 时，复用 group_work → member_target 返回 NotFound，而 C-1 的内部损坏契约要求 Storage。先补测试重现：实际 NotFound，不满足预期 Storage；没有修改期待值。

处置：新模块的 required_group_work 在 caller 已确认组存在的前提下，将内部 NotFound 映射 Storage，三处读取共用。旧控制/执行错误语义不改；合法的不存在组查询仍由原 group_record 处理。回归覆盖 member_control/member_execution/source_control 缺失时 preview/apply 拒绝及全表不变。独立复审关闭 B1。

## B2 未知执行历史不应只比较空表

独审发现初始 fixture 的 execution_attempts 为空；CommitUnknown 测试不能代替 pending/unknown 远端历史。

处置：增加非空 responded/uncertain 两条执行记录和有区分性的 sequence/enable 消费状态，确认 priority 实际变化后 attempts、member_execution、operation_targets 逐字段不变；最终 WorkSnapshot 和重开后仍为 Unknown。独立复审关闭 B2。

## C1 重复读取成本

最初有完整目录校验及每组计算读取、目标 helper 写前读取、最终版本回读。独审作为非阻断指出成本；单模块两次为 7906/9043 ms，但首轮全量实测 14034 ms，超过原 10 秒断言。该轮 334 passed、1 failed、1 ignored，不计验收通过。

处置：不放宽阈值，整轮复用同一 IMMEDIATE 事务已核验的本组快照，模块私有 apply_current_targets_tx 保留同一份集合/revision/Keep/安全递增检查；独立调用入口仍重新读当前组，写后最终版本仍回读。不新增缓存、表或另一套解码。其间改变的是独立组状态、其他组成员和旧操作 phase，不改本组输入；operation_targets 保留，故 managed 身份不变。补跨组共享操作回归，独立复审确认无遗漏跨组依赖。

优化后隔离运行首次写入测得 5699/5371 ms，主工作树机械检查为 5934 ms，最终变异恢复复跑为 5874 ms，原 10 秒断言保留；最终全量结果见审核记录。后续接线仍需观察锁内耗时，不将最大容量测试预算宣称为生产响应承诺。

## 未纳入本步的门禁

C1/C2 共享采集与取消、C5 成员许可/过期、worker 的原子发布/资格恢复唤醒、远端 dry_run 拒写均未实施。本步不声称这些条件通过，也不以 SQL fixture 作为生产路由已切换的证明。
