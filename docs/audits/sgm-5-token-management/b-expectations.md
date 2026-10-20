# SGM5 B 持久令牌操作事前契约

2026-10-18，基线ec17fc9；从架构及[B细化](../../design/source-groups-token-management/subplans/sgm-5-token-management-b.md)抽取，B实现尚未开始。独立审核者已先于实现从架构/A边界另抽十节清单。模板缺失，按workflow正文§6。

## 1 契约源

架构G4/G6、I6/I7/I8、同库单写者与ControlOperation；SGM5 A原生约束及B/C边界；B细化全部。B只持久化，不发HTTP/挂worker/部署。

## 2 Schema字段

DB v8新增managed_tokens、token_operation_targets、token_execution_attempts；旧操作表增加token目标，保存操作phase/error唯一真值。TokenIdentityView wire1为13字段：schema_version/token_id/native_token_id/owner_user_id/owner_scope/origin/group_id/explicit_group/display_name/revision/create_state/created_at/deleted_at。OperationView wire3字段不变、新kind/target枚举明确；旧CatalogSnapshot及NativeTokenView wire不变。无完整key、任意原生JSON或重复实际状态缓存。

## 3 枚举

复用TokenOwnerScope、TokenStatus、CreateState、ExecutionOutcome、OperationPhase、ControlErrorCode；新增TokenOrigin created/imported，OperationKind create_token/set_token_enabled/delete_token，TargetType token。私有动作create/enable/disable/delete与原prepared/responded/not_sent/uncertain证据匹配。新/旧kind字段组合均有SQL与读取校验。

## 4 步骤

T5B.1纯校验/投影，T5B.2迁移/恢复，T5B.3登记/查询，T5B.4提交/重放，T5B.5持久prepare，T5B.6结算/回读/重试。prepare提交先于票据，回读确认先于成功。

## 5 行为

原生ID全历史唯一、不改owner；空/auto/未管理组保留，映射可空。Create仅issuer且显式单组；关联由目录instance+OperationId产生，不用显示名。未知创建Missing不重POST，歧义不选ID，唯一需逐字名字/固定spec，过期/耗尽仍能恢复。创建ACK不成功；启停按单ID回读；删除严格缺失才确认且保留历史。

## 6 时序状态

同ID重放先于当前revision/expiry/ready，同载荷同操作、异载荷冲突。逐token单未完成写；未确认Prepared重开→Unknown，已成功的phase/收据不降级；可信NotSent才允许创建恢复Never。匹配GET不能洗白未知状态写，重试不解除屏障；可能永久未决要明确。旧失败操作重试须当前revision及无较新操作，否则零修改。Work绑定instance/逻辑版本/证据版本及0..60秒窗口（倒退/过期拒绝）；旧ACK/旧读取不确认新目标。有效未收敛读取必须保存处理时间，pending只是候选、不做claim。token_send_allowed为B只读最终门禁，C仍须叠加真实就绪。创建事实Succeeded不代表当前可用，组后续ready/revision变更不破坏其历史验证。

持久终态须反向有证据：删除须成功缺失收据，已绑定创建须非NotSent尝试及成功收据；命令版本按created 1..revision、imported 2..revision唯一连续，失败也保留。attempt/证据序号允许因诊断及重试留间隙。

## 7 属性不变量

至少5组128 cases：独立幂等/关联、创建交付/崩溃重开、状态/票据/回读栅栏、legacy归属/删除/历史隔离、复杂旧库迁移镜像。生产函数不生成oracle；回执、账本、源/组/成员历史均独立快照比较。

## 8 性能资源

最多10000身份含删除历史、列表100条、pending64项；历史操作/attempt验证流式，不无界载入内存。pending使用部分索引，Waiting/Unknown至少30秒重查间隔。固定性能基线为10000条imported身份且零token操作/attempt历史的投影/验证，以及含10000条旧控制操作的指定历史迁移，各独立release≤5秒；不泛称created及任意历史最大规模，PBT正确性不代替该性能证据。保留既有18性能门禁，不混跑计时。无网络/第二预算/后台队列。

## 9 安全副作用

只写应用库v8，旧DDL/GLM/火山/账本算法/原生接口行为冻结。旧库完整校验/一致备份先于迁移；提交未知停写，失败不返回ticket。token私有票据无Deserialize，普通视图无秘密。只用临时库，不读生产配置/数据库/日志/key，不请求供应商、不重启/部署/推送。

## 10 一致性和退出

schema/枚举/Step/冻结/精确测试清单含负例；至少8类有效编译变异；默认debug/release、clippy/build、独立性能；独审0未关闭问题及五维度。B不能冒充C的真实就绪/HTTP执行组合，不以本地fixture通过证明服务可用。迁移须保留旧已成功收据与复杂历史，不仅空库。

Step 5结果（2026-10-18）：机械27项精确入口、25项普通定向、5组各128 cases PBT、10类编译变异及独审通过；最终debug/release各603 passed/0 failed/22 ignored，20项独立性能全部通过，clippy/build通过。初轮失败、已接受布局提示和性能夹具边界保留于[B审核](b-audit.md)，本清单不扩展到C或运行服务。
