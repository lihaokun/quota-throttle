# SGM6 A2.2 事前契约清单

2026-10-19，基线65a4235。独立审核者先只读架构、三页计划、A2总体和已验收H1，提取这十节；随后A2.2细化补精确wire与预算。尚无A2.2生产实现，本清单不从实现回填预期。

## 1 契约源

架构G4–G6及SGM4/5共享类型、三页计划、A2总体§1–6、A2.1全部边界、[A2.2细化](../../design/source-groups-token-management/subplans/sgm-6-admin-business-a2-2.md)和workflow§3/4.3/4.4/6/附录A。只做管理适配，不改GLM/火山、身份与统计权威，不接旧CLI/status/run。

## 2 Schema

逐路由专用DTO，不能直接反序列化ControlCommand/SourceDraft/票据。实体ID是区分local/native类型的正十进制字符串；各revision、秒/毫秒、operation_id规则沿原契约。请求raw重复键/数字词法/UTF-8字节数/归一化及关系约束不能用schema声称全覆盖。

复用旧Source/Group/Member、OperationView wire3、TokenIdentity/NativeToken/GroupCheck/GroupReferences原wire。新增SourceCreated/model_origin、SourceModels、两种分页、TokenReveal、UsageSnapshot和独立AdminError均有schema。请求/响应校验按路由具体$defs，不能只凭总anyOf通过；嵌入旧schema须机械逐字一致。H1的状态和错误schema不变。

## 3 枚举

ControlErrorCode、来源/成员/操作/owner/原生状态复用原定义；目录/分析/适配新错误唯一映射。ScanLane/ScanPhase/ScanFault的wire应复用原as_str，不复制漂移集合。无协议组、火山临期、新路由许可或前端principal。模型三模式、any/unknown/exact与dry_run动作分类明确定义。

## 4 步骤

H2.1 route/DTO；H2.2 准入与有界业务job；H2.3 来源发现与只读模型；H2.4 目录/控制；H2.5 token/敏感复制；H2.6 同份用量wire；H2.7 响应/关闭。每条路由的method、Allow、body、revision类别、callee、成功状态与副作用可机械核对，无通用指令旁路。

## 5 行为

目录201仅保存，无operation；重复来源带原ID，不覆盖；组和成员按唯一关系恢复，planned不等于渠道已创建。成员创建取后端standby，EnsureMember固定显式意图并由原worker验真实准入。

持久操作202原OperationView，原ID/摘要重放，旧revision拒写；零targets Succeeded只说明设置保存。operation/retry不换ID，HTTP超时不伪造202/rollback/NotSent。check_group(false)也会写ready；引用计数不授权删组。

新来源默认真实model list，本次配置/凭据绑定；明确手填/fallback，成功发现绝不混手填，失败不建占位来源。原有来源发现失败不覆盖模型；新发现结果再验catalog合法域。复制是唯一明文出口，新token只固定普通issuer、单组且两协议共用，缺issuer不回退管理员。

## 6 时序和状态

所有持久写、目录创建、核验/登记和复制统一拒dry_run；POST usage是只读允许。GET state仍原5秒producer，业务请求不触发panel_snapshot/探针/采集；旧cache可暂时落后已收到收据，不能反向否定提交。

4个permit覆盖handoff到实际执行/编码/结果消费，connection timeout不归业务槽；唯一supervisor保留所有句柄。最后调用前明确检查deadline/closing/本地等待者，之后可能执行，物理TCP断线不保证已检测。有序shutdown先停准入、join全部wrapper/publication、再等原usage槽真正空闲；运行时保持驱动，强制abort不承诺drain。

query_usage原内部timeout会放弃等待，但读槽仍归真实读任务；barrier不得持Core/commands，不close semaphore。所有其他查询调用方也须停止接纳，不能忽略排队wrapper或另造任务registry。

## 7 PBT不变量

至少6组各128 cases：非法路由/DTO/跨源零调用；dry_run和版本拒写；操作重放/身份隔离；发现/失败/重复不覆盖；同份用量和unknown分项守恒；排队/超时/完成/关闭轨迹维持真实4槽。oracle来自输入模型/原契约，不调用适配器本身作expected。

固定组合再覆盖group/type/local-native ID、原生缺页不删、本地历史不丢、明文仅复制、丢响应后同ID查回、后台收敛与设置保存分离、Ready截止。TCP/假上游组合不冒充真实浏览器。

## 8 性能和容量

保留H1：16连接、1publisher、target8KiB/head16KiB/64字段/body1MiB、5/10/5/45秒、5秒发布/10秒TTL。新增业务共享4槽（pending与running不能各算4），handoff4且try_send；本地10秒、网络30秒、usage15秒等待均受connection_end-5秒约束，原query10秒不改。

所有业务完整编码32MiB；Usage最大合法域不保证都装得下，超限整份失败，不静默Top-N/截断。新增DTO解析1秒/固定大用量编码2秒两个独立release门禁，原29release+4debug不减。普通用例无墙钟性能断言，固定预算有独立字面值断言。

## 9 安全和副作用

继承loopback/固定Host-Origin/精确X-QT-Panel/JSON/Fetch Metadata/无CORS；敏感请求无Origin不放行。新路径只有/api/admin/v1命名空间，旧API不改不作fallback。无公网身份认证扩展。

请求不接owner user_id、backend_ready、permit、send_allowed、create_name、任意priority/status；秘密不进URL/日志/普通快照/收据/浏览器持久化，serde/底层错误不反射。模型普通读取不查凭据表。HTTP不直连NewAPI/另开writer；除必要薄入口、展示类型映射和HTTP外，DDL/依赖/策略/transport/CLI冻结。

本轮不访问真实配置、DB、密钥或供应商，不重启服务；fermat正式为全新初始化，隔离测试不等于部署。

## 10 跨实现一致性

route/schema/错误状态/worker原语义逐项一致；local/native ID、三类revision、秒/毫秒、any/unknown/exact不跨层改义。用量由同一UsageSnapshot映射，scanned不宣称全量complete；缺失为null不变0。state只沿A1/H1年龄，B只能降级显示。B浏览器及SGM7运行组合未实现的部分明确承接，不以H2通过代替。

## 设计和实施退出

设计阶段必须schema静态正负例、路由/错误映射/冻结检查及独立§4.4复审通过，用户确认后才编码。实施阶段再跑精确测试清单、至少12种有效编译变异、恢复重编、默认debug/release、全部性能、clippy/build及五维度。设计样例不是真实实现输出，也不计PBT或运行验收。

本轮设计验证已完成：独立§4.4复审0个未关闭设计阻断；65正/44负静态样例、6份旧schema规范化一致、28业务code与状态表、实现冻结检查通过。原页码/ledger revision及三类标签域已按callee修正，见[设计审核](a2-2-design-audit.md)和[处置](a2-2-decisions.md)。仍待用户确认和实现阶段动态验收。

用户随后已确认并进入实施，原十节约束保持。当前证据和剩余退出门禁见[实施审核](a2-2-audit.md)；静态样例不替代真实wire或运行测试。

实施退出：80/4精确清单、14 Step、真实wire专属schema、六组128 cases PBT、16种有效编译变异和恢复重编、默认debug/release各739/0/34、31release+4debug性能、clippy/build全部通过。独立审核五维度通过，0 critical / 0 unresolved。严格对象/标量形状、Ready竞争、真实query尾部及H1正文冻结的补强均保留；详见实施审核，不将本批扩大为B/C页面或SGM7完成。
