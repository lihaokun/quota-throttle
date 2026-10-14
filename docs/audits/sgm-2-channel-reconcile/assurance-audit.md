# SGM-2 R8/R10 防退化独立审核

日期：2026-10-15。分支：`research/source-groups-token-management`。审核基线：`6747ef32915338e98fe86bc82a9fb3b2395ac6c9`；对象为相对该基线的未提交 R8/R10 补齐及审核期间的修订。

结论：✅ 本轮 R8/R10 补齐的独立代码与定向测试审核通过；无未处置的阻断发现。该结论不代表全部 SGM-2 退出条件通过，不覆盖真实预检、部署绑定、运行构建归因或生产接线。

## 方法与阅读边界

按 `docs/workflow.md` §6.5 路径 B，先阅读 CLAUDE.md、workflow §3/§6/附录 A、细化设计的分布式/隐私/退出契约及新增验收节、主架构共享类型，再提取下面的期待。仓库没有 `templates/contract-audit/subagent-prompt-template.md`，本报告使用等价的“独立期待—差异证据—发现—验证—范围”结构。

随后检查 `git diff 6747ef3` 的在域文件；新增 `assurance_tests.rs` 和 `sgm2_assurance.sh` 用 `git diff --no-index /dev/null` 阅读。只为核实 Fake、发送/登录路径及既有边界、隐私测试读取必要上下文，未通读未改的完整业务模块，未读取其他审核报告。

阅读边界偏差：第一次读取 expectations 时范围过宽，工具输出包含了 Step 5 历史记录。已立即通知主 agent，获明确指示继续独立推导、记录此偏差；未将历史测试结果或主 agent 的本轮测试结果作为证据。因此本报告不声称满足“完全未接触 Step 5 历史内容”的更强隔离条件。以下通过记录均为审核者自行执行；收到的修订说明也均重新对照实际 diff。

## 从契约独立提取的期待

| expectations 节 | 本轮适用期待 | 审核结果 |
| --- | --- | --- |
| 1 契约源 | 架构、细化、共享类型与退出条件共同约束；不能从测试现状缩减要求 | ✅ 已交叉核对，候选预检方案不属于本次实现授权 |
| 2 Schema | OperationView 无凭据、原始错误或 payload 摘要；测试真实 store 输出 | ✅ 既有实际 OperationView 隐私 PBT 独立通过；本轮无 wire/schema 改动 |
| 3 枚举 | Control/Observe/Backfill 共用实例预算；不新造另一套分类 | ✅ 测试复用生产 RequestClass；无枚举变更 |
| 4 流程 | P2 每次实际发送计费，P3 登录及重放同样计费 | ✅ 真实业务/登录 HTTP 数与准入数对照；本轮生产发送步骤未改 |
| 5 行为 | 401 只作有界认证恢复，429 为管理流控；不能默认成功 | ✅ 四种 fake 序列必须最终成功或明确失败；原有认证/错误回归通过 |
| 6 时序 | 共享冷却、并发槽释放、取消与重登不绕预算 | ✅ 新的冷却时间 oracle 加原有单飞、取消、队列回归；不声称远端 CAS |
| 7 属性 | R8/R10 至少 128 cases；oracle 不读取被测桶剩余额度；直接检查敏感内部类型 | ✅ 两个新增 PBT 均配置 128；宏锁定禁止输出能力的类型，直接哨兵覆盖允许 Debug 的类型 |
| 8 性能/流控 | 1/s、burst=2、在途 2、背景 1、等待 64；虚拟时间有界，无真实长 sleep 驱动新增轨迹 | ✅ 新性质检查任意子区间，原有容量/预留槽测试保留；驱动最多 50000 个 10ms 步进 |
| 9 安全/副作用 | 仅隔离 HTTP fake/临时目录/假凭据，不碰生产；不改策略、数据库或依赖 | ✅ 本轮源码增量为 cfg(test) 记录与测试注册/断言；未访问生产配置、数据库、服务或真实 key |
| 10 变异与退出 | 绕预算和误加输出 trait 必须被防退化门禁捕获；小步不能冒充整体完成 | ✅ 负断言机制独立编译验证，计数与时间断言具备对应失效点；仓库定向变异的实际执行证据仍由路径 A 汇总，本报告不代签 |

## 主要证据与发现处置

### ✅ A1：实际 HTTP 计费、时间窗口及共享冷却

`src/newapi/transport/assurance_tests.rs:68` 的 fake 按唯一业务路径记录尝试并产生 success、401→success、429→success、401→429→success 四种序列。两个独立认证客户端持有同一预算，每波四个并发逻辑请求且有两种不同序列；固定前缀保证 shrinking 不能退化成从未登录/冷却的样本。`assurance_tests.rs:128` 的业务次数期待由输入模式独立决定；`:161` 统计实际登录，`:163` 检查业务加登录总数，`:166` 与全部准入事件数比较，覆盖登录、认证重放和显式 429 重试的计费。

`assurance_tests.rs:57` 检查所有连续事件子区间的 `count ≤ 2 + floor(duration / 1s)`，不读取 credit/last，也不调用被测许可计算作为期待。任意窗口可以缩到其中首尾事件，故该枚举足以检查此轨迹的桶上界。`:183` 保持驱动可运行，反复让出 reactor 后仅推进 10ms，避免 paused runtime 在 socket 尚未处理时自动跳到总超时；任务异常和有界驱动耗尽都会失败。

初版仅有 rate/burst oracle，429 后由请求自身 sleep，不能独立证明其他身份也遵守短冷却。审核提出后已修订：`src/newapi/transport.rs:237` 的纯测试事件保存冷却头处理时刻和下一准入序号；`assurance_tests.rs:168` 将事件数与输入计划中的 429 数对照，并在 `:170` 独立要求 fake 声明的 2 秒，在 `:171` 检查其后每次准入均不早于截止。序号不会误伤同一虚拟时刻、但头处理前已经在途的请求。最终版本已独立重跑 128 cases 通过，此项关闭。

### ✅ A2：负 Debug/Serialize 断言是真正的编译期门禁

`assurance_tests.rs:8` 的两个 trait 候选在类型未实现目标 trait 时只有唯一推导；误加 trait 后出现 E0283。该机制不读取源码字符串，不依赖格式化安全外层，也不为测试添加 Serialize 实现。`:31` 直接覆盖 transport/auth、raw channel、渠道操作/扫描、planner 的私有类型；`:51` 锁定 SecretString、SourceDraft、SourceMaterial、WorkTicket 的非序列化约束。分页累积器及 worker 的 Core/缓存/Plan/ReconcileWorker 分别在 `src/newapi/managed_channels/tests.rs:7`、`src/reconcile/worker/tests.rs:9` 直接断言。

审核者将同一宏放入独立 rustc stdin 样例：无 trait 时编译成功，分别添加 `derive(Debug)`、`derive(serde::Serialize)` 后均在对应负断言处 E0283，确认其失败机理。未修改仓库业务文件。这个最小样例不替代路径 A 对真实仓库类型进行的定向变异。

### ✅ A3：内部 Debug 与真实 OperationView 的互补覆盖

新增 `r10_internal_debug_checks_each_sensitive_field` 对 ExecutionEffect::Create、ExecutionEffect::Fields、PlanStep::Patch 放入可区分的 NAME/GROUP/URL/MODEL 哨兵，直接格式化并逐值检查。已有 `property_c7_no_secret_in_snapshots_or_debug` 覆盖实际私有来源、key/header/selector/指纹；`journal_effects_and_debug_exclude_private_material` 覆盖真实 WorkTicket/效果；`operation_views_and_direct_command_debug_do_not_leak_material` 覆盖实际提交返回的 OperationView 与 ControlCommand。后三项由审核者独立执行，全部通过。

初版 `scripts/contract_audit/sgm2_assurance.sh` 只跑 r10_，其调用的 worker 脚本也只运行 schema 样本，不能单独代表上述全部隐私门禁。审核提出后，最终脚本 `:12–14` 已显式加入这三项既有测试；本轮未弱化它们的断言。此项关闭。该脚本仍是补齐入口，完整 SGM-2 验收必须保留全量回归等其他门禁。

### ⚠️ W1：接受且必须保留的证据范围

时间 oracle 检查的是预算准入时刻，服务端轨迹用于业务/登录数量及路径计数；没有跨网络逐请求时间配对，也不证明物理网络到达硬实时。最终细化的“限流与隐私防退化验收”已明确该区别，符合分布式契约限制。不得将这组测试描述为所有网络到达时间窗、所有生产身份或任意外部写者的全量证明。

新增随机轨迹最多四个同时运行的逻辑请求，未独立覆盖 64 个排队者、全部取消/超时状态或完整 worker 重试状态机；这些继续由原有边界与后续组合验收承担。负 trait 断言阻止的是指定 trait 能力，不是对任意手写日志/新增类型的自动信息流证明。此范围说明不构成本轮代码阻断项。

## 独立执行记录

均为 `cargo test --offline`；涉及回环绑定的调用通过工具 escalation 执行。未启动原生生产服务。

| 验证 | 本轮独立结果 |
| --- | --- |
| 最终 R8 HTTP 共享身份/登录/重放/冷却 PBT | 1 项通过，配置 128 cases，20.16 秒；在冷却 oracle 修订后执行 |
| `r10` 过滤测试 | 5 项通过，含新增内部 Debug PBT（128 cases）和既有错误正文 PBT |
| `newapi::transport::tests` | 27 项通过，12.16 秒；包括原有 R8 桶 PBT、64 队列、控制优先、背景预留、共享长冷却、并发 401、取消、迟到 guard 不退款 |
| 实际 OperationView、私有来源 key/header/指纹、WorkTicket 隐私测试 | 3 项通过，5.75 秒；前两项 PBT 各配置 128 cases |
| 独立负 trait 编译样例 | clean 成功；添加 Debug、Serialize 各自 E0283，符合预期 |
| `git diff --check 6747ef3` | 通过 |

一次初始精确测试名筛选因模块路径不匹配得到 0 tests，未记为通过证据；查明实际模块后重新选取并确认上述三项真实运行。未重跑全量 suite、release、原生 NewAPI 集成或完整机械脚本，也未读取主 agent 的相应结果；这些不在此表的证明范围。

## 必须处置项与最终范围

当前无未关闭的代码/测试阻断项。主 agent 汇总时仍须保留 W1 的范围说明，在 decisions 中记录 A1/A3 的处置，并提供路径 A 的真实仓库定向变异和其他必需验收证据，不能用本报告的最小编译样例替代仓库变异。

本报告仅关闭 R8/R10 补齐的路径 B 独审。真实实例绑定、v0.0.0 构建归因与预检 producer 仍待确认；未实现候选预检方案，未授权部署，不能宣布整个 SGM-2 完成。审核者仅新增本文件，未提交。
