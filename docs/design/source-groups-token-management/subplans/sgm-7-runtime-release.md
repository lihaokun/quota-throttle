# SGM6 C 与 SGM7 运行交付

2026-10-19，基线 ab59a88。用户要求一次性完成剩余开发，并指定在 fermat 实验；本批连续完成接线、隔离组合验收和全新部署，不再逐子步骤等待确认。既定来源、组、Key 与三页功能不扩展；本地现网、主工作区未提交修改以及 fermat 既有 3000/3001 服务不动。

## 1 范围与复用

新增显式 `serve <config>` 入口，在同一进程组合已有 CatalogStore、ReconcileWorker、PanelServer、来源探针、统一采集和补归属。旧 up/run/sync/down 不改行为；不自动导入旧 keys，不同时启动旧 Orchestrator/status。新配置仍复用 Config 的连接和 GLM 参数；根层 `keys=[]`，新增 `[source_groups]` 只被 serve 消费，不给旧 Config 增加运行期字段。

G1–G3 由既有 worker/策略/collector 完成，G4–G6 由已验收面板完成。本批只提供生命周期和启动材料。一个应用库、一个 worker、一个 collector、管理员和普通 issuer 共用一个 ManagementBudget；没有新调度器、数据库、推理代理或后台任务表。

## 2 配置与私有类型

`RuntimeOptions` 位于 runtime 模块，只用于启动：catalog_path（绝对路径，文件名 catalog.sqlite3）、可空 panel_origin、可空 public_api_base_url、可空 issuer、可空 legacy_admin_user_id。issuer 含 user_id 正整数、username、password；固定账号与管理员不同，不继承管理员 token/headers。内层拒绝未知字段，不实现泄露凭据的 Debug/Serialize。缺 issuer 不回退管理员；允许无 token owners 启动，相关动作沿原 NotReady。

运行配置只读普通文件、无叶端 symlink、权限无 group/other；解析错误输出固定安全文字，不输出 TOML 行或密码。来源目录只能来自 catalog，非空旧 keys 在 serve 下拒绝。status_addr 必须回环、端口非零；调用根沿 UiOptions 校验，不能由 Host 推导。manage 必须存在、目录为私有绝对目录、base 为同端口 http 回环根、版本固定原支持版本。GLM 原参数经过原 validate_config，轮询周期限制 5–3600 秒。

`RuntimeError` 是私有固定错误类别，启动/存储/后台任务错误不包含配置、远端正文或密钥。watch<bool> 是唯一停止信号，JoinSet 只持本次启动的五项任务，任务不增加第二业务权威。

## 3 启动与函数契约

### R1 load

输入为显式配置路径。先检查文件类型/权限，再读取和独立反序列化旧 Config 与 RuntimeOptions；所有错误脱敏。检查空旧 keys、路径/端口/固定版本/参数和 issuer 约束，构造 UiOptions。无网络、无目录写入。

论证：验证在所有资源获取前完成；拒绝非空旧来源避免双真值，固定身份不从运行时列表猜测。有限输入反序列化和有限字段验证必终止；成功输出满足后续调用前置，失败没有启动副作用。

### R2 start

先绑定唯一 PanelServer（原 Host/Origin 规则），打开私有 catalog；失败即回收已取得资源。复用 NewApiProcess 的下载/健康/启动功能，但新增受管来源模式明确给子进程设置 SQL_DSN=local、LOG_SQL_DSN=local、SQLITE_PATH 指向自己的目录、ERROR_LOG_ENABLED=true；不修改已运行进程环境，也不重启或修改外部实例。验证配置/PID/exe/cwd/listener 的真实本地绑定后才使用管理凭据。服务入口不隐式创建账号、修改余额或改 NewAPI options，首次初始化由显式新安装流程完成。

创建一个预算，分别构造管理员和 issuer client；issuer 清除管理 token、额外管理员 header、manage 与渠道模板，只保留同实例必要连接和自己的登录材料。TokenOwners 沿原固定 owner 约束注入同一 worker，不做管理员签发 fallback。无 owners 则调用原 new。

启动调用新增薄入口 refresh_runtime_backend；它复用原 refresh_from 的一次撤销→检查→发布边界。检查future内先执行原 check_local_backend（此时结果只在栈上，未发布），再由只读 runtime prerequisites 检查同一个PID/exe/cwd/listener前后不变、实际环境 SQL_DSN/LOG_SQL_DSN=local、ERROR_LOG_ENABLED=true，以及原 `/api/option/` 中 LogConsumeEnabled=true、channel_affinity_setting.enabled=false。两组检查全部通过且原lease未到期，才向refresh_from返回ready；缺字段、重复字段、冲突、读取失败则返回不可用。启动不通过即安全退出，正常维护也使用这个合并入口，避免第二段检查期间暴露短暂ready。旧 refresh_backend/check_local_backend 调用者语义、日志lease、原发布fence和许可类型均不变。仅支持原受管Custom type 8，不为火山放宽为type 1。

论证：catalog 锁先于后台任务，防止第二写者；端口先绑定避免误占旧服务；同实例绑定和原预检是发写必要条件。启动错误会回收本次 Rust 资源，既有托管 NewAPI 的独立进程生命周期不被推定撤销；新安装失败目录保留以便核查。

### R3 supervised tasks

只启动五个有归属任务：原 worker.run、原 run_fact_collection、原 PanelServer.serve_ui、GLM 刷新、维护循环。一个任务意外成功退出、错误或 panic 都触发统一停止；外部 SIGINT/SIGTERM 同样触发。先广播停止再 join 全部实际句柄，继续保留已发生的错误；PanelServer 原关闭过程包括停止准入、业务任务和 usage barrier。等待不可抢占 blocking 工作不能用单 HTTP 45 秒截止冒充整体关闭上界。没有正常路径 abort-all，也不因第一个错误丢弃剩余句柄。

论证：JoinSet 对固定五项拥有唯一句柄，停止前循环数不限但没有积压 tick；停止后不创建替代任务。各任务原关闭契约在共同 stop 下收敛，全部 join 后才 drop 最后 worker/catalog 引用；超时/取消不被当作操作未发送。

R4/R5新增循环的每次worker调用使用私有 `owned_job`：持有一个spawn_blocking句柄，由阻塞线程的Handle::block_on执行原异步入口，Tokio runtime保持驱动。其内部仅在await处选择stop取消可取消的网络/采样；正在执行的同步Core/SQLite代码不可抢占，必须实际返回。外层无论stop与否都await该句柄，不能select后丢弃它。每个循环每次仅一个job，各项前后检查stop；停止后不启动下一项或替代job。原worker.run/collector/PanelServer继续复用它们已经审核的句柄和关闭契约，不另写第二执行器。普通shutdown和任务失败均经监督器等待所有job；强杀仍按§4故障边界处理。

### R4 GLM loop

单个循环按原 poll_interval_secs 调用 refresh_glm_policy；没有并发轮次，轮结束后再等待周期，shutdown 可取消未完成采样（原整轮不发布半轮）。QueueFull/RevisionConflict 等非存储问题只记录固定码并进入下一周期；Storage 为致命错误。单独任务确保慢 GLM 探针不饿死火山周期与预检。不改 95%/100%/临期/pin 或来源探测顺序。

论证：SourceProbe 原互斥/序号、原提交和 fence 决定可见性；这里只改变何时调用，所有网络与异常语义仍由原 callee 承担。有限单轮输入与原请求超时提供每轮终止，shutdown 不重试旧写。

### R5 maintenance loop

单循环、每轮结束后至少 5 秒：到期以refresh_runtime_backend合并刷新真实backend及启动前提（成功后30秒再检查，失败下轮重试），然后 refresh_volcano_policy，再 repair_attribution_once。同循环内 backend 检查不与自己的火山发布互相打断；GLM/管理并发引起 RevisionConflict 时沿原失败关闭，不伪造 ready。火山用量仍由唯一 collector 写入，消费成功清计数、共享额度与周期重置沿 O4。存储损坏终止整个监督树，其它暂态按原错误返回且不输出私有细节。

论证：backend lease 最长仍为原 60 秒；计划 30 秒续期不是有效性保证，延迟必须由原 ready_at 拒绝。周期任务不直接改 priority、计数或 checkpoint；原事务和发布栅栏保持 I1–I8。补归属每次仍至多 256 行，循环不批量无界补扫。

实际C联调补充：[后台发布与组注册协调](../../../fixes/sgm7-fix-background-group-registration.md)。组核验和新合并预检持私有automatic_updates独占guard；自动策略/补归属/采集只在提交段try_read，未取得就按原QueueFull/Busy延后，不能先推进epoch。手动pause/retire等不取此锁，仍能撤销过时Frame。仅显式注册register=true开始前要求原lease至少剩30秒，不延长原期限、不保证一定完成；register=false保留旧有效lease条件。新空库的补归属NotInitialized只等待collector，不能把未知改成零或误作存储损坏。

## 4 初始化与部署

只对经核对不存在的新目标目录执行安装。复用固定 SHA256 的 NewAPI 二进制；应用二进制记录 SHA/提交，配置和凭据 0600、目录 0700。先独立启动新原生实例、证明 PID/目录/监听归属及 setup 未初始化，再创建随机管理员密码和普通签发账号。新实例的自动恢复关闭、消费与错误日志开启；必要普通账号内部余额由首次初始化显式准备，不在运行循环修改，更不是供应商额度。生成配置中不写真实来源或旧 token。

`initialize_fresh(target, native_binary, app_binary, ports)` 为独立安装脚本，不被serve调用。目标不存在、两个端口空闲、二进制固定SHA是前置；用排他mkdir取得新目录。先生成并以0600写入带随机密码的config.pending.toml（issuer ID暂非法，不能启动），再启动自己的NewAPI，保留Popen句柄并按ss/PID/exe/cwd核验。所有HTTP请求限定该回环实例、禁用代理/重定向、有超时；响应只检查固定字段，不输出body或Cookie。请求顺序为：

1. GET `/api/setup` 确认status=false且root_init=false；POST同路径 `{username,password,confirmPassword,SelfUseModeEnabled:true}` 仅一次。POST `/api/user/login` 后 GET `/api/user/self` 回读同名管理员、正ID、role=100/status=1；后续管理请求带其New-Api-User。
2. GET `/api/option/` 保存原QuotaForNewUser。仅新空实例 PUT option `{key,value}` 设置 AutomaticEnableChannelEnabled=false、LogConsumeEnabled=true、RetryTimes=1、channel_affinity_setting.enabled=false；回读逐值匹配。关闭默认亲和只作用于本次全新无业务实例，不改已有GLM或未纳管路由；运行时仅检查，不自动纠正。
3. 临时设置QuotaForNewUser=1000000000000（仅NewAPI内部计费余额）；POST `/api/user/` 一次，字段 `{username,password,display_name,role:1}`。立即恢复原QuotaForNewUser并回读。普通账号独立cookie登录，GET self核实不同于管理员的正ID、同username、role=1/status=1/quota>0；该ID才写入最终配置。角色/余额不符合则失败，绝不把管理员当issuer。
4. 生成最终config并原子替换私有pending文件（最终只有一份凭据配置）；初始来源和token为空。请求序列已由仓库newapi_token_probe和token_service_probe核验，最终C再使用同一初始化函数验证。固定版本默认亲和的依据见[原生设置源码](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/setting/operation_setting/channel_affinity_setting.go)。

任一步异常/HTTP拒绝/丢响应即停止，不再次POST账号或setup，不把ACK当最终身份；临时赠额恢复尽力执行并核对，恢复失败同样整体失败。未知创建只允许GET setup、账户精确查找或使用已保存凭据登录回读，不自动重发；保留私有pending文件供人工恢复。失败关闭本次Popen持有的新进程，不删除新目录，不触碰其它PID。完成仅将初始化进程安全停止交给serve重新启动，后续再次运行安装器因目录已存在而拒绝。该过程有限固定请求、唯一创建尝试，回读通过才发布最终可用配置，满足新实例限定和未知交付不重放。

部署门禁要求实际选项关闭亲和、普通issuer不能用token后缀指定channel、新token固定group且cross_group_retry=false。C用错组正文、错模型、指定channel负例及全组耗尽证明不越组；复用既有token服务生成/回读，不手工构造生产token。配置后来出现覆盖规则则runtime prerequisites失败并保持未就绪，不静默重写规则。

首次初始化不覆盖已存在目录、数据库、配置或服务；失败保留目录和安全报错，不自动重放不确定的账号创建。重复启动复用现有配置/数据库，不重新 setup 或重建身份。专用 systemd 用户服务启用重启恢复；保持 NewAPI 原独立托管生命周期，停止控制器不宣称撤销已下发渠道。回退仅停本次新服务、保留整个私有目录，不删除来源/Key/历史。

服务单元固定 Type=simple、KillMode=process、Restart=on-failure、TimeoutStopSec=180、UMask=0077。SIGTERM只发送本次控制器主进程，控制器广播并等待全部任务；NewAPI即使仍在同一cgroup也不被默认control-group连带终止，不把setsid当作脱离cgroup的证明。180秒是systemd异常强杀边界，不是优雅关闭保证：若超界强杀控制器，按原持久未知写/检查点恢复，不伪报drain成功。显式停止独立NewAPI必须另核验本次PID/exe/cwd，普通控制器stop/restart不执行它。

本次实验目录 `/tmp/quota-throttle-sgm7-empJic`，使用随机非生产端口和假上游；正式目标 `/home/lihaokun/apps/quota-throttle-sgm`，拟用 13000/13001（绑定前再检查）。面板始终回环+SSH隧道；不部署公网面板认证或修改防火墙。NewAPI 原生推理端监听行为不被伪称为回环限定，调用地址显式写入配置，推荐通过 SSH 隧道使用。

用户随后明确同意改用13000/13001。只读证据表明原3000/3001属于yixiao(UID1002)，不是本次目标，不停止或访问其私有材料。源码传输需按自动安全审核要求明确取得测试必要文件授权；不上传设计/审核文档或任何真实运行材料。

## 5 SGM6 C 真实组合与测试

实验运行实际 release 应用与固定 NewAPI，全部操作走真实管理 HTTP，不用直接 SQL 造许可、ready、统计或渠道 ID。假上游提供 models、GLM quota、成功/失败和流式响应，避免请求真实供应商。轨迹包括：新空目录→模型发现来源→同类型组/共享成员→禁用创建→显式组注册核验→原策略启用→普通账号签发→同一 Key OpenAI/Claude（含 SSE）→实际日志→collector/账本→HTTP 统计与真实页面。

固定验收清单还包括不同组独立首选/来源共享计量、原 GLM 策略切换、火山成功清失败计数、连续失败熔断/撤错、上限与暂停/重置边界、全组不可用不跨组、旧 revision/非法请求零写入、Key 启停删除及历史、控制器重启后 ID/用量不重复、未知数据不填零。已有详细状态机/PBT继续复用；C补组合证据，不为每个已验收规则重造测试框架。

新增普通测试验证新配置错误脱敏/路径与旧 CLI 隔离、唯一任务所有权、正常/异常停止全回收；至少两组各 128 PBT覆盖配置值域和监督完成顺序。定向变异至少四项，均实际编译并命中目标语义断言、恢复复验。最终冻结源码后顺序执行完整 debug/release、Node/浏览器和原独立性能，不在同一个 target 的全量执行期间重编译。fermat 的 C 浏览器明确无 API 拦截；B 的 fixture 套件继续保留。

## 6 交付条件

实施补充：同一个automatic_updates独占guard也覆盖每个token_step（原lane内，不包整个批次）及显式reveal（commands→lane→guard→Frame），人工控制仍沿原fence撤销。未发送暂态操作仍按原T5C等待显式Retry，未知交付不重放。新增collector页已收到后提交遇锁的回归；Catalog/DDL/GLM/火山算法未改。

C固定清单收紧为15项，增加真正旧revision及非法字段零写入反例；要求禁用创建的原生回读、GLM切换前后各自真实命中、成功清错同周期且命中原来源、删除后原Key拒绝。原B7/O4不保证启动完永远选第一个成员，夹具只从同一已收敛快照按source_id重绑定A/B测试角色，后续期望固定不随响应改变。浏览器四配置不拦截API。通用轮询只吞明确的暂态读错误；显式Retry固定同operation/目标，reveal单次POST。测试清理失败必须非零退出，SIGTERM实测进入清理且只关闭自建进程。

用户已明确授权在fermat完成全部测试和全新部署，必要源码/测试文件/公开二进制白名单上传已获准并完成；此前首次包含设计文档的上传被拒，未绕过，后续未传设计/审核文档、真实配置/密钥/数据库。

实现、机械检查、独审五维度和隔离端到端全部通过，正式新服务进程与监听/数据库归属核验、停止及重启恢复通过，才报告部署完成。新供应商凭据尚未提供时部署为空来源，不能声称真实供应商推理已可用；用户提供两把火山 key 后再注册，不从历史聊天或旧生产配置擅取密钥。

通信仍复用原连接池/管理预算/固定鉴权；无自动业务写重放，重试由原操作 ID 与版本控制。部署 SSH 只传白名单源码/公开二进制/测试数据；网络失败停止当前复制或安装步骤，不自动覆盖不确定目标。新服务只拥有自己的固定任务与目录，不操作其它进程。函数前置、错误/取消、调用方/被调方、有限轮次和副作用已逐项明确；设计审查由独立审核者在编码前执行。
