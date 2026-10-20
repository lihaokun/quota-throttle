# SGM5 A 令牌接口事前契约

2026-10-18，基线eb0b727；实现前从已确认架构/研究及[细化](../../design/source-groups-token-management/subplans/sgm-5-token-management.md)抽取，不从新实现反填。

## 1 契约来源

架构I6/I7/I8、token_service、两会话共享预算、SGM5 A/B/C边界；A负责安全原生接口，不拥有持久业务操作，B/C未完成前不能暴露创建/复制为完整管理API。

## 2 Schema字段

NativeTokenView wire1：字符串token_id/owner_user_id、owner_scope、name、masked_key、explicit_group/effective_group、status、created_at/expires_at、remaining_quota/used_quota、unlimited_quota、model_limits_enabled/model_limits、allow_ips、cross_group_retry。不得含原key、未知原生字段或上游绑定；空原生组显示继承账号组，不静默改组。

## 3 枚举和类型

OwnerScope为issuer/legacy_admin，状态为enabled/disabled/expired/exhausted；沿原ManagementErrorKind/DeliveryEvidence。普通role1、legacy10/100，id固定匹配，拒绝角色漂移。创建关联qt-t-加32小写hex；新key只允许一个合法目录group、禁auto、cross_group_retry=false。

## 4 步骤

T5A.1输入与脱敏投影；T5A.2固定owner与有限读取；T5A.3写白名单/显式复制；T5A.4原发送guard/共享预算。精确native缺失单独机械/负向测试，不把success=false普遍放行。

## 5 行为

列表每页100、最多100页/10000条，只声称当前页；缺字段/错owner/非降序/重复/计数不符整页拒绝，不默认0。remaining_quota允许安全负数，used_quota非负。查回0/1/多条分别Missing/核name逐字相等及spec后One/Ambiguous；历史查回允许有效期已过，新建及最终发送才要求未过期，不能因恢复晚而找不回创建。创建ACK无ID不认领。仅issuer有新建入口，余额和可用组不足不POST；旧管理员key可查询/启停/删除/复制，不重签。native创建不能保持禁用，确认归属前上层不得开放复制。

## 6 时序状态

无A层持久写、缓存授权或自动重试；写前读取失败NotSent，真正写的不确定结果保持Uncertain。预算/重登后的guard必须再次通过。每个token业务请求携带私有expected_user_id，检查Session ID并固定实际New-Api-User，覆盖401重登及cookie等待窗口；Bearer亦固定，旧请求None行为不变。两个账号会话不得共享cookie或回退身份；只有同一预算Arc共享。key删除不删除来源/组/历史。native准确缺失仅token单条GET可接受。

## 7 属性不变量

至少3组128 cases：独立脱敏/字段投影、输入/单组创建白名单、owner/响应身份/写作用域轨迹。参考不调用生产解析或payload构造生成期望；私有明文类型无Debug/Serialize，普通投影不出现完整key。

## 8 性能和资源

沿原连接3s/请求10s、单体1MiB/列表8MiB、64请求槽及1req/s突发2，不创建第二预算。每次方法固定请求数、每页≤100。最大页解析串行release≤2秒，原17性能预算不改。

## 9 副作用

只改newapi令牌接口/必要窄lookup与可选固定owner支持、tests/schema/docs/probe。GLM、火山、账本、目录DDL、控制回执和运行CLI冻结；不改依赖。隔离探针hash冻结，随机非3000/3001端口、新库和假上游，自建进程退出；不读生产配置/数据库/日志/key，不充值/部署/重启现行服务。

## 10 跨实现及退出

跨语言业务算法N/A；固定NewAPI源码与原生探针对照，Rust枚举/schema同源核验。至少6编译变异、默认debug/release、精确机械清单含负例、clippy/build、独立性能和workflow§6独审0未关闭问题。历史prompt模板缺失，按workflow正文十节审查；不能省略独审。A通过后仍须B/C细化和实现。

## Step 5 验证记录

2026-10-18，A范围退出通过。精确24入口及schema/枚举/Step/旧实现冻结机械检查通过；3组PBT各128 cases，11类有效编译变异均被捕获，副本还原后23普通测试通过。修正后默认debug/release各578 passed/0 failed/20 ignored；独立18性能通过，新最大页15ms；clippy仅6项既有警告，release构建通过。原生隔离探针15项通过并停止自建进程。独立设计/实现/身份修正及收尾文档审核无未关闭问题，五维度通过，详见[A审核](a-audit.md)。未执行的两个既有opt-in入口及B/C、页面、运行发布边界保留。
