# SGM-8 第 5 批 B2：C2 独立审核

2026-11-02；固定被审提交 `961972902d929f004dda46f6be476b79326ffed9`，基线 `dae7c4e`，分支 `research/source-groups-token-management`，归 PR #5。

**结论：C2 暂不通过。** 当前 B2 产品实现未发现已确认的契约违例，但存在 **1 项 major、2 项 minor 测试证据缺口**。四个故意违反契约的变异均编译成功，现有 14 项 B2 测试仍全部通过，故不能依据原来的“128例 PBT、五项变异通过”宣称覆盖完整。此外，独立浏览器完整门禁为 **91/1，未通过**；定向复跑不能代替完整门禁或确定根因。

## 审核方法

- `/root/b2_c2_contract_audit`：先从设计独立列预期，再核对 diff、必要 helper 与原始变异材料，未读取作者通过结论。详见 [契约/五维报告](b2-c2-contract-review.md)。
- `/root/b2_c2_pbt_audit`：先抽取 B2 的 12 条不变量，分别核对 PBT 生成域、事件路径、oracle、断言与定向补足；不把 B3–B8 未实施列作 B2 缺陷。详见 [PBT 完整性矩阵](b2-c2-pbt-review.md)。
- `/root/b2_c2_validation`：预声明路径 A 机械检查，固定 SHA/191个 src、Cargo、schema 输入后独立执行默认并行 Rust、Node、契约与浏览器门禁。
- 主任务在全新 `/tmp` 源码副本及独立复制的 target 执行四个新增逃逸变异；原工作树、旧变异证据和运行服务均未改动。两位 reviewer 只读复核原始 patch、编译/测试日志、恢复哈希，没有冒称亲自执行这些 Cargo 命令。

遵循 workflow §6.5：路径 A=机械检查，路径 B=subagent 独立审核；汇总后作 §5.2 五维判断。模板文件缺失时用明确审核任务执行同样的契约优先、隐藏作者映射要求。仅审 B2，不是整个第5批退出审核。

## 必须处理的发现

| 编号 | 级别 | 缺口与动态证据 | 最小修正方向 |
|---|---|---|---|
| F1a | major（F1子项） | `tokens.rs:378/382` 两个公开 profile 入口的 `Some(deadline)` 改为 `None` 后，原14项仍14/0。现有入口测试只测快速响应/身份，预算PBT绕过入口，不能防止截止漏传。 | 分别从 `observe_profile_until` / `profile_until` 进入，在预算/认证等待持续阻塞时断言原截止退出、NotSent与资源归还。 |
| F1b | major（F1子项） | 仅将 `transport.rs:591` 初始Pending的认证截止延长1s，原14项仍14/0。`balance_tests.rs:376` 等仅用最多推进40s的finish等待最终结果，没有断言原end。 | 在初始Pending锁/登录准入等待及64槽场景中钉住实际结束时刻，不以“最终会结束”替代同一绝对截止。 |
| F2 | minor | `transport.rs:88` 的 `>=` 改为 `>`，原14项仍14/0。普通时钟的now输入会变成严格晚于截止；paused用例保持阻塞，由timeout本身挡住，没有空闲且恰到期先Ready的分支。 | 固定虚拟时间，空闲预算，deadline恰等于now，从until入口验证QueueFull/NotSent且handler入口0；保留到期前/后分区。 |
| F3 | minor | `users.rs:75` 的payload `value:amount` 改为 `value:(amount as i32)`，原14项仍14/0。精确请求体只验证17，cap正例仅计数而未核金额。 | 对cap及大金额断言发送值精确保真；大ID同类边界也核对。无需为了此缺口新增大型状态机。 |

以上是**测试没有挡住错误修改**，不是已发现当前产品中存在这些错误。真实代码仍使用 Some、严格截止、i64原值；本轮没有修改产品代码。

## PBT 完整性的准确范围

新增PBT有效验证了一个窄性质：固定Control/Observe各占一个在途槽，第三个未获选waiter到期退出、guard不调用、队列和在途计数归还。生成域为 **3个请求类别 × 1–499ms**，128例；没有生成不同准入、取消、冷却或认证事件顺序。增加次数不会生成当前策略中不存在的路径。

既有transport/token PBT与新增13个定向测试补足了速率/容量、401、身份、已准入HTTP等大部分契约，不能把“所有分支都必须有PBT”当成新要求。上表四处缺口先补最小定向断言并杀死对应变异即可；随机事件轨迹是可选增强，不强推大型模型。

整个第5批的PBT尚未完成：未知/年龄只降级、精确金额舍入参考、每笔before+amount界限、同owner取消/结算/封锁及不同owner隔离等四组已规划性质，主要属于待实施的B3/B7；B1/B2现有PBT不能替代它们。

## 动态逃逸证据

目录：`/tmp/sgm8-b2-c2-coverage-inkv4m98/`。基线、四项单点变异和最终恢复均出现 `Compiling quota-throttle` 并运行全部14项，均 exit0、14 passed/0 failed：

| 标签 | 单点修改 | 结果 |
|---|---|---|
| `baseline` | 原始提交 | 14/0 |
| `profile-deadline-dropped` | 两个profile入口Some→None | 14/0，逃逸 |
| `deadline-equality-accepted` | 截止>=→> | 14/0，逃逸 |
| `amount-narrowed-i32` | amount窄化为i32 | 14/0，逃逸 |
| `pending-deadline-extended` | 仅初始Pending的deadline+1s | 14/0，逃逸 |
| `restored` | 恢复全部原始文件 | 14/0 |

每项的 `.patch`/`.log` 与 `results.json` 保存精确变化和实际输出；`restored.json` 确认179原始文件恢复。专项 reviewer 另对 `source-hashes.json` 逐文件复算，副本和被审工作树均179/179一致。这里没有对每个变异复跑整个801项套件，也没有声称如此。

旧五项变异确实被检出，原证据仍有效；本轮证明它们不足以覆盖新增的四个反例。不得用“部分变异杀死”推断测试已完整。

## 门禁与最终处置

| 门禁 | 本轮独立结果 | 证据 |
|---|---|---|
| `cargo test --release`，默认并行 | 801 passed / 0 failed / 35既有ignored，820.69s；新增14项全部实际通过 | `/tmp/sgm8-b2-c2-rust.log` |
| Node / UI契约 | 60/0；check:ui通过，实际60项/8组PBT | `/tmp/sgm8-b2-c2-ui.log`、`/tmp/sgm8-b2-c2-check-ui.log` |
| SGM7 | 通过，26精确Rust名称；15 C/4浏览器只是清单，未冒称Native探针实测 | `/tmp/sgm8-b2-c2-sgm7.log` |
| 机械/格式 | 40项检查、5 Rust文件rustfmt及diff检查通过 | `/tmp/sgm8-b2-c2-mechanical.json` |
| 浏览器原full命令 | **91通过、1失败**，92个唯一(project,title)与清单一致；fixture正常退出，未生成成功full报告 | `/tmp/sgm8-b2-c2-browser.log` |
| 唯一5次实际定向诊断 | 原失败case Firefox-mobile，原断言/时限；5/0；reporter因重复注册退出1，不能算full通过 | `/tmp/sgm8-b2-c2-browser-diagnostic.log` |
| 最终完整性 | HEAD仍为9619729，191个src/Cargo/schema输入前后完全一致；仅主任务审查文档变化 | `/tmp/sgm8-b2-c2-final-state.json` |

独立验证报告 `/tmp/sgm8-b2-c2-validation.md`；39个原始证据文件的SHA256清单为 `/tmp/sgm8-b2-c2-artifacts.sha256`。机械脚本首版泛匹配 `Step B2.*` 误包括既有日志/账本的旧步骤，定位后按本轮六个具名标记聚合，各标记的全源唯一性与函数归属断言保留；这是审核脚本修正，没有更改源码或放宽契约。

### V1：浏览器红门禁的处理边界

Firefox-mobile 的 `API fixture clipboard denial reuses one reveal and changing identity clears the manual key`（`tests/browser/panel.spec.mjs:132`）在点击“启用”后等待 `#secret` 隐藏的原5000ms断言超时，展示区仍可见。原始trace/截图/上下文已复制到 `/tmp/sgm8-b2-c2-browser-failure`，未用后续运行覆盖。

离线trace显示后台usage work与点击/按钮禁用重建时间交错；这只是候选原因，尚未确定根因。此UI及测试文件在B2中零修改，未证实为B2引入的产品回归，也不能据此忽略失败。一次五次实际定向诊断全部通过，但自定义reporter不接受repeat-each的重复(project,title)，因此入口exit1；没有修改reporter或再补跑。最初使用锚定grep未匹配Playwright完整title的零用例运行另存 `/tmp/sgm8-b2-c2-browser-diagnostic-selector-error.log`，不计作实际诊断或测试通过。

原完整门禁保持91/1未通过，V1作为验证异常待定位或在后续真实完整门禁中重新核验；定向5/0不代表偶发已消除。V1不冒充已证实的B2产品缺陷，也不与PBT的F1–F3混淆。

源码、依赖、设计schema与被审SHA保持不变。本轮仅新增/同步审核文档，未推送、部署或访问运行数据。

五维汇总：实现一致性/风格/静态正确性/性能/可维护性未发现产品缺陷；**正确性验证证据不完整，不能给出B2无保留通过**。本轮不接受这些缺口为偏离，要求补测试并复核四项逃逸后重新审查。B2停留C2，不进入B3。
