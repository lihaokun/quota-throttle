# SGM-8 B2 C2 独立契约审核

审核对象：`961972902d929f004dda46f6be476b79326ffed9`，基线 `dae7c4e96834790502150cb79b5b0a1b97b74e6d`。只审 B2，不构成第 5 批完成、推送或部署许可。

方法：按 `docs/workflow.md` §6.5 路径 B，先读 §5.2 / §6.5、细化 §4.2 / §5.3 / T-B4 / T-B5 / T-B15 与 architecture 共享类型，独立列以下 expectations，然后才读取产品代码 diff。未读作者 implementation-review、实现位置映射或旧 review。模板缺失，依 workflow 明文执行。工作树只读；不运行 Cargo、不访问实际服务、配置、数据库或凭据。

## 独立 expectations（读取 diff 前）

| ID | 来自契约的预期 |
| --- | --- |
| E1 | 原 `profile()` 仍 Control/None；`send_fenced` 等旧调用的预算、优先级、在途数、速率和 HTTP_TIMEOUT 不变。新增入口复用原链。 |
| E2 | `observe_profile_until` 为 Observe、`profile_until` 为 Control；共享 profile parser，固定 owner / expected user / 身份角色 / 安全整数校验不变。 |
| E3 | 同一 optional absolute deadline 贯穿原业务排队、refresh 锁、登录预算、初始 Pending 与 401 恢复/重放，任何阶段都不重置或漏回无截止路径。 |
| E4 | 只对尚未准入的队列/锁等待施加 timeout_at；队列满或已过期立即 QueueFull/NotSent。取消或超时清自己的 waiter，不泄漏 64 槽，不取消其他登录。 |
| E5 | 最后检查必须在准入处同时检查 guard 与截止；Ready 边界返回后再检查期限，过期释放 admission，原业务入口计数为零。每次 401 重放重查 guard。 |
| E6 | 登录未准入超时不触发 RefreshCancellation 熔断；登录一旦准入正常执行到真实结果/原 10s 上限，认证状态落定后已过期不得原业务/重放；显式 stop/异常取消沿原熔断语义。 |
| E7 | 已准入业务 HTTP 不受普通业务 deadline 取消、不提前释放在途预算；保留真实 delivery。最多一次 401 恢复重放，其余错误不重试。 |
| E8 | add 拒绝 user_id≤0 或 amount 不在 1..=BALANCE_CAP，InvalidInput/NotSent 且零入口；正 id 不另猜账号。精确 POST `/api/user/manage`，仅 id/action=add_quota/mode=add/value，Create、Control、SINGLE_BODY_LIMIT。 |
| E9 | add 成功仅 2xx/success:true。200 false 为 RemoteRejected/ResponseReceived/status=None；合法 JSON 500 保留 ResponseReceived/Some(500)；非 JSON 429/执行超时为 Uncertain，连接建立失败 NotSent。 |
| E10 | quota unit 为 GET `/api/status`，Read/Observe/SINGLE_BODY_LIMIT/envelope；比例为正有限数值，display 为 1..=16 ASCII 字母数字字节，缺失/非法 InvalidResponse，不猜比例。 |
| E11 | 测试须能判别真实发送；入口计数在 handler 分支前，正常发送正例钉住计数。T-B15 覆盖持续 Control / 续冷却、64 槽归还、未选中 waiter、refresh 锁、登录准入前后、Pending、401 原截止。 |
| E12 | 不变量应有真实生成输入与独立预期，不能把固定场景或例数充当完整性证据；检查边界/故障路径测试是否对应实际实现。 |
| E13 | 五维兜底：契约一致、沿既有风格、安全与时序正确、不增加无用等待/拷贝、不重复实现预算/认证/解析。未修改旧 GLM/火山策略、无新增数据库或后台任务。 |

## 审核结论

**✅ 产品代码：未发现可复现的 B2 正确性、回归或安全缺陷（0 critical / 0 major / 0 minor 产品缺陷）。❌ B2 C2 验证门禁不通过：本审独立发现的 Pending 截止缺口已经动态确认；与专项审核汇总为 1 Major（F1，截止接线，含两 profile 入口及 Pending）和 2 Minor（F2/F3，等号边界和大额 DTO）测试缺口。** 128例PBT不能支持覆盖完整的结论。需要修补测试证据，没有证据要求修改当前产品代码；完整分级与处置由主审核汇总。

### ⚠️ C2-B2-T1（本审局部 Minor，汇总并入 F1）：初始 Pending 的绝对截止可以被延长而不被现有测试察觉

- 契约：细化 §5.3.4 要求初始 Pending、refresh 锁和登录预算携带同一绝对截止；T-B15 要求等待者到期退出。
- 位置：`src/newapi/transport/balance_tests.rs:376` 的 `balance_pending_login_budget_and_refresh_lock_are_bounded_without_fuse` 在第 397 行只推进 1 秒，随后用 `finish` 等到结束；第 398–405 行只断言错误类别和资源清理。`finish`（第 40–51 行）允许再推进约 40 秒。第 584–617 行的 64 槽 Pending 测试也没有结束时刻断言。
- 具体反例（先独立静态推演，后**动态确认**）：仅把 `transport.rs:591` 的初始 Pending 参数改为 `deadline.map(|end| end + Duration::from_secs(1))`。锁/登录预算等待者会在原截止后 1 秒才以同样的 QueueFull/NotSent 返回，所有队列/许可仍可正确清理，上述测试没有区分这一违约；其他 B2 初始登录测试同样不核对这个等待截止。已有 PBT 直接调用 budget helper，不经过该接线。主任务在 `/tmp/sgm8-b2-c2-coverage-inkv4m98/project` 独立副本、独立target运行此单项变异；本人只读核对其 patch、原始log、results.json。编译成功后原14测试全部通过、exit=0，确证该变异逃逸。
- 区分：当前产品第 591 行确实传原 `deadline`，因此这不是产品 bug。401 等待测试第 573 行已有 `Instant::now() == end`，它覆盖的是另一分支，不能代替初始 Pending。
- 修补验证应直接确认初始 Pending 锁/预算分支在原截止返回，并以这个分支的截止延长反例证明断言有判别力；无需为此增加无关测试框架。

## 契约对照结果

| Expectations | 结论与证据 |
| --- | --- |
| E1 旧 None 行为 | ✅ `transport.rs:553–559, 615–617, 653–655, 669, 676–682, 720` 保留旧准入与认证链；`tokens.rs:371–372` 保持 Control/None。原无 guard 的 `send` 本就调用 `send_fenced(..., true)`，新统一处理没有改变含义。队列/速率/HTTP 常量、执行器及优先级算法未改。 |
| E2 owner/principal/role | ✅ `tokens.rs:333` 仍 for_user；`371–407` 共用 parse_profile；原 parser `154–176` 核固定 ID、status=1、Issuer=1 / LegacyAdmin=10或100、非空组及安全整数。`transport.rs:865–873` 拒绝重登换 principal 并固定 header。新测试 `270–323, 620–654` 覆盖两新入口的角色/ID/数值与重登身份漂移；旧测试继续覆盖 parser 与 LegacyAdmin 的共享路径。 |
| E3 绝对截止贯穿 | ✅ `transport.rs:580, 591, 594, 616, 628, 664–671, 691, 717` 携带同一 Option<Instant>，没有重算时限或落回无截止 helper。⚠️ 对初始 Pending 接线的测试判别力见 T1。 |
| E4 队列/锁取消清理 | ✅ `403–421` 仅 timeout admit future；`432–442` Drop 按 active/queued 清资源；`582` 的 RAII permit 包括认证等待。锁等待 timeout 只丢当前 lock future。`balance_tests.rs:326–409, 527–617, 723–744` 检查排队、冷却、锁与64槽。 |
| E5 最终 guard / 边界 | ✅ `411–420` 在最终 check 同时检查截止，并在 Ready 返回后再检查，慢 guard 跨界会 drop admission；`603–619` 每次发送都经 guard。`balance_tests.rs:142–155, 462–524, 698–720` 分别覆盖 false/过期、401后guard变化、慢guard跨界。 |
| E6 登录与熔断 | ✅ `714–727` 在登录准入成功后才建立 RefreshCancellation；`728–751` 正常执行与认证落定不受业务 deadline timeout；下一轮 `594` 阻止过期业务。`balance_tests.rs:412–459` 直接核 holder 未提前结束、未熔断、Session(41)、generation=1、未发原业务。异常取消沿旧 guard；旧 `transport/tests.rs:694–756` 确认真实登录入口后的取消熔断，`323–375` 覆盖显式恢复取消的既有特例。 |
| E7 准入后执行与401 | ✅ `619–620` 持 admission 直到原 execute 完成；`890–981` 仍只受 HTTP_TIMEOUT/体积限制。`593, 623–629` 至多一次401恢复，错误不重发；旧 `transport/tests.rs:825–847` 覆盖第二次401终止，新 `balance_tests.rs:657–695` 确认到普通截止后调用仍未完成并保留成功/Uncertain结果。 |
| E8 add 输入与请求形状 | ✅ `users.rs:69–80` 复用 BALANCE_CAP，前置拒绝非正ID/非法金额；精确 id/action/mode/value、POST、Create、Control、单体积上限，无账号猜测或额外自动重试。T-B4 的正常入口 hook 在断言前增加，零发送用例也有正常正例验证 hook 生效。 |
| E9 delivery | ✅ add 走原 execute/envelope，没有将业务 deadline 包在 HTTP 外；`transport.rs:897–904, 943–979, 215–224` 保留连接失败NotSent、200 false/None、500 JSON/Some500、非JSON429/Uncertain等分类。`balance_tests.rs:54–176, 657–695` 对应实际 HTTP 夹具证据。 |
| E10 unit | ✅ `users.rs:20–58` 正确 GET、Read、Observe、SINGLE_BODY_LIMIT；as_f64 + 正有限数值、ASCII字母数字1..16字节，缺失/非法InvalidResponse。测试 `179–267` 包含整数/小数/最小正subnormal、缺字段、null/string、非正、17字节/Unicode/空格及1e999非法数字。 |
| E11 测试真实性 | ✅ 核对 Fake `transport/tests.rs:80–87`：handler 在 Recorded保存之前，B2关键写入/重登入口使用Atomic计数且能正向到达；没有用未知路由panic当零发送证据。T-B4、T-B5和主要T-B15场景有直接断言。⚠️ 期限准确性缺口为T1。 |
| E12 PBT边界 | ⚠️ `balance_tests.rs:723–744` 真实生成 class(三类) 与 millis(1..499)，预期独立规定未选中waiter应到期、guard不调用、队列/在途归零，M2有实际shrunk反例；但状态固定为已有Control+Observe各一，无None/Some接线、Pending、401、锁、登录、unit、amount或动态guard生成。128只表示该性质的样本数。专项审核可给出更完整矩阵，此处不宣称其余性质已获PBT证明。 |
| E13 副作用边界 | ✅ 产品diff限于newapi模块；BALANCE_CAP从catalog导入，未复制常量；旧GLM/火山策略、运行时、数据库、HTTP路由及UI不在产品diff中。没有新后台任务或自建重试器。 |

## workflow §5.2 五维审核

| 维度 | 结论 |
| --- | --- |
| 一致性 | ✅ B2接口、请求类型/路径/字段、共享QuotaUnit类型、统一绝对截止与旧None行为符合 §5.3；报告只覆盖B2。 |
| 风格 | ✅ 延续中文Step注释、既有Result/ManagementError、RAII与单一transport链；没有新风格造成语义障碍。`git diff --check` 通过。 |
| 正确性 | ✅ 当前产品无发现；所有新增等待的取消边界与真实HTTP证据符合契约。⚠️ 测试完整性保留T1及PBT覆盖范围说明。 |
| 性能 | ✅ 未改队列上界/限速/并发；deadline timer只作用未准入段，未增加轮询；unit字段验证有16字节上界，复用原体积限制，无新增网络往返。同步短临界正常返回仍是契约前提。 |
| 可维护性 | ✅ send_inner、统一profile parser、admit/refresh共享旧实现，新增wrapper用于保留既有API和显式恢复语义；未发现重复预算/认证状态机或无需求的抽象。 |

## 原始测试与变异证据复核（独立代码结论形成后）

读取 `/tmp/b2-transport-mutations-siiy548x/artifacts/` 的 manifest、各 command/patch、baseline/M1–M5/restored原始log与exit，核对运行器的结果判定。未执行 Cargo 或外部服务。

- 将 manifest 中 178 个 `src/**`、Cargo.toml、Cargo.lock 条目的 SHA256 对固定提交逐项比对：全部相同。未读取配置文件内容。`original_transport.rs` 与审核版本逐字节相同。manifest按运行器 `json.dumps(m, sort_keys=True)` 算出的摘要为 `3ba1240a77971c8dfcfa426ab5e888bf0d14fdad949ef6d1798058981de36d28`，与记录一致。
- baseline/restored原始log均有 `Finished test profile`、`Running unittests`、`14 passed; 0 failed`，退出0（0.16s/0.19s）。这是已有证据复核，不是本人新运行。
- M1删除guard：编译成功；`balance_tests.rs:146` 的 unwrap_err 收到 Ok，退出101。
- M2删除队列timeout：编译成功；第738行独立1ms保护失败；proptest缩减反例 `class=0, millis=1`，退出101。
- M3删除refresh锁timeout、M4删除登录预算deadline：均编译成功，受控测试第51行 `bounded balance call did not finish` 语义失败，退出101。
- M5在401恢复传None：编译成功，第573行实际时间比原end迟28秒（30秒对2秒），退出101。
- 运行器对这些命令使用普通 `subprocess.run`，未用外部timeout/kill把挂死伪装成变异杀死；M3/M4是测试内部受控失败。五项证据能支撑这五项变异被杀，不能证明其它接线或输入域完整，尤其不消除T1。

## 范围与限制

本审核没有发现须修改的产品代码；本审独立发现1项测试准确性缺口，经动态证实后并入专项审核的F1。本人未执行任何修复、Cargo或部署；动态变异由主任务在独立副本运行，本人只读复核。汇总测试缺口未解决，因此B2局部C2不通过；B3–B8与整个第5批退出均不在本报告结论内。

## 动态逃逸补充复核

主任务完成四项独立变异后，本人读取 `/tmp/sgm8-b2-c2-coverage-inkv4m98/` 下每项patch、原始log、results.json及restored.json；未仅依赖汇总口述。每项日志均有 `Finished test profile`、`Running unittests`、`14 passed; 0 failed`，results均为exit=0。

| 变异 | 本人复核的证据 | 对应验证缺口 |
| --- | --- | --- |
| pending-deadline-extended | patch只改初始Pending调用为deadline+1s；原14测试仍14/0，0.16s；日志编译41.49s，exit=0 | 本审T1，汇总F1的一部分 |
| profile-deadline-dropped | 两个profile_until入口的Some(deadline)都改None；编译31.51s，原14测试14/0 | 专项F1：两入口没有测试阻塞时原deadline确实下传 |
| deadline-equality-accepted | check_deadline的`>=`改`>`；编译27.80s，原14测试14/0 | 专项F2：没有确定的等于deadline且可立即准入反例 |
| amount-narrowed-i32 | POST value从amount改`amount as i32`；编译30.37s，原14测试14/0 | 专项F3：大额正例未核对精确DTO值，cap会被截断 |

baseline和最终restored也均编译成功、14/0、exit=0。本人将source-hashes.json中178个src/Cargo条目对固定SHA计算hash，并核对最终project副本内容：无不一致；restored.json另外记录179原文件全还原（本人未读取配置内容）。因此这四项不是编译失败、测试筛选错误或未恢复源代码造成的假逃逸。固定提交的原产品实现仍然正确；失败的是现有测试对这些明确违约的判别力。
