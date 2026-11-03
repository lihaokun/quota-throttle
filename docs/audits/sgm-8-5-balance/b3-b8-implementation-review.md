# SGM-8 第5批 B3–B8 实施、验证与独审汇总

2026-11-03；差异基线 `929863618e87f375e901f6ee6189cc4c8093e018`，代码提交 `ef44c070bd967cfec602d8cbc4e11c08585ebbbc`，分支 `research/source-groups-token-management`，继续归入 PR #5。下文保留各轮证据时点，最终固定提交复核另行归档。

**当前状态：B3–B8已实现，固定代码提交的C2、workflow §6.6及五维兜底通过，未关闭问题0。** B7 F1已修复并由原审查者独立56例复核关闭；修后最后Node68/0、check:ui、full浏览器112/112、余额8/8及全部JS25/25变异通过。独立验证者在`ef44c070`重新执行完整Rust831/0、31项release及4项debug性能，275个输入与HEAD相同且前后无漂移；主任务补齐UI性能2/2、浏览器性能4/4。用户授权连续完成B3–B8，最终统一交付；部署前确认保留。现有stable服务未升级。见[F1八节修正报告](../../fixes/sgm8-balance-policy-raw-amount.md)、[最终代码独审](final-contract-review.md)和[固定SHA验证](final-fixed-sha-validation.md)。

## 1. 实现与契约对照

| 模块 | 实现位置 | 行为与验证位置 |
|---|---|---|
| B3 余额服务 | `src/reconcile/worker/balance.rs`；`worker.rs`、`token_service.rs` 构造接线 | 固定两个 owner、失败保留读数且未知、Observe 周期读、补足1/1b/2–7、自动策略发送前重查、取消先结算后解锁、私有failed与catalog中毒分离；`token_service_tests/balance.rs` 10项集成及 `balance/tests.rs` 4项确定性/3项PBT |
| B4 独立循环 | `src/runtime.rs::{balance_loop,balance_loop_jobs}` | JoinSet第六任务、立即首轮、轮末等待60s、普通错误继续、真实refill→round→loop Storage致命传播、stop等待实际句柄；`runtime/tests.rs` 两项余额测试及原监督测试 |
| B5 HTTP | `panel_http.rs`、`admin/{route,input,bridge,dispatch,output}.rs` | 两条同步写路由，scope400/金额422/dry_run403/未知504、字符串ID/显式null；仅授权后的精确补足路由60s业务/75s连接，accepted_at锚点，旧路由45s和16/4槽不变；HTTP余额测试、真实慢头4s+body9s+70s截止、T1⑨⑩ |
| B6 快照与wire | `worker/panel.rs`、三份schema、`sgm6_b_contract.mjs`、`sgm6_b_wires.{mjs,py}`、HTTP fixture/capacity | wire2仅增加balance根；七新字段族/三枚举、严格记录组合、Rust原请求/响应同pair及balance-context sidecar；实际原请求绑定后校验，不从回复反造输入 |
| B7 前端 | `panel_http/ui/{core,client,app,render}.mjs`、`tests/ui/balance.test.mjs`、`tests/browser/panel.spec.mjs` | BigInt规范十进制半上舍入、冻结范围/单位/版本、余额年龄随tick只降级、两同步动作、70s客户端补足预算、就地拒绝/未知结果、静态封锁采样余量、复用既有CSS/交互保护 |
| B8 探针与文档 | `scripts/probes/newapi_balance_probe.py`、`groups_runtime_probe.py`、README、SGM7运行文档 | 显式固定SHA binary、新私有目录/随机端口/假账号、10项原生加额契约；组合清单15→16，启动首轮两账号自动补足与原生回读，后续双协议/GLM/火山原流程保留。CLAUDE.md API更正已随B2完成 |

不改供应商配额、倍率或GLM/火山策略；初始化器仍给新issuer原来的1e12，启动后的独立余额轮按默认策略补到1e15。I10只约束单笔amount及本次before+amount，允许合法退款导致实际余额暂越cap；不承诺跨重启去重或不可抢占同步工作硬实时。

## 2. 已执行门禁及证据时点

| 门禁 | 实际结果 | 原始记录 |
|---|---|---|
| release Rust全量，默认并发 | **831通过/0失败/35既有ignored**，0 filtered；724.93s | `/tmp/sgm8-balance-full-rust.log` |
| F1修后最后Node与check:ui | **68/0**；精确清单、生成契约、DOM sink通过 | `/tmp/sgm8-balance-node-final.log`、`/tmp/sgm8-balance-check-ui-final.log`；作者修后首轮日志另保留 |
| F1修后最后完整Rust fixture浏览器 | **112/112**，4项目，full reporter通过；268.08s，fixture退出 | `/tmp/sgm8-balance-browser-final.log`；`/tmp/quota-throttle-sgm6-b-browser-ZPlYy1/report.json`。修前旧full日志/报告保留 |
| 跨实现wire | UI **43正例/25族/76余额反例**；JS消费Rust **2050回复/15族**；Python核 **3386报文/20请求族/15回复族**及sidecar身份 | `/tmp/sgm8-balance-wires-final.log` |
| 真实Native组合 | **16/16**检查，另**4/4**真实浏览器、`apiMocks=false`；控制器与Native均清理 | `/tmp/sgm8-balance-combination.log`；`/tmp/quota-throttle-sgm7-combination-nnbwnv53` |
| 手动T-B13固定rc.20 | **10/10**，退出码0，临时进程与私有材料清理完成 | `/tmp/sgm8-native-balance-probe-fixed-final.log` |
| 探针静态回归 | Python新增**2/0**；原组合脚本单测**11/0**，精确10/16检查清单与格式检查通过 | `scripts/tests/newapi-balance-probe-test.py`、`groups-probe-test.py`；`/tmp/balance-b4-b8-report.md` |

**历史时点保留**：上表首轮全量Rust早于两处测试harness修正：`balance/tests.rs`先join回收并保留JoinResult，再断言并发publication；`runtime/tests.rs`有界接收并确保释放Condvar后再断言。该轮不作为最终字节证明。最终独立全量已在`ef44c070`覆盖两处修正，831通过/0失败/35既有ignored，595.92s，日志`/tmp/sgm8-final-c2-rust.log`。旧112浏览器/wires/组合早于F1，其历史时点不改写；F1修后full112/112与变异另有新证据，固定SHA还重验3386报文。Native组合的生产Rust与最终提交相同。

最终性能门禁沿用原阈值：Rust性能31/31 release、4/4 debug及runner回归24/24；UI两项为85.6ms/1000ms、212.5ms/500ms，浏览器性能4/4。日志分别为`/tmp/sgm8-final-c2-performance.log`、`/tmp/sgm8-final-ui-performance.log`、`/tmp/sgm8-final-browser-performance.log`。275输入聚合SHA256为`cabae87ddbe2092e33314808dc5bb05b54ec6fa9c55438a1397f98a12b191718`；最终release二进制SHA256为`7aeba806f454b56334d4ef1829b10c3b5d7821a83adb5be4a0805408a8685fde`，未安装到运行实例。

## 3. Named PBT矩阵（每组128例）

| 名称 / 位置 | 生成域与独立参考 | 证据与边界 |
|---|---|---|
| `balance_pb_policy_trace_matches_independent_revision_model`；`catalog/balance_tests.rs` | 1..23步策略轨迹、开关、0..11阈值/目标、修订号模式与重开；独立策略/修订号模型 | B1已通过；小金额轨迹性质，不覆盖任意存储故障或全部金额域，边界/Busy/迁移另有定例 |
| `balance_deadline_pbt_unselected_waiter_never_leaks_or_calls_guard`；`newapi/transport/balance_tests.rs` | 三种class×1..499ms，固定两在途槽已满、waiter未获选 | B2窄性质继续保留；不冒称所有认证/准入/HTTP路径，公开入口/等号/Pending/大整数由补充定例与七变异钉住 |
| `balance_projection_pbt_age_failure_and_signed_values`；`worker/balance/tests.rs` | quota∈±JSON_SAFE，age=-10..180010，failure/missing；独立状态分支模型核未知、保留历史与字符串ID | B3全量包含；有界观察投影，不将0当未知 |
| `refill_bounds_pbt_actual_http_matches_wide_integer_reference`；同上 | before∈±JSON_SAFE，exact=-1..cap+1，target=2..cap，refund=0..999，auto/manual；独立i128计算准入，真实HTTP/Fake实际写入与入口计数 | B3全量及独立恢复通过；M8反例为before+amount=cap+1。退款参与实际after，不参与错误的实际硬cap约束 |
| `cancelled_attempt_pbt_seals_one_owner_before_release`；同上 | 随机owner/source/1..cap金额、五种取消/已结算终态及后续auto/manual；独立记录/封锁/退避模型，再调用真实refill核本owner准入与另一owner隔离 | G2扩充后独立128例通过；有界guard终态+后续准入性质，不是任意长度并发网络trace穷举。实际晚写/排队取消/回读取消另由T-B16定例覆盖 |
| `quotaFromDollars rounds and bounds`中的fast-check；`tests/ui/balance.test.mjs` | cents=1..1e8、tenths=1..1e6；独立整数商/余数半上舍入参考 | B7 Node全量通过；固定科学记数/极小单位/350位金额/cap/0.29×50等向量补随机域之外边界 |
| `balance state and replies validate`中的fast-check；同上 | 初始age=0..190000、至多30个0..3000ms增量、failure；独立累计年龄/unknown单向模型 | B7 Node全量通过；根fresh跨180000ms定例及浏览器仍保留；不本地升级后端unknown |

这些是已声明有限生成域中的随机性质证据，不是无穷全域证明。旧B1/B2的受限PBT不被B3/B7新增性质覆盖性表述替换。

F1额外独立56例是7种比例×4组合法raw策略×两种开关的表单回环矩阵，另有真实open→submit定例；它与上述7组各128例PBT分别记账。最后Node68/0和full112/112已在F1修后运行。

## 4. 独审、变异与处置

| 范围 | 已取得结论 | 记录 |
|---|---|---|
| B3/B4生产静态契约 | 未发现确认生产缺陷；G1真实runtime stop→HTTP bridge waiter→晚Native实际写、G2随机owner/终态/后续实际准入缺口均补齐并独立关闭 | `/tmp/balance-b3-b4-independent-review.md` |
| B3关键错误 | **8/8**编译成功后被指定语义断言捕获；初始6目标基线、修正后M5基线、最终恢复6目标及G2额外128例通过 | `/tmp/balance-b3-mutations-report.md`；隔离目录 `/tmp/quota-throttle-b3-independent-mutations-naze6bvo` |
| HTTP原45s外层回退 | 独立基线1/0→旧45s变异0/1→恢复1/0；真实51s成功路径区分 | `/tmp/balance-b5-b6-report.md`、`/tmp/sgm8-b5-budget-{fixed,mutant-old45,restored}.log` |
| HTTP accepted_at调用锚点 | 独立基线1/0→仅调用实参改Instant::now并编译后0/1→恢复1/0；真实头4s/body9s/t70/零发送断言不变 | `/tmp/balance-b5-accept-anchor-mutation-report.md` |
| B5/B6独审 | H1慢头接线证据已由上述矩阵关闭；S1 Python schema尾换行漏检已用显式长度/非法字符约束及两反例关闭 | `/tmp/balance-b5-b6-independent-review.md`、作者报告及上述锚点矩阵 |
| B7显示/F1修正与余额变异 | 静态封锁余量stamp遗漏capture及F1未编辑raw回环均已修；最新**8/8余额变异**加载/目标语义失败/恢复通过，旧7/7保留 | [F1修正](../../fixes/sgm8-balance-policy-raw-amount.md)、`/tmp/balance-b7-policy-roundtrip-mutations.log` |
| B4最终三变异 | **3/3**编译后明确语义失败；独立基线2/0、恢复2/0、183文件hash无漂移；有界清理harness修正后执行 | `/tmp/balance-b4-mutations-report.md` |
| 全部JS变异最后一轮 | **25/25**实际加载/目标语义失败/恢复通过；含新F1变异，没有把历史子集相加冒称整轮 | `/tmp/sgm8-balance-js-mutations-unrestricted.log`；`/tmp/quota-throttle-sgm6-b-mutations-L4jXMQ` |
| 全局代码契约终审 | **通过，未关闭生产问题0**。F1原向量及7比例×4策略×2开关的独立56/56复核关闭；工作树代码独审不等同固定SHA C2/全部退出门禁 | [最终代码独审](final-contract-review.md) |

所有Rust反向验证使用独立源副本和独立CARGO_TARGET_DIR，未共享主target。B3 M5首次仅overflow/JoinError而未到语义断言的运行保留但不计有效捕获；修正测试清理/断言顺序后重新基线、变异、恢复。JS首次“拒绝关闭表单”变异仅触发TypeError也未计通过，补明确editor身份断言后重跑。具体接受/修复决定见 [decisions.md](decisions.md)。

## 5. 失败历史与修正范围

- B2旧C2四类变异逃逸和浏览器失败仍在旧报告，未回写为通过。
- 本轮首个余额Rust专项36/1：50s后回读需要第三份预算，测试仅yield导致虚拟时钟不推进；修测试显式再推进1s，保留200/仅一次add并新增恰51s断言，生产时限未改。
- B7临时浏览器执行器首次过滤错误零测试不计门禁；之后12/4失败来自app.now与Lanes时钟不一致，修测试同一时间源后16/16局部通过；最终正式full112/112单列，不以局部替代全量。
- B7静态余量600秒未随新capture变595秒是真实显示缺陷；仅在有lock/backoff时把captured_at_ms并入stamp，保留静态采样语义和手势保护，永久Node/浏览器与变异已补。
- T-B13原探针普通账号密码/显示名超rc.20字段上限，且英文权限说明使用privileges而旧匹配只认permission；修假数据与固定中英权限文案精确白名单，新增固定stage诊断。原失败及中间2/10日志保留，最终10/10不放宽HTTP/普通用户正确身份头/余额不变/其它九项契约。
- Python schema `$`尾换行差异已实测修复；JS无m正则与Rust本来拒绝，不据错误推测修改解析器。
- 最终独审发现B7原始金额保真缺陷：合法策略值不在整美元分网格时，仅切auto也把向下预填再换算写回；例100001/200003→100000/200000，raw1/2无法关闭auto。现逐字段保留未编辑文本对应的冻结raw，只有编辑字段才精确换算，原单位/版本guard仍先执行；真实open→submit永久回归、新反向变异、修后Node68/0及原审查者独立56/56复核已关闭F1。此前绿色门禁未覆盖它的历史事实保留；最后F1修后full112/112复跑也已通过。
- 最终JS整轮首个沙箱执行无子进程输出而失败，日志保留；未把它计作语义捕获。获准后独立重跑25项全部加载、各目标语义失败并恢复，最终以unrestricted日志为准。

本轮产品语义未偏离已确认I10/D1/七步设计；主要实现选择为余额Node八名称集中新文件、测试使用私有生命周期接缝和实际请求sidecar。文档、schema、fixture与精确清单同步，旧冻结审计脚本仍保留历史范围。

## 6. 最终收口与部署边界

1. F1修后full112/112、Node68/0/check:ui、余额8/8、全部JS25/25与代码独审完成；修前与修后日志分别保留。
2. 最终代码`ef44c070`的独立全量、机械契约、性能及输入一致性验证通过，十节expectations齐全，所有已确认问题关闭，满足本子计划workflow §6.6与C2；不扩大为其它SGM-8批次完成。
3. 按主任务统一安排收口本地提交、固定SHA复核及既定PR/CI流程，不恢复逐模块用户停点；本报告不额外授予推送或部署许可。
4. 部署仍需用户确认v8→v9迁移、私有备份/WAL-SHM整体回滚边界及首轮原生加额；原生已成功加额不会随目录回滚撤销。未获确认不更换任何stable部署，不读取或记录远端配置、凭据或主机接入细节。
