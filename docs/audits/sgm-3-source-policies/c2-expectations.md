# C2 管理命令事前契约

日期：2026-10-18，基线da35d8e。先于C2实现，从用户确认范围、已确认架构、C接线和[C2细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies-c2.md)提取。

## 1 契约源

管理组固定、火山撤错/编辑、来源models；来源跨组共享、GLM策略冻结、原执行器/回执/单写者复用。O4已提供周期历史，管理动作不能另开事务丢过渡。workflow §4.3/5/6要求设计论证、PBT及独审。

## 2 Schema

DB v7只替换meta和操作kind CHECK，旧DDL冻结；操作记录/targets/执行记录/账本保留，无长期新表。OperationView wire2，CatalogSnapshot wire1，新增四kind；局部命令Succeeded/0仅表示设置保存。迁移的父子表副本只能在同一事务内存在，FK始终开启。

## 3 枚举与类型

SetGroupPin、ClearVolcanoError、UpdateVolcanoPolicy、DiscoverSourceModels复用ControlCommand/OperationKind/OperationId/错误码。策略配置与runtime/组状态复用原类型；私有模型输入无Debug/serde。命令源/组及次级版本显式，未初始化Runtime仅Update+expected_runtime_revision=0合法。

新增wire固定为set_group_pin、clear_volcano_error、update_volcano_policy、discover_source_models，按上述Rust顺序一一对应；旧六kind和全部phase/error集合不变。

## 4 步骤

C2.0 保留记录迁移；C2.1规范化/幂等；C2.2局部状态+回执原子提交；C2.3发现输入/有限GET/版本复核+模型目标；C2.4提交后版本发布/原策略及执行链收敛。

## 5 行为

Pin只对合法GLM组及已管理成员，原quota释放规则仍生效；同值Pin不增加policy revision，实体group revision仍按新操作增加。撤错先推进到期周期，clear_error本身只消当前周期错误原因/计数，不清当前周期limit或人工暂停；同ID不得再次推进持久error_epoch或周期。设置成功与远端执行成功不同。发现只显式触发、每源一次、成功权威、失败不改模型，同ID提交成功后重放不得再请求。

## 6 时序

同事务配置/Runtime/历史/回执，幂等在版本变化之前判定，冲突不写。发现等待不持Core或事务，返回后核来源版本及退役意图；未知提交poisoned、旧许可按实体版本失效、不提前授新许可，其他组/provider不被顺便更新。dry_run新入口零网络/零写。

## 7 属性不变量

至少4组128 cases：独立命令/版本/重放模型、完整迁移/备份/重开、模型发现成功失败与规范化/请求次数、worker跨来源/组版本和旧结果隔离。参考不调用生产提交或策略helper计算期望；零目标局部成功不冒充渠道回读。

## 8 预算

沿原来源/组/成员容量、单writer、64命令槽；发现全局单槽、GET10秒、1MiB/1000模型，拒绝重定向，不加推理探测或周期poll。万条操作迁移≤5秒，独立串行release；全量普通测试默认并发，旧性能预算保留。

## 9 副作用

仅开发worktree、临时私有库、假模型端点/假NewAPI。不得读真实key/config/DB或请求供应商，不改NewAPI、服务进程、环境或生产数据。原GLM/火山算法和账本冻结；命令/报告/错误不得泄漏材料。

## 10 退出验证

命令/JSON schema/DDL版本、全Step、精确测试清单和空/全ignored负例、至少8编译变异、debug/release全量、性能、clippy/build；独审0 critical/unresolved及五维度。历史模板文件在当前仓库缺失，独审按workflow §6正文十节执行，不据此省略审查。只称C2内部管理操作完成，不称key/UI/部署完成。

跨语言实现对照N/A：本模块只有Rust实现；共享枚举、DB CHECK与JSON schema必须一致，业务性质以独立测试参考模型核验。

Step 5 验证：默认debug/release各555 passed/0 failed/19 ignored，最终定向各23 passed/1性能ignored；4组各128 cases PBT、10类有效编译变异、精确机械检查、clippy/release build和17项独立性能通过。新万条操作迁移563ms<5秒，独立复审0 critical/unresolved，五维度通过。完整范围、初次失败和复验见[C2审核](c2-audit.md)及[处置](c2-decisions.md)。仅C2内部能力退出，未扩大到key/UI/生产接线。
