# SGM-4 B2 统一采集事前契约清单

日期：2026-10-17。设计基线 `95374da`，实施基线 `812b190`；用户已确认，以下保留为事前期待。B2.0–B2.3 已完成本地退出验收；结果另见 [审核记录](batch-b2-audit.md)，不能从结果倒改期待。

## 1 契约源

- [B2 函数细化](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b2.md)：范围、默认值、页契约、两条持久检查点、真实能力、并发/恢复和退出门槛。
- [B 总体及 B1](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b.md)：数据集约束、A/B1 复用、v5 原子入账、完整性边界。
- [A](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics.md)：规范事实白名单、精确整数、类型过滤、历史归属及联合用量。
- [架构](../../design/source-groups-token-management/architecture.md)：G5/G6、一个单写者/应用库、原生转发与 GLM 不改、服务不接线。
- [能力研究](../../research/source-groups-facts-capability-research.md)、[workflow](../../workflow.md) §4.3/4.4/6、原 [preflight 契约](../../design/source-groups-token-management/subplans/sgm-2-channel-reconcile.md)：能力前提及不回退既有路由门禁。

## 2 Schema 字段

数据库仍 v5，旧 v1–v5 DDL 全冻结，CatalogSnapshot/OperationView wire 不改；没有公开 HTTP JSON，新 wire schema=N/A。输入 API 仍需机械测试字段类型及缺失/重复逻辑。

| 对象 | 必核字段和限制 |
| --- | --- |
| 日志 query | 固定 /api/log/、type=0、p、page_size=100、start/end；只有数字，不接受任意过滤或 URL |
| 页外壳 | success=true，data.page/page_size 与请求匹配，total 非负安全整数，items≤100；未知字段不入库 |
| 原始行分页键 | 所有类型均核 id 正 i64、created_at 非负安全整数且在窗、type 非负整数、页内严格倒序及 ID 唯一 |
| CheckedLogPage | raw_count/total/first_key/last_key/PageEnd/rows；原始 Value 仅单页内存，无 Debug/serde |
| LocalLogLease | 私有真实进程快照、检查起点/期限；不可从 bool 或前端 JSON 构造；不是数据库代次 |
| CollectionReport | 最多两条进度、当轮已提交计数、固定 issue/pagination_changed；无完整性授权或秘密 |

restart_scan 只改变当前 checkpoint 的 generation/page/phase/observed/fault 与 ledger_revision，不改变 window、initial_requested_from、事实/计量/归属。

## 3 枚举与常量

- import 既有 ScanLane/ScanPhase/ScanFault/PageEnd、FactError、JSON_SAFE、RequestClass/ManagementErrorKind 和响应上限，不复制定义。
- 新固定类型严格沿 B2 §5，尤其 SqliteUnverified 与 LocalBinding/Expired 分开；不加入 FinalOutcome/Ready/Verified/Complete 之类替代证据。
- 默认值只有一处定义：31 天初始范围、300 秒近期回看、100 行页、4 次逻辑页尝试、轮次结束后 10 秒、预检 20 秒/逻辑页 25 秒/轮次 60 秒/lease 60 秒。
- 业务枚举不扩展，v5 存储 fault 集合不改；新增采集内存错误不写成供应商错误原因。

## 4 流程步骤

后续代码使用 `Step B2.n` 标记，与函数细化逐项核对。

| 标记 | 必需顺序 |
| --- | --- |
| B2.1 | 校验窗口/页算术→固定 query→完整页外壳和全 raw 行校验→按 raw_count 判断 PageEnd |
| B2.2 | 初始化/时间/致命故障先验→分 lane/phase 规划→保留历史起点及未完成固定窗 |
| B2.3 | usable/IMMEDIATE→matched 完整票据→checked 新代次→仅 checkpoint/revision→commit |
| B2.4 | 托管绑定前验→有限启动证据→预算内版本检查→绑定后验→短期 lease；原路由预检不弱化 |
| B2.5 | 发送前 guard→共享 transport/单次鉴权恢复→页 decoder；无 collector 业务重试套娃 |
| B2.6 | 一个槽/单飞→期限/能力/状态→至多 4 次尝试→页前后绑定/时钟→B1 原子提交→报告/下次到期 |
| B2.7 | 显式后台入口→可取消轮次→结束后等待→shutdown；不积压 tick，不自动接 CLI |

## 5 行为契约

- 初次默认 from=max(0,now−31 天)，重启不移动它；可在首次显式初始化选择更早起点，无事后静默修改。
- Recent 新窗回看 300 秒；History 新窗从持久起点重扫到 now；活动窗口截止不随轮次变化。慢历史不截断为最近 100 条或 101 页。
- 满 100 条继续、短页结束；无关日志类型仍占 raw_count。total 是提示，不用于提前结束或强制 count/页一致。
- 仅页内乱序/重复为非法页；跨页重叠/total 变化可以继续去重，报告变化并在后续历史遍历再次访问，不每次变化都重启造成饥饿。
- 同计数替换可能检测不到；反例必须体现“observed≠complete”，不得期待采集器凭空检测全部删除或补出被删事实。
- 缺失/超限/坏核心字段拒绝全页，不能默认零或跳过坏行。A 的可选 metadata/标签降级规则保持，不升级为整页故障。
- PID 正常重启丢弃旧响应，重新真实预检后同窗第一页恢复；更换数据集不换 UUID 自动合并。同 ID 异值停两条方向。
- 保守 SQLite 前提是同一进程初始 SQL_DSN/LOG_SQL_DSN 各一次且均为 local。缺少证据返回无法确认，不指称实际一定非 SQLite，不读取原生日志库验证。
- 新日志能力不依赖 RecoveryDisabled；原路由能力仍必须拒绝 RecoveryEnabled/Unknown。

## 6 时序和状态

- begin_scan 重放不改原语义；restart_scan 只接受完整匹配 Scanning ticket，生成新代次。Stalled 仍同窗 begin_scan。
- 进程/采集器重开及取消丢弃在途页后从第一页重扫；跨普通轮次可按已接纳代次续页。已经入账的页保留。
- 同 worker 启动新后台生命周期时清 pagination_changed，同生命周期内保留累积告警；Busy/启动前shutdown不清告警。仅诊断变化，不修改持久数据、覆盖判断、路由或 next_due。
- 两条方向交错，next_lane 在尝试前改变，慢/失败方向不总占首位；每 lane 每轮至多开一个新窗口。
- 单飞/commands→短 Core 的锁序，所有 await 无 Core/协调 lane；写桥接保留精确 LedgerError，poisoned 不重新打开发送 gate。
- 每页单独事务，轮次可以部分成功；ACK 丢失、旧票据和未知提交不能通过重复加算修复。
- 429 结束本轮并共享冷却；已知 AuthFailed 不自行调用 retry_authentication。判断 status=429 不依赖正文是否可解析。
- Clock 回拨不伪造时间落诊断；不能清 checkpoint 时刻再用较早 now 绕过门禁。
- 取消释放槽/锁并使在途 lane 下次重扫；同步提交可能已经完成，取消不承诺回滚它。next_due 防取消重入/多个后台入口绕过 10 秒间隔。
- dry_run 零账本写/零网络；缺 capability 不初始化完整零。普通显式重试不能清身份冲突或换窗。

## 7 属性和反例测试

每项至少 128 cases。参考模型用独立列表/集合/整数，不调用被测 plan_scan/decoder/投影构造期望；保留失败种子。

| 编号 | 随机空间 | 核心不变量 |
| --- | --- | --- |
| PB1 页边界 | 秒/页码/整数边界、0/99/100/101 行、混合类型、坏字段/重复/乱序 | 合法 query 端点等价；全 raw 行受检；坏页无入账；末页不按筛后长度/total |
| PB2 窗口计划 | 初始起点、离线时长、两条 phase、代次、跨秒、时间回拨 | 持久 from 不变；未结束只同窗；无非法窗口/伪造时间；历史不丢初始区间 |
| PB3 恢复事务 | 中断点、旧票据、重开、page/generation 上界、SQL/commit 故障 | 重扫只改检查点/revision；确定失败回滚，未知提交原子且 poisoned；去重计量守恒 |
| PB4 分页变化 | 插入/删除/同计数替换、跨页重复、>300 秒迟到、混合日志、有限最终静止列表 | 已收到规范事实只计一次，未收到不伪造；在保留记录且最终静止并获得足够轮次时后续历史遍历取得该范围剩余事实 |
| PB5 并发与预算 | 两 lane、重复入口、暂停控制请求、401/429、取消/期限、队列满 | 单飞/有界请求数/next_due、取消无槽泄漏、控制优先；历史在条件满足时得到机会 |
| PB6 绑定与安全 | PID/启动代次/URL/版本变化、环境缺失/重复/边界、跨请求换绑定 | 无生产者证据不入账；旧进程页不入新遍历；秘密不进 report/error/持久库；不更改策略或渠道 |

确定性门槛还包括：100 条均为无关日志的满页、空页但 total>0、total=0 但合法非空页、id>JSON_SAFE、to=1、页码超出支持域、同 ID 异值触发全局停止、本页最后事实失败、两 lane 相同事实、控制写使 publication 变化但正常统计读不被无谓撤销。page≤JSON_SAFE 且页大小 100 时乘积不可能溢出 i64；仍保留 checked 乘法，不为了触达不可达分支放宽页码域。

原生隔离探针必须提供 >200 条混合类型记录、同秒多 ID、至少三页及空尾页，逐项核 ID/排序/边界；查用户日志作展示 ID 的负向对照。只用假上游和探针自建临时 SQLite；若精确秒边界需种数据，只能在自建进程停止后向该空测试库写合成行，再重启自建进程，探针不承担生产采集的数据库读取实现。显式 local 值与冲突 .env 组合也须验收；实际 Rust producer 还需与自建进程做组合验证。

可控 HTTP 假服务覆盖 Count/Find 期间变化、并发重复轮次、插入/删除/同计数替换、迟到、401/429（含坏正文）、半响应、页前后 PID 改变及各取消点。同步用 barrier/Notify/可控时钟，不用短 sleep 假定任务已到达。

至少 10 个可编译的定向变异：按筛后长度停页、按 total 停页、忽略页码回显、半开上界错一秒、重开续旧 offset、重启移动初始起点、重扫不提升代次、只查页前绑定、无证据接纳 SQLite、绕过单飞/4 页限制。源码/target 双隔离；断言失败才算捕获。必要时增加 next_due/取消/路由门禁的变异，不以编译失败替代。

## 8 性能和资源

- 每页≤100 行、body≤8 MiB，环境读取≤1 MiB，采集状态固定两 lane；不能在采集器缓存全历史。账本容量沿 B1，不能为预算删历史。
- 每轮≤4 次逻辑页尝试；额外能力状态 GET 和鉴权物理请求单列计数，均过同一 ManagementBudget；不把逻辑页数冒充物理请求总量。
- 时间门禁采用可控时钟验证 20/25/60 秒和完成后 10 秒；真实 HTTP 每次沿 transport 10 秒。普通默认并发测试不夹短墙钟性能断言。
- 独立串行 release 新增门禁：100 行/10000 历史映射的单页解析加事务≤2 秒；10 万事实下 restart_scan 加读取两 checkpoint≤1 秒，核不全扫事实。原 B1 的 1024 行/5 秒、10 万重开/10 秒及其余门禁保持。
- 对自建 10 万/20 万历史记录测完整分页请求数、页尾 offset 延迟和常驻内存趋势，报告全窗重扫成本；不把 20 万当产品上限或假定生产吞吐。
- 有限 HTTP/租期截止不意味着能中止已在运行的同步 SQLite 提交。性能入口须纳入串行 runner/CI，不能仅标 ignored 后跳过。

## 9 安全和允许范围

允许页解析/计划器、catalog 新重扫方法、preflight 共用 helper 和日志能力入口、worker 私有桥接/任务函数、测试/探针/机械与性能 runner/文档。零修改：v1–v5 DDL、GLM 算法、火山规则、生产认证行为、现行 recent_logs、CLI/面板/boot 启动行为、配置和依赖。

所有测试用临时应用库、回环假服务和随机非 3000/3001 端口，原生进程管理写前验监听归属，finally 仅停自建 PID。不读生产配置/数据库/日志/key/环境，不请求供应商，不重启/部署/推送/合并。API/env/路径秘密哨兵与负 Debug/serde 测试必需；原始环境仅短暂有界内存，不写诊断。

## 10 对照和退出

机械检查覆盖 Step B2.1–B2.7、共享类型/常量、零 DDL/业务改动、负 trait、用例确实执行、默认 debug/release 全量、clippy/release、全部独立性能入口。保留 B1/A、原 preflight 和认证回归，不能用更简单夹具或放宽超时掩盖失败。

实现后按 workflow §6 给独立 subagent 契约及 diff，隐藏实现位置列；关闭 ❌、处置 ⚠️ 后记录 batch-b2-audit/decisions，再做五维度复核。最终执行记录：40 项新增入口（38 普通/2 性能）、6 组各 128 cases PBT、15 种变异通过；机械、三步独审及最终五维度完成，debug/release 各 445 passed/0 failed/11 ignored。10 项独立性能与 1 项原生隔离门禁另行通过；具体证据保存在审核记录，本清单保留为事前期待。B2 完成后仍不代表 B3/C、三页面或部署完成。
