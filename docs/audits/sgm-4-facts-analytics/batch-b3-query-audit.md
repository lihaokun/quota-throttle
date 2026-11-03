# SGM-4 B3 查询计算审核记录

日期：2026-10-17。实施基线 `603f83f`。范围仅第一小步、函数 B3.1–B3.2；本步本地退出验收通过，不代表整个 B3 已完成。

## 实现范围

新增 `ledger/analytics`：查询输入与封闭过滤、完整分钟/边缘互斥分区、固定偏移小时/日桶、同一累计器的总计/模型/token/桶分项。复用 A5 精确加法和标签规则；模型/ID 不改写。QueryControl 本步只有取消标志与单调截止，没有 SQL 中断注册或后台任务。

push 错误会销毁工作集并锁存，后续 push/finish 不能返回先前的部分用量。零 token 消费保留行数；None 模型与字面“未知”分开；模型/结果和借用维度没有 Debug/Display/serde。finish 要求已初始化 LedgerState，不能把未初始化包装成零；真正同数据库快照的状态生产者留到第二步。

除 ledger 模块声明外，既有业务源码、DDL、依赖、catalog guard、worker、CLI、面板和 NewAPI 客户端均未改。测试和脚本仅新增本步入口/性能清单，不新增数据库、采集器或策略路径。

## 测试与失败处置

新增 15 项入口：14 普通、1 独立性能；含四组各 128 cases 的 PBT：时间/边缘与时区、逐消费过滤和分区参考值、分钟增量与逐条计量等价、独立预判的取消/错误轨迹。PB1/PB2 的内存计算范围在此覆盖，原始 Error 行筛选、真实一致快照和 PB3–PB6 的数据库/恢复部分仍待后续组合，不以纯 fixture 代替。

首次编译失败为 proptest 的 prop_assert 自动消息把表达式中的 `{32}/{745}` 当格式参数；改为显式固定消息，未改变断言。测试运行前还校正最大容量夹具的 token 从 0 起造成非法身份的问题，改为正 ID，未放宽生产校验。首轮 14 passed/0 failed/1 ignored，0.73 秒。

独审指出两处测试证据不足：取消 PBT 从实际 Err 推预期，最大输出夹具没有同时达到所有维度上限。已独立预判错误位置/类型，并将夹具提高到 1024 模型、4096 token、745 桶、65536 桶模型项和 256 字节标签。强化后机械门禁 14 passed/0 failed/1 ignored，1.31 秒；空清单与全部忽略清单负例均正确拒绝，原性能 runner 的 8 项入口回归通过。

## 独立审核与五维度

依 workflow §6 启动只读独立审，先抽契约再读 diff，未给实现位置映射；审查没有运行测试。初审无确定的实现错误，提出上述两项覆盖建议，复审均关闭，当前无未解决静态项。反馈处置见 [decisions](batch-b3-query-decisions.md)。

- 一致性：只落 B3.1/B3.2，不把后续 SQL/补正假装完成；错误全失败和观察口径与契约一致。
- 风格：保持 Rust/中文注释与封闭内部类型，复用现有标签/数值检查，无新依赖。
- 正确性：半开分区、负偏移、极值、None/大小写、AND 过滤、零消费、溢出和错误锁存均有直接断言/参考属性。
- 性能：有界分桶、分项 map，先核容量再插入；输出不是 token×model×time 的稠密矩阵。实际最大输出门禁见最终执行表。
- 可维护性：生产增量局限于一个 analytics 模块及 query 子模块，未复制采集、数据库或 provider 状态机。

## 最终执行证据

| 验证 | 实际结果 |
| --- | --- |
| 新增普通测试/属性 | 14 passed/0 failed/1 性能 ignored；4 组 PBT 各 128 cases；最终机械门禁 1.25 秒 |
| 默认 debug 全量 | 459 passed/0 failed/12 ignored；472.62 秒 |
| 默认 release 全量 | 459 passed/0 failed/12 ignored；365.05 秒 |
| 独立串行 release 性能 | 11 passed/0 failed，8.30 秒；新最大输出 118 ms，预算 5 秒，其余 10 项既有门禁保留 |
| 定向变异 | 10 项均编译后被对应断言捕获（各 exit 101/1 failed），不是编译失败 |
| 变异恢复 | 独立副本恢复后 src 与开发树逐文件一致；再次 14 passed/0 failed/1 ignored，1.32 秒 |
| 机械/入口负例 | Step/范围/格式/精确清单通过；空清单及全 ignored 清单均拒绝；性能 runner 回归 8 项通过 |
| 构建/静态检查 | release build 32.35 秒通过；clippy all-targets 通过，仅 6 项原有警告 |
| 独立契约审核 | 两项测试建议已关闭，复审无未解决项；五维度复核通过 |

10 个变异分别为：半开上界包含、短窗边缘重复、负偏移截零、遗漏 token 过滤、None 模型改成字面“未知”、容量允许 +1、错误不销毁工作集、忽略取消、截止等号放行、跳过零 token 消费。源码及 target 均位于 `/tmp/quota-throttle-sgm4-b3-query-mutants.bZHhwT`，没有在主树试写错误实现。

12 个默认忽略项中，11 项性能已单独执行；另 1 项显式原生 NewAPI 进程门禁本步没有重复启动。其相关 HTTP/worker 源码零变更，最近隔离执行证据保留于 [B2 审核](batch-b2-audit.md)，不将旧证据冒充本次运行。release 全量首次执行请求的权限审核超时、进程未启动；按工具提示重试同一命令后执行成功，不是测试失败。

日志目录 `/tmp/quota-throttle-sgm4-b3-query-checks.EBrT83/`，临时日志可能被系统清理；复现入口为 `scripts/contract_audit/sgm4_b3_query.sh`、`scripts/tests/performance-gates.sh` 和默认 cargo test。全量保持默认并发，无 RUST_TEST_THREADS/test-threads 覆盖；性能在其余测试结束后单独串行执行，没有重编正在运行的同一测试可执行文件。既有 CI 调用性能 runner，因此新增入口已进入其检查清单；本轮未触发远端 CI。

## 运行隔离

开发和验证在 `research/source-groups-token-management` 的专用 worktree。未读取生产配置、数据库、日志、key 或环境，未访问运行服务/供应商，未重启、部署、推送或合并。此步完成后仍需只读数据库组合和历史补归属，再进入三页面及发布工作。
