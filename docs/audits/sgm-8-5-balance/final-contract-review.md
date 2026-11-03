# SGM-8 第5批 B3–B8 最终集成契约独立审查

当前代码审查结论：✅ 通过，0项未关闭生产问题。独立发现的F1已由作者最小修复，并经本审针对性执行复核关闭。完整最终门禁/固定提交SHA及部署许可仍由主任务单独收齐；本报告不把修前门禁当作修后执行。

## 范围与独立性

- 分支 `research/source-groups-token-management`，工作树 `/home/haokun/projects/quota-throttle/.worktrees/source-groups-research`，base/HEAD `929863618e87f375e901f6ee6189cc4c8093e018`。
- 首先阅读 `docs/workflow.md` §5.2、§6.5 和余额子计划 §4–9，独立抽出13项不变量并写入 `/tmp/sgm8-balance-final-contract-invariants.md`，之后才读生产diff、新文件及依赖上下文。未以作者完成结论反推期望。
- 本审只读源码；仅在 `/tmp` 写审查产物。未运行cargo、未读改现有部署服务/config/state/native，未启动服务、迁移、原生加额、提交或推送。Node纯函数反例仅操作合成对象。
- 开始时间：2026-11-02T20:35:55.280236+00:00。
- 开始全输入SHA256（451个git tracked及untracked非ignored普通文件的排序[path,sha256] JSON聚合）：`d691eeb5587652b14335cd3a461e235941ef99455b21ad0b1d5ea202e66666e8`。
- 逐文件开始清单：`/tmp/sgm8-balance-final-contract-input-start.json`；结束清单与漂移核对见报告末节。

## 独立不变量与源码映射

| 不变量 | 独立核对证据 | 判断 |
|---|---|---|
| 固定正数不同owner，issuer在前；字符串ID、显式null、state v2/admin v1 | token_service.rs:29–56、141–151；balance.rs:139–165；panel.rs:11、301；admin/output.rs:70–109；三份schema及生成器 | ✅ |
| owner锁覆盖新鲜读、计算、结算、回读；潜在add取消先封锁再解锁 | balance.rs:267–308、399–435、507–568；cancelled_refill_settles_before_unlocking_waiting_manual 使用真实HTTP bridge + owned_job/watch stop + Notify后才实际quota写入 | ✅ |
| 私有failed单向关闭且不无Core写publication；普通读/策略保存不改publication | balance.rs:202–209、294–306、515–520、579–600；private_balance_failure_does_not_overwrite_concurrent_publication | ✅ |
| Core→slot固定锁序；无同步guard跨await；CommitUnknown仍持Core封MAX | balance.rs:414–448、479–493、589–599；原worker.mutate/publish协议；真实提交错误测试 | ✅ |
| failure/缺读数/负年龄/>180000ms为unknown，保留历史且不据失败旧值自动补 | balance.rs:122–165、223–236、330–371；projection PBT及round_unknown测试 | ✅ |
| 每笔amount及新鲜before+amount≤cap；发送前重新看自动开关 | balance.rs:457–505；i128独立参考驱动真实HTTP PBT及refund反例 | ✅ |
| 一个绝对deadline传owner/budget/auth/login/401 replay；普通超时不取消已准入HTTP | balance.rs:387–408、460、515–525；transport.rs:403–421、572–638、658–730 | ✅ |
| 准入证据分类；≥500即uncertain；unknown→HTTP504；after不替代add确认 | balance.rs:527–568；admin/dispatch.rs:71–87；HTTP500+JSON与第二笔零发送反例 | ✅ |
| 60/70/75真实接线，accepted_at锚点，不早归还4业务槽，其他路由原期限不变 | panel_http.rs:246–252、300–381；bridge.rs:82–114、151–178；input.rs:235–250；app.mjs:246–250；51s实际HTTP成功、70s慢头body锚点与四槽测试 | ✅ |
| runtime首轮立即、轮次串行、停止join真实工作；Storage真实链通监督器 | runtime.rs:56–83、183–221、316–365；runtime/tests.rs:288–512 | ✅ |
| BigInt按规范十进制半上舍入、金额有界、预填向下；无单位禁用输入 | core.mjs:592–627；8组Node中金额128次PBT；极小比例/负数/cap向量 | ✅（转换算法） |
| policy独立revision、原请求body快照绑定；编辑动作不能误改未编辑金额 | core.mjs:631–674（修复669行）；client.mjs:110、130；app.mjs:466–471 | ✅ F1复核关闭 |
| UI每秒保守年龄只降级，chip/strip/预填/上限/错误提示同源，未知不重发 | core.mjs:354–363；app.mjs:390–441；render.mjs:30–45、466–508；浏览器与Node时间边界反例 | ✅ |
| sidecar绑定同pid/序号/族的实际原request，不由response构造请求 | sgm6_b_wires.mjs:60–75；sgm6_b_wires.py实际捕获分支；admin tests emit_balance_pair | ✅ |
| 隔离探针固定SHA、自建进程/目录/监听确认后初始化、只清理自己的句柄 | newapi_balance_probe.py:51–64、177–260；groups_runtime_probe.py首次余额与原生回读断言 | ✅（只读审查，不重跑原生） |

## F1：本审发现的生产行为缺陷 — 未编辑金额被重新量化（P2，已修复关闭）

位置：`src/panel_http/ui/app.mjs` openBalancePolicy（原469–471行），`src/panel_http/ui/core.mjs` setBalancePolicy（原665–667行）。

合法原始策略不限于美元分网格。表单用 `balanceDollarsFloor(raw,unit)` 预填，提交时对两个字段无条件 `quotaFromDollars`。因此只切开关/原样保存也会静默改变已存金额；小于一美分的合法原值被预填成空字符串，无法通过表单关闭自动补足。

本审已用真实core导出函数、合成emptyState只读复现：

| unit | 原warn/target | 未编辑预填 | 原实现只关auto请求 |
|---|---|---|---|
| USD500000 | 100001 / 200003 | 0.20 / 0.40 | 100000 / 200000（静默降低） |
| USD500000 | 1 / 2 | 空 / 空 | ActionError，未发送（开关无法保存） |

这是本批B7当前可触发行为，不归类为其它SGM8后续设计，也不是浮点算法问题。建议对未编辑的文本保留冻结策略原raw；只有变更文本调用精确换算，仍复核冻结unit和policy revision。需要checkbox-only/no-op、仅编辑一字段、极小原值及主动清空非空字段回归。

主任务已确认此缺陷，由B7作者实施最小修正；本审未修改源码。

修复及独立关闭：`core.mjs:669–670` 在冻结unit与policy revision检查之后逐字段比较表单文字和冻结raw的预填文字，未变文字保留raw，改变文字才精确换算；非USD仍保持原有保留raw规则。wire、合法策略域与金额舍入算法均未更改。

本审亲自执行：

1. `node --test --experimental-test-isolation=none --test-reporter=tap --test-name-pattern='^balance submit success binds body and shows result$' tests/ui/balance.test.mjs`：精确目标1通过/0失败。新增回归实际调用openBalancePolicy捕获预填→submit→真实route/requestJson/validateReply，覆盖两组原向量的no-op及checkbox-only、分别改一字段、tiny只改target、主动清空非空值、单位变化和revision变化。
2. 独立直接buildAction边界矩阵：7种比例`5e-324,1e-7,7.3,50,500000,1e20,1e308` × 4组策略`1/2,100001/200003,3999999999999998/4000000000000000,1e14/1e15` × 两种开关状态，共56/56通过；所有未编辑金额严格等于原raw。
3. 作者保留修前失败日志 `/tmp/balance-b7-policy-roundtrip-before.log`，本审读取确认它明确显示实际100000/200000与期望100001/200003不等，非零测试/编译失败。

因此F1及对应“未编辑policy回环”测试遗漏关闭。新增永久反向变异`balance-policy-reconverts-untouched-amounts`已检查为唯一语义回退；修后8/8余额变异日志已读：每项模块成功加载、目标一项语义失败、恢复并重跑；证据 `/tmp/balance-b7-policy-roundtrip-mutations.log` 与 `/tmp/quota-throttle-sgm6-b-mutations-bvkJjx`。

## 测试覆盖与证据边界

- 本审未重跑已有完整套件；主任务提供的Node68/0、Rust831/0、browser112/112、actual3386wires/2050JS、Native10/10、真实组合16+4为外部证据，不冒充本审执行结果。
- 阅读确认：Rust三组128次PBT分别覆盖view、实际HTTP+i128每笔界限/退款、owner多终态后续准入与隔离；UI两组128次PBT覆盖年龄只降级与独立商余数换算。
- F1暴露既有测试遗漏：原测试验证金额转换和“用户明确编辑”成功，但没有合法非分网格策略的open→不编辑金额→submit回环；现已增加真正表单链回归并由本审执行关闭。
- runtime harness现先释放Condvar再做语义断言，避免变异导致测试挂死。本审已读取 `/tmp/balance-b4-mutations-report.md` 及 `/tmp/quota-throttle-b4-independent-mutations-z6m_v1y6/final-verification.json`：3/3逻辑变异编译后语义失败、恢复2/0、183公开输入当时一致。计入的是有效语义断言，不是早期执行器分类失败。
- 同样已读 `/tmp/balance-b3-mutations-report.md` 和对应final-verification：8/8有效变异；早期M5只出现overflow/JoinError未到during断言的失败明确不计入，重排清理/断言后的语义变异才计入。
- 已读 `/tmp/balance-b5-accept-anchor-mutation-report.md`：真实serve_connection调用处改accepted_at→Instant::now()，基线1/0→编译后目标0/1→恢复1/0。反例针对真实慢头/body接线，不只修改纯helper。
- 已读 `/tmp/sgm8-balance-combination.log`：16项检查完整通过（含首次内部余额补足）、4真实浏览器通过、apiMocks=false、controller/native均已停止。此组合执行早于F1修复，不能据此声称F1修后浏览器已执行。
- 本审额外验证 `quota_unit.display_type` 的末尾换行反例，两种输入均被真实validateState拒绝，没有发现schema/JS差异。
- `git diff --check 9298636` 本审执行通过（无输出）。

## 五维度兜底

- 一致性：当前B3–B8跨层契约与源码映射一致；F1补充的未编辑金额保真规则已在细化§5.8.2正文同步，本审已读。
- 风格：沿既有worker/bridge/client/renderer组织、固定枚举和生成契约，未见需阻断的风格问题。
- 正确性：取消、锁序、证据分类、deadline、存储失效与UI投影已独立核对；F1已独立复核关闭，未发现其它当前生产行为问题。
- 性能：owner最多2个、轮次串行、预算/认证等待绝对截止、UI BigInt输入353字符上限；未发现无界等待或每tick无界重建。
- 可维护性：余额状态留worker、传输只处理证据/准入、HTTP构造wire、UI集中金额/投影；模块责任明确。

## 设计边界（不是当前缺陷）

- I10只承诺每笔金额及补足前before+amount，不承诺退款后的实际余额≤cap；退款超过cap不应被报bug。
- 正常publication变化不阻断余额补足；私有余额故障只failed，真正catalog CommitUnknown才持Core封MAX。
- 普通业务deadline后已准入HTTP可以继续，取消也可能有晚到原生写；10分钟封锁及重启丢封锁是已批准边界，并非exactly-once。
- 周期迟到观察覆盖回读、单位读取失败保留旧单位、其它SGM8批次均按已声明范围处理，未提出范围外新功能。
- 生产部署仍需用户确认；本审不授予部署许可。

## 修后机械证据附记

本审只读核验作者最终日志：

- `/tmp/balance-b7-policy-roundtrip-node.log`：68 tests、68 pass、0 fail、0 skipped。
- `/tmp/balance-b7-policy-roundtrip-check.log`：check:ui通过，68实际Node名称、8现有总体PBT组、无不安全DOM sink。
- `/tmp/balance-b7-policy-roundtrip-mutations.log`：8个余额JS逻辑变异均加载成功、精确目标语义失败且恢复复验，包括F1新永久变异。

F1修后的完整浏览器112项、JS整轮及固定SHA全量不由本审重复执行；当本报告形成时这些最终主门禁仍须主任务收齐。代码独审通过不等同全部退出门禁已完成。

## 最终输入与退出说明

- 结束时间：2026-11-02T20:46:06.550523+00:00。
- 结束HEAD仍为 `929863618e87f375e901f6ee6189cc4c8093e018`；本审对象为其上未提交B3–B8工作树。
- 结束全输入SHA256：`8de0880ff93f09d7ce8df1040f6317ca0ed8482db34a4d3c38f4b7effcc1c365`（452个文件）。
- 逐文件结束清单：`/tmp/sgm8-balance-final-contract-input-end.json`。
- 审查期间变化的输入：
  - `docs/audits/sgm-8-5-balance/audit-report.md`。
  - `docs/audits/sgm-8-5-balance/b3-b8-implementation-review.md`。
  - `docs/audits/sgm-8-5-balance/decisions.md`。
  - `docs/audits/sgm-8-5-balance/expectations.md`。
  - `docs/design/source-groups-token-management/subplans/sgm-8-5-balance.md`。
  - `docs/design/source-groups-token-management/subplans/sgm-8-handoff.md`。
  - `scripts/contract_audit/sgm6_b_mutations.mjs`。
  - `src/panel_http/ui/core.mjs`。
  - `tests/ui/balance.test.mjs`。

其中生产源码只有 `src/panel_http/ui/core.mjs` 的F1修复，已独立复核；其余为该回归/永久变异及主任务的实施文档整理。B3/B4/HTTP生产实现及关键Rust测试未在本审期间漂移。

最终核心源码hash：
- `src/reconcile/worker/balance.rs`：`b1a83e0a56e4f1f286b28a9c5a0ad3ffd55ed340b777153fc71e30f2f77e4610`。
- `src/runtime.rs`：`347d51a47ec0384b0b0686934d148879fb56441808ff389f829d2d2dd9a2027f`。
- `src/runtime/tests.rs`：`6e0a25f8ff092a672fd8f5b335bd11633513000a923b06ac6fa0dd9db2c991eb`。
- `src/panel_http.rs`：`41585483cc11aacfc1625f747c2bd23d5836a5af639cdef05ad5d105aba275a1`。
- `src/panel_http/ui/core.mjs`：`3ffd0a5595421243a1f139912371c20e1715a52c135db6f409a483d4ffc42587`。
- `tests/ui/balance.test.mjs`：`61f202ada8cc75097e2a72e827eb22d6153c8c95d29bdb32b33dc9845c4734bd`。

结论：代码契约独立审查与五维度兜底通过，F1及对应回归遗漏关闭，当前未关闭生产问题0。F1正文规则已同步；主任务需更新最终状态、精确测试名对应说明，并将修后full浏览器/JS整轮与固定SHA门禁证据收齐后再声明整批退出；这些收尾不改变本审对已冻结生产源码的结论。其它SGM8批次没有被提升为本批阻断项；生产部署仍需另行明确确认。

文档微调提示（非生产缺陷）：§6新open→submit回环向量应标在实际精确测试名`balance submit success binds body and shows result`下，而非`balance actions build exact bodies`；已反馈主任务。
