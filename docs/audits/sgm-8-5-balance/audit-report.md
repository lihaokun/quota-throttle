# SGM-8 第 5 批 B1：C2 独立审核

> 当前总入口（2026-11-03）：B3–B8已连续实施，用户取消模块间停点；[最终代码契约独审](final-contract-review.md)通过、未关闭生产问题0。F1已修并独立56例关闭；修后最后Node68/0/check:ui/full112/112、余额8/8、全部JS25/25通过。仅最终固定SHA完整Rust/C2汇总pending，**不宣称固定SHA C2/整批退出已完成**。见[实施汇总](b3-b8-implementation-review.md)、[F1修正](../../fixes/sgm8-balance-policy-raw-amount.md)、[处置](decisions.md)、[十节实施对照](expectations.md)。部署前确认保留，stable部署未变。

> B2历史进度：C2意见修正后的专项复审及门禁曾通过（Rust803/0、Node60/0、浏览器full96/96），见[修正记录](b2-c2-fix-review.md)。下文与旧B2失败报告均为各自固定SHA的历史结论，不替代新提交复核。

> 后续审核状态（2026-11-02）：**B2 提交 `9619729` 的 C2 暂不通过**，1 major、2 minor 测试证据缺口，四项错误变异逃过原14项测试。详见 [B2 C2 汇总](b2-c2-audit-report.md) 与 [PBT完整性专项](b2-c2-pbt-review.md)。下文保留 B1 的历史通过结论，不代表 B2 或整个第5批通过。

2026-11-02。被审提交：`94362c46b1f7eeb9951f0562f1cbcbc9b295eb34`；差异基线：`7e5cf97`；分支 `research/source-groups-token-management`，归 PR #5。

**结论：B1 局部 C2 审核通过。** 路径 A、路径 B 与 §5.2 五维汇总均通过；剩余 blocker/major/minor 为 0/0/0。固定提交上的门禁本轮全部一次通过，189 个输入文件在执行前后完全一致。

本报告只审核 B1：内部余额策略持久化、v9 迁移及测试。B2–B8 尚未实现，本报告不是整个 `sgm-8-5-balance` 的 workflow §6.6 退出或部署许可。

## 方法与独立性

依照 workflow §6.5，**路径 A 为机械检查，路径 B 为 subagent 独立审核**；汇总后再按 §5.2 五维判断。前轮报告中的“局部路径 A”是历史命名，本轮按 workflow 的正式含义区分。

三位 reviewer 均从无会话历史的新上下文开始：

| Reviewer | 职责 | 独立性措施 |
|---|---|---|
| `/root/c2_b1_contract_v2` | 路径 B：契约重新提取、diff 对照 | 先读设计、固定独立预期，再读 diff；未读作者审查报告或 expectations 的实现映射，只为具体疑点定点查看 helper |
| `/root/c2_b1_correctness_v2` | 代码正确性、测试充分性及五维审核 | 先形成代码结论，再检查变异原始证据；未读作者或旧审核报告 |
| `/root/c2_b1_validation_v2` | 路径 A、固定 SHA 独立门禁 | 执行前声明检查项，记录 189 个 src/Cargo/schema 文件哈希；独占 cargo，全量保留默认并行度，浏览器在重型 Rust 回归结束后执行 |

仓库缺少 `templates/contract-audit/subagent-prompt-template.md`，本轮以明确任务说明执行 §6.5 的契约优先、diff 审核、隐藏实现映射要求，没有据此省略独立审查。

## 路径 B：独立预期及核对

以下预期由契约 reviewer 在读取实现差异前固定，全部核对通过。

| 独立预期 | 实现/测试证据 | 结果 |
|---|---|---|
| STRICT 单行表，安全 revision、精确 0/1、`0 < warn < target <= 4e15`；Policy/Draft 字段及共享 cap 一致 | `schema_v9.sql:1`；`balance.rs:12`、`:29`、`:33` | ✅ |
| 新建及迁移默认 `(1,1,1,1e14,1e15)`；默认行由 Rust 插入，SQL 文件只含 CREATE | `store.rs:480`；`balance_tests.rs:88`、`:238` | ✅ |
| 读取缺失/损坏报 Storage；打开验证恰好一行、singleton=1，损坏报 CorruptData，不补默认行 | `balance.rs:33`、`:72`、`:117`；`balance_tests.rs:340` | ✅ |
| usable→输入校验→IMMEDIATE→读旧行→版本比较→no-op→bump→条件 UPDATE→commit | `balance.rs:84`；`balance_tests.rs:137` | ✅ |
| 同值但旧版本仍冲突；JSON_SAFE 版本的 no-op 成功、有变化报 InvalidInput 且不写 | `balance.rs:95`；`balance_tests.rs:166`、`:176` | ✅ |
| BEGIN/COMMIT Busy→QueueFull、回滚且仍可用；其它提交失败保守中毒并拒绝后续读写 | `balance.rs:63`、`:89`、`:112`；`store.rs:790`；`balance_tests.rs:200`、`:379` | ✅ |
| 原 v8 先只读检查与 0600 一致备份，再同事务保留 instance_id、建表/插默认/切换版本并验证 | `store.rs:137`、`:256`、`:467`；`balance_tests.rs:238`、`:288`、`:315` | ✅ |
| >=9 排除 v8 meta、追加 v9 DDL；原业务表和历史数据保持，未来版本拒绝继续有效 | `store.rs:929`；`token_migration_tests.rs:31`、`:137`；`observed_publication_properties.rs:4` | ✅ |
| T-B1–B3 覆盖边界、原子性、迁移、备份及真实锁冲突；PBT 不调用生产实现作为 oracle | `balance_tests.rs` 的 10 个用例，`:428` 起显式 128 例独立状态模型 | ✅ |
| 新行为只属于 catalog，无 worker 发布或 Native 写入，无旧策略、runtime、HTTP/UI 功能改动 | 本提交仅改变 catalog 下 9 个源文件 | ✅ |

两路审核均未找到契约违例或实现缺陷，**❌ 项 0、待决策 ⚠️ 项 0、遗漏项 0**。代码 reviewer 额外核对了锁定依赖 rusqlite 0.40.2 的事务 Drop/commit 实现，确认错误返回后的 RAII 回滚语义。

## 路径 A 与独立执行

| 检查 | 本轮独立结果 | 原始证据 |
|---|---|---|
| SQL 与批准设计 §4.1 | 685 字节精确相同，SHA256 `eb71b26abd936c153d1ef3f523bf4d6a6acf043cadfc9114085f3929a2ea993f` | `/tmp/sgm8-b1-c2-mechanical.json` |
| Step / 共享上限 / 改动范围 | 四个余额 Step 在对应函数内且全源唯一；Rust 与 SQL cap 均为 4e15；未改 GLM/火山策略、runtime、newapi、UI | 同上 |
| 格式与差异 | 8 个 Rust 文件 rustfmt（skip_children=true）、工作区与提交 diff --check 均通过 | 同上 |
| `cargo test --release` | 787 passed、0 failed、35 既有 ignored、0 filtered；默认并行，524.53s | `/tmp/sgm8-b1-c2-rust.log` |
| `npm run test:ui` | 60 passed，0 failed/skipped/cancelled/todo | `/tmp/sgm8-b1-c2-node.log` |
| `npm run check:ui` | 60 项精确清单、8 组 PBT、契约生成和 DOM sink 检查通过 | `/tmp/sgm8-b1-c2-check-ui.log` |
| `python3 scripts/contract_audit/sgm7_check.py` | 通过：26 个精确可运行 Rust 用例；15 C/4 浏览器为清单核对，不冒称实测 | `/tmp/sgm8-b1-c2-sgm7.log` |
| `npm run test:browser` | 原 full 命令一次通过 92/92，0 失败/跳过/重试；23 名称 × 4 项目逐项匹配，fixture 正常退出 | `/tmp/sgm8-b1-c2-browser.log`；`/tmp/sgm8-b1-c2-browser-report.json` |
| 输入完整性 | SHA 未变，189 文件前后哈希完全一致；输入清单 SHA256 `e8597523c6bcec7c894318fdc5204babe63399abaf488639b81b648256b05498` | `/tmp/sgm8-b1-c2-final-integrity.json`、`/tmp/sgm8-b1-c2-inputs-{before,after}.sha256` |

JSON wire schema 在 B1 中没有变更或新出口，B2–B8 的 JSON/HTTP/UI 门禁属于后续模块；未将它们伪报为 B1 已实现或项目检查缺失。已有性能断言与变异证据由代码 reviewer 和主任务复核，源码未出现新的语义变化，因此本轮不重复已有成功运行。

## 变异与性能证据复核

已查看 `/tmp/quota-throttle-b1-mutations-521d9511/` 的原始失败日志、恢复日志及 177 文件快照，不能仅据作者总结认定通过：

- 删除版本比较：编译成功，旧 revision 错误返回 Ok，精确目标测试失败。
- Busy 映射改 Storage：编译成功，真实 BEGIN Busy 的 Storage/QueueFull 断言失败。该次运行在 BEGIN 分支被杀死，**不冒称这一次变异运行也执行了 COMMIT 分支**；恢复基线用例确实遍历两分支。
- 绕过 commit helper：编译成功，真实提交错误后的 poisoned 断言失败。
- 三项均为语义失败、exit 101；初始和最终恢复为 10/0，PBT 保持 128 例，177 文件恢复哈希全匹配。与固定提交的差异仅四行 Step 注释及测试表达式的换行缩进，没有可执行语义变化。

原始独立串行性能日志 `/tmp/sgm8-b1-performance.log` 的四项均通过：1 万身份 233ms、v7 万操作迁移至当前版 449ms、最大 Native token 页 7ms、v6 万操作迁移至当前版 720ms；原阈值没有放宽。这些是前轮语义相同源码的证据复核，不冒称本轮重新执行了变异或性能门禁。

## §5.2 五维汇总

| 维度 | 结论 |
|---|---|
| 一致性 | ✅ 字段、值域、错误分类、revision 行为、迁移顺序均匹配独立契约 |
| 风格 | ✅ 复用既有错误类型、bump、IMMEDIATE/commit 与迁移组织，无无关重构 |
| 正确性 | ✅ 未发现逻辑/CWE 缺陷；真实事务失败、损坏拒绝、全表镜像与恢复证据充分 |
| 性能 | ✅ 日常为单行查询/更新，验证在打开/迁移发生；没有新增逐来源请求、全表复制或多余持久层 |
| 可维护性 | ✅ 职责集中、共享类型和 cap 单点定义，未提前混入 B2–B8 的传输与调度逻辑 |

原始独立报告：`/tmp/sgm8-b1-c2-contract-review.md`、`/tmp/sgm8-b1-c2-correctness-review.md`、`/tmp/sgm8-b1-c2-validation.md`。主任务汇总独立契约预期、机械结果、实际门禁、代码审核与原始证据后，确认 B1 局部 C2 通过，无需代码修复或接受偏离。

本轮仅新增/同步审核文档；没有改动被审源码。后续实施模块为 B2（有截止的 Native 管理调用）；B2–B8 未完成，整个子计划尚未满足退出条件，不能推定为可部署。未推送 PR #5、未部署或修改运行服务。
