# 渠道协调器与持久控制操作细化设计

子计划：sgm-2-channel-reconcile。日期：2026-10-15。基线：f9f02aa。状态：用户以“继续吧”确认推进，第一批管理传输与渠道接口已验证；目录 v2 与协调器待实现，不授权部署。

## 1 范围

本步把 sgm-1 的来源/组/成员目录接到原生 NewAPI 管理接口，实现禁用建渠道、成员映射、来源全局启停与退役、组内 priority 同步、持久操作回执和失败恢复。对应架构 G1/G6，并为 G2/G3 提供不改变策略的执行层。

事前验收清单见 [expectations](../../../audits/sgm-2-channel-reconcile/expectations.md)，操作回执格式见 [schema](../schemas/control-operation.schema.json)。当前只是设计产物，不是测试通过记录。

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
- control_operations：operation_id PK、kind、target_type/ID、payload_digest、target_revision、phase、error_code、created_at/updated_at。
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
