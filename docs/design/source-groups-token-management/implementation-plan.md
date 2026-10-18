# 来源共享功能实施准备计划

日期：2026-10-15，进度更新：2026-10-19。状态：[架构](architecture.md)已确认，sgm-1、sgm-2内部执行/恢复与预检、sgm-3内部策略/管理、sgm-4账本/采集/查询及观测发布、sgm-5 A/B/C原生令牌/持久操作/组与服务组合已完成各自本地验收；三页面和运行组装/部署未完成。用户指定fermat全新部署，允许隔离测试；不迁移旧配置/数据，不影响现行本地服务。

## 当前完成与未完成

2026-10-17 授权，进度更新2026-10-18：用户要求连续完成剩余开发，不再逐小步等待确认；仍按 workflow 逐步实现、测试和独立审核，且不接管/重启/部署现有服务。B3 已验收。用户随后明确接受[观测型熔断](subplans/sgm-4-observed-breaker.md)，并要求成功有 token 流时清零计数：不改 NewAPI，不再以可靠最终结果补丁阻塞开发。观测归并核心 O1–O3、O4 持久事务/worker 显式接线及 C-2 内部管理命令均已验收；key、三页面和运行组装仍须各自完成。

- 已有：[需求方案](../../research/source-groups-token-management-research.md)、[实现前审核](review.md)、已确认架构、可复现的隔离 NewAPI 契约探针。
- 已完成：[sgm-1](subplans/sgm-1-catalog-store.md) 的独立目录库及导入函数、[schema](schemas/catalog-snapshot.schema.json)、[审核与复审记录](../../audits/sgm-1-catalog-store/audit-report.md)。复审后的局部修复为 85 项测试及 release 构建通过，四种定向变异被 PBT 检出；不接管 CLI/运行服务。
- 已验证：58 个既有 Rust 测试通过；11 项原生 NewAPI 契约检查通过，隔离进程退出。
- 当前实施：[sgm-2 细化](subplans/sgm-2-channel-reconcile.md)、[操作 schema](schemas/control-operation.schema.json)、[事前 expectations](../../audits/sgm-2-channel-reconcile/expectations.md)。第一批管理传输与渠道接口完成测试和[独立审核](../../audits/sgm-2-channel-reconcile/audit-report.md)。第二批目录 v2 与本地持久意图完成[审核](../../audits/sgm-2-channel-reconcile/batch-2-audit.md)：当前全量 141 项普通测试及本批 3 项变异通过。协调执行和剩余整体验收仍待第三批。
- 第三批进度：[计划器与发送门禁](../../audits/sgm-2-channel-reconcile/planner-audit.md)、[v3 持久日志](../../audits/sgm-2-channel-reconcile/execution-journal-audit.md)、[绑定/回读确认](../../audits/sgm-2-channel-reconcile/confirmation-audit.md)、[HTTP 交付与执行器](../../audits/sgm-2-channel-reconcile/worker-audit.md)完成小步审核。worker 小步时 240 项普通测试及隔离原生执行器禁用创建/绑定/回读/退役通过，8 项变异被捕获。随后[R8/R10 门禁补齐](../../audits/sgm-2-channel-reconcile/assurance-audit.md)后为 245 项普通测试通过，新增 2 项 PBT 各 128 cases，6 项变异（含 2 项编译期负 trait）被捕获；机械检查与 release 通过。真实预检 producer、完整跨模块组合验收和 CLI 接线仍未完成，不宣称整个第三批完成。
- 最新组合验收：[执行器多组轨迹](../../audits/sgm-2-channel-reconcile/scenarios-audit.md)完成本轮独审，新增 4 项 PBT 各 128 cases；全量为 249 项通过，5 项变异被捕获，机械检查和 release 通过。组合覆盖部分失败/重开/重试、历史收据、混合提供者隔离、未知创建及一般未知写，不替代其他既有门禁；真实预检和原生/部署集成仍未完成。用户已重申本地托管为既有架构，范围已确定，不再等待选择；撤回远程管理和数据库 inode 登记扩展，按现有 boot 配置完成本地只读预检。
- 当前本地预检：[独立审核](../../audits/sgm-2-channel-reconcile/local-preflight-audit.md)通过，真实 producer 已接到 worker 显式刷新入口；不再用手传 verified=true 作为原生预检证据。复用现有 boot 配置/PID，未增加配置/schema/身份登记。新增 11 项普通测试（含 2 项各 128 cases PBT），全量 260 项通过；原生隔离测试、6 项变异、机械门禁和 release 通过。旧范围阻塞已解除；下一实施重点为 sgm-3 来源策略，再依次推进事实用量、key 管理、三页面及发布接线。
- SGM-3 [第一批](subplans/sgm-3-source-policies.md)已实现共享 GLM quota 与原策略分组适配，并完成[双路径审核](../../audits/sgm-3-source-policies/audit-report.md)：新增 17 项测试，其中 4 项 PBT 各至少 128 cases，6 项变异被捕获；最终全量 277 passed、1 ignored，机械门禁与 release 通过。独审发现的过滤前唯一性校验和测试身份区分问题均已修复。此批不改 GLM 算法、不创建火山空表、不接运行服务；下一步细化火山状态与持久化，再做协调器/事实接线。
- SGM-3 B 已完成：[纯函数小步](../../audits/sgm-3-source-policies/batch-b-pure-audit.md)为 14 项测试、4 项 PBT、8 项变异；[v4 存储小步](../../audits/sgm-3-source-policies/batch-b-storage-audit.md)为 15 项测试、2 项 PBT、6 项变异。2026-10-16 补齐复杂历史迁移组合 B5，新增 4 项测试、1 项 128 cases PBT、6 项变异，[B 整体退出验收](../../audits/sgm-3-source-policies/batch-b-final-audit.md)通过。B 合计 33 项测试、7 项各至少 128 cases PBT、20 项定向变异执行；最终全量 321 passed/1 ignored，机械门禁、release、独立复审和五维度检查通过。下一步为 C 细化；旧 DDL/GLM 不改，服务未接线/部署，真实终态与采集覆盖继续作为 C/SGM-4 门禁。
- C-1 已完成：[事务适配](../../audits/sgm-3-source-policies/c1-tx-audit.md)为 15 项新增测试/2 项 PBT/8 项变异，[批次发布/成员许可](../../audits/sgm-3-source-policies/c1-publication-audit.md)为 13 项新增测试/2 项 PBT/12 项变异；最新[采集及整体验收](../../audits/sgm-3-source-policies/c1-acquisition-audit.md)新增 11 项测试/2 项 PBT，最终 10 项变异被捕获。C-1 合计 39 项新增测试、6 项各 128 cases PBT、30 项最终定向变异；全量 360 passed/1 ignored，机械、release、独审及五维度通过。共享探针单飞、取消不发半轮，组状态/目标/许可/恢复同事务后整体发布；保留 GLM/no-op/未接管/历史/dry_run 边界。全量暴露的重复回读成本已优化并复验，不放宽性能断言。未接运行 CLI 或部署；下一步为 C-2 管理动作与 C-3/SGM-4 事实能力细化，不能提前启用火山自动调度。
- C-3/SGM-4 事实能力调研：[固定版本隔离核验](../../research/source-groups-facts-capability-research.md)完成，增强探针连续两次通过 8 个请求场景及日志边界断言。确认了同源重试成功、换源期间错误先可见、取消仍有消费、流式 EOF 分类；请求中改变重试配置及自由文本日志插入反例表明，不能用当前 RetryTimes/尝试数或控制台行补出终态。消费统计可单独细化，纯失败请求的可靠结束证据仍待方案确认；没有修改已确认业务规则、Rust 或运行服务。
- SGM-4 [A 批细化](subplans/sgm-4-facts-analytics.md)及[事前十节清单](../../audits/sgm-4-facts-analytics/batch-a-expectations.md)已完成确认，2026-10-17 进入纯函数实施，最新结果见下方及[审核记录](../../audits/sgm-4-facts-analytics/audit-report.md)。B 的持久账本/分页补采/图表查询与 C 的可靠终态/火山组合仍是必做后续，不将 A 的纯函数视为生产采集完成。
- 2026-10-16 [性能门禁修复](../../fixes/sgm-fix-parallel-performance-gates.md)的历史记录：保留原功能/PBT，六个原预算转为四个串行release入口并由CI显式执行。修改前复现359/1/1，修改后release为360/0/5，独立性能4项、入口回归8项及负向门禁通过，静态独审通过；debug为359/1/5，额外暴露导入后重开AlreadyOpen。失败种子保留，当时只验证锁继承机制并准备[目录锁修正方案](../../fixes/catalog-fix-inherited-lock-release.md)，尚未改锁实现，因此当时不能称整体CI全绿。
- 2026-10-17 已按用户确认关闭上述[目录锁问题](../../fixes/catalog-fix-inherited-lock-release.md)：仅成功持锁后构造内部guard，在SQLite连接关闭后显式unlock；不改DDL/API/调度。新增8个测试入口，含128-case所有权PBT、实际子进程及关闭顺序回调；旧实现红例和两个错误变异都被捕获。最终本地默认debug/release各368 passed/0 failed/5 ignored，独立性能4项、入口回归8项、clippy/release build和独审通过；旧失败记录保留，远端CI未触发。来源共享功能仍未部署，下一步是已明确边界的SGM-4 A实现。
- 2026-10-17 SGM-4 A 本地验收完成：A1–A5 白名单解析、批内去重、历史归属、UTC 分钟联合用量和安全加算通过 18 项普通测试（6 项各 128 cases PBT）、10 项变异、机械检查及独立静态审核，3 项审核反馈已关闭。初次 debug 的 385 passed/1 failed/6 ignored 保留在[审核记录](../../audits/sgm-4-facts-analytics/audit-report.md)；用户确认后[单独修正旧认证测试同步](../../fixes/newapi-fix-auth-waiter-test-sync.md)，30 次复测和两项变异通过，未改生产认证。最终默认 debug/release 各 386 passed/0 failed/6 ignored，独立性能 5 项通过（账本最大批次 39 ms），clippy/release build 通过。A 没有接采集或运行服务；下一步是 B 持久化账本与统一采集的细化，确认后才实施。
- 当前未完成：sgm-2/3/4运行接线、公开管理接口与最终整体验收；sgm-6/7三页面UI、启动组装和fermat全新部署。sgm-5 A/B/C已完成内部访问key管理，但不是已有可用页面或在线服务。原 C-3/sgm-4 C 的观测事实、周期历史与火山内部发布合流已由 O1–O4 完成，不代表运行服务已启用。
- 2026-10-17 B1 本地验收完成，基于 `9595837` 实现应用库 v5 的事务入账；[B 总体细化](subplans/sgm-4-facts-analytics-b.md)、[事前清单](../../audits/sgm-4-facts-analytics/batch-b1-expectations.md)和[B1 审核](../../audits/sgm-4-facts-analytics/batch-b1-audit.md)分别保存契约及证据。新增 24 项入口（21 普通/3 性能），6 项各 128 cases PBT、8 项变异、机械及独审通过；默认 debug/release 各 407 passed/0 failed/9 ignored，8 项串行性能、clippy/release build 通过。复用一个应用库/单写者，规范事实、联合投影和检查点同事务；不增加数据库身份登记或自动跨库合并。下一步 B2 统一采集及 B3 查询/补归属仍须分别细化，未接运行服务。
- 2026-10-17 B2 设计阶段记录（后续实现进度见下一项）：[统一采集细化](subplans/sgm-4-facts-analytics-b2.md)及[事前十节清单](../../audits/sgm-4-facts-analytics/batch-b2-expectations.md)形成待确认稿，保留 v5/两条检查点，显式同窗重扫、近期回看与全历史循环补采、共享预算/单飞/恢复均有函数契约。固定源码复核发现本地启动仍需确认日志 SQLite 分支，拟用显式 local 启动值作保守只读证据，不新增身份登记。当时尚待确认并执行 B2.0 隔离分页/启动条件探针，未实现采集器或修改任何运行配置。
- 2026-10-17 B2 已完成 [统一采集细化](subplans/sgm-4-facts-analytics-b2.md)范围内实现和[本地验收](../../audits/sgm-4-facts-analytics/batch-b2-audit.md)：40 项新增入口（38 普通/2 性能）、6 组各 128 cases PBT、15 种变异、机械与三步独审通过；默认 debug/release 各 445 passed/0 failed/11 ignored，10 项独立性能、真实原生组合、clippy/release build 通过。保留 v5/一个写者/共享管理预算，无新增任务表、数据库登记或生产配置修改。下一步是 B3 查询与补归属细化；B2 未接 CLI/面板，不能视为来源分组功能已部署。
- 不开展：逐访问 key 上游绑定、火山临期、积分换算、硬 token 限额、修改 GLM 算法、定制 NewAPI 推理路由。

- 2026-10-17 B3 [查询与补归属细化](subplans/sgm-4-facts-analytics-b3.md)和[事前十节](../../audits/sgm-4-facts-analytics/batch-b3-expectations.md)已形成待确认方案：同应用库 WAL 一致读、秒级边缘补算、完整有界模型/token/时间分项、查询取消与目录 guard 生命周期、小批原子补归属及重扫恢复。仅文档，未改 Rust/DDL/依赖或运行服务；确认后按查询语义、只读组合、补归属三个小步实施，不将设计审查视为实现验收。

- 2026-10-17 用户确认后，B3 第一小步（B3.1–B3.2）[查询计算验收](../../audits/sgm-4-facts-analytics/batch-b3-query-audit.md)完成：14 项普通测试、4 组各 128 cases PBT、10 项变异/独审通过；debug/release 各 459 passed/0 failed/12 ignored，11 项独立性能通过，新最大输出 118 ms。只新增 ledger/analytics 与测试/门禁，没有改 catalog、worker、DDL、依赖或现行服务。下一步是 B3.3–B3.4 同库只读快照与取消，补归属、页面和发布仍待后续。

- 2026-10-17 B3.3–B3.7 及整个 B3 [本地验收](../../audits/sgm-4-facts-analytics/batch-b3-completion-audit.md)完成：新增 37 普通/2 性能/1 趋势入口，5 组各 128 cases PBT，16 种变异和独审通过；debug/release 各 496 passed/0 failed/15 ignored，13 项独立性能通过。100k 总体/token 查询为 120/4 ms，256 行/10000 映射补正 28 ms；200k/1m 一致快照查询含控制写 248/1143 ms，读结束后 WAL 回收验证通过。B3 合计 51 普通入口、9 组 PBT 和 26 种变异；无 DDL/依赖/GLM/火山策略变更，不接现行 CLI/面板。源码分三笔本地提交，未推送/合并/部署；后续可靠终态所需后端补丁范围仍待用户决定，访问 key 和三页面并未实现。

- 随后用户改选观测型熔断，撤销上一项的后端补丁范围前提。O1–O3 [核心验收](../../audits/sgm-4-facts-analytics/observed-breaker-audit.md)通过：按 source/request 去重，成功正 token 活动清零（含已锁存时的计数，但不解锁耗尽），其他源/重复成功不误清。11 项普通含 3 组各 128 cases PBT、9 种变异、独审通过；debug/release 各 507 passed/0 failed/16 ignored，14 项独立性能通过，新分类/合并十万次 41 ms。不改 NewAPI/GLM/旧状态机算法/DDL；源码本地提交 `88b1b36`、`64876c5`。O4 持久去重、组目标/许可事务接线、key 管理及三页面仍未完成，不是在线自动熔断已生效。

复审修正已完成：健康会话显式 retry 多余登录已在 `3b1bbae` 修复，[F1 记录](../../fixes/newapi-fix-healthy-auth-retry.md)包含红绿回归、11 项新增测试（含 128 cases PBT）和 4 项变异；全量 288 passed/1 ignored，release 通过。P1 已补[迁移归属及只读增量核对](../../audits/sgm-1-catalog-store/migration-scope-review.md)，保留历史审核证据。SGM-3 B 已按用户确认完成；下一批仍先细化再确认，不以可选代码清理扩大范围，不改运行服务。

2026-10-18 O4 [本地验收](../../audits/sgm-4-facts-analytics/observed-publication-audit.md)通过，代码/契约本地提交 `f7e54a2`：应用库 v6 持久请求去重、周期历史、来源绝对用量、全部组目标/许可同事务，worker 显式刷新按提供者 scope 发布。新增28入口，4组各128 cases PBT，9类可编译变异及独审通过；默认 debug/release 各533 passed/0 failed/18 ignored，16项独立性能通过（十万消费80ms、最大目录5010ms）。源代码不改 GLM 算法或 NewAPI，不新增运行定时器；下一步 C-2 管理动作及 key/三页面/CLI 接线仍需各自验收。服务未部署。

2026-10-18 C-2 [内部管理验收](../../audits/sgm-3-source-policies/c2-audit.md)通过：GLM组固定/取消、火山撤错/上限及重置设置、按来源模型发现沿原命令/策略/执行链接线。DB v7保留历史操作/成功回执/账本，OperationView wire2区分局部保存与远端确认。新增22普通（4组各128 cases PBT）及1性能入口；默认debug/release各555 passed/0 failed/19 ignored，10种有效编译变异、最终定向/机械/clippy/build及独审通过，17项独立性能通过，万条操作迁移563ms。没有改原GLM/火山算法、NewAPI或现行服务；下一步为SGM-5访问key管理，再接三页面及运行/发布组装。

2026-10-18 SGM-5 A [原生令牌接口验收](../../audits/sgm-5-token-management/a-audit.md)通过：复用原transport/共享预算，固定普通签发与legacy账号，安全分页、精确查回、单组创建、启停、单ID删除和显式复制；新增23普通（3组各128 cases PBT）及1性能入口。默认debug/release各578 passed/0 failed/20 ignored，11类可编译变异、15项隔离原生检查、机械/clippy/release build及独审通过；18项串行性能通过，新最大令牌页15ms。重登principal漂移经红绿修正并复验，不改变旧请求默认行为。原GLM/火山算法、账本/目录DDL、依赖和运行服务未改。下一步为B持久操作、未知创建恢复及幂等，再做C组/签发身份就绪；A不代表完整key管理、页面或部署已完成。

2026-10-18 SGM-5 B [持久操作验收](../../audits/sgm-5-token-management/b-audit.md)通过：同应用库v8保存token身份、目标/收据及执行尝试，复用原幂等操作表；未知创建只查回不重发，ACK不确认，旧回读与旧失败重试不覆盖新版本，重开保留未决证据和成功历史。新增25普通（5组各128 cases PBT）及2性能入口，10类编译变异全部捕获，独审0未关闭问题；最终debug/release各603 passed/0 failed/22 ignored，20项独立性能、机械/clippy/build通过。万条imported身份零操作历史的校验/投影459ms、指定万条旧操作迁移574ms；不泛称任意历史规模都满足该预算。初轮沙箱、版本预期和计时失败及接受的clippy布局提示保留在审核记录。没有改变GLM/火山/账本算法或原生HTTP行为，未重启/部署。下一步是C签发身份/组就绪、引用保护及原worker组合，再接三页面和运行/发布。

## 分步实施顺序

2026-10-19 SGM5 C[内部服务验收](../../audits/sgm-5-token-management/c-audit.md)完成：同一worker组合组注册/引用核验、固定owner、正向准入、持久操作执行/回读及显式复制。新增42普通含6组各128 cases PBT、2性能及1原生组合入口；14类编译变异通过，独审0未关闭。首轮旧计时失败按既有方案补齐隔离，原功能断言和5秒不变，另1类零预算变异通过。最终默认debug/release各645 passed/0 failed/26 ignored，23项release性能及大组debug、机械/clippy/build、原生8项组合通过；无算法/DDL/依赖/在线服务变更，未推送/部署。

2026-10-19 用户要求先开始下一阶段，已启动[SGM6三页面与管理接口细化](subplans/sgm-6-three-page-panel.md)：先安全投影/HTTP适配，再三页与统一刷新，最后交互和隔离组合验收。独立前置契约提取完成，[A1安全状态视图](subplans/sgm-6-panel-state-a1.md)处于设计复核，尚未实现。fermat只做过环境只读检查，未安装或启动项目；后续使用独立测试目录/数据库，正式路径为全新初始化，不安排旧数据导入。

下列 ID 为本 feature 子计划；sgm-1 已完成实现及验收，其余仍待各自进入编码前完成设计，不以此表代替契约。

| 子计划 | 范围 | 主要退出条件 |
| --- | --- | --- |
| sgm-1-catalog-store | 共享类型、来源/组/成员目录、状态库、幂等旧配置导入 | 类型/唯一性约束、迁移重复执行无重复来源、历史映射不复用、数据库权限/备份策略明确 |
| sgm-2-channel-reconcile | 原生 API 封装、全局流控、持久操作、禁用建渠道、源全局启停 | 跨组 priority 独立；暂停竞态/部分失败/未知 POST 对账；不使用裸 CopyChannel；批量状态回读 |
| sgm-3-source-policies | 共享 quota 采集、GLM 分组适配、火山组首选与双判据/重置 | GLM golden 等价；同源跨组仅一份计数；规则 1 去重观测归并、成功 token 活动清计数与遥测前提；人工撤销与重置 fencing |
| sgm-4-facts-analytics | 完整元数据采集、分页/补采/去重、时间×模型×token 投影 | 多页/迟到/重启/缺口测试；共享来源与整体不重复；图表分项合计一致；采集不饿死控制 |
| sgm-5-token-management | 普通签发身份、组注册权限、token CRUD/复制/操作对账 | 新 token 单组隔离并关闭跨组重试；旧 token 保持可用；组删除引用保护；创建结果关联唯一；默认快照和日志无凭据 |
| sgm-6-three-page-panel | 来源页、组与 Key 页、数据页、分模型堆叠柱 | 类型专属展示正确；GLM pin/临期保留、火山无临期；写操作显示 pending/failed；整体/per-key 同口径 |
| sgm-7-migration-release | 预检、备份、旧库导入、灰度、回退、发布说明 | 独立环境端到端通过；错误遥测和账号配额就绪；获得生产重启/切换窗口确认 |

sgm-3 的 token 上限及错误规则以 sgm-4 的事实接口为依赖；先定义接口和纯函数 fixture，待 sgm-4 完成后合流验收，不用 mock 通过冒充生产采集已完成。实现可按依赖调整先后，但不跳过各自前置契约。

## 每个子计划的 workflow 要求

1. 先写细化：范围、复用、数据结构、错误路径、并发责任、函数前后条件及非平凡函数论证。
2. 从契约独立提取十节 expectations，定义 schema/枚举/步骤、不变量、性能与副作用检查。
3. 只实现当前子计划范围，补基本、边界和属性测试；禁止修改期望掩盖失败。
4. 执行机械检查和 workflow §6 要求的独立 subagent 契约对照，记录 audit-report/decisions；此要求用于后续实施验收，本轮未冒充已完成。
5. 业务行为和文档同步后报告结果，再进入下一子计划。

## 第一批要锁定的实现细节

- rusqlite 的锁定版本、状态库 schema、ID 和 SecretRef 格式、一次性导入/导出方式。
- 组轮转顺序与 GLM 输入映射；不改 GLM 原规则或 default 历史 ID。
- NewAPI 错误事件分类、同 request/source 结果归并、采集完整边界和迟到处理。探针仅证明一例 503→备用成功可关联，不能代替完整错误分类验收。
- 管理 API 的关联命名、操作 ID、revision、回读集合、重试/退避与预算机械化测试。
- 普通签发身份初始化、可用组和内部余额检查；不可用时明确阻止生成，不能偷偷退回管理员 token。
- 火山受管请求的亲和/指定渠道规则预检与隔离负向用例；不得为此静默修改 GLM 或未纳管路由。

## 验证和部署隔离

测试使用随机或明确的非生产端口、全新数据库和假上游。不得将生产 config.toml 或数据库复制进普通测试目录、不得请求真实供应商来制造失败。

生产前预检需要确认 ErrorLogEnabled 不是无效的管理 option，而是当前版本实际启动环境 ERROR_LOG_ENABLED=true；已运行 NewAPI 如需变更必须经过部署窗口确认。本轮没有改环境或重启生产。

发布先备份敏感配置和两套数据库的一致性快照，记录渠道/来源映射；迁移保留原 token、URL 和 default 渠道。回退先停止新成员可路由状态，再回退程序和兼容配置，不自动删除用户新建来源或访问 key。

## 交付判定

只有完成全部必要子计划、无未解决的阻断项、隔离端到端与 GLM 回归通过，才能称为功能实现完成。此准备计划和当前探针通过不等于可以合并或部署。
