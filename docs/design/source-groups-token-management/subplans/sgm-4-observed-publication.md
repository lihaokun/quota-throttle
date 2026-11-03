# 火山观测持久去重与调度发布

日期：2026-10-17，基线 `ce23dc7`。承接已确认并验收的[观测归并核心](sgm-4-observed-breaker.md)、SGM-3 B/C-1、SGM-4 A/B。用户已授权连续开发；本步 O4 实现应用库中的完整事务及 worker 显式刷新入口，不部署、不改 NewAPI 或旧 GLM 算法，不接现行 CLI/面板。

## 1 行为与复用

每源跨组共享观测和 token；每组仍独立稳定首选。正 token 成功活动清连续计数，但不自动解除已锁存错误、token 上限或人工暂停。原来源周期、error_epoch/cutoff 与旧请求隔离继续生效。同请求重试/成功的幂等由 O1–O3 状态转移和本步事务提供。

复用一个应用库、一个 worker Core、原账本/分钟投影、原火山纯状态机和组选择器、原执行日志及回读晋升屏障。Native log ID 可以大于 JSON_SAFE，不作本地批次序号；VolcanoEvidence 游标是每个 period/error_epoch 内由事务生产的逻辑批次号。

统计范围为本周期已观测消费，不宣称上游完整账单。熔断只处理近期观测：复用 B2 的 RECENT_SECONDS=300，历史补采仍全量计入用量，但距当前观测时刻超过此窗口的事件不重新触发/清除当下熔断。顺序为本地首次入账/首次补明来源的顺序；同一批按原事实 ID 排序，不宣称全局请求完成顺序。晚到数据和重试可能造成已接受的观测型误判。

## 2 数据库 v6

冻结 v1–v5 DDL；v6 仅替换 catalog_meta 版本约束，增加三个实际消费的表、两个来源时间索引及一个请求最早事件索引。CatalogSnapshot/OperationView wire 仍为 1，无新依赖。

| 表 | 字段及约束 |
| --- | --- |
| volcano_observation_queue | sequence INTEGER PRIMARY KEY AUTOINCREMENT，1..=JSON_SAFE；native_log_id UNIQUE 外键到 ledger_facts；source_id/kind 复合外键到 volcano 来源。只存待处理引用，不复制事实或 token；另建(source_id,sequence)索引 |
| volcano_request_observations | 主键(source_id,request_id)，source/kind 外键；request_id 沿 A 的 ASCII/64字节规则，first_event_at 0..=JSON_SAFE；phase 仅 failed/token_flow。只记录去重相位，不存第二份失败计数 |
| volcano_period_history | 主键(source_id,period_id)，source/kind 外键；old_start<old_next<=new_start<new_next；旧 reset_minute、utc_offset_minutes 沿原值域。只存周期过渡，不存另一份 token 总额 |

索引 ledger_cell_source_time(source_id,minute) 与 ledger_fact_source_usage(source_id,occurred_at,native_log_id) WHERE kind='consume' 仅服务同事务源用量读取；ledger_fact_source_request(source_id,request_id,occurred_at) 服务已知首次事件的有界 MIN 查找。不改变 B3 图表索引/扫描语义。

迁移先按旧版本完整验证和私有一致备份，再单事务建 v6/保留 instance/提升两个版本标记；不补入迁移前的旧错误队列、不清任何旧来源计数/原因/凭据/控制日志。旧消费仍用于周期用量。回滚不留下半个 v6，未知提交沿 poisoned；原生 NewAPI 数据库不在范围内。

open及迁移前验证新表、索引、队列与事实的 source 一致、请求 ID/phase、周期历史及外键；备份沿用原一致性拷贝。损坏拒绝迁移，不补表或重建成健康。历史夹具仅新增删除 v6 侧表/索引的步骤，保留旧复杂材料和原业务断言；仅“打开最新版本”的断言使用 DATABASE_SCHEMA_VERSION。

## 3 原子入队

`enqueue_observation(tx,fact,observed_at)`（O4.1）：调用者已经在 B1/B3 事务中保存该规范事实；验证有界安全时间。仅已知 Volcano/type 8、O1 可分类且 occurred_at 位于 [observed_at-RECENT_SECONDS,observed_at] 的记录入队。仅用 ON CONFLICT(native_log_id) DO NOTHING 防同事务重复挂接，不能忽略 CHECK/外键错误；序号耗尽使外层事务失败回滚。不读取来源 secret，不对 GLM、零 token/排除错误或老历史建队列。

B1 只对真正新增事实调用，和事实/分钟投影/采集检查点同事务；重放旧事实不能重新入队。B3 只在 source_id 首次由未知变已知时调用，使用现有 recent.last_observed_at 作有界参考，没有该观测则不凭当前时间伪造。消费端仍按真实 now 再检查近期窗口；来源已知后的仅 group 补正不重复入队。原生字段及计量不改变。

论证：原账本唯一 ID 和首次归属条件保证已处理后删除的队列行不会被重放补回；队列只引用同一事实，任何入队失败使外层事实/归属事务一起回滚。队列 sequence 提供稳定本地顺序，不把原生 ID 当安全批次号。

## 4 事务观测与周期

`project_source(tx,source_id,now,initialized,fresh,budget)`（O4.2）：发布入口在同一 IMMEDIATE 事务和当前目录下逐 Volcano 来源调用，读取配置/控制/原 Runtime，不读取密钥。最多 MAX_SOURCES；每源处理至多16条队列，避免一个繁忙来源饿死其他来源。未初始化来源在账本已初始化后按原日格初始化；未初始化账本不给任何火山正向许可，不把未知默认成已查到0。

已初始化来源调用 advance_period；到期同时写 PeriodTransition 历史。manual_paused 不丢用量或观测，但不授予路由；退休/退役意图来源不新增健康状态，安全停止不依赖日志新鲜。now 早于当前周期或队首日志来自未来时，该源等待，不推进未来队列或签发许可。

当前周期绝对 input/output 由同一事务的原分钟投影和秒级边缘读取（O4.3）。复用 A5 checked 加算，不用 SQLite SUM 隐式溢出，不因队列处理分批漏算 token。旧 Runtime 已用量大于账本观测时拒绝该轮，不降低计数掩盖不一致。累计更新仍调用 apply_evidence/apply_usage。

按队列 sequence 读取并解码原事实，校验归属和适配范围，再读(source,request)的旧相位并调用 O1/O2。已入队但处理时超过近期窗口/已跨周期的事件仍保存相位和最早事件，只过滤其计数输出；否则排队旧 F 被丢弃后，新周期同请求 S 会失去旧请求隔离。原 token 仍参与适用周期统计。

每次合并再以索引查同 source/request 的所有已知规范事实最早 occurred_at，将 next/outcome.first_event_at 下界补齐；包括未入队的历史事实，防止历史 F 在撤错/重置前、新 S 在之后却被误当新请求。该下界来自同事务账本，不是外部传入的开始时间，也不证明尚未知历史。有效相位与清零/失败结果在同事务保存，处理过的队列行同事务删除。重复成功/错误不再输出，重启从持久相位和剩余队列继续。

VolcanoEvidence.from_cursor 取原 Runtime，to_cursor 为新的逻辑批次号（None→1，否则checked+1）；只有处理队列、用量/观测范围变化或首次投影才推进，没有变化不写 Runtime。period/error_epoch 来自本事务推进后的状态，不把旧异步批次改贴新代次。coverage_end 是本地已观测账本查询上界 min(now+1,next_reset)，不是上游完整性证明；仅无新输入、用量不变时不因墙钟前进制造写入。

每个来源的 Runtime 在本轮最多写最终值一次，内容变化才增 revision，初始化为1。消费去重、原因、周期历史任一失败整轮回滚。现有 apply_source_policy_tx 发生 PeriodTransition 时也写同一历史 helper，避免内部 Tick/后续管理动作丢弃历史；不改变纯算法。

论证：同事务读原事实和状态，调用已验证纯函数，不存在成功清内存但失败落盘；提交未知不发布，重开按真实队列恢复。每源16条和固定目录容量界定工作量；纯函数相位单向，来源和请求键唯一，使重放不能重复增加/清零。历史/用量无增量双算。

## 5 读取预算与状态发布

来源用量采用原分钟互斥分区函数（只提取复用，不改 B3 计算）；完整分钟读新来源索引，边缘读消费专用来源索引。每 SQL 页256行，使用同时间 ID 后继/严格后续时间两段 keyset，避免密集时间前缀重扫。整轮统计最多1000000行，单调时钟预算10秒；超界固定 NotReady/Storage，整事务回滚，无部分目标/许可发布。MAX_REQUESTS 不另外放内存全表。

正向许可前提（O4.4）：原 BackendReadiness 有效、组 ready、成员已接管/已绑定且未暂停/退役、来源 type8 且 Runtime Available、该源本地队列已消费完、recent 页近期成功。recent 允许 scanning/scanned 且 last_observed_at 存在、无 fault、不是未来、距 now<30秒；不把正常分页称为上游完整证明，也不从 idle/stalled 伪造健康。lease 截止取 backend、last_observed_at+30、now+30 最小值。

各 Volcano 组调用原 select_volcano_group，来源状态跨组复用；只有来源不可承载首选（退休/退役、Runtime 未初始化、账本未初始化、周期未开始、非 type8）才传 None（耗尽仍传 Runtime），优先级 Keep 不填0。观测新鲜度与本源待处理队列只限正向许可，不得折叠成 None 进入组选择器，否则采集空窗会清空首选、恢复时轮换（2026-10-20 修正，见 [fixes 记录](../../../fixes/volcano-policy-fix-stale-ledger-preferred-churn.md)）。只改变 managed/已绑定、未暂停/退休/退役成员的必要目标；已知耗尽令本源所有可管理成员 Disabled，健康且门禁满足才准备 Enabled，未知保留旧 status/priority。暂停/退役由原控制意图优先，不自动接管成员、改来源 manual_paused 或创建控制操作。

保存组状态、必要 priority/status 目标与新许可在本事务中完成。目标变化才增 member revision、清 applied、标 dirty、supersede 未完成旧收据；旧成功回执/执行记录保留。从非 Enabled 进入 Enabled 时按原格式增 enable_epoch、设 enable_requested_revision、清 consumed；无变化不反复补授权。GLM 授权规则完全不变。回读最终成员后构造许可，沿原下调/回读/晋升屏障执行，不绕过 unknown write。

## 6 Worker 与两类许可隔离

`refresh_volcano_policy()`（O4.5）仅为显式内部入口：try_acquire commands，经既有 publish fence 调用单次 Store 事务，无网络或跨 await Core 锁；成功提交后一次替换该类的 source_disabled 与 permits，input_epoch 一起推进，失败保留旧缓存，poisoned 保持 fence 关闭。dry_run 返回只读等待/来源计数，不初始化、不消费队列、不推进周期或发布许可。

许可 map 不能互相覆盖：GLM/Volcano commit 各自返回完整 membership scope，worker 只移除本类 scope 再扩入新许可，保留另一类原 lease（不顺便续期）。scope 包含本类所有历史/当前成员；目录身份永久保留。GLM 纯选择、状态/目标保存算法不变，仅发布所有权扩展。恢复许可时同事务唤醒已有 NotReady dirty 待办，不新建任务。

本步不增加新周期任务，运行组装由 SGM-7 接入；现有 worker.reconcile_once 用这些真实意图/许可执行 NewAPI 管理请求。组合验证用临时假 NewAPI 服务，不能以只检查数据库替代真实执行顺序。

## 7 验收

迁移：v1–v5完整记录/实例/备份保留，v6失败回滚、坏队列关联/phase/历史拒绝，数据库与读接口版本同步。持久轨迹：F/F/S/F、同请求跨批/跨组/重开，第三次锁存后成功清0不解锁，手动cutoff/重置不吃旧事件，旧历史只计用量，未知归属后补入队、group-only不重复，原生ID超过JSON_SAFE。

事务故障：队列、相位、用量读取、Runtime、历史、各组目标、commit 每个边界注入失败，镜像全回滚；未知提交关闭发布，重开不双计。组合：多组共享耗尽→各组备用独立切换，GLM与Volcano刷新交替不互相撤许可，dry_run/未接管/组未ready/超期/未决远端写仍安全；成功/源暂停/模型变更与刷新串行版本核验。

至少4组各128cases PBT：迁移/重开、随机请求观测/重放/重开、故障原子性、混合提供者发布；参考不用生产选择/归并 helper。至少8个可编译定向变异。默认debug/release全量、clippy/build、原性能入口全保留；新增串行release十万消费的周期投影与16事件/源最大目录组合各≤10秒。独审后记录机械范围/步骤、schema、PBT、性能及部署边界，不将worker内部接线称作已切换线上版本。

## 8 函数契约与正确性论证

下列 helper 均仅接受当前目录的应用库连接/事务；SQL 错误沿 Storage 传播，数值/时间不合法沿 InvalidInput 或 Storage，预算耗尽沿 NotReady。它们不提交、不开网络、不写日志材料；除明确的只读入口外，失败由外层事务回滚。计数与日格使用 SGM-3 已验收函数，不另做算法。

### 入队和请求相位

`enqueue_observation` 前置为已规范解码且已在同事务保存的事实。先验证时间，再调用 O1（已知来源/有效 request 才有分类），再查目录类型及生命周期。非适配/旧历史正常无写返回；其余只插入事实引用，原生日志 ID 唯一冲突不再插入，CHECK/外键/序号耗尽不能忽略。B1 的 fresh 条件、B3 的首次来源补明条件保证消费后不重入；不改变事实字段。任何 SQL 失败使调用方本轮事实/归属/检查点一起回滚。

`request_state` 前置为合法 source/request，主键至多命中一条。无行返回 None；有行校验时间和 phase 后构造 O2 状态，未知值拒绝；只读。`save_request` 前置为 O2 已验证状态及账本补齐的合法最早时间；按同一主键 UPSERT 只写相位和首次时间，不写计数。唯一键阻止跨源/跨请求合并，SQL 错误交外层回滚。

`consume_queue` 前置为已验证 source、当前安全 now 和共享预算。每源按 sequence 取最多16条，逐条校验 canonical source，未来事件终止循环且保留队列。其余先读取旧相位并调用 O2，再用索引 MIN 补齐已知最早事件；仅状态不同才写相位，仅近期输出才送入原状态机；无效分类不发事件。最后精确删除本条，删除数不是1即拒绝。循环每次减少选中条数，最多16；未处理行保留并用 EXISTS 返回 pending。由同事务读写和 O2 单向相位可知重启不重计，旧相位不会因为过期丢失；没有 token 双算副作用。

### 来源投影与读预算

`ProjectionBudget::check/row` 前置是本轮新建预算；check 以单调时钟判10秒，row 先验余量再增一次。恰1M行可成功，第1M+1行拒绝，不溢出；只修改函数局部计数。new 为 trivial 初始化。

`source_usage/sum_window` 前置是有效 source 与安全半开时间窗。复用 partition_window 的完整分钟/边缘互斥后置，完整分钟读原联合投影，边缘只读 consume 原事实。每页≤256、每行计预算并 checked 加算；同一时间的 ID 后继查完后切到严格后继时间，游标单调且范围有限，因此无遗漏/重扫/重复累计。超过值域或读预算返回错误，不发布部分总额；没有 DB 写。partition_window 保留原算法，新增的验证只放开图表31天限制，原 UsageQuery 仍有旧限制。

`project_source` 前置为当前 Volcano source、账本初始化/新鲜判定和同一预算。退休先返回不可授予；否则依原 Runtime 初始化/推进日格，transition 用下述历史 helper 保存。当前时间未进入周期则不读用量/消费队列。有效周期读取完整绝对用量并处理适配队列；只有首投影、队列消费或总额变化才生成新逻辑 cursor，原 apply_evidence 同时执行 period/cutoff/error_epoch、单调用量与 reason 规则，失败整轮拒绝。最后验证 Runtime，仅差异时写一次最终版本，ready 包含本源无待处理和全部来源门禁。因 Runtime 与请求相位/历史同事务，所以不会只清内存或只删除队列。

`record_period` 前置为原 advance_period 返回的合法过渡和旧 period_id。新键插入，旧键读回必须完全等值，否则拒绝；不以覆盖消除冲突。`apply_source_policy_tx` 仍按旧规则先计算 Runtime/配置，新增的 transition 保存发生于同事务，不改变控制命令的成功/失败边界。只写同一个历史表，不另存计量。

`ledger_ready` 前置为当前 schema。只读 state，取得 Recent；初始化与新鲜度分开返回。Scanning/Scanned 且无故障、存在非未来30秒内观测才返回 Some，否则 None，不把未初始化或采集缺口解释成0用量。不宣称采集完整性。

### 目标和内存发布

`targets/writable` 前置为同一事务取得的 work、原选择器结果及逐源投影。每个 work 消费一个对应优先级，未知 Keep 沿旧值；非 writable 原样跳过，剩余只有真实差异才 bump revision。非 Enabled→Enabled 同时更新原 enable_epoch/授权版本；其它转移不重复发授权。supersede 只针对未完成旧操作，不删成功回执/执行记录。最终 load_work 回读版本用于许可，未消费的优先级项判 Storage。循环上界原组成员容量，任何错误回滚所有先前组，而非部分生效。

`preview_volcano_round` 校验配置/时间后只读有界 snapshot，返回等待计数和0许可；不会模拟后提交状态，显式 rollback，无发布副作用。`commit_volcano_round` 校验后开一个 IMMEDIATE，读取有界 snapshot、来源投影、原组选择与目标，再以最终版本和三方最短 lease 构造许可。每源/组迭代均有容量及时间界限，ready 门禁失败只跳过正向许可；结果构造完成后才 commit，无提交后可失败查询。确认成功才能发布结果，commit 未知沿原 poisoned。scope 从完整 snapshot memberships 按来源类型筛选，包含 retired，不能从过滤退休的 group_work 推断所有权。

`commit_glm_round` 的计算/写入/原许可门禁不变，仅在同事务取得完整本类 scope。`apply_glm_cycle/refresh_volcano_policy` 在既有 worker publish fence 中调用上述提交，确认成功后 retain 非本类成员再 extend 本类新许可；火山 source_disabled 同理，另类的截止时间没有改写路径。预算 busy/事务失败不改变缓存；未知提交保留闭锁；dry_run 只用 preview 且不碰发布 epoch。无新增 await 内 Core 锁或后台任务。

### 打开与迁移

`migrate_v5` 前置为旧版全校验/私有备份已完成。一个事务换 meta、建三侧表和索引、保留 instance、改 user_version，再用 v6 schema/数据核验；失败回滚，未知提交 poisoned。没有从历史事实补入队列的 SQL。`verify_schema/open` 只把 v6 加到原逐版链，旧 DDL 和恢复规则不变。

`verify_observed` 前置为已确认 v6 结构及原 foreign_key/quick_check/ledger/policy 核验。拒绝队列 source 与事实不一致、历史指向未初始化/当前或未来 period、非法请求相位/时间、非法或倒退的 sqlite_sequence；请求遍历是有限逐行读取，不构建全表内存。数据检查与已有 schema/FK/Runtime 检查互补，任何错误拒绝打开，不自动修复。只读，备份仍沿原一致性拷贝。

完整性自检：上述前置、分支、正常/错误退出、副作用、callee 后置及循环终止已覆盖；纯字段投影/状态布尔组合为 trivial。静态负例及独立审核在本步审计记录中回填。
