# SGM6 A2.2 管理业务接口细化

2026-10-19，基线65a4235，分支research/source-groups-token-management。本稿是待确认的函数级方案，不是实现完成。A2.1的解析、安全边界和统一状态发布已验收；本批只补管理业务适配，不接CLI、旧status或现行服务。

## 1 范围与架构对应

实现架构G4–G6的来源/组/成员管理、访问key管理、操作恢复及用量查询。一个访问key只对应一个同类型组，同时支持OpenCode/Claude；来源配置、额度、异常共用，成员渠道/priority按组独立。GLM原95%/临期/固定和火山原状态/重置/撤错规则不改。

不增加组删除/换类型、逐key上游绑定、前端任意priority/status、通用ControlCommand入口、登录/RBAC、HTTP持久任务表、新调度器、第二写库连接或新依赖。fermat正式部署是全新初始化，SGM7才处理运行组合，不迁移旧配置或数据。

本批是一个子计划，内部依次完成wire、资源桥接、目录/控制、token、用量和组合测试；这些实现顺序不扩大业务范围，也不以只完成部分路由宣称A2.2完成。事前预期见[a2-2-expectations](../../../audits/sgm-6-three-page-panel/a2-2-expectations.md)。

## 2 复用点与必要薄入口

| 既有权威 | 本批用法 |
| --- | --- |
| A2.1 request/Response/唯一serve supervisor | 保留16连接、全部安全/framing/Ready截止；业务分发追加在授权和完整body之后；state仍只读原cache |
| ReconcileWorker create_source/create_group/plan_membership | 原单写者保存目录；HTTP不直接持CatalogStore或NewApiClient |
| submit/operation/retry、C2发现/固定/火山设置 | 继续原operation_id、载荷摘要、revision、发布栅栏及回读语义 |
| SGM5 token_service | 固定配置owner、原生页、本地身份、登记、创建/状态/删除/复制；不新建签发身份 |
| query_usage | 原查询单槽、10秒deadline、取消/interrupt和只读目标；不在HTTP重算SQL或另取两份统计拼图 |
| ModelCatalogClient::discover_bounded | 原3秒connect、10秒总体、1MiB响应、1000模型、禁止重定向的worker专用client |

必要增量只有：worker只读dry_run与standby_priority标量；创建前发现薄入口；按凭据查重复来源的只读入口；安全SourceModelsView；正常关闭用的wait_usage_idle。standby选择标量getter，不再同时新增另一套成员创建包装。

两个getter固定为is_dry_run(&self)->bool、standby_priority(&self)->i64，直接读取原不可变policy_config；trivial，无锁/网络/写入，不对外序列化Config。现有Scan枚举仅开放crate内as_str访问，存储字符串与业务类型不变。

源模型读取在catalog内部复用policy::source_record，同次读取revision和settings中的models，不查source_secrets、不经read_source_material。重复来源查找只对本次提交的SecretString计算既有fingerprint，查询原唯一索引；不返回fingerprint/凭据，也不建立索引副本。

## 3 请求和公开类型

### 3.1 共同规则

路径前缀均为/api/admin/v1，大小写精确，不接受尾斜杠别名。H1负责HTTP字节和同源检查；H2只接收完整UTF-8消息。请求不先解析成可覆盖重复键的serde_json::Value，逐DTO拒绝未知字段和任意层重复字段；不得对内部ControlCommand/SourceDraft做通用Deserialize。

所有业务POST/PUT/DELETE body均为JSON对象，必带schema_version=1；即使没有其他字段也发送该对象。缺字段、错误类型、未知字段/枚举、重复字段、版本不等于1、尾随第二JSON和非法JSON为400 bad_request，不回显serde错误。已知类型的语义值域/ID溢出/不合法URL等为422 invalid_input；wire DTO的ID先按字符串读取，再调用原ID解析规则。JSON Schema验证结构和值域，不替代原始重复键、数字词法、UTF-8字节数、trim后长度和跨字段关系检查。

ID都是无前导0的正十进制字符串，数值不超过i64::MAX；分别解析SourceId/GroupId/MembershipId/ManagedTokenId和原生token ID。revision用JSON整数1..JSON_SAFE；火山首次设置的runtime_revision允许0，撤错必须>=1。时间默认秒，唯A1原字段与响应年龄按其原毫秒定义；不自动转换客户端的单位。operation_id仍为16–64字节，首字符字母数字，其余只允许字母数字/下划线/连字符。

只有GET /tokens接受after，GET /native-tokens接受owner_scope与page；其余路由query必须为None，空问号也拒400。两个分页query逐键解析，不允许重复、未知、空键/值，且不接受%或+编码（值仅数字或固定ASCII枚举），不做lossy解码。after若提供必须合法ID；owner_scope必需，page省略为1、范围1..100；本地身份页固定100，不接收limit/page_size。

### 3.2 来源创建

CreateSourceRequest字段：schema_version、name、credential、settings必需；note省略为""；model_selection省略为{"mode":"discover"}。所有嵌套对象拒未知/重复字段，输入类型不派生Debug/Serialize。

settings是专用输入DTO而不是直接开放内部SourceSettings：
- type=glm：upstream与quota必需；不接受policy。
- type=volcano：upstream必需，policy省略用原VolcanoPolicy默认值；不接受quota。
- upstream包含channel_type、base_url、model_discovery；后者省略或null只在manual模式允许。model_discovery含url和auth，auth省略沿现有bearer；可显式authorization_raw/x_api_key，不按来源类型猜鉴权。base_url不推断models URL。
- quota含quota_url，global_headers/key_headers省略为空；每组至多64个{key,value}，保持原HeaderName/HeaderValue检查。输入URL只http/https、无用户名/密码/fragment；base/models URL不得query，quota_url可以query。完整URL最多8192 UTF-8字节，header的单个名称/值和credential都仍受整个1MiB body硬上限。
- name按原规则trim后1..128字符，note<=2048字符且无NUL；credential非空、首尾无空白且无CR/LF/NUL。原业务验证仍在store执行，不让前端校验代替它。
- policy字段沿原daily_token_limit(null或1..JSON_SAFE)、failure_threshold(1..100)、reset_minute(0..1439)、utc_offset_minutes(-720..840)。省略整个policy时默认null/3/60/480；提供policy时四字段齐全。没有临期/过期tag。

model_selection只有三种：
1. discover：要求model_discovery；本次相同配置/凭据真实发现，成功且满足原catalog模型合法域才创建。失败不落来源。
2. manual：manual_models必需且1..1000；不发发现请求，明确手填。
3. discover_or_manual：model_discovery与manual_models均必需；优先真实发现，只有失败才用这次明确提供的手填列表。成功结果权威，不拼接fallback；返回model_origin=fallback，不能冒充discovered。

手填与发现结果都做原trim/去重/非空/无逗号/NUL/至多1000校验；发现客户端本身不保证逗号/NUL合法，适配必须再检查。没有预览token、占位模型或先创建后补模型的中间记录。创建前先验校验并查凭据唯一性，已有来源立即409 duplicate_source带原SourceId，避免201丢失后再次依赖已失效的发现服务；提交时仍由原唯一约束处理并发竞争。

返回201 SourceCreated：{schema_version:1,source:原SourceView,model_origin:discovered|manual|fallback}。只表示目录保存，没有渠道/可路由保证；不把model_origin写入新的业务表。重复凭据不覆盖旧配置，含退休来源也不隐式恢复。目录创建没有operation_id。

### 3.3 复用和新增响应

既有SourceView/GroupView/MembershipView、OperationView wire3、TokenIdentityView、NativeTokenView、GroupCheckReport和GroupReferences保持原字段/枚举。GroupCreated/MemberPlanned分别包装{schema_version:1,group|membership:原视图}；planned后须从state取得member control_revision，再显式EnsureMember，不能把applied_revision当控制版本。

新增共享白名单：
- SourceModelsView：schema_version=1、source_id、source_revision、models(完整去重非空列表)。允许读取退休来源的历史模型，但不是修改/路由授权。
- TokenIdentityPage：schema_version=1、items、after(null或输入游标)、next_after；不足100时next_after=null，恰100时为最后ID，只是继续游标，不证明下一页非空；不伪造total。
- NativeTokenPageView：schema_version=1、owner_scope、page、page_size=100、total、items(原NativeTokenView)。各owner各页独立时刻，缺页或失败不表示删除，不拼原子完整列表。
- TokenReveal：schema_version=1、token_id(本地ID)、key。只在复制分支临时编码；RevealedToken本身不增通用Serialize/Debug，结果不进入cache/收据/日志。
- UsageSnapshotView见§3.4。以上类型供HTTP和后续B页面消费，归属同步到architecture。

请求/响应schema分别为panel-admin-requests.schema.json、panel-admin-responses.schema.json；响应中的旧schema完整嵌入并机械比较原文件，避免同名字段独立演化。原panel-http-error及panel-snapshot schema不变。

模块私有AdminFailure只承载H1错误码/固定Allow或AdminError，统一交error_response转换，不派生公共Serialize；它不是新增错误协议。AdminReply只列上述白名单成功变体，不提供Raw(Value)或任意worker材料逃生口；敏感复制单独编码。

### 3.4 用量wire

POST /usage/query是受同源保护的只读动作，dry_run允许。请求必需from、to、utc_offset_minutes、grain(hour/day)，半开秒区间0<=from<to<=JSON_SAFE且长度<=31天；filter省略为四维any。

filter.source_id/group_id/native_token_id/model各自为{mode:any}、{mode:unknown}或{mode:exact,value:...}，省略该维等同any。exact模型复用原valid_label（非空、最多256 UTF-8字节、无Unicode控制字符）；不把unknown和any都编码为null。这里不trim模型标签，不与catalog模型规则（禁逗号/NUL并trim）或token名称规则（另要求trim不变）混用。native_token_id明确是NewAPI原生ID，不接受本地ManagedTokenId冒充；尚无native ID的pending key不能改成any查询。

UsageSnapshotView={schema_version:1,query,ledger,usage}：
- query回显已规范化的from/to/时区/grain及全部四个filter。
- ledger含revision、initial_requested_from和原两个ScanProgress的安全字段：lane、generation、window、next_page、phase、last_observed_at、fault。revision与next_page均>=1；scans固定recent/history顺序，idle必须generation=0、window=null、next_page=1、观察与fault=null，非idle沿原phase/generation/window/故障关系。时间均秒，window保留null或半开区间。不返回instance/scan ticket/SQL/原始事件。Scan枚举仅提升原as_str的crate内可见性供wire复用，不修改存储枚举。
- usage=null当且仅当原ledger未初始化（initial_requested_from=null），不填0；查询失败走错误而非成功null。已初始化时保留total、models、tokens、buckets；每个Totals为input/output/total/consumption_rows，total仅用原安全加法相加，不进行金额或积分换算。
- models项为{model:null|字符串,usage:Totals}；tokens项为{native_token_id:null|正ID字符串,usage:Totals}；bucket为{from,to,total,models}。所有项从同一个UsageSnapshot映射，保留unknown项、顺序及零桶，不拼两份边际统计。
- scans只表示已观测采集进展，scanned不证明供应商日志绝对完整；不新增complete=true或推测“无缺口”。故障/未初始化/迟到补采等按原状态展示。

## 4 逐路由矩阵

下面路径省略共同前缀。字段列省略必需的schema_version；op表示operation_id，rev表示expected_revision。所有路径ID仍单独解析，不允许body覆盖路径目标。

| 方法 路径 | 输入 | callee与输出 | dry_run |
| --- | --- | --- | --- |
| GET /state | 无query/body | 原A2.1 bytes，200或503 | 允许 |
| POST /sources | §3.2 | 重复检查→本次发现/手填→create_source；201 SourceCreated | 拒 |
| GET /sources/{s}/models | 无query | source_models；200 SourceModelsView | 允许 |
| PUT /sources/{s}/models | op,rev,models | submit(SyncSourceModels)；202 OperationView | 拒 |
| POST /sources/{s}/models/discover | op,rev | discover_source_models；202 OperationView | 拒 |
| PUT /sources/{s}/paused | op,rev,paused | submit(SetSourcePaused)；202 | 拒 |
| POST /sources/{s}/retire | op,rev | submit(RetireSource)；202 | 拒 |
| POST /sources/{s}/clear-error | op,rev,expected_runtime_revision>=1 | submit(ClearVolcanoError)；202 | 拒 |
| PUT /sources/{s}/volcano-policy | op,rev,expected_runtime_revision>=0,policy | submit(UpdateVolcanoPolicy)；202 | 拒 |
| POST /groups | name,newapi_group,type | create_group；201 GroupCreated，ready不默认true | 拒 |
| POST /groups/{g}/check | rev,register | check_group；200原GroupCheckReport | 拒 |
| GET /groups/{g}/references | 无query | group_references；200原报告 | 允许 |
| PUT /groups/{g}/pin | op,rev,expected_policy_revision,membership_id为null或ID | submit(SetGroupPin)；202 | 拒 |
| POST /memberships | source_id,group_id | plan_membership，priority取worker当前standby标量；201 MemberPlanned | 拒 |
| POST /memberships/{m}/ensure | op,rev | submit(EnsureMember)，status固定Enabled；202，原worker仍验真实准入 | 拒 |
| POST /memberships/{m}/retire | op,rev | submit(RetireMember)；202 | 拒 |
| GET /operations/{op} | 无query | operation；200原OperationView | 允许 |
| POST /operations/{op}/retry | 仅版本 | retry同一ID；200原OperationView | 拒 |
| GET /tokens | after可选 | token_identities；200 TokenIdentityPage | 允许 |
| POST /tokens | op,group_id,expected_group_revision,display_name,expires_at为null或秒 | create_token；202，不返回明文，不接收owner | 拒 |
| GET /tokens/{t} | 无query | token_identity；200原TokenIdentityView | 允许 |
| PUT /tokens/{t}/enabled | op,rev,enabled | set_token_enabled；202 | 拒 |
| DELETE /tokens/{t} | op,rev | delete_token；202，保留历史本地身份 | 拒 |
| POST /tokens/{t}/reveal | rev | reveal_token；200 TokenReveal，仅显式动作 | 拒 |
| GET /native-tokens | owner_scope必需,page可选 | list_tokens；200 NativeTokenPageView | 允许 |
| POST /tokens/register | owner_scope,native_token_id,display_name | register_token；200原TokenIdentityView，不宣称原生创建 | 拒 |
| POST /usage/query | §3.4 | query_usage→同份wire；200 | 允许 |

owner_scope只能issuer/legacy_admin且由worker映射预配置身份，不能传user_id/principal/client。新token不提供owner选项，原worker固定issuer，不可用就拒绝，不回退管理员。expires_at初次提交必须为未来，重放沿原持久命令语义，HTTP不在幂等判断前新增当前时刻检查。

所有持久提交即使返回已有Succeeded也固定202并原样返回收据，HTTP不重造phase。GET操作、显式retry固定200但不把phase=unknown/failed写成成功；C2零targets的Succeeded仅表示设置已保存。check_group的ready=false也可200，必须保留error_code；register=false同样可能保存ready，不能混入自动刷新。

未知路径404；业务路径先识别模板再验方法，不支持方法405并返回精确Allow（GET、POST、PUT、DELETE或固定组合），支持的方法再验ID/query。state保留H1既有query/方法优先级。HEAD从不执行业务，只发对应错误头、不发body。Response的allow_get改为内部固定Allow字段，原state仍Allow: GET；不反射请求方法。路由和字段均不接受任意原生patch。

## 5 错误与恢复契约

A2.1的15种边界错误仍用原HttpErrorBody。业务错误使用独立AdminError：
{schema_version:1,error:固定字符串,existing_source_id:null|SourceId}。
existing_source_id仅duplicate_source必须非null，其余一律null。客户端不能把额外字段当原H1 schema已扩展；两个错误schema分别验证。

ControlErrorCode直接复用并Serialize，不抄另一份控制枚举；新的目录/分析/适配错误使用单一私有枚举。完整业务code集合及映射：
- 422：invalid_input、type_mismatch、invalid_query、limit_exceeded、arithmetic_overflow。
- 404：not_found。
- 409：duplicate_source、duplicate_group、duplicate_membership、retired、channel_conflict、revision_conflict、idempotency_conflict、binding_drift、ambiguous_create。
- 403：dry_run（不是NewAPI认证失败）。
- 429：rate_limited。
- 503：queue_full、not_ready、query_cancelled。
- 502：auth_failed、remote_rejected、invalid_response。
- 504：transport_unknown、query_timeout、result_unconfirmed。
- 500：storage、response_too_large。

Catalog InvalidInput/NotFound/上述重复与类型/退休/ChannelConflict逐项保留；Busy映射queue_full，其余存储/损坏/未知提交/路径/导入/备份类统一storage，绝不输出incomplete_backup/路径或原错误文本。DuplicateSource缺原ID属于内部异常，映射storage而不虚构ID。

Analytics InvalidQuery/LimitExceeded/ArithmeticOverflow/Busy/Cancelled/TimedOut映射上述固定码，Control沿原映射，Ledger错误统一storage。原query_usage对未初始化的正常null结果不是异常，不改成storage或0。

无论500/504还是TCP断开，只要已通过最后worker调用前检查，就可能发生过本地提交或远端调用；错误码不包含“已回滚”承诺。持久请求使用原op查回/同ID重放；短时NotFound不证明在途任务永不会提交。目录按凭据、newapi_group、source/group唯一关系核对；目录重试可返回409而不覆盖旧实体。核验/登记可能已写，复制丢响应不自动重做。HTTP层自动重试为0，既有transport的认证恢复/安全重放与worker恢复规则保持原样。

## 6 有界业务执行和关闭

### 6.1 所有权与背压

保留16个连接和1个A1 publication，不使state占业务槽。新增4个全局业务permit和capacity=4的私有mpsc handoff；permit在发送前try取得，覆盖handoff、Tokio blocking池排队、真实执行、编码和完成结果消费，不能把队列4与执行4算成8个在途任务。Full/Closed即503 queue_full，返回原请求对象后drop，零worker调用，不等待排队空间。

唯一serve supervisor直接持有业务JoinSet，只有它调用spawn_blocking；publisher和业务分派使用serve传入的同一Arc<ReconcileWorker>。闭包用原runtime Handle::block_on调用同步/异步worker并完成有界编码，只返回结果，不能自行写HTTP或cache。没有新持久task ID、任务查询表、业务去重表或第二执行器。现有async token/model/retry方法仍同步取Core，不能直接放回主async执行线程冒充非阻塞。

supervisor优先回收完成项，并只在JoinSet.len<4时取handoff；正常返回把permit随JobOutcome保留至结果消费。panic也须join回收，不能以丢JoinHandle代替停止。结果通过one-shot交给原HTTP等待者，接收者已取消则丢弃完整结果；敏感明文不缓存。业务panic使sender销毁，HTTP等待者在有效期限内收到关闭就回原H1的500/internal；已到期则按504 result_unconfirmed或关socket处理。supervisor只记固定失败并继续drain，不用?提前退出，也不另建task→sender注册表。

运行前提：沿现有Tokio标准blocking池，不人为缩小；原默认是512，DNS和其他worker也会使用该池，不能声称“8线程一定足够”。运行时及IO/timer驱动必须存活至drain结束，Handle::block_on不能独自驱动已停的current_thread runtime。SGM7必须维持此生命周期，不新建每请求runtime。

### 6.2 截止和取消

本地目录/控制提交/普通只读预算10秒；含网络的来源创建、重新发现、retry、组核验、原生列表/登记/复制预算30秒；usage等待预算15秒，内部查询仍10秒。每个等待期限取min(进入分发时刻+预算, connection_end-5秒)，保留最多5秒发送窗口；H1的5/10/5/45秒完全不放宽。

H2取得槽、提交handoff、supervisor spawn及闭包开始前均显式核同一deadline/closing；闭包紧邻worker调用再核one-shot.is_closed。最后检查通过是“可能执行”的边界；之后不因HTTP超时/断开abort业务job，不提前归槽。同步锁/系统调用不可抢占，既有异步网络保留原guard/预算，任务可能比HTTP等待更晚结束。

one-shot关闭只表示本地HTTP等待任务已结束，不证明已实时检测浏览器物理断线。未额外监听socket时，断线可到实际发送才被观察。等待超时在发送窗口内回504 result_unconfirmed，否则按H1只关socket；不伪造202或NotSent。结果恰在截止返回也先显式核期限，不能再靠timeout_at的Ready顺序判成功。

使用持久操作的执行由原worker后台完成，HTTP202不是远端ACK。若HTTP等待已结束而job刚提交，客户端仍按原op查回；新的HTTP请求不能通过换op绕过原未知创建保护。

### 6.3 有序关闭

先置私有closing并停止accept，关闭handoff并丢弃未启动请求；abort并join纯async连接等待者。已经spawn的业务句柄继续逐个join，含尚在blocking池排队的闭包（开始时看到closing即退出）；同时保留原publication句柄直到实际完成。单个panic/错误只记录固定失败，不能中途丢其他任务。

全部业务wrapper与publication结束后，调用worker.wait_usage_idle：仅acquire原query_slots唯一许可后释放，不close semaphore、不持Core/commands等待、不通过已关闭handoff执行。旧query_usage内部timeout可能drop读任务handle，但许可仍归真实读任务持有；barrier证明SQLite读目标/目录guard已退出，不伪称拿到了该内部JoinHandle结果。

此时所有query_usage调用方必须已停止接纳；当前非测试生产调用只由本批HTTP引入，SGM7须保持单一admission/关闭前提。不能只停止listener就等待barrier，却放任已spawn wrapper之后新启动query。正常shutdown没有固定墙钟上限；强制abort/drop整个serve或进程崩溃不承诺drain，不能据此立刻重开目录。

## 7 函数细化与正确性论证

### H2.1 parse_admin_route和decode_admin_request

parse_admin_route(method,path,query) -> Result<Route, AdminFailure>。前置是H1已验证的ASCII target；先区分静态子路径与参数（tokens/register不能误当token ID），未知资源模板404。业务模板先验方法，错误即405固定Allow；再按已支持的方法解析ID/query，state仍沿H1优先级。query缺必需键、重复/未知/空键值、禁止的%/+编码、未知owner_scope为400 H1 bad_request；资源ID/operation_id的字符串语法或数值溢出、after非法、page非整数或超1..100为422业务invalid_input。每条路径最多固定段数，query长度<=8KiB、键<=2，循环每步消费一段/一对。Route只包含白名单强类型参数，不含请求原文。

decode_admin_request(Route, &[u8]) -> Result<AdminRequest, AdminFailure>。从<=1MiB完整UTF-8按路由具体DTO反序列化并检查end；所有嵌套结构拒未知/重复，随后纯验证数字/URL/header/model与kind关联，400/422按§3.1固定。先构造SecretString再丢原body；不派生请求Debug/Serialize、不把错误链透传。适用业务请求按§3默认，GET无额外body。

论证：H1提供唯一消息边界，路由白名单决定唯一DTO；ID/类型不能由body改写，纯验证失败不调用worker。有限段/字节/数组终止；没有外部写或日志副作用。JSON Schema只作机械互证，重复键等由原始解码测试保证。

### H2.2 admit_admin和run_admin_job

admit_admin(request, reply, deadline, gate)先依route语义检查dry_run（来源/组/成员/控制/token写、check(false)、登记及复制均拒；usage POST和普通只读允许），再核截止和closing、try取得同一4槽permit、try_send。失败固定错误并释放全部输入；不会拿业务槽等待HTTP body或重新解析。

supervisor收到handoff后先核closing/截止/receiver存活，失败丢弃而不spawn；通过则JoinSet.spawn_blocking(run_admin_job)。闭包再次检查准入，再Handle::block_on dispatch_worker；每个分支按下面callee契约处理，安全映射后有界编码，返回JobOutcome(reply,response,permit)。one-shot消费失败丢弃响应；任何编码超限不发截断成功。

论证：permit不可复制且跨pending/实际工作移动，HTTP等待者不能归还它；唯一supervisor保留句柄，故超时不增加实际并发。进入worker后的副作用只来自所选callee，不由错误码撤销；panic和正常结果都在同一JoinSet回收。所有枚举match必须穷尽，无任意closure/plugin注册接口。

map_admin_error对Control/Catalog/Analytics按§5逐项转换；Control直接保留共享枚举，Catalog仅DuplicateSource携带已校验原ID，其余清空上下文，未知存储类固定storage。error_response(AdminFailure)->Response只从H1码或AdminError编码，Allow取路由静态常量；转换/编码异常退固定H1 internal。前置为已分类错误，后置恰是两个既定schema之一且没有输入/SQL/供应商原文；有限match终止，无重试、回滚或业务写副作用。

### H2.3 source_model_lookup和discover_candidate_models

worker.source_models(SourceId) -> ControlResult<SourceModelsView>：在同Core保护下让catalog读取同一source_record，按type仅取upstream.models和config_revision；NotFound/损坏沿固定错误，允许退休历史，不从material取凭据。store只读事务没有发布、探针或网络；返回完整models，最终编码超限由HTTP失败处理。论证依原source_record的同记录/有效settings约束，公开字段枚举穷尽，不能带出selector/URL。

worker.source_for_credential(&SecretString) -> CatalogResult<Option<SourceId>>：同Core、store.usable，既有fingerprint算法和唯一索引读取，含退休；未找到为None，DB失败不当None，库中非法ID按损坏/存储错误而不是客户端invalid_input。不能返回指纹或更新最后使用时间。论证依唯一索引，至多一行，零业务写；最终create_source仍验唯一性，前置读不是锁定身份许可。

worker.discover_candidate_models(config,credential) -> ControlResult<Vec<String>>：先以共享验证规则检查URL/auth/credential，不接受重定向；dry_run拒绝，try原commands和model_catalog，调用discover_bounded并检原catalog模型合法域。成功非空完整列表；任何网络/格式失败安全invalid_response，忙queue_full，不新开client或改目录。两个guard随该调用完成释放，不在持有commands时再调用create_source，避免二次占槽。

handle_create_source(dto)：请求纯验证→重复查找（已有即409原ID）→按mode发现/手填，成功发现绝不合并fallback→组装本次同配置/凭据SourceDraft→原worker.create_source→201或精确目录错误。fallback只处理发现失败，不掩盖dry_run/queue_full/Storage/重复等准入与目录错误；没有任何路径先存占位模型。整个job的实际调用不因HTTP等待过期重发。

论证：原目录事务是唯一提交点，失败/取消前未调用create_source则无来源写；提交后丢HTTP结果按唯一键核对。配置/凭据和发现结果由同一owned request携带，没有跨请求预览缓存；并发重复最后由原唯一约束收敛，不覆盖原来源。

### H2.4 handle_catalog和handle_control

handle_catalog分别构造GroupDraft或调用plan_membership(source,group,worker.standby_priority())；不接收ready、priority、channel_id或lifecycle。原store验证类型/退休/容量并提交，成功201包装原视图；DuplicateGroup按newapi_group、DuplicateMembership按source/group从后续state核对。cached state短暂落后不推翻已收到的201。

handle_control逐路由构造固定ControlCommand，字段对应§4且版本类别不替换：来源用config_revision，成员用control_revision，固定同时用group revision/policy_revision，火山额外runtime_revision。EnsureMember的Enabled是显式创建/恢复意图，实际发写继续由原group/来源/permit/策略判定；不允许通用SetGroupPriorities或前端status patch。Discover仅调用专用异步发现入口，不把内部DiscoverSourceModels直接submit。

原submit先用统一operation命名空间查重放/冲突；成功202原OperationView，失败按原code。operation查询/显式retry不换ID；retry可能已写重试状态后认证失败，不能说本次完全没执行。check_group两种register值都按写动作处理，200保存的ready/error原样；group_references只读且deletion_authorized仍false。

论证：HTTP只做类型化映射，旧revision在原事务拒绝，同ID重放不增加逻辑操作；所有成员/跨组关联由原store校验。局部Succeeded/0与远端已收敛不混同。每次只有单个命令/目录动作，没有跨请求事务或HTTP侧补偿写。

### H2.5 handle_tokens和encode_reveal

本地列表/单项直接用token_identities/token_identity；after按原升序ID推进，精确100项时给游标但不假造总数。原生列表只选预配置owner_scope及page，返回原NativeTokenView/分页字段，不把分页缺失写成删除。没有原生观察时间字段，B按本次请求/接收时刻标记刷新时间，不把created_at当观察时间。

register_token先由原owner查原生token并在其frame下登记；成功200原身份，既有身份重放由原store决定。create_token只传op/group/revision/name/expiry，owner由原服务固定或历史重放取得；HTTP不在查重放前额外要求expiry仍在未来。启停/删除沿原身份及expected_revision提交；成功202不把queued/unknown改成已执行，删除历史仍可查询。

reveal_token严格沿原owner/本地revision/原生状态/期限/当前组和实际候选的多次检查；只有需要issuer的原callee路径检查issuer，不增全局issuer前置。仅配置legacy owner时，原允许的legacy管理/复制仍可执行；对应owner不可用或未就绪则沿原规则拒绝。成功只在encode_reveal中把secret.expose()写入专用短时wire，输出后释放，不入cache/操作；取消/丢响应不自动重试。这个POST在dry_run拒绝，避免把敏感副作用当普通只读。

论证：固定owner解析在worker且请求无principal字段，local/native ID各用独立类型；历史身份与原生状态不混为一个原子列表。复制是唯一明文出口，其他分支只序列化原白名单视图；无通用RevealedToken Serialize/Debug。

### H2.6 handle_usage和encode_usage

从规范化DTO构造原UsageQuery::new，依其区间/时区/筛选pre再调用worker.query_usage；不调用repair/采集器，不占管理命令槽。同一个返回值完整映射query、LedgerState和UsageReport；usage None直接null。Totals复用原安全加法，溢出整份失败；native ID转正十进制字符串，未知保留null。循环由模型<=1024、token<=4096、bucket最多745及联合模型项<=65536限定。

编码沿H1 BoundedWriter提取的通用私有encode_bounded，不先无界to_vec；32MiB是业务响应完整编码硬上限，不宣称任何合法最大UsageReport都能装下。合法但转义后超预算也整份response_too_large，用户缩短区间/筛选；不能静默截断/Top-N/丢unknown。原query错误按§5映射，不输出LedgerError文本。

论证：所有分项来自同一数据库快照/UsageReport，映射只改形状/ID编码；分项守恒由原加算及PB互证，不另算第二份查询。有限列表终止，超限没有可发布部分正文。HTTP查询等待结束不取消写连接，实际查询槽释放沿原guard，正常关闭另等usage idle。

### H2.7 respond_admin和shutdown

Response保留H1固定安全headers、字节Content-Length、HEAD不发body和绝对写deadline；新增成功status201/202与逐路由固定Allow，响应不反射header/path/请求正文。取得job结果后先核等待截止，再发送；超时/发送失败不追加第二响应或写入补偿。

正常shutdown按§6.3执行，错误只累积固定码并继续drain。worker.wait_usage_idle()前置是全部HTTP job已join且其他query调用方也已停；await原单槽许可，不持Core/commands，取得后释放。它不改变查询算法、DDL或调度状态。

论证：pure async连接可取消，actual blocking句柄只归supervisor，生命周期覆盖完整任务；读槽barrier覆盖旧查询内部脱离HTTP等待的尾部。强制终止不作相同承诺，SGM7不得在drain前重新打开目录。

## 8 性能 安全与验收

所有新业务响应32MiB上限，state保持原A1容量证明；其他响应单独测完整/超限路径。原生页最多100项且本身有既有上游body上限，模型列表<=1000；用量最大合法域可能因转义超预算而失败，需保留此可见边界。Secret wire最大沿原sk-加16..128字节字符规则，不借普通大响应预算无限返回秘密。

新增两个独立release性能门禁：最大受理body的DTO解析<=1秒、固定大UsageSnapshot完整映射/编码<=2秒；声明输入构成和字节数，不能冒充任意合法域的最坏耗时证明。原29release+4debug全部保留，普通测试只验证结构/容量，不夹墙钟预算。

至少6组各128 cases PBT：路由/DTO非法组合零调用；dry_run与动作分类/旧revision；operation重放与跨ID隔离；模型发现/失败/唯一性不覆盖；用量/unknown映射守恒；取消/排队/完成/关闭轨迹维持实际4槽与句柄所有权。真实TCP另测全部路由、Ready deadline、满槽、丢响应后查回、shutdown期间迟到提交、敏感输出以及原1publisher未被请求触发。

至少12类成功编译变异，包括漏dry_run、错误revision字段、非issuer创建、managed/native ID混用、重复JSON覆盖、query重复覆盖、提前归业务槽、关闭丢句柄、复制进普通缓存、发现成功混fallback、未知用量填0、错误重放换op；不以编译失败/空测试当捕获，恢复后必须重新编译。

机械检查须包含逐路由(method/path/Allow/DTO/callee/status)矩阵、共享旧schema逐字一致、所有新增公开输出schema、枚举/七Step、精确测试清单与禁止空跑、私有类型不可Debug/Serialize、冻结范围。只允许panel_http及必要worker/catalog薄入口和展示枚举访问性增量；旧算法、transport策略、DDL、CLI/status及依赖冻结。默认debug/release、全部独立性能、clippy/build和独审五维度通过才退出。

## 9 分布式契约与设计自检

1. 连接：沿H1单请求短连接，断开不删除持久operation；物理断线不等于已检测本地取消。
2. 超时：H1不放宽，H2按10/30/15秒等待且预留发送预算；worker既有3秒connect/各请求deadline及查询10秒保留，不承诺抢占同步锁。
3. 重试：HTTP服务层0自动重试；所有持久恢复同op，目录唯一性核对；客户端读重试/退避由B细化，写与复制不自动重做。
4. 交付/顺序：每连接至多一次分发，跨连接无FIFO；原单写者/持久摘要处理并发和重放，不宣称网络exactly-once。
5. 失败：入worker后回执丢失可能已提交；读取失败不填零，部分远端执行由原phase/error表达；崩溃恢复沿原操作表，没有HTTP task库。
6. 状态/会话：没有HTTP登录会话，原owner是启动配置；连接重开不续许可/观测时间，不缓存密钥。
7. 背压：16连接、1publication、合计4业务permit、原1查询槽；满载拒绝，actual task结束前不归槽，正常关闭等待资源真正退出。

- [x] 范围/复用、公开与私有数据结构、逐路由和全部错误分支已列。
- [x] 非平凡函数列出callee前提、成功/失败/取消与副作用，有限循环及所有权论证明确。
- [x] 新增共享类型归属、ID/版本/时间、dry_run和目录/持久操作语义分别声明。
- [x] 独立§4.4设计复审及schema静态验证完成：0个未关闭设计阻断，65正例/44负例、6份旧schema规范化一致，见[设计审核](../../../audits/sgm-6-three-page-panel/a2-2-design-audit.md)。
- [ ] 用户确认本细化后才编码；设计审核不代表PBT/全量/部署已完成。
