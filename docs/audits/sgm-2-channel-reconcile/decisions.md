# 渠道协调器审核处置

日期：2026-10-15。此记录覆盖前两批、第三批计划器、持久日志与绑定/回读；SGM-2 未达到整体退出条件。

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

## 计划器与发送门禁处置

- 迟到退款可能突破 burst：已删除退款路径，最终检查后重新取时间补桶再扣额度；长 guard 准入轨迹回归通过，不声称物理发包硬实时。
- permit/peer 判定测试不足：补独立字段矩阵和随机唯一创建集合 PBT，GLM preserve 属性推进到字段更新后的下一步。
- 未知禁用创建是否阻挡其他来源：按恒 status=2、先绑定再启用及单写者前提给予无绑定 Never/May 豁免，但保留集合/身份检查；已绑定 unknown 不豁免。
- 跨普通 revision 的启用消费、退役中 peer、execution revision 与 fence 发布：明确为持久执行器接线前硬约束，尚未宣称实现。
- 验证与剩余范围见[当前审核](planner-audit.md)。本小步仅注册纯计划模块，旧运行循环不变。

## 持久日志处置

- W1 多票/多成员阶段误报：按整项操作的目标汇总，uncertain 优先 unknown，其次 prepared→applying；其他成员 ACK 不遮盖未决状态。跨成员、prepared 补偿回归及多票乱序 PBT 覆盖，独审复核关闭。
- W2 私有材料迁移 oracle 不全：逐 SourceId 比较全部 SourceSettings 与 credential，包含 GLM 不同模型、多 header，以及火山模型/日上限/重置字段；保留公共快照、实例 ID 和历史操作断言，独审复核关闭。
- 迁移版本：冻结 v1/v2 DDL，追加 v3。旧 v1 fixture 先移除新增 v3 表才能构造真实 v1；旧“当前数据库版本”断言改用 DATABASE_SCHEMA_VERSION，wire=1 等业务断言不变。历史阶段脚本未放宽，新增 sgm2_execution.sh。
- 迁移失败按步骤回滚：v1→v2 成功后 v3 失败保留 v2 和初始 v1 备份，不声称整条链原子回滚。
- 本小步没有网络发送、绑定和回读完成入口；实际 DeliveryEvidence 生产者与 worker 继续列为硬性未完成项。详见[独立审核](execution-journal-audit.md)。

## 绑定与回读处置

- C1 已物化 prepared 创建阻挡下一请求：绑定事务将其转 uncertain，并以 Resolved+绑定证明效果完成；不伪造 HTTP 响应、不修改唯一 prepared 索引。直接 bind→Enable、矛盾 NotSent 回滚、迟到回执与重开回归通过，独审复核关闭。
- W1 重复安全条件：pending 的创建豁免集中为一个 SQL 常量，终端错误仅由一个 Rust 分类函数定义。没有增加通用工作流框架，独审关闭。
- W2 测试边界：补实际代次/写/重试性质、消费授权 tuple、future/占用 ID/绑定回滚和 GLM 消费写失败回滚。更广的多成员 partial→再写→延期/重开网络组合继续作为 worker 阶段 R3–R7 门禁，不用本步各局部测试替代完整组合证明。
- 原生构建版本：固定测试二进制的内嵌 module/vcs 确认为 rc.20/6ce7305、modified=false，但 `/api/status` 返回 v0.0.0。测试在启动前校验冻结 SHA-256，再使用测试已核实的 canonical version；生产预检规则未放宽，此差异交由真实 producer 显式处理。
- 当前没有自动 worker 或生产接线。代码、测试与独审见[绑定/回读审核](confirmation-audit.md)，正常服务未被重启或修改。
