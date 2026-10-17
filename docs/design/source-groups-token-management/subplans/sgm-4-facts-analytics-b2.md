# SGM-4 B2 统一采集细化方案

日期：2026-10-17。基线 `95374da`，状态：待确认。承接已验收的 [B1 事务账本](sgm-4-facts-analytics-b.md)，本轮只做设计，不接运行服务。确认后先做隔离能力探针，再按本文件的小步实现；探针与实现验收结果另记，不把方案当作通过证据。

## 1 范围和实施顺序

B2 实现一个全局日志采集器，将 NewAPI 管理日志经 A 校验、B1 去重和事务投影写入同一个应用库。近期采集和历史补采是它的两条工作方向，不按来源、组、访问 key、模型或浏览器增加轮询。对应架构 G5 的统计基础和 G6 的恢复；不改变 GLM 或火山业务规则，不产生 FinalOutcome、VolcanoEvidence 或 RoutingPermit。

| 小步 | 交付与退出 |
| --- | --- |
| B2.0 能力探针 | 固定版本隔离实例验证管理员多页、时间端点、稳定 ID、显式 SQLite 启动条件；成功后才实现依赖这些能力的生产入口 |
| B2.1 页契约与重扫 | 严格页解析、纯窗口计划器、catalog 显式同窗重扫、对应 PBT 和故障测试 |
| B2.2 本地采集能力 | 复用托管进程预检，独立的日志读取证据和预算内只读请求；不混用路由就绪条件 |
| B2.3 单飞与组合 | worker 单写者适配、有限轮次、后台任务函数、取消/故障恢复、受控分页变化测试、机械检查和独审 |

每小步实现、测试、审核后报告，再进入下一小步。up/run/sync、现行 recent_logs/面板、生产配置及 boot 启动行为本批不改；只提供尚未接入运行入口的函数。正式发布由 SGM-7 显式替换旧采集，不能同时保留两套周期采集。B3 的查询/补归属、C 的终态和火山接线仍单独实施。

## 2 复用和最小变更

- 保留 catalog v5 和两条 ledger_checkpoints，不加 DDL、任务表、历史窗口队列、数据库身份登记或新的 HTTP 客户端。历史整窗扫描的位置已经由 History 的 window/page 表达。
- 复用 B1 的初始化、开始、入账、诊断事务；仅新增 `restart_scan`，解决活动 Scanning 无法显式回到第一页的问题。原 `begin_scan` 的重放行为不改。
- 复用 A 的事实解码和计量；B2 只解析页外壳及所有原始行的分页键。消费/错误字段仍由 B1 调 A 验证，不抄第二套事实 decoder。
- 复用 ManagementTransport 的鉴权、限流、冷却、连接池和响应大小限制；采集只用 Read。Observe/Backfill 共用每秒 1 请求、burst=2、队列 64、总在途 2/背景在途 1 的预算。
- worker 新增一个采集单飞锁和有限运行状态，复用 commands 一个槽及 Core 唯一写者。目录事务不跨网络 await；不逐页调用策略、不改组选择或渠道目标。
- 本地进程的 PID/启动代次/exe/cwd/监听归属检查从现有 preflight 中共用；原路由预检的 RecoveryDisabled 条件和调用行为必须保持。日志能力使用自己的证据，不能要求为统计关闭自动恢复。

## 3 原生能力和支持前提

### 3.1 API 与数据集

固定版本 `v1.0.0-rc.20`、提交 `6ce7305cd36f16506fb6a2c3c524a5a318539ba7`。只调用管理员 `GET /api/log/`，参数为 type=0、p、page_size=100、start_timestamp、end_timestamp，不加 group/channel/token/model 过滤。管理员路由不同于重新分配展示 ID 的用户日志；SQLite 管理查询按 `(created_at DESC,id DESC)`，时间两端包含，Count 与取页分开。[路由](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/router/api-router.go#L267)、[日志查询](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/log.go#L447)

半开窗口 `[from,to)` 编码为 start=from、end=to−1。时间 0 表示该端不过滤，所以拒绝 to=1；from=0 在本契约的非负时间域等价于不设下界，仍逐行校验窗口。页码从 1 开始；必须 checked 计算 `(p−1)×100`，不溢出受支持 64 位后端的整数。没有 101 页/10000 行历史总量上限。

沿用 B 总体方案的数据集前提：一个 catalog 对应同一原生日志数据集，已存原生规范事实及 ID 不改写。PID 改变不等于数据库改变；PID 相同也不证明数据库未换。catalog UUID、页 total、最大 ID 都不作为数据库代次证明。同 ID 异值触发全采集停滞；不一致恢复/替换走 SGM-7，不自动清事实或换 UUID 合并。删除尚未取得的日志无法从该 API 恢复。

### 3.2 SQLite 需要真实启动证据

本地 boot 只显式设置 PORT，继承其他环境。固定 NewAPI 启动先加载 `.env`，之后用 SQL_DSN 和 LOG_SQL_DSN 选择主库/日志库；日志库可以不同于主库。不能根据工作目录有 one-api.db 或 `/api/status` 返回正确版本就认定日志来自 SQLite。[本项目启动](../../../../src/boot.rs)、[原生初始化](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/main.go)、[数据库选择](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/main.go)

本批建议的最小可验证前提是：同一个已核对的 Linux 托管进程，其启动环境中 **SQL_DSN=local 且 LOG_SQL_DSN=local**。读取 `/proc/<pid>/environ` 时有 1 MiB 上限，只判这两个键各出现一次且精确相等；不打印、不存储、不返回其他环境内容。读取失败、超限、重复或缺失都返回 SqliteUnverified，不猜默认值。该显式非空值由固定版本选为 SQLite；所用 godotenv 1.5.1 的 Load 不覆盖已存在环境变量，`.env` 不能改变这两个选择。[加载器实现](https://github.com/joho/godotenv/blob/v1.5.1/godotenv.go#L184)

这是保守的支持门禁：缺少这两个值不等于当前服务不是 SQLite，只代表该方法无法确认。也不把进程环境当作防恶意同用户篡改的安全证明。B2 不修改启动环境、不读取实际生产环境，不因缺证据自动重启；若发布时缺少条件，由 SGM-7 在获得窗口授权后明确设置并重启。替代证据方案须先补能力验证，不能偷偷读 NewAPI 数据库。

预检还须确认：客户端 URL 与既有托管配置相同、回环 HTTP/端口一致；PID、启动代次、exe、cwd 和唯一监听归属前后相同；配置/运行版本符合现有固定版本规则（v0.0.0 仅接受既有已知 SHA256）。实际成功读取管理员日志页才证明当前身份有日志权限。没有新身份注册、凭据探测请求或系统实例登记。只读探针不要求 RecoveryDisabled，也不读取全 options。

## 4 调度和覆盖语义

### 4.1 两条检查点足够表达首版计划

建议默认：首次起点为 `max(0, now−31×86400)`；Recent 回看 300 秒；每轮至多 4 次逻辑日志读，轮次结束后至少等 10 秒。now 是合法 UTC 整秒且至少为 2，当前秒不纳入固定截止时间；不满足时初始化前即拒绝。首次显式初始化可指定更早 from；初始化后不因重启重新计算或移动起点。本批不增加改变已有起点的管理 API。

| lane/状态 | 下一动作 |
| --- | --- |
| Recent Idle 或上窗 Scanned | 建立 `[max(initial_from,now−300),now)`；now 不前进或为空则本轮无新窗 |
| History Idle 或上窗 Scanned | 建立 `[initial_from,now)` 的完整历史重扫；不只扫最近 31 天。now 不前进或为空则无新窗 |
| 任一 Scanning，当前采集会话已接受该代次 | 继续固定 window 和 next_page；不每轮改截止时间 |
| 任一 Scanning，采集器新启动/取消后恢复/本地进程正常重启 | 同窗显式 restart_scan，generation+1、page=1；不跳过未完成区间 |
| Stalled Transport/Clock | 能力和时钟恢复后，通过 begin_scan 同窗从 page=1 重新开始 |
| Stalled InvalidPage/InvalidFact/ResourceLimit/HistoryUnavailable | 不自动跳页或换窗，等待显式重试；重试也仅同窗第一页 |
| 任一 ConflictingIdentity/BindingChanged | 两条方向均不继续；须明确核对绑定/恢复，不用普通重试清除 |

一次轮次每 lane 最多开始一个新窗口；扫完后不会在同轮反复查空窗。活动窗口可以跨任意多个轮次，不设每窗页数截断。History 每遍从保存起点重扫到新的固定截止，能在后续遍历发现超过 5 分钟的迟到记录；长期离线期间仍在这个持久范围内。Recent 可先显示最近观测，离线缺口由 History 补采，不能把两窗之间尚未扫描的区间标为完整。

为了不让失败/慢近期任务永远排在历史前，轮次保存一个**内存** next_lane，准备尝试前即交替；即使本次超时也让另一条在下轮先尝试。两条都可运行且预算有进展时交错读取；某条完成或停止，其剩余机会可给另一条。页数预算包括失败尝试，最多 4；没有可运行方向即退出。进程无限频繁重启或控制请求持续占满预算时不承诺补采完成，状态必须显示滞后。

历史全窗重扫是首版的明确取舍：无需额外调度持久化，全部迟到范围可被再次访问；代价是历史越多，重扫延迟和 offset 成本越高。只承诺每轮资源有界，不承诺任意流量下 10 秒采齐；记录 10 万/20 万行分页耗时与请求数，若无法满足实际规模，再讨论分窗或游标能力，不先增加多队列框架。

### 4.2 分页移动不会伪装成完整

每页必须核对外壳与请求页码/页大小、total 非负安全整数、items 数组≤100，以及**全部原始行**的正 i64 ID、非负安全时刻、非负整数 type、窗口和页内严格倒序。ID 保留 i64，不误限为 JSON_SAFE；同页重复 ID 或乱序为 InvalidPage。未知日志类型可由 A 忽略，但仍占原始页长度并参与分页校验。

原始行数=100 时继续下一页；小于 100（含空）时仅标记本遍 Scanned。不按 A 过滤后行数判断末页；不按 total 提前停页，也不强求 total 与取页长度恰好一致。

跨页 total 变化或本页首键不小于上页末键表示观察到分页移动。保存于当前轮报告/会话的固定诊断 `pagination_changed`，不保存原始行；仍可将合法页交 B1 去重并继续遍历，避免每次插入就无限回到第一页。该诊断不是“漏了多少条”，进程重启不恢复它，也不因诊断缺失宣称稳定。删除、同计数替换可能未被发现；后续全窗重扫提供再次发现机会，不能补出已清除记录。

统一口径始终是“已观测用量；API 无快照完整性证明”。两遍相同、没有检测到变化、扫描完成、空桶均不表示完整覆盖；不生成 coverage_end 或最终请求数。是否启用原生消费/错误日志及其保留期属于发布检查；B2 不根据空页判断记录开关，也不自动开启它们。

### 4.3 时间、退避和错误

开轮及入账前都校验 now；now 小于任何持久窗口 to/last_observed_at 或本会话已见时间时暂停，不用 clamp 伪造观测时间。回拨时若不满足 B1 诊断时间前置条件，只发布内存 Clock 状态，不能为了落诊断再损坏持久时序；时钟追上后继续/同窗重扫。

普通网络失败保存该 lane 的 Transport，下一轮同窗重扫；429 立即结束本轮并服从 transport 的共享冷却。鉴权失败沿既有熔断，只允许既有显式 retry_authentication 恢复，不新增重复登录。QueueFull/发送前取消不必写失败，保留页；下次可继续已接受代次，丢弃过在途页则标记需同窗重扫。解析/计量/容量错误不得跳页。HTTP 404/拒绝返回固定错误，不据此断言历史被清理。

同 ID 异值保存 ConflictingIdentity 并停止两条方向。确定的单页失败不影响之前已提交页；mark_scan_fault 是独立诊断事务，不伪装成失败页的一部分。CommitUnknown、库损坏或无法写入时不再补写诊断，保留 poisoned/Storage 状态，重开严格验证后才恢复。不自动清坏事实、历史或统计。

## 5 私有类型和接口

| 类型 | 字段、生命周期及约束 |
| --- | --- |
| LogPageQuery | window、page；构造后私有，只能编码固定管理路径、固定 page_size/type，无用户文本过滤 |
| CheckedLogPage | rows、raw_count、total、first_key/last_key、PageEnd；最多 100 行，仅本次内存持有，不实现 Debug/serde，不是规范事实或完整性证明 |
| LocalLogLease | 私有 ProcessSnapshot、验证起止时刻/单调期限；只由真实预检构造，不可反序列化，不表示数据集代次；最长 60 秒 |
| LogCapabilityIssue | LocalBinding/UnsupportedVersion/SqliteUnverified/Management(kind)/Expired；无路径、环境值或响应正文 |
| ScanAction | Skip/Begin(window)/Continue/RestartSameWindow/RetrySameWindow/Blocked(fault)；纯计划器输出，不直接调用 SQL/HTTP |
| CollectorSession | 每 lane 已接受代次、上一页键/total、是否需重扫；next_lane、上次墙钟、可空 lease、单调 next_due、固定诊断；至多两份 lane 状态，取消靠 RAII 标记，非持久 |
| CollectionReport | 本轮逻辑页尝试/提交数、inserted/duplicates、安全 LedgerState、固定 issue、pagination_changed；至多两条进度，不含原始日志或成功路由声明 |
| CollectionError | Ledger(LedgerError)/Control(ControlErrorCode)/Capability(LogCapabilityIssue)/Clock/Busy/InvalidPage；固定码，无底层自由文本 |

所有新增类型 crate 私有，不新增面板 wire schema。已有 ScanLane/ScanFault/PageEnd 等只 import，不重复定义。CollectionReport 的已观测属性不允许转换成策略 Ready；B3 再设计公开状态投影。

## 6 函数细化

### B2.1 页请求和解析

`LogPageQuery::new(window, page) -> Result<Self, CollectionError>` 校验 B1 数值域、to≥2、page≥1 和 offset 的 checked 乘法；失败无 IO，成功可 `into_request()` 构造固定路径的 ManagementRequest(Read, LIST_BODY_LIMIT)。先满足 ManagementRequest 的相对 /api 路径及大小前置条件，只有十进制数字参与 query，不接受任意 URL。正确性：半开区间的整数等价转换与显式拒绝不可表达端点，保证请求不意外变成无时间限制；固定字段不会泄漏标签或 key。

`decode_log_page(body: Value, query: &LogPageQuery) -> Result<CheckedLogPage, CollectionError>` 先核 success=true/data 对象和页外壳，再逐行检查 §4.2 的公共键；每步失败返回 InvalidPage，不返回部分页。未知响应字段忽略而不持久化。页内维护上一键和 ID 集合，最多 100 次；严格序与 ID 唯一分别检查，不能只检查复合键。最后按 raw_count 构造 PageEnd；消费/错误其余字段由 B1/A 后验。正确性：完整页校验先于入账，所有控制翻页的原始行均受约束，忽略业务类型不会提前停页；循环上界和 8 MiB body 上界独立生效。

`observe_page_change(previous, page) -> bool` 仅比较上次同 lane/generation/window 的 total、末键与当前首键；无 previous 或任一页空时仅比较可用部分。无 IO，常数步骤；返回 true 只追加固定诊断，不回写事实、不代表完整性反证的精确数量。

### B2.2 纯窗口计划器

`plan_scan(state: &LedgerState, lane, session, now) -> Result<ScanAction, CollectionError>` 先核初始化/两个 lane 状态及 §4.3 时间；若未初始化，调用方单独执行 B1 initialize 后再规划。任一致命持久 fault 先阻止全部扫描。然后按 §4.1 表分支：活动且会话接纳→Continue；未接纳→RestartSameWindow；可重试 Stalled→RetrySameWindow；不可自动重试→Blocked；Idle/Scanned→checked 生成固定新窗或 Skip。不得把历史下界改为滚动 31 天。

正确性：状态输入合法和 now 有界使所有差值/窗口可校验；分支穷尽四种 phase，只有 Scanned/Idle 可换窗，活动/失败恢复保留原窗，故不会因重启遗失整个初始区间。有限两条状态，无无界循环、无副作用。更早首次 from 是初始化参数，不是无条件覆盖已存 from。

### B2.3 显式同窗重扫

`CatalogStore::restart_scan(ticket: &ScanTicket, now) -> LedgerResult<ScanTicket>`：usable→合法 now→IMMEDIATE 事务→initialized/matched 完整比对 ticket→要求 now≥旧 window.to/last_observed_at→generation checked+1、page=1、phase=Scanning、observed/fault 清空→同事务 bump ledger_revision→构造新票据→既有 commit。只接受当前 Scanning；Stalled 仍走 begin_scan。失败无已确认部分写；未知提交保持 poisoned。

正确性：matched 的 namespace/lane/代次/window/page/phase/时间比对先于更新，新代次排除旧在途回执；同窗和原始事实/投影不变，重扫由 B1 持久去重防重计。无网络/无循环；溢出不提交。原 begin_scan 同窗重放仍返回原票据，不能把重试 begin 偷改为每次清页。该新增接口及原调用者边界纳入 B1 回归，不修改 v5 枚举或 DDL。

### B2.4 本地日志读取证据

`NewApiClient::check_local_logs(process: &NewApiProcess) -> Result<LocalLogLease, LogCapabilityIssue>`：在最长 20 秒总期限内，依次核 local_target→PID及进程快照→有界启动环境检查→共享 Observe 预算读取 /api/status 并核版本/必要旧构建 hash→再次 PID/进程快照相等→构造最长 60 秒 lease。时间从检查开始计，不用完成时间续租。私有 `sqlite_startup_evidence(pid) -> Result<(), LogCapabilityIssue>` 读取最多 1 MiB+1，逐 NUL 项扫描，只保留两个局部判断；缺/重复/非法/IO 统一 SqliteUnverified。扫描由字节上限终止，hash 沿原 256 MiB 上限。不检查原生自动恢复，不做渠道列表请求。

`recheck_local_logs(process, lease) -> Result<(), LogCapabilityIssue>`：核 lease 未到期且墙钟未回拨，重读 PID/进程快照并比较；超时/取消/变化失效证据。每页请求前后检查，不只开轮检查；同一个 live PID 的 exe/cwd/监听归属仍须一致。可以共用 helper，不缓存跨进程证据。

正确性：已核固定程序、初始非空 local 值和不覆盖式加载构成 SQLite 分支前提；前后同进程检查避免明显响应换绑。此证据不防外部静默换库，不证明日志完整；依赖 §3.1 已声明的运行约束。PID正常重启后旧页丢弃，重新成功预检才可同窗第一页恢复；配置目录/URL/版本变更不自动接受。所有失败无 catalog/远端管理写入，仅现有鉴权可发 login。

共用提取会影响原 `probe_pid/check_local_backend` 的 callee 路径，须补等价测试：原 RecoveryEnabled/Unknown 拒绝和租期不变；采集仅日志条件不能放宽路由许可。

### B2.5 有界单页读取

`NewApiClient::read_log_page(query, lease, class, guard) -> Result<CheckedLogPage, CollectionError>`：只允许 Recent→Observe、History→Backfill；构造 B2.1 Read 请求，调用 send_fenced，检查后交 decoder。guard 在最终准入和 401 后重放时检查取消与 lease 时限，不获取 Core；不能把 worker 每次 publication 改变当成日志读取作废条件。

一次逻辑读取最多沿 transport 发生一次 401 重放，所有物理请求和 login 均计共享预算；B2 不额外包业务重试。HTTP 执行沿既有 10 秒，调用方再设最长 25 秒含排队/鉴权的单页截止，超时丢弃页。ManagementError 保留固定 kind/status；429 看 status=429 优先结束本轮，即使错误正文超限也不当成可立即再读。正确性：请求可重复读取，持久有效一次由 B1 身份去重实现，不宣称网络 exactly-once；页未确认不推进检查点。

### B2.6 worker 事务桥接与单轮

`ledger_mutation(f) -> Result<T, CollectionError>` 使用既有 mutate(None, closure)，closure 返回嵌套 LedgerResult 以保留精确 LedgerError，不把身份冲突抹成通用 Storage；外层映射 ControlError。只在内部闭包调用 catalog 初始化/开始/重扫/提交/诊断。复用 mutate 在写前封 publication、可用时写后解封、poisoned 时保持关闭的保证；不更新 input_epoch、许可或 last_glm_cycle。读取状态用短 Core 锁，不走写 publication。

正确性：内层失败不转换为事务成功，外层封门/解封仍依据 Store usable 而非内层 Result 的外包装；因此错误类型保留且未知提交不重新授予发送许可。闭包之外只有既有 publication/notify 副作用，无新增业务写入。

`ReconcileWorker::collect_facts_once(process) -> Result<CollectionReport, CollectionError>`：

1. 若 dry_run，返回未执行报告，零网络/零账本写；否则 try_acquire 一个 commands 槽，再 try_lock 单飞。已有采集或容量满→Busy，无排队新任务。检查会话 next_due，未到期→安全 no-op。
2. 设单调总截止 60 秒，安装取消 guard；获取真实 lease 并读 LedgerState。失败只报告固定问题，不初始化成完整零。初始化仅在空状态时调用 B1；已初始化使用原起点。
3. 每次循环先核停止/截止，选择并提前轮换 next_lane，按 B2.2 决定动作。Begin/Restart/Retry 经 B2.6 事务桥接取得新票据；Continue 重新读取当前票据。每 lane 最多开启一窗。动作/读取失败不得留在原分支空转：存储/致命错误退出，其他错误把该 lane 标为本轮不可运行；两条均不可运行则退出。
4. 在 Core 外重核绑定，计一次逻辑页尝试，使用 min(25 秒,本轮剩余期限) 调 B2.5；返回后再次核绑定、时钟和取消。失败不入账，按 §4.3 分类决定诊断、阻塞或结束；单 lane 的非全局故障允许另一条继续。
5. 持短 Core 锁，通过 B1 commit_fact_page 同事务入账/推进。成功后才更新会话已接受代次/页尾与安全计数；旧 ticket 冲突重读状态、标记该 lane 下轮重扫，不循环抢写。未知提交/损坏立即终止本轮，不发布成功。
6. 4 次尝试、无任务或总截止任一达到即结束；成功、错误和取消退出均把 next_due 设为此刻单调时间之后至少 10 秒，不能借 drop/再次调用绕过间隔。返回已提交部分及两条持久进度；没有“一轮全成才落库”的假原子性。取消 guard 只更新有界内存恢复标志/next_due 并释放锁/槽，不在 Drop 中写库；已经成功提交的页保留。

正确性：单飞确保同一 worker 没有并发采集，唯一写者和完整票据保证旧页不能覆盖新检查点；各页原子但轮次可部分成功。循环以 4 次尝试/有限两个不可运行方向为界，无空转；网络等待不持 Core，不持协调器的 lane 锁。取消发生在同步提交中时不能中断 SQLite 的原子操作，恢复以持久状态为准，不能声称取消回滚所有已落页。

`retry_fact_lane(lane, expected_generation, process)` 是尚未暴露到 UI 的显式重试入口：同一 commands/单飞协议，先核真实能力和时间，再核当前 Stalled/代次；仅允许 InvalidPage/InvalidFact/ResourceLimit/HistoryUnavailable 经 begin_scan 同窗重启。其他 phase 返回冲突；ConflictingIdentity/BindingChanged 不提供普通清除。失败保留故障及事实；成功只重启扫描，不宣布问题已消失。

正确性：检查先于写，单飞期间不能被第二采集入口改代次；最终持久 begin_scan 仍执行 generation/window 校验，故不能用失效重试请求覆盖较新故障或越过绑定冲突。有限检查后一次事务，无重试循环，也不绕过 next_due 发日志页。

### B2.7 后台生命周期和诊断

`run_fact_collection(process, shutdown: watch::Receiver<bool>)` 只由未来运行组装层显式调用，本批不接 CLI。开轮前检查 shutdown，调用 B2.6；select shutdown/截止使在途 future 取消。每轮结束后等待 next_due，不补发积压 tick；普通错误保留安全状态并等待，Storage/身份冲突保持阻塞而非高速重试。永久后台循环只靠 shutdown/致命存储错误终止；每一轮和睡眠都可取消。

正确性：轮次不 spawn 脱离生命周期的采集任务；shutdown 丢弃当前 future 后由其 guard 释放资源，重启只读持久状态。60 秒 timeout_at 覆盖轮次全部 await（含页前后检查），但不冒充同步提交硬中断；next_due 与单飞共同防重叠和补 tick 风暴。

状态只组合持久 LedgerState 与有限内存 issue；重启后 lease/页尾/变化诊断为未知，不能默认健康。单次 report 的 inserted 等是当轮已提交量，不是历史总量；总量由 B3 读取账本。错误格式不含日志原文、模型标签、环境/路径/SQL 或认证响应。

## 7 并发和跨进程契约

锁序为 commands 槽→采集单飞→短 Core；网络/进程检查时释放 Core。transport 的 Budget→auth 顺序不改，采集 guard 不反向拿 Core。来源控制仍使用自己的协调 lane；采集没有资格修改源、组、成员许可。

| 分布式维度 | 本批约定 |
| --- | --- |
| 连接/会话 | 原 reqwest 连接池与管理员会话；断线不清本地账本；不新建登录器 |
| 超时 | 原单 HTTP 10 秒；预检 20 秒；逻辑页最多 25 秒；轮次 60 秒；本地同步事务不能被异步截止强行中断 |
| 重试 | 401 单飞一次重放由 transport 负责；429 共享冷却；其他失败下一轮/显式同窗重扫，不嵌套立即重试 |
| 交付/顺序 | 页可重复或变化，单 lane 顺序处理；B1 log ID 去重使已收到规范事实有效一次，不能证明未收到事实完整 |
| 失败 | 管理不可达只停统计推进，原生转发不变；绑定失效丢弃页；存储未知停止写；无跨节点共识 |
| 状态 | 窗口/page/代次在同一 catalog；瞬态 lease/页比较不是恢复真值，重启同窗重扫 |
| 背压 | 单采集任务、一个 commands 槽、最多一条背景在途；每轮最多 4 个逻辑页尝试，不随实体数放大；控制优先，不承诺持续控制负载下补采有界延迟 |

“每轮 4 页”不是“所有 HTTP 总共 4 次”：另有至多一次 /api/status 能力刷新，以及 transport 可能的登录/401 重放；它们均走共享预算，物理次数另测。不为采集提升到 Control，登录例外沿既有规则。

## 8 验证和完整性审查

事前十节清单见 [B2 expectations](../../../audits/sgm-4-facts-analytics/batch-b2-expectations.md)。源码、集成测试、PBT、变异和机械门禁分别留下证据，workflow §6 的独立审核在实现后进行。

架构协作论证：页入口保证受支持接口及有界完整页；计划器保持固定窗口/持久起点；B1 为新增事实和进度原子提交；worker 限制并发与预算。因此可推出“已取得合法事实不重复计量、失败不跳过当前页、两条扫描可恢复”，不能推出 API 不提供的完整性/终态。外部前提是固定本地程序、未不一致换库、原生日志保留和可观测性；进展还依赖网络/控制预算最终可用及有限固定窗记录。

- [x] 函数、分支、退出、callee 前后条件和循环界限已列出；异步截止不被表述为同步提交硬实时中断。
- [x] 新增 restart_scan 明示 B1 增量契约，未偷改 begin_scan/DDL；共享数据结构随架构补记。
- [x] 近期/历史、公平性、全历史重扫成本、取消、时钟回拨、长期离线、解析/容量和未知提交均有处理路径。
- [x] 不把 total、空页、无变化、扫描结束或本地 UUID 当作完整覆盖/原生数据库身份。
- [ ] B2.0 原生多页/时间端点和启动条件实测；当前只有固定源码依据。
- [ ] 用户确认建议默认值及保守 SQLite 启动门禁；未确认前不编码后续入口。

当前只是细化及事前期待完成，不宣称 B2 通过实现验收或可以部署。
