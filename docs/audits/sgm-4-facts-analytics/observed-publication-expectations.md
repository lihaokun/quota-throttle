# 观测持久发布事前契约

日期：2026-10-17，基线 ce23dc7。从用户的观测型规则、成功 token 清计数及[O4细化](../../design/source-groups-token-management/subplans/sgm-4-observed-publication.md)提取。尚未实现，不从实现反推期待。

## 1 契约源

观测核心O1–O3、SGM-3 B/C-1、SGM-4 A/B、架构来源共享/独立组和workflow §5/6。保留GLM、NewAPI转发、不部署约束。

## 2 Schema

v6增加待处理引用队列、请求相位、周期过渡三表及来源时间索引；旧v1–v5不改。queue.sequence安全递增，native_log_id允许完整正i64；source和事实一致，request/source唯一，phase两值，历史边界与规则合法。公开wire保持1。

## 3 共享类型与上界

复用ObservationCandidate/RequestObservationState/OutcomeKind、Runtime/PeriodTransition、原组选择/许可；不重复计数或复制事实。RECENT_SECONDS沿B2的300，每源最多16队列记录，SQL页256、全轮统计1M行、10秒、lease≤30秒。

## 4 步骤

O4.1 同事实/首次归属事务入队；O4.2 当前目录/周期/去重/原状态机/历史原子推进；O4.3 同事务精确周期用量；O4.4 原组选择与必要目标/许可；O4.5 worker提交后按提供者scope发布。

## 5 行为

F/F/S/F计数1/2/0/1，已有耗尽成功只清计数；同请求跨成员重试一次，其他源成功不清本源，重放成功不清后来失败。旧历史补采只计量，不因恢复旧日志触发新健康或失败。迁移不回放已存在旧错误，不清旧状态。

## 6 时序

queue删除、request相位、Runtime、周期历史、全部组目标同事务；未知提交沿poisoned，内存只在确认提交后整体替换。period/error_epoch/cutoff与未来日志守卫不放宽。GLM/Volcano各自更新许可scope，不相互清空或续期。dry_run完全无写；未接管不隐式接管。

## 7 PBT

至少4组各128cases：完整迁移/重开、事件/重试/跨组/重启、失败回滚/重放、混合提供者许可与共享源组隔离。计量/计数用独立原始事件参考；不调用被测纯函数算期望。负例包含0 token、取消、不明错误、边界时间/ID、SQL及commit失败、过期/缺口/忙。

## 8 性能

索引keyset无前缀重扫，读量/写批/时间有界；无逐key新增请求、无全历史内存map。十万消费周期投影和最大目录16事件/源各独立串行release≤10秒；原性能预算保留，普通测试不改默认并发。

## 9 安全

仅开发worktree/私有临时库/假凭据与随机回环端口；不读生产配置/数据库/日志/key/进程环境，不访问供应商，不重启部署。无NewAPI补丁/推理代理/第二账本；私有request资料不进通用Debug/序列化或报告。

## 10 退出

实现前设计/十节、实现后机械范围/DDL/Step/清单及空/全ignored负例、至少8变异、全量debug/release、性能、clippy/build、独立契约审与五维度。原迁移夹具只去新侧表和更新最新版本断言，不削弱原记录比较。通过后仅称O4内部事务/worker接线完成，key/三页面/CLI发布另外验收。

2026-10-18 验证记录：本步退出门禁通过，具体结果和失败处置见[审核报告](observed-publication-audit.md)及[处置记录](observed-publication-decisions.md)。新增28入口、4组各128 cases PBT；debug/release各533 passed/18 ignored，16项独立性能、9类可编译变异、最终机械/构建/独审通过。序号CHECK不允许静默忽略，提供者scope包含退休成员；补充的中间断言独立验证重复成功不能误清计数。无部署。
