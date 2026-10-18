# SGM5 C 组与令牌服务事前契约

2026-10-18，基线`bc8f33e`。先按已确认架构和A/B边界抽取，随后由[C细化](../../design/source-groups-token-management/subplans/sgm-5-token-management-c.md)补齐具体类型/步骤；设计独审通过前不进入Rust实现。本批不接现行服务。

## 1 契约源

架构G1/G4/G6、I6/I7/I8及组注册/权限/引用约束；SGM5总体细化A接口与C责任；B持久操作、工作票据、最终发送检查、未知交付和严格回读。C复用原worker、单写者、预算及A/B，不另建状态权威。

## 2 数据结构

复用目录Source/Group/Membership、OperationView wire3、TokenIdentityView wire1与A原生安全视图，DB保持v8。GroupCheckReport wire1五字段为schema_version/group_id/revision/ready/error_code；ready与error是否为空互斥。GroupReferences wire1十字段为schema_version/group_id/active_memberships/historical_memberships/live_tokens/pending_creations/historical_tokens/unknown_group_tokens/known_local_only/deletion_authorized，最后两项恒true/false。两份schema位于架构schemas目录。TokenOwners/TokenAdmission私有、不反序列化、不输出凭据，公开HTTP与页面不在本批。

## 3 枚举

复用OperationPhase、ControlErrorCode、TokenOwnerScope、TokenStatus、CreateState和ExecutionOutcome；不造同义phase或第四种来源状态。组类型保持GLM/火山，同key两协议不创建协议专用组。

## 4 步骤

T5C.1原生组读取/有界合并，T5C.2组状态及引用，T5C.3固定身份/只读资格，T5C.4命令与候选，T5C.5持久准备/最终发送/结算，T5C.6回读/登记/复制。每个Step注释恰一次，辅助函数沿该主步骤契约。

## 5 行为

新签发仅普通账号、余额可用、显式单组且关闭跨组重试；不创建账号或改余额，不回退管理员。组注册及签发权限必须保留其他组/倍率且回读一致；没有可路由成员则不就绪。旧token空/auto/未知组与管理员能力照实保留，不静默改到default。引用中的组不能删除/换类型。删除key不删源/组/账本。

ready只指注册/授权/至少一条成员路由配置核验，允许禁用成员，不能替代当前正向资格。新组倍率只补1，两个倍率别名预先不一致时拒绝写；已注册组不整理别名。仅合并issuer.account_group的special权限，保留其他项，拒绝target deny，不把它声称为user_id级授权。部分option写/崩溃歧义停止，不自动猜测修复。引用只读报告含无原生ID的创建、无映射但同显式组的token、空/auto未知范围；不存在删除/换类型API，零引用也不授权删除。

显式register_for对已注册目标也不豁免两个倍率别名及注册键集合的一致基线；第一ratio写成功/第二alias失败后，同目标再次注册仍须零PUT，不能偷偷追加permission并宣称成功。纯register=false不改倍率。

TokenOwners允许issuer-only/legacy-only，至少一方；缺issuer不能阻断legacy管理或改用管理员新签发。已有Succeeded读取/retry仍历史no-op，未完成缺身份操作NotReady节流、不误派其他owner。

组配置核验复用scan_group(group,None)且要求complete，再核全部有效绑定成员ID及每条启用渠道的目录身份/type/模型。未知或已退休仍启用的渠道必须拒绝，包括尾页；未知禁用渠道不阻塞也不授予资格。不用Some(required)的提前返回结果冒充完整扫描。

配置核验仅火山追加auto_ban=0，GLM不改；完整配置行不套30秒实时行龄，最终backend的60秒lease不放宽。正向单次观察仍fresh/30秒，同frame有限尝试候选，首候选禁用不遮蔽后项可用者，漂移不被跳过。10000容量不是lease内必能完成的活性承诺。

## 6 时序状态

操作重放先于易变准入；持久prepare先于HTTP；每次最终guard核当前版本/资格，覆盖预算等待与401恢复。ACK不成功；创建未知只查唯一关联，匹配GET不能洗白未知状态写。取消/重开不得记为安全未发送；失败查询也须持久诊断与节流。单实体写串行，重复候选一轮至多处理一次。复制只对已确认当前身份且最终准入仍有效，明文不落库。

C提交的身份/configured owner查询也属于易变准入，必须在B复用canonical的只读重放判定之后；异载荷指向不存在token仍冲突，历史同载荷不因owner当前缺失改判。

Create隐含owner在同提交临界区从已有Create/token操作恢复，仅用于规范payload判重；新ID才依赖当前issuer，其他kind同ID冲突。不能借历史owner发新写。

正向资格TTL≤30秒且受backend/permit/expiry限制；已创建key用当前组revision，不永久要求创建时revision。禁用/删除不查余额或活跃来源。复制允许Disabled但拒绝Expired/Exhausted/有效期已过，不顺带启用；敏感返回前再核最终guard。prepare后drop guard只可记Uncertain，存储失败闭锁，进程退出由B恢复。

资格失败也必须note节流；Waiting+Never等待显式retry，不查回未发创建。Enable写前独立匹配B当前Work/固定spec，不能等写后才报漂移。prepare自己的发布使epoch加2，最终gate绑定该确定代次；预算锁中的guard只读原子/time，B票据和本地资格在同Core prepare完成时核验。等待超60秒的note_token_delay只核同状态后记固定NotReady，不能更新确认证据、返回新Work或解除unknown。未配置owner不将token待办纳入run唤醒。

## 7 属性不变量

至少6组各128 cases：组配置合并独立参考和负例；引用/版本；多owner/组资格变动；持久交付/取消/重开；回读/重放历史；legacy及副作用隔离。无关组/倍率恒等，旧资格不得授予写，未知结果不产生第二次创建，历史收据保持，令牌管理不改GLM/火山/账本。PBT预期从契约和原始输入独立构造，不调用生产决策作oracle。

## 8 性能资源

共用1req/s突发2、64请求槽和原控制面背压；有限页/每轮候选，无逐token后台轮询或第二预算。网络await不持目录锁；有界循环、读写超时及取消资源回收在细化明确。保留全部20项独立性能门禁，新增预算不得与自有其他编译/测试并跑。

原生组/option/map最多10000项，正文1/8MiB；token候选每轮≤64，一次处理一项动作，原run选择最早成员/token到期。新增10000项组配置解析/合并及10000身份引用报告各独立release≤5秒。单HTTP connect3秒/总10秒；round受预算/退避影响，不宣称硬墙钟总截止。

性能固定夹具分别为9999项配置值解析后补足10000，以及10000条imported身份零token操作历史的引用查询；不含网络/开库历史校验。C本体精确入口45项（42普通/2性能/1原生opt-in），PBT6组；schema/枚举/Step与空/全ignored/漏项负例由sgm5_c.sh核对。收尾按[独立旧计时修正](../../fixes/catalog-fix-group-operation-performance-gate.md)另加1个入口，最终性能为23项release及大组操作单项debug；同一原5秒预算/计时点，原普通功能断言完整保留。

## 9 安全副作用

只改C必需的原生组能力和内部worker组合，不改GLM/火山/账本算法、NewAPI源码、推理路由或依赖；不读生产配置/数据库/日志/key，不重启/推送/合并/部署。原生验证只用新临时库、随机隔离端口、假上游。组option整体写没有远端CAS，独占管理写者前提和失败边界须明确。

## 10 一致性与退出

实现前函数契约、设计独审与精确机械清单；基本/边界/6组PBT、至少10类有效编译变异、默认debug/release、clippy/build、23项独立release性能及原大组debug预算、固定原生组合与独审五维度。组别名/账号组special权限探针已在新实例通过，不替代A+B+C组合。C完成不等于三页面或运行发布完成；尚未执行的检查不可标通过。

Step 5最终记录（2026-10-19）：两份schema/枚举/6步骤及精确45项C清单通过，6组各128 cases PBT和14类有效编译变异通过；默认debug/release各645/0/26 ignored，23项release性能及旧大组debug预算、clippy/build、隔离原生8项组合通过。独立静态审核0未关闭及五维度通过。旧计时门禁另1类零预算变异与12项runner自检通过，初轮失败/中止证据保留于[c-audit](c-audit.md)，不混作C业务变异数量。
