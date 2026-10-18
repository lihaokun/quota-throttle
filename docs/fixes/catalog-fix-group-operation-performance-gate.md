# 大组操作计时门禁补齐隔离

2026-10-19，SGM5 C收尾时发现；沿已确认的[性能门禁分离方案](sgm-fix-parallel-performance-gates.md)补齐遗漏用例。本八节先于修复写下，设计/实现独审与最终验收通过。只调整测试编排，不改业务、预算或运行服务。

## 一 现象与复现

C首轮默认debug为644 passed/1 failed/25 ignored，1261.87秒；唯一失败是`control_tests.rs:791`的`started.elapsed() < Duration::from_secs(5)`。原阈值单项连续三次通过，完整case耗时1.10/1.13/1.17秒。第二次默认并发又观察到同一断言所在测试失败；未把这次未完成运行写成完整结果。源码和原测试相对`bc8f33e`均零改动，B前批也有该断言失败记录。

## 二 根因

此旧测试仍将并行功能套件中的墙钟耗时直接与5秒比较，遗漏了此前的性能隔离改造。数量/优先级/版本正确性与共享机器CPU/IO等待无关。不能凭这些记录精确分解CPU、磁盘或调度的贡献，也不声称业务性能回归；已经证实计时环境不受控且同case单独运行满足原预算。

## 三 参考机制

复用`glm_round_tests::maximum_catalog_case`与既有`performance-gates.sh`，不引入新框架/依赖。原普通容量测试保留，独立性能入口仍执行相同case和全部功能断言。为不将原debug预算悄悄换成较快的release预算，本用例额外在独立debug阶段也执行5秒门槛。

## 四 修复方案

从原测试提取共享case，原计时开始/结束位置及MAX_SOURCES夹具不变，返回被测片段Duration；保留目标数量、未确认数量及每个成员优先级/版本全部断言。普通测试调用同case但不判墙钟；新增ignored的`performance_gate_control_group_priorities`断言原5秒。

性能runner完整release集合由22增加至23，再对这个入口以debug/ignored/exact/单线程执行。debug也先校验精确单项清单，防止零测试假绿。runner的失败传播回归覆盖debug列表为空/错误和运行错误。现有CI已显式调用runner，不修改CI或降低普通套件并发。

## 五 正确性论证

共享case保留原全部非时间断言及夹具，普通回归没有被忽略。单独门禁的计时窗口与5秒值不变，release和debug均实际执行。列表精确匹配与失败传播防止遗漏；预算设0的独立副本须在成功编译后真正断言失败。生产control及所有调度、DDL、网络路径零diff。本调整是确认后的测试职责错误修正，不以修改业务预期掩盖失败。

## 六 测试同步

| 检查 | 要求/当前状态 |
| --- | --- |
| 红例 | 完整首轮保留，第二轮重复失败保留为未完成运行 |
| 功能 | 原测试名/最大容量及全部值断言保留，字节归一化对照与默认debug/release通过 |
| 门禁 | release23项通过（57.99秒），本项103ms；debug单项通过（1.12秒），本项214ms；原5秒不变 |
| runner | 原8项保留，增加debug空列表/非空错名/列表失败/执行失败，12项已通过 |
| 负向 | 私有副本正常预算1/0（227ms），预算0成功编译后0/1断言失败（189ms），全源码恢复对照后1/0（252ms）；原5秒未变 |
| 全量 | 默认debug/release各645 passed/0 failed/26 ignored，分别1194.94/957.10秒；机械/clippy/build、全部性能及C原生组合通过 |

## 七 代码同步

只改`src/catalog/control_tests.rs`和两个性能runner脚本；C机械脚本增加该独立测试修正的精确允许范围、两个wrapper精确文本及原control测试归一化对照。不改`src/catalog/control.rs`或任何生产函数。设计独审通过后已实施，提交待验收后回填。

第二次重复验证仅观察到同一用例再次失败后，核对并停止自己的runner/Cargo/测试进程；没有子进程，保留日志为中止/未完成，不捏造第二轮完整计数，也不启动其后续自动阶段。

负向副本`/tmp/quota-throttle-sgm5-c-control-gate.FPFH1S`，三个日志为`baseline.log`、`zero-budget.log`和`restored.log`。变异要求真实文本变化、exit101、运行精确1项且0 passed/1 failed/0 ignored；不能以编译失败代替捕获。最终主树验证日志前缀`/tmp/quota-throttle-sgm5-c-complete-`，整组脚本退出0。

## 八 文档同步

C的审核/决策、细化和expectations注明额外性能隔离修正，C本体仍45入口，普通全量仍645项，性能ignored由22增加为23，总ignored由25增加为26。更新实施计划的最终结果，但不覆盖B或本轮首轮失败记录，也不把本地验证称为远端CI或部署已完成。
