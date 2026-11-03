# SGM-2 执行器组合轨迹独立审核

日期：2026-10-15。分支：`research/source-groups-token-management`。对照基线：`4b4a79e`。范围仅为本轮 worker 组合轨迹 PBT，不能据此宣称 SGM-2 整体验收完成。

最终结论：✅ 本轮组合测试增量通过独立审核；F1/F2/F3/F5 已修正并复核，无未关闭实现阻断项。F4 是必须保留的覆盖范围说明。四个新增性质均取得 128 cases 的独立通过证据，详见验证记录。

## 审核方法与独立性

先读 `CLAUDE.md`、workflow §5.2/§5.3/§5.4/§6.2–§6.6、架构的跨组/GLM/历史归属约束和 SGM-2 执行器、绑定、未知写与新增组合验收契约，再独立抽取下列预期。`expectations.md` 通过标题匹配在 `## Step 5` 前精确截断；未打开主审核结果或其他审计/测试记录文件。模板目录不存在，使用等价结构。

独立性补记：独立预期、差异分析及本报告初稿全部完成后，为查看实现方状态调用 `list_agents`，该工具自动附带历史 agent 完成摘要；未将这些摘要用于本轮预期或判断，亦未据其打开关联报告。此非所要求的完全隔离阅读，特此披露。

随后审 `git diff 4b4a79e` 中本轮代码/设计差异；未跟踪的 `scenarios.rs` 使用 `git diff --no-index /dev/null` 阅读。未通读未修改生产源码，仅补读被复用的 fixture、HTTP fake、finish/reopen 相关 helper，以及未知写分支的必要上下文。未改任何实现或测试文件。

## 独立预期

| 编号 | 从契约抽出的可检查条件 |
| --- | --- |
| E1 | 命令输入独立推进各成员的 status/priority/models；逐渠道检查 source/group/channel 归属、原生字段及 applied，不能只计数量。 |
| E2 | 同一操作 ID 重放不产生新效果；历史成功收据跨后续目标更新和真实重开保持不变。 |
| E3 | 单组优先级变更的所有写请求均限于该组；暂停/退役覆盖同源全部副本；GLM preserve/status=3/auto_ban 不受火山规则影响。 |
| E4 | 故障必须在真实 HTTP fake 被触发；部分成功不能冒充全集确认或提前归档；修复远端后仅通过原操作 retry 和正常 worker 收敛。 |
| E5 | 提升/启用前必须已完成对应降低/standby 和新鲜回读；最终状态正确不能代替中途屏障正确。 |
| E6 | 创建未知后只按稳定关联名查询，缺失/歧义/漂移不重发 POST；旧创建恢复后只绑定原身份，并服从最新停止/退役意图。 |
| E7 | 旧 ACK 只结算旧票，不能更新新代次 applied/成功收据；一般字段/状态未知写不能被匹配 GET、重开或 retry 清除。 |
| E8 | 不直接改延期、意图或未决记录制造成功；保留真实预算，使用私有临时 Store 和真实 open；不弱化原测试，不接线生产。 |

## 发现与处理状态

以下保留初审问题及冻结版本的修正证据。生产代码未因这些意见改动。

### ✅ F1：一般未知写的字段 PUT 分支已补齐

初审 `unknown_general_write` 仅提交 `SetSourcePaused`，不能捕获字段 PUT 专有的恢复退化。修正后的 `scenarios.rs:587` 由随机 `fields` 选择模型字段更新或暂停；逐副本核对实际 models/status，重开和 retry 后仍要求 unknown、确认数为非故障副本数量、来源未归档。`scenarios.rs:618` 另明确断言故障已消费。

独立复跑曾发现新增 fields 夹具缺少当前许可：最小输入 `groups=2, failed=0, retries=1, fields=true`，原断言得到 Waiting 而非 Unknown。原因是模型更新推进来源代次，旧许可失效，PUT 尚未发送。实现方在提交后重新发布可信测试许可，保留 Unknown 原断言并增加故障消费断言；没有修改 Store 意图或弱化失败条件。回归种子保留。

### ✅ F2：回读证据与未达目标的降低屏障已补齐

初审只检查 fake 当前远端值，无法区分“已降低”和“已回读降低”。`tests.rs:139` 现比较各同组成员的实际值与此前成功 GET 的值；`tests.rs:182`/`:197` 仅记录所返回页或实体。提升时还比较来自输入数组的 peer 目标，未调用生产计划器作 oracle。`scenarios.rs:308` 强制旧首选只降到 50 并返回拒绝，而目标为 10、新首选目标为 100；两成员均路由时明确要求新首选仍为 10、确认数为零，之后才正常重试。不能仅凭 50<100 提前放行。

### ✅ F3：所有实际写方法均已限制到命令目标集合

初审只过滤 PUT，可能漏掉越权 status POST 被随后补偿的轨迹。`scenarios.rs:106` 的 `only_writes_to` 现检查全部非 GET 请求，仅允许预期的 PUT/status POST，并要求 ID 属于该命令的独立目标集合；不认识的写路径直接失败。每个随机命令及最后退役均调用它，GLM ID=5 不在任何火山命令写集合中。

### ⚠️ F4：覆盖编号须保持增量边界

这些场景可补充相关 R 项，不能单独等价于完整 R1–R7/R9/R11 验收：未生成同 ID 异载荷、重复/跨实体错投 ACK、各创建崩溃窗口、批量 changed/遗漏 ID、v1 迁移回滚或私有材料保真。GLM 在混合轨迹中主要作为 status=3 的隔离参照；R11 在本轮证明重开及退役后的稳定四元组，而非迁移全覆盖。该项是必须保留的范围说明，不是要求本轮重做所有既有门禁。

历史核对已从仅比较 `OperationView` 补强：`scenarios.rs:95` 使用只读连接保存逐目标 membership_id/confirmed_at/receipt_json；`:388`/`:426` 在后续命令、重开与退役后逐项比较，确认原成功收据不被改写。

### ✅ F5：暂停输入现由独立参考状态推进

`scenarios.rs:324` 现以参考数组生成 paused 输入；`:231` 反向核对实际 Source.manual_paused，并核对来源类型。强制前缀同时包含暂停和恢复，避免随机尾部未覆盖恢复；source/group 当前 revision 只用于合法命令和可信许可，不作为期望结果 oracle。

## R 编号与本轮证据边界

| 编号 | 本轮增量证据 | 不据此追加的覆盖声明 |
| --- | --- | --- |
| R1 | 受理时和历史成功后重放原 ID/原命令，完整 OperationView 与逐目标收据不变、零新增 HTTP | 同 ID 异载荷冲突的全集性质 |
| R2 | 两源共享两组，逐命令检查全部写 ID 集合和五渠道最终字段 | 任意规模/任意模型集合的完全证明 |
| R3 | 暂停/恢复/退役、部分拒绝、未知创建和迟到 enable ACK 组合 | 远端硬实时阻断或撤销已发推理 |
| R4 | 迟到旧 enable ACK 后新目标未 applied/未成功，旧操作 superseded | 重复/任意乱序/跨实体错投 ACK 全集 |
| R5 | 坏创建回执、重开、0/多/漂移查询、恢复唯一对象、始终只一 POST/成员 | 所有崩溃边界、401 安全重放与完整分页专项 |
| R6 | 单副本拒绝，实际确认集合不完整且不归档；重开原操作 retry 后全集收敛 | 原生 batch changed=0/遗漏 ID 专项 |
| R7 | 部分降低到 50、目标 10 的拒绝；实际读回证据、standby-before-enable、目标屏障 | 任意时钟/并发调度与远端 CAS 保证 |
| R9 | 相同模型名的 GLM status=3/auto_ban=1 隔离；GLM/火山均参与迟到人工启用组合 | GLM 决策算法等价的完整 golden 验收 |
| R11 | 真实重开后 member/source/group/channel 四元组、历史成功收据保持，未知写不消失 | v1/v2 迁移、回滚、私有凭据材料全部保真 |

## 已确认的正向证据

- ✅ 生产代码、依赖与启动接线无本轮差异；`tests.rs` 只扩展 fake 和注册新测试模块。原测试主体/断言没有删除或放宽，Default 与原 fixture 的零值初始化等价。
- ✅ 五渠道参考数组逐项检查 status/priority/models/group/base_url/auto_ban，此外检查 observed/applied 和映射保持，明显强于仅比较数量。
- ✅ fake 使用 `127.0.0.1:0` 的真实 TCP/HTTP；故障在真实请求处理处注入，损坏回执在实际副作用之后返回；延迟启用在写入后延迟正文，能形成旧 ACK 与新命令交错。
- ✅ Runtime 虚拟时间推进 HTTP/预算，临时 SQLite 和 `CatalogStore::open` 为真实实现；未发现运行中直接修改 next_attempt_at、删除未决记录或覆写目标来制造成功。新增 GLM SQL 仅构造首轮运行前 imported 夹具。
- ✅ 未知创建使用真实先创建后坏回执、重开、损坏远端对象及修复；断言创建次数、稳定 ID、禁用状态和零启用。一般未知状态写明确断言安全停留，未伪造终止证据。
- ✅ 既有真实时钟、取消和队列边界测试未修改。每个新增 proptest 配置 128 cases，管理预算未替换。

## 独立验证记录

- `git diff --check 4b4a79e -- src/reconcile/worker/tests.rs docs/design/source-groups-token-management/subplans/sgm-2-channel-reconcile.md`：通过。
- `git diff --exit-code 4b4a79e -- Cargo.toml Cargo.lock src ':(exclude)src/reconcile/worker/tests.rs' ':(exclude)src/reconcile/worker/scenarios.rs'`：通过，确认本轮没有生产源码或依赖变更。
- `rustfmt --edition 2021 --check src/reconcile/worker/tests.rs`：通过，包含新增子模块；`PROPTEST_CASES` 环境变量未设置，采用代码明确的 128 cases。
- 首轮 `cargo test --offline scenario_pbt -- --test-threads=1`：4/4 通过，237.71 秒；该轮为补强前版本，不作为最终版本结果。
- 补强后冻结版本的同命令：3 passed / 1 failed，328.71 秒。late_enable_ack、shared_groups、unknown_create 各 128 cases 通过；唯一失败为上述 general_unknown 字段许可夹具问题，最小输入由独立 `--nocapture` 单跑复现。
- 许可夹具修正只影响 general_unknown，未改其余三个场景或共享 fake。独立单跑 `cargo test --offline scenario_pbt_general_unknown_survives_matching_get_reopen_and_retry -- --test-threads=1`：1 passed / 0 failed，57.58 秒，128 cases。故最终四个性质各有完整通过证据；不把此前 3/1 的整组命令伪写成 4/0。
- 全部独立运行仅假 key、私有临时库与回环端口，已通过工具 escalation；未读取生产配置/数据库或访问生产服务。`sgm2_scenarios.sh` 仅审新增差异，未用其中或其他报告的执行结论替代本审核。

复核代码指纹（SHA-256）：`scenarios.rs = dbdd622d6347d2f80dfa88f9d03076397fcf49803bb45a93172401b53975794c`；`tests.rs = 64bfcf595da05ded9e99ce29806512c32bd26ae4f2267b785d9133d9626b5452`。本审核主动编辑仅此报告；失败测试生成的 `proptest-regressions/reconcile/worker/scenarios.txt` 由实现方保留为回归种子，未改断言或生产文件。

## 结论边界

F1/F2/F3/F5 的修正及对应运行已复核。F4 为增量验收必须保留的范围限制。共享 fake 的既有测试未被弱化，主流程仍须汇总完整回归/路径 A 结果；本报告的独立运行范围是新增四项 PBT。此报告不构成真实实例身份/版本预检、原生协议验收或部署互斥验收；真实预检范围仍待用户选择，SGM-2 整体仍未完成。
