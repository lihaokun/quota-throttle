# 火山观测归并核心审核

日期：2026-10-17，基线 `1872ab3`，范围 O1–O3，本步本地验收通过。用户确认“观测型熔断，成功有 token 流清空计数”。源码分两笔本地提交：`88b1b36`（内部命名）与 `64876c5`（归并核心）。O4 持久/运行接线和整个来源分组功能尚未完成，不能由本步替代。

## 实现与边界

统一 ledger 新增正 token 消费/明确上游失败分类与单请求状态合并。SourceId/request_id 绑定去重状态，不以渠道、组或协议为计数单位；Failed 只可转 TokenFlow，同源重试错误最多输出一次，随后成功可输出一次清零，成功重放/迟到错误不再输出。没有另一份失败计数器、全局去重 cache 或 IO。

原 VolcanoRuntime 的成功分支已清零连续计数且保留耗尽锁存；本步只把内部 FinalOutcome/completed_at 改称 OutcomeObservation/observed_at，旧逻辑和断言不变。不改变 token 计量、日重置、人工撤销、GLM 或数据库格式。

正 token 口径为 input+output>0，包含非流式和 Done/Eof/ClientGone 的有效消费活动；不声称 EOF/取消意味着完整响应成功。零 token、损坏/未知流式状态或显式失败不清计数。失败仅使用结构化类型/HTTP/code 白名单，参数、上下文、内容拒绝与取消排除优先，不读取报错正文。

O4 持久与运行接线未完成：provider/适配范围由后续可信 catalog 核验，请求状态与 Runtime/目标/许可须同事务发布。当前无新增后台任务或在线自动熔断入口，不能把本核心称为整个功能完成。

## 自动测试与独审

新增 12 入口：8 确定性、3 组各 128 cases PBT、1 独立性能。确定性覆盖 F/F/S/F=1/2/0/1、F/F/F/S=count0 且仍锁存、同请求跨组渠道去重、另一来源成功不清本源、重复成功不清后续新失败、input-only token、状态/标签坏值和原 cutoff/period。

PB1 用独立决策表并在每 case 强制正向分类覆盖；PB2 用独立失败/成功集合和最早时刻 map；PB3 将实际合并结果送入原 Runtime，与独立来源计数/锁存数组逐步比较。参考模型不调用生产分类/合并 helper 生成预期。

独立代理先读契约再审实现，提出的测试独立性、随机主分支覆盖、适配范围前置条件及 Display 负 trait 已关闭，静态审核 0 unresolved，见[处置记录](observed-breaker-decisions.md)。主代理负责以下执行证据。

| 验证 | 当前实际结果 |
| --- | --- |
| 首次定向 | 11 passed/1 性能 ignored，0.28 秒 |
| 强化后机械入口 | 11 passed/1 ignored，0.52 秒；精确 12 项清单、空/全 ignored/漏项负例、格式/范围/Step、旧算法/断言机械等价通过 |
| 性能 runner 自检 | 8 项通过，保留 13 既有性能入口并追加本步一项 |
| 默认 debug 全量 | 507 passed/0 failed/16 ignored，827.08 秒；其间仅新测试夹具的等值初始化写法调整，断言和生产逻辑不变 |
| 默认 release 全量 | 507 passed/0 failed/16 ignored，458.78 秒 |
| 独立串行 release 性能 | 14 passed/0 failed，23.58 秒；新增十万次分类/合并 41 ms（64 字节 request、128 次尝试元数据），预算 5 秒；13 项既有门禁保留并通过 |
| 可编译变异与恢复 | 9 种均编译后断言捕获（各 1 failed/exit 101），恢复后 src 逐文件比较一致，11 passed/1 ignored，0.25 秒 |
| clippy/release build | 最终 clippy all-targets 通过（11.62 秒），仅 6 项既有警告；release build 通过 |

9 种变异为零 token 当成功、成功不清计数、重复错误再次计数、重复成功再次清零、跨 source 混用请求状态、成功解除已锁存错误、参数错误误计、忽略 input-only token、任意 403 均当上游不可用。其中前四项由独立 PBT 捕获，其他由直接断言捕获。回归种子保留在隔离副本，恢复后正例通过，不在开发树试写错误实现。

默认全量未覆盖 RUST_TEST_THREADS 或 test-threads；性能等全量、构建、变异结束后独立串行执行，没有重编任何正在运行的同一测试可执行文件。16 个默认 ignored 项中，14 项性能本轮执行；原生 NewAPI 门禁及 B3 百万历史趋势未重跑，相关 HTTP/运行接线/查询算法无变更，沿用各自最近审核，不把历史结果称为本轮执行。没有触发远端 CI。

本轮无生产代码红例或放宽断言；事前独审发现的覆盖问题在退出前补强。Clippy 初次指出新测试的字段默认值构造写法，改为相同数值的结构体初始化，不改断言。测试日志位于 `/tmp/quota-throttle-observed-*`，变异脚本/源码在 `/tmp/quota-throttle-observed-mutants.wwZqIm`。复现入口为 `scripts/contract_audit/sgm4_observed_breaker.sh`、默认 debug/release cargo test 与 `scripts/tests/performance-gates.sh`。临时日志可能被系统清理。

## 五维度与运行隔离

- 一致性：新观测规则、成功 token 清零和旧原因锁存同时成立；不再宣称可靠请求终态。
- 风格：复用事实、枚举、数值和 request_id 校验，只增加私有分类/合并模块。
- 正确性：跨来源身份、重试和成功重放、时间隔离有直接和独立属性断言。
- 性能：单事实与单请求有界，没有新增全历史扫描、计数镜像或缓存驱逐逻辑；最终门禁另列。
- 可维护性：类型更名消除终态误导，旧 caller 和测试只机械适配；保持 O4 的事务责任清晰。

仅开发 worktree `research/source-groups-token-management`。未读取生产配置/数据库/环境/密钥，未请求实际服务或供应商，未改 NewAPI、未重启、部署、推送或合并。整套功能仍有持久/运行接线、访问 key 管理和三页面待完成；不以新纯核心测试代替这些验收。
