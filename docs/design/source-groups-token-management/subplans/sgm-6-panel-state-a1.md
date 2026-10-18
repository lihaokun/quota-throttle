# SGM6 A1 安全面板状态投影

2026-10-19，沿[三页面计划](sgm-6-three-page-panel.md)先实现只读状态生产。编码前独立设计复审通过，基线`999ca41`。fermat是全新部署目标，允许隔离测试，不导入旧配置或数据；本批不接HTTP、run或在线服务。

## 1 范围和复用

G5由目录/控制版本、共享来源状态及已提交GLM观察共同组成；G6由同一只读事务、原Core互斥、白名单和未知语义保证。复用DB v8、原CatalogSnapshot子视图、source_record/read_runtime/read_group_policy/member_target及原GLM发布路径，不另建数据库、计时器、探针、策略或许可。执行尝试历史不进常规快照；具体操作的Unknown/Succeeded沿A2按需OperationView读取，不能用dirty或applied字段代替操作结果。

产出供后续单一状态发布者调用的`ReconcileWorker::panel_snapshot()`；未来HTTP读取已发布结果，不让每个浏览器请求跑一次SQL/probe。查询不运行协调器，不注册组，不推进火山周期，不查NewAPI，不触发用量采集。完整模型列表、token原生列表、账本图表和所有写接口留A2/后续，不在常规面板快照里塞入全部模型或私有配置。

文件落点为`catalog/panel.rs`的目录读投影、`worker/panel.rs`的已提交观察组合；原GLM commit仅追加安全展示结果，worker成功发布时更新私有缓存。旧算法、操作收据、DDL、依赖和原HTTP不改。

## 2 数据结构

公开JSON只支持Serialize，不接受前端反序列化为内部许可。所有实体ID沿原十进制字符串；revision/时间/计数沿原JSON安全整数。新增快照wire=1，既有wire不变。

`PanelCatalog`含sources/groups/memberships三组按稳定ID排序的完整有界数组，分别最多1000/100/10000项；包括历史退休身份。

- `PanelSource`：`source: SourceView`、`retire_requested: bool`、`model_count: u32`、`volcano: Option<VolcanoPanel>`。不含endpoint、headers、selector、凭据、指纹或SecretRef。完整模型显式查询另做，不扩大每次刷新负载。
- `VolcanoPanel`精确字段为`policy: VolcanoPolicy`、`runtime: Option<VolcanoRuntimeView>`。runtime精确字段为`revision`、`period_id`、`period_start`、`next_reset_at`、`used_input`、`used_output`、`consecutive_failures`、`error_exhausted`、`limit_exhausted`、`coverage_end`；仅coverage_end可null，三项时间为epoch秒，其余整数/布尔沿原Runtime值域。未初始化runtime整体为null，保留两种原因，不填零。周期是否已过由外层快照时间显式比较，不在读取时推进。
- `PanelGroup`：`group: GroupView`、`policy_revision`、`selected_member_id`、`pinned_member_id`、`regime`。selected分别来自原GLM active/火山preferred；火山pinned/regime为null，GLM regime沿原`Regime::as_str`。这些是保存的策略选择，不是当前可路由证明。
- `PanelMembership`：`membership: MembershipView`、`control_revision`、`status_directive`、`retire_requested`、`create_state`、`dirty`、`next_attempt_at`、`error_code`。控制revision/时间为原安全整数，next_attempt_at为epoch秒；error_code可null，其余扩展字段非null。不暴露create_name或执行票据，不查询pending_writes/执行历史；持久observed/applied表示上次确认，不冒充实时探测或操作成功。

`PanelSnapshot`含`schema_version=1`、`captured_at_ms`、`backend_ready`、`catalog: PanelCatalog`、`glm: GlmPanelView`。backend_ready来自原lease在该时刻的判断，不持久化或接受客户端设置，不表示供应商可用。客户端写操作仍使用各自实体/策略/运行revision，不以快照时间代替版本。

`GlmPanelView`精确字段为`cycle_id: Option<String>`、`completed_at_ms: Option<i64>`、`sources: Vec<GlmQuotaView>`、`groups: Vec<GlmGroupAssessment>`。cycle_id为正u64十进制字符串，时间为epoch毫秒；数组按当前身份排序且只含GLM（包括已归档身份，但不会给旧样本续期），新进程无缓存时仍列出当前身份，数据明确未知。

- `GlmQuotaView`精确字段为source_id、source_revision、observed_at_ms、state、five_hour、weekly、level。source_revision始终是当前目录config_revision；observed_at_ms为可空的原样本epoch毫秒。state为唯一面板枚举unknown/observed/unavailable/stale，不表示路由资格。无样本→unknown；样本版本不符、未来或年龄>30000ms→stale；其余原Unavailable或Available但双窗口均无效→unavailable；否则observed。年龄端点0与30000均包含。
- 五小时/周窗口结构精确为`percentage: f64`、`reset_at_ms: Option<i64>`；百分比须finite且0≤值≤JSON_SAFE，否则该窗口null；reset只在1..=JSON_SAFE时保留（epoch毫秒），否则null。只有observed保留窗口和有效level（非空白、无NUL、最多128个Unicode字符）；其余三态窗口/level全null，stale/unavailable仍保留原样本时间，unknown时间null。原策略输入不因此修改。
- `GlmGroupAssessment`精确字段为group_id、cycle_id、evaluated_at_ms、current、imminent_members、pin_release。cycle_id为可空正u64十进制字符串，evaluated_at_ms为可空epoch毫秒；current非null布尔，imminent_members为可空成员ID数组。pin_release可空，存在时精确为`membership_id`、`pct`、`limit`；两个数均finite且0..=JSON_SAFE，否则仅抑制此显示事件，不改变提交。语义为“最近一次已提交策略判定”，不是读取时重新算出的实时选择。
- 无缓存/过期时current=false，imminent_members和pin_release均null；有旧缓存时保留其cycle/time作为过期诊断，无缓存则二者null。current=true才返回原imminent数组（可空数组）及当轮安全pin_release；下一成功轮无事件即清空，UI按cycle去重，不能每次刷新重复弹出。

私有`GlmDisplayCache`保存一轮有界、安全的来源样本与原决策展示。每个组附输入stamp：group_revision、保存后的policy_revision、完整成员集合的membership/source ID、source config_revision及暂停/退休标记、成员control revision/order/channel_id/lifecycle/退休标记。普通observed/applied/重试时间不参与stamp。它只用于判展示是否过期，不是新fence、许可或调度状态；不持久化，无通用Deserialize。

这里的完整集合精确定义为原`group_work`的`lifecycle!='retired'`输入域；包括暂停、请求退休、未绑定和非临期成员，但不包括已经退休的目录历史。目录输出仍保留全历史。退休转移使旧集合变化、新一轮则可重新current；无当前成员引用的已退休历史元数据不影响该组判定。此callee边界经[退休范围修正](../../../fixes/panel-state-fix-retired-stamp-scope.md)澄清，producer/consumer必须相同。

## 3 错误与并发

延续ControlError固定码：非法读取时刻InvalidInput，Core忙QueueFull，必需行/类型不一致或存储不可用Storage。没有部分成功快照；公开错误不带SQL、路径、原始配置或供应商正文。已有catalog错误沿原安全映射，不伪造缺失行为。

worker只读入口尝试取得原Core，WouldBlock立即QueueFull、Poisoned为Storage，不加入无界等待；持Core期间执行一个按当前目录规模读取的事务，不跨网络await。SQLite读取结束释放事务后构造只读结果；任何错误由事务drop/rollback释放资源，不改publication、命令队列或持久操作。

GLM展示只在原成功提交/发布临界区整体替换。失败、回滚、preview、旧cycle拒绝和取消均不更新；新进程为空。展示数据的筛选/清理为非失败转换，不给原策略增加新的拒绝条件。新展示不得被planner、路由或额度判据反向读取。

## 4 函数细化与论证

### A1.1 CatalogStore panel_catalog

沿原open核验固定DDL与单一受控写者前提；member_control的revision/next_attempt_at由冻结CHECK约束保证JSON_SAFE值域，原解析器保证枚举。没有外部绕过CHECK写库的合法路径；本批不增加重复全库校验或历史扫描。

前置为可用v8 Store。先usable检查，开启原连接的DEFERRED只读事务，再用read_snapshot取得目录；已有callee保证主键/类型/容量及安全字段。逐Source调用source_record取得设置与退休意图，读取模型数量；GLM只投影公共字段，火山用原policy/read_runtime校验取得可空运行状态。逐Group用read_group_policy取得保存的策略版本/选择，逐Membership用member_target补控制状态。不读execution_attempts或聚合全历史；任一缺必需行/错误终止整次读取，成功显式结束只读事务并返回。

循环上界为目录原1000/100/10000，每轮消费一个稳定身份，不按历史操作分页遍历。所有读取同一事务，不查询source_secrets。复用原解析/策略校验，不根据字段缺失补虚构状态。

论证：SQLite同一读事务使各侧表对应同一目录版本；每个目录身份恰有一条安全扩展，顺序不变；原callee保证语义，白名单转换不复制私有设置，因此输出一致且不泄露凭据。只读结束与错误释放均无数据库写。模型数量不会让模型正文进入快照；性能预算不泛称任意私有文本/历史规模。

### A1.2 已提交GLM决策展示提取

在commit_glm_round内、commit之前，从apply_glm_round_tx返回的最终groups提取原decision.imminent/pin_release，以及已保存后的g.policy和已刷新目标版本的g.work。转换不重算decide，不改targets/permits/changed列表；所有结果在原commit前准备完毕，commit失败不返回展示。

每组stamp按membership_id规范排序；保留原group_work的全部未退休成员，不只记录带标签者。总条数不超过原目录容量。来源名/凭据/模型正文不进入stamp；保留实际决定可影响的身份、版本、顺序和生命周期字段。

论证：原apply的post保证policy/目标为本次事务终态，提取因此对应实际提交而非预期。全组stamp使另一个来源暂停、增加成员或渠道绑定时也会失效；不加入与决策无关的回读字段，避免每次确认使显示闪烁。无新持久状态或策略分支。

### A1.3 worker GLM显示发布

apply_glm_cycle保持原dry_run、旧cycle和commit分支。成功后在同一Core内将当轮安全quota样本与commit返回的评估整体替换，再沿原发布返回。来源样本以当前cycle中的原observed_at_ms为时刻，不能用completed_at延长新鲜期。清理非有限数字/过长level只影响显示，不影响原策略计算和提交结果。

论证：原提交成功是缓存替换的必要条件，Core互斥禁止观察半轮。下一成功轮整体替换自然移除旧失败数据与旧pin_release事件；失败/取消不会把旧观察包装成新鲜数据。缓存容量由原ProbeCycle/目录上界约束，重启为空不会改变持久路由。

### A1.4 worker panel_snapshot

样本自带cycle_id不等于缓存整轮cycle_id时也归stale，与原适配层忽略错轮样本一致。火山runtime.coverage_end为null时，used字段即使是初始化0也不是已观测零用量；后续页面须同时展示覆盖状态，不把这些标量单独当完整统计。

获取服务端current_ms；非法时刻先拒绝。尝试取得Core，核Store可用并调用A1.1；取得当前backend lease判断和缓存的安全副本，在该一致状态上投影GLM显示。只有当前GLM来源/组进入结果，不让旧缓存复活已归档身份或火山临期标签。

来源按§2的有序分类分支投影。组current当且仅当：(a)缓存存在且完整stamp相等；(b)0≤now_ms-evaluated_at_ms≤30000，checked减法；(c)每个相关来源存在相同cycle、config_revision的样本，0≤now_ms-observed_at_ms≤30000；(d)每个原imminent成员的有效周reset_at_ms仍严格大于now_ms。空集合也不能豁免(b)。

相关来源由该组完整stamp中有channel、未退休/请求退休的成员且来源未暂停/退休/请求退休者去重确定，与原decide候选过滤一致；其余成员仍参加stamp，但无样本不破坏(c)。相关样本的原Unavailable满足(c)，表示本轮明确未知而非零；Available双窗口均无效不满足(c)。采集期间变更版本使该样本被原适配忽略时，版本不符使current=false。空组/全部暂停时相关集合为空，可在(b)窗口内展示原空判定，到期必须过期；全失败但近期样本齐全可展示原判定，不说明可路由。

匹配时只展示原提交结果，不重新算eligible或active；跨入新的临期lookahead不由页面重新判定，等下次策略轮。当前目录/策略选择仍来自A1.1，不从缓存覆盖。

论证：目录与缓存读取受同一Core保护，stamp和时间检查阻止跨版本/旧周期标签；空、失败与过期三路显式而不填零。结果不写任何状态或调用网络，因而重复读取不会刷新quota、backend或permit期限。current只修饰已提交判定的可展示性，不授权正向操作。

### A1.5 纯字段投影

Source/Group/Member/Volcano字段选择及窗口数字处理集中为小函数；每种来源/策略枚举穷尽匹配。只有外部quota和pin_release显示数值采用§2的null/未知清理；持久目录/策略的坏值、未知枚举或缺必需行必须Storage整体失败，不能清洗成null。错误不带输入正文。序列化输出字段和值域由schema机械验证。

论证：安全输出类型没有凭据/配置/错误正文容器；穷尽分支与原ID/枚举类型保持类型隔离。纯函数不产生副作用。类型getter/固定字段拷贝为trivial，其余循环/范围校验用边界与PBT验证。

## 5 测试与资源门禁

至少3组各128 cases PBT：混合来源/退休/控制版本的独立投影；GLM窗口/源版本/时间/失败的分类；组输入stamp变化与观察字段变化对显示的不同影响。固定测试另覆盖读事务失败零写、提交回滚/preview/取消不更新、下一轮清事件、重开未知、其他来源变动使整组stale、未知创建/状态写不会显示已确认、普通读取零HTTP。

机械核对新schema/唯一枚举、A1.1–A1.5注释、原DDL/算法/网络/收据冻结及精确测试入口。至少6类有效编译变异：漏秘密白名单、漏来源版本、完成时刻续期、漏全组成员、提交前更新、把unknown变空数组。失败必须来自成功编译后的断言，不将空跑/ignored算捕获。

新增独立release性能夹具为1000来源/100组/10000成员、各一短模型、无执行历史，完整只读投影≤5秒；普通容量测试保留值断言，时间只在串行门禁。既有全部性能继续执行；不承诺任意历史量或超大私有配置同样耗时。schema与完整JSON的输出容量由A2 HTTP边界另行明确，不能静默截断。

全量暴露旧导入10秒、组快照3秒墙钟失败，系统扫描另找到GLM纯容量5秒遗漏，按原批准机制另行[补齐计时隔离](../../../fixes/catalog-fix-import-performance-gate.md)，不改A1或原业务。最终性能集合为27个release入口，另4个debug入口覆盖大组5秒、导入10秒、原快照10秒、组快照3秒、GLM纯决策5秒五个原预算；A1业务测试仍19入口（18普通/1性能）。

SGM5 C尚未退出前只写设计与审核材料；之后先以独立测试目录实施，fermat可用于源码/假数据隔离验证，不上传生产配置或凭据。A1本地验收不代表页面/HTTP/运行接线已完成。

## 6 退出清单

性能夹具的worker尚无GLM观察缓存，衡量最大目录的完整未知状态投影；不把该数字称为最大已缓存stamp/临期判断的预算。后者在有缓存的组合与PBT中验证正确性，若未来承诺其最大规模耗时需独立固定该夹具。

- [x] 目标、复用、类型、错误、并发、循环上界与非平凡函数论证已列。
- [x] 独立设计复审通过，移出历史扫描并锁定完整显示谓词/字段/错误分支，0个未关闭设计阻断。
- [x] 事前十节[expectations](../../../audits/sgm-6-three-page-panel/a1-expectations.md)与[输出schema](../schemas/panel-snapshot.schema.json)就位。
- [x] 实现、基本/边界/PBT、变异、默认全量和性能通过；证据见[A1审核](../../../audits/sgm-6-three-page-panel/a1-audit.md)。
- [x] 机械、独审及五维度退出，文档/提交同步；实现提交`b4116ef`。
