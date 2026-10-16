# SGM 默认并行测试与性能门禁分离

日期：2026-10-16。修复基线 `6ae9345`，实现及证据提交 `4d4c9f0`。用户已确认先修测试门禁、澄清 SGM-4 A，再进入新模块实施；本轮不改变业务实现或运行服务。性能门禁修复已实现并验证，debug全量额外暴露独立目录锁问题，因此不宣称整体CI全绿。

## 一 现象与复现

外部报告 `/tmp/quota-throttle-sgm-review-20261016.md` 在默认 `cargo test --release` 下得到 359 passed/1 failed/1 ignored；最大 GLM 事务计时 62942 ms，超过 10 秒。相同测试隔离运行 1226 ms。此前本项目 360 passed 的记录使用了 `--test-threads=4`，不能据此声称默认并行命令已经通过。

复现命令为 `cargo test --release`，或带 `--locked --offline` 的同一默认测试并发；不设置 RUST_TEST_THREADS 或 `--test-threads`。根因路径是 `glm_round_tests` 最大目录测试的墙钟断言。另五处同类断言位于同文件的输入采集/恢复、policy_tests 的迁移、policy_migration_tests 的随机迁移、volcano/tests 的最大选择。

预期：默认功能测试不因别的测试占用 CPU/磁盘而误判业务失败；性能退化仍必须能让独立门禁失败。外部报告是已知红例；本轮修改前/后实测结果在验证后补记，不虚构本机已复现。

## 二 根因

测试把共享机器上的调度等待/IO 竞争计入固定墙钟预算，并混入默认并行的功能回归。容量、字段保真、回滚等正确性断言并不依赖这些等待时间。限制整个测试集的并行度只改变复现条件，没有修正职责混合。

没有证据把全部耗时差异精确归因于 CPU 或断言所有 CI 必失败；修复针对的是计时环境不受控这个事实。普通测试中的结构/结果验证不能替代性能预算，故预算保留并单列执行。

## 三 参考机制

这是测试编排缺陷，不改变 NewAPI/GLM 算法。Rust 原生测试框架支持 `#[ignore]`、筛选和 `--ignored --test-threads=1`，可把性能测试明确安排在独立阶段；无需新测试框架或依赖。[Rust 测试运行控制](https://doc.rust-lang.org/book/ch11-02-running-tests.html)

只标 ignore 会丢失门禁，因此必须同时提供清单核验、CI 显式执行和入口失败传播回归。

## 四 修复方案

1. 四个现有测试文件提取共用 case，返回被测片段耗时。原有普通测试及随机 PBT 仍执行全部非时间断言，名称和随机空间不变，不忽略整个容量/迁移测试。
2. 新增四个 `performance_gate_*` 测试并标 ignore，仅这些入口断言原预算：GLM 采集 5 秒、事务 10 秒、恢复 10 秒；基础迁移 5 秒；复杂历史迁移 5 秒；火山最大选择 5 秒。计时片段和 fixture 与原用例一致；复杂历史性能用例固定为随机空间内 n=6/nv=5、带 prepared 历史的样本，PBT 继续验证全部原随机空间。
3. `scripts/tests/performance-gates.sh` 先列出忽略性能测试并与四个完整名称比较，再用 release profile、同一筛选和 `--ignored --test-threads=1` 运行。缺失、多余、重命名、零匹配、list 失败和执行失败均使入口非零退出。入口不接受额外参数来覆盖筛选/串行条件。
4. CI 保留默认 `cargo test --locked`，追加入口脚本回归和性能步骤；原 release build 保留。普通测试通过而性能入口失败时整个 CI 仍失败。
5. 增加 shell 入口回归，以假的 cargo 函数模拟正常清单、空/缺/多清单、列举失败和执行失败；不会真的编译或访问网络。实际 Rust 性能用例另外实跑。

不修改生产函数、原 DDL、阈值/算法、配置/端口或服务启动。低优先级执行本地回归；不在全量测试期间并行重编译同一个 target，以免改变预检夹具执行文件。

## 五 正确性论证

- 根因消除：常规并行回归只断言逻辑结果，墙钟预算转由没有同套测试并发竞争的专门阶段检查；没有放大预算。
- 不变量保持：共用 case 保留所有原字段、数量、no-op、备份、坏数据和容量断言。随机迁移仍执行 128 cases；新性能入口不是它的替代品。
- 门禁可达：脚本先核完整测试集合，再运行同一集合；清单异常不能以零测试成功蒙混过去。CI 显式调用脚本，失败不忽略。
- 无业务回归：源代码 diff 限于 cfg(test) 文件。性能证明仍只针对标明 fixture 和串行环境，不承诺任意机器负载下必达 5/10 秒。

## 六 测试清单

| 检查 | 期望 | 状态 |
| --- | --- | --- |
| 修改前默认并行 release 全量 | 记录真实结果，若复现则保留完整错误 | 已复现：359 passed/1 failed/1 ignored，58404ms撞10秒，208.68秒，退出101 |
| 修改后默认并行 release | 原功能/PBT 全部通过，无测试线程参数覆盖 | 360 passed/0 failed/5 ignored，199.47秒，退出0 |
| 修改后默认并行 debug | 与CI相同的profile及默认测试并发 | 359 passed/1 failed/5 ignored，317.86秒；新失败为导入重开AlreadyOpen，非计时门槛 |
| 独立性能 | 四个入口实际执行，六个原预算全部通过 | 4 passed，4.06秒；六项为10/1118/762/20/29/3ms |
| 入口回归 | 正常通过；空/缺/多/重复清单、list/运行错误和参数覆盖全部拒绝 | 8项通过 |
| 负向门禁 | 在独立源码/target 降低一个性能预算，入口必须失败 | GLM事务预算0秒变异实际编译后3 passed/1 failed，断言失败，脚本退出101 |
| 范围/格式/独审 | 业务代码零 diff、旧功能断言保留、文档对应、workflow 独立审核 | 静态独审通过；diff/格式/脚本语法通过；clippy退出0（原6条warning），release build退出0 |

实际全量命令：`env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=2 nice -n 15 ionice -c 3 cargo test --release --locked --offline --quiet`；debug同命令去掉`--release`。没有传测试线程参数；编译jobs及CPU/IO优先级不是libtest并发覆盖。5个ignore为原1项加4个专门性能入口，普通功能用例未被忽略。

负向副本为 `/tmp/quota-throttle-perf-mutant.jv4SIr`，源码和target均独立，只把 `round < Duration::from_secs(10)` 改为0。第一次副本漏了公开测试模板 `config.example.toml`，仅编译失败，不计变异捕获；补齐后真正执行四个测试，目标断言失败而其余三项通过。主工作区的预算保持10秒，没有恢复/覆盖生产文件。

debug新失败种子已保留，单独回放128 cases通过（8.98秒）并不视为缺陷解决。文件锁继承机制已用独立fork门闩程序复现，另立[目录锁修正方案](catalog-fix-inherited-lock-release.md)，待确认后修改生产实现。当前不得把release或性能门禁通过写成所有回归通过。

## 七 代码清单

| 文件 | 改动 | 状态 |
| --- | --- | --- |
| src/catalog/glm_round_tests.rs | 共用完整容量 case，三个计时预算移至独立入口 | 已实现 |
| src/catalog/policy_tests.rs | 基础迁移 case 返回耗时，独立入口断言 | 已实现 |
| src/catalog/policy_migration_tests.rs | 随机迁移保真 case 返回耗时，固定代表样本性能入口 | 已实现 |
| src/source_policy/volcano/tests.rs | 验证/最大选择共用 case，独立预算入口 | 已实现 |
| scripts/tests/performance-gates.sh、performance-gates-regression.sh | 清单门禁、执行和入口回归 | 已实现 |
| .github/workflows/ci.yml | 默认回归之外显式运行入口回归与性能 | 已实现，未推送/触发远端CI |
| proptest-regressions/catalog/tests.txt | 保留额外暴露的导入重开失败种子，不删失败证据 | 已保留，锁问题未修 |

## 八 文档清单

- SGM-3 B/C 细化与 expectations：登记性能测试执行环境和强制入口，原预算及业务契约不变；历史审核数字不覆盖，追加本次修复索引。
- SGM-4 A 细化与 expectations：非法模型/组标签转未知保留计量；明确本地保护上限而非上游最大值；固定版本 request ID 为 23 位时间+8 位前缀+8 位随机字符。未来 A 的计时测试也按独立门禁执行，普通 PBT 不夹墙钟预算。
- SGM-1 细化：纠正残留“数据库当前 v3”，指向 v4 的历史迁移承接。
- 实施计划：记录修复和本轮验证范围，不把它当作 SGM-4 实现或部署完成。

上述文档已同步，历史审核数字保持原样并注明范围。[独立审核](../audits/sgm-3-source-policies/performance-gates-audit.md)与[意见处置](../audits/sgm-3-source-policies/performance-gates-decisions.md)记录静态范围，不替代动态结果。整体debug回归未通过，新目录锁问题保留为后续阻断。
