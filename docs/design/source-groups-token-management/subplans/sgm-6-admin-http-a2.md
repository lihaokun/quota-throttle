# SGM6 A2 本地管理 HTTP 接线方案

2026-10-19，原设计基线`ea7cf33`。本阶段把既有worker能力接到本地管理面，不新增调度器、写库连接或HTTP任务库。A1、[A2.1](sgm-6-http-boundary-a2-1.md)及[A2.2](sgm-6-admin-business-a2-2.md)均已完成内部实现验收；[A2.2证据](../../../audits/sgm-6-three-page-panel/a2-2-audit.md)不代表页面或现行服务已经接线。

## 1 范围与复用

对应架构G4–G6：管理员通过三页面管理，持访问key者只调用NewAPI。OpenCode/Claude仍是同一key接受的协议，不是来源组或新的权限体系。每来源一份配置/额度/异常状态，每组按原策略使用独立成员渠道。

新路径使用`/api/admin/v1/`。旧`status.rs`、`/api/status`、上游`/api/keys`和旧`/api/usage`不在本阶段改写，不把旧处理器当作绕过新安全校验的fallback。A2不调用`serve`或改CLI/run；运行模式选择、旧状态消费者的接线兼容及单一实际listener由SGM7负责，不能擅自把旧JSON格式替换成新快照。

| 批次 | 可独立交付的边界 | 不包括 |
| --- | --- | --- |
| A2.1 | 严格有界HTTP读取、回环/Host/Origin保护、完整响应、A1统一发布缓存及GET state | 目录/控制/token/用量业务路由、HTML页面、CLI接线 |
| A2.2 | 各业务请求DTO、错误映射、同步/异步worker调用、操作查询/恢复、模型与用量白名单 | 新调度规则、第二写者、通用异步任务系统、公开网络身份认证 |

基础使用已有Tokio、serde_json与安全DTO，不默认新增Web框架或依赖。旧HTTP仅作需求和兼容参考，不能复用其宽松解析作为安全边界：重复头覆盖、lossy UTF-8、无效长度当零、提前EOF以及无连接/读取deadline都必须在新入口被排除。

## 2 本地信任边界

仅监听127.0.0.1或::1。调用者是可信本机管理员及其SSH隧道；不能区分同机恶意进程、浏览器扩展或可任意发HTTP的本地用户。本阶段不提供公网登录、用户会话、RBAC或TLS反向代理认证。

Host必须匹配启动时固定的唯一浏览器authority。所有新API要求精确`X-QT-Panel: 1`；有Origin时必须匹配固定origin。写入及敏感复制固定为POST/PUT/DELETE，并且必须有正确Origin和JSON Content-Type；Origin缺失/null均拒绝，不靠Referer猜测。Fetch Metadata存在时拒绝cross-site/same-site，缺失不当作身份或同源证明；无CORS放行、无OPTIONS预检授权。

此组合采用API型CSRF防护，不新增nonce/session表；自定义头不是完整身份认证。依据为[OWASP的自定义请求头与Origin校验](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html#employing-custom-request-headers-for-ajaxapi)及[Fetch请求头限制](https://fetch.spec.whatwg.org/#forbidden-request-header)。具体可接受字节和失败路径在A2.1固定。

默认browser origin与实际监听地址一致；不同本地端口的SSH转发须在启动组合中显式配置唯一loopback browser origin，不能从请求的Host/Forwarded动态推导。远程暴露仍是独立认证方案，不由本设计授权。

## 3 A2.2业务接线清单

下表仅概览职责和结果语义；已实现的逐路由DTO/schema和函数契约见A2.2。内部ControlCommand不能直接反序列化为前端通用指令。

| 能力 | 既有入口或必要薄适配 | 成功/恢复语义 |
| --- | --- | --- |
| 状态读取 | A2.1已发布A1 bytes | GET 200；未知/失败/停更503，不在HTTP请求中跑SQL或探针 |
| 新来源 | worker.create_source；创建前模型发现需薄入口 | 201只表示目录保存；同凭据冲突返回已有SourceId供核对，不覆盖既有来源 |
| 新组 | worker.create_group | 201只表示本地组保存，ready不默认true；重试冲突后按newapi_group核对，不伪造operation |
| 来源加入组 | worker.plan_membership | 201返回planned；按A1的member control revision另发EnsureMember；丢响应按source/group唯一关系找回 |
| 暂停/恢复/归档来源、移除成员 | worker.submit及原专用ControlCommand | 202原OperationView；先保存意图，只有原回读收据表示远端收敛 |
| GLM固定/取消、火山撤错/设置 | 原C2命令 | 202原收据；Succeeded且零远端targets只表示设置保存，不表示渠道已切换 |
| 手动模型设置/重新发现 | 原SyncSourceModels/worker.discover_source_models | 专用请求DTO；失败不覆盖存量模型；不暴露任意priority/许可patch |
| 操作查询/显式重试 | worker.operation/retry | 查询原operation_id；重试不换逻辑ID，Unknown不当失败未执行 |
| 组核验/注册、引用 | check_group与group_references | 核验即使register=false也会保存ready，属受保护写；引用计数不授权删除组 |
| 本地访问key身份 | token_identities/token_identity | after游标每页100；返回历史身份，不把本地身份页称作原生实际状态 |
| 原生key页/显式登记 | list_tokens/register_token | owner分别分页；登记是网络读加本地写；缺页/失败不推断删除 |
| 创建/启停/删除访问key | create_token/set_token_enabled/delete_token | 202原操作；新签发用固定普通owner、单组，两协议共用；不返回明文 |
| 显式复制访问key | reveal_token | 受保护POST，沿原owner/期限/当前组/版本复核；200短时明文，no-store，不缓存到普通状态 |
| 用量 | query_usage | 新白名单wire保留query/账本revision/覆盖/缺口及同份模型联合分项；不是旧NewAPI usage接口 |
| 完整模型列表 | 新worker只读白名单入口 | 按需返回模型列表及source revision，不返回SourceMaterial/settings/selector |

不开放组删除/换类型、任意渠道status/priority、原生通用patch或直接来源凭据读取。创建来源的模型模式须在A2.2固定：默认真正发现、成功结果权威；明确选择手填/fallback，不能先写占位模型。已有discover_bounded的10秒/1MiB/1000模型限制可复用；新适配要复用worker的命令槽与model_catalog互斥，不引入普通刷新探测。

## 4 版本 幂等和取消

所有写/登记/核验在HTTP调用worker前统一检查dry_run，不能假定旧create_source/create_group/plan_membership/submit/retry都自带相同门禁。敏感复制按原worker约束处理，不把无持久写等同无授权要求。

entity ID复用原正十进制字符串，revision与时间单位明确，不接受前端传owner principal、backend_ready、permit、create_name或send_allowed。操作ID继续用原16–64字符校验和唯一命名空间。原持久操作的载荷指纹与重复/冲突由worker/store决定，HTTP不另存去重表。

同步worker方法可能等待std Mutex及SQLite：必须在有限blocking槽中执行，槽归实际任务而非HTTP等待者。取消/超时后不得提前释放槽、再排无限任务，也不能声称写未执行；持久操作用原ID查回，目录创建按唯一性和返回身份核对。具体槽数、每路由deadline、取消发生在提交前后的行为在A2.2细化，不用通用包装掩盖差异。

异步管理调用沿已有发送guard、预算和取消结算。check_group可能已写NewAPI配置再丢响应；register_token可能已本地登记；敏感复制丢响应不自动再次复制。业务适配必须逐项说明这些结果，不能把所有HTTP失败统一解释为回滚。query_usage已具备单槽、blocking读任务、取消/interrupt及真实任务退出归槽，直接复用。

## 5 发布数据的消费约束

GET state返回A1原JSON，不重写captured_at_ms、observed_at_ms、evaluated_at_ms；current/backend_ready/state描述采集时刻，不是现在的授权或操作成功。A2.1额外返回保守的`X-QT-Snapshot-Age-Ms`，B用它加本次请求往返时长和接收后的单调时长计算展示年龄；缓存总龄到10秒、样本/评估超过原30秒包含端点期限、周reset已到时不得继续显示新鲜/临期。客户端只能降级过期显示，不能把stale/unknown重新提升为observed/current，不能重算active/pin或自行续期。

backend_ready没有公开lease截止时间，B只显示“采集时就绪”，不能把该bool变成当前可路由证明。浏览器写请求仍由worker在当前时刻验版本和真实准入。HTTP故障/缓存不可用时保留的旧页面必须明确过期，不显示零用量或操作成功。上述是展示失效，不在HTTP或浏览器重算GLM选择算法。

## 6 分步退出

A2.1先完成函数细化、错误schema、十节expectations与独立§4.4审查，再实现入口/缓存及假worker测试。A2.2必须另外完成全部请求/响应schema、action白名单、错误/状态表、blocking生命周期和缺失薄入口的细化，不以本表替代。

两批都需基本/边界/PBT、有效编译变异、精确机械清单、默认debug/release、独立性能及独审五维度。测试只用临时数据、假上游和随机端口；按需可在fermat隔离运行。B才做三页交互与统一浏览器刷新，SGM7才接运行并在fermat全新部署，不动当前本地服务。
