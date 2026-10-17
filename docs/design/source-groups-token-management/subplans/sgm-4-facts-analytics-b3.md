# SGM-4 B3 查询与历史补归属细化

日期：2026-10-17。设计基线 `9d6a67d`，实施基线 `603f83f`。用户已确认，第一小步（函数 B3.1–B3.2）已完成本地验收，见[查询计算审核](../../../audits/sgm-4-facts-analytics/batch-b3-query-audit.md)；只读组合和补归属尚未实现。承接已验收的 A/B1/B2，交付统一账本上的查询和显式补归属入口，不接现行服务、面板或火山策略。

## 1 范围与分步顺序

对应[架构](../architecture.md) G5/G6 和 [B 总体](sgm-4-facts-analytics-b.md) §7。整体、来源、组及访问 key 的模型时序共用一个查询；历史未知映射通过小批事务补正，同一事实不再计费一次。

| 小步 | 交付 | 不包含 |
| --- | --- | --- |
| 一 查询语义 已验收 | 函数 B3.1–B3.2：区间/过滤/分桶、精确整数累计、一致结果和独立参考测试 | 网络、前端 JSON、Top-N 截断 |
| 二 只读组合 | 函数 B3.3–B3.4：同应用库 WAL 快照、有限扫描、取消/资源和目录锁生命周期 | 第二写者、连接池、缓存、运行接线 |
| 三 补归属 | 函数 B3.5–B3.7：有界事务、可重扫游标、worker 显式单步入口和组合验收 | 新任务表、另一轮询器、自动修改已知身份 |

每小步完成测试和审核再进入下一步。全批保持数据库 v5，冻结 v1–v5 DDL、Cargo 依赖/features、GLM/火山规则、NewAPI 客户端与 B2 采集算法、CLI/boot/现行面板。允许最小调整 `CatalogStore` 持锁对象的共享所有权，以及私有 ledger helper 可见性；不是重写存储层。

第一小步仅增加 ledger/analytics 内存查询类型和计算、模块声明及测试/门禁。QueryControl 此时只实现单调截止与取消标志；SQL 中断注册、目录 guard、worker 和持久补正留到后两步。累计入口借用 `UsageDimensions {source,group,token,model}`，无原始行复制；调用方只提交已经解码的消费计量，持久过滤/同快照生产者由第二步验证。任何 push 错误清掉累计工作集并锁存安全错误，后续 finish 不得吐出先前的部分结果。finish 的 state 必须来自已初始化账本，未初始化整体返回由第二步读入口处理。

第一小步先覆盖 PB1/PB2，另补分钟增量拆分等价、取消/错误全失败的属性测试，各 128 cases；PB3–PB6 中涉及数据库/worker 的部分不冒充本步完成。容量断言覆盖 1024 模型、4096 token 和 65536 桶模型项的边界及 +1；新增独立串行 release 的纯累计最大输出≤5秒门禁，随后完整查询仍须满足原 5秒预算。

## 2 复用与依据

- A 的 `UsageTotals/add_usage`、规范标签/归属校验和 `attribute_batch`；B1 的 `rows::{fact_row,cell_row,check_known,history,key,add_cell}`、`state/initialized/instance/bump`、commit/poisoned 路径。只提升必要私有可见性，不复制另一套 decoder。
- 现有 catalog 已启用 WAL/FULL。新连接只读同一个应用库，不是 NewAPI 库。独立连接的同一读事务保持一致快照，WAL 允许其他连接提交写事务；不能以此承诺读者没有 IO 或 checkpoint 成本。[SQLite 隔离](https://www.sqlite.org/isolation.html)
- rusqlite 0.40.2 已提供 `get_interrupt_handle`，无需新 feature。取消标志配合 interrupt；interrupt 不会可靠取消尚未开始的语句，近完成语句也可能正常结束，因此仍须每页/每行/发布前检查标志。[SQLite interrupt](https://www.sqlite.org/c3ref/interrupt.html)、[rusqlite 接口](https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html#method.get_interrupt_handle)
- `spawn_blocking` 只把同步工作移出异步执行线程，已经开始的任务不能靠丢弃 JoinHandle 强制停止。查询槽必须由实际执行任务持有至退出；本地锁定 Tokio 源码的行为与其[官方说明](https://docs.rs/tokio/1.48.0/tokio/task/fn.spawn_blocking.html)一致，不依赖线程强杀。
- 现有 [CatalogLock 修复](../../../fixes/catalog-fix-inherited-lock-release.md)保证连接先关闭、guard 后显式 unlock。B3 仅将 Store 和只读任务对同一 guard 的所有权改为 Arc；CatalogLock 本身仍不 Clone，不复制 File 来充当新的持锁所有者。

## 3 业务口径与资源界限

### 3.1 查询口径

内部查询接收显式 UTC 秒半开区间 `[from,to)`、固定 UTC 偏移分钟和 Hour/Day 粒度。`0≤from<to≤JSON_SAFE`，单次跨度最多 31 天；时区 `−720..=840`，与现有固定偏移值域一致，但绝不读取来源 reset_minute。调用方冻结 now 后可构造近 24 小时/近 30 天；指定自然日由该时区午夜到下一午夜转换。B3 不解析日期字符串或使用主机时区，不自动移动请求区间。

对 source/group/token/model 分别使用 Any、Unknown、Exact；不同维度作 AND。ID 为正 i64（不截成 JSON_SAFE），模型 Exact 复用 A 的非空/256 字节/无控制字符规则，原样匹配。合法但未出现的 ID 返回无已观测消费，不通过当前目录或 token 列表拒绝历史身份。Unknown 只匹配 None；字面模型“未知”与 None 不合并。原始空模型已由 A 规范为 None，不新增空字符串身份；空串 Exact 为非法输入。

一次成功查询同时返回过滤后的总计、全区间模型合计、各 token 合计及时间桶内模型分项。三类合计都等于总计；token=None 也参与。不同请求可能来自不同 revision，不能拼成同一快照。整体查询的一份 per-token 合计就是同区间 key 汇总；切换单 key 图时重新查询其完整结果，不把旧整体总量与新快照分项强行比较。

只计 Consume 的 input/output/consumption_rows；0+0 消费仍是一行，Error 只保留在事实库。模型和 token 排序按 Option 的 None 在前、已知值按 Rust Ord；桶按 UTC 起点升序。结果没有 token_name、request_id、原生 metadata 或来源凭据。模型名是数据，不作为 HTML/SQL/诊断文本。

所有结果均为“已观测用量”。尚未初始化账本返回 `usage=None` 和真实 LedgerState，不伪造零；初始化后无消费可返回计数零及空模型分项，仍不表示完整零。Scanned、补正遍历结束、无故障均不表示完整覆盖或来源可用。查询本身不请求 NewAPI、不启动采集，也不清采集故障。

### 3.2 资源与超界

| 资源 | 固定上限或行为 |
| --- | --- |
| 区间/桶 | 31 天，Hour 最多 745 个相交桶；Day 最多 32 个；只返回裁到请求范围的桶 |
| 只读 SQL 页 | 每次至多 256 行；按索引 keyset，不用 OFFSET 和全表 GROUP BY/SUM |
| 单次扫描 | 投影行加边缘原始行至多 1000000，过滤前计数；另允许一个判断有无下一行的探测值 |
| 输出 | 最多 1024 个模型身份、4096 个 token 身份、65536 个非空桶×模型项；None 各占一个身份 |
| 查询并发 | 每 worker 一个槽，try-acquire；忙时立即 Busy，不建立无限等待队列 |
| 查询截止 | 从准入起 10 秒，含阻塞池排队、打开连接、扫描与组装；取消/超时不返回部分结果 |
| SQLite 读等待 | busy_timeout=0，遇忙返回 Busy，不在查询内部反复重试 |
| 补归属 | 每次最多检查 256 个事实 ID，永久映射仍受 A 的 10000 条界限约束 |

这些是应用资源保护，不是供应商或总历史容量上限。超界报 LimitExceeded，调用方可缩小时间范围，或用带现有索引的 token 条件减少扫描；仅在内存过滤的 source/group/model 条件不保证减少扫描成本。不能悄悄丢模型、删除历史或把截断称为完整结果。首版不提供跨 HTTP 页的可变快照游标，不新增快照缓存：一个内部请求要么得到完整有界结果，要么失败。时间截止是合作式取消，不承诺能中断内核文件 IO 或已开始的同步提交。

## 4 共享与私有数据结构

下列 ledger/analytics 类型由 catalog 和 worker 消费，主架构同步其边界；全部为内部接口。带标签的请求/结果不实现 Debug/Display/Serialize/Deserialize，不直接成为页面 wire。错误可以安全格式化。

| 类型 | 字段与不变量 |
| --- | --- |
| UsageQuery | window: ScanWindow、utc_offset_minutes: i64、grain: UsageGrain、filter: UsageFilter；构造后私有字段，满足 §3 |
| UsageGrain / DimensionFilter<T> | Hour/Day；Any/Unknown/Exact(T)，只有一个定义 |
| UsageFilter | source: DimensionFilter<SourceId>、group: DimensionFilter<GroupId>、token: DimensionFilter<i64>、model: DimensionFilter<String> |
| UsageSnapshot | query、state: LedgerState、usage: Option<UsageReport>；state.revision 是整份结果的统计版本，未初始化 iff usage=None |
| UsageReport | total: UsageTotals、models: Vec<(Option<String>,UsageTotals)>、tokens: Vec<(Option<i64>,UsageTotals)>、buckets: Vec<UsageBucket>；分区守恒，无稠密 token×model×time 笛卡尔积 |
| UsageBucket | from/to 是裁剪后的 UTC 半开边界、total、models；桶连续覆盖请求区间；无消费桶的 models 为空 |
| AnalyticsError | Ledger(LedgerError)、Control(ControlErrorCode)、InvalidQuery、LimitExceeded、ArithmeticOverflow、Cancelled、TimedOut、Busy；无自由文本/标签/路径 |
| AttributionCursor | 私有 instance、after_id、through_id；0≤after≤through≤i64::MAX，through 为此遍开始时已存最大 ID；不提供 serde/前端构造 |
| AttributionBatch | checked、changed、ledger_revision、next: Option<AttributionCursor>；next=None 只表示此遍结束，不表示未知项为零 |
| AttributionReport | preview_only、checked、changed、ledger_revision、pass_finished；不含事实、凭据、路由成功或终态声明 |

私有 `UsageReadTarget` 含 catalog 的受检路径、instance 和同一 `Arc<CatalogLock>`；仅 Store 可生成。`UsageReader` 持有只读 Connection 与该 target，字段顺序保证连接先销毁。它们不公开路径、不可反序列化、不接受任意数据库名。私有 `QueryControl` 取消上下文包含原子取消标志、固定 Instant 截止和可选 `Arc<InterruptHandle>`；只能中断本查询连接，不触碰唯一写连接。

修正游标仅在 worker 内存保留；不写 v5 新字段。重启丢失游标从 0 重扫；仅丢失批次回执可重放旧 cursor，已补正行自然成为 no-op。初始 high-water 防止持续新增事实让本遍永远追不到末尾；下一遍重新取上界并从 0 开始，重新检查游标后方才出现映射的旧未知行。

## 5 错误和并发协议

查询输入非法在读库前拒绝；持久行坏值、非法组合、缺必需状态按既有 CorruptData；合法计数的查询合计超界为 ArithmeticOverflow。SQL 原始错误统一折叠到既有 CatalogError；取消标志优先映射 Cancelled、已到截止优先 TimedOut，非本查询取消导致的异常 interrupt 不能假称成功。

查询：try 查询槽 → 短 Core 取 read target/usable → 释放 Core → 阻塞任务打开只读连接、同一读事务完成全部结果 → 关闭连接 → 短 Core 再核 usable → 返回。不取 commands、协调 lane、probe 或 collector 锁；不调用 mutate，不改变 publication/input_epoch。写者普通变更不使有效旧快照失效；仅变成 poisoned/不可用时拒绝发布。Core 竞争也计入期限，采用 try_lock 与可取消异步让出，不在查询任务的 async 线程阻塞等待互斥锁。

查询槽的 OwnedSemaphorePermit 移入阻塞任务，直到实际清理完成才释放；超时/调用 future drop 只设置取消并 interrupt，不提前释放 permit。取消发生在阻塞任务尚未启动/尚未安装 handle 时，任务开始和安装后均检查标志，不再开始扫描。已取消的任务不会发布数据，即使 SQL 已经成功返回。每页读取前/每行/每次输出分配及最终发布都检查取消/截止；SQL 页只做有限索引读取，避免“interrupt 落在两条语句间”后执行不受界限的全表聚合。不得强行关闭另一个线程的连接。

只读 target 克隆同一个 Arc guard，不重新申请独立目录锁。Store 关闭而查询尚未结束时，guard 继续阻止另一个 Store 打开/迁移该目录；最后一个实际连接/target 退出后才显式解锁。原先只有裸 File 副本存活的修复仍有效，因为裸描述符不持有 Arc guard。禁止先手动 unlock 再等任务；错误退出也必须连接先于 guard 释放。

补归属：commands.try_acquire → repair_session.try_lock → mutate 中短 Core/唯一写连接。不持查询读连接、不等网络、不获取协调 lane；一次调用只执行一批，不在持锁时循环扫全历史。与 B2 同属唯一写者，任意入账/映射变更在批间串行。只改变账本派生归属/投影及必要 ledger_revision，不改输入 epoch、来源状态、组选择、许可 map 或采集进度。同步写开始后取消不能承诺回滚已经提交的批次，丢失回执按重扫对账。

## 6 函数细化与正确性论证

### B3.1 查询构造与时间分区

`UsageQuery::new(window, offset, grain, filter) -> AnalyticsResult<Self>` 先校验 §3 全部域/跨度/标签，错误无 IO。纯分钟分区（实现为 `UsageQuery::partition`）求首个完整分钟起点 L=ceil(from/60)×60 和末端 R=floor(to/60)×60。L<R 时：左边缘 `[from,L)`、完整分钟 `[L,R)`、右边缘 `[R,to)`，丢掉空段；L≥R 时无完整分钟，整个 `[from,to)` 只作为一个边缘，避免短窗双算。构造器的短路校验先建立 0≤from<to≤JSON_SAFE，故本分区 from+59≤JSON_SAFE+58<i64::MAX；固定运算可直接使用已证明范围内的 i64。时区分桶用 i128 中间值，不能浮点取整。

`bucket_ranges(query)` 用 width=3600/86400、shift=offset×60，anchor=floor_euclid((from+shift)/width)×width−shift，逐 width 前进，输出与请求范围的非空交集。`bucket_index(t)` 仅接受 from≤t<to，以同一 anchor/width 求索引；Hour/Day 边界必为整分钟，完整分钟单元不会跨桶。负偏移/epoch 附近使用欧几里得除法，不用 Rust 截零除法猜自然日。

论证：三个区间互斥且并集等于原窗；短窗分支只读一次。桶同样构成互斥分区，前进量为正且至多 745 次；i128 中间算术与输入界限避免秒/偏移相加溢出。全部为纯函数，失败没有持久副作用。

### B3.2 查询累计与全结果

`UsageAccumulator::new(query, control)` 建至多 745 个空桶；`push(dimensions, occurred_at_or_minute, totals, control)` 前置为 A/B1 codec 已校验的消费计量，完整分钟已由 B3.1 证明落入单桶。逐项精确应用 filter，未匹配不增加统计；匹配则分别用 A5 对总体、对应模型、token 及桶内模型加算。新增维度/桶模型项之前核容量与 QueryControl，任何超界/取消/错误丢弃整个局部 accumulator；不原地公布半份结果。

`finish(state, control)` 在同一累计结果中生成稳定排序的模型/token/桶数组，遍历与分配继续核取消/截止；空桶保留空模型数组，已初始化的全空查询 total=0。返回前复核分区合计与范围/容量不变量，禁止构造“其他模型”桶掩盖被截断数据。未初始化由查询入口返回 usage=None，不进入此累计器。

论证：初始所有分项为零，每条合格计量恰好进入一个模型、一个 token、一个时间桶，保持三种分区各自和为总体。A5 保证精确/有界加法；错误无可观察部分结果。排序/复核循环均受输出上限约束，不需要保存全历史事实。

### B3.3 只读 target 和一致快照

`CatalogStore::usage_read_target(&self)` 在 usable 后读取本地 instance、克隆受检路径与 Arc guard，O(1) 状态读取，不全扫/复制库。`UsageReadTarget::open(cancel)` 在 Core 外检查取消和既有私有目录/文件约束，使用 READ_ONLY（不含 CREATE/URI/shared-cache）打开本应用库；连接设 query_only=true、busy_timeout=0，不切换 journal_mode、不迁移、不调用 CatalogStore::open。路径检查只验证现存目录/文件，不调用可能创建目录的 prepare_private_directory；缺路径即失败。

开始一个 DEFERRED 读事务并以首个状态读取建立快照；在同一事务检查 application_id、user_version=5、catalog_instance_id 与 target 一致、journal_mode=wal，然后调用 B1 的状态 decoder。失败关连接，不“修复”权限或创建表。Store 已完成全量 open 验证；查询只严格核此次读取的状态/计量行，不每次重做全历史 verify。外部同用户恶意换文件不是此 guard 的安全模型；不引入 inode 数据库登记。

`UsageReader::query(query,cancel)`：未初始化返回同快照 state/None；否则按 B3.1 分区读取投影和边缘事实。投影用 `(minute,rowid)` 升序 keyset，原始事实用 `(occurred_at,native_log_id)`，保留原始 rowid 为内部游标，不当公开身份。每页最多 256 行。token=Any 使用现有时间索引；token=Exact/Unknown 使用固定参数化 token 等值/IS NULL 加时间查询，走现有 token/time 索引。source/group/model 不进无索引长扫描，读出且计入预算后在 Rust 过滤；token 也再次按统一 filter 核对。边缘读取全部 kind 后只对 Consume 累计。分页不结束读事务，不用每页重新取 revision；不做浮点 SUM、无界 GROUP BY 或长 OFFSET。

扫描每取得一行先核预算/取消，再复用 rows codec，送 B3.2。上限正好用完时只探测是否仍有一行，有则整请求 LimitExceeded，无则可以成功。最后 finish、结束只读事务，连接关闭后返回结果；失败销毁局部累计。实现必须用 EXPLAIN 与容量测试证明两个 keyset 查询按现有索引推进而非反复从头扫；无匹配筛选也须受同一原始行预算。

论证：WAL 快照使 state、完整分钟、边缘和三种统计来自相同已提交版本；分区与索引游标递增保证已存消费不重不漏。查询不得被描述为原生日志完整性证明。只读连接/参数绑定限制副作用，guard 延长正确目录持锁期；页数受 1000000 行预算及尾部探测约束，取消/错误走统一关闭路径。

### B3.4 worker 查询与取消

`ReconcileWorker::query_usage(self: &Arc<Self>, query) -> AnalyticsResult<UsageSnapshot>`：校验后 try-acquire 唯一查询槽，创建截止/取消上下文；在可取消的短 Core 访问中取得 B3.3 target。将 target、query、permit 移入 spawn_blocking，同步函数自行检查截止；外层 select 等结果或 10 秒截止。调用 future 的 RAII guard 在未完成就 drop 时置取消并调用已注册的本查询 interrupt。

阻塞池任务开始晚于取消/截止就不打开连接。handle 注册与 cancel 标志读写使用同一安全注册协议：先存 handle，再核标志；取消方先置标志再取 handle 调 interrupt，避免丢失注册竞态。SQL 间隙取消由后续检查覆盖。正常结果到达后再次核截止/取消及 worker.store.usable；不要求当前 ledger_revision 等于结果版本，否则持续采集会使查询饥饿。JoinError 返回固定 Storage 错误；不打印 panic payload。

论证：permit 在实际任务内拥有，外层结束不会增加实际并发；RAII 保证所有取消路径请求停止。结果只发布一次完整对象，旧快照可合法展示且版本明确；poisoned 写者不能被此只读入口伪装为健康。取消不是线程杀死承诺，控制写测试须在读事务仍存活时提交成功，不能只验证查询结束后的写。

### B3.5 补归属游标与候选批次

`CatalogStore::begin_attribution_pass() -> LedgerResult<AttributionCursor>` 在 usable/已初始化下读取 instance 与最大 native_log_id（空库为 0），after=0；只读且不改 revision。`reattribute_batch(cursor) -> LedgerResult<AttributionBatch>` 在 IMMEDIATE 事务内核 instance 及 0≤after≤through；按主键读取 `(after,through]` 的前 256 条完整事实并关闭 SELECT 后再修改，不在活动游标上一边 UPDATE 一边继续迭代。

每批读取全部合法永久映射（包括退休），对已知归属调用 B1 check_known；只对 source/group 仍未知的行调用 A3。新结果必须是旧结果的单调细化：已知 source/group 保持，未知可变已知；原始 NativeFact 完全相同。无法进一步解析的行仍推进此次检查游标，不反复卡住第一条未知项。若候选为空或已检查到 through，next=None；否则 next.after=最后检查的 ID，即使 changed=0 也能前进。若恰好 256 条且最后 ID 小于 through，允许下一次空页结束，不依赖记录数量猜末尾。

论证：有限冻结上界和严格递增主键使一遍可终止；不要求 ledger_revision 在遍历中冻结，正常 B2 入账/其他映射变化不导致不断重启。下一遍从 0 开始，解决映射迟到或较小 ID 后补入账；重开也从 0 开始，不存在丢失不可重建的持久游标。本遍结束不是所有未知可解的证明。

### B3.6 原子搬移与恢复

对 B3.5 中确有归属改善的每条 Consume：由 B1 key 取得旧/新联合键，以单条 input/output/1 为 delta；`subtract_cell(old,delta)` 参数化读取并校验旧单元，逐字段 checked_sub，不足则 CorruptData。剩余 consumption_rows=0 时必须输入输出也为 0，才删除恰好一行；仍有行数时即使 token=0 也保留该单元。然后复用 add_cell 给新键加同一 delta。Error 行不改分钟表。

随后只 UPDATE 事实的 source_id/group_id/attribution_issue，要求恰好一行；原生列不在 SET 中。一批 changed>0 才 checked 提升 ledger_revision 一次；changed=0 不写账本状态。构造安全返回值，再用既有 commit 提交，最后才发布 cursor。任一行/单元/版本溢出、SQL 或已知身份冲突整批回滚；commit unknown 毒化 Store，不能尝试“补写游标”，重开后以持久事实/投影核验再重扫。

论证：每个 Consume 的相同 delta 从旧分区撤出、向新分区加入，全部变化同事务；总体、时间、模型、token 统计守恒，来源/组分项可改变。不可变 NativeFact 使 B1 重放仍识别同值重复。重复批次看到已补正事实则不再搬移，ACK 丢失不产生重复计量。事务中循环最多 256 条、映射最多 10000；游标、结果和写连接状态不跨网络 await。

### B3.7 worker 补正单步

`ReconcileWorker::repair_attribution_once() -> AnalyticsResult<AttributionReport>` 使用 §5 的 commands/单飞/短 Core 顺序；dry_run 返回 preview_only=true、checked=changed=0，不开始新遍、不改持久或内存进度。普通调用在 repair_session.cursor=None 时先 B3.5 取上界，再经已有 mutate/精确 LedgerError 桥接执行一批；成功后才保存 next，下一次 next=None 时重新开一遍。失败保留旧 cursor，Storage/CommitUnknown 不自动重开库或清错。

不新增定时任务，也不把该函数埋入页面读请求；后续运行组装层可与唯一采集循环顺序调用，每次最多一批。新进程从 0 开始；仅本地补正，不产生政策证据。取消发生在同步写已开始后，允许该批原子提交但回执丢失；下一次旧 cursor 重放无重复。当前 GLM/来源/组快照、许可和 input_epoch 必须与调用前相同，publication fence 沿 mutate 原保护路径。

论证：单飞避免两个内存 cursor 并发覆盖；事务串行令映射与计量一批内一致。有限每次工作保留控制任务在批间的机会，任意稳定输入下经足够显式调用能结束一遍；不承诺持续崩溃/永远不调用时仍有进展。

### 私有 helper 责任

`matches_filter` 逐项 Exact/Unknown/Any 比较，无 IO；`read_page` 只构造固定 SQL 并绑定数值游标，每次读取严格递增键；`query_check` 检查取消、期限和预算，无墙钟推断；`subtract_usage` 先核两输入不变量再 checked_sub，差值也校验；`write_attribution` 只更新派生三列、核恰好一行。涉及错误的 helper 不提供自由文本载荷，所有循环均受页/输出/映射预算限制。

## 7 验证与实施门禁

事前十节见 [B3 expectations](../../../audits/sgm-4-facts-analytics/batch-b3-expectations.md)。至少 6 组 PBT 各 128 cases，独立逐事实参考计量；补上 SQL 快照并发、页中取消/注册竞态/阻塞池未启动取消、丢弃调用方后槽仍占用、目录锁释放顺序、原 File 副本回归、poisoned 发布、补正 ACK 丢失与故障注入。禁止只检查汇总等式而缺原始事实参考值。

至少 10 种可编译变异：上界包含、同分钟边缘双算、时区截零、过滤漏 token、未知模型与字面标签合并、每页新事务、提前释放查询槽、连接关闭前解锁、补正只加不减、游标跳过未解项后永不重扫；额外覆盖零 token 行误删、原生列覆盖及未知提交自动重试。独立副本/target 执行，断言失败才算捕获。

性能单独串行 release：合成 10 万消费事实、20 模型/100 token/共享来源和未知维度、跨 31 天含秒边缘，整体日桶查询及一个 token 小时查询各≤5秒；256 行补正/10000 映射单批≤1秒。增加 20 万/100 万历史的耗时、扫描数、峰值内存和 WAL 保留趋势记录；不把趋势样本当产品容量上限。普通测试不夹默认并发墙钟性能门槛。

容量测试覆盖边界及边界+1；预算计算选定索引范围实际读出的行，不能只数 Rust 过滤后的匹配行。EXPLAIN/访问计数覆盖时间与 token/time 两种 keyset 和最大 ID 查询，不能反复扫描历史前缀。控制写可进展由 barrier 协调两个真实连接验证，超时只是防挂，不以短 sleep 推断并发时序。

机械检查范围包括 Step B3.1–B3.7、类型唯一/负 trait、v1–v5 DDL/Cargo/业务路径零改动、Store guard 最小增量、只读连接 flags/无网络、精确测试入口清单、默认 debug/release 全量、clippy/release build 与全部性能入口。实现后按 workflow §6 独立契约审 diff、关闭反馈并完成五维度复核，才宣布 B3 本地验收。

## 8 设计审查与确认范围

G5 由精确分区、统一累计和同快照读实现；G6 由有界任务、目录 guard、唯一事务写者及可重扫恢复实现。所需假设：应用库未被其他进程绕过锁恶意改写，永久 channel 映射不改认，B1 投影不变量已成立，本机文件 IO 最终返回。读者保持观测口径，补正保持总量与原生字段，组合后仍不能推出原生日志完整性或请求可靠终态。

- [x] 范围、复用、共享/私有类型、错误与并发协议明确；不建库/表/连接池/第二采集器。
- [x] 函数分支、退出、callee 前后条件、循环上界及正确性论证已列出。
- [x] 非分钟边缘、固定偏移、未知项、整数和资源边界、取消/目录锁及未知提交均有测试义务。
- [x] 事前十节/PBT/变异/性能及零修改范围已定义；第一小步已验收，B3 整体验收尚未完成。
- [x] 用户已确认内部查询契约、保护预算和最小 guard 生命周期扩展，按查询语义→只读组合→补归属开始实施。

实现只在开发 worktree 进行，不访问实际服务。SGM-4 C 的可信终态、访问 key 管理、三页面与 SGM-7 发布仍单独推进。

第一小步证据：15 项新入口（14 普通/1 性能），4 组各 128 cases PBT、10 个变异及独审通过；默认 debug/release 各 459 passed/0 failed/12 ignored，11 项独立性能通过，最大输出 118 ms。当前没有打开只读连接或延长 catalog guard；下一步按既定 B3.3–B3.4 实现真实快照/取消生命周期，不能用本步合计等式替代数据库一致性验证。
