# 来源目录后续迁移的版本范围与增量核对

日期：2026-10-15。源码基线：`2c65f20`。对应外部审核 P1。本记录补齐 SGM-1 与 SGM-2 的契约承接，并只读核对受影响路径；不是重写历史审核、重跑测试或新增一轮完整独立 §6 验收。

## 版本归属

| 阶段 | 负责的契约 | 既有验证入口 |
| --- | --- | --- |
| SGM-1 | v1 目录/私有材料/旧配置导入；公开 CatalogSnapshot wire=1；不自动补损坏表 | [历史审核及 163f97b 局部修复](audit-report.md) |
| SGM-2 第二批 | v1→v2 备份、事务迁移、控制侧表和幂等操作 | [第二批审核](../sgm-2-channel-reconcile/batch-2-audit.md) |
| SGM-2 持久执行小步 | v2→v3、执行侧表、验证及 prepared 重开恢复 | [执行日志审核](../sgm-2-channel-reconcile/execution-journal-audit.md) |
| SGM-3 第一批 | 共享 quota 和 GLM 组适配，无数据库版本变更 | [第一批审核](../sgm-3-source-policies/audit-report.md) |

当前 DATABASE_SCHEMA_VERSION=3，但 CatalogSnapshot/OperationView wire 仍为 1。v2/v3 都由 SGM-2 引入，不属于 SGM-3 A；旧阶段文档中的版本号和验证数字必须按其历史范围阅读。

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
