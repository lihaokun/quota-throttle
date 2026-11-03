# SGM6 A2.1 HTTP边界与统一状态发布

2026-10-19，基线`ea7cf33`，编码前设计，独立§4.4复审通过。实现范围仅为[A2](sgm-6-admin-http-a2.md)的严格本地入口、统一A1发布缓存和GET state；不实现业务写路由、不改旧status/run，不启动在线服务。

## 1 目标 复用与假设

G5由单一发布者和完整安全JSON保证；G6由回环信任边界、无歧义framing、跨源拒绝、资源上界及失效缓存保证。复用Tokio TCP/timeout/Semaphore/JoinSet/watch、serde_json、A1的worker.panel_snapshot与PanelSnapshot schema。没有新依赖、DB表、许可或调度器。

新增`src/panel_http.rs`及其`request.rs`、`publication.rs`和测试文件。main只声明模块，不调用serve或改变现有入口。HTTP模块不获得NewApiClient或CatalogStore，不读配置文件/密钥/生产路径；唯一数据生产接口是已有A1安全投影。

显式假设：一个runtime拥有一个PanelServer，不能同时为同一worker重复启动发布者；调用者必须watch shutdown后join服务，不能用abort/drop代替正常回收；本机和SSH隧道操作者可信，不提供本机用户身份隔离；浏览器遵守Origin/CORS限制，XSS/恶意扩展不在本机访问控制能力内；A1遵守既有输出白名单和固定目录容量。所有wall-clock异常安全失败，不将时钟用于刷新业务许可。

模块协作论证：受限parser只给安全校验完整且唯一的head；安全校验失败不调用数据接口；GET仅clone已完成A1 bytes，故请求数量不影响采集次数。缓存只接受期限内成功结果，失败/迟到不能续旧观察；客户端继续按原观察时间显示，不把HTTP缓存当路由权威。

## 2 数据结构与资源界限

共享输出沿`PanelSnapshot` wire1及原[schema](../schemas/panel-snapshot.schema.json)，不增加JSON字段。新错误输出`HttpErrorBody { schema_version: 1, error: HttpErrorCode }`，无任意message或底层错误链，见[错误schema](../schemas/panel-http-error.schema.json)。类型仅Serialize，不反序列化成内部授权。

私有类型：

- `PanelOrigin`：启动时验证后的唯一http origin/authority，host仅127.0.0.1、localhost或[::1]，端口1..65535；URL默认80端口按标准origin序列化省略。无用户信息、query、fragment、非根path。请求不能改它。
- `RequestHead`：原大小写method token、path、`query: Option<String>`、唯一小写头名到ASCII值、可空content_length；无Debug。无问号是None，空问号是Some("")，不能丢失区别。整个target只能含可见ASCII，path/query的进一步限制见§3。JSON body保留Vec<u8>且不输出日志。
- `PublishedPanel`：完整`Arc<[u8]>`、原captured_at_ms、本次生产开始Instant；不含worker、凭据或可执行闭包。`PanelCache`至多保留一项成功结果，默认None。
- `ActivePublication`：唯一在途blocking任务的JoinHandle、started_at和expired标记，只由服务主循环持有。没有队列、业务generation或持久状态。
- `PanelServer`：独占listener生命周期、固定origin、16连接槽、`Option<ActivePublication>`、cache、5秒interval和shutdown接收端。不Clone；连接任务只持必要共享字段。不另spawn一个持blocking句柄的publication父任务。

| 边界 | 固定值及超限行为 |
| --- | --- |
| request target | 8KiB，414；不含request line其他部分 |
| 完整head | 16KiB，含request line和末尾CRLFCRLF；最多64字段，超限431 |
| body | 1MiB，由Content-Length先检查；超限413 |
| header读取 | accept起绝对5秒，未完整则408并关闭 |
| body读取 | head完成起绝对10秒，受整连接期限约束；不按每个chunk续期 |
| response写入 | 首次写起最多5秒，受整连接期限约束；超时只关闭 |
| 整连接 | accept起45秒，到期关闭，不拼第二个响应 |
| 并发 | 16个已接纳连接，无应用等待队列；满时直接关闭，尚未解析方法，不发送可能违反HEAD规则的正文 |
| 状态生产 | 5秒周期，missed tick Skip；同一时刻一个blocking投影/编码任务 |
| 状态TTL | 本次生产开始起严格小于10秒；不从编码完成或HTTP读取重新计时 |
| 完整JSON | 32MiB，越界停止编码、清cache并503；不截断、不Top-N |

32MiB用于A1有界DTO而非任意JSON：目录1000/100/10000，来源note最多2048字符、name最多128字符，类型/ID/枚举/整数均有限，模型正文不在快照内。按serde_json实际UTF-8/控制字符转义保守分配：每来源连同quota/火山扩展16KiB，每成员1KiB，每组连同固定评估字段2KiB，临期ID总量另计22×10000字节，根结构余量64KiB，合计27,114,336字节，小于32MiB。临期总量依赖A1每个membership仅属一个组、imminent只含本组成员的跨集合不变量，不由schema逐数组上限单独推出。实现前边界夹具须用真实A1合法域的最长转义文本、最大目录、最长ID/数值核实逐项界限，不以短文本性能夹具代替容量证明。连接只clone Arc，不复制整份body；编码失败不返回半个JSON。

## 3 HTTP格式与安全决策

本入口是直连面板使用的受限HTTP/1.1子集，不是通用HTTP代理。格式参考[RFC9112](https://www.rfc-editor.org/rfc/rfc9112.html#section-2.2)的字节framing和完整消息要求；这里进一步限制客户端输入，不声称实现全部HTTP功能。

按字节定位CRLFCRLF，禁止lossy UTF-8、裸CR/LF、obs-fold和缺冒号字段。request line恰为`method SP target SP HTTP/1.1`；方法大小写保留，非法token400；版本token不符合`HTTP/一位数字.一位数字`形状为400，合法但非1.1为505。整个target先拒SP、所有CTL/DEL及非ASCII；按第一个问号拆分，保留None/Some("")区别。path须以单个`/`开始，仅允许ASCII字母数字及`/_-.`，拒绝重复斜杠、单独`.`/`..`段、percent/path反斜杠、absolute URI、authority-form；任何fragment符号`#`也拒绝。query仅保留可见ASCII，具体路由解码在A2.2。target过长414，未完整head不能进入业务。

头名按ASCII token校验并转小写；冒号前不允许空白，值只接受HTAB/SP/VCHAR并只trim首尾OWS。所有重复头（大小写不敏感）400，包含Host/Origin/Content-Length/X-QT-Panel及普通头，不合并列表。Host必有且语法合法，否则400；语法合法但不等于固定authority则403。Origin若存在必须逐字等于固定origin，null/多值/路径/后缀欺骗均403；不信任Forwarded或X-Forwarded-*来改变目标origin。

任何Transfer-Encoding拒400，包括TE+CL；不支持请求压缩，Content-Encoding出现即415；Expect出现即417；Upgrade或Connection: upgrade拒400。Content-Length仅十进制数字（可前导零），禁止正负号/逗号，溢出400，大于body上限413。GET/HEAD只允许缺失CL或0；POST/PUT/DELETE必须有CL，缺失411。body提前EOF400，绝不把截断内容送入handler；JSON正文须严格UTF-8，不能修补替换字符。

每连接只处理第一条完整请求并发送Connection: close。收到第一条body之后的字节全部忽略并关闭，不能执行第二条；不承诺能预知尚未到达的尾随字节。完整单消息在任意TCP分片下语义相同；pipelining测试只要求第一条至多执行一次、第二条零执行。

所有`/api/admin/v1/`请求须有唯一精确`X-QT-Panel: 1`。GET无Origin可接受，但若有必须匹配；POST/PUT/DELETE必须正确Origin及`application/json`或`application/json; charset=utf-8`（media type/charset大小写不敏感，其他参数拒415）。Fetch-Metadata的Sec-Fetch-Site缺失可接受；存在只接受same-origin/none，其余403，不能用它代替Origin。未提供CORS允许头，OPTIONS/TRACE/CONNECT等不执行，合法但未支持的方法405。HEAD合法请求的错误响应也不发送body，Content-Length可表示相应错误body的长度。

H1唯一成功路由为`GET /api/admin/v1/state`，禁止query（含空问号），返回A1原JSON。路径已知但方法不符405并带`Allow: GET`，其他路径404；跨源/头部错误优先拒绝，不能进入fallback。写路由及其dry_run统一门禁留A2.2，不因测试stub通过而声明已支持管理写入。

固定JSON响应头：Content-Type application/json; charset=utf-8、Content-Length按UTF-8 bytes、Connection: close、Cache-Control: no-store、X-Content-Type-Options: nosniff、Cross-Origin-Resource-Policy: same-origin、Referrer-Policy: no-referrer、X-Frame-Options: DENY。不反射任意header/path/body。200 state另带十进制`X-QT-Snapshot-Age-Ms`，值0..9999；不返回ETag/304，不发送Access-Control-Allow-*。

## 4 错误与状态表

| code | HTTP状态 | 副作用/恢复 |
| --- | --- | --- |
| bad_request | 400 | framing/字节/正文不完整，未分发 |
| header_too_large | 431 | 未分发，关闭连接 |
| target_too_long | 414 | 未分发 |
| body_too_large | 413 | 未分发，不继续读大body |
| length_required | 411 | 未分发 |
| unsupported_media_type | 415 | 未分发 |
| expectation_failed | 417 | 不发送100 Continue，不分发 |
| unsupported_version | 505 | 不降级解析 |
| forbidden | 403 | Host/Origin/CSRF/Fetch Metadata拒绝，零worker调用 |
| not_found | 404 | 无该路由；不转交旧接口 |
| method_not_allowed | 405 | 不执行，已知state路径带Allow: GET；HEAD不发送body |
| overloaded | 503 | 已接纳请求的短cache锁不可得；连接满发生在解析前，直接关闭而不构造错误JSON |
| request_timeout | 408 | 仅读取阶段到期，未分发；发送/整连接到期只关闭 |
| snapshot_unavailable | 503 | 尚无数据、生产失败/迟到/过期/编码超限；不返回旧成功body |
| internal | 500 | 固定安全错误；不带panic/SQL/路径/输入 |

读缓存成功不保证整个write成功；对端断开或发送超时不再发送错误JSON。业务ControlError/CatalogError/AnalyticsError映射及操作ID回执由A2.2另写，不能用本表伪造持久写的NotSent/rollback结论。

## 5 函数细化与正确性论证

### H1.1 validate_origin与bind

`PanelServer::bind(listen: SocketAddr, browser_origin: Option<&str>) -> Result<Self, StartupError>`先拒绝非127.0.0.1/::1；允许端口0供隔离测试，bind后以local_addr取得实际端口。默认origin由已核定实际地址生成；显式origin用reqwest::Url做语法检查，再按§2限制并得到唯一规范origin/authority。显式配置错误不得监听服务；为端口0默认origin只能在bind后补定，失败即释放listener。StartupError只含固定枚举，不带原配置正文。正常运行不得同时创建两个同worker的server。

论证：未信任请求文本便完成边界配置；Origin配置来自可信组合入口而非网络，Host检查可防任意域名重绑定。所有失败释放已绑定socket，无DB/网络管理请求。

### H1.2 read_head与parse_head

`read_head(stream, accepted_at) -> Result<(RequestHead, Vec<u8>, Instant), HttpFailure>`使用固定4KiB读块，在accept+5秒内累计不超过head上限加一个块的暂存。每次先找终止符，找到后仍核终止位置≤16KiB，不能因同包包含终止符绕过上限；保存终止符后的有限剩余字节供body使用，并返回本次head完成Instant。无终止符且超过上限431，EOF400，读IO错误只关闭；成功交parse_head。按[Ready截止修正](../../../fixes/panel-http-fix-ready-deadline.md)，应用层读前、每次读返回及完整解析返回均显式核同一deadline，不能只靠timeout_at。

`parse_head(&[u8])`按§3逐项验证request line、target、字段数/字节、重复头、framing、版本，返回固定错误码。循环上限来自16KiB和64字段；Content-Length用checked解析，不从失败解析补0。方法/路径未知但语法合法交后续路由，不执行任何回调。

论证：任意TCP分片只有完整head进入纯parser；界限在找到delimiter后再次检查。保留每个字段直到重复检测完成，故大小写变体和重复长度不能改变唯一framing。错误不包含原字节，解析零外部副作用。

### H1.3 authorize与read_body

`authorize(head, origin) -> Result<(), HttpFailure>`只读校验固定Host、可空Origin、方法对应JSON/自定义头和Fetch Metadata。安全相关校验完成后才读声明body并分发。H1只读路由本应无body；`read_body`作为A2.2共用的有限读取函数仍需独立测试，不假装已有业务写handler。

`read_body(stream, buffered, content_length, deadline)`仅消费所需n字节；n≤1MiB由head保证。deadline固定为head完成Instant+10秒并受整连接期限约束，不能从authorize结束重起表。优先消费已有字节，再读剩余数量，EOF拒绝，收满后验证UTF-8；额外字节不作第二请求。已缓冲/零长度路径、每次读前后及返回成功前都显式核期限。它不解析业务JSON、不新建operation。

论证：跨源不能抵达业务调用，也不以body内字段授权；内容边界唯一，分片不改变n。每轮至少消费字节或退出/等待有界IO，期限不能由慢速分块续期；无副作用/无敏感日志。

### H1.4 encode_panel与缓存

`encode_panel(snapshot: PanelSnapshot) -> Result<Arc<[u8]>, PublishError>`仅接受A1 DTO，以serde_json::to_writer写入有32MiB上界的私有Writer。每次write先checked_add并判容量，越界返回固定错误；成功才冻结Arc。不能先无界to_vec再判长度，也不序列化任意worker材料。循环由有限DTO和字节预算终止。

`PanelCache::replace(Option<PublishedPanel>)`只在短锁内swap，旧Arc在释放锁后drop；锁poison则返回固定内部失败，使publication结束，get也不能读旧数据。`get(start_now, wall_now_ms)`用try_lock，忙503，取结果后释放锁。wall时刻非法/早于captured时刻，或age=max(单调时钟距生产开始向上取整的毫秒, wall_now_ms-captured_at_ms)≥10000时，取走并清除该entry，锁外drop，返回snapshot_unavailable；时间异常也不能让同一entry在回拨后复活。只允许失效清除，不修改正文/时间或续期。成功返回Arc和age；年龄header和body来自同一entry，未取到即不伪造空快照。

论证：仅完整合法bytes可发布；最大目录文本容量另有边界夹具证明。GET不取Core、不运行SQL/探针、不重新序列化，旧观察不会被读取续期。单调龄阻止回拨续期，wall前跳也会过期；失败不填零。缓存只说明采集时刻，B必须遵守A2§5展示时效，不能把bool当当前路由证明。

### H1.5 start_publication expire_publication与finish_publication

`start_publication(worker, started_at) -> ActivePublication`仅在server的active=None时调用，spawn_blocking闭包依次调用原A1 panel_snapshot和H1.4编码，只返回结果，不持cache或发布能力。JoinHandle立即存入server唯一active字段，不能交给另一个可被取消的父task。无需第二publisher信号量或业务generation。

`expire_publication(active, cache)`在started_at+10秒到达时将expired置true并清cache，保留同一个handle直到真实退出；expired后关闭这条deadline分支，不能在已到期timer上忙循环。期间interval tick只跳过，不启动下一任务；不能取消SQLite写连接或把drop handle当停止。

`finish_publication(active, result, cache, shutting_down)`由server观察该handle完成后调用，先移除完成的active，再处理结果。expired、shutdown、ControlError、编码错误、wall非法或已过期均清cache并丢弃结果；仅仍在原期限内的成功结果可replace，开始时间不重写。JoinError/panic清cache并转server关闭流程；不存在父publisher异常退出丢句柄的路径。每个callee沿A1/有界编码/短锁的既有错误约定，失败不改worker业务状态。

论证：active从spawn一直保留到join完成，timeout不能腾出新任务位置；cache仅由server主循环整份替换，迟到闭包无法自行发布。三个函数无无界循环；周期/关闭循环归H1.7统一负责。阻塞系统调用可能延迟回收，不承诺可抢占或固定时间释放目录锁。

### H1.6 serve_connection与respond

`serve_connection(stream, permit, context)`持16槽之一直到socket生命周期结束；按H1.2→H1.3→路由→缓存取值→respond，不绕过校验fallback。H1仅GET state；query拒400，未实现路径404，已知路径方法不符405。合法HEAD错误响应仅头；响应头长度由固定模板和有界数字形成。

`respond`先组装小header，再以正进度写循环发送header及Arc bytes（HEAD除外），全程用同一绝对写期限；写前、每次进度后、header/body/flush切换及完成返回都显式检查。zero-write、IO错误、超时或整连接到期只关闭，不再拼错误响应；返回bool仅表示传输是否在期限内完整结束，不是操作成功收据。无法撤回已开始操作发送的字节，不宣称内核调用硬实时抢占。任务退出释放stream和permit，跨await不持cache/Core锁。H1没有请求级blocking任务；A2.2必须另行处理同步worker调用。

论证：一个连接最多分发一次，第二请求不会执行；固定响应模板与bytes长度消除反射/截断后成功。有限并发加header/body/写入deadline限制慢客户端占用。丢响应不意味着业务回滚，这一点在未来写路由仍成立。

### H1.7 serve生命周期

`PanelServer::serve(self, worker, shutdown)`是唯一监督主循环，直接持有5秒interval/Skip、active及连接JoinSet，同时select accept、tick、active完成、active未expired时的10秒deadline、连接完成和shutdown；active=None时禁用其完成/deadline分支。首个tick立即发生，active存在时所有新tick只跳过。accept后try取得连接槽；无槽直接drop stream，不解析请求、不发送正文、不spawn等待任务。接纳任务进入JoinSet，完成即回收。accept错误只记固定码并退避100ms，退避可被shutdown打断。

正常watch shutdown、blocking JoinError或不可恢复cache错误：先清cache，停止accept/新投影，abort纯async连接并join，再await当前active的blocking handle，结果只丢弃；真实任务退出后才返回server已退出。句柄仍由这个supervisor持有，不依赖已结束子task恢复。连接panic只回收该槽并记固定失败码。未接旧CLI，不默认新增进程/监听端口，也不回退旧不受保护处理器。

强制abort/drop整个serve或进程崩溃不是上述有序关闭：不能声称已回收不可抢占的系统调用或即时释放目录锁；运行组合必须使用shutdown+join，再允许重开。强制取消时blocking闭包也无cache发布能力，但可能继续只读工作。测试区分正常关闭、blocking panic、迟到丢弃，不用强制abort后的观察冒充drain证明。

论证：正常运行和有序关闭时，server从spawn到join持有每个任务；没有每请求producer、无限队列或无法从父task取回的blocking句柄。关闭不重新开放边界。短锁只做Arc swap/clone，无网络/数据库等待，原调度Core职责不变。

## 6 分布式契约

1. 连接：浏览器到回环TCP，HTTP/1.1单请求短连接；连接断开不清worker持久状态。SSH可用，但静态authority必须匹配浏览器所用地址。
2. 时限：server不主动connect，connect timeout为N/A；header5秒、body10秒、write5秒、全连接45秒。本批没有远端业务写，publisher10秒不是同步SQL抢占保证。
3. 重试：服务端0次自动重发；客户端次数/退避由B细化，本批不声称已实现。写操作幂等/稳定ID留A2.2，不由TCP或错误状态保证。
4. 交付与顺序：单连接至多一次分发，跨连接无顺序保证；cache原子整份替换，同一响应不混两个entry。没有声称跨网络exactly-once。
5. 失败：断线只关闭；producer失败/过期503；程序崩溃重启cache为空，不恢复旧body。HTTP不能修改原策略或把未知当零。
6. 会话：无cookie/login/session，固定origin是配置约束不是用户身份；重连不延长TTL。新进程重新初始化cache。
7. 背压：16连接、1active publication、无等待队列；连接满直接关闭，已接纳请求缓存锁忙503；业务worker/管理预算在A2.2继续复用，不由本模块另造。

## 7 测试与验收

至少4组各128 cases PBT：任意分片/唯一framing；随机Host/Origin/头名重复/路径变体的拒绝及零分发；缓存时刻/失败/迟到/读取轨迹不续期；完整白名单JSON编码与大小边界。固定测试补TE+CL、oversize head同包终止符、无CL、溢出CL、提前EOF、无效UTF-8、null Origin、同站不同端口、空query/控制字节、HEAD无body、第二请求不执行、慢读/慢写、满连接直接关闭、正常shutdown仍等待blocking、blocking panic及迟到丢弃。不能把强制abort当作实际回收测试。

普通测试不得夹墙钟性能断言；新增独立release门禁测固定最大关联目录的三种来源组合（最长转义文本、长数值与固定成员）编码≤2秒、满1MiB body解析≤1秒（固定分块模式），保留既有27项release与4个debug入口。32MiB证明另含逐字段宽度包络，覆盖最长枚举、负优先级、退休/固定/释放等可选结构；互斥字段最大值可保守叠加，但包络不作为合法快照、schema正例或性能输入。不得仅凭普通大目录就宣称穷尽最坏合法状态；也不把IO等待当纯函数性能证明。

至少8类有效编译变异：漏重复Host/CL、非法长度当0、提前EOF当成功、忽略Origin、header溢出同包绕过、从完成时刻续TTL、超时提前移除active、失败/迟到仍发布、处理第二请求。明确实际分发计数/公开输出，不用编译失败或空测试当捕获。

机械核对新错误schema/枚举/状态表、H1.1–H1.7标记、精确测试清单及核心冻结；源策略/DDL/worker/旧status/CLI接线零改，仅main新增模块声明。默认debug/release、全部性能、clippy/build及独审五维度通过才算A2.1退出。真实浏览器交互仍属B，不能以TCP夹具冒充。

## 8 设计自检

- [x] 范围/复用/数据结构/错误及附录A七项均明确。
- [x] 非平凡函数的前置、分支/退出、callee、循环/资源与正确性论证已列。
- [x] 与A1兼容：原wire、采集时刻、算法与持久状态不变；新增缓存仅展示。
- [x] 独立§4.4复审通过，未关闭设计问题为零；[处置记录](../../../audits/sgm-6-three-page-panel/a2-1-decisions.md)保留原问题及修订。
- [x] 编码与动态验收完成；精确35入口、4组PBT、12变异、最终默认全量/性能及构建证据见[实现审核](../../../audits/sgm-6-three-page-panel/a2-1-audit.md)。未接CLI或现行服务，A2.2及页面/部署仍待后续。
