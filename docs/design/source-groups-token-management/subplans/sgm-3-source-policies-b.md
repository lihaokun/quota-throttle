# SGM-3 第二批火山状态与策略持久化

2026-10-17 输入契约更新：用户确认[观测型熔断](sgm-4-observed-breaker.md)，旧 FinalOutcome/completed_at 内部名称改为 OutcomeObservation/observed_at；同请求同源可以先输出一次失败观测，再输出一次成功 token 观测，重复事件不输出。下文旧“一请求一次确认终态”生产者义务由该新细化替代；B 的成功清计数、错误锁存、代次/周期/游标和事务算法保持。

状态：用户确认的 B 已于 2026-10-16 完成纯函数、v4 存储、复杂历史迁移组合及[整体退出验收](../../../audits/sgm-3-source-policies/batch-b-final-audit.md)，历史小步见[纯函数审核](../../../audits/sgm-3-source-policies/batch-b-pure-audit.md)和[存储审核](../../../audits/sgm-3-source-policies/batch-b-storage-audit.md)。设计基线 `2ec8b50`，纯函数基线 `7f4fe7b`，存储基线 `0b52e50`，组合验证基线 `e0cfb82`；属于 [SGM-3](sgm-3-source-policies.md) 的 B 批。C 接线和生产发布仍未实施，GLM 算法、认证修复和既有部署架构不变。

## 1 范围与交付边界

本批实现火山三态、独立耗尽原因、日重置、修改策略、撤销错误判定、稳定组首选，以及来源运行状态/GLM 和火山组选择的应用库持久化。复用当前 CatalogStore 和 SQLite，不新增服务、依赖、推理代理或每访问 key 绑定。

只提供内部函数和事务接口，不接 CLI/HTTP/面板，不启动调度或日志采集，不改 NewAPI status/priority。组选择是本地策略结果，不是远端已切换。后续 C 批统一处理操作幂等收据、状态/渠道意图原子发布及协调器许可；SGM-4 唯一负责原始事实、终态归并和历史统计。

两条耗尽规则均作用于 SourceId：默认连续 3 个独立请求的上游不可用结果；可选本周期 input+output 软上限。手动暂停高于两种原因；清错误只清错误原因和连续计数，不清 token、不改暂停。火山没有临期或积分换算。

## 2 复用与错误处理

- 复用 SourceKind、SourceId/GroupId/MembershipId、VolcanoPolicy、WorkSnapshot、GlmGroupState/Regime，不重写 GLM decide。
- VolcanoPolicy 校验沿用现值域：上限为 None 或 1..=JSON_SAFE；阈值 1..=100；重置分钟 0..1439；固定 UTC 偏移 −720..=840 分钟。提取现有校验供策略编辑复用，不改变旧接受范围。
- 复用 CatalogStore 的私有权限、OS 锁、事务 commit/未知提交毒化、版本核验和一致性备份；不绕开单写者。
- 纯函数沿用 PolicyError：非法状态/时间/输入为 InvalidInput，批量超界为 Capacity，计数/版本不可表示为 SequenceExhausted。存储层复用 ControlResult 的 InvalidInput/NotFound/NotReady/RevisionConflict/Storage，不增加公开错误码。
- 存储映射：非法输入/批量超界→InvalidInput；不存在→NotFound；退休/未初始化或投影缺口→NotReady；版本冲突→RevisionConflict；数据损坏、计数耗尽、SQL/提交失败→Storage。缺必需状态行属于损坏，不是可自动初始化的不存在。
- 计算先在副本上完成，校验失败不发布半个状态。SQL 失败回滚；提交不确定沿用 poisoned，不能伪装保存成功。无错误正文、凭据或供应商报文进入状态。

## 3 数据结构与值域

本批类型在进程内使用，输入不实现 Deserialize。公开 CatalogSnapshot/OperationView wire 仍为 1，不新增面板 JSON。持久字段只有状态和身份，不保存原始日志或请求正文。

### 3.1 来源运行状态

`VolcanoRuntime` 字段：

| 字段 | 语义 |
| --- | --- |
| source_id、revision | 来源身份、运行状态版本；revision 与 Source.config_revision 分开 |
| period_id、period_start、next_reset_at | 来源私有周期身份及半开区间 [start,next)；period_id 不等于自然日序号 |
| used_input、used_output | 本周期已观测累计值，不是余额；缓存已计入 input 时不再加算 |
| consecutive_failures | 连续已确认的不可用结果数；成功归零，排除项不改变它 |
| error_exhausted、limit_exhausted | 两个独立原因；错误原因锁存，上限原因随当前上限重新计算 |
| error_epoch、error_cutoff_at | 错误判定代次；人工撤销时的秒级边界，未撤销可空 |
| evidence_cursor、coverage_end | 已接受账本投影游标及本周期已扫描截止；均可空，不把尚无观测显示成成功查询到 0 |

时间统一为 UTC epoch 秒，周期按固定时区偏移计算，不依赖主机时区/DST。revision/period_id/error_epoch 为 1..=JSON_SAFE；非负计数/游标不超过 JSON_SAFE，input+output 也必须可表示。时间端点为 0..=JSON_SAFE，start<next；cutoff 若有须位于当前周期。初始化和计算必须能表示前后边界，否则报错，不截断。

`VolcanoState` 只定义 Paused/Available/Exhausted：manual_paused → Paused；否则 error_exhausted OR limit_exhausted → Exhausted；否则 Available。不持久化派生 state，避免旗标失配。Available 表示未被来源策略禁用，不证明供应商健康或日志采集已就绪。

来源暂停只由既有来源管理写入；本批所有函数均不得修改 manual_paused。退休来源保留历史状态，但拒绝新增策略写入。

### 3.2 可信事实批次

`VolcanoEvidence`：source_id、period_id、error_epoch、from_cursor: Option<i64>、to_cursor: i64、used_input/used_output、coverage_end，以及最多 1024 个 `FinalOutcome`。

`FinalOutcome`：first_event_at、completed_at、kind（Success/Unavailable/Ignored）。调用者保证它已经按 (instance, request_id, SourceId) 归并；不是一条原生错误日志。first_event_at 是该请求在本来源已记录事件的最早时刻，用于撤错/周期隔离，不冒充精确请求开始时间。

SGM-4 生产者义务：

1. 一次真实来源请求只产生一个确认终态；同源最终成功优先，其他源成功不改本源结果。不能确认终态的请求暂不输出 Outcome，消费统计可独立继续。
2. 结果按账本确认终态的稳定顺序追加；批次是 from_cursor→to_cursor 的完整增量，旧批次内容不可变。不能拿 API 分页到达顺序或原生日志 ID 当作本契约的连续结果顺序。
3. token 是当前周期全部渠道副本的去重绝对累计值；不是增量重复相加，不包含别处使用同一上游 key 的不可见消耗。
4. 使用生产批次时捕获的 period/error_epoch；撤错或重置后重建投影，不把旧批次改贴新代次。覆盖信息不能单独作为终态证明。

本批只验证该接口的消费者及合法 fixture，不能证明真实生产者已经实现。其游标只是保存消费位置，不在 SGM-3 建第二份 request 去重账本。

### 3.3 组状态与周期过渡

`GroupPolicyState = Glm(GlmGroupState) | Volcano(VolcanoGroupState)`。火山 state 含 preferred: Option<MembershipId> 和 cursor: Option<(order, MembershipId)>。cursor 是最后一次选择的位置；全员不可用或该成员移除后仍保留，以便恢复时从稳定轮转位置继续。

`StoredGroupPolicy` 保存 group_id、独立 policy_revision 和上述 state。引用的成员必须属于本组；历史退休成员仍可作为旧选择/游标读出，由下一次策略计算处理，不能把跨组引用当旧状态容错。

`PeriodTransition` 返回旧周期起点/原 next、当前新起点/next、旧重置规则。跳过多日使用紧凑边界与规则表达，不生成逐日空记录。C/SGM-4 将该过渡写入历史账本时必须与运行状态同事务；本批只返回它，不把当前状态表当完整历史账本。

### 3.4 内部请求和返回

`SourcePolicyUpdate` 为 Tick、Evidence(VolcanoEvidence)、ClearError、ReplacePolicy(VolcanoPolicy)。`EvidenceDisposition` 为 Applied/Duplicate/Stale/Gap。`SourcePolicyWrite` 返回安全 SourceView、新 Runtime、可选 evidence disposition 和可选 PeriodTransition；不返回凭据或远端确认。上述类型供 catalog/C/SGM-4 使用，不是新面板命令。

## 4 私有数据库 v4

冻结 schema_v1/v2/v3.sql，新增 schema_v4.sql：保留实例 ID，重建 catalog_meta 的版本约束为 4，新增以下两张实际消费的状态表，不重建旧目录/控制/执行表。

### volcano_runtime

source_id 为主键；kind 固定为 volcano，通过既有 sources(source_id,kind) 唯一键建立复合外键。包含 §3.1 的其余字段。

每个火山来源必须恰好有一行；GLM 不得有此行。新来源/旧库迁移先创建未初始化行：revision=0、period_id/start/next=NULL、计数/旗标/error_epoch=0、cutoff/cursor/coverage=NULL。初始化后 revision/period_id/error_epoch≥1，端点齐全且符合 §3.1。两种形态由 CHECK 明确区分，不能把数据缺失当合法零状态。

未初始化是存储生命周期，不是第四个业务状态。读取返回 None/NotReady；显式 initialize 才建立周期。缺整行或已初始化行字段缺失均报损坏，不能静默重新创建而清掉耗尽原因。

### group_policy_state

group_id 为主键；kind 使用既有 SourceKind，通过 source_groups(group_id,kind) 建复合外键；字段为 revision、active_member_id、pinned_member_id、regime、cursor_order、cursor_member_id。成员 ID 外键引用永久保留的 memberships，group 归属由事务写校验及重开核对保证。

GLM：regime=normal/degraded，cursor 两字段空，active/pinned 可空。火山：pinned/regime 空，cursor 两字段同时有值或同时空；active 表示 preferred，active 非空时 cursor 必须存在且指向该成员，active 为空时可保留历史 cursor。revision≥1。每个组必须有一行，新增/迁移初值为空选择，GLM 为 Normal。旧服务真正的活动/pin 导入属于 SGM-7，不由本批猜测线上状态。

insert_source 在原事务追加火山未初始化行；insert_group 在原事务追加对应组初值。insert_membership 不额外复制来源运行状态。

## 5 纯函数细化

以下函数无网络/存储副作用；内部算术使用 checked 运算，不靠 release 溢出回绕。所有非成功返回均保留输入。

### 5.1 日边界和初始化

`daily_bounds(now, policy) -> Result<(start,next)>`。pre：policy 通过既有校验、now 与结果端点可表示。令 D=86400，offset=utc_offset_minutes×60，r=reset_minute×60；start=floor_euclid((now+offset−r)/D)×D+r−offset，next=start+D。校验 start≤now<next 后返回；非法或溢出报错。等于重置秒时返回新周期，next 总严格在未来。固定次数运算，无逐日循环。

`initialize_runtime(source_id, policy, now)` 调 daily_bounds，建立 period_id/error_epoch/revision=1，计数/旗标为 0，cutoff/cursor/coverage 空。没有事实覆盖时只称“尚无观测”，不声称查到了零消耗。若已经初始化，由存储入口返回旧状态，不重新初始化。

论证：欧几里得商定位唯一日格；相邻边界相距 D，明确 offset 保证主机时区无关；先检查再构造，无半初始化结果。

### 5.2 推进周期

`advance_period(runtime, policy, now) -> Result<(runtime, Option<PeriodTransition>)>`。先校验状态、policy 和 now 的非负安全范围。若 now<period_start 或 now<next_reset_at，返回原状态，不倒退周期；若 now≥next，令 k=floor((now−next)/D)，new_start=next+k×D、new_next=new_start+D。

新 period_id 和 error_epoch 各加 1；清两类原因、两类 token、连续失败、cutoff/cursor/coverage，保留 Source 上的手动暂停。返回旧边界/规则和新边界，不能把旧周期终点改成 now。多日离线只进行常数次运算；未实际创建的空日不分配虚假 period_id。

论证：new_start≤now<new_next，下一次相同 now 不再进入重置分支；旧周期/错误代次不再匹配，重复重放不会再次清空。存储 revision 不由本函数累计，统一在事务提交处提升一次。

### 5.3 用量与结果

`apply_usage(runtime, policy, input, output)` 校验非负、安全合计、同周期累计不下降，再替换绝对累计值；limit_exhausted = limit 存在且合计≥limit。缺省上限为 false。不写 error_exhausted、失败次数或暂停。

`apply_outcome(runtime, policy, outcome)` 校验 first_event_at≤completed_at 且时刻可表示；不属于当前周期的首次事件忽略；有人工 cutoff 时 first_event_at≤cutoff 也忽略。同秒边界保守排除，避免撤销前的秒级日志重新触发；下一秒的新事件正常参与。

其余结果：Ignored 不改变计数；Success 将连续计数归零但不清已锁存错误原因；Unavailable 将计数 checked_add(1)，达到当前 failure_threshold 时锁存 error_exhausted。完成时间必须落在当前周期，已跨出的旧周期结果不应用到新周期。消耗归属独立按记录记账时间计算，不因某个错误结果被忽略而漏掉合法 token。

`apply_evidence(runtime, policy, batch, now) -> Result<(runtime, EvidenceDisposition)>` 顺序为：校验源身份/批量上限/数值 → period/error_epoch 不匹配返回 Stale → 已接受游标且 to≤当前游标返回 Duplicate → from 与当前游标不同或区间倒退返回 Gap → 在副本上 apply_usage → 按批次顺序 apply_outcome → 更新 cursor/coverage，返回 Applied。Stale/Duplicate/Gap 携原状态，不发布半批。Gap 在存储入口映射既有 NotReady，不扩充 PolicyError。

首批 from=None，可用 to=0 表示空投影；后续新批 to 必须严格增加。coverage 为半开扫描区间的上界，在当前周期内且不下降、不超过 min(next_reset_at,now+1)；输出 Outcome 的 completed_at 必须早于该上界，未来记录不能提前改变状态。中途任一非法结果使整批不生效；最多 1024 项，每轮消耗一项，必终止。旧批次不能重算 token 或累加失败；真实请求跨批去重仍是生产者义务，不凭消费者游标宣称原生去重已经完成。

空投影 to=0 只允许空结果列表及 input/output=0。结构数值校验先于 Stale/Duplicate；只对真正要应用的新批次校验当前周期、单调用量及覆盖范围。当前 now 必须落在已推进的周期内；纯函数不会替调用方隐式重置。

### 5.4 撤错和编辑策略

`clear_error(runtime, now)` 要求 now 位于已推进后的当前周期，且不早于已有 cutoff；否则报 InvalidInput。清 error_exhausted/连续计数，error_epoch 加 1、cutoff=now、evidence_cursor=None；保留 token、limit_exhausted、coverage 和手动暂停。重置投影起点用于下一批按新 cutoff 重建错误增量，绝对 token 不能重复相加。

`replace_policy(runtime, old, new, now) -> Result<(runtime, Option<PeriodTransition>)>` 先校验 new，再调用 advance_period(old,now)。改时刻/时区时，用 daily_bounds(now,new).next 替换 next_reset_at，保留当前 period_start、用量和原因；新边界必须晚于 cutoff。改变上限立即调用 apply_usage 重评；错误原因保留，降低 failure_threshold 到已有连续计数以下可锁存，升高阈值不解除已锁存原因。若先前推进产生过渡，保留旧边界/旧规则，并将其当前新终点更新为最终 runtime.next_reset_at，不能丢掉过渡或把中间值当最终周期。

论证：推进旧到期边界早于编辑，排除配置保存凭空恢复额度；撤错和改上限只写各自原因，OR/暂停优先表达式持续成立。重复相同配置且无到期/状态变化不提升版本；明确的撤错是一次新状态变更，前端重试幂等由 C 的 operation_id 收据承担。

### 5.5 组内稳定首选

`select_volcano_group(cfg: &Config, group: &GroupView, members: &[WorkSnapshot], shared_states: &BTreeMap<SourceId, Option<VolcanoRuntime>>, previous: &VolcanoGroupState) -> Result<VolcanoGroupDecision>`。pre：完整同组 WorkSnapshot、SourceId 唯一的共享火山状态；容量沿用 MAX_SOURCES/MAX_MEMBERSHIPS。配置 priority 须安全且 active>standby≥exhausted，非法返回 InvalidInput，不改变旧 GLM 的配置校验行为。先核对组/来源类型、身份/版本及未 retired source/group 唯一性；所需来源缺 map 项为非法，显式 None 才表示未初始化；有 Runtime 时其 source_id 必须与 map 键一致。再过滤暂停/退休/退役意图/未绑定成员和未初始化/耗尽来源，不能先过滤后掩盖重复身份。

按 (order,MembershipId) 排序。preferred 仍在可用集则保持；否则从 cursor 后的第一个可用成员选择，尾部无候选则回绕到第一个；首次无 cursor 就选第一个。全员不可用返回 preferred=None，但保留 cursor。新选中时更新 cursor；返回来源恢复不抢占仍可用首选。

`VolcanoGroupDecision` 包含 group_id/group_revision、下一 state、eligible、所用来源 config/runtime 版本及成员 priority Option。输出排除已退休或退役中的成员/来源；source_revisions 也只包含保留成员引用的来源，按 SourceId 顺序输出，仍对被过滤输入先做身份核验。余下成员中，明确暂停/耗尽为 exhausted，未绑定或未初始化为 None（Keep），首选为 active，其他可用为 standby。未知成员不获得启用资格。阈值与 priority 取现有合法配置，不硬编码默认值。没有临期排序、逐 token 绑定或模型改名；C 将 Keep 转成保留既有目标，并让源级 status gate 高于 priority。首选只是对支持该模型的原生候选的偏好。

论证：稳定有界排序后最多扫描一次，唯一选中或无选中；成员身份而非 SourceId 作为组内结果，故同源不同组互不覆盖。当前可用优先分支保证粘滞；游标保留覆盖成员移除和暂时全不可用的恢复。

辅助函数契约：VolcanoRuntime::validate_shape 检查数字范围、周期/截点、cursor 有值时 coverage 非空；validate 再核当前 policy、上限原因、未锁存错误的阈值关系及 next 日格。来源选择无私有 policy 入参，依赖 catalog 已做完整校验；previous 的历史成员归组也由 catalog 读出校验，纯函数只核其 preferred/cursor 形状，允许已移除成员不在当前候选中。apply_usage 为编辑上限复用，仅先核旧状态形状，再重算新上限原因，不用新上限拒绝旧旗标；调用方最终作完整校验。state 和数值谓词为无副作用的简单分支；sum/increment 只返回安全计数或 SequenceExhausted；Outcome 校验检查有序安全时刻，不归并原生日志。

## 6 存储函数和并发责任

`src/catalog/policy.rs` 的接口可见范围仅 crate 内；复用 ControlResult。纯策略在 `src/source_policy/volcano.rs`，不让 SQL 层重写算法。

| 入口 | 校验与步骤 | 返回与副作用 |
| --- | --- | --- |
| initialize_source_policy(source_id, expected_source_revision, now) | 核可用 Store/来源类型/未退休/配置 revision；未初始化行调用 5.1，已初始化不清旧值 | 返回现有或新 Runtime；首次初始化一事务，无 HTTP |
| load_source_policy / load_group_policy | 有界单行查询、严格解码和跨表身份校验；缺少必需行返回 Storage，未初始化 Runtime 返回 None | 只读，返回 Runtime 或 StoredGroupPolicy，不以默认值修补损坏 |
| apply_source_policy(source_id, expected_source_revision, expected_runtime_revision, update, now) | IMMEDIATE 事务内复核两种 revision；未初始化 NotReady；Tick/Evidence/ClearError 先 advance_period 再处理动作；ReplacePolicy 调 5.4 内部推进一次；保留过渡，完整校验候选状态后写入 | 内容变化才提升 runtime revision 一次；策略配置变化另提升 source.config_revision；返回 SourcePolicyWrite，不写渠道 |
| save_group_policy(group_id, expected_group_revision, expected_policy_revision, state) | 同事务核类型、成员所属组与两个 revision；允许同组历史引用，拒绝跨组引用；相同内容 no-op | 持久选择版本，不产生 SetGroupPriorities、许可或远端成功收据 |
| verify_policy | 验证每个火山来源/每个组都有且只有合法行，类型、范围、跨组引用、初始化形态；limit_exhausted 与现有策略/合计一致，未锁存错误不能已有超阈值计数；next_reset 符合当前固定偏移的规则 | 只读；坏行拒绝打开，不补表/清零 |

update 是内部 enum，不是新增 ControlCommand 或前端 JSON。证据批次 Stale/Duplicate 不能改原因；若同次调用恰逢应重置，已完成的合法时间推进仍可提交。非法输入、Gap/NotReady 或版本冲突回滚整个事务，包括本次临时计算的重置。

事务级 helper 接受已有 Transaction，供 C/SGM-4 把运行状态、操作/事实检查点、历史边界和成员目标组成一个事务；本批便利入口只验证本地持久化，不声称已具备该组合保证。跨重启必须先读取持久值、运行首次 Tick，再发布路由许可；CatalogStore::open 不擅自读取系统时钟清空额度。

事务 helper 的调用前提：由已通过 usable 的 Store 单写者持有 IMMEDIATE 事务；任何 Err 都须回滚整个组合事务，不可捕获错误后提交前半段。便利入口以 RAII 回滚错误并复用 commit 的未知提交毒化。read_runtime/decode_runtime 严格区分缺行、全零占位和完整初始化；read_group_policy/decode_group_policy 核类型专属字段，再核成员归组。source_record/group_record 复用既有目录解码；前者只读本次所需配置，不读取 raw_key。get/volcano_policy/policy_error 为简单解码/类型分支/错误映射；write_runtime 仅按主键更新且要求恰好一行，不使用 UPSERT 补损坏。

存储正确性论证：先读出并验证旧状态及两个版本，才计算候选；纯算法错误不触碰 SQL。配置内容与运行内容分别比较，故只改未触及上限时仅提升 source revision；需要写时同一事务提交，SQL/递增失败全部回滚。组状态先验证类型与归属，再进行 no-op 判断/提升版本，不以内容相同绕过版本冲突。verify_policy 限界读取来源/组 ID，再核行数、逐行值域及引用；不把缺失自动初始化。默认行与目录行在同一事务建立，故失败时不会只留下来源凭据或空组。

所有运行期写入最终由既有 worker Core 单写者调用，网络不在事务/同步锁内。本批不新增 worker 写入口；C 接线前不得从旁路获取 Store 修改在线状态，也不得将新的 SourceSettings/Runtime 或组状态保存当成实际恢复成功。

## 7 迁移和兼容

`migrate_v3`：旧库先完整只读核验；原始 v3 升级前生成私有一致性备份，再在单事务创建 v4、回填火山未初始化行和全部组初值、保留 catalog_instance_id、更新两个版本标记，运行 verify_policy 后提交。旧 v1/v2 先走原有链，再升级 v4；保留进入 open 时原版本的备份，不承诺整条链原子回滚。

verify_schema 组合冻结的旧 DDL 时，仅跳过已被后续版本替换的 catalog_meta；v4 仍核验全部 v1/v2/v3 业务表、索引和约束。旧 v3 非法库不能先升级再掩盖损坏。v4 缺状态行拒绝，不以 initialize 修复；外库/更高版本不写入。

旧目录身份、凭据、SourceSettings、成员 priority、控制操作/确认收据、执行记录均保留；原 recover_executions 继续处理未决网络执行。新空库沿相同链初始化。生产数据库、本地 NewAPI 库和现有 config.toml 不在本批迁移范围。

实现后须将 v4 归属、open/insert_source/insert_group 的新增行为登记到迁移范围文档及本批审核，保留 SGM-1/2 的历史基线，不把旧报告直接改名为新版本已通过。

## 8 真实事实接线前的验证门禁

固定 NewAPI 提交的日志结构提供 request_id、channel、created_at、use_time 和消费 token；错误记录是在每次尝试失败后、决定是否重试之前写入，不能把 error log 直接当终态或把 created_at 当精确请求开始时间。[日志结构](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/log.go#L54) [错误与重试路径](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/relay.go#L205)

C/SGM-4 必须另行给出并实测：终态识别、同源重试最终成功、换源成功隔离、迟到/乱序/缺页、撤错前旧事件的归并、日志缺口、流式取消/完成。未知终态不能计作 Unavailable，也不能因一个等待时长过去就假定结束；原生错误日志能力和路由覆盖规则未验证时不能启用自动模式。

本批不为尚未确认的元数据能力伪造日志生产者。模型分项用量、历史边界落库和正常/异常采集覆盖属于统一账本；本批必须保留所需输入/过渡接口，不能另建独立统计来源。此门禁是既有实施顺序的依赖，不是改变已确定的火山业务规则。

## 9 实施与验收

2026-10-16 测试门禁修订：最大选择/迁移的5秒预算与普通功能/PBT分离，使用独立串行release性能入口及CI检查；原容量/材料保真/回滚断言不减，见[修复说明](../../../fixes/sgm-fix-parallel-performance-gates.md)。

按纯函数 → v4/存储 → 重开/迁移组合三步实施。每步完成测试、自审与 workflow 要求的独立契约审核；只有 B 的退出项全部通过才进入 C，不以状态机 fixture 冒充实际流量验收。

事前测试见 [B 批 expectations](../../../audits/sgm-3-source-policies/batch-b-expectations.md)。PBT 覆盖独立原因、时钟/改规则、证据幂等、轮转及持久化保真，至少 128 cases/项；旧 GLM golden 继续运行。变异必须源码/target 双隔离。

变更仅 source_policy 的 B 实现/测试、catalog 新策略持久化及迁移相关代码/测试、共享类型补充和本批机械脚本。旧 DDL、GLM 算法、NewAPI 管理/认证、worker 实际发布、CLI、面板零业务修改。旧测试保持原断言；从当前库构造历史版本的 fixture 只追加移除 v4 侧表，保留真实旧版本结构，不批量替换历史字段、版本或约束。

## 10 完整性检查

- [x] 类型、单位、初始化/生命周期与单写者责任明确。
- [x] 函数前后条件、错误/空输入/旧版本/时钟回拨/溢出分支已列出。
- [x] 循环有明确上界，错过多日不逐日循环。
- [x] 持久化和迁移契约覆盖现有 open/insert 链，旧 DDL 不改。
- [x] 与 C/SGM-4 的生产者和组合事务义务分开，不假称已接线。
- [x] 用户已确认 B，按纯函数、存储、组合三步实现；C 的事实能力验证及运行接线仍须后续设计/验收。
