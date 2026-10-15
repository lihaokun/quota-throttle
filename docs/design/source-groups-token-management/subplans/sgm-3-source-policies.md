# SGM-3 来源采集与分组策略细化

状态：用户确认的第一批 A 已完成实现与双路径验证，见[审核报告](../../../audits/sgm-3-source-policies/audit-report.md)。基线 `e1a9279`。本子计划实现[架构](../architecture.md)的 G2、G3；整体功能仍包括后续事实采集、访问 key、三页面和发布接线，不以本批替代完整交付。

## 1 范围和实施顺序

沿用本地托管 NewAPI 和现有协调器，不增加部署类型、远程登记、推理代理或另一套用量账本。GLM 与火山分别决策，只有来源身份、组成员和渠道执行入口共享。

分三批实施，每批完成测试和独立审核再进入下一批：

| 批次 | 实现内容 | 退出条件 |
| --- | --- | --- |
| A 已完成 | 来源级 GLM quota 采集、组输入映射、原 GLM 决策及 priority 结果适配 | 同源一轮只查一次；完整决策链等价；基本、边界、PBT 和独审通过 |
| B 下一次细化 | 火山纯状态机、周期计算、稳定轮转，以及来源运行状态和组选择的持久化 | 双判据独立；重置/撤错防旧事件；重启恢复；存储迁移和组合测试通过 |
| C 后续接线 | 策略向协调器原子发布、来源模型同步、与 SGM-4 事实接口合流 | 暂停与旧结果竞态、多组共享、真实错误归并和用量口径通过 |

本次已确认 A 的实现。B/C 的函数级契约和 expectations 在各批编码前补齐；尤其不能把“收到三条错误日志”当成已经设计好的规则 1。CLI、面板和生产部署仍不在 A 范围。

### 已确定且不重新讨论的业务规则

- GLM 保留 95% 预防、全员越线时的降级、90% restore、粘滞、周临期及手动固定。阈值继续读现有配置，不硬编码默认值。
- 每组一个首选；不是每个访问 key 单独绑定上游。各组 priority 独立，同源额度与异常共享。
- 火山没有临期、积分换算或硬 token 预留。首选可用则保持；不可用才按稳定成员顺序轮转，恢复来源不抢占现首选。
- 火山手动暂停优先；错误耗尽与 token 达限可同时成立。默认三次独立请求失败，人工撤销只清错误原因和连续失败计数。
- 默认每日 01:00、UTC+08:00 重置，可编辑。修改重置规则先处理旧规则已经到期的边界，再取严格未来的新边界，保留本周期已有用量。
- 下游 OpenAI/Claude 共用同一访问 key 和来源渠道，不按协议拆组。

## 2 与已有代码的复用点

| 复用项 | 本批处理 |
| --- | --- |
| `catalog::SourceId/GroupId/MembershipId/SourceSettings` | 使用原类型与限制，不重定义身份或来源配置 |
| `QuotaProbe::new/query` | 保留 quota URL、Bearer、两层 selector header 构造顺序、10 秒请求时限及 TOKENS_LIMIT/CREDIT_LIMIT 解析；不改鉴权或套餐语义 |
| `orchestrator::max_watch_pct/WeeklyInfo/decide` | 只开放必要的 crate 内可见性，不重写选择算法 |
| `orchestrator::tick` 的 priority 分支 | 提取为共享纯函数；旧 tick 和新适配器调用同一函数，分支顺序不变 |
| `Config` | 复用 watch_windows、阈值、lookahead 和三档 priority；不引入第二份调度配置 |
| `Membership.order` | 按 `(order, membership_id)` 形成稳定输入顺序；导入保留旧顺序 |
| SGM-2 协调器 | 本批只产出决策，不调用管理 HTTP、不写 status、不自动创建启用授权 |
| `WorkSnapshot` | 复用一致快照内的来源、组、成员及退役意图；不能仅用 MembershipView 推断尚未完成的退役 |

已新增 `src/source_policy/{mod,probe,glm}.rs` 和对应测试；`main.rs` 仅声明模块。`orchestrator.rs` 仅增加必要可见性和提取纯 priority 函数。不修改现行 up/run 的任务装配或旧控制循环的调度频率。

## 3 错误处理策略

输入身份重复、类型不匹配、组成员重复、容量越界或计数溢出返回固定类别的 `PolicyError`，不发布部分决策。探针单源请求失败不是整个采集轮失败：记录该来源 `Unavailable`，继续查询其他来源。

过期的来源配置 revision、其他轮次或缺失观测统一视为本轮未知，不复用历史成功值参与决策。历史成功值可以供后续面板显示“上次成功”，但不能冒充本轮 quota。

原始 HTTP 错误、响应正文、URL 查询参数、header 和 credential 不进入安全结果或 tracing。暂停/退休来源不会由采集器恢复；采集器也不将查询失败转成火山失败次数。

存储、远端确认、重试和操作回执由已有 catalog/reconciler 处理，本批没有第二套副作用错误模型。

## 4 数据结构定义

以下都是进程内类型；A 不增加公开 JSON、数据库表或配置项。共享类型的所有者和消费者同步见[架构补充](../architecture.md#第三子计划第一批共享类型补充)。

### ProbeInput 与 ProbeCycle

- `ProbeInput`：`source_id`、`config_revision`、GLM quota profile 和临时 `SecretString`。仅受信任的后端组装；不实现 Serialize，Debug 脱敏。退休、手动暂停和非 GLM 来源不进入本轮任务。
- `ProbeCycle`：单调递增 `cycle_id: u64`、完成时间及按 SourceId 排序的 `QuotaObservation`；一轮结束才形成完整结果，未查询成功的来源也有显式失败记录。
- `QuotaObservation`：`source_id`、`config_revision`、`cycle_id`、观测时间、`Available(QuotaStatus) | Unavailable`。失败不构造默认 QuotaStatus。
- 不变量：一轮每个 SourceId 至多一项；结果不含凭据；结果数量等于经过完整校验的任务数量；同源加入更多组不增加任务。
- 生命周期：采集任务生成一轮，组适配器只读复用。没有本批持久缓存或跨轮补造成功值。

### GlmGroupState 与 GlmGroupDecision

- `GlmGroupState`：`active: Option<MembershipId>`、`pinned: Option<MembershipId>`、`regime: Regime`。当前批次作为纯函数输入/输出，持久化归 B。
- `GlmGroupDecision`：`group_id`、输入 `group_revision`、`cycle_id`、新的选择状态、eligible/imminent 成员集合、pin_release，以及每个有效成员的 `priority: Option<i64>`。pin_release 使用 `GlmPinRelease { membership_id, pct, limit }`，将旧内部 PinRelease 的整数标签还原为 MembershipId，不把它误报为原生 channel_id。
- `None` priority 明确表示“本轮不改变该成员 priority”，不是 0、不是禁用。不能直接把稀疏结果作为 SGM-2 的全量 SetGroupPriorities 请求。
- 身份与不变量：成员必须属于该组；不含暂停、退休、退役中或没有 channel_id 的成员；created_disabled 但已经绑定 channel_id 的成员可进入策略输入，不能因原生 status=2 永久无法启用。原生 status=3 不作为 GLM 算法的新增过滤条件。
- 生命周期：调用方提供当前组状态，函数返回下一状态；不写存储或渠道。人工暂停/移除后的 active/pinned 先清理，再调用旧算法；正常未知或耗尽不能据此清空仍有效的 active。

`PolicyError` 仅包含 `InvalidInput`、`Capacity`、`SequenceExhausted`。探针的正常查询失败用 `Unavailable` 表达，不复制供应商错误字符串。

## 5 第一批函数细化

### 5.0 构造与只读辅助

`ProbeInput::new(source: &SourceView, material: SourceMaterial)` 由受信任目录组装层调用；前置为同一来源的合法私有 material。先检查 source 为 GLM、未暂停/归档、revision 在 1..=JSON_SAFE，再匹配 material 的 GLM 分支；任一失败返回 InvalidInput，不发请求，否则转移 quota/credential 所有权构造输入。来源身份来自 source，不从 key 推断。正确性由校验先于构造、无外部副作用保证；函数不取代目录的完整配置校验。

`now_ms()` 只读系统时钟并检查 UNIX epoch 与 i64 毫秒可表示性；失败返回 InvalidInput。collect_cycle 在编号提交前、每次完成查询后、整轮返回前调用；若时钟无效则不发布半轮结果，已经消费的轮次编号不回滚。

ProbeCycle 的编号/时间/观测 getters 只借用已有值，Default 只建立空计数或空组状态，Error Display 仅输出固定枚举；均为 trivial，无存储或网络副作用。安全 Debug 不打印 ProbeInput 的 profile/credential。

### 5.1 collect_cycle

接口：`SourceProbe::collect_cycle(&mut self, inputs: Vec<ProbeInput>) -> Result<ProbeCycle, PolicyError>`。

调用方是后续来源采集任务，本批测试直接提供假来源。callee 为既有 `QuotaProbe::new/query`：new 接收 quota profile 对应的 ZhipuConfig；query 接收裸 key 与 key_headers，成功返回已解析窗口，失败返回错误。

前置：调用方已按目录截取来源配置；输入不得超过 MAX_SOURCES。函数仍防御性检查身份、revision、重复 SourceId 和容量，全部通过后才发出请求。

步骤：

1. `P1` 完整校验并按 SourceId 排序；空输入返回空轮次。cycle_id 使用 checked_add，溢出返回 SequenceExhausted，不复用旧编号，不发请求。
2. `P2` 为每个唯一来源构造原 QuotaProbe，按现有顺序串行 query；一轮不重试，不按成员创建查询。成功记 Available，失败记 Unavailable，继续下一源。
3. `P3` 收集全部结果并返回同一个 cycle_id；调用取消时不返回部分 ProbeCycle。下一轮用新编号，不能把旧结果混入新轮。

后置：每个输入来源恰好发起一次 quota 查询，产出一项安全结果；无管理 API、模型目录或推理请求，无持久写。

终止和并发：输入最多 MAX_SOURCES，循环每次消费一项；query 总时限沿用 10 秒。本批保留串行语义，不以查询并发改造作为共享采集的前置条件。`&mut self` 保证同一个采集器不重入；等待网络不持有 catalog/worker 锁。

正确性论证：全量校验早于网络，排除了重复来源和非法任务的部分副作用；每个循环分支都插入一项结果，唯一键保证多组不复制采集；只在循环结束返回，使一个组决策不会混合两个轮次。副作用穷尽为输入列表中的只读 quota 请求。

### 5.2 decide_glm_group

接口：`decide_glm_group(cfg: &Config, group: &GroupView, members: &[WorkSnapshot], cycle: &ProbeCycle, previous: &GlmGroupState, now_ms: i64) -> Result<GlmGroupDecision, PolicyError>`。

callee：原 `max_watch_pct` 按配置监测窗口取值；原 `decide` 接收稳定 ID 序列、成功 quota、weekly、当前/pin 和原阈值，返回选择、合格集、临期集及 pin_release；5.3 返回单成员 priority。这些函数均无副作用。

前置：配置已通过现有 Config 校验；now_ms 非负且 lookahead 换算可表示。输入是同一目录快照。本函数校验 group 类型、重复身份、所属组及 revision、source 引用/类型及容量；非法输入返回 InvalidInput/Capacity，不能悄悄跨组使用成员。WorkSnapshot 的 target 必须对应 member，不能把其他成员的 retire_requested 配到本成员。

步骤：

1. `P4` 从 WorkSnapshot 读取来源/成员及目标；先按目录唯一性规则检查 lifecycle != retired 的 source/group 组合唯一（暂停、未绑定、退役中也不能重复），再过滤 manual_paused、source_retire_requested、source.retired_at、target.retire_requested、已退役成员与未绑定成员，按 `(order, membership_id)` 排序。源级暂停意图高于 pin，退役中不能等待 lifecycle 变 retired 才排除。
2. 清理不在有效集合内的 previous.active/pinned；其余保持。由 MembershipId 映射原函数的整数 ID，不拿 SourceId 代替组内身份。
3. `P5` 每成员引用同源观测：仅 cycle_id 和 source.config_revision 匹配的 Available 进入 pct/weekly；失败、缺失和旧配置结果都不进入 pct。使用原 watch_windows/max_watch_pct，不新增配额估算规则。若 weekly.reset_ms 与 now_ms 的差无法用 i64 表示，返回 InvalidInput，避免向原算法传入会溢出的外部时间；不修改合法时间的临期规则。
4. `P6` 调原 decide，参数阈值、restore、exhausted 和 lookahead 均来自 cfg。pin_release 非空才按旧规则清 pin；`decision.active=Some` 更新 active，`None` 保留经过步骤 2 清理的旧 active。更新 regime，不按原生 status 增加第二套选择。
5. `P7` 按 5.3 为所有有效成员生成 priority 或显式 Keep（None）；原样保留 eligible/imminent 和 pin_release 归属，形成结果。空组合法，返回空集合和无 active/pinned。

后置：无新增暂停/移除且 quota 输入相同时，结果经身份映射与旧 tick 的完整决策链等价，包括“全部耗尽仍保留旧活动”的调用方行为。仅本组成员可能出现在输出，不发送或确认任何渠道变化。

终止：索引/过滤/映射都是有界来源和成员遍历，排序有限；原 decide 在有限成员集内选择。无轮询、重试或网络。

正确性论证：步骤 1 保证业务范围合法；步骤 3 仅把已知值传入旧函数，因此未知不成为 0；步骤 4 既复用旧决策又保留其调用方 None 语义，避免只比较 decide 而漏掉 tick 的退化；步骤 5 复用相同分支，确保 active、eligible、已知耗尽和未知依序处理。所有输出是新值，输入和外部状态均不变。

### 5.3 glm_target_priority

接口：`glm_target_priority(id: i64, active: Option<i64>, eligible: bool, quota_known: bool, cfg: &Config) -> Option<i64>`。

调用方：旧 tick 和 5.2。前置：直接使用 Config 中的三个 i64 priority，不在纯函数内新增配置校验或改变数值。按旧分支顺序返回：

1. id 等于 active → active priority，**即使本轮该源查询失败**。
2. 否则 eligible → standby priority。
3. 否则 quota_known → exhausted priority。
4. 否则 → None，不改 priority。

后置：与原 tick 分支逐项相同。旧 tick 的 applied 去重、dry_run、HTTP 成功/失败处理和快照发布位置不变。

正确性论证：四分支有序且覆盖所有布尔输入；提前命中 active 是旧行为，不可改成“所有未知都跳过”。纯函数无其他副作用。测试使用冻结的原分支参考，而不是拿新函数与自身比较。

## 6 后续两批必须落实的契约边界

以下约束承接已经确认的架构，不将尚未细化的函数伪装为本批可编码接口。

### 火山状态与持久化

B 定义纯 `advance_period / clear_error / apply_usage / apply_outcome / select_preferred` 和相应 catalog 事务。数据库只能追加新迁移，不能修改 v1/v2/v3 DDL；运行状态以 SourceId 唯一，组首选以 GroupId 唯一。保存成功先于发布，暂停、模型配置和旧许可的版本隔离继续由单写者维护。

周期使用半开区间；错过多日按算术定位当前边界，不逐日循环。时钟倒退不倒退 period_id，不重复清零。改上限立即重评规则 2，改重置时刻不凭空清空，重复命令按 operation_id 幂等。精确 SQL、命令载荷和 JSON 变更在 B 编码前补齐，而非本批先造空表。

### 与 SGM-4 的事实边界

SGM-4 唯一拥有原始日志去重、请求结果归并、采集覆盖和迟到处理；SGM-3 不查询日志、不维护另一个 request 去重表。

- 消费输入是指定来源/周期的已去重 input/output 累计值及事实版本；重复版本不能重复加算。缓存包含于 input 时不再额外相加；不以 quota 或积分代替 token。
- 失败输入必须是同 `(instance, request_id, source_id)` 的归并结果，而不是单条 error log。同源最终成功清未锁存的连续次数，其他源成功不清本源次数，已锁存原因不会因迟到成功自动撤销。
- 只有归因上游的不可用进入计数。下游鉴权、参数/上下文、取消、管理 429、组配置和不能归因的错误不直接耗尽来源。
- 人工撤错持久保存 error_epoch 与账本边界；归并层必须区分撤销前事件，禁止把迟到/重放旧事件贴上新 epoch。周期亦然，不能用采集到达时间代替事件归属。
- 终态证据、事件排序、时间精度、窗口关闭与边界不足的处理须在 B/C 和 SGM-4 对照固定 NewAPI 元数据验证。不能仅设置一个等待秒数，就宣称所有流式请求已终止；证据不足必须标记遥测未就绪，不增加推理代理补证据。

### 发布到协调器

C 在同一受控更新中保存新状态、源级 gate、完整组目标及新许可。先验证 revision，提交后发布；失败不得留下新选择配旧意图。不得逐成员 publish_route 期间短暂使用旧的同源启用许可。

组 priority 的 Keep 与 SGM-2 全量列表之间需要显式适配，保留当前目标而不是填 0；相同计划不重复生成操作或提升 revision。GLM 的普通 priority 变化继续 preserve 原生状态，不能恢复原生自动禁用；新成员/显式恢复则走 SGM-2 一次性授权。火山恢复先备用再启用，首选提升沿用协调器降旧/回读屏障。

模型发现按来源、按添加/显式同步触发；不进入 quota 周期、不按成员重复请求。成功用 `/models` 结果同步所有成员，失败保留已有非空列表。

## 7 第一批测试和审核

事前清单见 [expectations](../../../audits/sgm-3-source-policies/expectations.md)。基本测试覆盖分组、双计费窗口、selector 传递、单源失败；边界测试覆盖阈值、空组、未知、全部耗尽、暂停固定源、未绑定和原生禁用。

PBT 每项至少 128 cases，分别验证完整 GLM 链路等价、组隔离、同源共享采集、未知/旧 revision 隔离。等价基准冻结 `e1a9279` 的规则和关键 golden 输出；不能仅证明新旧调用同一个新函数。定向变异至少覆盖 `<` 改 `<=`、None 清 active、未知填 0、每成员重复查询、跨组混入。

只用假 key、随机回环端口和内存输入。A 没有数据库迁移，不启动真实 NewAPI，不请求生产端口或供应商。机械门禁限定本批文件，旧 SGM-1/2 的基线脚本不放宽；代码完成后运行本批与全量离线测试，再按 workflow §6.5 独立审核。

## 8 完整性自检和确认点

- [x] A 的类型、调用关系、前后条件和副作用已列出。
- [x] A 的成功/失败/未知/空输入/溢出/取消分支已列出。
- [x] A 的循环边界、网络时限和无目录锁等待已列出。
- [x] A 复用原 GLM 算法及 caller 行为，不引入火山规则。
- [x] A 的跨模块类型与事前十节 expectations 同步。
- [x] 用户确认 A 细化后开始编码。
- [ ] B/C 另补函数级细化、事实归并证据和各批 expectations 后才能编码；这不是待用户重新决定的业务规则。

第一批只完成共享 quota 与 GLM 策略适配，不宣称火山状态机、统一刷新页面或整套功能已经完成。
