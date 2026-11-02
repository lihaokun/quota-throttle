# SGM-8 第 5 批 B2：调用层实现与模块审核

> 后续 C2 独立审核未通过：本报告中的实现期门禁和已知问题关闭记录仍有效，但没有覆盖所有必需反例。对固定提交 `9619729` 新构造的四项错误变异均逃过原14项测试，见 [C2汇总](b2-c2-audit-report.md) 和 [PBT矩阵](b2-c2-pbt-review.md)。不得再据本报告的“模块问题0”宣称 C2 或 PBT 完整。

2026-11-02，基线 `dae7c4e`，分支 `research/source-groups-token-management`，归 PR #5。**B2 实现、独立模块审查、门禁和变异验证完成，随本次本地提交交 C2。** B1 的 C2 结果保留于 `audit-report.md`，不能用它替代本模块提交后的审核。

## 实现范围

- 新增 `newapi/users.rs`：从 `/api/status` 读取严格校验的 QuotaUnit；管理员通过固定 POST `/api/user/manage`、`action=add_quota`、`mode=add`、Create 语义加额。正 ID、正金额且不超过共享 BALANCE_CAP 在进入传输前校验，不接受 id=0。
- 新增 `send_fenced_until`，与旧 `send_fenced` 共用 optional deadline 的 `send_inner`。原调用传 None，预算/认证/错误证据不变；有界入口限制队列、refresh 锁及登录准入等待，初始认证与 401 恢复使用同一绝对截止。
- 已准入 HTTP 保留原 execute/10s/体积限制，不受普通业务截止中途取消；登录未准入时超时不会制造认证熔断，已发登录仍落定真实状态。准入前、最终 guard 与准入后边界均核截止；超时返回 QueueFull/NotSent。
- ManagedTokens 的 `profile_until` / `observe_profile_until` 复用原固定 owner、principal 与角色/额度校验；旧 profile() 保持 Control/None。
- 同步纠正 `CLAUDE.md` 旧的“没有调余额 API”结论，引用已确认的 rc.20 调研；未读取实际配置、运行库或调用实际 NewAPI。

B3–B8 尚未实施；没有 worker 补足状态机、后台任务、HTTP 新路由、快照/UI 变更，也没有更改供应商额度、GLM/火山策略或 catalog。

## 设计对照与测试

设计源：`sgm-8-5-balance.md` §5.3.1–5.3.4、T-B4/T-B5/T-B15；共享 QuotaUnit/BALANCE_CAP 契约与既有 DeliveryEvidence、RequestClass、RequestSemantics。

| 契约 | 实现 | 验证 |
|---|---|---|
| 仅合法正 ID/金额可进入固定 Create 请求，guard 每次准入有效 | `users.rs::add_user_quota_until`、`transport.rs::send_inner` | 精确 method/path/body、cap 正例、非法输入/guard/连接失败零写；真实入口 hook 有正例 |
| 保留 200 false/500 false JSON/非 JSON429/超时的证据，不普通重试 | 既有 execute 与 envelope，新增调用不改分类 | `add_user_quota_request_shape_and_delivery`、`balance_admitted_add_retains_real_result_past_deadline` |
| 正有限比例、1..16 ASCII 字母数字类型，失败不填默认值 | `users.rs::parse_unit` | 合法 USD/CNY/最小正值/16字符；缺失、零/负/null/string、非有限/超长/非ASCII/空格拒绝 |
| Owner/role/principal 保留，Observe 与 Control 分流，旧 profile 不变 | `tokens.rs::{send_with_deadline,profile_with}` | 三入口类别及负余额保留；错误 owner/role/超安全整数拒绝；401 后 principal 漂移禁止重放 |
| 队列与未被选中等待有独立截止，64 逻辑槽不绕过、不泄漏 | `transport.rs::admit_checked_until` / `send_inner` | 持续 Control、独立续冷却、64认证等待者、128例 PBT；到期归还 waiter/逻辑槽 |
| refresh 锁/登录预算共享原截止；不取消别人的登录；已发登录不被普通截止取消 | `refresh_session_with_deadline` / `refresh_session_locked_until` | 初始 Pending、401 后锁/预算等待、持有者与等待者、已发登录越截止后落定认证且零原业务 |
| 401 只恢复一次，deadline/guard/principal 在重放前生效 | `send_inner` | 成功重放、guard失效、登录迟到不重放、401 等待原截止及 principal 漂移 |
| 即使内部先 Ready，guard 耗时跨界后也不执行 HTTP | `admit_checked_until` 返回前再次校验 | guard 前冻结时钟、进入 guard 后恢复并模拟耗时，明确 guard 已调用且零 HTTP |

测试在 `src/newapi/transport/balance_tests.rs`，共 14 项，其中 1 组 PBT 固定 128 例。测试等待本身有界：task 用受控虚拟时间 finish；PBT 未选中 future 另加外层完成判定，遗漏生产截止时会明确失败而不是永久挂住。

## 独立审核及处置

两位新上下文 reviewer：`/root/b2_contract_review`（从契约独立提取后对照）和 `/root/b2_transport_review`（传输并发与测试反例）。最终静态审核均无生产实现缺陷，剩余 blocker/major/minor 为 0/0/0；一致性、风格、正确性、性能、可维护性均通过。原始契约报告与预期：`/tmp/b2-contract-review-report.md`、`/tmp/b2-contract-review-expectations.md`。

| 发现 | 处置与证据 |
|---|---|
| 首轮对 Create HTTP500 + success:false 错误地断言 RemoteRejected | 既有 execute 已返回 TransportUnknown/ResponseReceived/500；修正测试期望，未改生产错误分类。首轮 focused 10 passed/1 failed 的日志保留 `/tmp/sgm8-b2-focused.log` |
| 负例直接 await 未选中 waiter，移除生产 timeout 会挂测试 | 增加独立有界完成判定，reviewer 复核关闭 |
| 401 场景仅覆盖已发登录，无法捕获恢复调用漏传 deadline | 增加 401 后 refresh 锁/登录预算持续阻塞的两个分支，检查原 end、零登录/重放、槽归还，reviewer 复核关闭 |
| Control/续冷却被永久 Observe 占槽掩盖 | 分别改为两个 Control 每秒轮换、无在途请求且持续延长冷却；两类等待原因独立，reviewer 复核关闭 |

slow guard 测试进入 guard 前暂停时钟，消除调度延迟导致尚未进入 guard 就过期的伪覆盖。另补 64 槽认证等待与 principal 漂移后拒绝重放；没有放宽旧断言或新增生产补偿逻辑。

## 门禁与变异

| 门禁 | 状态 | 日志 |
|---|---|---|
| B2 focused | 14 passed / 0 failed，含128例PBT | `/tmp/sgm8-b2-focused-final.log` |
| 默认并行 `cargo test --release` | 801 passed / 0 failed / 35 既有 ignored，0 filtered；777.42s | `/tmp/sgm8-b2-full-rust.log` |
| `npm run test:ui` | 60/60，0失败/跳过 | `/tmp/sgm8-b2-node.log` |
| `npm run check:ui` | 通过，精确60测试/8 PBT组/契约生成/DOM sink | `/tmp/sgm8-b2-check-ui.log` |
| SGM7 | 通过，26 个精确 Rust 名称；15 C/4 浏览器为清单核对，不冒称 Native 实测 | `/tmp/sgm8-b2-sgm7.log` |
| 浏览器原 full 命令 | 一次 92/92，0失败，fixture正常退出；在重型Rust结束后执行 | `/tmp/sgm8-b2-browser.log`；`/tmp/quota-throttle-sgm6-b-browser-bwctzy/report.json` |
| 格式 / diff | 5个变动Rust文件 rustfmt(skip_children)与diff检查通过 | 本轮命令 |
| 机械项 | 六个 Step 在对应函数内且全源唯一；QuotaUnit 字段/共享cap导入匹配；5文件源码范围，无依赖变化 | `/tmp/sgm8-b2-mechanical.json` |
| 独立变异 | 基线14/14、五项编译后语义失败、恢复14/14；179文件哈希恢复且与工作树一致，PBT128 | `/tmp/b2-transport-mutations-siiy548x/artifacts/summary.json` |

五项单点变异全部由独立 reviewer 执行。每项先恢复原字节、施加单一变异，编译成功后运行精确目标；五项均 cargo exit101，未用编译错误或外部 kill 充数：

| 变异 | 真实失败 |
|---|---|
| M1 省略业务 guard | guard=false 却返回 Ok，目标 unwrap_err 断言失败 |
| M2 删除准入队列 timeout | PBT 有界等待判定失败；最小反例 class=0、millis=1，测试未挂住 |
| M3 删除 refresh 锁 timeout | 受控虚拟时间完成上界断言失败 |
| M4 登录准入改无界 admit | 受控虚拟时间完成上界断言失败 |
| M5 仅401恢复分支丢失deadline | 原截止断言失败：预期2s，实际30s |

原始补丁、目标、命令、日志、退出码见 artifacts 下 `M1`–`M5` 对应文件；`baseline.log` 与 `restored.log` 均14/0。179文件清单 SHA256 为 `3ba1240a77971c8dfcfa426ab5e888bf0d14fdad949ef6d1798058981de36d28`，transport 原始/恢复 SHA256 为 `720032eef5947e5b22aa8d3ba5c2a89b4f670db6ba9349f9050d4f9d96183078`。未共享/硬链接主 target，只复制公开编译输入，未复制真实配置或数据库。

## 偏差与后续

功能/错误/预算契约无偏差；私有函数名使用 `*_with_deadline` 或 `*_locked_until` 表达 optional deadline，旧签名以 None 委托，避免改动 bootstrap/retry 等旧调用。本地提交后停 C2，由独立 reviewer 在该 SHA 审核；本轮模块审查不冒称提交后的 C2 已通过。本轮不推送或部署。下一实施模块为 B3，尚未开始。
