# SGM-3 B 存储审核决策

## R1 版本耗尽错误映射

执行新测试发现 `control::bump` 将 JSON_SAFE 耗尽映射为 InvalidInput，而已确认 B 存储契约规定 Storage。原函数属于旧控制接口，不能为了新模块改变其语义。

处置：来源配置、运行状态和组策略三个递增入口继续复用 bump，仅在本模块映射错误。保留原红测试，新增 source revision 耗尽断言。独审核对最终映射正确；未改旧控制层或期待值。

## C1 最终退休覆盖

独审指出原测试只设置 retire_requested，名称比覆盖范围宽。补最终来源/成员退休、重开、历史 Runtime 可读与初始化/更新拒绝。复核通过。

该合法退休 fixture 保留 retire_requested=1，因此不声称单独检验了 retired_at 为真、retire_requested 为假的组合。

## C2 配置和运行版本独立

独审建议补仅提高未触及的 token 上限的情形。新增测试断言 source revision 提升而 Runtime 完整不变，同时覆盖 source revision 耗尽时私有设置/运行状态回滚；复核通过。M6 定向变异证明测试会拒绝无关的 runtime revision 递增。

## C3 本轮范围与旧 fixture

基础迁移与固定目录重开已验证，不冒称完成复杂 GLM/火山、旧操作收据及执行历史随机组合 PBT B5；该门禁留下一小步。

原 v1/v2 fixture 从当前版本临时库退回历史版本，必须先删新增 v4 状态表，否则不是真实旧库，升级会撞表。只补这两处清理，冻结的 v1/v2/v3 DDL、原字段、原断言均保留。不是批量替换旧版本 fixture。
