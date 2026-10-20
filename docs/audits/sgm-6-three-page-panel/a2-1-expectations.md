# SGM6 A2.1 事前契约清单

2026-10-19，设计基线`ea7cf33`。此十节在编码前由独立审核者从架构/三页/A1契约提取，再用A2.1细化限定本批；不从实现回填预期。实现基线为设计提交`80ba157`，后续契约补充与验收另行记录。

## 1 契约源

[架构](../../design/source-groups-token-management/architecture.md)G5/G6、[A2总体](../../design/source-groups-token-management/subplans/sgm-6-admin-http-a2.md)、[A2.1细化](../../design/source-groups-token-management/subplans/sgm-6-http-boundary-a2-1.md)、A1与workflow§4.3/4.4/6/附录A。业务写、模型/用量wire和worker阻塞调度留A2.2，不缩减整个A2目标。

## 2 Schema

200 state逐字为A1 PanelSnapshot wire1，原字段/ID/时间单位不变；新增15种固定HTTP错误的schema_version=1/error白名单，无任意message。X-QT-Snapshot-Age-Ms只在成功状态响应，0..9999毫秒，保守年龄不写回JSON。无密钥、配置或内部许可序列化。

## 3 枚举

HttpErrorCode唯一集合与状态码映射由A2.1§4定义，schema机械一致。A1与来源/控制枚举直接复用；不引入新来源状态或协议组。未提供业务ControlCommand的通用反序列化。

## 4 步骤

H1.1固定回环/authority；H1.2严格有界head；H1.3同源校验/完整body；H1.4有界编码/缓存；H1.5单producer生命周期；H1.6单连接/完整响应；H1.7服务任务所有权。代码注释及函数边界应机械可核。

## 5 行为

只成功提供GET /api/admin/v1/state且无query。其他路由不能假装已支持；已知路径错方法405、未知404。旧status/API/CLI不改、不挂不受保护fallback。解析/安全失败零worker调用。只有完整消息进入分发；每连接至多首条请求执行，第二条不执行。

## 6 时序状态

5秒单一producer，10秒TTL从生产开始算；唯一serve supervisor持ActivePublication直至join，失败清cache、迟到不发布、timeout不移走仍运行handle。GET不执行SQL/probe/序列化或续任何时间。current/backend_ready仅采集时状态，B按原时间+年龄header展示，不能当路由证明。有序shutdown停止新工作并join实际任务；强制abort/drop不承诺drain或即时释放目录锁。

IO截止必须在应用层操作前后及成功返回边界显式检查，包含Ready/全部缓冲路径；body起点由read_head返回的完成Instant固定。timeout_at不替代这一前置检查，不承诺抢占内核调用或回滚已发送字节。

## 7 PBT不变量

至少4组各128 cases：单条消息分片不改变framing；非法/跨源/歧义头零分发；缓存时间/失败/迟到轨迹不复活或续期；安全完整编码、容量与未知语义。expected从输入规则独立推导，不调用生产parse/authorize/cache函数作为oracle。固定故障测试覆盖真实TCP和阻塞任务生命周期。

## 8 性能容量

target8KiB、head16KiB/64字段、body1MiB、完整JSON32MiB，超限固定失败不截断。16连接/1active、无入队；连接满未解析方法便直接关闭。head/body/write/整连接分别5/10/5/45秒。最坏合法A1字段容量须单独验证，并依赖成员只属一个组/临期仅本组成员的不变量。新增2个独立release预算，既有27release/4debug全保留；普通功能/PBT不夹墙钟预算。

容量证据由最大关联目录及独立字段宽度包络共同组成，包络覆盖数值和全部可选形状，不冒充可路由快照；JSON数字保守按32字节估算，字符串用各字段最长合法转义值。固定性能夹具不扩称全部状态组合耗时证明，原2秒/1秒预算不变。

固定预算须用契约字面值独立断言，不能只引用实现常量来生成输入和expected；header跨界测试须包含不整除上限的分片，实际覆盖同一read跨界并读到delimiter。

## 9 安全副作用

loopback+固定Host/Origin+精确自定义头+写JSON+无CORS；缺失Fetch Metadata不证明同源。写/敏感请求Origin缺失/null拒绝。无登录系统，明确可信本机/SSH边界。所有错误不回显输入、凭据或底层正文；敏感请求头/body不进日志。生产策略/worker/DDL/旧status不改，main仅模块声明；不接运行服务。

## 10 跨实现一致性

HTTP成功bytes仍用A1原DTO/schema验证；唯一HTTP错误枚举、15个code/状态码表和错误schema须一致，不能在响应分支重复手写集合。query的None/Some("")、原时间单位/ID字符串及完整消息边界在parser、路由、测试oracle中保持同义。当前没有浏览器实现，跨语言验证为N/A，B必须承接年龄header及只能降级的展示失效规则；不把本批TCP夹具称作浏览器交互测试。

## 退出验证

设计独审通过后才编码；schema/枚举/步骤/范围/精确清单、至少8种有效编译变异、默认全量/独立性能/clippy/build及独审五维度。恢复后必须真正重编译，不能因保留旧mtime复用变异二进制。实现验收不含A2.2业务、B浏览器交互或SGM7部署。

设计阶段记录：独立§4.4复审0个未关闭设计阻断；十节结构/函数与附录A已齐，错误schema静态15正例/6负例及metaschema通过。

实现阶段最终验证见[审核](a2-1-audit.md)：33普通/2性能精确集合、4组各128 cases PBT、12种有效编译变异和还原重编、schema/状态/七步/冻结/24runner检查全部通过。最终默认debug/release各696 passed/0 failed/32 ignored，29release性能和4个debug入口、clippy/build通过；Ready截止与容量证明补齐均保留原红例和预算。旧运行入口未接线，A2.2/B/SGM7不计入本批完成范围。
