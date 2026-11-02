# SGM-8 第5批契约预期（Step0草稿，实施前独立核对）

2026-11-02，基线7e5cf97。预期由修订架构与细化 v3 在实施前提取，Step0 文本已由独立架构审查者核对通过。用户已批准 D1 的 60/70/75 配套；B1 局部实施位置见末节，其余仍待实施。此状态不是整批实现/schema/测试或部署通过。

## 1. 契约源

workflow §4.3/4.4、§6；主architecture的SGM8余额共享类型段；sgm-8-cleanup §2.9、I5/I8/I9、2026-11-02用户确认修订的I10；sgm-8-5-balance v3 §§3–8；原两轮C1报告作为缺陷来源，不作通过证据。

## 2. Schema字段

- DB v9保留catalog_instance_id、增加单行balance_policy；默认开关true、warn=1e14、target=1e15，0<warn<target≤4e15，版本比较/明确回滚/CommitUnknown有区别。
- PanelSnapshot wire2增加唯一balance根，policy/unit/accounts及账号/记录字段完整按细化§4；七族、字符串user_id、所有null字段不省略，amount/error_code组合严格。
- 两请求/两回复固定族，成功200、设置/金额/scope绑定原请求；错误沿原28码，无新公开错误码。Rust实际回复必须配原请求sidecar校验，不能反造预期。
- D1 的 60/70/75 具体配套已获用户确认，对应时间约束冻结；其它路由时限保持不变。

字段核对表（来源指实施前设计契约，不从代码现状反推；B1 policy 实施见末节，其余位置仍待填写）：

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

## 3. 枚举与常量

owner_scope复用TokenOwnerScope；ControlErrorCode/ExtraCode复用既有。新增state=sufficient/low/exhausted/unknown、source=auto/manual、outcome=refilled/not_sent/rejected/uncertain。JS只import生成常量，balanceCap从schema生成。余额读取预算20s、自动attempt60s、读取时效180000ms、封锁/退避600000ms为内部常量；原管理优先级/容量/速率不改。

| 共享项 | 定义归属（计划） | 复用/import与机械检查 |
|---|---|---|
| TokenOwnerScope | 既有newapi/tokens.rs | catalog/worker/input导入同一类型；JS ENUMS.ownerScope沿生成器，禁止重定义 |
| ControlErrorCode | 既有catalog/control.rs | worker/HTTP导入；JS ENUMS.controlError由既有schema生成 |
| ExtraCode::ResultUnconfirmed | 既有panel_http/admin.rs | dispatch映射该既有错误，错误集合仍28 |
| ManagementErrorKind/DeliveryEvidence/RequestClass | 既有newapi/transport.rs | users/tokens复用，类别与投递证据不另复制 |
| BalanceState | worker/balance.rs | snapshot serde snake_case；JS ENUMS.balanceState来自balance_account.state |
| RefillSource | worker/balance.rs | dispatcher/worker复用；JS ENUMS.refillSource来自balance_refill.source |
| BalanceRefillOutcome | worker/balance.rs | view serde；JS ENUMS.refillOutcome来自balance_refill.outcome |
| BALANCE_CAP | catalog/balance.rs | newapi/worker/HTTP均导入；schema maximum镜像检查一致，JS LIMITS.balanceCap从schema生成 |
| BALANCE_INTERVAL、READING_FRESH_MS、BLOCK_MS、BACKOFF_MS、BALANCE_READ_BUDGET、AUTO_REFILL_BUDGET | worker/balance.rs（余额内部常量） | runtime/worker复用；跨JS的180000ms语义用共享向量和边界测试核对，不引入另一配置权威 |
| JSON_SAFE、PANEL_SCHEMA_VERSION | 既有安全整数常量；worker/panel.rs版本升2 | 原校验器复用、fixture引用常量，JS版本/界限由schema生成 |

原生quota_per_unit/display_type不由前端常量猜测，十进制语义固定为其合法Number的规范串。实现位置允许待填，但字段名/定义归属/消费者/生成源必须按本表逐项核对。

## 4. 流程步骤

实现采用B1加载/迁移、B2有界读取、B3补足1/1b/2–7、B4取消结算、B5设置、B6视图/前端、B7HTTP截止等Step注释；机械检查应覆盖每个步骤及关键异常。Step0独立核对后才实现；每个模块先测试/审核，C2本地提交后停、C3部署前停，所有改动PR #5。

## 5. 行为契约

- 余额是NewAPI用户内部记账，所有组共用对应所有者；不改供应商token上限、GLM/火山策略或倍率、初始化脚本。
- 读失败/超龄/负年龄为未知，不写零，不根据旧低读数自动加；自动发送前重查当前策略。
- add仅支持正id、正金额、原子add；现有401最多恢复重放一次，其它未知不自动重发。明确None状态拒绝与500+JSON不确定分支须区分。
- 仅NotSent/明确拒绝可走前端R；未知一律HTTP504 result_unconfirmed。内存not_sent尝试记录不等于持久提交；T1断言精确允许差异。

## 6. 时序/并发/退出

owner异步互斥→短Core→短槽位，无反序、std锁不跨await。潜在add前装入持有owner锁的取消guard；取消先记uncertain/封锁后解锁；确定结论后取消回读不重写。普通业务截止只取消未发送准入/锁等待，不取消执行中登录；认证与401恢复不重置绝对deadline。已准入HTTP仍≤10s，真实槽位随实际工作回收。Storage须贯穿refill→round→loop，store中毒关闭旧Key/lane gate。内存封锁不承诺跨重启去重。

Drop无法安全结算时只原子置balance.failed、不写全局publication；各余额入口/最终guard拒绝，且真实mutate/publish完成后仍保持余额零准入。真正catalog CommitUnknown仅在Core内封全局栅栏。两个故障域不能混同，T-B9/T-B16分别验证，不能只测Drop瞬间MAX。

## 7. PBT不变量

各组至少128例，选择生成范围、独立参考与精确测试名在实施时登记，不能以定例/变异代替：

- I9：随机quota（含负）、failure、观测时刻、捕获时刻、浏览器elapsed；未知仅可降级、不因null/失败变零。根fresh时跨179999/180000/180001亦正确，后端unknown不能本地升级。
- 金额：用独立整数商/余数参考生成有限十进制金额与比例，验证半上舍入、1..cap、零/超限拒绝；含0.29×50等固定反例及科学计数法。实现BigInt合并公式不能当自己的参考。
- I10：随机before/目标/amount与合法退款，断言只有合法amount和before+amount才准入；不要求退款后的真实余额≤cap，不用错误旧不变量判失败。
- 串行/取消：随机所有者、取消点与交付结论，参考状态机核同owner结算/封锁先于后续准入，不同owner不混账；真实写延迟的确定性反例另保留。

## 8. 资源与性能

一个余额任务、至多两个所有者；每轮至多3个读取和2个自动attempt，新增异步等待均有绝对截止。原64请求槽/2在途/1后台/Control优先、HTTP体积上限、16连接/4业务槽保持。同步短临界正常返回为既有运行前提，不宣称不可抢占SQLite硬实时。BigInt输入长度和IEEE规范指数有界；投影只在类别变化时重绘，不每毫秒重建DOM。按handoff保留原并发度运行实现门禁/性能，不降并发掩盖失败。

## 9. 安全与副作用

只在研究worktree改本批；不碰主工作区dirty改动，设计阶段不访问运行配置/生产DB/凭据。日志只scope/id/数字/结论。探针显式锁定公开binary SHA、私有临时目录/随机端口、假账号，清理只归属进程；临时凭据0600、用后shred。v9迁移先备份、回滚注意WAL/SHM和权限；本机/fermat首次自动加额均须C3明确同意，不代替部署许可。

## 10. 跨层一致性与退出

精确schema、生成字段/enum、Rust实际正反报文与请求配对、Node及浏览器均核同一契约；金额使用明确规范十进制比例，不用浮点尾差推导。正常设置不改路由栅栏、CommitUnknown失败关闭。T-B1–T-B17、T1、八个Node/四个浏览器族及永久变异清单同步，实际执行数字另填，不在设计期声称通过。

实施前：技术 C1 与本十节独立核对已通过，D1 已批准；实施时填实际位置并执行机械项/PBT。实施后路径 A/B 及 §5.2 五维均通过才退出。B1 局部实施及审查见下文与 b1-implementation-review.md；不能以局部结果宣称整批退出，C1 证据见 c1-v3-independent-review.md。

## B1 实施对照（局部，不代表整批退出）

| 预期 | 实现 | 测试 |
|---|---|---|
| DB v9、单行策略、布尔/金额/安全修订号约束 | `src/catalog/schema_v9.sql`；`balance.rs::{valid,read,verify_balance}`；`types.rs::DATABASE_SCHEMA_VERSION` | `balance_policy_defaults_validation_and_revision`、`balance_validation_conflict_and_revision_exhaustion_are_read_only`、`balance_corrupt_v9_never_repaired_or_defaulted` |
| 修订号比较、不变不递增、变更递增、耗尽 422、只改策略行 | `balance.rs::save_balance_policy` | 上述前两项及 `balance_pb_policy_trace_matches_independent_revision_model`（128 例独立状态模型） |
| begin/commit Busy → QueueFull，回滚后仍可用 | `balance.rs::write_error` + `store.rs::commit` | `balance_policy_busy_is_queue_full_not_storage`，真实第二连接分别占写锁/读锁 |
| 非 Busy 提交失败 → Storage 且目录失效关闭 | `balance.rs::save_balance_policy` 调用既有 `commit` | `balance_commit_unknown_poisoning_requires_reopen`，真实延迟外键提交失败；`balance_unusable_store_rejects_reads_and_writes` |
| 旧库只读预检，v8 私有备份、实例与历史不变、默认策略、失败回滚、重开不重复迁移 | `store.rs::{open,migrate_v8,verify_schema}` | `v8_migration_adds_default_balance_policy_and_backup`、`balance_migration_failure_keeps_v8_and_backup`、`balance_corrupt_v8_rejected_before_backup_or_migration` |

B2–B8（Native 有界传输、worker、runtime、HTTP、快照、前端和探针）仍待实现，前文对应字段表及行为门禁继续待填。运行目录库没有打开或迁移；NewAPI 没有余额写入。

## B1 Step 5 验证记录（2026-11-02）

提交 `94362c46b1f7eeb9951f0562f1cbcbc9b295eb34` 的局部 C2 已通过。路径 A 独立机械检查及同 SHA 门禁通过；路径 B 从契约重新提取后对照 diff，未读取作者实现映射，遗漏/不一致/待决策项均 0；五维审核均通过。Rust 787/0（35 既有 ignored）、Node 60/0、浏览器 full 92/0，189 输入文件前后哈希一致。报告见 `audit-report.md`，处置见 `decisions.md`。

B1 未新增 JSON wire 出口，相关 JSON/HTTP/UI 扩展属于 B2–B8 的后续验证范围；本记录不把整个十节预期表宣称为已实现或整批 §6.6 退出。

## B2 实施对照（局部，尚待提交后 C2）

已按 §5.3 实现 `newapi/users.rs::{QuotaUnit,quota_unit_until,add_user_quota_until}`，`tokens.rs::{profile_until,observe_profile_until,profile_with,send_with_deadline}`，以及 `transport.rs::{send_fenced_until,send_inner,admit_checked_until,refresh_session_with_deadline,refresh_session_locked_until}`。所有 deadline 为同一个 tokio Instant；旧调用 None 路径保留，HTTP execute/认证熔断/预算常量未改。

T-B4/T-B5/T-B15 的实际位置为 `src/newapi/transport/balance_tests.rs`，14项（含128例独立预算资源PBT）覆盖矩阵与独立审查处置见 `b2-implementation-review.md`。模块静态审查剩余问题0；Rust全量801/0（35既有ignored）、Node60/0、浏览器full92/0，五项变异编译后均被语义断言捕获且恢复14/0。六个Step、共享cap、类型字段及源码范围机械项通过。本地提交后停C2，不能用模块结果提前宣称该SHA独审或整个子计划退出。B3–B8 仍待实现。
