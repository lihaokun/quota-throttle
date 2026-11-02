# SGM-8 第5批契约预期与实施对照

预期于2026-11-02从基线7e5cf97上的修订架构与细化v3独立提取，Step0文本核对已通过。2026-11-03回填B1–B8实际位置；不改写事前契约以迁就实现。用户已批准I10/D1及B3–B8连续完成，取消模块间停点，最终统一交付/部署前确认保留。F1修复已独立关闭，最终代码独审未关闭生产问题0；修后最后Node68/0/check:ui、full112/112、余额8/8及全部JS25/25通过，仅最终固定SHA完整Rust/C2汇总pending，详见[实施汇总](b3-b8-implementation-review.md)和[最终代码独审](final-contract-review.md)。

## 1. 契约源

workflow §4.3/4.4、§6；主architecture的SGM8余额共享类型段；sgm-8-cleanup §2.9、I5/I8/I9、2026-11-02用户确认修订的I10；sgm-8-5-balance v3 §§3–8；原两轮C1报告作为缺陷来源，不作通过证据。

实施来源追踪：细化§10.11记录模块位置/时点，§10.12与本表§7记录Named PBT；[decisions.md](decisions.md)保留问题处置。最新用户授权只取消逐模块等待，不免除最终审核或授予部署许可。

## 2. Schema字段

- DB v9保留catalog_instance_id、增加单行balance_policy；默认开关true、warn=1e14、target=1e15，0<warn<target≤4e15，版本比较/明确回滚/CommitUnknown有区别。
- PanelSnapshot wire2增加唯一balance根，policy/unit/accounts及账号/记录字段完整按细化§4；七族、字符串user_id、所有null字段不省略，amount/error_code组合严格。
- 两请求/两回复固定族，成功200、设置/金额/scope绑定原请求；错误沿原28码，无新公开错误码。Rust实际回复必须配原请求sidecar校验，不能反造预期。
- D1 的 60/70/75 具体配套已获用户确认，对应时间约束冻结；其它路由时限保持不变。

字段核对表（来源仍是实施前设计契约；实际位置按下方载体映射回填）：

| 对象.字段 | 类型/约束 | 来源及消费者 |
|---|---|---|
| balance.policy | BalancePolicy对象 | 细化§4.2/4.3；store→worker→state→JS |
| balance.unit | QuotaUnit或null | Native状态→worker→state→JS |
| balance.accounts | 账号数组≤2、issuer在前 | 固定TokenOwners→worker→JS |
| balance_policy.revision | 整数1..JSON_SAFE | catalog单行策略→设置/快照 |
| balance_policy.auto_refill | bool | 用户设置→store/自动补足 |
| balance_policy.warn_below | 正整数、<refill_to | store→worker/JS |
| balance_policy.refill_to | 正整数、≤balanceCap | store→worker/JS |
| quota_unit.quota_per_unit | 正有限f64 | Native /api/status→worker/JS |
| quota_unit.display_type | 1..16 ASCII字母数字串，USD才可金额输入 | 同上 |
| balance_account.owner_scope | TokenOwnerScope枚举 | 固定所有者→worker/JS |
| balance_account.user_id | 正i64的十进制字符串，不是JSON数字 | 固定所有者→worker/JS |
| balance_account.state | sufficient/low/exhausted/unknown | 捕获时纯投影→JS再降级 |
| balance_account.quota | ±JSON_SAFE整数或null | profile读数→快照/JS |
| balance_account.observed_at_ms | 非负安全整数毫秒或null，与quota同空 | 成功读取时刻→JS |
| balance_account.unknown_reason | ControlErrorCode或null | failure/缺读数/超龄规则→JS |
| balance_account.locked_until_ms | 晚于capture的安全整数毫秒或null | 不确定封锁→JS |
| balance_account.backoff_until_ms | 晚于capture的安全整数毫秒或null | 自动退避→JS |
| balance_account.last_refill | BalanceRefillView或null | 内存结算→快照/JS |
| balance_refill.at_ms | 非负安全整数毫秒 | 结算时刻→JS |
| balance_refill.source | auto/manual | 调用来源→JS |
| balance_refill.outcome | refilled/not_sent/rejected/uncertain | 交付分类→JS |
| balance_refill.amount | 1..balanceCap整数或null | §4.3各结论矩阵→JS |
| balance_refill.error_code | ControlErrorCode或null | 同上；refilled为null，uncertain为transport_unknown |
| BalanceRefilled.schema_version | 常量1 | HTTP output→JS |
| BalanceRefilled.owner_scope | 等于原请求scope | 固定owner/原请求→JS绑定 |
| BalanceRefilled.user_id | 正十进制字符串 | 内部i64在output显式转换 |
| BalanceRefilled.before | ±JSON_SAFE整数 | 本次profile→HTTP |
| BalanceRefilled.amount | 原请求整数，1..balanceCap | 本次已校验add→HTTP/JS |
| BalanceRefilled.after | ±JSON_SAFE整数或null | 本次回读，失败null |
| BalancePolicySaved.schema_version | 常量1 | output→JS |
| BalancePolicySaved.revision | expected或expected+1，≤JSON_SAFE | store回执→原请求绑定 |
| BalancePolicySaved.auto_refill | 原请求bool | store→HTTP/JS |
| BalancePolicySaved.warn_below | 原请求正整数 | store→HTTP/JS |
| BalancePolicySaved.refill_to | 原请求正整数 | store→HTTP/JS |

七族所有字段required、nullable仍输出；复用既有原始ID/ControlError schema规则。两个请求字段另明确：RefillBalance={schema_version:1,owner_scope,amount}；SetBalancePolicy={schema_version:1,expected_revision,auto_refill,warn_below,refill_to}，范围同表，不能缺字段、重复字段或带未知字段。privateBalanceContext/sidecar均不进入产品wire。

| 字段载体 | 已完成位置 / 机械证据 |
|---|---|
| DB策略及Policy/Draft四/三字段 | `catalog/schema_v9.sql`、`catalog/balance.rs`，B1真实提交/迁移/损坏测试 |
| balance根、policy/unit/accounts、账号全部字段、last_refill全部字段 | `worker/balance.rs::{BalanceView,BalanceAccountView,BalanceRefillView,Account::view}`；`worker/panel.rs::PanelSnapshot`；`panel-snapshot.schema.json`；`core.mjs::validateState` |
| QuotaUnit两字段 | `newapi/users.rs::{QuotaUnit,parse_unit}`；快照quota_unit定义；Python尾换行漏检已加长度/非法字符约束，Rust/JS本来严格拒绝 |
| 两请求DTO及两成功回复 | `admin/input.rs`、`admin/output.rs`；`panel-admin-{requests,responses}.schema.json`；`client.mjs`和`core.mjs::validateReply` |
| 七族字段/版本/跨实现配对 | `sgm6_b_contract.mjs`→`ui/contract.mjs`；`admin/tests/support.rs::emit_balance_pair`；`sgm6_b_wires.{mjs,py}`。实际3386报文/20请求族/15回复族、2050回复JS消费通过；43 UI正例/25族/76余额负例 |

上述实际wire捕获早于F1未编辑raw保真修复，产品wire/生成契约未改。F1修后最后Node/check:ui、8余额/25全部JS变异、独立56例及full112/112均通过；浏览器新日志为`/tmp/sgm8-balance-browser-final.log`，未把旧full当作修后执行。

## 3. 枚举与常量

owner_scope复用TokenOwnerScope；ControlErrorCode/ExtraCode复用既有。新增state=sufficient/low/exhausted/unknown、source=auto/manual、outcome=refilled/not_sent/rejected/uncertain。JS只import生成常量，balanceCap从schema生成。余额读取预算20s、自动attempt60s、读取时效180000ms、封锁/退避600000ms为内部常量；原管理优先级/容量/速率不改。

| 共享项 | 已实现定义归属 | 复用/import与机械检查 |
|---|---|---|
| TokenOwnerScope | 既有newapi/tokens.rs | catalog/worker/input导入同一类型；JS ENUMS.ownerScope沿生成器，禁止重定义 |
| ControlErrorCode | 既有catalog/control.rs | worker/HTTP导入；JS ENUMS.controlError由既有schema生成 |
| ExtraCode::ResultUnconfirmed | 既有panel_http/admin.rs | dispatch映射该既有错误，错误集合仍28 |
| ManagementErrorKind/DeliveryEvidence/RequestClass | 既有newapi/transport.rs | users/tokens复用，类别与投递证据不另复制 |
| BalanceState | worker/balance.rs | snapshot serde snake_case；JS ENUMS.balanceState来自balance_account.state |
| RefillSource | worker/balance.rs | dispatcher/worker复用；JS ENUMS.refillSource来自balance_refill.source |
| BalanceRefillOutcome | worker/balance.rs | view serde；JS ENUMS.refillOutcome来自balance_refill.outcome |
| BALANCE_CAP | catalog/balance.rs | newapi/worker/HTTP均导入；schema maximum镜像检查一致，JS LIMITS.balanceCap从schema生成 |
| BALANCE_INTERVAL、READING_FRESH_MS、BLOCK_MS、BALANCE_READ_BUDGET、AUTO_REFILL_BUDGET | worker/balance.rs（余额内部常量；封锁与退避共用BLOCK_MS=600000，不再重复同值BACKOFF_MS） | runtime/worker复用；跨JS的180000ms语义用共享向量和边界测试核对，不引入另一配置权威 |
| JSON_SAFE、PANEL_SCHEMA_VERSION | 既有安全整数常量；worker/panel.rs版本升2 | 原校验器复用、fixture引用常量，JS版本/界限由schema生成 |

原生quota_per_unit/display_type不由前端常量猜测，十进制语义固定为其合法Number的规范串。生成器机械核对已通过28枚举/51字段族；新增范围为三枚举/七族，原公开错误集合仍28，不混淆两种计数。

## 4. 流程步骤

已完成流程位置：`catalog/balance.rs`的B1.balance.load/save/verify及`store.rs`迁移；`users/tokens/transport`的B2.unit/add/profile/admission/send/authentication；`worker/balance.rs`的B3.observe、B3.refill.1/1b/2–7、B4.cancel、B5.policy、B6.balance_view；`runtime.rs`的B4循环；`panel_http.rs`的B5.connection。前端沿原U步骤接入，不用模块编号混充流程编号。用户最新已取消B3–B8模块间停点，连续修复/测试/独审后统一交付，最终固定SHA与部署确认保留，仍PR #5。

## 5. 行为契约

- 余额是NewAPI用户内部记账，所有组共用对应所有者；不改供应商token上限、GLM/火山策略或倍率、初始化脚本。
- 读失败/超龄/负年龄为未知，不写零，不根据旧低读数自动加；自动发送前重查当前策略。
- add仅支持正id、正金额、原子add；现有401最多恢复重放一次，其它未知不自动重发。明确None状态拒绝与500+JSON不确定分支须区分。
- 仅NotSent/明确拒绝可走前端R；未知一律HTTP504 result_unconfirmed。内存not_sent尝试记录不等于持久提交；T1断言精确允许差异。

实现/测试：`worker/balance.rs`，10项`token_service_tests/balance.rs`集成；`panel_admin::balance_http_*`及原T1⑨⑩直接store/slot断言；B7八个指定Node名称及四个浏览器名称。终审F1确认“只改auto时未保留raw”是生产缺陷，现逐字段保留未编辑raw并经真实open→submit永久回归、新反向变异和原审查者56/56复核关闭；[八节修正报告](../../fixes/sgm8-balance-policy-raw-amount.md)保留原失败。

## 6. 时序/并发/退出

owner异步互斥→短Core→短槽位，无反序、std锁不跨await。潜在add前装入持有owner锁的取消guard；取消先记uncertain/封锁后解锁；确定结论后取消回读不重写。普通业务截止只取消未发送准入/锁等待，不取消执行中登录；认证与401恢复不重置绝对deadline。已准入HTTP仍≤10s，真实槽位随实际工作回收。Storage须贯穿refill→round→loop，store中毒关闭旧Key/lane gate。内存封锁不承诺跨重启去重。

Drop无法安全结算时只原子置balance.failed、不写全局publication；各余额入口/最终guard拒绝，且真实mutate/publish完成后仍保持余额零准入。真正catalog CommitUnknown仅在Core内封全局栅栏。两个故障域不能混同，T-B9/T-B16分别验证，不能只测Drop瞬间MAX。

实际验证：`balance/tests.rs::cancelled_refill_settles_before_unlocking_waiting_manual`使用真实owned_job/watch stop、HTTP bridge waiter、Notify后才发生的Native quota写；同文件回读取消/私有故障与真实publish交错定例。`runtime/tests.rs::balance_loop_propagates_real_refill_storage_failure`用真实deferred FK CommitUnknown贯穿全链。B3八项/B4三项错误变异独立捕获并恢复；HTTP真实慢头4s/body9s锁住accepted+70s截止及零发送，调用处锚点变异已闭环。

## 7. PBT不变量

各组至少128例，选择生成范围、独立参考与精确测试名在实施时登记，不能以定例/变异代替：

- I9：随机quota（含负）、failure、观测时刻、捕获时刻、浏览器elapsed；未知仅可降级、不因null/失败变零。根fresh时跨179999/180000/180001亦正确，后端unknown不能本地升级。
- 金额：用独立整数商/余数参考生成有限十进制金额与比例，验证半上舍入、1..cap、零/超限拒绝；含0.29×50等固定反例及科学计数法。实现BigInt合并公式不能当自己的参考。
- I10：随机before/目标/amount与合法退款，断言只有合法amount和before+amount才准入；不要求退款后的真实余额≤cap，不用错误旧不变量判失败。
- 串行/取消：随机所有者、取消点与交付结论，参考状态机核同owner结算/封锁先于后续准入，不同owner不混账；真实写延迟的确定性反例另保留。

| 精确名称（全部128例） | 实际位置 / 独立参考与范围 |
|---|---|
| `balance_pb_policy_trace_matches_independent_revision_model` | `catalog/balance_tests.rs`；独立策略/修订号模型，1..23步、0..11小金额/重开。B1窄属性保留，真实存储故障由定例补足 |
| `balance_deadline_pbt_unselected_waiter_never_leaks_or_calls_guard` | `newapi/transport/balance_tests.rs`；class三种、1..499ms、固定占满两槽/未获选waiter。B2窄属性保留，不冒称覆盖全部认证/HTTP路径 |
| `balance_projection_pbt_age_failure_and_signed_values` | `worker/balance/tests.rs`；quota±JSON_SAFE、age=-10..180010、failure/missing，独立分类/读数保留模型 |
| `refill_bounds_pbt_actual_http_matches_wide_integer_reference` | 同上；before±JSON_SAFE、exact=-1..cap+1、target2..cap、refund0..999、auto/manual，独立i128 oracle对真实HTTP/add入口/实际余额；退款不纳入实际硬cap |
| `cancelled_attempt_pbt_seals_one_owner_before_release` | 同上；双owner、source、合法amount、五终态和下一次auto/manual，独立结算模型后调用真实refill核准入/隔离；G2扩充后独立运行通过。只覆盖有限终态+后续准入，非任意长度并发trace |
| `quotaFromDollars rounds and bounds` 内fast-check | `tests/ui/balance.test.mjs`；cents1..1e8/tenths1..1e6，独立商余数半上舍入；科学记数/极小比例/350位域/cap另有固定向量 |
| `balance state and replies validate` 内fast-check | 同上；baseAge0..190000、至多30个0..3000ms增量与failure，独立累计年龄单向unknown参考；根fresh的180000ms边界另有定例/浏览器 |

所有性质均有已执行证据（主full或专项独立恢复）；F1修后最后Node68/0也包含两组UI各128例PBT，另有独立56例未编辑raw回环矩阵和最后full112/112。旧PBT范围未被夸大，不能把有限生成测试或56例向量描述为无穷全域证明。

## 8. 资源与性能

一个余额任务、至多两个所有者；每轮至多3个读取和2个自动attempt，新增异步等待均有绝对截止。原64请求槽/2在途/1后台/Control优先、HTTP体积上限、16连接/4业务槽保持。同步短临界正常返回为既有运行前提，不宣称不可抢占SQLite硬实时。BigInt输入长度和IEEE规范指数有界；投影只在类别变化时重绘，不每毫秒重建DOM。按handoff保留原并发度运行实现门禁/性能，不降并发掩盖失败。

实施证据：`runtime.rs`第六受管任务与轮末delay；HTTP四槽Pending/JobOutcome RAII和原监督测试；`panel_http/tests/capacity.rs`把两最大账号纳入根宽度；主release全量831/0采用默认并发。35既有ignored继续明确列出，该普通full不被宣称为另外单独执行的ignored性能门禁。

## 9. 安全与副作用

只在研究worktree改本批；不碰主工作区dirty改动，不访问运行配置/生产DB/凭据。日志只scope/id/数字/结论。探针显式锁定公开binary SHA、私有临时目录/随机端口、假账号，清理只归属进程；临时凭据0600、用后shred。v9迁移先备份、回滚注意WAL/SHM和权限；本机/远端首次自动加额均须部署前明确同意，不代替部署许可。

实施证据：手动专用原生探针10/10，真实组合16/16+4无API mocks浏览器通过，报告确认自建控制器/Native清理；原生专用临时材料shred。输入只有已核SHA二进制和假账号，没有读取旧instance邻接config/state。本机与远端stable仍7e5cf97，本批未部署。

## 10. 跨层一致性与退出

精确schema、生成字段/enum、Rust实际正反报文与请求配对、Node及浏览器核同一契约；金额使用明确规范十进制比例，不用浮点尾差推导。正常设置不改路由栅栏、CommitUnknown失败关闭。T-B1–T-B17、T1、八个Node/四个浏览器族及精确清单已接线：68 Node、28名称×4=112浏览器、28 SGM7 Rust清单、16组合检查，专用原生10项手动不进CI。

实施前技术C1与本十节独立核对已通过，I10/D1已批准。实际位置/门禁已回填：Rust831/0+35ignored、F1修后最后Node68/0/check:ui/full112/112、实际wires3386报文/20请求15回复、Native10/10、组合16+4；B3八/B4三/HTTP二、余额8/8及全部JS25/25变异闭环，最终代码契约独审通过、未关闭生产问题0。**最终退出尚未满足**：主Rust编译早于两处测试harness有界清理调整，最终固定SHA完整Rust/C2汇总待安排。旧记录保留，不称固定SHA C2通过。见[实施汇总](b3-b8-implementation-review.md)、[最终代码独审](final-contract-review.md)。

## B1 实施对照（局部，不代表整批退出）

| 预期 | 实现 | 测试 |
|---|---|---|
| DB v9、单行策略、布尔/金额/安全修订号约束 | `src/catalog/schema_v9.sql`；`balance.rs::{valid,read,verify_balance}`；`types.rs::DATABASE_SCHEMA_VERSION` | `balance_policy_defaults_validation_and_revision`、`balance_validation_conflict_and_revision_exhaustion_are_read_only`、`balance_corrupt_v9_never_repaired_or_defaulted` |
| 修订号比较、不变不递增、变更递增、耗尽 422、只改策略行 | `balance.rs::save_balance_policy` | 上述前两项及 `balance_pb_policy_trace_matches_independent_revision_model`（128 例独立状态模型） |
| begin/commit Busy → QueueFull，回滚后仍可用 | `balance.rs::write_error` + `store.rs::commit` | `balance_policy_busy_is_queue_full_not_storage`，真实第二连接分别占写锁/读锁 |
| 非 Busy 提交失败 → Storage 且目录失效关闭 | `balance.rs::save_balance_policy` 调用既有 `commit` | `balance_commit_unknown_poisoning_requires_reopen`，真实延迟外键提交失败；`balance_unusable_store_rejects_reads_and_writes` |
| 旧库只读预检，v8 私有备份、实例与历史不变、默认策略、失败回滚、重开不重复迁移 | `store.rs::{open,migrate_v8,verify_schema}` | `v8_migration_adds_default_balance_policy_and_backup`、`balance_migration_failure_keeps_v8_and_backup`、`balance_corrupt_v8_rejected_before_backup_or_migration` |

历史状态（B1当时）：B2–B8尚待实施，未打开运行目录库或向运行NewAPI写余额。2026-11-03的实际位置与隔离测试证据见上方十节，不能把本句当作当前进度。

## B1 Step 5 验证记录（2026-11-02）

提交 `94362c46b1f7eeb9951f0562f1cbcbc9b295eb34` 的局部 C2 已通过。路径 A 独立机械检查及同 SHA 门禁通过；路径 B 从契约重新提取后对照 diff，未读取作者实现映射，遗漏/不一致/待决策项均 0；五维审核均通过。Rust 787/0（35 既有 ignored）、Node 60/0、浏览器 full 92/0，189 输入文件前后哈希一致。报告见 `audit-report.md`，处置见 `decisions.md`。

B1 未新增 JSON wire 出口，相关 JSON/HTTP/UI 扩展属于 B2–B8 的后续验证范围；本记录不把整个十节预期表宣称为已实现或整批 §6.6 退出。

## B2 实施对照（局部，尚待提交后 C2）

已按 §5.3 实现 `newapi/users.rs::{QuotaUnit,quota_unit_until,add_user_quota_until}`，`tokens.rs::{profile_until,observe_profile_until,profile_with,send_with_deadline}`，以及 `transport.rs::{send_fenced_until,send_inner,admit_checked_until,refresh_session_with_deadline,refresh_session_locked_until}`。所有 deadline 为同一个 tokio Instant；旧调用 None 路径保留，HTTP execute/认证熔断/预算常量未改。

T-B4/T-B5/T-B15 的实际位置为 `src/newapi/transport/balance_tests.rs`，当时14项（含128例独立预算资源PBT）覆盖矩阵与独立审查处置见 `b2-implementation-review.md`。当时模块静态审查剩余问题0；Rust全量801/0（35既有ignored）、Node60/0、浏览器full92/0，五项变异编译后均被语义断言捕获且恢复14/0。六个Step、共享cap、类型字段及源码范围机械项通过。当时按本地提交后C2安排，B3–B8尚未实施；后续C2缺口、16项补测及最新连续实施授权以下文/顶部为准，不用历史模块结果宣称整批退出。

## B2 C2 验证状态修正（2026-11-02）

独立审核固定提交 `9619729` 后发现：原14项和128例PBT不足以钉住全部B2契约，**C2暂不通过**。profile两个入口丢掉deadline、初始Pending把deadline延长1s、截止`>=`改`>`、amount窄化i32四项单点变异，均编译后仍14/0。需要补公开入口截止接线、原end时刻、空闲且恰到期分支、大金额请求值保真断言；对应契约未变，本次不接受测试缺口为偏离。

新PBT的实际域为class三种×1–499ms，路径始终两槽满、waiter从未获选；它本身有效，但不覆盖上述入口和边界。12条不变量→已有PBT→定向补足→缺口的完整矩阵见 `b2-c2-pbt-review.md`；原门禁绿色不得覆盖专项结论。第5批尚有B3/B7规定的四组PBT待实现，不把它们列为B2缺陷，也不宣称整个子计划退出。

本次固定SHA门禁：Rust801/0（35既有ignored）、Node60/0、机械/SGM7通过；浏览器原full91/1未通过。原失败case定向5/0仅为诊断，reporter重复注册exit1，不替代full或关闭原因。191输入文件与SHA前后均相同；完整证据及处置见 `b2-c2-audit-report.md`、`decisions.md`。本轮没有修改被审源码或测试。

## B2 C2意见修正验证（2026-11-02）

F1–F3已通过两个公开profile×三阻塞点、四公开until×截止前/等号/后、原Pending/64槽精确end、大整数JSON等值断言补齐；16个测试通过，原四逃逸及三个同类单点变异编译后均被捕获，恢复16/0。PBT仍为原128例限定性质，不能称随机全路径或整批PBT完成。

V1关联面板修复按单引用/现有pump落实；周期work让位但state/expiry继续、显式与在途work不抢占、最新权限守卫保留。新永久浏览器回归覆盖pointer/Space/cancel及Chromium-mobile可信touch，精确清单24×4=96。全量Rust默认并行803/0（35既有ignored），Node60/0、check:ui/SGM7通过，浏览器full一次96/96。独立专项复审无新阻塞项，提交后C2仍待固定SHA复核。详见 `b2-c2-fix-review.md`，原失败记录保留。
