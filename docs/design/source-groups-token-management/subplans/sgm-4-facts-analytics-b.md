# SGM-4 B 持久账本与统一采集方案

日期：2026-10-17。方案基线 `4929e7c`，实施基线 `9595837`。B1 已实现并完成本地退出验收，见[B1 审核记录](../../../audits/sgm-4-facts-analytics/batch-b1-audit.md)；不接运行服务。[B2 统一采集](sgm-4-facts-analytics-b2.md)已完成本地实现验收，见[B2 审核](../../../audits/sgm-4-facts-analytics/batch-b2-audit.md)；B3 仍待细化。不以本文件宣称整个 B 已可实现或已验收。

依据：[已确认架构](../architecture.md)、[已验收 A](sgm-4-facts-analytics.md)、[需求中的图表口径](../../../research/source-groups-token-management-research.md)、[固定版本能力核验](../../../research/source-groups-facts-capability-research.md)。本批不改变 GLM 策略，不把消费记录当作请求成功，不生成 VolcanoEvidence、FinalOutcome 或 RoutingPermit。

## 1 范围与实施顺序

| 小步 | 交付 | 不包含 |
| --- | --- | --- |
| B1 事务入账 已验收 | 应用库 v5、跨批去重、历史归属、分钟联合投影、采集检查点同事务、迁移和重开 | 网络采集、自动定时、图表 HTTP、策略写入 |
| B2 统一采集 已验收 | 管理日志分页、近期重扫与历史补采、单飞/共享预算、取消及重启恢复、采集状态 | 页面直接查 NewAPI、逐来源/逐 key 轮询、可信失败终态 |
| B3 查询与补归属 | 整体/来源/组/key 的分模型时序、边界秒补算、历史未知归属修正、一致读及容量门禁 | 三页面 UI 和正式发布接线 |

B1 结束后停下验收，再进入 B2 细化与确认；B3 同理。C 的可靠终态、来源周期历史和火山组合仍单独推进。现行 recent_logs、旧图表和 up/run/sync 不切换；统一消费入口正式替换旧路径属于后续页面/发布接线，不能上线后同时保留两套周期采集。

## 2 已核实能力与假设

### 2.1 日志接口

固定 NewAPI `v1.0.0-rc.20` / `6ce7305cd36f16506fb6a2c3c524a5a318539ba7`：管理接口 `GET /api/log/` 支持时间过滤和 `p/page_size`；页大小最多 100。所选 SQLite 路径按 `(created_at DESC,id DESC)` 查询，起止时间均为包含端点，Count 与取页分别执行。[控制器](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/log.go#L11)、[分页](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/common/page_info.go)、[查询](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/log.go#L447)

因此本方案不能假定 API 提供 ID 游标、快照 token、写入水位或原生数据库代次。普通用户/key 日志接口会重新分配展示 ID，ClickHouse 分支也有展示 ID；不能替代这里的管理员 SQLite 日志身份。源码核对补记见能力核验文档，本轮没有运行新的原生分页探针。

### 2.2 单实例与恢复边界

继续使用一个本地托管 NewAPI、一个应用 catalog 和既有 boot/preflight，不新增远程登记、数据库 inode 指纹、代理或第二套 SQLite。逻辑事实身份是 `(实例名称空间,native_log_id)`；在单个应用库内名称空间沿用 `catalog_meta.catalog_instance_id`，物理事实主键可只保存 native_log_id。

**名称空间不是原生数据库身份的证明。** 支持前提是这一 catalog 始终对应同一个、未被不一致替换的原生日志数据集，原生日志 ID 和已存规范事实不被改写。B1 只能验证入账票据与本地名称空间；真实托管绑定、固定版本/SQLite 条件和恢复操作约束由 B2/SGM-7 核验，不能由调用者传入一个 verified=true 代替。

本版本不支持把同一 catalog 静默改接另一份 NewAPI 库。替换/回滚须显式停止采集并走 SGM-7 的匹配备份恢复或新目录导入；不自动清游标、换 UUID 后叠加两份历史，也不复用旧 channel/token ID 的归属。发现同 ID 异值、明显回退或绑定变化时停止推进并报告异常。仅靠 API 不能保证检出所有“恰好内容相同”的静默替换，此为部署前提，不在这里宣称已自动解决。

### 2.3 用量与覆盖

区分三个事实：某行已持久入账、某查询窗已扫到末页、该区间所有真实请求/原生日志均已完整取得。前两项不能推出第三项。连续两遍相同、total 相同、最大日志 ID 增长或重叠窗口结束，都不作为完整性证明。

B 提供“已观测用量”和采集状态；正常展示已记录的模型分项，不因纯失败终态未解决而停止用量统计。空结果不显示为“完整零用量”，不推导供应商余额或自动路由可用性。待采、请求失败、解析错误、分页变化、历史不可补齐与未知归属分别保留诊断。与 SGM-3 的 coverage_end/最终结果游标不同，本批扫描进度不直接成为策略证据。

## 3 复用与所有权

- A 的 NativeFact、ChannelAttribution、AttributedFact、UsageTotals、decode_native_batch、attribute_batch、usage_delta 和 add_usage 原样复用。必要的私有校验 helper 可收窄地提升到 crate 可见，不能另抄一套校验或改变 A 行为。
- `CatalogStore` 保持唯一写连接；新增 `catalog/ledger.rs` 承担私有入账 SQL，`ledger` 保持规范事实和纯投影责任。没有第二个账本库或错误采集器。
- 复用 rusqlite、既有 commit/poisoned 语义、私有路径检查和一致性备份；不新增依赖，不改 v1–v4 DDL。
- B2 复用 NewApiClient 的 ManagementTransport、会话恢复、1 请求/秒和 burst=2 的共享预算。近期使用 Observe、补采使用 Backfill；不自建 reqwest 登录或绕过控制优先级。
- B2 写入仍经 worker Core 单写者及现有 mutate 保护，事务不跨网络 await。短事务的 publication fence 可以失效旧发送票据，但不修改组选择、成员目标、来源状态或现有许可；不为每页调用 GLM 决策。
- B3 大查询不能长时间占 Core。拟使用同一应用库的只读快照、有限扫描和取消期限，具体连接/取消契约在 B3 细化；这不是直读 NewAPI 库，也不增加写者。

## 4 B1 私有数据结构

这些类型不是面板 JSON，也不接受客户端提供的归属或“采集完整”标志。

| 类型 | 字段、约束与生命周期 |
| --- | --- |
| ScanLane | Recent / History；只有统一采集器的两个工作方向，不代表两个采集器 |
| ScanWindow | from/to，UTC 秒半开 `[from,to)`，0≤from<to≤JSON_SAFE；构造时 to≤调用者提供的 now |
| ScanTicket | 本地 catalog_instance_id、lane、scan_generation、window、next_page；由持久状态读取构造，不反序列化；重开后重新读取 |
| PageEnd | More / ReachedEnd；只表示调用方解析的这一次分页结果，不命名为 Complete/Verified |
| ScanProgress | lane、generation、window、next_page、Idle/Scanning/Scanned/Stalled、last_observed_at、固定可空 fault；Scanned 仅为该次遍历结束 |
| LedgerState | revision≥1、可空 initial_requested_from、按 Recent/History 固定顺序读取的两个 ScanProgress；未初始化不得伪装为已有覆盖；revision 只用于统计快照版本，不是策略结果游标 |
| LedgerCommit | inserted、duplicates、changed_cells、ledger_revision、ScanProgress；计数有界，不携带原生日志正文 |
| LedgerError | Catalog(CatalogError)、Fact(FactError)、RevisionConflict、InvalidCheckpoint、NotInitialized；均不含自由文本或原始载荷 |

带事实或标签的类型继续禁止 Debug/Display/Serialize/Deserialize。需要存储的 metadata 由专用白名单 codec 写入私有、带 version=1 的 JSON；没有原始 other、content、用户名、token_name、IP、路径、鉴权或提示词。codec 不给 NativeFact 增加通用 serde trait。

fault_code 使用封闭内部枚举：Transport、InvalidPage、InvalidFact、ConflictingIdentity、BindingChanged、HistoryUnavailable、ResourceLimit、Clock；它们描述采集状态，不是供应商失败计数。metadata 存储对象包含 version/state；Absent/Unusable 没有其余字段，Parsed 才带 A 定义的 status_code/error_type/error_code/attempted_channels/stream，未知字段或非法组合视为存储损坏。尝试 ID 用精确整数，不经过 JS number。

### 4.1 v5 表与索引

沿原迁移方式保留 catalog_instance_id，只替换 catalog_meta 版本约束为 5，新增以下四表；不建 C 尚未细化的终态/周期表。

| 表 | 必需字段与约束 |
| --- | --- |
| ledger_state | singleton=1，revision，initial_requested_from；迁移初值 revision=1、from=NULL |
| ledger_checkpoints | lane 主键，generation≥0，window_from/to，next_page≥1，phase，last_observed_at，fault_code；两条 Idle 初值，generation=0、窗口/时刻/fault 均 NULL。活动状态 generation≥1 且窗口齐全；Stalled 必须有固定 fault |
| ledger_facts | native_log_id 正 i64 主键；A 的全部规范字段；source_id/group_id 可空、attribution_issue；数值与标签约束按 A，Error token 必须为零。原生规范字段插入后不可覆盖；归属仅 B3 的显式修正事务可变 |
| ledger_minute_usage | minute、channel/source/group/native_group/token/model 联合维度及 input/output/consumption_rows；minute%60=0；非空单元 consumption_rows≥1；沿 A 的安全整数与合计约束 |

各表为 STRICT。ledger_facts 的列名为 native_log_id、request_id、kind、occurred_at、channel_id、token_id、model、native_group、input、output、is_stream、metadata_json、source_id、group_id、attribution_issue。kind 存 consume/error，issue 存 none/unmapped_channel/group_unresolved；其余值域按 A，metadata_json 为非空 TEXT 且≤16 KiB。ledger_minute_usage 的列名直接沿用 A UsageCellKey/UsageTotals；身份、标签和 NULL 组合的 Rust 解码校验不可由 SQL 类型检查替代。

事实和投影的来源/组引用保留退休实体，外键不得级联删除。归属还须在事务及重开检查中按永久 memberships/channel 映射复核，不能只凭外键存在判正确。

已知 source 必须匹配永久 channel 映射；已知 group 还要求日志原生组一致。GroupUnresolved 可以保留已知 source 和不匹配的原生组，不能因此判损坏。未知归属即使现在已有新映射也仍是合法待修状态。open 不自动补归属、不因“现在可以解析”判旧未知行损坏；分钟投影核验以已存归属为准，直到 B3 显式搬移。

NULL 维度不能依赖普通复合 UNIQUE 去重：分钟表使用静态表达式唯一索引，把 NULL ID 归一为内部 0、NULL 标签归一为空串；真实 Some(ID) 必须正值、Some(label) 必须非空，故不存在碰撞。SQL 行仍保留 NULL，不能把内部索引哨兵变成公开身份或展示标签。

SQLite 的唯一索引把不同 NULL 视为不同值；表达式索引与查询的表达式须一致，不能只写数学等价的另一个形式。[唯一索引语义](https://www.sqlite.org/lang_createindex.html#unique_indexes)、[表达式索引规则](https://www.sqlite.org/expridx.html)

索引至少覆盖：事实 `(occurred_at,native_log_id)`、`(token_id,occurred_at,native_log_id)`、`(channel_id,native_log_id)`；分钟联合唯一索引及 `(minute)`、`(token_id,minute)`。另为 consume 事实设置“UTC 分钟表达式＋上述 NULL 归一化联合维度＋native_log_id”的部分索引，供全量一致性核验顺序读取；文本使用 BINARY 排序，与 A 的字符串排序一致，不在核验时额外建立全历史内存 map。具体 SQL 由 B1 实施后核 EXPLAIN/约束负例，不通过“只有一个页面的数据量”验证性能。

### 4.2 数据保留建议

默认初始补采最近 31 天，覆盖既有近 30 天及跨时区边界需求；这是采集起点，不是最多保存 31 天。已存规范事实和投影不自动删除，图表不会因来源/key 删除丢历史。更早补采需 B2 的明确请求/进度，不能拿近 100 条替代。

先不加入自动清理策略，以避免尚未细化的删除破坏跨批去重、补归属或历史证据。磁盘满/写失败明确报错并停止推进，已有数据不清零。长期保留带来的库增长、迁移备份耗时在容量测试中记录；不能静默删除历史来满足预算。

## 5 B1 函数细化

以下均为应用库本地调用，不访问网络；调用方提供 UTC now，不在 open 时读取墙钟或自动初始化采集。所有写入口先 `usable()`，正常返回以 commit 成功为前提；CommitUnknown 保持既有 poisoned 行为，不承诺已回滚，重开后以持久状态决定是否重试。

### B1.1 migrate_v4 与 verify_ledger

`migrate_v4(&mut self) -> CatalogResult<()>`：open 先用旧 v4 规则只读验证，再按原路径规则制作一致性备份；IMMEDIATE 事务新增 v5 表/索引和三条状态初值，保留 catalog_instance_id，更新 catalog_meta/user_version，验证后用原 commit 提交。新库和 v1–v3 沿既有链到 v4 再升级；不承诺整条历史迁移链原子回滚。原 recover_executions 行为保留，不借迁移清掉未决写或火山耗尽。

`verify_ledger(conn) -> CatalogResult<()>`：验证 singleton、两个 lane、枚举/组合约束、事实 codec 和归属、分钟非空及安全计数。未初始化时必须没有事实/投影且两个 lane 都为 Idle。实现选用主键游标逐行流式核验，比事前最多 1024 行缓冲更小，不全量载入 Vec；完整投影一致性通过联合键索引上的事实与分钟行双流合并核对，使用 A5 精确加算，不使用浮点 SUM，不构建全历史内存 map。核验须位于同一只读事务；实施时验证索引确实被使用，不能因库大而只验证首批。未知 codec、坏行、缺状态行或投影不符均拒绝，不自动修复为零。

正确性：旧库验证/备份先于写；v5 标记和新表在同一事务，失败不发布部分 v5。核验游标严格增加，有限持久行数保证终止；双方有序流逐单元比较发现缺失、多余和数值差异，最多持有当前事实行和当前单元。现有 v5 在 open 的只读探测事务完成全量核验，迁移后的空账本在迁移事务核验；同一独占 guard 下后续不再重复全扫。迁移性能门槛见 §8；不能以迁移成功代表在线已接管。

### B1.2 initialize_ledger 与 load_ledger_state

`initialize_ledger(initial_requested_from, now) -> LedgerResult<LedgerState>`：校验 0≤from<now≤JSON_SAFE；未初始化则只保存初始请求起点并提升 revision。相同值重放为 no-op；已初始化但值不同报 RevisionConflict，不自动换数据集或清事实。起点不是删除线，后续更早窗口的明确补采不能被当作重复历史丢掉。

`load_ledger_state() -> LedgerResult<LedgerState>`：一个只读事务解码状态和两个 checkpoint；缺失/非法组合报 CorruptData，不用默认值补齐。

正确性：初始化不填用量、不完成扫描；状态变更与 revision 同事务。重放比较在完整校验之后，无循环，初始化失败不影响旧目录。

### B1.3 begin_scan 与 load_scan_ticket

`begin_scan(lane, expected_generation, window, now) -> LedgerResult<ScanTicket>`：初始化及窗口校验后在事务内核 generation。Idle/Scanned 可开始新窗口，generation checked+1、page=1、phase=Scanning；Stalled 可显式从 page=1 重扫同窗。已有 Scanning 且 expected_generation/window 均匹配时返回原票据，不静默丢弃进度换窗；参数不同则 RevisionConflict。丢失开始回执后须重读当前 generation，不能用旧 generation 猜测一次新的遍历。

每次真正开始都清该 lane 的旧 fault/last_observed_at，保持全局事实和投影不变；Stalled 请求另一个窗口时拒绝，不能借换窗丢掉未处理区间。组合约束：Idle 只有初始空窗口；Scanning 可以尚无 observed_at；Scanned 必须有 observed_at 且无 fault；Stalled 必须有 fault 和合法时间。More 保存下一页，ReachedEnd 保留最后已读页号，不再生成活动票据。

`load_scan_ticket(lane) -> LedgerResult<Option<ScanTicket>>`：读当前持久扫描；Scanning 返回票据，Idle/Scanned/Stalled 返回 None 及另行可读的进度状态。不产生“已采齐”证明。

正确性：每次显式新遍历有持久代次，旧网络响应不能写入新遍历；page 初值及窗口均固定。没有网络或无界循环；代次/revision 溢出拒绝整笔写入。

### B1.4 commit_fact_page

`commit_fact_page(ticket, rows: &[serde_json::Value], end: PageEnd, observed_at) -> LedgerResult<LedgerCommit>`，Step B1.4 顺序：

1. 原始行数最多 MAX_FACTS=1024，调用 A2 取得已验证、批内规范去重的事实；不重复实现 A1/A2。所有事实时刻必须在 ticket.window 内，observed_at 必须可表示且不早于窗口截止。原始 Value 只用于本次解析，不落库或进入诊断。
2. IMMEDIATE 事务复核本地实例名称空间、lane/generation/window/page 和 Scanning；observed_at 不得早于该 checkpoint 的 last_observed_at。票据不符返回 RevisionConflict，时间非法返回 InvalidCheckpoint，不能先写事实再检查。
3. 读取全部永久 channel→source/group/native_group 历史，包含退休成员；沿 A3 过滤前验证唯一性和上限，不能按当前启用状态推断历史。
4. 对每个规范 ID 查 ledger_facts：不存在才是新增；已存在且原生规范字段完全相同为重复；不同为 ConflictingIdentity，整批回滚。重复判断不比较可修正的派生归属，不覆盖原生字段，也不按 request_id 合并。
5. 只有真正新增的事实进入 A3/A4。写入规范事实及同事务确定的归属，再对每个联合单元读取旧计数，用 A5 精确加算并插入/更新。错误行仍存事实但没有消费增量，取消/EOF 的已记录 token 正常入账。
6. More 将 next_page checked+1，ReachedEnd 标 Scanned；两者都只记录本次扫描进度/时间，清该 lane 旧 fault，提升 ledger_revision。事实为空或全部重复仍可推进合法检查点，但不能设置完整用量/策略就绪标志。
7. 构造安全返回值，再用既有 commit 提交；duplicates 为本次 type=2/5 有效原始行数减 inserted，包含批内同值重复，其他日志类型不计入。SQL、codec、归属、范围或加法失败回滚本地事务。未知提交仅返回 CommitUnknown，不发布半批结果。

调用方责任：rows 来自同一受支持原生日志数据集；B2 已严格核 API 页外壳、原始行数和 PageEnd。B1 不接收前端 bool 充当该生产者，ReachedEnd 也不承担网络完整性证明。

正确性：唯一键及完整规范比较使跨批/跨重开重复不增加计量；只对新增集合做一次分区投影，保持“同一事实只属于一个联合单元”。事实、单元和页进度共用事务；确定回滚的错误不推进检查点，提交结果未知时不猜旧/新状态，只允许重开核对原子持久结果。ACK 丢失后旧 ticket 被拒绝，调用方重读持久进度；另起重扫则相同事实被去重，不需要第二份批次收据表。

循环均以本批≤1024、历史≤10000 和新增联合单元≤1024 为上界；不做全事实扫描或按每个事实遍历全部历史。不在事务内获取远端材料、调用策略或读当前 token 名称。

### B1.5 mark_scan_fault

`mark_scan_fault(ticket, fixed_fault, observed_at) -> LedgerResult<ScanProgress>`：仅接受匹配的 Scanning 票据，将该 lane 标 Stalled、保存固定原因及时间，提升 ledger_revision；事实、投影、窗口和页边界不变。旧票据或已 Stalled 的重复调用报 RevisionConflict，无副作用；时间必须可表示且不早于旧 observed_at。不得存原始 HTTP/SQL 文本。

入账失败本身不隐式再提交：B2 仅在 Store 仍 usable 时另调用此函数记录确定错误。CommitUnknown/损坏库不尝试“补写健康状态”，由运行层显示存储不可用。如此保留“commit_fact_page 的 Err 无已确认半批写”与独立诊断写入的区别。

### B1.6 私有辅助契约

| helper | 前后条件、退出与论证 |
| --- | --- |
| encode/decode_fact_metadata | 仅 A 白名单和 version=1；输出≤16 KiB；坏版本/类型/值固定错误，不保留底层原文；列表≤128，循环有界 |
| read_fact / insert_fact | 按主键读取/插入；读取严格重验规范字段及派生归属；不存在返回 None，存在坏行报损坏，禁止 UPSERT 覆盖原生事实 |
| read_channel_history | 同事务读取全部带 channel 的 memberships 及不可变原生组名；≤10000，退休保留；不能用当前展示名或 secret 猜测 |
| read/upsert_usage_cell | 参数化精确联合键，NULL 语义与唯一索引一致；旧行先核不变量，用 A5 后写；没有 FLOAT/SUM/饱和加法 |
| decode/check_checkpoint | 枚举和 NULL 组合、代次/页数/时间完整验证；预期票据逐字段匹配，不只检查页码；固定步骤无 IO |
| ledger_error / commit 包装 | 输入错误与已存数据损坏分开；复用 CatalogError/FactError；commit 未知时使 Store unusable，错误不带敏感自由文本 |

实现名称对应：progress/state/initialized 完成上述状态解码和初始化门禁，查询固定状态行及 EXISTS，不读全历史；matched 逐字段核票据与本地 instance。safe/increment/valid_window/check 为固定次数数值检查；save_progress/bump 要求恰好修改一行，调用方同事务回滚其失败。rows::key 只复制有界维度并按非负秒取分钟；cell_row 复用标签校验和 A5 校验计数，compare_cell 比较双流当前完整单元。codec 的 stream_name/value 只输出封闭枚举/字段和至多 128 个尝试；A 原 metadata 校验抽成同一 helper，原 decode 行为不变，写入和读回都复用它。

## 6 B2 统一采集约束和已验证门槛

以下保留总体方向；具体状态分支、默认值和最小增量见 [B2 细化](sgm-4-facts-analytics-b2.md)。B2 采用两条既有检查点、历史固定整窗循环重扫，不增加窗口任务表；已新增显式 restart_scan，v5 不变。启动证据、隔离能力和实现门禁已验证，未接生产入口。

采用一个后台任务、一个单飞入口和共享 commands/管理预算。每轮最多 4 次逻辑日志读取，近期与补采交错，默认轮次结束后间隔 10 秒；上一轮未结束不叠加任务。管理 401/429/超时沿既有 transport，401 重放与登录也计入共享物理请求预算，不额外重试登录或无限立即重跑。后续页面刷新只读应用库，不触发采集。

近期查询固定截止时间并重叠回看，默认回看 5 分钟；历史从持久化的初始 31 天起点整窗重扫，每遍冻结截止时间。回看长度只是发现迟到记录的策略，不是最大写入延迟保证。超过近期窗的迟到数据必须能由低优先补扫发现；原生已清理的数据只能保留缺口，不能补成零。

API 使用 type=0 取得同一全局日志序列，A 只保留 2/5；不得再分来源/组/key 查询。请求只用已核实的时间和页码参数，不发送并不存在的 since_id 游标。半开区间转换为 start_timestamp=from、end_timestamp=to−1；原生把 0 当未过滤，`to=1` 这种不能正确表达的窗口必须明确拒绝，不能意外全量查询。

每页检查 envelope、page/page_size/total、最多 100 行、排序键/时间窗和已知数值；非事实类型也要校验分页所需 id/time，不能只靠过滤后的事实数判末页。单次响应复用 8 MiB 限制，超限或坏核心字段不默认零、不跳过后推进。total 只是观测辅助，不能用它预先截断后续页；历史分页不能复用渠道目录的 101 页/10000 条总量限制。

以下门槛已在 B2 细化、测试清单和审核记录中落实，继续作为后续接线约束：

- 近期/历史窗口、页间移动、重启后旧 offset 重扫、计数/顺序变化时的重启策略及每轮上界；持续变化只能报告滞后，不能无限占网络/锁。
- 不丢初始待补区间的持久调度，长期离线、跨秒/小时、时钟回拨和历史重扫公平性。
- 真正的固定版本本地 SQLite/管理员日志能力检查；不得误用用户展示 ID、把现有路由 RecoveryDisabled 条件当成统计可读性的充分必要条件，或新造远程登记协议。
- 绑定变化/不一致恢复的停采及明确恢复流程；声明纯 API 无法保证检出所有静默替换，不以伪 UUID 检查消除这个前提。
- 隔离原生多页/时间端点探针和可控假 API 的插入、删除、同计数替换、迟到、乱序/重复、429/401、取消等反例；不能只拿静止列表测试代替这些门槛。

## 7 B3 查询与补归属方向和编码前门槛

沿用近 24 小时、近 30 天、按天看小时；同一次查询返回相同区间、时区、ledger_revision 下的总计和分模型桶。过滤使用稳定 SourceId/GroupId/token_id，不按名称认领；删除 key/退役来源仍参与历史。分项与总体来自同一联合投影，未知维度仍参与守恒。

分钟边界完整落在查询区间内的部分读投影；边缘不足一分钟的区间按原始秒级事实精确补算，分区必须不重叠、不漏。自然日按显式固定 UTC 偏移转换，不能复用火山每日 01:00 周期；未完整采集的空桶不填完整零。消费行数不改名为最终请求数。

补归属只针对尚未解析的历史事实，读取永久映射后在同一事务撤回旧联合单元、加入新单元并更新派生归属；总用量及时间/model/token 口径不变。不能把整批消费再加一次，也不能把已知 source/group 改贴新成员。修正期间的分页、断点和发布必须有独立 PBT。

B3 细化还需明确：只读一致快照与取消如何不阻塞控制写、结果分页/输出上限与多 key 汇总一致性、查询溢出/超时的全失败语义、未知标签展示、补归属游标和修正恢复。不能用截断 Top-N 冒充全部模型总量。页面公开 JSON/schema 在对应读接口确定后产出，不能直接把私有 NativeFact 序列化出去。

## 8 B1 验证门槛

事前清单见 [B1 expectations](../../../audits/sgm-4-facts-analytics/batch-b1-expectations.md)。至少包含 6 项各 128 cases PBT：跨批/重开去重、同 ID 冲突原子性、共享来源与未知归属、联合投影守恒、检查点/取消/重放、复杂旧库迁移保真。参考模型用独立 ID 集合与逐条整数计量，不用被测入账/投影 helper 计算预期。

确定性故障要覆盖事实写后、晚单元溢出、检查点写失败、commit 未知毒化、缺必需表/行、NULL 唯一索引、反向覆盖已知身份、非空执行历史及旧锁继承回归。新库/v1–v4 迁移均保留原目录、凭据、运行策略及未决操作；原测试不得通过清空复杂历史来适配 v5。

定向变异至少 8 项：重复再次累加、异值覆盖、按请求去重、只看活动成员、联合 key 漏 token/model、检查点提前提交、NULL 单元重复、吞掉 overflow。源码/target 双隔离，断言失败而非编译失败才算捕获。

性能在独立串行 release 门禁执行：1024 事实/10000 历史映射单次入账≤5 秒；含 10 万事实和联合投影的重开核验≤10 秒；迁移保留既有复杂历史≤5 秒。普通测试保留容量/内容断言，不夹默认并发墙钟阈值；如果固定夹具不能满足预算，先优化/讨论，不放宽断言。另记录更大历史的资源趋势，不把 10 万条当产品总量上限。

机械检查须核 v1–v4 DDL 冻结、GLM/认证/CLI/面板零改动、Step B1.1–B1.6、共享类型、私有序列化、函数外迁移改动清单及默认全量回归。按 workflow §6 进行实现后的独立契约审，再做五维度复核；实际执行与审核结果见[B1 审核记录](../../../audits/sgm-4-facts-analytics/batch-b1-audit.md)，不把方案本身当作验证证据。

## 9 方案自检与确认点

- B1 状态迁移、错误/未知提交、helper 调用、容量与循环、事务原子性和迁移影响已列出；实现与测试/独审完成，本地退出通过。
- B2 已完成分步实现及本地验收，B3 尚未细化；不会拿这个总体方案直接一次性实现所有模块。
- B1/B2 已获确认并验收；默认初始补采 31 天、轮次结束后 10 秒、最多 4 页已实现；本阶段不自动删除历史，也不承诺静默替换自动识别或严格完整覆盖。
- 若以后要求原生快照/严格采集水位，或原地换库并自动合并历史，需另行确认能力/架构范围；本方案不偷偷改为直读 NewAPI 数据库。

方案阶段只有文档；B1 实施阶段按本范围新增本地 Rust/v5 DDL 及测试，但没有服务访问、生产配置/数据库读取、重启、部署、推送或合并。
