# SGM-8 第 5 批 / B2 C2：PBT 与测试完整性专项独立审核

- 固定审核 SHA：`961972902d929f004dda46f6be476b79326ffed9`；父提交：`dae7c4e`。
- 工作树：`/home/haokun/projects/quota-throttle/.worktrees/source-groups-research`。审核时 HEAD 相符，工作树无修改。
- 先读 `docs/workflow.md` §5.2、§5.3、§6.2–6.6，及 `docs/design/source-groups-token-management/subplans/sgm-8-5-balance.md` §5.3、T-B4/T-B5/T-B15、§7、§9，独立抽取下列不变量后才读 diff / 测试。随后只核对 expectations §7 的契约文字。没有读取作者 `b2-implementation-review.md` 或既往审核报告。
- 本专项审核者只读源码、测试和原始运行证据；没有亲自运行 Cargo、变异或服务，没有访问运行配置/数据库/fermat，没有提交或推送。四项逃逸变异由主任务在独立 `/tmp` 源码/target 副本执行；本审核者随后核对原始patch、六份编译/测试日志、results.json，并独立重算179个文件恢复哈希。动态范围及证据见下文。

## 结论

**B2 的 PBT 不能称为“完整”。**新增的一个 128 例 PBT 有效覆盖了一个明确而狭窄的性质：两个在途槽均被占用时，从未被选中的第三个 waiter 到期会退出、不会调用 guard、不会遗留队列/在途计数。它只随机请求类别和等待时长，没有随机准入、取消、冷却、初始认证和 401 的事件路径。既有 PBT 和新增 13 个定向测试补足了大部分 B2 契约；不能据“128 例通过”宣称所有 B2 时序和边界已经覆盖，也不能因其没有随机所有路径就一概判实现错误。

本专项发现 **1 项 major 测试证据缺口（F1，含两个独立入口）和 2 项 minor 边界断言缺口（F2、F3）**；对应四项错误变异均编译成功且现有14项B2测试仍全部通过，已由动态证据确认覆盖空洞。当前生产代码没有因此被认定实际违反契约。**本专项 C2 结论：暂不通过，F1 的 major 测试缺口尚未闭环。**F1 应补足后再给 B2 无保留的 C2 通过；F2/F3 应补精确断言或在审核决策中明确处理，不能隐去后声称完整。补足这些项无需强制新增大型状态机 PBT，几个定向测试即可。

**整个第 5 批的 PBT 尚未完成。**批准的 expectations §7 / 细化 §9 明确要求的四组 128 例性质——未知/年龄只降级、独立整数商余数金额参考、每笔 `before + amount` 界限、同 owner 取消/结算/封锁与不同 owner 隔离——主要属于尚未实施的 B3/B7 及其组合。当前 B2 的 waiter PBT、既有 transport/token PBT 和已实施 B1 的策略 PBT不能替代这四组。B3–B8 未实施不是本次 B2 的缺陷，也不能提前出具整个子计划 §6.6 退出结论。

## 独立契约矩阵

简称：`BT`=`src/newapi/transport/balance_tests.rs`，`TT`=`src/newapi/transport/tests.rs`，`AT`=`src/newapi/transport/assurance_tests.rs`，`DT`=`src/newapi/transport/delivery_tests.rs`，`KT`=`src/newapi/tokens/tests.rs`。所有行号以审核 SHA 为准。“①”是本模块批准契约的证据缺口；“②”是定向测试和复用链已足够、PBT可选增强；“③”是后续模块范围。

| B2 不变量（契约来源） | 现有 PBT：生成域 / 时序 / 独立 oracle / 断言 | 定向测试补足 | 空洞及分类 |
|---|---|---|---|
| I1：`id > 0`，`1 <= amount <= 4e15`；不合法、guard 假均明确 NotSent，原业务零发送（§5.3.3/T-B4） | 新 PBT不调用 add；KT 的 create payload PBT针对 token，不是 add 的金额校验。 | BT:124–176 测 id 0/-1、amount 0/-1/cap+1、guard 假、到期、连接失败；独立入口计数；cap 正例证明 hook 有效。 | 校验分区基本足够。i64 极值、更多随机输入可补但不必为了“有PBT”重复。②。金额原样发送另见 I2/F3。 |
| I2：POST `/api/user/manage`，精确 id/action/mode/value，Create，合法金额整个区间无截断/隐式换算（§5.3.3/T-B4） | 无 add payload PBT；KT:914 的随机 payload 白名单只约束 `/api/token/`。 | BT:55–120 逐字段比较精确 body，不过唯一数值为 id=41/amount=17；BT:158 cap 正例只计入口/成功。 | cap 和宽 ID 的 wire 值没有断言；`amount as i32` 可逃逸，见 F3。① minor。无需强推PBT，直接加强现有边界正例即可。 |
| I3：2xx+false 是 RemoteRejected/ResponseReceived/status=None；500+false 是 ResponseReceived/status=500；非JSON429/超时 Uncertain；连接未建 NotSent；只有明确401可重放（§5.3.3/T-B4） | DT:268–276 生成 marker∈0..4、随机秘密串；固定200，独立列举true/false/非布尔/缺字段/数组，断言成功或交付分类及脱敏。AT:204生成多波mode/class/gap，见下文；不含普通业务deadline。 | BT:55 覆盖200真/假、500假、非JSON429并核次数；BT:124连接失败；BT:658慢成功/HTTP超时；DT:65–145补其它拒绝、畸形/超体积/截断；TT:825第二次401终止。 | 交付分区与无普通重试已有充分直接证据，不需要把HTTP状态码全集做PBT。②。 |
| I4：profile固定owner/header/principal，核id/status/role；负quota合法但须在±JSON_SAFE（§5.3.1） | KT:904随机token id、owner、legacy/inherited、remaining -10000..9999、status；独立mask/映射oracle，但直接构造profile，不能算profile quota边界PBT。KT:925随机owner/drift与1..4次启停，断言写序列；仍非新profile入口的截止性质。 | BT:271三个profile入口核Observe/Control分类、header、负quota -17、错id/role及+JSON_SAFE+1；BT:621重登为另一principal后不得重放；KT:559等既有身份恢复测试。 | 新入口共用未改动parse_profile。±JSON_SAFE的允许端点、-JSON_SAFE-1、i64::MIN、非整数/字符串quota尚未集中列出；这是可选加固，不据此要求重写既有parser或阻断B2。②。不得把token remaining的PBT误报成profile quota的PBT。 |
| I5：status换算须finite>0；display_type为1..16字节ASCII字母数字；失败不猜比例；GET/Observe（§5.3.2/T-B5） | 无换算字段PBT；金额BigInt/舍入PBT是后续B7，不是本函数职责。 | BT:180–267正常整数/小数、最小正subnormal 5e-324、长度16、0/负/null/数字串/缺字段/空串/17字节/中文/空格，以及raw JSON 1e999；逐值比较输出。BT:327从quota_unit真实入口验证阻塞Observe到期。 | T-B5各必需分区已覆盖。NaN/Infinity不是合法JSON数值，不能要求通过json!伪造非有限浮点才能算完整；raw overflow已验证拒绝。f64::MAX、长度1、更多ASCII字符可选。②。 |
| I6：observe_profile_until/profile_until把同一绝对deadline真正传入transport；旧profile()仍None（§5.3.1，特别明确禁止只改签名） | 新PBT直接调用budget，完全绕过profile封装；所有既有KT PBT调用旧profile相关链，deadline=None。 | BT:271仅即时返回；BT:621仅即时401和身份漂移，截止20s始终未到。 | 两个新profile入口均无过期或等待到期证据。Some改None可逃逸，见F1a。① major。 |
| I7：未准入等待共享绝对截止；持续Control/续冷却不延长；未选中的waiter也醒；当前时刻必须严格小于deadline才能准入（§5.3.4/T-B15） | BT:723–744：class三类、millis=1..499；固定Control+Observe两槽满，第三个waiter永不选中，推进恰到deadline。oracle是已知零guard、单waiter退出及最终active/background=0；1ms外层timeout防挂死。 | BT:327持续替换Control或每秒续冷却；断言now==end、入口0、queue空/64许可归还；BT:699 guard耗时150ms越过100ms截止后不发送。 | 固定阻塞路径没有“空闲/恰到期先Ready”的分支；普通时钟的now入口反例不能可靠锁定等号，见F2。① minor。更多随机release/cooldown路径仅为②增强。 |
| I8：64个逻辑槽、2个在途/1个后台；取消未准入waiter只移除自身；最终资源守恒，不提前释放已执行请求（§5.3.4/T-B15） | 新PBT验证queue和active/background，但不调用register_request，所以没有验证64个逻辑许可。TT:486随机2..64个gap/class事件、串行admit/drop；独立所有子区间速率公式。AT:204多身份并发4请求/波，服务器计数对照admission trace并核最终queue/active/background/64。 | BT:327/376/413/528/584/658查资源；BT:584持refresh锁填满64再拒绝第65；TT:436取消队列/Control优先；TT:127后台预留；TT:762慢后台仍可Control。 | 不能把新PBT宣称验证了逻辑64槽，但定向测试已补。新增Some路径随机abort可选；共用RAII的旧取消测试仍有效。②。 |
| I9：初始Pending的refresh锁、登录预算也使用原deadline；未准入到期不熔断；已准入登录落定认证，即使普通deadline先到也不被取消，且不再发业务（§5.3.4/T-B15） | TT:409随机0..64次显式healthy retry，核generation/session/预算不变；不是Pending的deadline PBT。AT的401/cooldown轨迹不传deadline。 | BT:376分别卡refresh锁/登录预算；BT:413真实慢登录+等待者，核等待者不取消holder、session/generation落定/业务未发；BT:584逻辑容量。 | 初始Pending测试均未断言其到期实际时刻，finish允许再推进40s；初始deadline延长1s可逃逸，见F1b。① major。已准入登录落定本身已有足够直接测试。 |
| I10：401登录及唯一一次重放沿原deadline；重放前guard与principal重核；到期不重放（§5.3.3/5.3.4/T-B4/T-B15） | AT:68–207强制前两波涵盖401/429，随机尾部mode/class/gap和身份交错；独立预期请求数/冷却数+所有窗口速率oracle。但是全走send(None)，没有截止/取消。 | BT:463成功重放、登录时guard转假、慢登录越过deadline；BT:528锁/登录预算等待、now==end、login0/业务1、无熔断/资源归还；BT:621身份漂移。TT:825第二个401终止。 | 关键新时序有定向覆盖，重用未改动的attempt上限测试合理。无需强制所有401×取消×截止排列都生成。②。 |
| I11：普通deadline不取消已准入HTTP；调用可在deadline后一个原10s执行窗口内返回，保留真实delivery，不把超时当撤销（§5.3.3/5.3.4） | 新PBT完全没有准入成功/执行阶段。 | BT:658在收到add入口后推进到deadline，断言任务未完成；延迟3s得成功，延迟30s得TransportUnknown/Uncertain；各只入一次、最终归还资源。TT:974既有总HTTP超时含body/归还槽。 | 新旧定向测试共同足够；可以再直接断言deadline时active/permit仍占用和最终耗时上界，但非必须补完整状态机PBT。②。 |
| I12：显式stop可取消future，已发登录沿原RefreshCancellation熔断；潜在add的业务取消结算不能与deadline NotSent混同（§5.3.4） | 新PBT无显式abort；既有随机HTTP轨迹也不生成取消点。 | TT:694真实登录到达后取消，核熔断/不得再登录/64及active归还；TT:323显式recovery取消；KT:824已发token写取消后不重试/预算可用。生产取消guard未在此次diff更改。 | B2的既有取消语义可复用；新增Some取消模型为②增强。实际余额同owner结算先于解锁、600000ms封锁等必须在B3做规定PBT及T-B16，属于③。 |

## 影响 C2 的具体测试缺口

### F1 — major：绝对截止的入口接线和实际结束时刻仍可被错误变异绕过

契约依据：细化 §5.3.1 特别要求“不能只改外层签名后仍走无截止send”；§5.3.4 要求初始Pending、锁等待、登录预算、401恢复保持同一绝对截止；T-B15要求明确按时结束，而非最终结束。

**F1a：两个profile入口未验证deadline。**

- 实现位置：`src/newapi/tokens.rs:374–383`；测试位置：BT:271–324、621–655。
- 最小错误变异：同时将 `profile_with(RequestClass::Observe, Some(deadline))` / `profile_with(RequestClass::Control, Some(deadline))` 的 `Some(deadline)` 改为 `None`，保留分类和其它代码。
- 现有测试仍只看到同样的即时响应/身份核验；新PBT没有调用这两个入口。主任务执行的 `profile-deadline-dropped` 已编译成功、14/0、exit 0，确认测试未识别该错误变异；按源码语义，在持续Control、续冷却或认证锁不释放时资料读取可一直等待。
- 最小补足：用paused时钟从两个真实profile入口各发一次，预先占满预算或持refresh锁，推进到捕获的同一个end；断言结束时刻、QueueFull/NotSent、profile入口计数0、queue/逻辑许可归还。单独只测已经过期的参数能抓完全丢deadline，但“实际等到end”还能抓重新起算，应优先后者。

**F1b：初始Pending deadline被延长仍可过现有定向测试。**

- 实现位置：`src/newapi/transport.rs:590–592`；测试位置：BT:376–409、413–460、584–618；防挂死helper在BT:40–51。
- 最小错误变异：只把初始Pending传给 `refresh_session_with_deadline(0, deadline)` 的参数变为 `deadline.map(|end| end + Duration::from_secs(1))`。原业务和401传值不改。
- BT:376/584期待QueueFull、无熔断、归还槽，但不查返回时刻。`finish()`最多额外推进40s，所以延长1s仍可满足。BT:413等待者延到2s退出而holder在3s完成，`!holder.is_finished()`也仍满足。401测试在BT:573精确查now==end，覆盖不到初始Pending。
- 动态确认：主任务执行的 `pending-deadline-extended` 编译成功、14/0、exit 0；本审核者核对其patch只改初始Pending传参，不改401传参或测试。
- 最小补足：BT:376两分支与BT:584完成后核 `Instant::now() == end`；BT:413在等待者返回处核其自身原截止，并核holder依然占用资源。防挂死上界不能替代业务期限oracle。
- 交叉核对：F1b由并行契约审核者独立提出后，本专项复核确认其静态路径；没有读作者自评或历史报告。

### F2 — minor：严格小于截止的等号边界缺少确定性反例

- 契约/实现：§5.3.4明确 `Instant::now() < deadline`；当前 `src/newapi/transport.rs:88` 正确使用 `>=` 拒绝。
- 最小错误变异：把 `>=` 改成 `>`。在paused时钟、空闲预算、`end = Instant::now()`时，检查会放行；timeout可以先poll到ready的准入future，后检查也错误放行，导致截止时刻发HTTP。
- 现有BT:151传 `Instant::now()`，但其测试不是start_paused；进入函数时通常已是严格大于，不能确定杀掉等号错误。其余paused到期用例仍被槽/锁阻塞，会由timeout自身拒绝，不证明ready-at-deadline分支。BT:699测的是150ms>100ms，亦非等号。
- 动态确认：主任务执行的 `deadline-equality-accepted` 编译成功、14/0、exit 0；本审核者核对patch仅把 `>=` 改为 `>`。这证明现有用例未锁住严格等号边界；“空闲且恰等于截止”行为反例来自源码语义推导，本次未另跑该新增反例。
- 最小补足：一个paused的表格用例核“end−1ns / end / end+1ns”，其中预算空闲；前者允许请求、后两者QueueFull/NotSent且入口0。至少固定now==end即可抓住本缺口。更广随机时间值不能替代这个精确边界。

### F3 — minor：合法大金额仅检查“被接受”，没有检查原值被发送

- 契约/实现：§5.3.3/T-B4精确body，合法amount上限4e15；当前 `src/newapi/users.rs:75` 直接序列化i64，代码本身正确。
- 测试证据：BT:92–95仅对17做精确body相等；BT:127–129的handler对任何body一律success，BT:158的cap正例只核成功及入口数。
- 最小错误变异：wire的`value: amount`改为`value: (amount as i32)`。小值1/17保留，cap+1仍被发送前校验拒绝；cap成功夹具仍接受。主任务执行的 `amount-narrowed-i32` 已编译成功、14/0、exit 0，确认现有用例未识别金额窄化；cap=4000000000000000按i32窄化会发送为-1827012608。最小跨边界合法反例为2147483648。`id as i32`亦缺大ID断言，但该ID变异没有执行，不列作第五项动态结果。
- 最小补足：给cap正例加完整body数值相等断言（`Value::as_i64()==Some(amount)`），并可在同一表中加入2147483648与大正ID。无需引入金额换算或前端JSON安全ID限制；内部接口契约只要求正i64 ID，不应擅加 `id <= JSON_SAFE`。

## 独立副本的动态证据

执行者为主任务；本专项审核者只读核验。证据目录：`/tmp/sgm8-b2-c2-coverage-inkv4m98/`，其中 `project/` 为源码副本，`target/` 为独立构建目录，未修改审核工作树产品代码。每项变异在恢复原文件后单独执行，最后再次恢复运行。

| 阶段/变异 | 已核对的原始文件 | 编译/执行结果 |
|---|---|---|
| baseline | `baseline.log` | 编译成功，14 passed / 0 failed，exit 0 |
| F1a：两个profile入口丢deadline | `profile-deadline-dropped.patch`、同名`.log` | 编译成功，14 passed / 0 failed，exit 0；错误变异逃逸 |
| F2：等号允许准入 | `deadline-equality-accepted.patch`、同名`.log` | 编译成功，14 passed / 0 failed，exit 0；错误变异逃逸 |
| F3：amount窄化i32 | `amount-narrowed-i32.patch`、同名`.log` | 编译成功，14 passed / 0 failed，exit 0；错误变异逃逸 |
| F1b：初始Pending截止延长1s | `pending-deadline-extended.patch`、同名`.log` | 编译成功，14 passed / 0 failed，exit 0；错误变异逃逸 |
| restored | `restored.log`、`restored.json` | 编译成功，14 passed / 0 failed，exit 0；179原始文件全部恢复 |

`results.json`记录上述六次退出码和摘要，六份原始日志逐名列出相同14项B2测试（包括128例PBT的测试函数），末尾均为0 ignored、822 filtered out。**动态结论只覆盖这14项B2测试，不声称每项变异都运行了全仓库801项Rust测试。**既有PBT是否覆盖这些入口仍以本报告静态矩阵为依据。

本审核者独立用SHA-256重算 `source-hashes.json` 的179个文件，在恢复后的 `project/` 和固定SHA审核工作树各比对一次，两处均179/179相符、差异0；与主任务 `restored.json` 的结论一致。四项变异都被恢复。动态证据证明测试对这些错误变异不敏感，不把变异后的错误当作当前SHA已存在的产品bug。

## “128例”应当怎样表述

新增PBT定义在BT:723–744，生成域只有 `class in 0u8..3` 和 `millis in 1u64..500`（1–499ms）。每例都执行相同路径：占用Control与Observe两个槽 → 注册第三个waiter → 确认pending → 推进到end → 要求1ms内以QueueFull/NotSent退出 → guard计数0 → 队列空 → 释放持有者后active/background为0。

该oracle来自契约预期，不是复用生产准入判定，因此测试有价值，普通assert也不会妨碍proptest捕获失败。但128是配置案例数，不是路径数、覆盖率或完成证明。对“任意正等待长度的未选中waiter到期清理”这个窄性质，128例是合理抽样，不能仅因未枚举1497组参数就判不够；真正缺的是生成域之外的时序和入口，而不是把128机械增加到1000。

现有相关PBT的真实补充范围：

- TT:486：2–64个随机时间间隔/类别，串行准入释放；独立全子区间令牌速率公式。没有真实HTTP、deadline或取消事件。
- AT:204：强制两波401/429，外加0–6波随机gap/mode/class，每波4请求与两身份；以服务器独立计数核真实业务/登录/重放均计费，再核速率/冷却和资源。是有意义的路径随机化，但使用 `send(..., None)`，不能替B2截止路径背书。
- TT:409：健康session显式retry次数0–64；核会话和预算不变。不是初始Pending/取消/过期模型。
- DT:271：5种success envelope形状与秘密串；是解析/证据/脱敏性质，非HTTP时序。
- KT:904/914/925：token投影、create payload、owner/id漂移及状态写序列；没有随机profile quota边界或B2截止。
- TT:506、AT:209随机脱敏串：提供已有安全回归，与本次deadline完整性无直接替代关系。

## 防挂死、零发送证明和后续范围

- BT的 `entered()`有100000次yield的有限上界；`finish()`有4000×10ms虚拟推进并最终abort；新PBT到期后有1ms外层timeout。不存在把“永久等待”当成合法断言的情况。不过finish是宽松看门狗，F1b证明它不等于严格截止oracle。
- 关键add无发送反例使用handler最前面的AtomicUsize入口计数，并以成功cap请求证明hook能计数，符合T-B4避免unknown路径panic掩盖发送的要求。`Fake.calls`是在handler返回后记录（TT:82–83），所以不能泛化说所有空calls都等于零网络发送；本批多数仅用空calls的场景handler不panic，或同时还有QueueFull/auth/session等独立断言，未据此发现当前测试恒真。
- BT:413慢登录handler在path断言后才计数；非登录请求若错误进入会产生TransportUnknown而非期待QueueFull，不能仅凭它空日志判断，但当前结论不是只依赖空日志。后续新增测试应继续把入口hook放在任何断言/分支前。
- B2的本地额度校验、profile signed-safe解析与quota_unit解析不负责 `before+amount`、退款、封锁、年龄投影或美元舍入。后者必须在B3/B7按批准PBT清单补齐；当下缺失属于③，不能塞进B2测试数量以宣称全批完成。
- 可选增强：围绕有限事件字母表（release slot / advance before-equal-after / cooldown extend / guard false / abort）构建小型模型PBT，再加入Pending/401分支；这会提高组合覆盖，但本次不把这种额外框架当作B2 C2硬要求。

最终建议：本专项暂不通过B2 C2。先补F1的真实入口与精确时刻回归，再补或明确处置F2/F3，并让新增断言分别识别这四项已确认逃逸的错误变异、恢复后复验。完成闭环后可以说“B2批准契约已由限定PBT和定向测试共同覆盖，后续模块PBT仍待实施”；不宜说“B2 PBT完整”或“第5批PBT已完成”。
