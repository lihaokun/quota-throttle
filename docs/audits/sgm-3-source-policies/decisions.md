# SGM-3 第一批审核决策

## R1 过滤前校验来源成员唯一性

首审发现：同源同组的重复成员若暂停、未绑定或正退役，会在唯一性校验之前被过滤，导致非法输入返回成功。

处置：修复。按目录 `active_source_group` 索引的 `lifecycle != retired` 条件，在所有候选过滤前校验；已退休历史记录可与新成员并存。`duplicate_source_group_is_invalid_even_when_paused_unbound_or_retiring` 覆盖三个非法组合和退休历史合法组合。细化 P4、expectations §5 同步；独立复审确认闭环。

## R2 属性测试必须区分三种身份

首审发现：初始测试总令 SourceId 等于 MembershipId，跨组只改 GroupId，可能漏掉用成员身份查来源 quota 的错误。

处置：修复。A1 使用分离 ID 和随机来源旋转映射；A2/A3 为不同组分配独立成员和 channel ID；A2 验证共享来源 quota 从可用到耗尽在两个组的正确成员上生效，A3 独立计算成功来源对应的 eligible 成员集合。增加 M6 身份混用变异，A1 能捕获。独立复审确认闭环。

## C1 合法退休 fixture 对齐目录约束

复审建议：正向退休历史例子应同时填写 retired_at；容量测试的不同历史成员不能复用 channel_id。

处置：修复测试数据，给退休行填写 retired_at，容量样本使用唯一 channel_id。未改预期或业务断言。

## C2 上游时间差的表示边界

自审发现：原临期函数假设 reset_ms-now_ms 可表示；新增适配器的外部 quota 时间若极端异常可能导致减法溢出。

处置：只在新适配器检查 checked_sub，失败返回已有 InvalidInput，保持旧函数及合法输入语义不变；新增 `unrepresentable_upstream_reset_is_rejected_without_panicking`。细化与 expectations 已同步，独审核实边界处理。

本批不接受未解决的契约偏离。B/C 的遥测归并、持久化和原子发布继续按原计划细化，不因本批测试通过而省略。
