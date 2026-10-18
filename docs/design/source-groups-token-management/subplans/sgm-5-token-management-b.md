# SGM5 B 令牌操作持久化与恢复

2026-10-18，基线`ec17fc9`。承接[SGM5总体细化](sgm-5-token-management.md)及连续实施授权。本批只实现应用库中的身份、意图、执行证据和确认，不发HTTP、不增加调度器；C负责把这些接口与已验收A、组/身份就绪及原worker组合。开发不接触现行服务。

## 1 范围和复用

G4/G6由B保存可恢复操作、A提供固定owner原生能力、C负责受控执行共同实现。B复用CatalogStore的唯一写者、事务、私有路径、备份、poisoned停写，复用OperationId、ControlErrorCode、OperationPhase、ExecutionOutcome和CreateState。不另建操作表、预算、登录器、数据库或消息队列。

应用库v7→v8，保留旧DDL文件。扩展原control_operations的kind及target_type，追加managed_tokens、token_operation_targets、token_execution_attempts。操作phase/error仍只归原操作表所有；三个侧表分别保存长期身份、不可变目标及历史收据、实际执行尝试，不能把期望当原生状态。目录快照wire1及NativeTokenView wire1不变；OperationView升级wire3（新增kind/target枚举，字段不变），新增TokenIdentityView wire1。

已确认的NewAPI行为不重新猜测：创建无ID回执、默认启用、名字不唯一，来自A的固定版本源码与15项隔离实证。B不复制渠道的禁用创建流程，不把持久操作测试叫原生服务测试。测试中所有数据库均为临时私有库。

## 2 数据结构与唯一权威

共享类型由catalog/tokens持有，消费者为C、未来安全面板；owner及原生状态继续复用A的枚举，不定义同义副本。

| 类型或表 | 字段与约束 | 生命周期 |
| --- | --- | --- |
| ManagedTokenId | 独立正i64 newtype，wire为十进制字符串 | SQLite分配；删除后保留，不复用 |
| TokenIdentityView | schema_version、token_id（本地）、native_token_id可空、owner_user_id、owner_scope、origin、group_id可空、explicit_group、display_name、revision、create_state、created_at、deleted_at可空 | 白名单投影；无key、masked_key、quota或原生状态缓存 |
| TokenOrigin | created / imported | 从create_operation_id是否存在推导，不存第二份判别字段 |
| managed_tokens | token_id、owner_user_id、owner_scope、native_token_id唯一可空、group_id可空、explicit_group、display_name、revision、evidence_revision、create_operation_id唯一可空、create_group_revision可空、expires_at可空、created_at、deleted_at | 创建来源身份/固定spec不可改；revision为逻辑命令版本，evidence_revision使旧读取失效；最多10000条含删除历史 |
| token_operation_targets | operation_id主键引用原操作表、token_id、token_revision、desired_enabled可空、confirmed_at可空、receipt_json可空 | 每个token操作恰一行；确认时间与安全收据同时存在；成功收据永不因后续操作重写 |
| token_execution_attempts | token_id、sequence、operation_id、token_revision、action、state、error_code、started_at、finished_at | 主键token_id/sequence；同token最多一个prepared；序号来自evidence_revision，历史不删除 |
| TokenCommand | CreateToken{owner,group_id,expected_group_revision,display_name,expires_at}、SetTokenEnabled{token_id,expected_revision,enabled}、DeleteToken{token_id,expected_revision} | 仅进程内可信输入，无Deserialize、任意原生patch或明文；kind复用OperationKind |
| TokenWork / TokenTicket | 私有目录instance、操作ID、本地token ID、命令版本、证据版本、owner、原生ID/固定spec、动作；Work另带加载时间 | 无Deserialize；Work为回读栅栏，Ticket仅由提交成功的prepare产生 |

新增OperationKind为create_token/set_token_enabled/delete_token，TargetType增加token，expected_targets固定1，token操作不使用Partial/Superseded。其他kind/target组合不变；全部当前OperationView输出wire3，但旧业务字段及历史收据内容不改。序列、版本、时间使用原JSON_SAFE边界；原生ID可用完整正i64并以字符串输出。显示名最多256 UTF-8字节、非空、无首尾空白、不含控制字符；显式原生组最多256字节，超界拒绝不截断。expires_at可空，否则为非负安全秒值；新创建提交及prepare要求未来，历史查回不要求。

创建关联名为`qt-t-`加SHA256域分离输入（紧凑UTF-8 JSON数组`["qt-token-create-v1",catalog_instance_id,operation_id]`）的前16字节小写hex，长度37；不含显示名或凭据。持久保存创建操作ID，读时确定性重建关联名；数据库不增加可失配的第二关联字段。创建记录必须issuer、已知组、固定非空/非auto原生组及创建组版本；导入记录不伪造创建意图，原生ID必须已有。

同实例原生token ID全局唯一；既有映射即使已删除也不改owner或重新认领。相同owner_user_id不能在目录同时标成两种scope。group_id仅在显式原生组恰好匹配目录组时记录；legacy的空组/auto/不受管组保留原文及空映射，不默认为default。账本仍按原生token_id归属，不按新增本地ID重写事实。

CreateState不是新的持久列：有原生ID→Resolved；无ID且存在非NotSent创建尝试→MayHaveBeenSent；否则Never。操作phase/error不复制到身份表。native状态/余额/模型限制仍从A读取。

## 3 错误与顺序

所有入口先检查Store可用及输入结构；规范payload摘要复用sha2并带token命令域。相同OperationId重放先于当前revision、组就绪及“有效期仍未来”检查；同载荷返回原操作，不增加身份或尝试，异载荷IdempotencyConflict，包括与旧渠道/策略操作ID冲突。

InvalidInput表示坏输入，NotFound表示本地身份/操作不存在，RevisionConflict表示旧版本或票据，BindingDrift表示身份/固定spec冲突，NotReady表示组未就绪、未确认身份或其他未完成操作。容量不足QueueFull。SQL/损坏沿原Storage/CorruptData；提交结果不明沿原poisoned停写，不发布票据或安全视图。错误不携带SQL、显示名、正文、URL或密钥。

一个token同时只接受一个未完成写操作。Queued/Applying/Waiting/Unknown阻止新的相反命令；Failed只有不存在prepared/uncertain非物化写时才释放阻塞。Succeeded历史不重写。此串行策略不引入抢占/补偿状态机；不存在“先承诺取消创建，再忽略晚到活动token”的分支。

| 证据 | 创建 | 启停 | 删除 |
| --- | --- | --- | --- |
| prepare已提交，未收到终止证据 | Applying；禁止第二次prepare | Applying；禁止并发写 | Applying；禁止并发写 |
| 可信NotSent | Failed，若所有创建尝试均NotSent则Never；显式retry后可再准备 | Failed，显式retry可再准备 | Failed，显式retry可再准备 |
| 明确成功ACK | Waiting，只能查回 | Waiting，仍需写后回读 | Waiting，仍需严格缺失回读 |
| 明确拒绝响应 | 仍保留May并查回，不把响应当未发送 | Failed，不当成功 | Failed，不当成功 |
| 超时/断连/取消/重开prepared | Unknown，只查回，Missing也不重发 | Unknown；匹配GET也不解除未决写 | Unknown，可严格缺失确认 |
| 唯一正确回读 | 绑定身份并确认创建事实（可已过期/耗尽） | 无未决写且状态匹配才确认 | 单条GET严格缺失才确认 |

非确认事件投影按以下优先级，避免phase和attempt出现两套真值：①已有成功收据保持Succeeded；②仍有未物化prepared保持Applying；③仍有未决uncertain，或本次发生明确拒绝/未收敛读取且创建已May未绑定，取Unknown；④本次可信NotSent或明确失败响应且不受②③覆盖，置Failed；⑤只读problem使用既有terminal分类（auth_failed/binding_drift/ambiguous_create/remote_rejected/invalid_input/not_found）→Failed，其余错误→Waiting；⑥明确成功ACK但未确认→Waiting（包括May未绑定的创建）。只有submit/retry产生Queued。Unknown优先于终止错误，不能通过Failed释放仍未决的写。created且已有原生ID、delete且已确认删除的尝试视为已物化，豁免其历史不确定屏障；不伪造其HTTP终止证据。

未决状态写在丢失原尝试终止证据后可能永久Unknown；重试不能靠等待时长或匹配GET清除它，后续相反命令仍NotReady。B不提供“强行认为未发送”。这是沿既有渠道执行证据的保守边界，不承诺网络exactly-once或无条件恢复活性。删除缺失可收敛依赖原生实例不被替换/回滚且ID不复用；迟到同ID删除不复活资源。

默认启用的创建一旦发出，后来的组就绪变化不能撤回远端写。B可记录已发生创建，但Succeeded仅表示唯一归属已确认，不表示当前可用/可复制；C须另核当前owner、组、版本与有效期。未确认归属不开放复制，B不自动发补偿删除或禁用。受托管身份/原生配置不被其他写者并发改动仍是前提；不满足时报告漂移。

## 4 函数细化与正确性论证

### T5B.1 类型和纯校验

`canonical(command) -> (kind,digest)`校验正身份、scope、名称、数字结构；不读取墙钟或组状态，Create期限只验值域。摘要输入为紧凑UTF-8 JSON数组`["qt-token-command-v1",kind,payload]`；Create payload按owner_user_id、scope、字符串GroupId、expected_group_revision、display_name、expires_at排序，Set按字符串token_id、expected_revision、enabled，Delete按字符串token_id、expected_revision。字段不排序改义。错误无副作用；同命令在任意时间产生相同摘要，故过期重放仍等价。`create_name(instance,operation)`检查固定instance格式与已有OperationId契约后散列，输出固定合法关联；数据库唯一原始操作ID及名字全量核对避免静默接受碰撞。

`parse_owner/row/view/receipt`逐字段强类型读取；非法持久值归CorruptData，不填默认0。owner枚举复用A；新origin从可空创建ID推导；CreateState从已存ID/尝试事实推导。View只构造schema白名单。收据字段固定为format_version、token_id、native_token_id、owner_user_id、owner_scope、token_revision、evidence_revision、status可空、missing、confirmed_at，不含原生任意JSON或key。简单newtype/accessor为trivial转换，其余校验分支覆盖非法/合法两路。

### T5B.2 迁移与完整性

`migrate_v7`前置旧v7经只读完整校验并一致备份。原控制父/子表重建沿已有helper；v8只放宽新kind/target组合，不放宽旧组合。建立三个侧表与pending索引，保留catalog_instance_id，meta/user_version同事务为8。任何失败回滚；commit未知停写。新表初始为空，不扫描NewAPI或导入token。

`verify_tokens(conn)`校验全部身份/owner/创建关联、固定组、容量、操作目标恰一行、kind/字段组合、版本不超当前、attempt动作与命令相符、sequence≤证据版本、prepared唯一、state/error/时间搭配、已确认收据与历史身份/版本一致。身份还须反向核对：deleted_at对应成功Delete及同刻missing收据；created已绑定对应至少一条原创建非NotSent尝试和成功收据；逻辑目标版本唯一且连续，created为1..revision、imported为2..revision（导入初始revision1无操作合法），失败也计入历史。证据版本及attempt sequence不要求连续。禁止token操作混入旧membership targets，反之亦然。逐行有界内存校验；循环由有限SQL结果终止，不把10000条身份上限错误套到历史操作总量。旧版本不查询不存在的新表。

`open`在旧库只读校验/备份/迁移后校验v8，再执行既有成员恢复与token恢复；不启动网络、策略或周期。`recover_token_executions`把prepared保守变uncertain、推进对应证据版本；未确认操作投影Unknown，已Succeeded操作及历史收据永不降级，同事务。counter到上界则拒绝，不部分恢复。组revision/ready后续变化不使历史创建记录损坏，verify只校固定组身份及创建版本合法，不要求它等于当前版本。原open已按版本逐步迁移，不新增整链原子性承诺。

论证：旧数据验证先于写，父子记录镜像保留，v8新表为空，故旧历史不被新token投影重新解释；恢复只降低未知写的确定性，不生成成功或Never。

### T5B.3 身份登记与只读投影

`register_token(owner, native_view, display_name, now) -> TokenIdentityView`只接受A已核验的显式单ID事实，并再核schema、正native ID、相同owner/scope、显式组上界及本地display_name。已知原生ID且owner相符返回原身份，不重写历史或已删除身份；不符BindingDrift。若native名称撞本库未决创建关联，拒绝普通登记，要求走该创建的唯一查回。新登记按显式组等值关联目录组（空/auto/未知为None），保留原生ID，显示名由显式本地标签提供；原生名称可为空且不被改写。事务中容量/重复/owner scope检查先于插入。

`token_identity(id)`与`list_token_identities(after)`读取安全投影，列表按本地ID升序keyset最多100条，包含删除历史；仅当前页，不声称原生列表完整。无key/配额/实际可用状态缓存。`token_reveal_target(id,expected_revision)`仅对已确认创建或显式登记、未删除且无未完成操作返回私有owner/native ID，不能直接取明文；C仍需当前准入和最终guard。

论证：原生ID唯一索引及拒绝错owner阻止重新认领；历史行不删除保证账本原ID不串到新身份；登记不会绕过未决创建确认。读投影白名单不可能序列化完整key。

### T5B.4 提交及重放

`submit_token_operation(id,command,now)`在同一事务先用统一operation_id检查摘要：存在且同载荷直接返回旧OperationView；不同冲突。Create新操作再验issuer、有效期未来、组ready及expected_group_revision，容量/owner一致后分配本地ID并保存固定spec；新记录revision=1，evidence_revision=0。Set/Delete要求身份存在未删除、revision匹配、无未完成操作及未决非物化尝试，再提升revision。三种操作均写原control_operations、恰一token target，phase=Queued/targets=1。关联create_operation_id使用延迟外键，使先分配本地ID再建立操作在同一事务内闭合。提交前构造输出，失败不返回未持久身份。

`operation_projection`按target_type选择对应收据表计数；token固定1，Succeeded必须confirmed=1。旧路径仍只数原membership targets。`verify_control`保留旧约束，并把token交给明确的新验证；不能因总计查错表出现Succeeded/0。

论证：摘要重放在易变前提之前，避免过期/旧revision产生第二次创建；身份/操作/目标原子提交，不存在外部可见孤立身份。逐token阻塞检查排除相反在途意图，不需要新增supersede算法。

### T5B.5 工作快照与发送前准备

`token_work(operation_id,now)`在读事务取得身份、目标、操作、当前组及证据版本，生成无Deserialize的Work；加载时间由调用方可信时钟提供，值域校验。`pending_token_operations(now)`只选token Queued/Waiting/Unknown，按updated_at/operation_id排序最多64；Waiting/Unknown距上次有效处理至少30秒，使用减法边界避免溢出。使用部分索引，不扫描全部成功历史；结果只是eligible快照、不做claim。C在一轮内去重ID，网络失败必须调用note，不能以重复调用pending代替完成一次处理。

`prepare_token_execution(work,now) -> TokenTicket`重核目录instance、token/操作ID、命令revision、evidence_revision和当前phase=Queued，禁止prepared或未决非物化写。创建还必须Never、当前组仍ready且与原组版本相等、期限未来；不以旧ready快照准入。Set/Delete要求原生ID已绑定。递增evidence_revision、持久prepared尝试并把操作置Applying后commit，成功才返回私有票据。不直接调用A，也不从前端布尔值生成准入。

`token_send_allowed(ticket,now)`为只读最终准入接口：完整票据匹配历史attempt，当前逻辑/证据版本仍等于票据、操作仍Applying且attempt仍prepared才为true；create另核当前组ready/创建组版本及未来expiry。已结算、被新证据覆盖或资格过期返回false，坏票据/存储错误返回安全错误，均无写。不核原生当前role、不生成远端就绪证据。

论证：prepare提交先于票据，任何进程退出最多留下保守prepared；再次prepare看见phase/尝试事实必拒绝。证据版本使prepare之前发出的GET失效。C须把token_send_allowed和真实就绪检查挂到原A最终guard，在预算等待/401后再验；本批不以fixture代替该组合保证。

### T5B.6 交付结算和回读确认

`settle_token_execution(ticket,outcome,now)`校验完整票据属于同实例/操作/历史attempt；同state/code重复结算为no-op，其他已终态不改判。仅prepared/uncertain接受可信终止证据。写attempt state/error/finish并推进证据版本，不生成确认收据。NotSent不得否认已经物化的创建。当前操作phase按§3表更新，已有Succeeded收据不被迟到响应改写；每项证据只影响自己操作，不确认未来revision。

`confirm_token(work, observation, now)`要求Work完整版本仍当前，且0≤now-loaded_at≤60；时钟倒退、超60秒或旧证据零修改。C必须在取得Work后再开始对应A查询，不可套用缓存。Create观察为CreationLookup：Never不得认领；Missing仍无ID，Ambiguous不选ID，按§3优先级记Applying或Unknown及安全错误，并更新updated_at；One再逐字段核owner/名字/固定组/期限/unlimited/model/IP/cross-group约束，允许当前expired/exhausted。绑定ID前查全局历史唯一性，原生ID及安全收据/confirmed_at/操作Succeeded同事务。确认创建/删除可将对应prepared记uncertain并保守设置finished_at，以释放prepared槽；历史写因物化豁免，不伪造responded。Set观察必须单ID存在、owner/原生ID/显式组一致且状态匹配；任何prepared/uncertain状态写阻止成功。Delete观察必须单条严格缺失，不能用列表缺项；历史token置deleted_at并保留ID，安全missing收据与Succeeded同事务。删除不写源/组/成员/账本。

每次有效confirm均更新updated_at并推进evidence_revision，包括Missing/歧义/字段漂移/错owner/状态不匹配/尚有未决写，旧Work不能再次使用；成功操作重放返回历史收据。有效但不收敛的读取按§3规则保存安全诊断并返回未完成OperationView，不写部分身份；输入观察类别与操作不符是InvalidInput零修改。网络/解析失败由C必需调用note，成功或失败处理时间均不倒退。所有支路均不返回或保存原生key。

`note_token_problem(work,code,now)`同样核完整版本及0..60秒窗口，按§3完整优先级更新phase/error/updated_at；有prepared时Applying、有未决uncertain时Unknown，不以终止错误绕过。不能覆盖成功或凭错误撤销历史绑定。此诊断写不改变命令revision，成功更新后的旧Work因evidence_revision递增而失效。

`retry_token_operation(id,now)`先保留Succeeded历史no-op；其余必须token未删除、目标token_revision仍当前且没有其他活动操作，否则RevisionConflict零修改。prepared返回NotReady；未决状态写返回TransportUnknown且保持原状；未决Create/Delete仅更新Unknown及处理时间/证据版本，保留May和原attempt，不重新准备。无未决写时，Queued为no-op，Failed/Waiting/Unknown转Queued并推进证据版本；token合法库不含Partial/Superseded，遇到属于损坏。原通用retry_operation按token target显式分派，不落入membership空目标路径。C可对显式重试立即做一次查回，自动pending仍遵守30秒间隔；其自动重试次数由C有限控制，B不自行发送或循环重试。

论证：ACK不能满足confirmed_at条件，严格回读与当前证据版本共同给出“目标已观察到”的事实。未知创建不会回到Never，未知状态写不能被匹配GET洗白；删除是单调终态，晚到同ID删除不能破坏已确认缺失。历史事实和当前可用性分离，不声称消除原生无CAS竞态。

## 5 不变量与跨进程约束

- B1：一个operation_id只有一个规范载荷；一个原生ID只有一个历史本地身份及owner。
- B2：无提交成功的prepare票据不允许C发写；未决create从不重新准备。
- B3：Succeeded恰有一份对应目标的安全回读收据；ACK不等价成功；旧证据不确认新版本。
- B4：一个token至多一个prepared及一个未完成逻辑写；未决一般写不被失败/重试消除。
- B5：token删除/登记不修改来源、组、成员、账本、旧操作历史；无完整key入库或普通序列化。

原目录单写者承担以上事务不变量，A承担固定owner及有限HTTP，C承担真实实例/角色/组就绪、时钟、发送guard、取消后结算及请求预算。没有新增并发锁；事务内无await。B连接/会话/网络超时N/A（不执行网络）；C必须沿A连接3秒/总10秒、一次401恢复、1req/s突发2及64槽，不能另开预算。B断进程依靠open恢复prepared；磁盘提交未知停止写入。环境必须保持同一原生实例及ID空间，不把应用UUID当原生数据库身份探针。

## 6 验收与实现门禁

基本与反例：输入/容量/溢出零副作用；重放优先于过期/旧版本；跨kind同ID冲突；issuer/legacy分离；空/auto/未知组及空原生名称登记；重认/名称抢占拒绝；每个prepare/settle/confirm崩溃窗口；Missing/歧义/错spec/过期唯一查回；ACK不成功；未知状态写阻塞相反命令及匹配GET；删除严格缺失与历史保留；迟到/重复票据；60秒边界/61秒/时钟倒退；有效失败读取刷新30秒间隔；最终send guard在settle/ready/revision/expiry变化后失效；副作用白名单与脱敏；损坏/高版本拒绝；事务故障与poisoned。必须含A(r2)失败→B(r3)成功→retry A零修改，以及prepared创建/删除先确认→重开仍保持Succeeded两条具体轨迹。

至少5组各128 cases PBT：幂等与独立摘要/关联参考、创建交付/重开轨迹、状态操作/证据栅栏、legacy身份/删除/历史隔离、复杂旧库迁移镜像。参考模型不调用生产状态决策生成期望。迁移必须覆盖有成功回执、未决尝试、来源策略、观测历史和账本的v7库及历史版本链，不只测空库；旧expected版本变化只更新夹具边界，不削弱旧断言。

独立release性能固定基线：10000条imported身份、零token操作/尝试历史的完整keyset投影与verify_tokens≤5秒；含10000条旧控制操作的指定复杂历史夹具v7→v8迁移≤5秒。历史条数本身无上限，校验成本随历史增长；这两项不声称任意10000条created身份及其全部历史都在5秒内。精确入口清单及空/全ignored负例、JSON schema/枚举/Step/旧DDL与算法冻结，至少8类可编译变异。默认debug/release全量、clippy/build、全部独立性能、workflow §6独审和五维度均通过才退出。性能不与自己的其他测试/编译并跑。资源沿既有两编译job/低优先级；不跑生产服务。

- [x] 数据类型、分支/退出、副作用、callee与终止上界已列。
- [x] 网络及当前就绪责任明确留给C，无第二状态权威。
- [x] 未知状态写的保守可用性限制显式列出。
- [x] 独立设计审通过后开始实施；最终实现独审0未关闭问题，证据见[B审核](../../../audits/sgm-5-token-management/b-audit.md)，不以A结果替代。
- [x] B本地退出：debug/release各603通过，20项独立性能、10类编译变异、机械/clippy/build及五维度完成；C、UI和运行发布不在本批完成声明内。
