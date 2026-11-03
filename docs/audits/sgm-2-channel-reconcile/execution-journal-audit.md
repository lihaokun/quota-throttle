# 持久执行日志独立契约审核

日期：2026-10-15。基线：`7c5efcc`。范围：SGM-2 的 v3 执行日志、准备与结算、启用授权、重开恢复；不是整个协调器验收。

结论：本小步独立代码审核通过，0 个 critical/major、0 个未解决发现。准备与结算不生成远端确认，旧授权与新显式授权分离，未知创建保持不可重复发送。下列两项 warning 已修正并重新对照代码和测试源码；完整 SGM-2 仍未完成。

## 审核依据与独立期待

按 `docs/workflow.md` §6.5 路径 B，先读取持久执行细化、架构第二子计划补充、expectations §1–10，再审基线 diff 及三个新增文件的 `/dev/null` diff。仓库缺少独立审核提示模板，采用 §6.5 的等价步骤。未以主实现者的测试通过记录替代契约判断。

从契约独立抽出的期待及对照结果如下。

| 独立期待 | 代码对照 | 结论 |
| --- | --- | --- |
| P13 四重代次一致且重新计划后才准备；创建意图提交先于票据发布 | `execution.rs` 的 `prepare_execution` 在 IMMEDIATE 事务内加载最新快照、比较 member/source/group/execution、重新计划、写 prepared/May，提交后返回票据 | ✅ |
| 未决尝试不能因新意图丢失，unknown 创建不能重新 POST | 新命令不删除执行日志；`load_work` 从日志计算 InFlight/Unknown；只有明确 NotSent、无其他可能创建且未绑定时回到 Never | ✅ |
| P14 票据匹配实例、成员、尝试、目标代次、授权和效果；重复结算无新效果 | `settle_execution` 对照持久行，重复同结果返回，冲突终态拒绝；迟到证据只终结原尝试 | ✅ |
| ACK 不等于成功收据；旧结果不写新目标 observed/applied/终态 | 本模块不写 observed/applied/operation_targets 确认列，不生成 succeeded；阶段修改受目标三重代次检查 | ✅，阶段聚合见 W1 |
| 准备、结算、崩溃恢复使旧观测失效 | 三条路径均推进 execution；重复结算和无 prepared 的再次重开不推进 | ✅ |
| 只有显式 enabled/来源恢复新增授权；普通 priority/models 保留消费证据 | `control.rs` 在显式操作下增 epoch；迟到 Enable ACK 仅消费同 epoch；planner 用授权起始 revision 保留 GLM 普通更新的 preserve | ✅ |
| v2→v3 不改核心身份、材料、历史操作和实例 ID，失败回滚 | `store.rs` 先验证/备份，迁移事务只重建 meta 并追加执行表；冻结 v1/v2 DDL，wire 仍为 1 | ✅，测试覆盖见 W2 |
| prepared 重开按 uncertain，不清暂停或创建关联 | `recover_executions` 在发布 Store 前同事务恢复 dirty/unknown，保留原意图和映射 | ✅ |
| 组屏障使用完整可能路由集合，含退役中成员 | `load_group_work` 排除已退休但保留退役请求；提升/启用前逐项比对数据库中的 peer 身份、代次及未决状态 | ✅ |
| 不泄露凭据，不影响现行 GLM 决策或运行服务 | Effect 为固定白名单；Ticket/Effect 的 Debug 固定脱敏；无 HTTP/启动入口调用，旧 orchestrator/quota 未改 | ✅ |

## 发现与处置

### W1 操作阶段聚合已修正

初始 diff 的 `settle_execution` 依据单票 outcome 直接写 waiting/unknown。同一目标产生多个补偿尝试，或一次来源操作包含多个成员时，另一张成功回执可能遮盖仍然 uncertain 的尝试。另一种顺序是旧票终结时新票仍 prepared，操作不应降为 waiting。

这不会直接绕过 `pending_write` 的安全门禁或产生 succeeded，但会使 OperationView 误报协调阶段。现有 `execution.rs:323` 的 `mark_operations` 已按操作全部目标聚合，优先级为 uncertain→unknown、prepared→applying、其余阶段。`execution_tests.rs:434` 的跨成员回归、`:488` 的 prepared 补偿回归及多票乱序 PBT 分别检查实际 phase/pending，而非只数确认行。✅ 已关闭。

### W2 迁移属性测试已比较完整私有材料

初始 `journal_pbt_v2_migration_preserves_full_records` 仅比较部分私有字段，不能支撑完整材料保真的测试声明。现有测试已保存每个 SourceId 的全部 SourceSettings JSON 值与 credential，迁移后逐源等值比较，并构造 GLM 各源不同模型及多 header、Volcano 模型/日上限/重置参数。这补足了原 oracle 的盲区；公共快照、实例 ID、历史操作和私有备份断言仍保留。✅ 已关闭，未发现迁移 SQL 实际篡改材料。

### 非阻断边界

v1→v2 与 v2→v3 是依次提交的迁移。第二步失败时当前库保留已完成的 v2，初始 v1 备份仍可恢复；不能把这个序列描述为一次跨两版本的原子回滚。v2→v3 本步的失败回滚与私有备份契约成立。

## 属性测试与设计复杂度

新增属性测试使用 128 cases。授权测试有独立 epoch/consumed 状态；多票结算测试用独立 resolved 集合判断残留 Unknown；崩溃测试包含暂停/退役与多次重开。它们比只比较行数更强，基本覆盖本小步的持久日志风险。旧票跨目录拒绝、四个准备代次、冲突终态、写入回滚及未知创建的确定性测试补充了边缘条件。

这些测试尚不等于完整 R3–R7：真实发出边界、跨 source/group 的网络错投、完整绑定/确认/暂停归档及组级并发 fence 仍依赖后续 worker。PBT 不能仅凭数量标记整项完成。

两张追加表、独立授权 epoch 和终止证据状态均对应明确失败模式，没有引入额外 provider 策略或第二套控制器。execution 同时作观测代次，因此允许序号间隙，不应被 UI 当尝试计数。当前未见需要删除的冗余抽象。

## 尚未验收的 SGM-2 工作

`bind_created`、`confirm_member`、真实 HTTP 交付证据生产者、重试入口、worker/fence 发布与回读调度尚未实现。当前公开类型及测试构造器不能证明这些生产者已接线。只有后续补齐这些实现、相应 PBT/隔离原生证据及独立审核，才能退出完整 SGM-2；本报告不授权部署或重启现行服务。

## 路径 A 验证记录

以下由主实现者在独立审核后补充，不作为上文独立契约判断的输入。

- 完整普通测试：185 通过、0 失败，另 1 项原生隔离测试保持默认 ignored。本小步未重复运行原生进程验收，也不以日志测试冒充端到端协调器验收。
- 新增 21 项用例，含 4 项 PBT，各 128 cases：授权/普通目标交错、重复乱序多票结算、创建准备后崩溃重开、GLM/火山完整材料迁移。
- `bash scripts/contract_audit/sgm2_execution.sh` 通过：P13/P14/P16 注释定位、目录零网络 IO、枚举/SQL、实际 OperationView 与旧 snapshot schema、冻结 v1/v2 及运行模块差异检查。
- `cargo build --release --locked --offline` 通过；`cargo clippy --locked --offline --all-targets` 仅 6 个原有风格警告，没有新增警告。129 成员组装测试满足 3 秒门槛。
- 首次普通沙箱运行因禁止监听端口导致 26 项 HTTP fake 测试报 PermissionDenied；允许随机回环端口后完整通过，未修改测试断言规避环境限制。

定向变异使用独立临时源码和独立 target，不与正常 worktree 共用本包构建缓存。下列 6 项均产生预期测试失败，正常代码完整回归仍通过：

| 故意破坏 | 捕获测试 |
| --- | --- |
| 准备时跳过四重 revision 核对 | journal_stale_each_generation_is_rejected_before_preparation |
| 旧 Enable 成功不检查授权 epoch | journal_pbt_late_ack_consumes_only_its_authorization；最小反例为新显式恢复后旧 ACK |
| GLM 忽略普通更新继承的已消费授权 | 同一 PBT；最小反例为普通模型更新后旧 ACK，正确结果应 preserve/Confirm 而不是 Reject |
| 可能创建过的请求无条件恢复 Never | journal_create_retries_only_after_proven_not_sent |
| 结算不推进 execution，保留旧观测有效性 | journal_prepare_and_ack_never_confirm_member_or_operation |
| ACK 跳过来源/组 generation 核对 | journal_late_ack_checks_source_and_group_even_without_new_member_target |

§5.2 五维度复核：契约与当前小步边界一致；Rust/SQL 与既有目录风格一致；准备/结算/恢复均事务化、旧结果受代次与授权约束；组装读取不复制全库秘密且有性能门槛；两张执行侧表复用现有目录/计划器，没有新增服务或策略框架。完整网络正确性与 worker 性能仍待后续验收。
