# SGM5 访问 key 管理实施细化

日期：2026-10-18，基线 eb0b727。承接已确认架构及用户连续实施授权，保持 research/source-groups-token-management 分支；不接管、部署或重启现行服务。

## 1 范围和顺序

一个访问 key 一个组，组只有一种来源类型；同一 key 同时支持 OpenAI/Anthropic 协议，不按协议复制 token。NewAPI 保持 token 权威，应用库只存操作意图、归属、显示名与历史身份，不另存完整访问 key。

| 小步 | 责任 | 退出边界 |
| --- | --- | --- |
| A 原生接口 | 精确 owner 校验、列表/查询、单组创建、启停、删除、按需复制、组可用性读取 | 假HTTP/PBT、固定版本原生隔离实证、机械及独审；没有持久操作或页面 |
| B 持久操作 | 来源目录同库保存 token 身份与稳定创建关联、版本/未知交付/重开/幂等、旧 token 兼容 | 未知创建不重发；回读才确认，完整 key 不入库；另写函数级细化后实施 |
| C 组和服务组合 | 普通签发身份就绪、组注册/可用组读取合并、引用保护、worker管理入口 | 共享原预算和控制面；签发身份/组/成员就绪后才能创建；另写细化和组合验收 |

本次先实现 A，不把原生接口 ACK 当作 B/C 已完成。运行配置、身份安装/额度准备、HTTP/三页面和生产切换分别在后续组装/SGM6/SGM7完成。普通签发身份缺失、被禁用、变成管理员或余额不可用时阻止新签发，禁止回退管理员创建。此步不创建账号、改余额或改全局赠额。

## 2 已核实协议和复用

固定 NewAPI v1.0.0-rc.20 二进制 SHA256 `90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205`，对应此前固定源码 `6ce7305cd36f16506fb6a2c3c524a5a318539ba7`。`scripts/probes/newapi_token_probe.py` 在全新临时库/假上游最初验证13项：普通账号、可用组、禁止管理渠道、创建无ID、精确关联查询、创建忽略禁用值、列表打码、owner隔离/不存在响应、按需取原始key、双协议越组拒绝、整组不可用不越组、启停保留限制、重复关联不唯一、单ID删除重放；部分性质合并为一项检查。最终扩展为15项，增加Bearer的GET self拒绝错header，以及cookie的POST create在账号/header不一致时拒绝创建的验证。

`/api/token/` 仅访问当前登录账号的 token；管理员会话不能代管普通账号 token。普通账号 role=1，管理员旧账号 role=10/100。创建忽略输入 status，实际默认启用，且不返回ID；名字无唯一约束。B必须先保存稳定标识、唯一精确查回并核字段，确认前不开放复制；不能用“最新一条”或重发未知 POST 补救。[Token控制器](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/token.go)

单条不存在是HTTP200、success=false、message严格为`record not found`，不是404。只给token单条GET增加显式、窄范围的缺失读取模式；其他错误不解释为空，其他管理请求仍严格校验success。[Token存储](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/token.go)

普通 token 附加渠道编号后缀被拒绝；显式组通过账号可用组及注册组检查。管理员旧 token 的特殊路由权限保留并标注，不计入新普通 token 隔离保证。关闭 cross_group_retry，不设置auto，也不改模型名。[原生鉴权](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/middleware/auth.go)

复用 `NewApiClient::with_budget`、`ManagementTransport`、`ManagementRequest`、`DeliveryEvidence` 和 `SecretString`。两身份独立cookie/认证锁，共享同一 ManagementBudget；不创建第二预算或登录实现。状态写用 `PUT /api/token/?status_only=true`，仅发送id/status；删除用原生`POST /api/token/batch`且ids恰好一项，不扩大通用传输层的DELETE白名单。status_only内部仍由原生Update实现，本工具不声称修复NewAPI并发消费与其额度字段写入的内部竞态。[Token路由](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/router/api-router.go)

## 3 类型与接口

以下是A的内部类型及可序列化安全投影，不是新HTTP API。跨层消费者为后续B/C和面板；原CatalogSnapshot及OperationView版本不变，A不改数据库。

- `TokenOwnerScope`：`issuer | legacy_admin`；`TokenOwner { user_id:i64>0, scope }`固定期望账号。不能以token列表中的user_id自动切换会话。
- `TokenOwnerProfile`：id、role、account_group、quota，仅为本次读取事实，无跨请求授权有效期。role和活动状态必须匹配owner；新签发另需quota>0。
- `NativeTokenView` wire=1：token_id、owner_user_id均十进制字符串，owner_scope、name、masked_key、explicit_group、effective_group、status（enabled/disabled/expired/exhausted）、created_at、expires_at可空、remaining_quota、used_quota、unlimited_quota、model_limits_enabled、model_limits、allow_ips可空、cross_group_retry。数值额度是NewAPI内部quota，不是token计量。
- `NativeTokenPage {items,total,page,page_size}`：每页最多100、total≤10000，页号从1起且≤100，ID严格降序不重复，条数符合该页计数。它只是当前页，不能以第一页缺项证明不存在，不宣称跨页同快照。
- `TokenCreateSpec { correlation_name, group, expires_at }`：关联名固定`qt-t-`加32个小写十六进制字符，由后续B从目录实例和OperationId生成；不接受任意原生JSON。组复用目录valid_group，拒绝空/auto/多组。新key不限NewAPI token quota、无额外模型/IP限制、cross_group_retry=false；可选有效期结构上为非负安全秒值，只有新建时要求它仍在未来，历史查回允许已过期。
- `CreationLookup`：Missing、One(Box<安全view>)、Ambiguous；仅命中项拥有投影，避免所有空分支都携带248字节枚举。不存在和重复不生成ID。单条还须与spec的name逐字相等、group/有效期/固定限制一致，否则InvalidResponse；不能把SQLite的LIKE大小写不敏感查询当精确字节匹配。用户显示名由B本地元数据承接，不用改关联名找回创建。
- `RevealedToken`：私有SecretString，不实现Debug/Serialize，仅显式`expose()`返回单一`sk-`前缀形式；不复制到普通view或任何持久记录。
- `ManagedTokens<'a>`：借用已有transport、固定owner、可选原发送guard，无会话/预算/后台任务的副本。

JSON schema固定安全view字段和枚举，未知原生字段不进入投影。native响应中的key无论是否打码都本地重新掩码；绝不信任“列表应该已打码”。模型/IP/名称等原生文本仅保留为数据，未来UI按文本呈现。

## 4 错误与时序

复用ManagementErrorKind；坏输入InvalidInput且零网络，owner/id/role不匹配AuthFailed，新签发余额或可用组不足RemoteRejected且未发送写；结构、页数、token owner/id、字段值域不符InvalidResponse。不记录URL/正文/key/登录字段。写前profile/组/GET失败一律标NotSent；真正写的超时/断连/畸形回执沿原Uncertain，不能变成安全重试。native明确拒绝保留ResponseReceived。

只有显式token lookup GET可接受严格的原生缺失响应；不能吞掉任意success=false、HTTP404、鉴权错误或空data。token状态与来源状态无联动，删除key不删组、来源、成员或账本。

所有token业务请求另携带私有expected_user_id，实际New-Api-User始终固定为owner.user_id；每次prepare（含401恢复后）若Session ID不同则AuthFailed/NotSent。固定header还能让等待期间换掉的cookie在原生handler前被拒绝；Bearer也覆盖错误静态header。默认旧请求None不改变原行为。所有写、GET和复制继续复用原最终准入guard，预算等待或401恢复后guard失效不得发送。A不自行构造业务版本许可；B/C负责持久版本和guard的依据。一次profile读取不能消除同ID在其后的外部角色变更竞态，依赖托管身份不被外部并发改角色/凭据，部署期须明确这一前提。

## 5 函数细化及论证

### T5A.1 输入与安全投影

`validate_owner/id/spec/page`：纯函数，先检查正ID/范围；spec只按固定关联名、单组、有效期安全值域做结构检查，历史lookup不得拒绝过去expiry。create另检查有效期仍在未来，最终发送guard也检查该条件；等待期间过期时NotSent/RevisionConflict，不发POST。调用方在任何网络前做结构验证，失败无副作用。`parse_profile`核必需类型、同owner、匹配role和status=1，再返回只含白名单的profile；其quota不被当余额保证。

`parse_token(value,profile,owner)`：严格读取字段、正i64 ID和同owner、status1..4、安全秒值；remaining_quota允许有符号安全整数（含消费后负数），used_quota为非负安全整数。expires=-1映射None，空group只继承本次account_group。key转为掩码，≤4全遮、≤8保留首尾2、其余保留首尾4且中间固定星号；返回不带unknown/key原文。字符串受原有限HTTP正文约束，数值无默认0。纯返回构造，无写。

`parse_page`：核回显页码/页长/total≤10000及精确期望条数，再逐条parse_token、核ID严格降序唯一。循环上界100，任一行失败整页失败，不返回部分成功。schema输出由同一安全view生成，不重定义状态枚举。

论证：类型/身份/计数先于构造，所有出口是完整白名单或错误；脱敏发生在唯一view构造点，额外原生字段不会被序列化。元数据字段中管理员主动录入的秘密不属于自动key脱敏的保证；实现不记录这些文本。

### T5A.2 owner和有限读取

`profile()`发一次GET self，按上述函数核对；`available_groups()`只读GET self/groups，验证map值含非负有限ratio（auto特殊项只忽略，不赋予创建资格），不写配置。创建前先profile再groups；只要任一步失败就无token POST。

`list_page(page)`先校验page/owner，再profile，GET固定100条页并parse_page；`get(id)`先校验，再profile和显式token lookup请求。仅严格缺失→None；存在则parse_token且返回ID恰等于请求。`lookup_created(spec)`先结构校验（不要求expiry在未来）、issuer scope、profile、关联名搜索，页长2；0→Missing，>1→Ambiguous，1→name逐字相等且所有固定spec字段匹配才One。名字不唯一，不能取首项；可找回现已过期或耗尽的历史创建，不要求当前status仍enabled。

终止性：每个方法固定至多三个业务请求，无分页循环/业务自动重试；原有初始化登录及明确401后的单次认证恢复另计，仍受原传输契约。token页循环上界100，groups迭代上界由1MiB正文限制。读失败不改远端token或应用库，返回旧状态的责任由上层承担，不伪造空集合。

`ManagementRequest::token_lookup(id)`只构造正ID的固定token单条GET，并设置私有缺失标志；常规new默认false，不改变已有请求。`ManagementResponse::token_missing`只在status=200、success=false、message逐字等于固定原生缺失值且data缺省/null时为true，其他结构/错误不匹配。原send_fenced只对该专用请求和严格响应跳过普通成功包络检查，身份、限额、401及最终guard全部照旧。该标志不能从前端反序列化，不通用于写或其他资源。

`ManagementRequest::for_user(id)`前置正i64，后置只增加私有expected_user_id，不改路径/body/语义/限制；不合法则InvalidInput且无网络。ManagedTokens统一send在使用transport前给每个业务请求绑定owner。`prepare_request`先沿原Auth生成headers，再仅在expected_user_id存在时核Session(id)一致并覆盖New-Api-User为期望ID，失败不返回builder；原生鉴权对cookie/Bearer实际ID与header作等值核验。故不同principal不能因重登被接受，同principal仍恢复，默认None的旧请求不受影响。详见[身份漂移修正](../../../fixes/newapi-token-fix-auth-principal-drift.md)。

### T5A.3 有限写和按需复制

`create(spec)`前置为上层已持久化意图、组/类型/成员就绪且给出有效guard；A再核输入、issuer scope、当前profile/余额和可用组。构造唯一白名单payload后Create语义POST。只有明确应用ACK返回()，不返回ID或key，也不宣称创建已对账。

`set_enabled(id,bool)`校验、profile并GET存在token，再PUT status_only、只发id/status；验证返回data对应ID和owner、状态达到请求值、组与限制元数据未变，但仍由B持久确认。到期/耗尽不能启用的原生拒绝原样归为RemoteRejected。`delete(id)`校验profile再单ID batch POST，核返回count∈{0,1}，不以count替代后续GET缺失确认。

`reveal(id)`校验profile、GET存在且身份正确，再POST key读取；仅接受裸ASCII字母数字、16..128字节的完整值，拒绝星号、空值、后缀或嵌入前缀，包装一次sk-。异常只输出安全错误码；完成/取消释放原传输请求槽，不保存副本。

`request`共用已有transport/send_fenced，token层额外要求status=200且success=true；201/202等其他2xx不能作为token ACK，返回InvalidResponse/Uncertain。token_lookup的精确缺失是唯一窄例外，不改原transport的通用2xx语义。写前错误map为NotSent，写后的结构错误维持Uncertain；每个await释放原budget admission，未增锁顺序。取消资源验证依赖原transport的64槽回收测试，加A调用链取消组合验证，不新加跨模块诊断API。

`matches_spec/same_settings`只比较固定创建约束或状态更新前后的静态配置，不把消费过程中合法变化的remaining_quota/used_quota当漂移。mask、数值/文本取值和时钟辅助函数无外部写入；输入错误在网络前，响应错误维持原交付证据。new/fenced/expose与字段映射是简单构造/只读访问，不复制会话、预算或凭据持久副本。

论证：scope/owner核验排除回退或认领别人的token；白名单限制每个请求作用域；A不做业务重试，只有明确401后原transport允许认证恢复并重放一次，取消不产生后台重试。A不存意图，因此无法独立承诺重启幂等，该责任明确留给B。

### T5A.4 分布式契约和复用边界

连接池和独立身份cookie由原transport所有，关闭连接不清业务意图；connect3秒、每请求含正文10秒，原1MiB单体/8MiB列表限制；原64请求槽、单实例1req/s突发2。管理写Control、列表Observe，共用预算；外部429沿原退避，不增独立轮询。只允许明确401后的原单飞登录重放一次，其他错误不做业务重试。跨账号无混用；断连/取消可能已交付，慢响应不持目录锁。A无持久状态或跨进程exactly-once承诺。

## 6 验收

基本/负向：issuer/legacy列表并存；新创建仅issuer；空/auto/多组/坏关联/过期/大页在网络前拒绝；身份变更/余额为0/缺组不写；错ID/错owner/缺字段/异常页拒绝；key本地重打码及复制负trait；精确查回0/1/多条；启停payload不带其他字段；单ID删除和缺失回读；不把任意false/404当缺失；超时/畸形写回执保持Unknown；guard在预算或401等待期间失效不写；两个会话同预算但不串cookie。

至少3组128 cases PBT：独立掩码/字段投影参考、创建白名单/单组限制、混合owner/响应ID及状态写轨迹。至少6种有效可编译变异；精确测试清单、空/全ignored负例、schema/枚举/Step/冻结范围机械校验，独立审和五维度。默认debug/release回归；新增最大100条投影解析串行release≤2秒，保留原17性能门禁；clippy/release构建。固定原生探针重复验证权限路径，不使用真实上游。

- [x] 函数推导、分支和全部退出点已列出。
- [x] callee的身份、交付、预算和正文约束显式承接。
- [x] token页循环≤100、groups迭代受正文限制，固定业务请求步数（认证恢复另计），无后台队列。
- [x] 实例、普通身份、独占角色/配置写者及B/C前置边界明确。

完整SGM5退出仍需B/C的迁移、幂等、引用保护和组合验收；A通过不得标记完整key管理或页面已交付。
