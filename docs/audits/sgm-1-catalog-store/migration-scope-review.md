# 来源目录后续迁移的版本范围与增量核对

日期：2026-10-15。原只读核对基线：`2c65f20`，对应外部审核 P1。前述历史核对补齐 SGM-1 与 SGM-2 的契约承接，不重写历史审核；文末另行登记 SGM-3 B 引入的 v4 及其独立验证记录。

## 版本归属

| 阶段 | 负责的契约 | 既有验证入口 |
| --- | --- | --- |
| SGM-1 | v1 目录/私有材料/旧配置导入；公开 CatalogSnapshot wire=1；不自动补损坏表 | [历史审核及 163f97b 局部修复](audit-report.md) |
| SGM-2 第二批 | v1→v2 备份、事务迁移、控制侧表和幂等操作 | [第二批审核](../sgm-2-channel-reconcile/batch-2-audit.md) |
| SGM-2 持久执行小步 | v2→v3、执行侧表、验证及 prepared 重开恢复 | [执行日志审核](../sgm-2-channel-reconcile/execution-journal-audit.md) |
| SGM-3 第一批 | 共享 quota 和 GLM 组适配，无数据库版本变更 | [第一批审核](../sgm-3-source-policies/audit-report.md) |

该历史基线 DATABASE_SCHEMA_VERSION=3，CatalogSnapshot/OperationView wire 为 1。v2/v3 都由 SGM-2 引入，不属于 SGM-3 A；旧阶段文档中的版本号和验证数字必须按其历史范围阅读。

## 受影响路径核对

| 路径 | 2c65f20 的实际行为 | 契约承接 |
| --- | --- | --- |
| CatalogStore::open | 先只读检查身份、版本、核心及对应侧表；合法旧库备份后顺序迁移，损坏库/外库/更高版本拒绝 | SGM-1 私有路径/身份保护继续成立；合法版本升级由 SGM-2 §4.2/§5.4 及 v3 补充覆盖 |
| migrate_v1 / migrate_v2 | 每一步单独事务提交；旧 v1 初始备份一次，旧 v2 升级前备份一次；第二步失败可保留 v2 与原始 v1 备份 | 不承诺两次升级整体原子回滚；新库按 v1→v2→v3 创建，无多余空库备份 |
| open 发布前验证与恢复 | verify_control、verify_execution 检查侧表；recover_executions 把 prepared 处理为 uncertain/dirty | 已有 v3 打开也不应描述成“绝无写入”；这是崩溃恢复，不是修补损坏表 |
| insert_source | 同事务写来源、私有凭据及 source_control | 共享来源唯一性与秘密不复制仍保留，追加控制初值由 SGM-2 负责 |
| insert_membership | 同事务写成员、member_control 和 member_execution，初值 preserve/dirty=false | 没有网络请求，建立本地映射不等于接管或启用渠道 |

迁移契约见 [SGM-2 细化](../../design/source-groups-token-management/subplans/sgm-2-channel-reconcile.md)。本次核对未发现表版本升级绕过既定设计：原“已有 v1 不修复性重建缺失表”与后续“校验完整的合法 v1 后执行版本迁移”不是同一动作。需要修正的是文档的版本范围与入口说明，而不是取消迁移。

## 验证证据与限制

当前测试源码已有：`migration_backups_preserve_v1_schema_and_are_never_replaced`、`migration_ddl_failure_rolls_back_metadata_and_preserves_backup`、`corrupted_v1_is_rejected_before_backup_or_migration`、`r11_v1_migration_preserves_complete_mapping_material_and_import_receipt`、`journal_v3_failure_preserves_v2_and_private_backup`、`journal_pbt_v2_migration_preserves_full_records`，以及 prepared 重开恢复测试。它们对应备份/回滚、损坏拒绝、材料保真与恢复边界，不以空表数量检查代替材料/身份断言。

SGM-2 的独立审核分别覆盖新增迁移和执行恢复；SGM-3 A 最终记录的 277 passed/1 ignored 为该代码基线的全量回归历史证据。本次只补文档和做只读代码对照，没有新增执行结果，也不宣称重做 SGM-1 全范围独审。

仍未接入生产运行服务。应用目录 schema 升级不等于旧 config 导入或 NewAPI 生产部署迁移；后两者的启用、备份与切换窗口仍归 SGM-7，需单独授权。

## SGM-3 B 的 v4 增量登记

存储小步实施基线 `0b52e50`。v4 由 SGM-3 B 引入，新增 volcano_runtime 和 group_policy_state；公开 CatalogSnapshot/OperationView wire 仍为 1。详见 [B 存储契约](../../design/source-groups-token-management/subplans/sgm-3-source-policies-b.md)及[存储审核](../sgm-3-source-policies/batch-b-storage-audit.md)。

| 路径 | v4 新增行为 | 保留边界 |
| --- | --- | --- |
| open / migrate_v3 | 原始 v3 先只读完整验证并生成私有备份；事务追加状态、回填占位/组初值，保留实例 ID | v1/v2 仍先走旧链，保留进入 open 时原版本备份；不声称整链原子回滚 |
| verify_schema / verify_policy | 继续验证 v1/v2/v3 业务 DDL，并核全部 v4 必需行、形态、策略一致性和组归属 | 损坏缺行拒绝；打开不读取当前时间清用量；原执行恢复链保留 |
| insert_source | 火山来源同事务追加未初始化行；GLM 不创建此行 | 不复制凭据或来源用量；缺行不被 initialize 自动修补 |
| insert_group | 同事务追加类型专属空选择状态 | 不是生产活动/pin 导入，不生成渠道许可 |
| insert_membership | 无 v4 来源运行行复制 | 同源多组共享一份 Runtime，独立保存组首选 |

v1/v2 历史测试 helper 仅追加删除 v4 表，使其继续构造真实旧库；冻结 DDL、旧字段与旧断言不变。存储小步验证基础迁移、重开与故障回滚；当时留待下一步的复杂历史随机组合 PBT B5 已于 2026-10-16 完成，见 [B 整体验收](../sgm-3-source-policies/batch-b-final-audit.md)。新证据覆盖旧业务表逐字段保真、prepared 恢复边界、原始备份、策略状态重开、损坏不修补及 v1/v2 最后一步失败的合法 v3/原备份保留；不改写 SGM-1/2 历史验收结论。

## SGM-4 B1 的 v5 增量登记

2026-10-17，实施基线 `9595837`。v5 由 SGM-4 B1 引入，追加 ledger_state、ledger_checkpoints、ledger_facts、ledger_minute_usage，CatalogSnapshot/OperationView wire 仍为 1。契约与实际验证分别见 [B1 细化](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b.md)和[B1 审核](../sgm-4-facts-analytics/batch-b1-audit.md)，不重写此前各版本历史通过数字。

| 路径 | v5 增量 | 保留边界 |
| --- | --- | --- |
| open / migrate_v4 | 原始 v4 完整只读核验和一致备份后，单事务新增账本及三个状态初值，保留实例 ID | 新库/旧 v1–v3 沿原链升级，保留原版本备份；不承诺整链原子回滚 |
| verify_schema / verify_ledger | 核全部旧业务 DDL及新表；现有 v5 在只读事务全事实流式验证、联合索引双流对照 | 缺表/行、坏 codec 或投影不符拒绝，不初始化修补；不自动补归属、清额度或启动采集 |
| open 发布与恢复 | ledger 由只读探测或迁移事务验证后，在同一独占 guard 下保持不变 | 原 prepared→uncertain 恢复链继续，不把旧未决写清空 |
| 来源/组/成员插入 | 不额外生成账本事实或复制来源计数 | 原 v4 控制/策略侧表责任不变；消费仅由显式入账事务写入 |

旧 v1/v2/v3 fixture 只先删除新增 v5 四表，再执行原降版步骤；两处成功 open 后的“当前版本”断言改用 DATABASE_SCHEMA_VERSION，备份及失败分支仍核原版本。新增 PB6 复用既有非空 B5 历史，对材料、成员、成功收据、未决执行和 v4 策略逐字段对照；恢复变化使用原独立期望，不只比较行数。应用库升级仍不等于生产迁移，服务切换授权归 SGM-7。
