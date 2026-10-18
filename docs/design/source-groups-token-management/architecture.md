# 来源共享 分组路由和用量面板架构

2026-10-17 最新决定：用户接受[观测型熔断](subplans/sgm-4-observed-breaker.md)，成功且有 token 活动的有效观测清零同源连续失败计数，包括已锁存时的计数，但不自动解除耗尽。无需修改 NewAPI；下文旧的“最终结果/可信终态”前提由该新细化的去重观测语义替代，不再阻塞独立的 key 管理/三页面开发。

日期：2026-10-15，进度更新：2026-10-18。状态：架构已确认，sgm-1 已完成，sgm-2 的执行/恢复和本地托管预检已实现并分步验证，sgm-3 A/B、C-1 和 C-2 内部管理层已完成验收；sgm-4 A、B1–B3 及 O1–O4 观测持久发布、sgm-5 A原生令牌接口已完成本地验收。尚未接现行服务，运行合流、sgm-5 B/C及sgm-6/7未完成。需求依据：[最新版方案](../../research/source-groups-token-management-research.md)。接口实证：[实现前审核](review.md)。架构确认不包含生产部署授权。

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

版本承接：SGM-1 定义 v1 目录；SGM-2 负责 v1→v2 控制意图、v2→v3 执行记录及打开时的恢复；SGM-3 B 追加 v4 来源/组策略状态；SGM-4 B1 追加 v5 事实、分钟联合用量、账本状态与采集检查点；[O4](subplans/sgm-4-observed-publication.md)追加 v6 待处理观测引用、请求相位、周期历史及来源查询索引；[C2](subplans/sgm-3-source-policies-c2.md)的 v7 扩展原操作表约束，不新增长期表。open 对合法旧应用库先完整校验/备份再逐版本升级，不修补损坏库，也不承诺整链原子回滚。来源/组/成员与各自必需侧表同事务落库；打开库保留未决执行恢复，但不按墙钟清火山额度、重放已有历史错误或自动启动采集。目录快照 wire=1、OperationView wire=2 与数据库 v7 分开。旧迁移审核见[迁移增量核对](../../audits/sgm-1-catalog-store/migration-scope-review.md)，v6 路径见[O4审核](../../audits/sgm-4-facts-analytics/observed-publication-audit.md)，v7 见[C2审核](../../audits/sgm-3-source-policies/c2-audit.md)；不改变 SGM-7 的生产切换授权要求。

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
- 同 operation_id 重放返回同一操作；载荷不同返回冲突。queued/applying/partial/unknown 不等于成功，超时也不等于未执行。C2新增局部设置kind的Succeeded/0 targets仅表示设置已保存；带远端targets的操作仍须完整回读。
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
| reconciler | 带远端targets的持久操作意图、当前 revision | 全部对应渠道回读后才确认；不创建孤立活动副本；不把局部设置回执当渠道确认 | NewAPI CRUD/status/priority 管理请求 |
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

判据作用于 SourceId，不按成员副本独立计数。默认 3 个不同请求的上游不可用观测，同源成功 token 活动清失败计数；已锁存错误判定只由人工撤销或日重置解除。已观测 token 达上限触发另一原因，两者可并存。

错误日志是异步观测，不承诺第三次失败请求返回前已经停用全源。相同 request/source 的错误只计一次；后到的同源成功 token 观测清零计数，重复成功不再次清除后续新失败。先熔断后观测到成功时只清计数，不自动解锁；这是已接受的观测型误判风险。迟到/重放、采集缺口与人工撤销 fencing 仍须故障测试，不能把三条重试日志当三次请求。未就绪/缺口在面板显式呈现。

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
| I6 未确认不显示已执行 | desired、observed、applied_revision 分开；带远端targets的操作只有回读齐全才能成功。C2局部设置kind的Succeeded/0只能显示设置已保存，不能显示渠道已切换 |
| I7 新签发普通 token 无隐式跨组或换模型 | token 指定组、关闭跨组重试、成员类型匹配；原生筛选组/model；本工具不设置 auto 或偷偷映射模型。管理员旧 token 的特殊权限不在此保证内 |
| I8 凭据不经普通快照泄漏 | SecretRef 私有、token 列表打码、明文复制单独操作；文本渲染与日志脱敏 |

目标映射：G1 由 catalog/reconciler 实现；G2 由 source_probe/glm_policy 实现；G3 由 ledger/volcano_policy/reconciler 实现；G4 由 token_service/NewAPI 实现；G5 由 analytics/status/panel 实现；G6 由 store/ControlOperation/流控共同保证。

组合论证：成员唯一性使组写入互不覆盖；共享事实把多组实际消耗收敛到一个来源；来源状态产生全量成员 gate，组策略只在 gate 内选首选。事务先于快照且外部写有回读，使“期望停止”和“实际停止”可区分；各视图从同一去重事实投影，保证相同完整区间下模型分项总和等于总量。

关键假设：只有一个控制进程；本地磁盘持久化可用；受管渠道不被其他自动管理器并发改写；受管火山请求没有绕过 priority 的亲和/指定渠道规则；当前部署日志为 SQLite 管理日志，ID 是稳定记录身份；火山规则 1 上线前已确认 ERROR_LOG_ENABLED=true 且相应错误可记录；时钟与源配置的固定 UTC 偏移正确。任一前提失效要显示未就绪/漂移/数据缺口，不能默认为可用或零消费。

## 阶段门禁

架构已确认；第一子计划目录库已实现并完成 §6 双路径验收，见[细化](subplans/sgm-1-catalog-store.md)和[审核报告](../../audits/sgm-1-catalog-store/audit-report.md)。其余子计划仍须独立经过细化确认、§4.4 审查与 §6 双路径验收。实施顺序与退出条件见[实施准备计划](implementation-plan.md)。

本轮不修改运行服务或导入生产数据。第一批目录函数不接入 up/run/sync 或面板写接口；其本地保存成功不代表外部渠道启停成功。

## 第三子计划第一批共享类型补充

[SGM-3 细化](subplans/sgm-3-source-policies.md)首先实现共享 quota 与 GLM 分组适配。下列类型是进程内接口，不增加公开 JSON 或数据库 schema；第一批不接运行服务。完整字段、不变量和函数契约见该细化 §4–5。

| 类型 | 所有者和消费者 | 关键约束 |
| --- | --- | --- |
| ProbeInput | 受信任来源组装层 → source_probe | SourceId/config_revision 与私有 quota 配置及临时凭据；不序列化 |
| ProbeCycle / QuotaObservation | source_probe → glm_policy，后续安全面板投影 | 一轮同源一次；成功/未知明确区分；源 revision 和轮次匹配后才能用于决策 |
| GlmGroupState | glm_policy，后续 catalog 持久化 | active/pinned 按 MembershipId，regime 复用旧定义；不与其他组共享选择状态 |
| GlmGroupDecision | glm_policy → 后续策略发布层 | 带组 revision/轮次；成员 priority 的 None 为保持，不是 0；不代表远端已完成 |
| GlmPinRelease | glm_policy → 后续面板投影 | membership_id、pct、limit；原整数标签转换为组内身份，不当作原生 channel_id |

此批复用原 GLM 选择及 priority 分支，既有状态库、协调器和本地托管架构不变。火山运行状态和事实边界在 SGM-3 后续批次细化，真实采集仍由 SGM-4 唯一拥有，不创建第二份去重账本。

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
| WorkTicket / WorkSnapshot | 目录实例、成员动作与 member/source/group generation、执行序号及启用授权 epoch，一致读取的目标及完整可能路由的同组状态（含退役中成员） | store/worker/planner；目录持久票据及快照供已实现的显式 worker 消费，尚未接现行 CLI；不包含公开可序列化凭据 |
| ExecutionEffect / ExecutionOutcome | 私有白名单写效果；明确未发出、完整响应（可失败）、不确定三种结算证据 | catalog/execution；仅可信执行器生产结算证据，不接受前端 JSON；完整响应不等于目标完成 |
| BackendReadiness | 已核实的管理能力、原生自动启用开关、实例绑定及检查时间 | 只读预检 → 协调器；冲突不静默修改现有全局配置 |
| BackendCheck / BackendIssue | 不可序列化的检查结果与固定安全失败原因；不含进程路径、环境或服务端正文 | 本地 preflight → worker/后续运行组装层；复用既有 NewApiProcess，不引入远程登记或数据库指纹 |
| ChannelObservation | 实际 channel 身份、路由字段、原生状态及观测时间 | NewAPI 客户端 → reconciler；原始 JSON 私有，未知不当零 |
| OperationView | 操作身份/类型/阶段、目标 generation、确认计数、安全错误码及时间 | store 发布给面板；具体格式由 [schema](schemas/control-operation.schema.json) 约束 |

创建关联名使用本工具实例 ID 与 MembershipId，不用 key 或可改展示名。状态管理区分 GLM 正常 preserve 与火山显式 1/2；人工暂停/退役覆盖两者。数据库 v2 通过私有侧表保存控制意图，v3 追加执行记录；冻结旧 DDL 并备份后逐版本迁移，核心 CatalogSnapshot wire 版本仍为 1。操作历史保存匹配 generation 的确认收据，不能因后来变更丢失旧成功证据。

NewAPI 无远端 CAS：本地 revision 防止错记旧 ACK，不等于可以撤销已经发出的写。目标完成以匹配回读为依据；未决相反写与漂移必须显式显示，并在既定单写者假设下持续对账。不能把此执行层宣称为硬实时或严格 exactly-once 控制器。

sgm-2 源码核对发现原生自动启用也可能成为并发状态写者。因此受管状态模式上线前必须确认其关闭；若现有实例开启，不自动修改、不中断旧部署，须在发布阶段另行确认处理方式。GLM 的原 auto_ban 和额度选择规则不变。此为细化新增的部署前提，不是已核实当前生产配置满足条件的声明。

## 第三子计划第二批共享类型

以下对应已验收的 [SGM-3 B 细化](subplans/sgm-3-source-policies-b.md)，代码数据库为 v4。不改变模块归属或公开 JSON；保存来源运行状态和每组选择，原始事实/历史统计仍由统一账本拥有。

| 类型 | 所有者和消费者 | 约束 |
| --- | --- | --- |
| VolcanoRuntime / VolcanoState | source_policy 计算，catalog 持久化，C/面板消费 | 来源唯一；暂停优先、两个原因独立；状态派生；未初始化不默认可用 |
| VolcanoEvidence / OutcomeObservation | SGM-4 生产，source_policy 消费 | 绝对 token、去重观测转移、周期/错误代次/投影游标；不能直接从重试日志条数或前端布尔值构造；同源正 token 活动清失败计数 |
| EvidenceDisposition | 策略 → 内部存储入口 | Applied/Duplicate/Stale/Gap；缺口要求重建，不部分累加 |
| SourcePolicyUpdate / SourcePolicyWrite | 内部控制/账本 → catalog → C | Tick/Evidence/ClearError/ReplacePolicy；返回安全来源/运行状态及过渡，不是前端命令或远端回执 |
| VolcanoGroupState / GroupPolicyState | 策略和 catalog，C 消费 | 火山 preferred/稳定游标；GLM 复用既有 GlmGroupState；不按访问 key 建绑定 |
| StoredGroupPolicy / VolcanoGroupDecision | catalog / 策略 → C | 组选择版本与输入来源版本明确；只表示本地选择，priority None 为 Keep |
| PeriodTransition | 周期函数 → C/SGM-4 | 旧边界与当前新边界/旧规则；历史落库和运行状态须在后续组合事务中一起提交 |

B 只实现有界纯函数和内部事务。原始日志去重、终态识别与缺口、历史边界账本、操作幂等回执和渠道许可的组合保证仍须 C/SGM-4 完成，不用 B 的 fixture 或本地保存成功代替上线验收。

## 第三子计划 C-1 共享类型与实施边界

对应已确认的 [C 接线方案](subplans/sgm-3-source-policies-c.md)。[catalog 事务适配](../../audits/sgm-3-source-policies/c1-tx-audit.md)、[批次发布/成员许可](../../audits/sgm-3-source-policies/c1-publication-audit.md)、[共享采集及 C-1 组合验收](../../audits/sgm-3-source-policies/c1-acquisition-audit.md)均完成。保留现有模块边界与 SQLite v4；C-1 接入协调器显式刷新，不改变火山状态/目标或授予火山许可，也不接运行 CLI。

| 类型/责任 | 共享方 | 约束与状态 |
| --- | --- | --- |
| GlmPolicyCommit | catalog 事务 → worker | 组状态/必要目标同事务，提交前构造完整许可 map，提交后一次替换，不逐成员发布 |
| PolicyRefreshReport | catalog 提交前构造 → worker → 后续组装层 | 区分 preview_only 和本地提交；不包含私有材料或远端成功声明 |
| RoutingPermit 的成员绑定 | policy → planner/worker | 在原 source/group/priority/time 外补 membership_id/member_revision，防止旧目标许可复用 |
| 配置/采集器所有权 | worker | 原 Config 生命周期只读、dry_run 双写门禁、SourceProbe 单飞和单 commands 槽；Core/目录事务不跨网络 await，取消不发布半轮 |

C-1 不对未 managed 的目录成员写自动目标，不通过周期操作增加接管授权；稳定 no-op 不新建操作/提升版本，资格恢复只唤醒已有 NotReady 待办。内部管理动作由 C-2 承接，火山观测事实与发布由 O4 承接；HTTP/运行组装及生产接管/部署仍属后续范围。

## 第四子计划 B1 共享接口与版本边界

[B1 存储细化](subplans/sgm-4-facts-analytics-b.md)把 A 规范事实持久化到同一个 catalog，不增加数据库服务或写者。以下均为进程内接口，不新增前端 JSON；当前验证见[B1 审核](../../audits/sgm-4-facts-analytics/batch-b1-audit.md)。

| 类型/责任 | 所有者和消费者 | 约束 |
| --- | --- | --- |
| LedgerState / ScanProgress | catalog/ledger → 后续统一采集器、查询投影 | 未初始化/Scanning/Scanned/Stalled 明确；Scanned 只是本次遍历结束，不是完整遥测或路由就绪 |
| ScanTicket | catalog 生成，后续采集器带回 | 绑定本地名称空间、lane、generation、window、page；不反序列化；不证明原生日志库未被替换 |
| LedgerCommit | catalog 事务 → 后续采集器 | 只报告本地插入/重复数、统计 revision 和进度；事实/投影/检查点成组提交 |
| LedgerError | catalog/ledger → 内部调用方 | 固定错误；输入非法与持久损坏分开；未知提交沿既有 poisoned 停写 |

事实以单个 catalog 名称空间内的 native_log_id 去重，已知 channel 身份永久保留，未知归属可等待后续显式修正。原生库不一致替换/回滚仍受 SGM-7 恢复前提约束，不能把 catalog UUID 当作自动识别原生数据库的证明。B1 不生成 FinalOutcome/VolcanoEvidence 或渠道许可；B2 采集、B3 查询/补归属及 C 的可信终态仍有各自门禁。

## 第四子计划 B2 统一采集共享接口

[B2 细化](subplans/sgm-4-facts-analytics-b2.md)保持模块划分与 v5。以下进程内接口已通过能力探针、分步实现及本地验收，不改变公开 wire 或现行服务，尚未由运行入口启动。证据见 [B2 审核](../../audits/sgm-4-facts-analytics/batch-b2-audit.md)。

| 类型/责任 | 所有者和消费者 | 约束 |
| --- | --- | --- |
| LogPageQuery / CheckedLogPage | NewAPI 日志入口 → 统一采集器 | 固定全局管理员查询，≤100 原始行，页键全验；无公共 serde，不是完整性证据 |
| LocalLogLease / LogCapabilityIssue | 本地日志预检 → 采集器 | 真实托管绑定、固定版本及 SQLite 启动分支；独立于路由 BackendReadiness，不降低自动恢复门禁 |
| CollectionReport / CollectionError | worker 单飞采集 → 后续运行组装/状态投影 | 当轮已提交计数、两个检查点及固定诊断；不产生来源策略许可 |
| restart_scan | catalog → 采集器 | 当前完整票据匹配后同窗新代次第一页，事实/投影不变；不改变 begin_scan 重放 |

近期与历史复用现有两个 checkpoint；历史从保存起点全窗重扫，不增加任务表。SQL_DSN/LOG_SQL_DSN 显式 local 的已确认启动前提仅用于核验日志 SQLite 分支，不是数据集身份凭证。若当前服务缺少可验证条件，B2 只报告；设置启动环境及任何重启都留给获得授权后的 SGM-7。

## 第四子计划 B3 查询与补归属方案

[B3 细化](subplans/sgm-4-facts-analytics-b3.md)的内存计算、只读数据库组合和补归属均已完成[本地验收](../../audits/sgm-4-facts-analytics/batch-b3-completion-audit.md)。继续复用同一应用库的 v5 事实与分钟联合投影；B3 本身不增加表、写者、采集器或公开 HTTP 接口。字段与值域见 B3 §3–4，事前验收见 [B3 expectations](../../audits/sgm-4-facts-analytics/batch-b3-expectations.md)。

| 共享类型/责任 | 所有者和消费者 | 约束 |
| --- | --- | --- |
| UsageQuery / UsageFilter / UsageGrain | ledger/analytics 定义，catalog/worker 消费 | 显式半开区间/固定偏移、Hour/Day、source/group/token/model 的 Any/Unknown/Exact；不按名称认领身份 |
| UsageSnapshot / UsageReport / UsageBucket | analytics/catalog → worker → 后续状态投影 | 同一 LedgerState/revision、总计/模型/token/桶模型分项；未初始化 usage=None，全部是已观测口径，不公开原始事实 |
| AnalyticsError | 查询/补正入口 → 内部调用方 | Ledger/Control 及固定输入、容量、溢出、取消、超时、忙错误；无标签/路径/正文 |
| AttributionCursor / Batch / Report | catalog → worker 私有补正进度 | 冻结事实 ID 上界，每批≤256，只有未知归属可细化；原生字段/总体 token/检查点不变，结束不代表已无未知 |
| 只读连接与目录锁 | catalog/worker | 同应用库 WAL 读事务，不取长 Core；Store/读任务共享原持锁 guard，最后连接退出后显式解锁；只读不是另一个写者 |

查询一次返回完整有界结果，不以截断 Top-N 或跨请求可变分页冒充同快照总计。补归属经单写者原子撤旧/加新/改事实，内存游标丢失可重扫，无新增任务表。查询和补正都为显式内部入口，运行调度与面板接线仍属后续小步。

## 第四子计划 O4 观测发布共享接口

[O4 细化](subplans/sgm-4-observed-publication.md)承接 O1–O3 和 B1–B3，保持一个事实账本和原 VolcanoRuntime。v6 的三个侧表只保存待处理引用、请求去重相位和周期过渡；source/type8 适配由目录核验，GLM 不进入该观测熔断。

| 类型/责任 | 所有者和消费者 | 约束 |
| --- | --- | --- |
| VolcanoPolicyCommit | catalog 单事务 → worker | 队列/相位/Runtime/历史/全部组目标同事务，返回本类完整 membership/source scope、最终版本 permits/disabled；不序列化 |
| VolcanoRefreshReport | catalog → worker | 仅来源/消费/等待/变更计数及 preview_only，不报告远端已收敛，不含原始请求或凭据 |
| GlmPolicyCommit.scope | catalog → worker | 新增本类全部历史/当前成员；只撤换本类许可，不延长另一类 lease，原 GLM 选择/授权规则不变 |
| refresh_volcano_policy | worker 显式内部入口 | 共享命令槽/发布 fence；dry_run 只读；提交失败保留缓存，未知提交闭锁；不新增周期任务 |

来源处理每轮16条队列，使用原 source/request 单向归并与 period/cutoff 隔离。当前周期绝对 token 从原分钟投影与精确秒边缘读取，最大1M行/10秒。近期消费页成功、队列已处理、预检和原成员资格共同限制正向许可；它们不证明上游日志绝对完整。按原组选择器独立首选，跨组共享来源耗尽。实现验收见[O4报告](../../audits/sgm-4-facts-analytics/observed-publication-audit.md)，管理命令按下节 C2 承接；运行 CLI、页面和生产部署仍是后续范围。

## 第三子计划 C2 管理接口

[C2细化](subplans/sgm-3-source-policies-c2.md)扩展原 ControlCommand，不增加 HTTP 入口或后台调度。新局部操作使用原 OperationView 的 Succeeded/0 表示设置保存，不能作为渠道已执行的证据。

| 命令 | 作用域及约束 |
| --- | --- |
| SetGroupPin | GLM 组内固定/取消固定；校验组/策略版本及成员资格，不改变原95%释放规则 |
| ClearVolcanoError | 同一火山来源清当前错误判定；先推进日格，不清当前周期 token 上限或人工暂停 |
| UpdateVolcanoPolicy | 同源上限/重置设置；未初始化时只保存设置，不伪造零用量 |
| DiscoverSourceModels | 按来源显式有界 GET，成功沿原模型目标/执行回读链同步所有组，失败保持存量模型 |

局部命令、持久版本和周期历史同事务；新来源/组版本使旧许可失效，后续真实 C1/O4 刷新才能发布新许可。发现等待不持 Core，返回时重核来源版本；成功重放不再次 GET，取消不留半操作。私有发现材料不实现通用 Debug/Serialize。验收边界见[C2审核](../../audits/sgm-3-source-policies/c2-audit.md)；key 管理及三个页面不由这些内部入口替代。

## 第五子计划 A 原生令牌接口

[SGM5细化](subplans/sgm-5-token-management.md)先提供有限原生接口，再由B承接持久操作、C承接组及身份就绪。两种管理身份仍在后端托管，使用者只拿访问key，不需注册面板账号；它们必须属于同一已核对的原生实例并共享原请求预算。

| 类型或接口 | 所有者与消费者 | 约束 |
| --- | --- | --- |
| TokenOwner/TokenOwnerScope | token_service → native接口 | 固定user_id及issuer/legacy_admin，不依据返回数据切换会话；新建只允许普通role1 |
| NativeTokenView | native接口 → B/C/未来页面 | wire1、字符串原生ID、18个安全字段；列表本地重打码、空组显式继承账号组、原生quota不是token用量；[schema](schemas/native-token.schema.json)约束 |
| CreationLookup | native接口 → B | Missing/One/Ambiguous；唯一项逐字核关联名及固定设置，历史过期不妨碍查回；不自行认领或重发创建 |
| RevealedToken | native接口 → 未来显式复制入口 | 私有SecretString、无Debug/Serialize，统一单个sk-前缀；不入普通快照、日志或应用库 |
| ManagedTokens | 原client/transport → token_service | owner/role、响应ID与有限页严格校验；请求固定expected_user_id及header，重登不能改认账号；相同预算和最终发送guard，写前失败NotSent、写后不确定维持Uncertain |

原生token创建忽略禁用status且不返回ID，因此不能借用渠道的禁用创建承诺；B必须先持久化稳定关联并查回确认，之后才能开放复制。新增接口不写组配置/账号余额，不创建后台任务或持久表，也未挂HTTP。仅专用token单条GET可将固定原生缺失响应解释为None，其他管理响应语义保持不变。A的ACK不代表SGM5整体完成，验收范围见[A记录](../../audits/sgm-5-token-management/a-audit.md)。
