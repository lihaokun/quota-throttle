# SGM5 C 组就绪与令牌协调服务

2026-10-18，基线`bc8f33e`，沿用户连续实施授权。C把A的固定身份原生能力与B持久操作接到原ReconcileWorker，补组注册及引用报告；不接现行CLI/面板，不部署。设计独审通过后逐步实施。

## 1 范围与复用

G4由A的owner核验、B的持久事实和C的当前准入/执行闭环共同保证；G1由原目录及渠道协调器保证，C只核对组注册/权限及成员路由配置；G6沿原事务、共享预算、发布fence及有限后台轮次。一个访问key仍同时支持两种下游协议，无逐key上游绑定。

保留DB v8、旧DDL、全部旧wire和依赖。复用ReconcileWorker的Core/store/publication/lane/commands及原run，不另建线程、队列、计时器、调度器或许可权威。新增native groups、catalog groups和worker token_service三个窄模块，worker内部按执行/访问拆文件；A/B仅补C必要的只读身份/票据访问器和同预算校验。既有GLM/火山/账本算法及普通管理请求行为不改。

组类型/原生名称保持不可变，本批不增加组删除或换类型API。引用报告只展示本地已知引用，不能授权删除；后续UI不得据零计数绕过这个边界。账号创建、配置安装、额度准备及生产迁移仍属SGM7。

## 2 已核实原生行为与就绪分层

固定rc.20源码与相同SHA256二进制实证：`GroupRatio`和`group_ratio_setting.group_ratio`更新同一运行map，但`GET /api/option/`的两个缓存值互不跟随；不得在预先不一致时选一边覆盖另一边。`group_ratio_setting.group_special_usable_group`按账号group合并权限，不是user_id级权限；保留全局`UserUsableGroups`、`GroupGroupRatio`和无关账号组。`-:name`可取消组，`+:name`及裸名可加入；冲突项不自动删改。[固定组服务](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/service/group.go)、[配置更新](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/option.go)。隔离探针为`scripts/probes/newapi_groups_probe.py`。

`Group.ready`表示注册、签发权限及至少一个有效受管成员的路由配置已核对，成员可处于禁用状态；它不是额度/可服务性缓存。这样旧策略可在ready后发布许可、原协调器启用成员，不形成ready←enabled←ready循环。新建token及受管正向启用/复制额外要求本次新鲜的原生status=1、当前策略许可及来源资格。组正常版本演进后，已有key按本次当前组版本核验；只有未完成创建仍绑定B保存的创建组版本。

## 3 数据结构与错误

复用全部A/B身份、操作、状态和错误枚举。新增类型仅如下：

| 类型 | 字段与生命周期 | 消费者 |
| --- | --- | --- |
| TokenOwners | 可选的固定issuer owner/client与独立可选legacy owner，至少配置一方；字段私有，无Debug/Serialize/Deserialize | worker初始化后不可替换；已配置issuer与管理员同规范base URL/同一Arc预算，scope固定且两者ID不同；legacy复用原管理员client |
| GroupCheckReport | schema_version=1、group_id、revision、ready、error_code可空 | 显式组核验结果，不是令牌发送许可；错误时ready=false |
| GroupReferences | schema_version=1、group_id、active_memberships、historical_memberships、live_tokens、pending_creations、historical_tokens、unknown_group_tokens、known_local_only=true、deletion_authorized=false | 目录只读报告；未决创建也占引用，旧空/auto组单列未知范围 |
| TokenAdmission | 私有当前owner、group/member及版本、检查起点/截止时间、原生token可选 | 单次网络执行栈内，无序列化/持久化；TTL最多30秒及backend/permit/原生有效期交集 |

原生组option完整JSON仅在有限调用栈内解析/合并，不Debug、不入库、不返回前端。新组倍率仅在缺项时设1，不覆盖已有倍率或用户组倍率；已有正向权限保留原描述。解析缺字段、重名option、负/非有限倍率、错误map形状或别名歧义均拒绝，不猜空map。

引用计数使用u32，目录容量下每项≤10000，JSON中为整数；SQL类型/越界转换失败报Storage，不截断或使用as转换。

ControlErrorCode沿旧分类；native异常经原code映射。新鲜检查失败保存NotReady/固定错误；版本过时零确认、存储不明闭锁。账号不足不能回退管理员。HTTP成功、局部option写入和创建事实Succeeded都不代表当前可调用。

## 4 函数细化与论证

### T5C.1 原生组读取与有界合并

`ManagedGroups::registered()`读取GET `/api/group/`，严格200/成功/data字符串数组、唯一名称；最多10000项，未知/坏值错误，不吞为空。`read_options()`读取GET `/api/option/`，最多10000项且受原8MiB上限，仅提取三个必需option（两个倍率别名及special map），重复/缺失拒绝；其他option不保存。map解析严格类型、总条目最多10000，单次PUT受原1MiB上限。

`register_for(group, account_group)`前置合法单受管group、非空有界account_group与有效最终guard；不执行账号创建。先读registered和options；任何显式注册/权限写入前，两个倍率map都必须语义相等且键集合与registered相等，否则InvalidResponse零PUT，目标已存在也不豁免。目标已注册时不整理或重写倍率别名；目标未注册时从一致基线计算一次next=old+{group:1}，依次PUT `GroupRatio`、回读核本次old/next精确中间态、PUT namespaced别名、回读核两者next及registered集合。无变化则不PUT。纯check_group(register=false)只读真实注册/权限/成员，不用显式注册写路径自动修复既有别名。

每次PUT均沿原transport fenced/idempotent-write，只有限一次，无透明业务重试。若第一写失败/未知立即返回未就绪，不继续第二写或补偿；若进程退出遗留别名不一致，后续新组注册拒绝自动修复。两个字段没有原生事务/CAS，操作者需明确修复歧义；不为此新增恢复框架。部分注册不授予token资格，不声称自动无损恢复。

权限只在special[account_group]加入目标裸名，且仅当它既无裸名也无`+:group`；存在`-:group`直接RemoteRejected，不移除人工deny。已有正向项不改描述；其他map项逐值保留。PUT前再读options，若本次基线已变化拒绝；写后回读special必须等于next，全局usable/账号组倍率不在写白名单内。此权限会影响同account_group账号，不宣称仅影响某个用户。最终C还从真正issuer会话GET self/groups核验可用性。

论证：先验证两个倍率真值一致才能保留无关项；单次只添加缺项且所有PUT后核完整目标，最终成功蕴含本次观察的注册/权限成立。未知或第三值不被“修复”。循环上界10000、业务请求有限；网络不持目录锁。无外部并发option写者为必要前提，写前重读不能替代远端CAS。

### T5C.2 目录组状态和引用

`group(id)`按正ID读取安全GroupView；缺失NotFound。`group_for_name(name)`用原唯一索引精确查显式原生组，无匹配为None，不把空/auto映射default，用于导入发生在本地建组之前的身份。`set_group_ready(id,expected_revision,ready,now)`在原事务校版本，变化时安全加1、更新ready并唤醒该组未终止的成员待办；未变化不加版本。提交未知停写，错误零部分更新。只由可信worker核验路径调用，不暴露HTTP布尔设置。操作/旧成功收据不重写；worker撤销该组旧permit/observation，新策略显式刷新后重发许可。

`group_references(id)`同一只读快照计数：有效/退役中成员与已退役历史分开；未删除且有原生ID的显式组相符token（包括group_id为空但名称恰匹配）计live；无原生ID的本组创建计pending；已删身份计history；空/auto组未删除身份计unknown。数据库行有限，计数不扫描原生账号或推断空组实际归属。known_local_only恒true、deletion_authorized恒false；没有删除/换类型出口，所以报告过时也不会误删。

论证：本地名称与ID共同匹配排除登记先于建组的漏项；持久未知创建不因无原生ID被漏掉。历史不删除，归属不复用；零引用不证明所有原生账号无引用。

### T5C.3 固定身份及只读资格

`NewApiClient::shares_management_scope(other)`仅比较已规范base URL及同Arc预算；不返回URL/凭据。`ReconcileWorker::new_with_token_owners(...)`沿原new创建，验证至少配置一方、已配置的scope/正ID、两方不同ID及issuer同scope客户端；失败无网络，旧new保留token完全未配置模式。legacy-only可管理旧key，缺issuer只阻止依赖它的新签发/组签发配置核验，不回退管理员；已成功操作的读取/retry仍保留历史无网络结果，缺身份的未完成操作记录NotReady且节流。TokenOwners只在初始化注入，不支持运行中换账号。

`check_group(group,revision,register)`占原commands槽及lane，dry_run一律拒绝，因为register=false也会持久更新ready。持锁取当前组和有效成员快照、原backend/发布代次，然后释放锁。先固定issuer.profile；余额只影响签发，不阻塞组配置核验。register=true调用T5C.1，false只做原生只读检查。issuer.available_groups须含目标。原ManagedChannels.scan_group(group,None)做完整有界扫描，必须complete；不能传Some(required)，因为旧接口覆盖这些ID后可能提前返回部分结果。全部有效绑定成员ID必须出现在扫描；每条status=1都须对应本组非退休/非退役中目录成员且identity/type/单组/models一致，否则BindingDrift。至少一条已知绑定非退休成员配置一致（可status=2）才能ready，未知写不能当可路由证据。未知禁用渠道不阻塞，但不计作就绪依据、不接管或删除。初次无成员保持未就绪。回到Core重核发布输入代次、全组/成员版本及backend窗口，才set_ready(true)；失败仅在仍对应当前组版本时set_ready(false)，不把旧失败覆盖较新的核验。报告为完成观察，不是长期许可。

`positive_admission(work/native)`前置B当前身份和已配置owner。受管创建或映射到管理组的身份：读取固定issuer/legacy profile与可用组，核owner角色、启用状态；Create/Enable另核quota>0。组ready及当前组版本有效；有限遍历当前组来源未暂停/退役/耗尽、成员未退休/未决、原permit有效的候选，单IDGET核身份/模型、火山auto_ban=0（GLM保持原值），漂移/网络错误终止。status非1或priority尚未收敛可尝试下一候选，不把首个本地候选当作唯一可用者。观察仍须fresh，选中后绑定该成员；无匹配NotReady。所有请求共用检查起点和≤起点+30秒、backend/permit/expiry截止，不重开窗口；截止前未找到也可NotReady，不承诺最大目录的活性。返回之前再核发布代次及所有本地版本。

导入的空/auto/不受管组保留兼容分支：按固定owner、原生ID、未删除/无未决本地操作、显式组未漂移核验，不伪造GroupId或强隔离保证。Disable/Delete不要求余额、组ready或活跃来源，但仍需同实例backend和正确owner。已有受管key不要求创建时组版本仍当前，防止正常组演进永久锁死管理。

论证：真实profile/组列表和单ID渠道观察产生一次性资格，不能由前端bool或持久ready代替。两身份共享预算且不串会话；不存在fallback。资格最终在同一Core当前快照复查，旧发布/过期观察不能授予写。

正向单token操作只复核当前候选，依赖该组此前成功完整配置核验及其后没有外部原生写者；不把这个前提说成已经实时监控了所有原生渠道。目录变化先失效epoch，新增成员仍由原禁用创建/确认流程接入。

完整组扫描的稳定配置行不套用单候选30秒fresh条件；配置helper核身份/模型及仅火山auto_ban=0，最终仍受原backend/epoch约束。真实backend lease保持60秒，10000是容量边界，不承诺100页在共享预算下必能完成；不提高原观察TTL或新增分页状态机。

### T5C.4 令牌命令与候选

`create_token/set_token_enabled/delete_token`是可信内部入口，固定owner来自TokenOwners；占原commands槽，dry_run拒绝。B补`replayed_token_operation(id,command)`只读复用原canonical/replayed_operation；C在目标身份/configured owner查询前调用，同载荷返回原view、异载荷返回冲突；仅新操作核配置身份再B事务提交。B事务内仍重复完整幂等检查，不另写摘要或把查询当claim。提交只承诺持久Queued，不承诺原生已创建；真实当前资格在执行前检查。通用retry对token分派到对应owner认证恢复，不替换为管理员会话；Succeeded保留无网络no-op，unknown状态写仍按B拒绝。

Create的owner是隐含配置输入：同一提交临界区先查既有operation；若已有Create/token，用其不可变持久owner构造本次完整命令再复用B判同载荷/冲突；其他kind同ID直接冲突。只有新ID才从当前TokenOwners取issuer。历史owner只用于判重，不用于绕过配置发新请求；缺issuer的旧Queued仍可读回，后台保持NotReady节流。`submit_token_built`与现有静态命令包装共用同一个commands/publish入口，无第二次查找提交竞态。

B补只读`TokenWork::kind/group_id/loaded_at/enabled`必要访问器，`next_token_attempt_at()`从原pending索引计算Queued及Waiting/Unknown的最早到期；保留30秒间隔和64候选。`round`沿原lane，在成员动作之后处理一次本轮去重token候选（最多64），每候选最多一项发送或回读，不在一次round无限追到成功。旧未配置模式零token网络且不把token待办纳入唤醒；配置后原run取成员/token最早到期，不加另一个计时器。

### T5C.5 持久准备 最终发送与结算

`token_step(operation,stop)`加载B Work；Succeeded/Failed不自动重试，Waiting/Unknown只读，Queued才考虑写。Create只在Never准备，其他创建状态查唯一关联；Set/Delete按原生ID。资格失败必须B.note更新phase/处理时间，Queued转Waiting或Failed而不是保留到期热循环。Waiting+Never不能查回认领未发送创建，仅保存NotReady并等待显式retry回Queued；不放宽B。

Enable在prepare前必须A.get当前token并调用B只读`validate_token_work(work,native,now)`：核完整当前Work/0..60秒窗口、固定owner/id/显式组，created另核原创建spec；允许该Work自身活动命令，不能复用会拒绝活动命令的reveal_target。失败零PUT并按上述路径记录。

取得T5C.3资格后，以捕获epoch调用`mutate(Some(epoch))`，其中B.prepare提交后、Core仍持有时核B.token_send_allowed及当前来源/成员/许可，派生原子最终gate的`epoch+2`和截止时间；不能在解锁后读取一个可能属于其他命令的新epoch。票据成功提交才构造A API。原transport会在预算mutex中调用guard，故guard只执行原gate(epoch,start,end,stop)与dry_run，不在预算锁中查SQL、取Core或做网络。所有store/资格变更均经过mutate先封旧epoch；相同偶数epoch蕴含刚核验的B票据和本地资格仍不变，时间条件由截止时间独立约束。因此预算等待与401恢复后的每次guard等价于重核当前票据/资格，而不引入锁反序或第二状态权威。

prepare已commit后的检查可能失败，必须返回`(ticket, check_result)`到锁外，立即建立TokenExecutionGuard，再在未调用A的分支明确NotSent结算；不得在closure内用`?`丢弃ticket，也不得在持Core时drop会重锁Core的guard。若SQL/poison阻止结算则闭锁、不发布可发送gate，由B重开恢复。测试必须覆盖prepare成功后资格失败/到期，零HTTP且不会留可恢复存储中的永久Applying。

发送只调用A create/set_enabled/delete，最多一次业务写，不自动重发未知结果；A自带唯一401恢复继续保留。成功ACK→Responded(None)，明确响应失败→Responded(code)，可信未发出→NotSent(code)，其余→Uncertain。结算自己的原票据，不以最新意图改写交付事实；成功后下一候选轮用新Work回读。网络和预算等待无Core锁。

`TokenExecutionGuard`在prepare后创建，正常可信结算成功才解除；future取消/异常drop时同步尝试Uncertain结算，存储已poisoned则保持闭锁、由B重开恢复。它不启动清理任务、不把取消猜成NotSent。原reconcile_once/run的自有任务继续负责有界round，caller取消不直接中断已发写；drop guard补直接future取消的窗口。进程终止仍依赖B prepared→uncertain恢复，不承诺进程崩溃时执行析构。

论证：唯一lane和B逐token串行保证没有相反在途写；prepare持久化先于HTTP，drop不生成成功/未发出证据。最终guard依据当前持久票据和一次性资格，旧ACK只结算旧尝试，不能确认目标。发送/结算失败留可诊断未决，不改变原GLM/火山策略。

### T5C.6 回读 登记与复制

`observe_token(operation)`在网络前新取B Work，随后A.lookup_created或get；成功调用B.confirm，失败调用B.note_token_problem。固定一次有限读取，不缓存跨操作的原生token正文；0..60秒B窗口及完整证据版本由B再核。observe/list/register均挂stop/当前backend/固定owner的最终原子guard，但历史创建查回不要求活跃来源或未来expiry。若等待超过窗口或本地状态变化，不能把旧事实包装成新Work确认；下一轮重新读取。unknown创建Missing不重POST，unknown状态写matching GET仍未决。

B补窄`note_token_delay(old_work,now)`：同事务核instance/token/operation/action/完整record（含evidence）与operation projection仍相同，且时钟不倒退，只豁免loaded_at年龄；固定NotReady走既有problem/project刷新处理时间及证据版本。无NativeTokenView/任意错误入参，不返回新Work，不成功/绑定/解除unknown；状态变化零修改。C仅在本次等待超窗、未完成处理时用它节流，然后重取事实；不能用它延长旧观察的确认期限。

`list_tokens(owner_scope,page)`仅固定配置身份的一页原生安全投影，不写目录，不把缺项当删除；`register_token(owner_scope,native_id,label)`在网络前取发布代次，A单IDGET后核代次再B登记，未配置owner拒绝，不从响应选择账号。已有本地列表沿B keyset投影；两个owner不做一页合并或静默截断。

B补`validate_token_access(id,revision,native)`只读验证：当前reveal_target合法、owner/原生ID/显式组一致，created身份另核固定创建spec；imported不强加新建限制。它复用B已有固定spec比较，不重定义身份权威。

`reveal_token(id,revision)`先B当前身份，再固定owner单ID读取及validate；禁用状态允许复制但不会启用，过期/耗尽或已过有效期拒绝。受管身份另取本次positive_admission（复制不要求quota>0）；legacy不受管分支不强加本地来源。A.reveal使用最终guard，并在取得SecretString后、向调用方返回前再次核同一B版本/当前资格；失效时丢弃secret并报错。完整key不放进日志、快照、操作结果或数据库。

论证：读取先于确认Work，后置完整栅栏防止旧证据确认新命令；按需敏感返回独立于普通列表，取消自然释放内存及原预算槽。复制是只读敏感操作，不能顺带清余额/启用或改组。

## 5 并发与跨进程边界

锁序沿原commands→lane→短Core；网络只持有有界槽/lane，不持Core；transport最终同步guard只读原子fence/时钟，不取Core、认证锁或执行SQL。publication失效先于本地事务，group改动撤销旧许可，源/组/成员命令仍走原发布入口。外部原生配置/账号角色与受管token不得存在并发写者；本地fence不能撤销已经发送的远端HTTP，也不提供远端CAS。

连接仍为原reqwest池，connect3秒/单HTTP10秒，正文1/8MiB；1req/s突发2和64槽同实例共用。读取每次处理有限一次，业务写一次，只有A原明确401重登一次；未知结果不可当重试成功。拒绝/断连/慢响应保留安全错误及交付语义。整个round有限候选但受共享预算和远端退避影响，不承诺硬墙钟截止；shutdown先stop阻止后续发送，仍可能等待原预算排队/退避结束后guard拒绝以及已发单请求终止/结算，不能把单HTTP10秒称为shutdown总上界。没有第二数据库/会话实现或后台清理队列。

## 6 验收与设计审查

基本/反例覆盖组别名歧义零写、精确old/next中间态与部分失败、无关倍率/权限保留、deny与同account_group范围、零成员/禁用已配置成员引导、ready不代替活跃资格；错owner/跨预算/跨实例拒绝；无余额与无上游仍可禁用/删除；legacy空/auto/未知组；组版本正常演进不永久拒绝既有key；所有prepare/发送/回读取消窗口、旧fence/401/配额等待、未知创建与未知状态写、strict missing删除、复制后置失效及无秘密。

至少6组各128 cases PBT：组map合并与负例、组引用/版本、不同owner/组资格变动、持久交付/取消/重开轨迹、回读/重放历史、legacy与副作用隔离。至少10类有效编译变异，不能把编译失败或零测试当捕获。默认debug/release、clippy/build及原20项独立性能；新增10000项组配置解析/合并与10000身份引用报告各release≤5秒。固定原生新组/权限/别名及A+B+C组合验证不使用真实上游。精确测试入口、schema/enum/T5C.1–6及冻结范围由机械脚本核验，独审0未关闭与五维度后退出。

收尾还按既有已确认方案补齐[旧大组计时隔离](../../../fixes/catalog-fix-group-operation-performance-gate.md)：普通功能断言和原5秒/计时点不变，另一个ignored入口同时在独立release/debug执行。C本体仍45入口；最终性能总集23项release及该项debug，总ignored26。机械脚本只为这项纯测试修正增加精确正文/归一化对照，不放开生产control代码。

- [x] 目标/复用、共享类型、分支/错误、循环上界与callee责任已列。
- [x] ready引导环、部分option写、legacy与未知交付边界显式。
- [x] 实现前独立设计审通过，组合边界修订见[c-decisions](../../../audits/sgm-5-token-management/c-decisions.md)，当前0个设计阻断。
- [x] 实现、测试、机械/独审及五维度退出证据齐全，2026-10-19本地验收见[C审核](../../../audits/sgm-5-token-management/c-audit.md)。
