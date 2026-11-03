# SGM-8 第 5 批 B1 模块实现审查

2026-11-02，基线 `7e5cf97`，分支 `research/source-groups-token-management`，归 PR #5。用户已确认 D1 的 60s 业务/70s 客户端/75s 连接配套；C1 完成。本文只覆盖 catalog 模块；B2–B8 未实现，不能作为整批 C2 或部署通过的依据。

## 实现与契约

- `catalog/balance.rs` 新增共享策略类型、上限、校验、读取与带修订号的保存。保存无变化时不递增；坏输入/修订号耗尽为 InvalidInput，冲突为 RevisionConflict。沿用统一提交函数的 CommitUnknown 中毒行为，仅对确认回滚的 Busy 单独映射 QueueFull。
- `schema_v9.sql` 仅 CREATE 表；默认策略由 Rust 迁移插入。新建库与 v8 迁移均得到 revision=1、自动开启、预警 1e14、目标 1e15。v8 迁移前先只读验证与 0600 备份，同事务保留实例 ID、切换版本并插入策略；新库不伪造旧库备份。
- 历史版本夹具移除 v9 策略表，镜像比较只增加预期的默认行，未来版本测试使用当前版本 +1；旧业务断言保留。

函数/字段对应见 `expectations.md` 的 B1 局部对照。未新增依赖，未连接远端或运行目录库；补足/调度/HTTP/前端行为尚未接线。

## 独立审查

后端 reviewer：`/root/sgm8_balance_backend_design_review`，只读核对细化 §§4.1、5.1、5.2、T-B1–B3 与代码，按一致性、风格、正确性、性能、可维护性五维审查。

初审 blocker=0、major=0、minor=1：直接设置 poisoned 的测试无法证明新保存入口会处理真实 CommitUnknown。已补 `balance_commit_unknown_poisoning_requires_reopen`：通过 AFTER UPDATE 触发器与延迟外键制造真实 commit 失败，验证 Storage、目录不可用、事务结束、全表镜像回滚、后续读写拒绝及去故障后重开恢复。reviewer 静态复核关闭此项；实际测试与变异证据另列，不以静态审查代替测试通过。

架构 reviewer：`/root/sgm8_balance_architecture_review`，从原契约重新提取 B1 要求，对照 SQL、类型、保存入口、迁移、只读校验与测试，而非采信作者回填。指出本批专属 Step 注释缺失；已补 `B1.balance.load/save/verify/migrate`，限定余额文件/对应函数作唯一性核对，不让旧账本 `Step B1.1` 充数。SQL 与细化 §4.1 原文机械精确比较通过。reviewer 复核后 B1 局部路径 A 与五维静态审查均通过，剩余 blocker/major/minor 为 0/0/0。

## 故障记录及处置

首轮 `cargo test --release balance_ -- --nocapture`（旧测试名称）8 通过、2 失败，未隐去失败：

1. 新备份测试枚举器把只读打开备份后生成的 `.sqlite3-wal` / `.sqlite3-shm` 计成额外备份，重开计数得到 3 而非 1。镜像、实例与 0600 断言此前均通过；修正测试枚举器只计 `.sqlite3`，生产迁移代码未因此更改。
2. 宽泛过滤器包含旧 HTTP 测试 `t5c_disable_delete_ignore_balance_group_and_source_outage`，沙箱禁止临时 localhost 监听（Operation not permitted）。完整回归改为获准在沙箱外运行隔离假服务，生产端口不变。

`npm run check:ui` 首轮在沙箱内拿不到子进程的 Node 输出（actual=[]），独立 Node 60 项全过。沙箱外原命令复跑通过，未修改前端代码、清单或断言。

浏览器首轮 91 通过、1 失败：firefox-desktop 的 `rejected delete keeps confirm open with reason and tracks nothing` 在 30000ms 等待“确认提交”按钮超时，尚未发送 DELETE。失败轨迹与上下文保留在 `/tmp/sgm8-b1-browser-first.hbXTCG/test-results/`，原始门禁日志 `/tmp/sgm8-b1-browser.log`，门禁报告 `/tmp/quota-throttle-sgm6-b-browser-Gunjn4`。该轮与重型 Rust/PBT 同时执行；超时原因不能仅据负载推定，按原断言与时限复跑四项目，不修改业务代码或测试以放宽门禁。

## 门禁与后续

| 门禁 | 本轮结果 | 证据 |
|---|---|---|
| `cargo test --release` | 787 passed / 0 failed / 35 原有 ignored，默认并行度 | `/tmp/sgm8-b1-cargo-release.log` |
| `npm run test:ui` | 60 passed / 0 failed / 0 skipped，原有 8 组 PBT 保留 | `/tmp/sgm8-b1-node.log` |
| `npm run check:ui` | 通过：精确 60 项清单、契约生成、8 组 PBT、DOM sink | `/tmp/sgm8-b1-ui-check-unrestricted.log` |
| `npm run test:browser` | 复跑四项目完整 92/92；首轮 91/92 失败记录保留如上 | `/tmp/sgm8-b1-browser-rerun.log`；`/tmp/quota-throttle-sgm6-b-browser-hZ6mil/report.json` |
| `python3 scripts/contract_audit/sgm7_check.py` | 通过，26 项精确可运行 Rust 清单；15 C/4 浏览器为清单核对，未冒称实测 | `/tmp/sgm8-b1-sgm7.log` |
| rustfmt（全部 8 个变动 Rust 文件，skip_children=true）与 `git diff --check` | 通过；不格式化未改动的子模块 | 本轮命令输出 |
| B1 机械核对 | SQL 与细化 §4.1 原文完全相同，4 个余额步骤锚点在对应函数内且唯一 | 独立路径 A 复核与本轮命令输出 |
| 独立串行性能门禁 | 4 passed / 0 failed：1 万身份 233ms、v7 万操作迁移至当前版 449ms、最大 Native token 页 7ms、v6 万操作迁移至当前版 720ms，原阈值未变 | `/tmp/sgm8-b1-performance.log` |

Rust 全量编译完成后，源文件仅有 Step 注释及 rustfmt 的无语义改动；未在全量执行期间重新链接其 release 测试产物。复跑浏览器的 debug 编译及独立性能门禁的 release 编译均包含这些注释。

测试日志位于 `/tmp/sgm8-b1-*`。普通全量 Rust 测试使用默认并行度；性能门禁单独串行。变异使用独立源副本与独立 target，禁止覆盖工作树产物。

## 变异验证

后端 reviewer 在 `/tmp/quota-throttle-b1-mutations-521d9511` 的独立源副本及全新 `cargo-target` 执行，未共享/硬链接工作树构建产物。每次只施加一个变异；恢复时校验源文件字节，最后核验全部 177 个文件与初始哈希一致。

| 变异 | 精确目标测试（均在 `catalog::store::tests::balance::`） | 结果 |
|---|---|---|
| 删除保存时的修订号比较 | `balance_validation_conflict_and_revision_exhaustion_are_read_only` | 编译成功；错误地对过期 revision 返回 Ok，1 failed / exit 101 |
| `write_error` 把 Busy 改为 Storage | `balance_policy_busy_is_queue_full_not_storage` | 编译成功；真实 begin Busy 得到 Storage 而非 QueueFull，1 failed / exit 101 |
| 保存绕过统一 commit helper，不标记中毒 | `balance_commit_unknown_poisoning_requires_reopen` | 编译成功；poisoned 断言失败，1 failed / exit 101 |

初始基线、第一次恢复及最终统一恢复均 10 passed / 0 failed；B1 PBT 保持 128 例。重新编排时主动中止的一次编译保留为 `02-busy-storage-orchestration-interrupted.log`，不计作变异捕获。

证据：该目录下 `summary.json`、`results.json`、`mutations.json` 与 `logs/04-final-restored.log`；B1-m1 的真实提交错误回归已通过测试及变异验证关闭。此结论仅覆盖 B1，不代表整批 C2 通过。

## 偏差与 C2 入口

B1 的字段、默认值、错误分类与迁移契约无功能偏差。原 T-B1–B3 按风险拆成 10 个真实用例（包含 1 组 128 例状态 PBT）；额外覆盖真实 commit Busy、CommitUnknown 与损坏旧库，不修改原业务预期。B2–B8 仍待实施，T-B4–B17 与整个余额功能的上线门禁尚未完成。

此报告随 B1 本地提交交 C2，审核目标以该提交 SHA 为准；前述工作树模块审查不冒称“本地提交后的 C2 已通过”。当前不推送 PR #5，不部署，不打开运行目录库、不写 NewAPI；用户没有新增未决功能选择。

按 workflow 逐模块推进；B1 本地提交后停 C2 交审。B2–B8 仍未实现；没有推送、迁移运行库或部署许可。
