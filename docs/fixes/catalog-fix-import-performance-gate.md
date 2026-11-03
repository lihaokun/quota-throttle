# 目录与 GLM 容量测试的计时隔离补齐

2026-10-19，SGM6 A1 全量验收暴露，先记根因与方案再实施。沿已确认的[默认并行与性能分离方案](sgm-fix-parallel-performance-gates.md)及[大组操作补齐](catalog-fix-group-operation-performance-gate.md)，不改变业务或原预算。

## 一 现象与复现

fermat 隔离目录 `/tmp/quota-throttle-sgm6-a1.LAtp1k` 的默认 debug 全量为 662 passed / 1 failed / 27 ignored，464.22 秒，退出 101。唯一失败是 `catalog::store::tests::import_and_snapshot_scale_within_budget`，栈明确指向 `src/catalog/tests.rs:1073` 的导入 `start.elapsed() < Duration::from_secs(10)`。完整日志 `full-debug.log` 保留；后续 release/性能因前项失败尚未运行。

在该完整运行结束后，以同源码、同 debug profile、原 10 秒阈值精确单独运行三次，均为 1 passed / 0 failed，完整 case 分别 0.74 / 0.75 / 0.73 秒。日志为 `import-isolated-{1,2,3}.log`。单项包括原功能断言；不称其为本次全量已通过。

第二轮默认debug为662 passed/1 failed/28 ignored、521.19秒，完整栈确认旧`journal_group_snapshot_has_bounded_cost`在execution_tests.rs:561命中3秒断言，日志`final-debug.log`保留。未同步后续测试修改的原fermat副本精确单项三次通过，完整case为0.15/0.15/0.14秒（journal-isolated-{1,2,3}.log）。主代理与独立审核系统扫描全部elapsed/Instant/duration_since，确认另有两处普通测试仍混入纯容量预算：此129成员组快照3秒，以及`source_policy::tests::capacity_bounds_and_maximum_snapshot_are_finite`的万成员GLM纯决策5秒。后者原样单项通过0.04秒，没有观察到失败，不编造红例。

## 二 根因

旧容量测试仍混入导入和快照两段各 10 秒墙钟预算，遗漏了既有性能隔离改造。全量并发竞争下导入超时，独立运行满足原阈值；源码相对 `999ca41` 该函数未改。可以确认计时环境不受控，不能由此精确分解 CPU/IO 等待或证明业务性能回归。

该测试后半段还验证来源、组、成员容量及拒绝新增时零变更。整项忽略会丢失功能保护；扩大阈值、降低全量并发或只复测到绿色都不能修正职责混合。

本轮系统扫描确认的遗漏为上述三项容量用例。锁占用立即拒绝、SQLite Busy期限、探针/transport整体超时和虚拟时钟的延迟边界属于行为约束，继续在普通套件执行，不能一概移出。已有ignored性能入口保持原阈值与执行路径。

## 三 参考机制

复用已实施的 `full_group_operation_case` 和性能 runner：原普通入口保留，完整共享 case 返回被测耗时；ignored 性能入口实际断言原预算，runner 精确清单核验和串行执行。旧 debug 预算仍要在 debug 单独执行，不能仅用 release 替代。

## 四 修复方案

提取 `import_and_snapshot_capacity_case() -> (Duration, Duration)`。setup 和 build_legacy_import 仍在计时外；第一段严格只测原 import，第二段仍为导入后的原 snapshot 及原来源数量断言，不挪到万成员构造后。完整后半段容量/失败零变更断言保持。

原普通入口同名调用共享 case，不判墙钟。新增 `performance_gate_import_and_snapshot` 使用同一 case，分别断言两个 Duration < 10 秒，不合并预算；输出两段耗时。release 性能集合由本批 24 增为 25，独立 debug 保留原大组并新增此入口。runner 正反例须覆盖第二个 debug 入口的空/错清单、list 失败、执行失败，不能只保护第一个。

生产 store/导入/schema/worker/策略均零改。机械检查对旧函数归一化后逐字比较，严格识别两个 wrapper 和原计时片段；精确允许此次测试文件和 RCA，不扩大业务修改范围。

系统核查后的增补方案（先独立审核再实施）：组快照提取原完整case，计时仍从`load_group_work`前到129条数量断言后，返回Duration，新gate原3秒；GLM容量提取原完整case，构建10000成员仍不计时，原decide和优先级数量断言计时，超容量拒绝仍在同case，gate原5秒。两个普通测试名/全部值断言保留。新增这两个release及debug入口后总计27个release、4个debug入口，后者保留原5个预算。每个新debug入口同样增加四类失败传播自检；两个预算各作一次零预算编译变异。

## 五 正确性论证

所有普通功能断言继续运行，只有墙钟判断转交独立阶段；四个计时窗口和预算均相同。runner 校验所有门禁的可达性，任一失败保持非零。分别将导入、原快照、组快照、GLM纯决策预算变为零，必须成功编译后断言失败，证明四个分支未被漏跑。测试编排变化不进入生产二进制，不影响在线服务。

## 六 测试清单

| 检查 | 要求与状态 |
| --- | --- |
| 原红例 / 单项证据 | 两轮完整662/1/27和662/1/28及各三次原样1/0；日志保留 |
| 功能保真 | 三个共享case归一化逐字比较通过；普通名字、全部容量和值断言保留，独立实际diff复审通过 |
| 原预算 | 10/10/3/5秒不变；最终release为90/3/11/2ms，debug为307/14/40/18ms，均通过 |
| runner | 原12项保留，三个新增debug入口各补四类失败传播，共24项通过 |
| 负向 | 四种零预算分别成功编译后0/1捕获；逐字恢复并实际重新编译，各入口1/0通过 |
| 全量 | 最终默认debug/release各663/0/30，506.94/354.31秒；27项release性能及4个debug入口全部通过，机械/clippy/build通过 |
| 系统补齐 | 已区分纯容量与锁/Busy/超时等行为约束；GLM纯容量是静态同类发现，不声称红例，原行为断言不删除 |

## 七 代码同步

首项修改 `src/catalog/tests.rs`、两个 `scripts/tests/performance-gates*.sh` 和本批机械脚本 `scripts/contract_audit/sgm6_a1.sh`，在A1变异恢复通过后才实施。保留原导入预算10秒、快照预算10秒及全部普通断言。全部验收通过，代码提交`b4116ef`。

第一项及后续系统补齐均按先写方案/独立批准后实施；两个新增测试文件为`src/catalog/execution_tests.rs`及`src/source_policy/tests.rs`，不改生产execution/store/GLM函数。实际diff复审通过，机械脚本逐字归一化两份原测试；source_policy冻结仅排除此明确测试文件，生产实现仍逐字冻结。fermat第二轮运行期间副本未变，结束并原样复现后才同步最终测试代码。A1普通数量不变，原27 ignored经三项容量补齐后为30 ignored。

## 八 文档同步

本RCA、A1审核/decisions/expectations/细化和实施计划已记录旧失败及性能补齐，提交引用已回填。8类A1业务变异与4类零预算变异分开计数；最终门禁为27个release、4个debug入口，后者包含原大组5秒及本批10/10/3/5秒共五个预算。旧25/2只是第一项补齐后的阶段目标。不会覆盖历史失败、把本次隔离验证称作远端CI或部署。

负向证据分别在本机`/tmp/quota-throttle-sgm6-a1-import-gate.siKruU`和`/tmp/quota-throttle-sgm6-a1-remaining-gates.A5PC37`。两批恢复都显式更新时间戳，编译日志分别记录18.77秒、27.16秒，防止Cargo复用旧变异二进制；完整来源/测试内容逐字恢复一致。详细输出见[A1审核](../audits/sgm-6-three-page-panel/a1-audit.md)。
