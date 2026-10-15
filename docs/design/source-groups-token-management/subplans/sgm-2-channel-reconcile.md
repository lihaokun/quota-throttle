# 渠道协调器与持久控制操作细化设计

子计划：sgm-2-channel-reconcile。日期：2026-10-15。基线：f9f02aa。状态：用户已确认推进并重申本地托管架构；管理接口、目录意图、计划器、v3 持久日志、绑定/回读、HTTP worker 及本地预检已分步验证。全应用策略/权限/运行接线仍由后续子计划完成，不授权部署。

## 1 范围

本步把 sgm-1 的来源/组/成员目录接到原生 NewAPI 管理接口，实现禁用建渠道、成员映射、来源全局启停与退役、组内 priority 同步、持久操作回执和失败恢复。对应架构 G1/G6，并为 G2/G3 提供不改变策略的执行层。

事前验收清单见 [expectations](../../../audits/sgm-2-channel-reconcile/expectations.md)，操作回执格式见 [schema](../schemas/control-operation.schema.json)。本文定义完整设计，各小步实际验证范围以对应审核记录为准。

外部复审修正提案：`2c65f20` 的显式 retry 会对未熔断 Session 额外登录，已确认偏离“仅显式恢复认证熔断”的意图。修正方案及待验证的健康 no-op、单飞/取消边界见[认证重试修正方案](../../../fixes/newapi-fix-healthy-auth-retry.md)；方案待确认，代码尚未修复，不能将本补记算作验证通过。目录 v2/v3 对 SGM-1 的覆盖关系另见[迁移增量核对](../../../audits/sgm-1-catalog-store/migration-scope-review.md)。

仍然不代理模型请求，不维护 NewAPI fork，不实现逐访问 key 绑定。GLM 的 decide/pin/95%/降级/临期规则由 sgm-3 适配，火山耗尽判定由 sgm-3/4 实现；本模块不能根据 429 或管理错误自行判断上游额度。组注册/签发权限在 sgm-5，面板在 sgm-6，正式模式切换在 sgm-7。

分三批实现并逐批测试：管理传输与渠道接口；目录 v2 与持久意图；协调执行与故障恢复。全部通过才结束本子计划，不能只做传输层就标记完成。新增模块拟为 src/reconcile/{mod.rs,types.rs,planner.rs,worker.rs}、src/newapi/{transport.rs,managed_channels.rs}、src/catalog/control.rs 及迁移 SQL/测试。newapi.rs 复用新传输，不保留第二套重登逻辑；旧 orchestrator/quota 算法与面板本步不改。

生产仍运行旧部署。开发阶段只注册新模块、公开供后续集成的库接口，不在 up/run 中自动打开新目录或开始协调。最终新模式与旧 priority 写循环必须互斥启动，不能同时成为相同渠道的写者。

## 2 复用与已核实接口

- 复用 CatalogStore 的私有目录、OS 锁、事务/错误类型、来源身份、成员历史、SecretString 和 backup；不新增数据库服务或 ORM。
- 复用 reqwest、tokio、serde、sha2；tokio 明确启用 sync，测试启用 test-util 以虚拟时间验证限流/退避。Retry-After 的 HTTP 日期解析可直接声明锁文件中已有的 httpdate=1.0.3，不增加新的依赖包或服务；直接依赖清单变更在编码时显式列出。
- 模型集合继续用 normalize_models_csv/model_sets_equal；发现成功结果由来源层一次生成后扇出，不因一个源有多条渠道重复探测。
- 既有 get_channel/GET→局部改字段→PUT 思路继续使用，但新接口严格解析 success/字段，不以空数组、0 或 HTTP 200 代替成功。
- SecretString 只增加 pub(crate) 的受控后端取值方法；保持不 Serialize、Debug/Display 脱敏，不为浏览器增加明文来源 key 接口。

固定 NewAPI 版本仍为 v1.0.0-rc.20，对照源码提交 6ce7305cd36f16506fb6a2c3c524a5a318539ba7。确认的接口事实如下；此前隔离探针已验证禁用创建、跨组 priority 和批量状态的关键行为，本步仍须跑对应集成验收。

| 接口 | 使用约束 |
| --- | --- |
| GET /api/channel/ | p 从 1 开始，page_size 最多 100；按 id 升序分页，不能只取前 100 条或按名字覆盖重复项 |
| GET /api/channel/:id | 必须回读实际 ID、type、group、base_url、models、priority、status；返回正文只在后端内存使用 |
| POST /api/channel/ | 使用 mode=single 的包裹对象；显式 status=2；成功响应不保证提供新 ID，随后按唯一关联名定位 |
| PUT /api/channel/ | 删除 status 字段；只改本次拥有的字段，保留未知配置；正常 priority/model 更新不发送新 key |
| POST /api/channel/:id/status | 只写 1/2；成功后的 changed=false 不等于失败，也不等于已经满足目标 |
| POST /api/channel/status/batch | 用于同目标状态的有限批量；返回变化数量可能忽略不存在 ID，必须逐目标确认 |

依据：[渠道处理器](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/channel.go)、[渠道路由](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/router/channel-router.go)、[分页](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/common/page_info.go)。本工具不调用 CopyChannel、标签全量启停或物理删除接口。

原生请求头覆盖支持 {api_key}，并在适配器默认头之后应用，可避免把真实 key 写入 override 模板。[请求头实现](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/relay/channel/api_request.go)。这只证明机制存在，不证明火山真实请求兼容性；来源鉴权适配和普通/流式/tools 验证仍须在后续来源适配及发布验收中完成。本步保留已有 header_override，不猜测 /models 鉴权就是推理鉴权。

另一个必须显式处理的前提：原生 UpdateChannelStatus 不对“手动禁用→自动禁用”作条件保护；自动测试的恢复判断使用已取出的 channel.Status，并由全局 AutomaticEnableChannelEnabled 控制。因此迟到的自动结果可能与暂停竞争，不能只靠本地 revision 防止原生重新启用。[状态更新](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/channel.go)、[恢复判断](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/service/channel.go)、[自动测试](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/channel-test.go)。

首版的受管状态模式须确认原生自动启用关闭，并且没有别的自动启用写者；这里只读预检，不自动改全局 option。如果现有实例开启此项，保持旧部署不动，报告冲突，待用户在部署阶段选择处理方式。GLM 的 auto_ban、95%/restore/pin/临期算法仍不修改；这项前提不能偷换成“关闭 GLM 自动禁用”。本步不接管 OAuth、多 key 等带额外自动恢复机制的渠道。

AutomaticEnableChannelEnabled 已在固定版本的 [option 初始化与更新](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/option.go) 中核实为有效键；只接受明确的 false 作为该项就绪证据，缺失、空值或解析失败都是未知，不使用默认 false。

2026-10-15 的[三个隔离对照场景](../../../research/newapi-pause-recovery-race.md)进一步验证：自动启用 true 时，已回读的手动暂停 2 被测试结果改回 1；改渠道 auto_ban=0 仍复现；全局自动启用 false 时保持 2。生产配置未读取、未修改。

## 3 错误模型及分布式契约

### 3.1 错误与回执

新增 ControlError 分类：invalid_input、not_found、revision_conflict、idempotency_conflict、queue_full、not_ready、binding_drift、ambiguous_create、auth_failed、remote_rejected、invalid_response、rate_limited、transport_unknown、storage。错误只带固定代码、HTTP 状态和安全实体 ID；不得保存服务器原 message/body、请求头、私有渠道 JSON 或凭据。

HTTP 429、登录失败、分页缺口是管理面问题，不进入来源失败计数。200 且 success=false 不能当成功，200 缺 success=true 是协议错误；写请求返回 5xx、断连、超时或不可解析回执时，执行结果可能未知，不能直接当“未执行”。拒绝响应也不能证明所有写入均无副作用，仍须核对；除已核实的认证拒绝外，不把它作为安全重发创建的凭据。

OperationView 使用独立 schema：queued、applying、waiting、partial、unknown、succeeded、failed、superseded。accepted/queued 只代表意图持久化；失败回执也不撤销已经保存的暂停/退役意图。用户重试通过同一 operation_id 查询/恢复，不复制逻辑操作。succeeded 表示该 generation 的目标曾被完整回读确认，不承诺外部系统以后不会再变。

### 3.2 七项分布式契约

1. 连接：管理请求使用连接池和独立 cookie jar；连接中断不清本地意图。模型发现继续使用来源客户端，不共用管理限流。
2. 时限：connect=3 秒、单次 HTTP 含读取正文总时限=10 秒；队列等待另计，可取消尚未发送的读请求。已发送写请求不因调用方断开就视为取消成功。最大正文：单渠道 1 MiB、列表页 8 MiB，超限明确失败。
3. 重试：同一代次每轮普通读/目标幂等写最多 3 次，重试退避 1/2 秒；仍失败则保留 dirty，30 秒后进入下一轮有界对账。401 单飞重登最多一次并计入请求预算，失败后熔断至显式重试/配置修正，避免登录限流。429 使共享预算进入冷却，采用合法 Retry-After，超过 60 秒时本轮返回 waiting，由后续轮次继续等待，不能截短服务器要求；无该头时冷却 4 秒。重试前重新检查目标 revision。创建 POST 一旦可能发出，不自动重放。
4. 交付/顺序：本地意图和成员目标同事务；幂等目标至少一次尝试，靠回读收敛。单协调器、同资源串行写；组 priority 先完成降低再提升。不是端到端 exactly-once，也不能用本地 revision 声称 NewAPI 支持 CAS。
5. 失败：假设服务非恶意、请求最终完成或失败。管理不可达时显示未收敛，NewAPI 可能仍按旧状态转发。在途请求不被撤销；超时的远端写可能迟到，须重新观察及补偿，不能让旧响应更新本地新代次。
6. 会话：认证状态锁只用于快照和更新，不覆盖普通网络等待；单独 refresh 锁只保护重登。显式 admin_token 被拒绝时不改用 root。重登仅登录已有实例，不隐式 setup；首次 setup 仍是明确的引导步骤。后续普通签发身份用独立会话但共享同实例预算。
7. 流控：同一实例所有管理请求共享 1 请求/秒、突发 2 的桶；登录及每次重试都计入。控制优先于普通观测和历史补采；最多 2 个在途请求，背景最多占 1 个。发送队列最多 64 个，满则 queue_full；已有操作保持 waiting/dirty，不丢意图。后续控制入口自己的 64 槽队列满时在受理前返回 503，不阻塞数据库事务。内存队列不代表全部持久工作量。

传输层关闭自动重定向，管理路径显式规范化：集合 POST/PUT 使用尾斜杠，实体/status 路径按已核实路由生成；绝不把带管理凭据的请求跟随到未知目标。旧 NewApiClient 对应集合方法同步改用规范路径，保留原功能。

## 4 类型与持久化

跨模块类型及所有者同步到主架构“第二子计划共享类型补充”。接口字段不是最终面板 API；面板接入时仍需管理员鉴权/CSRF 门禁。

### 4.1 共享类型

| 类型 | 字段和不变量 | 生命周期与消费者 |
| --- | --- | --- |
| OperationId | 16..64 位 ASCII 字母/数字/下划线/短横线，首位字母或数字；不 trim 改写 | 调用方生成并复用；控制面、store、协调器；不是权限凭证 |
| ControlCommand | EnsureMember、SetSourcePaused、RetireMember、RetireSource、SetGroupPriorities、SyncSourceModels；带目标 ID、expected_revision、规范参数 | 仅明确的受管动作，不接受任意 SQL/JSON patch；部分生产者在后续子计划接入 |
| StatusDirective | preserve / enabled / disabled | GLM 正常 preserve；火山由状态机给出 enabled/disabled；人工暂停和退役始终覆盖为 disabled |
| MemberTarget | membership_id、revision、desired_priority、status_directive、retire_requested、create_name | 持久化的执行目标；revision 正整数且 ≤JSON_SAFE，只有 store 更新 |
| RoutingPermit | source_id、source_revision、group_id、group_revision、允许启用及期望 priority | 仅可信策略/预检层产生；非序列化，过期拒绝；不是从 HTTP 直接接收的布尔值 |
| WorkTicket | membership_id、revision、action、attempt、必要身份/组/来源 generation | 私有执行快照，不含可对外序列化的 key；每次发出前核对是否仍有效 |
| WorkSnapshot | 单个 member/target/source/group、来源退役请求、所需同组目标与观测 | store 在一致读事务发布；普通成员任务不加载全库；提升屏障按需取同组数据 |
| BackendReadiness | 已核实的版本/接口、原生自动启用开关、绑定部署信息及检查时间 | 只读预检产生；unknown/冲突不能按 ready 处理，也不触发全局 option 写入 |
| ChannelObservation | channel_id、type、group、base_url、models、priority、status、auto_ban、观测时间 | 后端 typed 结果；缺失字段是错误，不能默认可用；未知原字段仅 RawChannelDocument 保留 |
| OperationView | schema_version=1、operation_id、kind、target_type、target_id、phase、target_revision、total_targets、confirmed_targets、error_code、updated_at | 白名单只读视图；confirmed≤total，succeeded 必须全部确认；无秘密/原始错误正文 |

ControlCommand 的引用和类型匹配在同一事务验证。相同 operation_id 先查原收据：参数相同返回原操作，不要求旧 expected_revision 仍是当前版本；参数不同返回 idempotency_conflict。新 ID 才执行 expected_revision 检查。payload 摘要只来自规范结构，不包含时间或遍历顺序；敏感值不放操作表。

命令参数：EnsureMember 指定已有计划成员和目标状态；SetSourcePaused 指定来源及 paused，恢复还需策略层提供新鲜许可；RetireMember/RetireSource 指定待退役身份；SetGroupPriorities 指定组与完整有效成员的 member_id/priority 集合；SyncSourceModels 指定来源和非空成功发现模型集。expected_revision 分别对照 member_control.revision、sources.config_revision 或 source_groups.revision。RoutingPermit 由执行时的可信策略层刷新，不作为可由前端自行签发的许可保存。

### 4.2 数据库 v2

保持 sources、source_groups、memberships、source_secrets 和 legacy_import 的现有记录/ID/历史归属。新增私有侧表，避免为本步改写所有核心表：

- source_control：source_id PK/FK、retire_requested。来源手动暂停仍在 sources.manual_paused；来源控制变更增加 config_revision，后续策略改变可路由资格时也必须使相关控制 generation 失效。
- member_control：membership_id PK/FK、revision、status_directive、retire_requested、create_state、create_name、dirty、next_attempt_at、last_error_code。create_state 为 never / may_have_been_sent / resolved；create_name 唯一，retired 后不复用。原 observed/applied 字段继续在 memberships，不复制第二份观测值。
- control_operations：operation_id PK、kind、target_type/ID、payload_digest、target_revision、expected_targets、phase、error_code、created_at/updated_at。expected_targets 冻结提交时的目标数量，不因子收据行丢失而把非零操作误判为零目标成功。
- operation_targets：operation_id/member_id 联合主键、target_member_revision、confirmed_at、非敏感回读摘要；确认数从已核对的目标收据集合计算，不凭收到几次 ACK 递增。成功操作的历史收据不因后来更新成员而消失；尚未完成的操作被新目标取代时标为 superseded，旧回执不能完成新操作。

catalog_meta 重建为 version=2 并增加随机 catalog_instance_id（SQLite randomblob 生成 128 位十六进制标识），与 PRAGMA user_version 在同一事务更新。新库先建不可变 v1 核心，再运行相同 v1→v2 迁移；旧库先只读验证 v1 身份/约束和完整性，成功备份后才迁移。迁移失败回滚，未知提交重开核对，绝不初始化成空库。

v1 的 CHECK(schema_version=1) 不能直接 UPDATE 成 2，必须事务内重建这一无外部 FK 的元表。侧表为全部存量来源/成员补行，成员 revision=1、status=preserve、dirty=false；imported 仍未接管，不能因升级而发启停请求。

数据库 schema 版本与公开快照格式版本分离：核心 CatalogSnapshot 字段不变，wire schema_version 仍为 1；数据库版本升为 2。OperationView 另有格式版本。此处通过具名常量区分，不让 UI 把数据库版本误当 JSON 格式变更。

新 source/member 写入时在原事务中补对应 control 行。申请退役后拒绝再加入新成员，但仍保留全部历史。新操作更新目标并增加相关成员 revision，同时使旧 applied_revision 失效。旧回执最多用于诊断/重新观察，不能将新意图标记完成。

operation 记录和关联名不自动删除，以保持重放语义；读取采用分页或按 PK，不全量载入内存。组/来源/成员容量保持 sgm-1 上限，dirty 表最多一行/成员。删除来源是停路由后归档，不删除供应商 key 或 NewAPI 渠道。目录元实例 ID 是本工具的命名空间，不是 NewAPI 提供的数据库身份证明；更换/恢复 NewAPI 数据库必须走 sgm-7 的显式绑定核对，不从同一 URL 推断仍是旧实例。

### 4.3 状态控制边界

- GLM 正常调度只更新 priority，status=preserve，保留已有 auto_ban 和 status=3。不能把 GLM 达到 100% 自动改成火山式停用，也不能因 quota 未知把已有渠道关闭。
- GLM 的新成员首次启用和人工恢复需要可信层提供当前组的 RoutingPermit/priority 计划；协调器先完成会造成双首选的降级，再启用。正常运行后回到 preserve，不持续把原生自动禁用拉起。
- 火山渠道 auto_ban=0，状态由来源策略统一下发；恢复前 priority 先降到 standby。其规则 1/2、重置及未知遥测的可用性判断不由本模块生成。
- manual_paused、source/member retire_requested 或 retired 覆盖任何 enabled。恢复只清人工暂停，不清耗尽原因；若无有效 permit，保持禁用并显示 waiting/not_ready，不伪造“已恢复可用”。
- Group.ready=false 时不启用新渠道；imported 仅做绑定核对，未明确接管前保留原状态和优先级。已有 default 流量不能因迁移的临时未就绪状态被自动关闭。
- 组优先级计划必须包含该组完整有效成员集及期望 revision；只修改该组渠道，不触及另一组副本。每次提升前重新确认阻挡它的旧高优先级已降低或禁用。

## 5 模块与函数细化

### 5.1 调用契约与并发所有权

C1 CatalogStore：单写者持有连接；事务不 await 网络；新目标/操作同提交，未知提交的句柄不可再用。C2 管理传输：按 §3 返回严格响应或分类错误，认证/限流不泄密。C3 NewAPI：只提供原生写入和回读，不提供 generation CAS。C4 策略层：提供匹配当前 source/group revision 的 permit 和完整 priority 计划，不把来源未知当可用。C5 时钟：调度用单调时间，持久 next_attempt_at 用 UTC；进程恢复将逾期任务视为可检查，不伪造过去回执。

协调器独占所有受管渠道写入。store 的同进程单写者同时接收控制请求、策略意图和网络结果；HTTP 执行不持有数据库事务或认证状态锁。调度器每次只取有限工作，发出前验证 ticket；结果返回后再验证 revision。所有可能破坏跨源/跨组不变量的更改通过 store 方法，不给面板生产 setter。

### 5.2 传输和认证

函数：ManagementTransport::send(request: ManagementRequest, class: RequestClass) -> Result<ManagementResponse>。请求类型包含固定 method/path、内部请求体、Read/IdempotentWrite/Create 语义；不对外 Debug 请求体。

前置：合法实例 URL、已规范化管理路径和可重建请求；caller 不能把创建伪装成幂等写。后置：一次明确响应或分类未知结果；每次实际发送都消耗预算，401 恢复最多一次，秘密不出现在错误中。

P1 验证 method/path/正文上限，排队；满则在发送前 queue_full。P2 调度获得预算及在途槽后读取 auth generation 快照，释放 auth 锁再发送，完整读取有限正文。P3 非 401 交给分类器；401 先释放响应/在途槽，token 模式直接 auth_failed，会话模式进入 refresh 锁，若其他请求已更新 generation 则复用，否则只登录一次，再按预算和在途限制重放一次。不能拿着全部在途槽等待另一条登录请求，否则会死锁。第二次 401 或登录失败返回 auth_failed，释放所有锁。

普通 GET 和设置目标字段的重试由 worker 决定，send 不自行无限重试；创建 POST 只允许确认被认证中间件拒绝的 401 重放。登录 POST 不因未知结果循环请求。setup 不进入重登路径。循环上限两次业务发送；没有持锁等待普通响应导致全体饥饿。

正确性：C2 的分类让调用者区分拒绝和执行未知；auth generation 防止一波并发 401 重复登录，单飞锁不包围普通请求；请求体只在后端构造，错误输出白名单不暴露秘密。

函数：Budget::next_ready(now, queues) / admit / cancel_before_send。维护两枚令牌容量、1 秒补一枚、cooldown 截止及 64 个等待槽。先选控制，再普通观测，最后补采；每次只做一个决定并返回下一唤醒时间，不 busy-loop。退避中的低优先请求重新排队，不占执行槽。测试使用虚拟时间，断言任意时间区间内发送次数≤2+floor(持续秒数)，而不只测一次 sleep。

### 5.3 渠道接口

函数：scan_channels(query, required_ids) -> Result<ChannelScan>。

P4 从第 1 页开始，请求 sort_by=id/sort_order=asc/page_size=100；逐页严格校验 success、total/page/items、正 ID 和非重复 ID，规范化必需字段。P5 已找到全部所需 ID 可返回有覆盖范围的部分扫描；需要完整扫描时直到完整总数或末页，最多 101 页/10000 条。总数变化、重复页、缺页或超界返回 incomplete，不把“本页没找到”当资源不存在。列表不是远端原子快照；已知目标回读优先按 ID，创建关联名的缺失不能证明 POST 未执行。

函数：get_channel(id) -> RawChannelDocument；document.observation() -> ChannelObservation；patch_owned_fields(document, patch) -> RequestBody。RawChannelDocument 无公共 Serialize/Debug，保留服务器未知字段；patch 明确白名单，删除 status 与 key（本步不轮换已有 key），只更新指定 priority/models 或火山 auto_ban 等受管字段，其他字段保持原样。缺少必需字段或 ID 不匹配即失败，不能用默认 0。模型集合相同时不 PUT。

函数：check_backend_readiness() -> BackendReadiness。只读取版本/管理能力及 AutomaticEnableChannelEnabled；字段缺失、无读取权限或自动启用开启均返回 not_ready，不发 option PUT，也不开始接管已有渠道。后台确认的 ready 不能替代 sgm-5 的组权限就绪或 sgm-3 的来源就绪。对暂停/退休渠道仍保留观测范围；原生迟到禁用可从 2 变 3，两者都不路由，但本工具需再对账到明确的手动禁用 2。

函数：create_disabled_channel(spec) / write_channel_fields(id, patch) / write_status(ids, status)。P6 新建固定 mode=single、明确单组、status=2、standby priority；火山 auto_ban=0，GLM 沿用原生默认。P7 写状态使用专用 endpoint，批量≤100 个且 ID 去重；返回只代表收到接口回执，所有写函数均不直接更新 applied_revision。只支持单 key 渠道，不接管 multi-key 模式。

正确性：有界分页避免静默只读首屏；白名单 PATCH 和明确 group 防止跨组覆盖；创建固定禁用排除未知结果期间的流量副作用；C3 决定最终成功必须另行回读。

### 5.4 v2 迁移与提交操作

函数：open_v2(path) / migrate_v1(tx) / verify_v2(conn)。P8 先执行 sgm-1 的私有路径/身份检查；高于 2 的版本只读拒绝。v1 先验证和一致性备份，再单事务新增元信息/侧表/回填，最后提交版本。新库走同一迁移路径；失败保留原库及备份，不删除或认领其他文件。核心 v1 DDL 冻结；v2 验证元表、侧表约束、外键和一对一完整性，不靠猜表名存在就通过。循环按原有容量上限终止。

函数：submit_operation(operation_id, command) -> Result<OperationView>。P9 规范参数，查同 ID；相同摘要返旧收据，不同摘要冲突。P10 对新操作验证目标、类型、expected_revision、完整成员范围和 retired/retire_requested 约束，保存源暂停/退役意图、成员目标/新 revision 和 operation_targets，同事务提交。队列只作唤醒提示，丢失唤醒不能丢意图。提交后再发布 queued，不能在事务前声称受理成功。

SetSourcePaused(true) 不依赖策略 permit，所有关联渠道目标为 disabled；false 必须逐成员重新计算资格，不能复用暂停前的过期启用决定。RetireSource 原子标记暂停及退役请求，阻止后续加入；RetireMember 只影响指定组成员。SetGroupPriorities 仅接受完整同组计划。SyncSourceModels 只接受来源层的非空成功发现结果，失败发现不产生写意图。

并发 pre/post：只有一个 Store writer；同一代次成员目标唯一。成员期望字段改变必须递增成员 revision；来源/组 generation 也参与发出前与 ACK 后检查。单纯依赖就绪变化可以保留原等待操作，但必须取新 ticket/permit；只有新命令改写目标才将旧未完成操作 superseded，不能把等待条件满足错误解释成取消。create_source/plan_membership 的 v1 接口继续保留；未显式提交协调操作的目录记录不自动发送网络请求。

### 5.5 纯计划与执行

函数：plan_next_action(snapshot, observation, now) -> PlanStep。

P11 先应用源暂停/退役与成员退役 gate，再判断绑定和 ticket 是否新鲜。无绑定且 never：生成 CreateDisabled；已可能发送但无绑定：只生成 LookupCreate，禁止第二个 POST。已绑定但存在 group/type/endpoint 身份漂移：返回 Drift，不偷偷重建或改认 channel。imported 未接管：只读核对，不自动降级/启用。

P12 对有效目标产生有限下一步：需要禁用先禁用；需调整模型/priority 先字段写；需启用必须有有效 permit、组就绪、无源暂停，且优先级安全屏障已回读。火山恢复先 standby 再 enabled；GLM normal preserve 不生成自动启用。源归档/成员退休必须等全部应停止的目标确认后才最终标记。

纯函数不修改数据库/网络；同输入同结果。它不选择模型、不运行 GLM/火山策略、不把管理失败变成耗尽。每次只返回一个 Step 或 Waiting/Done/Error，避免持有过期整份行动序列长时间执行。

函数：execute_step(ticket, transport) -> StepOutcome。P13 发出前由 store 再确认 revision；CreateDisabled 先持久标记 may_have_been_sent，才发 POST，防止崩溃后重复创建。已经写下发送意图但实际未发出的崩溃窗口也保守按 unknown 处理。其他请求遵循预算和有限重试，错误返回不清本地期望。

创建关联名为 qt-<catalog_instance_id>-m<membership_id>，与来源展示名无关、不包含 key，存储后不变。LookupCreate 按完整精确名字匹配并核对稳定绑定属性（type、单组、endpoint、非 multi-key）：0 个保持 unknown；1 个才绑定；多个 ambiguous_create，全部保持未确认，不取“最新一个”。models/priority/status 是可协调字段，不因创建期间发生新发现而误判为另一个来源。缺失不能触发无条件重新 POST，人工处理入口须显示该未知状态及安全 ID。

### 5.6 回读与恢复

函数：apply_step_result(ticket, outcome) -> Result<OperationView>。

P14 事务内重查 revision。过期结果不能更新新目标的 observed/applied 或终态；旧创建若带回确定 ID，可只登记永不变更的身份并立即按最新暂停目标重新协调，不能沿用旧启用计划。已有不同 channel_id 则冲突，不覆盖历史。

P15 对本代次写后重新获取目标实际状态，逐 ID/字段比较。只有本代次要求的字段全一致才设置 applied_revision 并保存该代次确认收据；preserve 字段不伪造固定值。batch changed 数量不作为确认依据，缺失或不可读渠道保留 partial/unknown。确认计数按收据中的唯一成员计算，不能因重复 ACK 加两次；源退役确认所有成员后再归档，成员物理渠道保留禁用状态。

函数：recover_pending / reconcile_once。P16 重开时使缓存观测失效并回读，已发送创建只查关联名。每轮从 dirty 表取至多 64 个工作且按控制优先；全部延期则返回最早唤醒，不忙轮询。暂时不可达/429 在每轮有界重试后继续保留 waiting/partial 和 dirty，网络恢复后自动对账；认证失败、绑定漂移等需修正配置的情况才 failed。未知创建仍只重查。不得为了继续运行丢失 source.manual_paused。

函数：retry_operation(operation_id)。这是显式恢复请求，不改变原始命令/幂等 ID。事务中核对目标仍是本操作的有效代次；已 succeeded 只返回旧收据，superseded 返回冲突而不重放旧意图；其他状态重新安排观察。unknown 创建保留 may_have_been_sent，只查关联名，不因点击“重试”获得第二次 POST 许可。认证熔断只允许此次显式恢复再尝试一次，失败仍关闭，避免自动登录风暴。

迟到写限制：NewAPI 没有条件写。即使本地拒收旧回执，远端仍可能在超时后才执行旧请求；因此持久 dirty/周期复核必须再次修正。没有已知终止结果的写不得仅凭一次恰好匹配的 GET 就清掉 unknown。对同资源的相反操作，先保留未收敛状态并重新核实，禁止宣传线性化或立即阻断保证。

### 5.7 helper 规约

| helper | 前置与推导 | 后置及终止 |
| --- | --- | --- |
| canonical_command_digest | 已验证枚举/ID；集合按 ID 排序，拒绝重复目标，再稳定序列化 | 同语义同摘要，错误不带敏感载荷；有限目标集 |
| build_creation_spec | 身份映射未绑定、关联名已持久化；从 SourceMaterial 取 key，仅在内存组包 | 必定单组/status=2，火山 auto_ban=0；不改 store |
| parse_envelope / classify_response | 有界正文、HTTP 状态、请求语义 | 区分拒绝/未知/限流；任何缺失字段不默认成功；纯有限解析 |
| load_work_snapshot / is_ticket_current | 一致读事务取目标及必要同组数据，核对 member/source/group generation、BackendReadiness 和 gate | 快照或 bool/storage 错误；无写入，旧票不能通过；最多遍历同组合法容量 |
| verify_binding / verify_readback | 正确目标 ID、最新观测与规范目标 | 分别校验稳定身份及受管字段；不从当前名字推翻历史 SourceId |
| group_promotion_barrier | 当前同组完整目标和新鲜观测 | 只有所有阻挡提升的旧高 priority 已降低/禁用时放行；不扫描其他组 |
| completion_projection | 操作目标及匹配代次的确认收据；待执行目标有新代次时先 superseded | confirmed≤total；新操作全部新鲜匹配才 succeeded；旧成功记录保持历史收据，不因重复 ACK 增加计数 |
| backend_secret_access | 仅后端 crate 内可信代码调用 | 返回临时借用；不新增 Serialize/Debug 明文行为或 HTTP 接口 |

所有状态修改沿 C1 事务路径，SQL 用绑定参数。新增非平凡 helper 须先补同等契约；不引入通用工作流引擎或任意插件系统。

## 6 完整性检查和退出条件

- [x] 目标与模块、接口类型和所有者明确；不让协调器重写供应商策略。
- [x] P1–P16 有成功、拒绝、未知、过期、重启和部分失败路径；循环有页数/容量/次数上限。
- [x] schema v1→v2、不可逆的远端未知结果和本地 revision 的能力边界明确。
- [x] 已产出操作 schema、设计样例及十节 expectations，先于业务代码。
- [x] 用户确认细化后逐批实现；不得把当前设计当作验收通过。
- [ ] 全部基本/边界/PBT、变异、机械检查、隔离原生接口验收、独立审核通过后才结束 sgm-2。

测试不使用生产目录、端口 3000/3001 或真实 key；HTTP fake 绑定随机回环端口，NewAPI 原生实测也使用独立数据库/账号/假上游。新模块验收必须覆盖 85 个现有测试，不弱化 sgm-1 的完整映射、直接脱敏和容量重复规则。

关键 PBT：重复 operation_id 与乱序 ACK 的参考状态机；同源两组暂停扇出；A 组 priority 不写 B 组；未知 POST 不重复创建；退役期间新成员/迟到创建不能启用；group 提升屏障；分块分页与重复/缺口；限流发送轨迹。每项至少 128 cases，按不同来源/组生成可区分模型、priority、状态和凭据，不仅比较总量。

定向变异至少包括：删掉发出前 revision 检查、删掉 ACK revision 检查、batch success 直接计完成、unknown POST 自动重发、恢复时先 enable 再降 priority、把 GLM preserve 改成 enable、分页只读首屏、管理请求绕过预算。目标测试必须明确失败；全量正常代码仍须通过。

### 第一批实现边界及辅助函数对应

ManagementBudget 以显式共享 Arc 注入各身份的传输，容量和令牌由同一个短临界区维护；Admission 负责排队取消与在途释放。第一批增加保守入口门禁：最多 64 个逻辑请求，包括等待认证、等待发送和在途；内部登录不重复占逻辑槽，但仍消耗发送预算。这比“64 个等待槽另加 2 个在途”更严格，防止认证锁外形成无界等待。admit 先注册通知再检查条件，避免丢唤醒；只有选中的最高优先级可发送者设置定时唤醒，其余等状态通知，不忙轮询。后台只占一个槽，取消排队不消耗令牌，发送之后取消不退还令牌。不可表示的 Retry-After 截止时间保持冷却，不缩短服务器要求。

refresh_session 对应 P3：生成代次快照→单飞锁复核→共享预算→登录→更新会话。登录已经发出后取消，RefreshCancellation 将对应代次标为认证失败，避免下一等待者无条件再次登录。排队尚未获准不属于登录失败。retry_authentication 只重置认证熔断并尝试登录，不重放业务命令；bootstrap 只用于既有 CLI 显式首启，自动重登不调用它。保留反向代理 base_url 路径前缀；拒绝 URL 自带凭据及覆盖 Authorization/Cookie/Host 的额外头，配置错误不回显内容。

RawChannelDocument::patch 对应 patch_owned_fields；ScanAccumulator::append 对应 P4/P5 的逐页校验；creation_payload 对应 build_creation_spec 的 HTTP 组包部分，稳定关联名的持久化仍由后续协调器承担。字段更新只返回是否收到写回执，不产生 applied_revision。NativeRecoveryCheck/check_native_recovery/parse_recovery_option 只验证原生自动启用开关，不能被当作完整 BackendReadiness；实例绑定、版本/能力和策略许可将在协调执行批次合并。

这些辅助函数只操作有限输入或预算状态：逐页最多 100 项、全扫描最多 10000 项、队列最多 64 项；HTTP 总时限包括响应体读取。解析失败不产生空成功；错误类型不保存原始响应或 reqwest 错误链。新模块供后续协调器调用，尚未接入生产运行模式。旧 NewApiClient 仅替换管理传输和集合尾斜杠，模型发现使用独立 HTTP client，原 GLM 选择函数不改。

NewApiClient::new 委托 with_budget；后者由组装层注入同实例预算，managed_channels 借用同一传输，不创建第二套管理会话或预算。三个包装函数仅构造/借用既有类型，无新业务状态；新模式仍不得与旧调度器同时写相同渠道。ManagementError::for_request 将写请求的不可解析/超限回执转为 transport_unknown，读请求保留 invalid_response；显式 setup 和受管业务写均使用此分类，login 失败仍统一认证熔断。会话身份在 HeaderMap 中覆盖 New-Api-User，避免追加静态旧 ID；token 模式保留配置的用户 ID。

### 第二批数据库与操作边界

本批实现 P8–P10，仅产生本地目标和 queued/superseded/无外部目标时的 succeeded；不发送 HTTP，不生成远端确认，不实现 P14–P16 的 ACK/重试入口。控制共享类型暂归 catalog/control，后续协调器引用，不重复定义枚举。数据库版本常量与 CatalogSnapshot/OperationView 格式版本分开。

open 保留现有私有路径和 OS 锁检查。旧库在只读连接中核对版本、DDL、quick_check、外键、完整目录及私有材料，再打开可写连接。合法旧 v1 库使用既有 backup_to 生成同私有目录下不覆盖的 catalog-v1-before-v2-<随机标识>.sqlite3，备份文件与所在目录同步成功后才开启迁移事务；新空库无需冗余备份。迁移先重建元表，再建立侧表与索引、填充所有来源/成员控制行、设置两个版本标记，提交后复核。迁移失败保留原库和备份，重试生成新备份，不覆盖旧文件。

verify_schema 按版本比较不可变 v1 核心及对应元表/侧表 DDL；verify_control 检查一对一完整性、创建名与实例/成员身份的一致性、创建状态与 channel_id 一致性。新增侧表使用 STRICT 和有限值域/FK/CHECK，操作按 PK 读取，目标最多 10000 个，不把全部历史操作加载内存。insert_source/insert_membership 在原事务补对应控制行；retire_requested 或已归档来源拒绝新成员。

canonical_command_digest 的输入只有受限命令：操作 ID 16..64 ASCII、正 expected_revision、合法实体 ID；priority 集合排序并拒绝重复，模型列表规范为非空有界集合后排序；正文不含凭据。摘要包括 expected_revision，不包括当前时刻。submit_operation 先规范载荷并查同 ID，完全相同返回历史收据；新 ID 才核对当前 revision 与完整有效成员集。相同 ID 不同参数、缺失对象、退休对象、溢出或不完整组计划均整笔拒绝。

提交时 source/group 命令递增相应实体 generation，受影响成员递增 revision、清 applied_revision、置 dirty，并在同事务保存操作与目标行。来源暂停 true/退役指定 disabled；恢复记录 enabled 请求，但不生成 RoutingPermit、不开启渠道、不清供应商耗尽状态。完整组 priority 只更新该组有效成员，来源模型成功发现只更新自身 settings 中的模型字段及关联有效成员目标，保留 quota/header/key。退休来源不能新增成员，成员退役请求不得被普通 EnsureMember 撤销。

新命令替换成员目标时，关联的旧未完成操作标 superseded；已有 succeeded 收据及目标行不删除。来源级命令覆盖仍有效的全部成员，已退休身份继续保留而不再启用。零成员的暂停/模型更新仅有本地效果，可立即 succeeded；零有效成员的来源退役同时归档。非零目标无论是否已有观测均 queued，确认数为 0，后续必须新鲜回读。operation_projection 只按目标收据集合计算确认数并验证 succeeded/failed 的组合，不凭 phase 或 ACK 次数编造确认。

审核补充：原始 expected_targets 在操作创建时固定，OperationView 的 total_targets 来自这一值，并核对实际子行数；membership 类型必须恰好一个目标。子行丢失时读取/重开均报告存储损坏，不能把“当前没有子行”解释为原本零目标本地操作。此私有计数不替代逐成员收据或新鲜回读。

正确性依据：单写者事务使意图/成员代次/收据目标同时可见，失败回滚不留下半个暂停；先查幂等 ID 保留重放语义；保留旧成功收据避免历史随新目标改变。新库及迁移库使用同一侧表初始化，实例 ID 只生成一次，创建关联名不依赖名称/模型/凭据。所有循环受目录容量或当前操作目标数限制；备份继续使用有界分步 SQLite backup。

本批测试须覆盖随机 v1 来源/组/成员完整材料迁移、迁移 DDL 中途失败回滚、私有备份与旧格式、侧表破坏拒绝、容量重复不退化；操作重复/不同载荷/过期 revision、跨组 priority 隔离、来源暂停扇出、退休拒绝新增成员、同事务注入故障、重开与历史收据保持。R1/R2/R11 可在本批测试本地语义；实际网络 ACK/gate/恢复仍待第三批，不把本地测试冒充远端协调验收。

### 第三批计划器与发送门禁

第三批先实现 P11/P12 的纯计划与传输发送门禁，再接持久执行记录/worker。计划器不持有 key、数据库连接或网络客户端；WorkSnapshot 包含来源/组/成员/目标、端点身份、是否已有明确受管意图及未决写状态。上下文包含可信预检与策略许可、当前组完整成员 ID 集及逐成员观测。生产者必须从一致目录和实际观测构建输入；这些输入类型本身不是已经完成预检或策略接线的证明。

BackendReadiness 和 RoutingPermit 无 Deserialize，字段私有，只由 crate 内可信预检/策略层构造。许可核对来源/组 ID、generation、期望 priority 及有效时间窗；组 ready=false 或许可缺失/过期时不启用/提升。预检至少包含固定受支持版本、自动启用明确关闭与实例绑定已核实；首版计划器只接受本项目已验证的 Custom(type 8) 渠道，不猜测 OAuth 等渠道的恢复行为。

观测绑定实际 channel ID 与目标 generation，观测时刻不因转换刷新；超过 30 秒或来自未来的观测视为需重读。提升屏障要求同组完整可能路由的成员集合，无重复/缺失/跨组项；包括仍可能是旧首选的退役中成员，不能复用排除 retire_requested 的可编辑 priority 集合。每个已绑定 peer 都有当前代次的新鲜身份匹配观测，未决写必须先收敛，需降低或禁用的 peer 必须已实际完成。高于 standby 的优先级提升还不能碰撞另一个未降级的同级/更高可路由渠道。此检查不能被“收到了 batch success”替代。

计划先处理退役/暂停 gate，再校验绑定和观测。尚未发送过创建的无渠道退役成员可以本地确认不存在；may_have_been_sent 始终查关联名，不再生成 POST。导入但没有明确接管意图的成员只读观察。存在未决写时不把一次匹配 GET 当完成，也不发相反启用；它需要后续持久执行记录保留跨重启语义。查创建结果必须完整扫描，0 个保持 unknown，多个 ambiguous，唯一匹配还须核对 type/group/endpoint/单 key。

火山从禁用恢复先降到许可给出的 standby（若目标更低则用更低值），再启用，最后才按完整组屏障提升到目标 priority。GLM 正常 preserve 不改 status；显式启用在本代次已经回读确认后按一次性动作处理，不能持续把原生 status=3 拉回 1。后续 priority/model 命令继承已完成 GLM 启用目标时规范为 preserve；未完成的人工恢复仍保留 enabled 意图，新的显式恢复可再次请求启用。

GLM 的一次性启用还需持久 enable_consumed 证据：确定的启用回执之后，若后续回读已变为 2/3，报告未恢复成功而不再次 enable；不能等全部字段 applied 后才防止反复拉起。消费证据属于一次显式启用授权，不是一般 member revision：即使尚未 applied，后来的 priority/model 命令也必须继承已消费证据或转为 preserve，只有新的显式启用/恢复可以重新授权。“未完成恢复保留 enabled”仅适用于尚未消费的启用。该证据由后续执行记录产生，计划器仅消费；当前目录规范化仅覆盖已有 applied 证明的情况，剩余持久证据必须在 worker 接线前补齐。未知旧写允许在身份核对后补偿禁用，但保持 unknown，不因此确认暂停成功。在禁用状态更新 priority 不改变路由；会实际改变路由的提升以及所有启用仍须经过许可与组屏障。

ManagementTransport::send_fenced 在最终预算准入临界区同步调用可信 guard，位置在预算等待之后、HTTP send 之前，包括 401 重放；同时复核认证熔断。guard 返回后重新取单调时间补桶，通过才扣额度/占在途槽；拒绝不计费，已准入后取消不退款，避免迟到退款突破 burst。普通 send 使用恒真业务 guard 保持旧接口。guard 只允许有界的原子/已发布状态读取，不做数据库、IO、等待或预算重入；固定锁序 Budget→auth，普通网络等待不持这些锁。保证的是最后本地检查与预算准入，不是操作系统调度下物理发包时刻的硬实时上限。

worker 需在提交新意图前使相关旧 fence 失效，提交后发布新代次/资格，失败则重载；不得在 DB 已提交而发布尚未更新的窗口继续放行旧票，也不能只检查闭包捕获的旧快照。ACK 后仍须再次查数据库。这个门禁不撤销已经发出的请求，不宣称远端 CAS。

观测还带 execution revision：同一意图内每次发起写也使旧观测失效，不能只凭 member/source/group revision 相同便复用写之前的 GET。持久执行器负责推进该序号、给读请求盖章并拒收旧读回执；同组影响路由的动作须串行到回读屏障完成，或提供等价的组级执行 fencing。上述执行序号与组级 fence 同样属于后续持久记录/快照装配，不在纯计划器中伪造。

无绑定 peer 仍须计入完整集合。只有从未绑定且 CreateState 为 Never/MayHaveBeenSent 时，才可依“创建恒 status=2、先持久绑定才允许 Enable、type 8、自动启用关闭且无其他写者”证明其唯一可能效果是禁用副本，因此不阻挡其他已绑定来源切换。Resolved 却无绑定是异常；已绑定 Unknown/InFlight 继续阻挡。数据库恢复/映射遗失无法证明上述前提时不能豁免。创建自己的 unknown 和禁止重复 POST 不变。

本小步以随机 gate/代次/peer 顺序/未知创建、逐步恢复轨迹及真实 HTTP 队列/401 重放验证纯计划和发送门禁。真正的 WorkTicket 持久记录、来源/组变化发布、ACK/回读确认、崩溃恢复与 worker 仍须随后完成。新增执行数据不得直接改写已经提交的 v2 DDL，需显式版本迁移；不得以只有内存里的 unknown 标志冒充跨重启恢复。

### 持久执行与 worker 细化

新增数据库 v3，仅重建版本元表并追加 member_execution / execution_attempts；冻结 v1/v2 DDL，CatalogSnapshot 和 OperationView 仍为格式 v1。旧 v1 库保留现有 v1→v2 备份/迁移，再做 v2→v3；原生 v2 库先只读完整验证并生成私有 v2 备份。新空库直接顺序执行相同迁移，无多余备份。每次迁移失败回滚到该次起始版本，保留已有备份；例如 v1→v2 成功后 v3 失败，保留 v2 和原始 v1 备份，不声称两次迁移是一个事务。不篡改实例 ID 或历史归属。

member_execution 一行/成员，保存单调 execution revision、显式启用授权 epoch、授权起始 member revision、消费标记。execution_attempts 以 member/sequence 为主键，保存当时 member/source/group revision、授权 epoch、固定动作、无凭据字段效果、prepared/responded/not_sent/uncertain 状态和安全错误码；不存 key、请求头或完整原生渠道 JSON。prepared 在发出前提交，崩溃恢复转 uncertain；新意图不删除旧未决尝试。未绑定创建即使未确认，也只能通过稳定名字对账，不能重新 POST。

启用授权只在显式 EnsureMember(enabled) 或恢复来源时递增并清消费位，普通 priority/model 更新保留。确定 Enable 回执或已满足启用目标的回读消费该授权；迟到回执只消费相同授权，不覆盖后来新显式授权。GLM 普通新意图继承已消费授权时按 preserve；旧授权已消费但本次显式恢复尚未完全确认且原生又禁用时，不重复 Enable。

传输错误附带交付证据，区别 NotSent、ResponseReceived、Uncertain：参数/队列/最终门禁拒绝、确认的 401、连接建立失败为 NotSent；完整且可识别的应用回执为 ResponseReceived，不等于动作成功；断连、超时、正文未收完/超限及不可识别的响应为 Uncertain。固定同步管理接口 UpdateChannel、status 和 batch handler 在写入/缓存更新后才返回 JSON，见[原生处理器](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/channel.go#L996)。仅收完代理错误页不能证明该处理器已结束；非法 JSON/缺少布尔 success 的写回执继续 transport_unknown/Uncertain，不能无条件当作终止证据。已绑定的 Uncertain 写不靠匹配 GET 清除；可以补偿禁用但保持未收敛，不能发相反启用或确认完成。确切终止响应若迟到，允许只结算旧尝试的终止性，而不更新新代次观测或成功收据。

固定版本的[管理审计](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/log.go#L229)没有请求 ID，且[请求 ID 中间件](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/middleware/request-id.go)由服务端生成 ID。因此不按时间/名字相近的审计记录认领某次未知写。需要服务端已重启等额外终止证据的场景保持 unknown；此轮不自动重启服务或增设“点击重试就认为已终止”的入口。

Store 函数契约：

| 函数 | 前置与步骤 | 后置/失败边界 |
| --- | --- | --- |
| load_work / load_group_work | 一致读事务加载核心实体、控制行、执行行及同组所有未退休成员（含退役请求）；材料只取当前来源设置，不复制全库 key | 得到新鲜代次和完整 barrier 集；无意图记录的目录项仍 unmanaged |
| prepare_execution | 重新核对 member/source/group/execution；动作来自当前计划、没有冲突在途写；创建要求 Never 且无 channel | 同事务递增 execution、插入 prepared，创建标 May；返回私有票据；失败零发送 |
| settle_execution | 票据存在且字段匹配；区分未发送/完整响应/不确定 | 旧票可更新自身终止状态，不确认新意图；明确未发送创建可回到 Never，unknown 创建不回退；消费标记核对授权 epoch |
| bind_created | 稳定名称完整唯一匹配及身份相符，过去确有创建意图 | ID 单调绑定且永不改认；旧创建结果只绑定身份，当前 gate 重新执行；冲突拒绝 |
| confirm_member | 最新代次、最新 execution 的实际观测，重新调用计划检查确为 Confirm/RetireAbsent，且无不确定写 | 逐目标保存匹配代次收据、applied；全部目标确认才 succeeded；最后一个退役成员确认后才归档来源；旧成功收据不变 |
| defer / retry_operation / recover_pending | 安全错误、当前目标、有限重试；unknown 创建保持 May | 暂时失败保留 dirty 和下一时刻；显式重试不重放 superseded 意图、不清未知写；重开不复用旧观测 |

worker 独占 CatalogStore 的外部写入口，通过短互斥区串行事务；网络等待不持数据库锁。提交新意图前使发布 fence 失效，完成或失败重载后发布新的偶数 epoch；最终 guard 仅比较原子 epoch 与许可/观测时间，不访问数据库。先以单控制执行通道串行管理写及其回读，背景管理观察仍受共享预算约束，避免同组相反动作并行。用户控制提交可在网络等待期间落库并使旧 guard 失效。

每轮取至多 64 个到期 dirty 成员；优先停止/降低，再创建/字段同步，最后提升/启用。每次只执行一个计划动作并在下一步重新装配；组观测可以一次完整分页查询共享，不能每个来源重复采集 quota。完整响应后仍重新读实际状态；观察带请求发起时的 execution 和目标代次，过期读不发布。一次轮次的普通读/目标写最多三次，1/2 秒退避；仍失败则延期 30 秒，不忙轮询。401 单飞仍属于传输层。新 worker 仅通过显式库入口运行，不改 up/run 或部署。

正确性：持久 prepared 先于准入消除崩溃后重复创建；目标与执行双重代次隔离旧结果；启用授权 epoch 隔离普通更新和新显式恢复；逐成员收据加冻结 expected_targets 证明完整确认；未决写不被新意图覆盖，避免未知相反写被一次读取掩盖。验证需覆盖 v2→v3 保真/回滚、准备后重开、迟到/重复/乱序回执、授权跨普通更新、完整暂停/退休、真实 HTTP fake 与独立原生实例，而非只测试类型构造。

### 持久日志小步边界与函数论证

本小步仅实现 v3 迁移、load_work/load_group_work、prepare_execution、settle_execution、recover_executions，以及显式授权的持久化。bind_created、confirm_member、重试入口、传输交付证据生产者和 worker 后续实现；本小步不能将任何非零操作置为 succeeded。ExecutionOutcome 只由可信执行器提供，没有 Deserialize，尚未连接真实 HTTP。

- load_work 在一致读事务按成员 PK 读取来源/组/目标、执行序号和未决状态，不读 source_secrets；group_work 查询完整未退休集合，含退役请求。每轮处理一个成员，最多目录容量，遍历终止。缺实体/私有材料无效返回安全错误，不产空成功。verify_peers 对影响路由的动作将调用方屏障集合与数据库完整集合逐项核对身份、代次、目标和未决状态，防止内部装配漏项；不要求停止动作等待无关 peer。
- prepare_execution 先核对四重 revision，再以当前快照重新调用纯计划器；只有 Create/Fields/Enable/Disable 可保存，其余返回 not_ready 或计划错误。事务内更新 execution、插入 prepared、创建标 May、操作置 applying（同操作其他成员未知时仍 unknown），提交才返回票据。创建记录保存当前 priority（已禁用，不产生路由；启用前仍经过 standby/屏障）、模型及身份，不保存 key。所有数据库错误回滚；未知提交使句柄不可复用。因此“拿到票据”蕴含发送意图已提交，但仍须 worker 最终 fence。
- WorkTicket 绑定 catalog_instance_id、member/sequence、目标三重代次、epoch 和效果，不能跨目录错投。settle_execution 先比对整张票，再处理重复（零写）、冲突终态（拒绝）、prepared/uncertain→证据状态。结算及重开恢复也递增 execution，使终止证据之前发起的 GET 失效；因此序号可有间隙，不是尝试计数。明确 NotSent 只有在无其他可能创建记录且未绑定时才恢复 Never。迟到成功只消费相同 epoch 的 Enable，不写 observed/applied/收据；目标三重代次不再匹配时也不改新操作阶段。mark_operations 检查同一操作全部成员的 uncertain，不被另一成员 ACK 覆盖。循环/SQL 范围由成员或该操作目标限制，历史记录按索引查找。
- recover_executions 在发布 Store 前同事务把 prepared 转 uncertain、dirty 置位、保留暂停与创建状态；仅对这些成员推进 execution，重复重开无新推进。verify_execution 检查侧表完整、代次上界、唯一 prepared、效果白名单和状态/错误组合；流式校验历史行，不一次加载全部历史。迁移复用同一校验，失败不发布部分升级的 Store。协议语义错误不含原始材料。
- submit_operation 只有显式 enabled/来源恢复增加授权 epoch，普通 priority/models 保留；溢出与成员 fan-out 均同事务回滚。GLM 继承已消费授权时 preserve；若成功回执晚于普通更新，则通过授权起始 revision 区分旧授权与新显式恢复，仍不重新启用。新显式恢复 epoch 不同，旧回执不能消费它。上述规则不改变火山恢复规则或 GLM 额度算法。

### 绑定和完成确认

本步落实 P14/P15 的身份绑定、逐目标回读确认，以及 P16 的持久延期/显式重试/有限工作读取。仍不接生产 up/run；HTTP 交付证据与 worker 在后续接线。

创建是未决写的一项有条件特例，不改变一般字段/状态未知写的规则。固定版本 AddChannel 的 single 模式只产生一个渠道，BatchInsertChannels 在同一事务内创建渠道和能力记录；提交后 handler 只重置代理缓存、写审计和返回响应，不再次写渠道。因此，在同一已核实实例、单写者和稳定唯一关联名的前提下，完整唯一匹配且新鲜的对象证明该次创建效果已经持久化。依据：[AddChannel](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/channel.go#L563)、[BatchInsertChannels](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/channel.go)。这是效果已完成的推论，不伪造收到 HTTP 响应。

bind_created 复用 member_control.create_state=resolved 和唯一 channel_id 保存上述证据；尚未结算的 prepared 创建在同事务转 uncertain，表示交付仍未知而非伪造 responded，也释放唯一 prepared 槽供下一动作使用。仅已完成绑定的 create 尝试不再参与 pending_write；其他 unknown/prepared 不豁免。已物化创建不能接受矛盾的 NotSent 结算。不新增数据库版本或凭据副本。0 匹配保持 unknown；多匹配 ambiguous；全部明确 NotSent 的尝试不能认领同名对象。绑定不写 observed/applied/确认收据，旧创建结果只能登记不可变身份，随后必须按最新暂停/退役目标重新回读。

| 函数 | 前置与分支 | 后置及论证 |
| --- | --- | --- |
| pending_creation | 按成员 PK 查询且恰有一个非 NotSent 创建记录，重建内部 WorkTicket，校验固定效果与实例命名空间；无/多候选分别拒绝 | 重开可查创建，不发第二次 POST；有限索引查询不加载全库材料 |
| bind_created | 比对整张持久票、当前稳定身份、有效 BackendReadiness；调用 resolve_created 检查完整唯一集合，再核对匹配对象时效；冲突 ID/属性拒绝 | 同事务绑定 ID、Resolved、created_disabled、dirty、推进 execution；重复同 ID 零写；从未改认既有 ID，任何错误整笔回滚 |
| confirm_member | 一致事务重新装配并比较四重 revision；重新计划仅接受 Confirm/RetireAbsent，无未决写；有渠道必须新鲜身份匹配观测 | 更新当前 observed/applied，按匹配成员 revision 仅首次保存安全摘要收据；不会用 ACK/批量 changed 计数；退役确认保留物理 ID，源的最后成员退休才归档 |
| refresh_operations | 读取未终结操作的冻结目标集合；校验数量/当前 revision，按真实收据集合投影 | 旧 succeeded/superseded 不变；其余依次为代次失效→superseded、全确认→succeeded、unknown、终端错误→failed、部分确认→partial、prepared→applying、指定 waiting/queued。循环受目标数上限，不能通过丢行得到成功 |
| defer_member | 比较四重 revision，验证固定安全错误码与时间；失败不撤销原意图 | 临时错误保留 dirty，next_attempt 至少 now+30 或更晚 Retry-After；认证/漂移/歧义/明确拒绝等需人工处理则 failed 且停止自动尝试；不清 unknown |
| retry_operation | 原幂等 ID，先读历史；成功返回原收据，superseded 或目标代次不符拒绝；只重排未确认目标 | 不新增操作、不清暂停/耗尽/启用消费、不重发未知创建；持久 queued/unknown/partial，由 worker 再观察；认证显式重试仍由传输层执行 |
| due_members | 合法 UTC 时刻，读取 dirty 且到期未退休成员，最多 64 个 | 停止/退役优先，其次已知降低，再普通任务；仅 ID，无全库 key；全部延期返回空，由 worker 计算等待而非忙轮询 |

确认不会重新写已存在的历史成功收据；同一 member revision 重复回读不增加 confirmed。未完成操作的某成员如果再次准备写，该成员先前的临时确认同事务失效，必须重新回读；否则其新 ACK 可能错误地凑齐全集。此规则不删除 succeeded/superseded 历史，也不重开 GLM 已消费的启用授权。当前观察可以反映后来的原生状态，但成功收据只陈述曾经完成。GLM Enabled 在实际 status=1 被回读满足时消费当前授权，即使没有发 Enable（例如原本已开启）；普通后续更新仍 preserve。若同源多个组只确认一部分，其操作不能成功/归档；错误或回滚点不留下半张成功收据。source/member 退休、applied、收据和操作阶段同事务。

事前测试：唯一/缺失/歧义/属性漂移/已占用 ID、旧创建在暂停/退役后绑定、崩溃后创建对账；过期各维代次与观测时间、字段不符、unknown/ACK 不确认；同源两组暂停/退役的随机回读顺序与重复、部分失败/重开/重试、source 归档门槛、GLM/火山状态所有权。至少 128 cases 的 R3/R4/R5/R6/R11 组合性质；定向删除完整集合/代次/未知写/收据全集检查必须使对应测试失败。

实现补充：一般未决写和操作阶段使用同一个 PENDING_EFFECT_SQL；terminal_error 逐目标读取错误并复用 Rust terminal 分类，不重复维护 SQL 错误码集合。recover_executions 将 prepared 转未知后复用操作投影，因此已绑定的创建不会在重开后伪装成尚未物化。上述 helper 只遍历当前成员或操作有限目标，不复制材料，不产生网络副作用。

隔离原生测试额外校验固定构建 SHA-256，因该 rc.20/6ce7305 构建的 `/api/status` 实报 v0.0.0，测试用已核实构建身份提供 canonical version。生产 BackendReadiness 的严格版本检查未放宽；真实版本/构建身份及实例核验 producer 仍待实现。该已知差异必须在生产预检细化中显式处理，不能按“相同 URL”或未核实版本默认 ready。

### HTTP 交付证据

ManagementError 增加固定 DeliveryEvidence，不保存原始错误链。execute 的参数校验/最终门禁及 reqwest 明确 connect/builder 失败为 NotSent；发送后超时/断连为 Uncertain。除已核实认证拒绝 401 外，错误响应也完整有界读取，429 在收到头时立即共享冷却，随后正文失败仍保留 HTTP 状态和 Retry-After。只有完整 JSON object 且 success 为 bool 才可识别为应用回执；非 2xx 即使包体 success=true 也拒绝，不能把代理的纯文本 502/504 或 malformed 200 当作写已经终结。

ManagementResponse 的 parse/envelope 错误保持同一证据口径；写 InvalidResponse 仍统一为 transport_unknown。ManagedChannels::write_channel_fields 的前置 GET、身份解析及局部 patch 若失败，一律将本次 PUT 证据标 NotSent；GET 本身可能收不到响应，不代表 PUT 已经发送。没有字段差异时返回 false，执行器可按未写而重新观察，不生成虚假写成功收据。

正确性：发送准入之前不可能产生业务写；connect/builder 分类来自锁定 reqwest 的明确错误类型，其他网络错误保守未知。完整应用回执只证明受支持同步接口的应用响应，不证明目标已满足；所有成功仍经回读。字段写的 GET 前置路径明确置 NotSent，避免把只读故障变成永远无法收敛的未知写。队列/login/401 重放共用既有预算，不增加无界重试。测试覆盖真实 HTTP 前置 GET/PUT 分支、401/429/5xx/非法包体、断连/慢正文、连接拒绝和随机成功标记，且直接检查秘密不进入诊断。

### 执行器组装

ReconcileWorker 独占 CatalogStore 和观测缓存，借用同一个 Arc<NewApiClient>，不新建第二份管理预算。Core 的同步 mutex 只覆盖目录事务和短快照装配；单独异步 lane 串行一轮控制执行，不持 Core 锁等待网络。控制入口用 64 槽 try-acquire，满时在意图受理前返回 queue_full。组/来源策略与 BackendReadiness 仍是可信预检/策略生产者的进程内输入，不能从前端 JSON 接收。此执行器不生成供应商策略或组注册证明，不在现有 up/run 启动。

每次会改变目录/策略/预检/观测的 Core mutation，先在同一锁下核对可用性和可选 expected epoch，再将原子 publication epoch 改为奇数（拒绝旧 gate），提交/失败回滚后发布新的偶数；未知提交保持奇数。计数耗尽停写，不允许回绕形成 ABA。HTTP guard 只读 epoch、停止信号与时钟，验证 backend/permit/必要观测的截止时间，绝不读取数据库或锁 Core。HTTP 返回后再次核对原目标和 publication；旧写可结算原日志，旧 GET 不发布为新观测。

| 入口/步骤 | 分支与后置 |
| --- | --- |
| new / work / operation / snapshot | new 接管 Store 所有权，初始 backend 不就绪；只读接口不改意图。构造不发 HTTP |
| submit / retry / 来源组成员创建 | 命令槽→Core mutation→既有 Store 事务，通知后台；retry 只按原 ID 重排，认证显式重试调用独立登录恢复，不 bootstrap/setup |
| publish_backend / publish_route | 仅可信组件调用；route 核当前来源/组 generation，保存源级停用 gate 与该成员 permit；所有组共享同源 gate。新输入使旧发送 fence 失效；GLM 不接受供应商额度停用 gate |
| assemble | 一致加载当前成员及完整同组可能路由集，接上 revision 匹配的缓存观测和可信输入，调用纯计划；不复制全库凭据 |
| observe_group / lookup_create | 共享完整/覆盖扫描；观察按请求开始时的 revision 盖章，epoch 改变则丢弃；Lookup 只按持久创建票绑定。大扫描后匹配对象过期时重新 GET 校验，不把时间戳刷新成“新观察” |
| execute_write | 重新准备并提交票据→捕获新 publication→受预算/fence 的 HTTP→按证据结算。仅 NotSent 的暂时错误允许最多三次、1/2 秒退避；已可能执行的写不盲重发；字段 no-op 记未写并继续回读 |
| reconcile_once | 最多 64 个到期成员，每成员一项计划动作。Observe/写/绑定后保留 dirty，由后续动作回读；Confirm 才完成。每轮 lane 唯一；调用者断开不取消已受理轮次，进程崩溃仍由持久 prepared 恢复 |
| run | 外部显式调用的后台循环，按最早到期时间或通知唤醒，最短 1 秒防忙循环；停止信号拒绝后续准入并等待当前已发请求结算。尚未接 CLI/服务启动 |

普通读取同样最多三次重试，退避后复核 fence；长 Retry-After 交由持久下一时刻，不截短共享冷却。来源/组新意图使旧轮次停止处理旧 generation，不能将旧错误延期写到新操作。依赖其他组成员的降低/禁用时，先刷新陈旧 peer 观察；确有同轮可执行阻挡项则下一轮再检查，其他等待按持久延期处理。所有遍历由目录/64 个到期成员/三次重试限制，后台不忙轮询。

正确性：mutation 前奇数 fence 消除 DB 已改而旧票仍获准的窗口，短锁与独立 lane 避免持锁网络等待及同组写乱序；Store 再次计划/回执代次检查负责最终本地约束，异步结果不能直接写 applied。Core 缓存以实际请求 revision 发布，epoch 拒收陈旧 GET；创建原始 key 仅在单次内部 HTTP 组包持有。初始化 readiness=false、permit 缺失拒绝启用/提升，后续生产者与部署绑定未验证时不会自动接管流量。

### 执行器审核修正契约

Core.input_epoch 记录最近一次成功外部发布所对应的偶数 publication 值，不另建计数器。submit、有效 retry、目录创建及可信预检/策略输入成功后更新它；内部观察、准备、结算和延期只推进 publication。确定失败回滚不推进 input_epoch，未知提交使 Store 不可用并保持奇数 fence。这样旧 HTTP 错误不能覆盖后来成功发布的立即执行状态，内部自己的回读/结算又不会让合法延期失效。defer 在同一 Core 锁内核对 input_epoch 和当前目标三重 revision，再使用当前 execution revision 执行持久延期。发送票的 gate epoch 固定为准备前 epoch+2，不能解锁后再读取可能属于新命令的 epoch。

观测缓存为私有 Found(StampedObservation) 或 Unavailable{revision, code, checked_at}。不可用证据只在四重 revision 匹配且年龄 0–30 秒内有效；完整组扫描缺失 ID、身份漂移或过期时必须移除旧 Found，不能继续确认旧正结果。正常轮询复用新鲜负结果以免热循环。显式 retry 成功重排非成功历史操作后清空正/负缓存，强制重新观察远端修复；不清未知写、启用消费或历史收据。清空全缓存是有界的简单恢复动作，只发生于人工重试；成功历史重试不清缓存、不重登。

| helper | 前置、步骤及后置论证 |
| --- | --- |
| record_observed | 调用方先核对身份/时效；输入不超过目录上限、ID 唯一，status/priority 同时有值或同时为空且值域合法。事务内逐项比较 member/source/group/execution revision，只写 observed_status/priority 诊断与排序字段；任何项失败整笔回滚。绝不写 applied、意图或确认收据，不能由观察暗中完成操作 |
| due_members_with_stops | 接收合法时刻及有界 SourceId 停用集；SQL 在 LIMIT 64 之前排序人工停止/退役/共享来源策略停止，再按已观测的降低需求排序。只读 ID，不取 key；避免前 64 个未就绪成员饿死后面的停用任务 |
| wake_pending / next_attempt_at | wake 事务只重排纳管、未退休、非终端错误目标，按来源和 include_confirmed 决定范围，不清错误原因；发布内存输入在 wake 成功后。next 只读最早 dirty 到期时间，后台至少等待 1 秒 |
| can_route / peers_need_observation | 自身缺许可或未就绪先延期，不把另一个同样未就绪成员当成必须立即执行的屏障；只有缺少新鲜正/负证据的绑定 peer 才触发扫描。读取缓存需核对 publication，拒收混合快照 |
| actionable_blocker | 核对 publication 后复用当前 Plan 的组快照和 Core 缓存，单次线性遍历到期 dirty peer，不逐 peer 重载组或 SQLite。只接受纯计划器产生的 Disable 或 priority 降低；诊断调用传空屏障集合，让 Enable/提升必然拒绝，不能把它用作执行许可。真正执行仍重新装配全组并经 Store 准备验证 |
| refresh_creation_observations | 只对完整扫描中稳定名匹配且已过期的对象重新 GET；复用预算和 epoch guard，成功使用真实响应时间而非刷新旧戳。失败经 defer 持久延期（包含 Retry-After），若延期存储失败优先传播 Storage；不立即重扫、不认领身份、不清 unknown |
| catalog_change / accept_background_round | 目录入口保留 CatalogError（包括 DuplicateSource 原 ID）；命令槽满报目录 Busy，其余控制入口报 QueueFull。后台遇 lane QueueFull 只等待重试，不退出；其他致命错误保留 |

读取 5xx 属于暂时故障，最多三次尝试后延期；完整应用拒绝不等于所有 HTTP 状态都终端失败。HTTP 交付判断假设连接到已核实、不会伪造 NewAPI 应用回执的受信管理端；布尔 success 结构不是密码学身份认证，代理任意伪造同形 JSON 不在此故障模型。不可识别的代理回执始终 Unknown。

验证补充：立即修复远端缺失 ID 后 retry 不等 TTL 即重新 GET；旧失败与新发布交错；65 成员窗口的停止优先；互相缺许可的持久延期；缺失 ID 清除正缓存且不热轮询；后台 lane 忙不退出；目录错误保真。record_observed 的代次冲突、批次回滚、零 applied/收据副作用单独验证。这里只修复执行器局部契约，真实预检 producer、R8/R10 整体门禁和部署接线仍不视为完成。

### 限流与隐私防退化验收

R8 在既有桶数学性质之外增加实际传输轨迹：两个独立认证身份共享一个预算，真实回环 HTTP fake 返回 401→登录→重放、429→冷却→显式重试及成功响应。随机请求优先级、身份、间隔和失败序列，至少 128 cases；覆盖登录和每一次重放，不能只统计逻辑请求。测试用手动推进的虚拟时钟和有界驱动避免真实长等待；所有已发送请求必须出现在服务端轨迹，按事件时间独立计算所有子区间的 burst=2、rate=1/s 上界。服务端观察时间和准入时间可能因调度不同，测试须明确区分，不声称物理网络到达硬实时。并发、64 槽边界及发送后不退款仍由既有边界回归验证；新增性质不可替换这些边界。

R10 采用两种互补门禁：允许 Debug 的类型直接放入多种可区分哨兵，检查真正内部类型的输出和实际 OperationView；禁止 Debug/Serialize 的私有 wrapper 以编译期负 trait 断言锁定。不得为了测试而给私有类型添加序列化实现。负断言通过互斥候选推导：未实现 trait 时唯一候选成立；误加 trait 后出现歧义，编译必须失败。定向变异验证误加派生确实触发该断言，不能只 grep 源码或格式化安全外层。

HTTP 轨迹每波四个并发逻辑请求分属两个身份，包含不同响应序列。仅 cfg(test) 的预算事件记录准入时间和冷却开始时的准入序号；与 fake 完整收到的业务/登录次数对照，独立按 Retry-After=2 秒检查其后所有身份的准入都不早于截止。序号区分同一虚拟时刻冷却前已在途的请求，不把旧在途误判为绕过。时间 oracle 仍为准入，而非服务端到达；这里只做数量/路径计数对照，没有宣称跨网络的逐请求时间配对。

本轮不改变生产限流参数、隐私接口或数据库格式；不增加依赖。预检实例身份与 v0.0.0 构建归因仍需单独确定部署绑定方案，不把防退化测试通过当成预检完成。

### 已确认的本地托管预检

用户已明确“1，这是我们之前的架构”，并要求避免冗余设计。本地托管是既有边界，不再作为待选择项。[已撤回] 远程支持选择、另建实例身份登记及数据库 inode 跟踪的候选设计；不据此阻塞开发。复用 boot::NewApiProcess 的 data_dir、binary、port、base_url 与既有 PID 文件，不增加配置项、数据库表或新进程管理框架。

check_local_backend 只读检查当前 Linux 托管进程和管理接口：客户端 URL 必须与受管 URL 一致且是直连回环 HTTP/相同端口；PID 对应的 exe/cwd 与现有 binary/data_dir 相符，监听端口由该 PID 唯一持有。检查前后比较 PID 与启动时刻，避免检查途中换进程。使用现有系统工具 ss（3 秒超时）及有界 /proc/PID/stat 读取；启动时刻依据 [Linux proc 定义](https://docs.kernel.org/filesystems/proc.html)。不读取 NewAPI 数据库内容或建立数据库指纹，数据库恢复/替换仍按受信管理员显式停写、迁移重绑定的既有约束处理，不承诺自动识别原地恢复。

管理检查复用同一 NewApiClient/预算：严格 GET status 的版本、GET option 的原生自动启用关闭、GET channel 首个有限页面的结构/权限。常规版本必须为已支持版本；仅已知 v0.0.0 构建允许通过运行中 exe 的冻结 SHA-256 映射到 canonical version，其他未知版本拒绝。回环管理客户端不使用环境代理，避免“核对本地进程却经代理访问另一实例”；非回环连接的原行为不变。整个检查最多 20 秒，成功许可自开始起有效 60 秒；超时、时间回退、解析/权限失败均不就绪。检查不 bootstrap/setup、不写 option/channel，不生成策略或组权限许可。

| 函数 | 前置、步骤与后置论证 |
| --- | --- |
| configured_version / endpoint_is_loopback | 复用已解析托管配置和 URL，前者为只读 getter；后者仅匹配 localhost/回环 IP，不从域名后缀猜测本机。无 IO/状态变更 |
| read_pid / process_snapshot | 有界读取既有 PID/stat；核对 exe/cwd、端口唯一归属；返回内部 pid/start 元组，不保存命令行、环境或原输出。任一步失败只返回固定 LocalBinding；重读相等只证明本次检查期间的进程一致性 |
| known_legacy_build | 仅 v0.0.0 分支调用；阻塞线程有界流式哈希运行中 exe，与已核实构建比较；不由可改文件名或用户布尔值证明版本。失败/超限拒绝，无写入 |
| check_local_backend | 先检查连接/托管配置，读取 PID，前置进程核对→共享预算的管理读→后置进程/PID核对→有期限 BackendReadiness；失败返回固定原因及不可用许可。版本映射、自动恢复与能力均通过才 ready；所有循环和正文受既有限额约束 |
| refresh_backend / refresh_from | 前者将实际检查 future 交给后者；同 Core publication 先撤销旧 ready 并清观察（不做网络）；保存这次撤销对应的 input_epoch，再 await 实际检查。同锁比较当前 input_epoch 后发布结果、清观察并唤醒 pending；成功外部命令/新预检使旧检查失效，但内部 observe/defer/settle 不阻挡合法检查。复用已有 input_epoch，不增加代次计数器。任何失败保留不可用，不以测试布尔值代替公开入口；私有 future 接缝用于精确复现迟到结果 |

假设仍为单受信本地托管者，无并行配置/数据库替换或原生自动启用写者；检查是有期限的证据而非远端 CAS。普通新增能力不改变 GLM auto_ban/95% 算法。本步注册库入口但不启动旧服务上的新写者，正式接线与切换按 sgm-7。

文件读取/哈希在阻塞线程执行，调用方的 20 秒截止仍生效；小文件打开前拒绝非普通文件，读 PID≤32 字节、stat≤8192 字节。ss 只读其专用管道，最多 1 MiB，3 秒超时或检查结束仅清理该 ss 子进程，绝不向 NewAPI 发信号。运行构建哈希最多读 256 MiB，超时遗留的阻塞任务只做有界只读工作，不发布结果。非 Linux 返回未就绪；这是当前本地运行环境的检查，不扩展跨平台进程管理。

事前验收：本地目标错配/无 PID/错误 PID/不同 cwd/exe/端口冲突、canonical/v0.0.0/未知版本、自动启用 true/缺失/重复、响应错误与失效时间；固定 HTTP 与至少 128 cases 的字段/时间窗口性质；worker 先撤销再发布与迟到结果隔离；实际独立原生进程走真实 producer 而不传 instance_verified=true。所有测试使用假 key、临时目录和随机非生产端口。

### 执行器组合轨迹验收

在现有真实回环 HTTP fake 上增加 R1–R7/R9/R11 的组合性质，不引入新的运行框架。每项至少 128 cases，管理预算保持原值，虚拟时间只驱动 HTTP 排队/回执；SQLite 仍使用真正私有临时库，源/组许可是明确的测试输入，不等价于真实预检或供应商策略。

- 两个火山源分别共享到两个组，优先级相反；另有相同模型名称的 GLM 源保持原生 status=3。随机交换单组首选、更新单源模型、跨组暂停/恢复及重开目录；独立数组从输入命令推进期望 status/priority/models，逐实际渠道核对，未目标组与 GLM 渠道保持不变。相同操作 ID 重放返回原收据且零新增请求；历史成功收据跨后续更新/重开不变。
- 暂停/恢复/退役的一个副本被确定拒绝，另一个成功：先验证不冒充全成功、确认数等于真实成功副本且不能提前归档，再重开/显式重试原操作并验证完整收敛。fake 在真实更新处检查旧首选尚未降低时不能提升另一渠道，以及火山先 standby 后 enable。
- 首选交换强制覆盖两个渠道都在路由、旧首选降低被拒绝的场景：在显式重试前新首选仍为 standby、确认数为零；禁用副本的 priority 更新不误当成已产生路由。随后重试原操作，旧首选降低回读后才提升新首选。
- 降低失败还包含已部分降低到 50、目标为 10、新首选拟升至 100 的情况，不能仅因 50<100 就放行。fake 保留实际成功 GET 返回的 status/priority，并在真实 enable/提升前要求对应回读；仅记录请求实际返回的页，不把尚未返回的对象算已观察。目标来自输入数组，不从计划器复制。暂停命令从独立数组推进，隔离检查覆盖所有 PUT/POST，历史逐目标 confirmed_at/receipt_json 以只读 SQL 比较，不只比 OperationView 计数。
- 创建回执未知后，改变为暂停/退役并重开；完整查找中注入缺失、多匹配或属性漂移，均不能误认或重新 POST。恢复远端原对象、重试后按最新意图绑定/确认，创建名和历史 ID 不复用，始终零启用。
- 旧启用回执延迟期间交错多次暂停/恢复，最终暂停/退役：旧回执只结算旧票，不能完成新目标。另验证一般字段/状态未知写即使远端已匹配、重开并重复重试仍不被 GET 清除；保持 unknown 与未归档，不将创建的物化特例推广到一般写。

回归保留既有真实时钟/取消/预算队列边界；组合测试不以直接改 next_attempt_at 或删除未决记录制造成功。测试的故障注入只改变 fake 的答复/副作用，重开只使用真实 CatalogStore::open。未知写不可收敛时明确断言安全停留，不伪造终止证据。此门禁仍不替代原生协议验收、真实身份/版本预检及最终部署互斥。
