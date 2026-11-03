# C-1 事务适配小步审核

实施基线 `fce5187`，用户确认后实施，2026-10-16 完成本步验收。本步只实现 C3 当前目录计算、Keep 适配，以及 C4 的组状态/必要目标同事务部分。没有 worker、共享采集、RoutingPermit 成员绑定或发布接线，不代表整个 C-1 完成。

## 实现与独立契约审核

依据 [C-1 细化及 §7 小步边界](../../design/source-groups-token-management/subplans/sgm-3-source-policies-c.md)、[事前 expectations](batch-c-expectations.md)。`sgm3_c1_tx_audit` 按 workflow §6.5 先读契约，再审基线 diff/新文件 diff；未读取主代理测试结论或报告，未运行服务或测试。

| 契约 | 核对 |
| --- | --- |
| 当前目录与原 GLM | 读取一致事务内目录，调用原 adapter，旧算法不改；所有组复用同一轮次 |
| Keep 与接管边界 | None/排除项保持旧值；未 managed、未绑定、暂停和退休/退役不可编辑；完整集合/版本在目标写前核验 |
| 最小目标变更 | 只递增实际变化成员；不新增操作/接管关联，不提升来源/组配置版本 |
| 旧授权与历史 | 已完成/消费 Enabled 归 Preserve；不重置 epoch/consumed；旧成功收据和执行记录保留 |
| 原子性 | 所有组共享 caller 的 IMMEDIATE 事务；Err 由 caller 回滚；返回只是事务内最终快照，不是提交/发布成功 |
| 无副作用扩展 | 只有内部 SQL；不改 DDL、GLM/火山算法、执行器、认证、运行 CLI 或面板 |

首审指出两项阻断：必需控制行缺失被传播成 NotFound，以及非空未知执行历史未覆盖。均已修正/补回归，复审核实关闭。读取成本最初为非阻断，但首轮全量暴露 10 秒预算不足；已减少重复读取并补跨组共享操作回归，增量独审确认未改变契约、无剩余阻断。详见[决策记录](c1-tx-decisions.md)。

## 自动验证

新增 15 项测试，含 2 项 PBT，各 128 cases：

- `c1_targets_match_independent_model_and_noop`：独立计算简单无临期/无 pin 场景的 Normal/Degraded、最低用量选择、目标与版本；随机多组共享、未知、managed、绑定、暂停和退役；未变行逐字段保留，二次执行 total_changes 不增加。
- `c1_every_write_failure_rolls_back_all_groups`：随机来源/组数量、选中成员、故障组/成员和四种 SQL 写位置；失败后所有相关表原样保留。此项只覆盖 C-P3 的 SQL 子集，不声称验证了尚未实现的许可发布。

确定性覆盖空目录、dry_run 只读/低层拒写、完整集合错误、未来/非法批次、缺必需行、两个版本耗尽、pin/降级/未知/退休历史、已消费/未完成 Enabled 与 Disabled、历史成功回执、非空 responded/uncertain 执行记录、提交不确定性毒化及重开。

规模 fixture 为 1000 来源、100 组、10000 成员，实际更新全部成员后再验证 no-op。原实现单模块 7906/9043 ms，全量负载下 14034 ms 触发原预算失败；优化后隔离复跑 5699/5371 ms，主工作树机械检查为 5934 ms，最终变异恢复复跑为 5874 ms。没有放宽 10 秒断言，最终全量回归中的该用例也通过；不作生产暂停响应时间承诺。

`bash scripts/contract_audit/sgm3_c1_tx.sh` 已通过：变更白名单、既有 helper/Config 只改可见性、ProbeCycle 仅测试工厂、C3/C4 标记、共享枚举/无网络/无许可边界、格式、公开 v1 JSON schema 样本及本步 15 项测试（32.06 秒）。clippy 返回 0，仅有原有 6 条警告；release 构建返回 0。

最终冻结代码运行 `cargo test --locked --offline -j 2 --quiet -- --test-threads=4`：336 passed、0 failed、1 ignored，295.02 秒，退出码 0；保留原有忽略用例。期间未重编译同一 target。全量结束后的变更仅为审核和进度文档，`git diff --check`、脚本语法和本步 rustfmt 检查通过。

### 定向变异

优化后的最终代码在独立源码副本和独立 target 中逐项变异；每项成功编译，指定测试因断言失败返回 101，均为 0 passed/1 failed，不以编译失败计捕获。每项结束即恢复源码。

| 变异 | 错误改动 | 捕获测试 |
| --- | --- | --- |
| M1 | Keep 默认填 0 | `c1_targets_match_independent_model_and_noop` |
| M2 | 相同 priority 仍提升版本 | 同上 |
| M3 | 忽略 managed 限制 | 同上 |
| M4 | 写目标时重置 enable_consumed | `round_enabled_consumption_disabled_and_success_receipts_are_preserved` |
| M5 | 将成功操作也置 superseded | 同上 |
| M6 | 保存组状态后提前提交、再开启事务 | `c1_every_write_failure_rolls_back_all_groups` |
| M7 | 必需行缺失返回 NotFound | `round_missing_required_control_rows_are_storage_errors_without_writes` |
| M8 | 写目标时删除 execution_attempts | `round_nonempty_unknown_execution_history_survives_target_change_and_reopen` |

全部恢复后，副本与主工作树的实现及测试逐文件比较一致；重新运行本步测试为 15 passed/0 failed，耗时 33.84 秒。早期优化前的变异结果不代替这次最终代码验证。

## 五维度复核与退出边界

- 一致性：内部计算/应用范围对齐 C3/C4；compute 可预览，写 helper 拒绝 dry_run；资格恢复唤醒和真实许可仍留后续。
- 风格：复用目录/组解码、B 的组状态事务 helper、原 GLM adapter、既有错误/枚举。没有新依赖、表或公开命令。
- 正确性：缺必需行映射修复有红绿回归；非空未知执行历史及消费者状态跨目标变更/重开保留；完整集合预检、版本耗尽、SQL 失败和未知提交均有测试。
- 性能：同一事务复用计算输入，独立 helper 仍完整核验，写后最终版本仍回读；减少重复 SQL 而不引入缓存/镜像表或大规模查询重构。
- 可维护性：新代码集中于 catalog/glm_round；复用 helper 只扩大必要的内部可见性。测试原始 cycle 工厂仅 cfg(test)，未开放生产构造器。

本步自动验证、独立契约审和五维度复核通过，无未解决阻断。当前没有运行期独立提交入口；未来正式 commit_glm_round 必须在同一事务补齐许可构造/调度唤醒，再由单写者一次发布。本步未访问生产配置/数据库/服务端口，未部署或重启 `run`。
