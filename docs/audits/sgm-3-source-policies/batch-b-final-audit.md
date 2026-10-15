# SGM-3 B 组合验证与整体验收

日期：2026-10-16。本小步基线 `e0cfb82`，仅新增测试、机械脚本和文档，生产代码、依赖与运行入口零变更。组合验证、独立复审、隔离变异、机械门禁和最终全量回归均通过，SGM-3 B 完成验收；C 接线和生产发布不在此结论内。

## B5 迁移和重开保真

依据：[B 细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies-b.md)与[事前清单 §7](batch-b-expectations.md)。新增 4 项测试，其中 `b5_complex_history_migration_and_policy_reopen` 为 128 cases PBT，其余三项是确定性边界/故障矩阵。

每个随机样本同时包含 GLM/火山私有材料、两类多组共享来源、绑定/未绑定/退休成员、空组、导入收据、旧成功收据与后续操作，以及 responded/not_sent/uncertain 和可选 prepared 执行记录。比较全部 v1/v2/v3 业务表及 sqlite_sequence 的列名、行数、逐字段原始值，实例 ID 与版本另外核验，不只比较公开快照或计数。

重开恢复的参考只从旧表建立允许变化集：prepared 变 uncertain/transport_unknown、执行 sequence 增一、dirty/next_attempt 变化，以及对应未决操作投影。时间必须落在 open 前后区间内才归一化比较，其他字段保持不变；不调用生产恢复函数计算期望。第二次打开必须完全幂等。

迁移后用独立字段模型建立多来源 Runtime 与 GLM/火山组状态，覆盖不同日界、两类耗尽、撤错、暂停、未初始化、历史成员及保留游标。保存和两次重开均核全字段；历史周期远早于当前墙钟，open 仍不得清额度。新策略写入不改变旧控制/执行记录。

每例最后删除必需状态行或制造同类型跨组引用；open 必须失败，失败后再次比较全部表，不能顺便修补。确定性矩阵保证三类损坏均执行。另覆盖两处 v4 DDL 冲突、复杂 v3 的完整回滚和重试保留首次备份，以及 v1/v2 最后一步失败留在合法 v3、原版本备份仍在。

## 独立审核与五维度复核

`sgm3_b_history_audit` 按 workflow §6.5 先独立提取契约，再看本小步 diff；未读取主代理测试结论或报告，未运行测试/服务。首审无阻断项，非阻断建议已补充：中间 v3 的 core/control/execution 全核验，及 v2 主库/备份实例身份保留。增量复审确认闭环，认为 B5 五条细化覆盖完整；静态结论不代替自动验证结果。

- 一致性：对应 B5 和迁移失败边界；区分原样保留与既有崩溃恢复，不把 C/SGM-4 接线算作 B 的实现。
- 风格：复用既有私有 TempDir、目录创建/导入和控制接口；仅用测试 SQL 构造历史记录，不新增运行服务或依赖。
- 正确性：原始表映像、时间窗口和 Runtime/组状态参考相互独立；缺行/错组拒绝后还核验没有修补。
- 性能：生成规模有界，单次迁移预算 5 秒；128 cases 不提高生产轮询频率或增加后台任务。
- 可维护性：原始表映像集中于测试辅助函数，失败输出逐表/逐行定位；不修改旧生产算法或放宽旧测试断言。

测试夹具修正与编排问题记录于[决策记录](batch-b-final-decisions.md)。

## 自动验证与隔离变异

`bash scripts/contract_audit/sgm3_b_final.sh` 通过：B1–B10 标记、生产零 diff、共享枚举、格式、两个公开 v1 JSON schema 及样本输出；14 项纯状态测试和 19 项策略存储/组合测试全部通过。两个旧阶段脚本的历史基线不被改写。

`cargo clippy --locked --offline -j 2 --all-targets` 通过，只有原来的 6 条警告；`cargo build --release --locked --offline -j 2` 通过。固定最终测试产物后的 `cargo test --locked --offline -j 2 --quiet -- --test-threads=4` 为 **321 passed、0 failed、1 ignored**，440.88 秒。既有 ignored 原生 NewAPI 集成未启动；全量只获准使用随机回环假服务与临时材料。

变异副本 `/tmp/quota-throttle-sgm3-b-history-mutations.HnxoSZ` 与自身 target 独立于开发工作树。最新 4 项组合测试先跑通，再逐项注入/执行/恢复；6 项均为断言失败和 exit 101，不以编译失败充数。恢复后相关源码逐文件相同，4 项再次全部通过。失败种子只在临时副本保留。

| 变异 | 捕获证据 |
| --- | --- |
| M1 v4 迁移改写 GLM selector | B5 原始 sources 行不相等 |
| M2 删除旧确认收据的详细字段 | B5 operation_targets 行不相等；合法 JSON 不能掩盖内容丢失 |
| M3 prepared 恢复漏增 sequence | B5 member_execution 的实际 1 不等于期望 2 |
| M4 open 清空已保存用量/原因 | B5 Runtime 重开后不等于独立期望 |
| M5 跳过已保存成员归组校验 | 损坏矩阵发现跨组引用被错误接受 |
| M6 迁移把 GLM 默认档设为 Degraded | 默认组状态不等于约定的 Normal |

## B 整体退出核对

| 范围 | 对应验证 |
| --- | --- |
| B1–B7 纯函数与原因/周期/投影/轮转 | [纯函数审核](batch-b-pure-audit.md)：14 项测试，4 项各 128 cases PBT，8 项变异 |
| B8–B10 v4 形态/事务/核验/基本迁移 | [存储审核](batch-b-storage-audit.md)：15 项测试，2 项各 128 cases PBT，6 项变异 |
| B5 复杂历史与迁移组合 | 本轮 4 项测试，1 项 128 cases PBT，6 项变异及独立复审 |

B 合计 33 项测试、7 项 PBT 各至少 128 cases、20 项定向变异执行；这是三小步的汇总，部分变异检验同类风险，不声称 20 种互不重叠的缺陷。

事前十节清单、自动检查、三小步独立契约审核和五维度复核均闭环，无剩余 B 范围内阻断项。B 的完整退出条件满足。

未访问生产配置/数据库/服务端口，未重启、切换或部署 `run`。B 的本地策略与持久化验收不等于自动火山调度已上线；下一步是 C 的原子发布/协调器接线细化，真实终态生产者、统一事实账本和整体部署继续由 C/SGM-4/SGM-7 验收。
