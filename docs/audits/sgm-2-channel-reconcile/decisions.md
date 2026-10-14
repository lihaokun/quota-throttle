# 渠道协调器审核处置

日期：2026-10-15。此记录覆盖前两批；SGM-2 未达到整体退出条件。

| 审核项 | 决策和状态 |
| --- | --- |
| 认证等待无界 | 已修复；共享逻辑 permit 覆盖等待认证/发送/在途，取消释放 |
| 静态用户头覆盖登录用户 | 已修复；会话 insert 覆盖，token 保留配置，真实 HTTP 回归 |
| 写回执解析失败未归类 unknown | 已修复；业务 POST/PUT 与 setup 共用分类，独立补审通过 |
| 64 槽容量口径 | 接受保守实现：64 个逻辑请求包含在途；实际发送仍最多 2、背景最多 1；细化已同步 |
| R8 随机发送轨迹覆盖 | 保留为整体退出门禁；本批预算 PBT 与固定 HTTP 并发/认证/429 用例不能冒充完整组合 PBT |
| R10 包装类型防退化 | 保留为整体退出门禁；当前无 Debug/Serialize 不等于测试能杀死误加派生的变异 |
| NativeRecoveryCheck 与完整 readiness | 分批边界：当前只检查自动启用；实例/版本/能力/策略许可后续合并，禁止将其直接当作 ready |
| 历史 sgm-1 机械脚本 | 不修改其固定基线以适配新功能；第一批脚本单独验证保护范围及旧快照真实 schema |
| 部署与生产开关冲突 | 未部署；不读取/修改生产开关、不重启服务。正式切换仍须单独确认 |

第一批验证命令：`cargo test --locked --offline`、`bash scripts/contract_audit/sgm2_batch1.sh`、`cargo build --release --locked --offline`。原生验收需显式设置 `QUOTA_THROTTLE_NEWAPI_TEST_BINARY` 后执行 `cargo test --locked --offline native_managed_channel_contract_on_fresh_database -- --ignored`，不得传入已有服务 URL 或生产数据库。

## 第二批处置

- 收据子行丢失可能伪装成功：已修复。增加私有 expected_targets、成员类型恰好一目标约束，读取/重开核对；独立复核及丢一行/全部行回归通过。
- 迁移随机空间偏 GLM legacy：已补混合提供者、多组、无渠道成员的确定性回归，不将局部随机测试夸大为所有场景的证明。
- R1 完整 ACK 状态机：仍属第三批门禁。本批只验证本地意图/幂等/代次/历史，不声称远端确认逻辑已实现。
- 历史脚本：sgm1.sh 与 sgm2_batch1.sh 保留其阶段基线，不为当前新模块放宽旧保护清单。当前第二批执行 sgm2_batch2.sh，覆盖新增 SQL/操作 schema，并保护第一批和运行策略模块。
- 第二批验证与变异详见[报告](batch-2-audit.md)。生产迁移、重启和切换仍未授权执行。
