# C-1 批次发布小步审核

实施基线 `791bc56`，2026-10-16 完成本步验收。本步实现 C4 正式提交与恢复唤醒、C5 成员许可，以及 C2 无网络的发布半段；共享采集输入、单飞/取消和 refresh_glm_policy 的组合验证留下一小步。不接运行 CLI，不代表整个 C-1 完成。

## 契约与独立审核

依据 [C 方案](../../design/source-groups-token-management/subplans/sgm-3-source-policies-c.md)及[事前清单](batch-c-expectations.md)。审核者 `sgm3_c1_publication_audit` 按 workflow §6.5 先从契约抽取 expectations，再读取基线 diff 和新增文件 diff；不读取主代理测试结论，不运行测试或服务。

| 契约 | 实现与验证 |
| --- | --- |
| 提交先于发布 | Catalog 在同一 IMMEDIATE 事务完成状态/必要目标/最终许可/恢复唤醒和报告；worker 在 commit 成功后整体替换 map |
| 失败保护 | 普通 SQL 失败保留持久旧状态、缓存、成功轮次和 input epoch；真实 deferred-FK commit 失败触发 Store poisoned，发送 fence 不重新开放 |
| 成员与时限 | 许可绑定最终 membership/member/source/group revision；只给合格受管成员；截止取来源观测下取整秒加 30、预检和发布预算交集 |
| 稳定与恢复 | 无变化不增版本或操作；旧许可缺失/失效恢复时，只提前已有未来 NotReady 待办；保留观测和其他调度字段 |
| dry_run | 预览不提交/发布/推进轮次；准备票据前和实际发送 guard 双重拒写，包含创建和禁用 |
| 既有语义 | GLM 原算法、火山状态机、DDL、未知执行/启用消费/回读屏障和认证实现不改；旧测试仅适配构造签名 |

独审未发现实现阻断。三项验证建议均已补齐并复审关闭：实际批量恢复的性能证据、确定性成员身份/版本负向断言、新唤醒 UPDATE 失败的跨组回滚。详见[决策记录](c1-publication-decisions.md)。

## 自动验证

新增 13 项测试：catalog 5 项、worker 8 项。其中 2 项 PBT 各 128 cases：

- `publication_pbt_lease_intersection_and_member_scope` 用独立布尔公式和整数时间交集计算资格/截止，随机 managed、绑定、暂停/退役、组注册和时限；不调用许可生成器计算期望。固定用例另保证 member ID/revision 失配和来源时间过期必经授权分支。
- `policy_pbt_failed_rounds_never_publish_and_replays_do_not_renew` 随机组数、失败组和轮次；比较全字段持久状态、旧许可和轮次/epoch，验证失败全旧、成功整批替换、重放不续租。

确定性覆盖零成员、预检未就绪/未来、旧来源或轮次、全部未知/耗尽、lease 到期、恢复过滤、重开无许可、64 命令容量、组状态失败、恢复写失败和不确定提交。数据库写锁阻塞期间检查旧发送 gate 已关闭且 Core 不可观察；dry_run 五类远端写无执行票据，另直接准备合法 Disable 票据验证发送层独立拦截。

既有最大目录用例扩展为正式 commit，保留 1000 来源、100 组、10000 成员及原 10 秒门槛，不另加重复重型测试。首轮完整目标/许可为 6900 ms，稳定 no-op 后实际恢复 10000 个未来 NotReady 待办为 5319 ms；最终机械复跑分别为 6434/4854 ms，均满足各自 10 秒门槛，且还原允许变化的时间字段后逐表完全相同。

`bash scripts/contract_audit/sgm3_c1_publication.sh` 最终通过：文件白名单、C2–C5 标记、共享枚举、无网络/时钟边界、逐成员发布仅测试、dry_run 双门禁、成员绑定、格式及公开 v1 schema 样本。catalog 20 passed（45.55 秒，包含前一事务小步 15 项）、worker 8 passed（11.20 秒）。clippy 返回 0，仅保留原有 6 条警告；release 构建通过。

最终冻结代码运行 `cargo test --locked --offline -j 2 --quiet -- --test-threads=4`：349 passed、0 failed、1 ignored，330.86 秒，退出码 0；保留原有忽略用例，最大目录提交/恢复预算也通过。期间没有并行重编译同一 target；结束后仅同步文档。最终 diff/脚本语法/本步格式检查通过。

### 定向变异

在独立源码副本和独立 target 中执行以下 12 项变异，均成功编译，再由指定测试的断言失败捕获（每项 0 passed/1 failed、退出码 101）；不将编译错误计为捕获。每项结束恢复源码。最终副本与主工作树 src 逐文件比较一致；恢复复跑 catalog 5 passed（14.03 秒）、worker 8 passed（15.31 秒），退出码均为 0。

| 变异 | 错误改动 | 捕获测试 |
| --- | --- | --- |
| M1 | 删除 member revision 校验 | `publication_final_versions_history_preview_and_reopen` |
| M2 | 删除 membership ID 校验 | 同上 |
| M3 | 用整轮完成时间替代来源观测时间续期 | `publication_lease_boundaries_do_not_use_cycle_completion` |
| M4 | extend 许可集合，留下旧成员许可 | `policy_batch_replaces_all_scopes_noop_keeps_observations_and_replay_cannot_renew` |
| M5 | commit 前先清旧许可缓存 | `policy_sql_failure_keeps_entire_old_cache_and_unknown_commit_closes_gate` |
| M6 | 不拒绝重复/更旧轮次 | `policy_batch_replaces_all_scopes_noop_keeps_observations_and_replay_cannot_renew` |
| M7 | 有效旧许可每轮也唤醒待办 | `publication_wakes_only_recovered_not_ready_and_preserves_every_other_field` |
| M8 | 删除发送层 dry_run guard | `policy_dry_run_dispatch_guard_blocks_preexisting_ticket` |
| M9 | 删除准备票据前 dry_run 检查 | `policy_dry_run_preview_and_every_remote_write_path_are_blocked` |
| M10 | 给未知/不合格成员补健康许可 | `publication_stale_unknown_ineligible_and_future_observations_never_get_permits` |
| M11 | Store poisoned 仍重新开放发送 fence | `policy_sql_failure_keeps_entire_old_cache_and_unknown_commit_closes_gate` |
| M12 | 忽略恢复唤醒的 SQL 错误 | `policy_recovery_write_failure_rolls_back_earlier_targets_and_cache` |

## 五维度复核与范围

- 一致性：只实现已确认的发布半段，手工 ProbeCycle 夹具不被当作生产采集已接通的证据。
- 风格：复用原 Config 校验、Store commit/poison、worker publish/mutate 和 RoutingPermit；没有新依赖、表、服务或公开命令。
- 正确性：先提交后发布、整批替换、旧版本/lease 拒绝、失败回滚、未知提交关闭和双 dry_run 门禁均有独立断言。
- 性能：同事务复用已核验最终快照；按来源建立观测索引、按组建立 eligible 集，恢复只遍历有限成员；no-op 不清观测、不写逐成员状态。
- 可维护性：新增实现集中在 catalog/glm_publication 和 worker/policy，原执行器只增加配置/成员绑定/写门禁；下一小步复用同一提交入口，不新增第二套发布链路。

本步自动验证、独立契约审和五维度复核通过，无未关闭阻断或审核建议。未读取生产 config 或数据库，未访问运行端口或供应商，未执行 up/run/down/sync、重启或部署。
