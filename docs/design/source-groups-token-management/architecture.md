# 来源共享 分组路由和用量面板架构

日期：2026-10-15。状态：架构已确认，sgm-1 及其复审修复已完成，sgm-2 前两批及计划器小步已验证，持久执行/恢复未接线。需求依据：[最新版方案](../../research/source-groups-token-management-research.md)。接口实证：[实现前审核](review.md)。架构确认不包含生产部署授权。

## 目标与边界

| 目标 | 交付 |
| --- | --- |
| G1 来源共享与组隔离 | Source 唯一，Membership 对应独立 NewAPI 渠道，priority 按组独立 |
| G2 保持 GLM | 原 decide/eligible/临期/pin 语义不变，用共享来源快照驱动各 GLM 组 |
| G3 火山三态 | 连续错误与 token 监测阈值、每日重置、撤销错误判定，作用于来源全部成员 |
| G4 访问 key 管理 | 每 key 一个组，同 key 两种协议，管理员管理、使用者只调用 |
| G5 三页与数据 | 来源页、组与 Key 页、数据页；整体和 per-key 的分模型堆叠时序 |
| G6 可恢复与安全 | 持久目录和操作意图，状态回读，保留历史映射，不影响未授权的运行服务 |

首版单 quota-throttle 控制进程管理一个 NewAPI 实例，不设计多控制节点共识。保留原生 NewAPI 转发和重试，不新增推理代理，不实现逐访问 key 绑定，不以此功能为理由修改 NewAPI 路由代码。token 上限是已接受的监测阈值，既有在途请求和采集延迟可能超出，不实现硬额度预留。

来源全局启停是新增管理能力，不等于改写 GLM 的选择算法。暂停来源后不再让其加入候选；对其他相同输入，原 GLM 规则必须给出相同结果。

## 核心流程和权威状态

    客户端访问 key → NewAPI 鉴权与 group/model 筛选 → 组内渠道 priority → 真实上游

    管理员 → 三页面 → quota-throttle 控制接口 → 状态库中的操作意图
                                             → 协调器 → NewAPI 管理 API → 回读确认

    智谱 quota → 按 SourceId 采集 → GLM 各组原决策 → 该组成员渠道 priority
    NewAPI 日志 → 统一元数据账本 → 火山来源状态机／整体、来源、组、token、模型视图
    周期时钟 ─────────────────→ 火山来源状态机 → 协调器 → 全部成员渠道 status

状态归属：

- 来源、组、成员映射、火山周期/原因、组首选与控制操作意图，以 quota-throttle 状态库为权威。
- NewAPI 是实际 channel/status/priority、访问 token、转发日志的权威；本地保留期望值与已观察值，不把期望冒充已经执行。
- 模型目录按来源凭据发现一次，更新所有成员渠道的基础 models；发现失败保护存量。
- 图表与判据的本地账本是 NewAPI 可观测记录的投影，不是供应商真实余额，保留采集覆盖范围。

## 持久化选择和旧配置兼容

采用应用自己的 SQLite 状态库，而非直接读写 NewAPI 数据库。它用于稳定身份、操作意图、事实去重、检查点及周期汇总；这些更新需要事务，单一 JSON/TOML 文件与另外一份增量日志不易保证成组提交。

sgm-1 已采用 rusqlite 0.40.2（bundled、backup），版本锁入 Cargo.lock。只用本地单写者连接及有限批量，不引入独立数据库服务。数据库及凭据存储权限限制为当前用户，备份按敏感文件处理。访问 key 明文不另存副本。

旧 config.toml 保留服务连接、GLM 原调度参数等设置。首次显式迁移将旧 keys、备注、selector 与 default 组成员导入状态库；保留 NewAPI 原渠道 ID，不复制现有 default 渠道。迁移标记提交后，来源目录以状态库为准，重启不再次从旧 keys 复活已暂停或删除条目。明确导入/导出与回退流程在迁移子计划细化，不能默默形成两个来源真值。

测试和开发使用独立 state_path；正式库路径在部署时确认，不覆盖生产 one-api.db。schema_version 标识兼容性，旧版本遇到更高 schema 需拒绝写入；备份需使用一致性备份方式，不能忽略 WAL。

## 核心共享数据结构

字段为架构级契约，序列化名称、完整 JSON schema 与函数签名在各子计划细化。

### Source

- source_id：稳定主键；type 为 glm 或 volcano，已有成员后不允许就地换类型。
- name、note、endpoint、secret_ref、credential_fingerprint、quota_selector、model_profile、config_revision。
- manual_paused、retired_at；火山附加 VolcanoPolicy 和 VolcanoRuntime。
- 同一来源跨组只有一份记录；敏感指纹只供服务端去重，不展示或记录明文。
- 创建后可无成员；删除先保存停止意图，禁用全部成员成功后归档，SourceId 和历史映射不复用。
- 所有者：目录模块；消费者：采集、调度、协调器、面板。只有来源状态机修改运行期原因。

### Group 与 Membership

- Group：group_id、newapi_group、name、type、revision、ready；运行时持有当前首选及类型对应的选择状态。
- Membership：membership_id、source_id、group_id、channel_id、order、lifecycle、desired_priority、observed_status/priority、applied_revision。
- 唯一约束：有效 source_id/group_id 组合唯一；一个 channel_id 永远只映射一个来源和管理组。
- source.type 等于 group.type；一条受管渠道的 NewAPI group 只包含该管理组。
- 新成员创建经历 planned、created_disabled、ready；旧渠道导入为 imported，表示映射已存储但尚未由协调器核对，不改变外部状态。移除后 retired，历史映射仍可解析日志。
- Group 的独立 priority 不能覆盖 Source 的全局暂停/耗尽。各组可以恰好选择同一来源，不承诺配额隔离。
- 所有者：目录与协调器；消费者：GLM/火山组策略、统计、面板。

### VolcanoPolicy 与 VolcanoRuntime

- Policy：daily_token_limit 可空且非空时为正；failure_threshold 默认 3；reset_minute 默认 60；utc_offset_minutes 默认 480。
- Runtime：period_id、period_start、next_reset_at、used_input/output、consecutive_failures、error_exhausted、limit_exhausted、error_epoch、revision。
- 有效状态由 manual_paused 优先，其次任一耗尽原因，再次可用推导；不另存可能失配的第四种业务状态。
- 规则 2 由本周期已观察输入/输出合计与当前上限计算。提高上限立即重评，不能清规则 1；撤销规则 1 不能清规则 2 或暂停。
- 周期边界及运行状态跨重启保持；旧周期/旧 error_epoch 事件不能覆盖人工撤销或新周期。
- 所有者：单写者来源状态机；消费者：候选资格、协调器、数据视图。

### Fact 与 CollectionCheckpoint

- Fact：instance_id、native_log_id、native_request_id、type、occurred_at、channel_id、source_id_at_event、group_at_event、token_id、model、input_tokens、output_tokens、归一化错误类别及所需元数据。
- 去重主键至少为 instance_id/native_log_id；NewAPI 实例替换或数据恢复不能直接继承旧 ID 游标。
- 消费事实只来自 type=2；错误事实不增加 token。同 request/source 的重试结果须归并，其他来源的成功不能清掉本源失败。
- Checkpoint：查询窗、已处理边界、补采状态、覆盖起点/末尾、错误或缺口。事实、聚合与对应检查点在一个事务内提交。
- 历史 source/group/token 身份不因当前改名或成员移除变化；未知映射进入未归属项并报警，不静默丢弃。
- 所有者：账本采集器；消费者：火山判据与图表。不得保存 prompt、完整 key 或鉴权头。

### ControlOperation 与 TokenView

- ControlOperation：operation_id、kind、target_id、expected_revision、payload_fingerprint、phase、已确认资源、错误、created_at/updated_at。
- 同 operation_id 重放返回同一操作；载荷不同返回冲突。queued/applying/partial/unknown 不等于成功，超时也不等于未执行。
- TokenView：token_id、owner_scope、name、masked_key、explicit_group、effective_group、status、expires_at、限制；不带上游绑定。
- 新 token 通过托管非管理员调用身份签发。既有管理员 token 可按原身份管理，但不宣称它们具备新 token 的强分组隔离。
- 所有者：控制面/令牌管理；消费者：NewAPI 客户端及页面。

## 模块划分与功能规约

| 模块 | 前置条件 | 后置条件与不变量 | 副作用 |
| --- | --- | --- | --- |
| catalog/store | 合法身份、类型匹配、revision 一致 | 来源唯一，成员不可错认，变更或操作意图原子持久化 | 写应用状态库，不写 NewAPI 库 |
| source_probe | 来源已配置正确 quota/model 鉴权 | 每源采集一次，多组复用；未知不变成 0，模型失败保留存量 | 智谱/目录只读请求，更新观测快照 |
| glm_policy | 原阈值、组成员和同源 quota 快照 | 调用原决策规则，作用域仅当前组；相同输入结果等价 | 产生 priority 意图，不直接 HTTP |
| volcano_policy | 事实归属/版本有效，时钟可用 | 共享三态、两个原因独立；源状态变化使受影响组重选 | 写运行状态与源级同步意图 |
| reconciler | 持久操作意图、当前 revision | 全部对应渠道回读后才确认；不创建孤立活动副本 | NewAPI CRUD/status/priority 管理请求 |
| ledger/analytics | 管理消费/错误日志可读取 | 去重事实投影一致，保留 time/model/token/source 维度与缺口 | 预算内查询，事务更新账本/统计 |
| token_service | 受托管签发身份、组就绪、管理员操作 | 一个 key 一个组；生成归属可对账；明文按需短时返回 | NewAPI token 管理，不保存明文副本 |
| status/panel | 只读已发布快照或持久操作结果 | 三页面口径一致，期望与实际分开，火山无临期标签 | 本地 HTTP；写操作经统一控制入口 |

这些类型归属于本 feature，不为它们新增跨产品层框架。组、来源和统计虽跨模块共享，仍在本架构定义，避免重复定义枚举。

## 关键流程

### 加入来源到组

P1 校验来源/组未归档、类型匹配和成员唯一，保存创建意图与稳定操作关联名。

P2 以源配置创建 group 明确且 status=2 的渠道，priority 先设备用；火山 auto_ban=0，GLM 保持原配置。创建结果未知时按关联名对账，不重复 POST。

P3 将返回/解析到的 channel_id 与成员关系持久绑定，核对 group、models、status、priority。来源在此期间被暂停或版本变化时，保持禁用并重算目标，不用过期的启用决定。

P4 按全局源状态、成员状态和组资格决定是否启用；再由该组策略选首选。所有回读一致后 operation 才为 succeeded。

不使用 CopyChannel 裸复制活动渠道。API 创建/批量状态返回 success=true 也不能省略回读。

### 暂停 删除和恢复来源

来源管理器先持久化 manual_paused 或归档意图，阻止新成员启用与新首选选择；协调器按最新成员集合批量 status=2，再回读全部 ID。新增成员必须重查来源状态，不能在批量快照之后漏开一条。

部分失败保持停止意图，展示具体未收敛渠道并重试。来源已退休的成员历史不删除，不自动撤销上游供应商凭据。未完成不声称全局暂停成功，在途推理不自动中止。

恢复时只开启仍有效、业务可用的成员。火山旧首选渠道先恢复成备用 priority，再启用；各组当前有可用首选就保留，没有则自行选择。全局状态与组内 priority 是不同字段，组策略不得自行写 status。

### 组内选择与切换

GLM 继续原算法，用共享 Source 快照重映射为该组成员 ID 后决策；当前成员顺序和旧 default 渠道 ID 保持兼容。

创建组还需注册 NewAPI 组配置及签发身份可用组，读取后合并保留无关组与倍率；成员可路由、组注册和权限回读一致后才标记 ready。已有成员或访问 key 引用的组不能直接删除或更改类型，先明确处理引用，不把旧 key 静默改到 default。

火山正常保持首选，只在源不可用/移除或组首次选择时按稳定成员顺序轮转可用源。切换在组内串行，先解除旧的独占高优先级、回读，再提升新首选并回读；中间可能暂时没有唯一高优先级，显示切换中，不制造已确认的双首选。

priority 是对支持该请求模型的候选表达偏好。原生单次重试仍可临时走备用，不把一次成功重试解释为持久首选变化；不增加逐 token 绑定。

该路径的前提是没有适用于受管火山请求的亲和命中、指定渠道或跨组重试等覆盖规则。上线预检必须检查这些规则；有冲突则显示组未就绪并要求显式处理，不静默改动 GLM 或未纳管路由。新签发 token 指定单组、关闭跨组重试，普通签发身份的隔离另做负向测试；保留的管理员旧 token 不计入此强隔离保证。

### 火山错误 上限与重置

判据作用于 SourceId，不按成员副本独立计数。默认 3 个独立请求的不可用结果，成功清失败计数；已锁存错误判定只由人工撤销或日重置解除。已观测 token 达上限触发另一原因，两者可并存。

错误日志是异步观测，不承诺第三次失败的请求返回前已经停用全源。相同 request/source 的多个错误需去重；若存在同请求同源的最终成功，应按最终结果归并。终止结果识别、迟到处理、采集窗关闭与人工撤销 fencing 需要在 sgm-3 细化并故障测试，不能仅以收到三条日志实现。未就绪/缺口在面板显式呈现。

每天按源时区边界推进周期；manual_paused 保留，旧周期原因/计数重置。先推进周期再执行同刻管理操作，使用 expected_revision 防止重复覆盖。修改上限即时重评，修改时刻只调整下一个未来边界，不重记历史。

火山渠道 auto_ban=0，受管 status 使用 1/2，避免原生自动禁用成为第二业务写者；不改变 GLM auto_ban。原生外部修改仍可能造成漂移，协调器必须报告并按已授权的期望状态收敛，不能把未知的禁用一律叫额度耗尽。

### 统一采集与图表

单个采集器按固定截止时间分页采集必要消费和错误元数据，使用重叠窗与持久去重处理重复/迟到，完整批次落库后再推进覆盖标记。不能凭 type=2 缺失推断成功未消费，也不能只保存最近 100 条。

来源按历史 channel 映射合并，访问 key 按 token_id，组按发生时 group，模型按记录 model；各维度是同一事实表的投影。图表返回每时间桶的模型分项与总量。自然日按请求携带并显示的图表时区聚合；源重置周期单独展示，不把午夜切图当重置。

当前 /api/data/flow 可辅助区间对账，但没有 time bucket，不按“每 key × 每小时”反复调用。导出/保留期需要覆盖产品历史区间并标注覆盖起点，来源归档不能让历史串到新来源。

## 跨模块接口与分布式约束

### 面板到本地控制面

输入为受校验的实体 ID、expected_revision 和 operation_id；输出为快照或操作状态。写与敏感复制需同源/Host 校验和防 CSRF，不把存在自定义请求头当完整身份认证。默认只监听回环，远程暴露另行设计认证。

提交持久操作返回 202 与 operation_id，前端显示处理中并查询同一操作；完成才显示已暂停/已创建。不能以 HTTP 等待超时创建第二个逻辑操作。队列满立即 503，不静默丢弃写请求。

### 本地控制面到 NewAPI

1. 连接：复用 reqwest 连接池；正常断连接续后通过操作状态对账，不清除本地暂停意图。
2. 时限：建议 connect 3 秒、单请求总时限 10 秒；超时写结果记 unknown，不当成未执行。大批同步的总操作独立于一次 HTTP 时限。
3. 重试：保留现有会话 401 单飞重登一次。读取与已知幂等目标写最多重试 3 次，退避后继续后台对账；创建 token/channel 先查唯一关联，不直接重放未知 POST。
4. 交付/顺序：本地操作意图至少一次尝试，靠稳定 ID、revision 和回读实现有效一次；同来源/成员写有序，跨组 priority 不互相覆盖。不声明网络写具备 exactly-once。
5. 故障：管理面不可达时保存期望与旧观测，UI 显示未收敛；NewAPI 可能仍按旧配置转发，不能承诺本地停用已即时阻断。独立测试覆盖这一边界。
6. 会话：管理员渠道管理与普通签发身份分别持有会话；重登不混用 owner_scope，不把管理员新 token 当普通消费凭据。
7. 流控：建议总预算 1 请求/秒、突发 2，单实例共享；控制操作优先，历史补采低优先。内存工作队列上限 64，待执行操作已持久化且按实体合并；外部 429 退避，不扩增无限队列。

客户端到 NewAPI 的模型调用继续使用原生入口、鉴权、转换和流式实现；不由 quota-throttle 代理推理字节，也不承诺已输出的流可跨源续接。

## 并发与状态同步责任

- 状态库写入由单写者任务执行，事务内不发网络请求。采集结果、管理命令和时钟事件进入同一受控状态更新路径。
- 协调器是本工具唯一的 NewAPI 渠道写入口；来源管理、模型同步和组策略只产生字段级意图，不各自 GET/PUT 整对象竞争。
- 事务完成 happens-before 发布新快照。网络回读只能在 revision 仍匹配时确认 applied；旧操作回执不能覆盖更新的暂停或重置。
- GLM 外部探针在独立任务运行，慢查询不持有目录写锁。其失败语义仍是未知，不参与火山错误计数。
- 批量 status 后必须回读全部目标；失败不抹掉 Source 停止意图。组 worker 不负责恢复 source/status。
- 日志去重、模型时序投影和检查点同事务提交；旧数据重放不重复累计 token 或触发同一错误判定。

## 正确性论证

| 不变量 | 维护方与理由 |
| --- | --- |
| I1 每来源只有一份配额/异常状态 | SourceId 唯一；事实按成员历史映射合并；状态机不按 channel 建第二份原因 |
| I2 组内 priority 独立 | Membership 的 channel_id 不跨组复用，协调器只写目标成员；共享状态通过单独源级 gate 传播 |
| I3 GLM 策略等价 | 原决策函数不改规则，适配层仅分组选集与源快照映射，旧 default 迁移保留 ID/顺序；golden 回归检查 |
| I4 人工停用不会被重置/撤错绕过 | 状态由暂停优先表达式推导；reset/clear_error 不写 manual_paused；版本检查排除旧回执 |
| I5 同一消费只计一次 | instance/log ID 唯一键与事务聚合；原始事实只有一次，多个投影视图不再相加 |
| I6 未确认不显示已执行 | desired、observed、applied_revision 分开；只有目标回读齐全才能操作成功 |
| I7 新签发普通 token 无隐式跨组或换模型 | token 指定组、关闭跨组重试、成员类型匹配；原生筛选组/model；本工具不设置 auto 或偷偷映射模型。管理员旧 token 的特殊权限不在此保证内 |
| I8 凭据不经普通快照泄漏 | SecretRef 私有、token 列表打码、明文复制单独操作；文本渲染与日志脱敏 |

目标映射：G1 由 catalog/reconciler 实现；G2 由 source_probe/glm_policy 实现；G3 由 ledger/volcano_policy/reconciler 实现；G4 由 token_service/NewAPI 实现；G5 由 analytics/status/panel 实现；G6 由 store/ControlOperation/流控共同保证。

组合论证：成员唯一性使组写入互不覆盖；共享事实把多组实际消耗收敛到一个来源；来源状态产生全量成员 gate，组策略只在 gate 内选首选。事务先于快照且外部写有回读，使“期望停止”和“实际停止”可区分；各视图从同一去重事实投影，保证相同完整区间下模型分项总和等于总量。

关键假设：只有一个控制进程；本地磁盘持久化可用；受管渠道不被其他自动管理器并发改写；受管火山请求没有绕过 priority 的亲和/指定渠道规则；当前部署日志为 SQLite 管理日志，ID 是稳定记录身份；火山规则 1 上线前已确认 ERROR_LOG_ENABLED=true 且相应错误可记录；时钟与源配置的固定 UTC 偏移正确。任一前提失效要显示未就绪/漂移/数据缺口，不能默认为可用或零消费。

## 阶段门禁

架构已确认；第一子计划目录库已实现并完成 §6 双路径验收，见[细化](subplans/sgm-1-catalog-store.md)和[审核报告](../../audits/sgm-1-catalog-store/audit-report.md)。其余子计划仍须独立经过细化确认、§4.4 审查与 §6 双路径验收。实施顺序与退出条件见[实施准备计划](implementation-plan.md)。

本轮不修改运行服务或导入生产数据。第一批目录函数不接入 up/run/sync 或面板写接口；其本地保存成功不代表外部渠道启停成功。

## 第一子计划共享类型补充

以下将架构字段落实为 sgm-1 的共享契约，完整校验和 SQL 约束见子计划 §4。消费者是 catalog、后续 source_probe、reconciler、policy、analytics 和 panel；本轮不提前实现这些后续模块。

| 类型 | 字段与约束 | 身份与生命周期 |
| --- | --- | --- |
| SourceKind | 唯一枚举定义 glm / volcano；由配置类型决定，不从模型名推断 | 创建来源/组时确定，sgm-1 不提供改类型接口 |
| SourceId / GroupId / MembershipId / SecretRef | 独立 newtype，内部正 i64；公开 ID 序列化为十进制字符串，SecretRef 不序列化到快照 | SQLite 分配；已提交 ID 不复用；引用不可互换 |
| SourceSettings | tagged enum：Glm { upstream, quota } 或 Volcano { upstream, policy } | 来源级设置，不复制到成员状态；私有存储格式 version=1 |
| UpstreamProfile | channel_type、base_url、models、model_discovery；models 非空且规范化，发现鉴权复用 ModelDiscoveryAuth | 按来源存储；不含 group、priority 或实际 NewAPI status |
| GlmQuotaProfile | quota_url、global_headers、key_headers | 两层 header 原样保留，后续按既有构造顺序使用，不趁迁移改写 GLM 请求语义 |
| VolcanoPolicy | daily_token_limit、failure_threshold、reset_minute、utc_offset_minutes | 默认无上限 / 3 / 60 / 480；sgm-1 仅存配置，不创建虚假的可用运行状态 |
| SourceView | source_id、type、name、note、manual_paused、retired_at、config_revision | 只读安全视图；不含密钥、指纹、SecretRef、selector 或私有配置 |
| GroupView | group_id、newapi_group、name、type、revision、ready | sgm-1 创建/导入时 ready=false；后续组注册和权限核对后才能置真 |
| MembershipView | membership_id、source_id、group_id、channel_id、order、lifecycle、desired_priority、observed_status、observed_priority、applied_revision、retired_at | 新建 planned 且 channel_id=null；导入 imported 且保留旧 channel_id；历史映射永不改认其他来源/组 |
| CatalogSnapshot | schema_version、sources、groups、memberships | 一致性只读快照；实体按稳定 ID 排序，包含退休身份；不是带凭据的恢复备份 |
| SourceDraft / GroupDraft | 前者为 name、note、settings、credential；后者为 name、newapi_group、type | 进程内请求，由 catalog 完整校验；不直接作为面板 wire 类型 |
| SourceMaterial | settings、credential | 仅后端 probe/reconciler 消费；无 Serialize，Debug 脱敏 |
| LegacyChannel / LegacyImportPlan / ImportOutcome | 无凭据渠道描述、规范导入内容及摘要、Applied/AlreadyApplied 收据 | 迁移调用方与 catalog 共享；Plan 字段私有，只由纯转换构造，收据无凭据 |

共享枚举在 src/catalog/types.rs 唯一定义；ModelDiscoveryAuth 沿用 config.rs。JSON 字段契约由 [schema](schemas/catalog-snapshot.schema.json) 固化。目录的请求类型与导入收据只在进程内传递，不是面板 API；外部操作 ID 和错误码待控制面子计划定义。

## 第二子计划共享类型补充

以下为 [sgm-2 细化](subplans/sgm-2-channel-reconcile.md) 的共享接口；不改变既定模块职责或提供者策略。

| 类型 | 共享字段与约束 | 所有者和消费者 |
| --- | --- | --- |
| OperationId / ControlCommand | 稳定幂等 ID、受管动作、目标身份、expected_revision 与规范参数；同 ID 不变载荷 | 控制入口 → catalog/control；具体字段见子计划 §4.1 |
| MemberTarget / StatusDirective | 成员 revision、priority、preserve/enabled/disabled、退役请求、稳定创建关联名 | catalog 写；reconciler 与后续策略读；源级 gate 始终更高优先 |
| RoutingPermit | source/group 身份与 generation、启用资格、期望 priority；非序列化 | 策略/预检层产生，协调器消费；前端不得提供许可布尔值代替它 |
| WorkTicket / WorkSnapshot | 成员动作与 member/source/group generation、写执行序号，一致读取的目标及完整可能路由的同组状态（含退役中成员） | store/worker/planner；纯计划类型已实现，持久票据/装配仍待接线；不包含公开可序列化凭据 |
| BackendReadiness | 已核实的管理能力、原生自动启用开关、实例绑定及检查时间 | 只读预检 → 协调器；冲突不静默修改现有全局配置 |
| ChannelObservation | 实际 channel 身份、路由字段、原生状态及观测时间 | NewAPI 客户端 → reconciler；原始 JSON 私有，未知不当零 |
| OperationView | 操作身份/类型/阶段、目标 generation、确认计数、安全错误码及时间 | store 发布给面板；具体格式由 [schema](schemas/control-operation.schema.json) 约束 |

创建关联名使用本工具实例 ID 与 MembershipId，不用 key 或可改展示名。状态管理区分 GLM 正常 preserve 与火山显式 1/2；人工暂停/退役覆盖两者。数据库 v2 通过私有侧表保存控制数据，核心 CatalogSnapshot wire 版本仍为 1；操作历史保存匹配 generation 的确认收据，不能因后来变更丢失旧成功证据。

NewAPI 无远端 CAS：本地 revision 防止错记旧 ACK，不等于可以撤销已经发出的写。目标完成以匹配回读为依据；未决相反写与漂移必须显式显示，并在既定单写者假设下持续对账。不能把此执行层宣称为硬实时或严格 exactly-once 控制器。

sgm-2 源码核对发现原生自动启用也可能成为并发状态写者。因此受管状态模式上线前必须确认其关闭；若现有实例开启，不自动修改、不中断旧部署，须在发布阶段另行确认处理方式。GLM 的原 auto_ban 和额度选择规则不变。此为细化新增的部署前提，不是已核实当前生产配置满足条件的声明。
