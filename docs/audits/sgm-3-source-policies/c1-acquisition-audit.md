# C-1 采集与整体验收

第三小步基线 `8fa37e1`，2026-10-16 完成采集小步及 C-1 整体验收。C1 来源输入与 C2 单飞网络链路复用前两小步的事务和许可发布。验收限于协调器显式刷新，不包括运行 CLI、火山事实、管理新命令、页面或生产部署。

后续复审补记：以下360 passed来自明确记录的受限并行命令。独立报告及本轮复现表明默认并行下固定墙钟断言会失败；该测试门禁缺陷不能被本历史结论覆盖。修复与新验证见[默认并行与性能门禁分离](../../fixes/sgm-fix-parallel-performance-gates.md)，原数字和代码审核范围保留。

## 实现与独立审核

依据 [C-1 细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies-c.md)、[事前清单](batch-c-expectations.md)。审核者 `sgm3_c1_acquisition_audit` 先从契约独立抽取要求，再读取本步 diff 和必要依赖差异；没有读取主代理测试结论、执行测试、写文件或访问生产。

C1 在同一只读事务最多扫描 MAX_SOURCES+1 行，复用原视图/材料解码，按 SourceId 取可采集 GLM 来源；控制行、材料或容量损坏返回 Storage，不返回部分输入。C2 只占一个 commands 槽，probe 锁覆盖完整采集与发布；Core 和读事务在 HTTP 前释放。取消直接丢弃 Future，已有 SourceProbe 消耗的编号不复用。完成后以当前目录调用同一发布 helper，没有新增周期任务、后台补采或另一套提交入口。

独审未发现生产实现阻断。超界测试夹具、真正的部分采集取消、确定性响应闸门和完整 peer 目标屏障四项反馈均已补齐并复审关闭，详见[决策记录](c1-acquisition-decisions.md)。随后按 C1–C5 连接复核，未发现额外权限放宽或提交链路偏移。

## 本步测试

新增 11 项测试：catalog 2 项、worker 9 项。其中 2 项 PBT 各 128 cases：

- `acquisition_pbt_shared_network_matches_simple_glm_model`：随机来源用量/失败及组数，经过实际 SourceProbe 和隔离 HTTP；独立计算无 pin/临期初始场景的 Normal/Degraded、active/目标与许可数，逐组核对，严格检查每源一轮一请求。
- `acquisition_pbt_pending_controls_never_authorize_old_source`：响应闸门内随机暂停、模型更新、退役及组数；控制操作先提交，再返回旧来源读数，检查旧版本不能授权，下一轮只查询仍可采集的来源。

确定性覆盖来源过滤、坏控制行/材料整轮拒绝、输入读取不写库、零来源、全部失败、dry_run、63 槽已占时仍能完整采集发布、64 槽拒绝、probe 单飞、两源中的第二源等待时取消/跳号、等待期添加成员及 managed 边界、来源各自时间形成不同 lease。

组合用例执行实际 SourceProbe → catalog commit/permit → 原 worker/FakeNative，两组轮转并验证原生 GLM status=3 保留、零 Enable。原 NativeState 屏障检查同时核当前回读、避免同级双 active 和 peer 已达到独立固定期望目标，不能只检查最终 priority。

最大目录复用既有 1000 来源/100 组/10000 成员用例。首次全量提交 11051 ms 超过原 10 秒门槛（359 passed、1 failed、1 ignored），没有计为验收通过。随后仅收窄写后回读范围：变化成员的实际 memberships/member_control 仍回读，其余同事务不变字段复用；逐字段对照、未知历史回归和独审确认该优化不改变契约。

优化后最终机械检查中输入组装 92 ms（预算 5 秒）、目标/许可提交 4655 ms、10000 个待办恢复 3107 ms（各预算仍为 10 秒）。额外通过仅测试 SQL 构造第 1001 条完整来源，验证容量拒绝且全表不变。

最终 `sgm3_c1_acquisition.sh` 机械检查通过：公开 v1 schema、原代码白名单/材料 helper 只改可见性、C1–C5 标记、许可/恢复/提交包装保持不变、无隐藏采集任务、格式；原 GLM/探针 17 passed（10.29 秒）、目录事务/许可/输入 22 passed（41.36 秒）、worker 发布/采集 17 passed（42.60 秒）。clippy 返回 0，仅有原有 6 条警告，release 构建通过。

最终冻结代码运行 `cargo test --locked --offline -j 2 --quiet -- --test-threads=4`：360 passed、0 failed、1 ignored，354.88 秒，退出码 0；保留原有忽略用例，最大规模预算在全量负载下也通过。期间不并行重编译同一 target，结束后只同步审核和进度文档；最终 diff/脚本语法/本步格式检查通过。

## 定向变异

初版独立源码和独立 target 中执行 8 项变异，均被捕获，恢复复跑输入测试 2 passed（0.51 秒）、采集测试 9 passed（44.22 秒）。全量性能失败后优化最终回读，再对最终代码重新执行下列 10 项变异：均成功编译，再由指定测试的断言失败捕获（0 passed/1 failed，退出码 101），不以编译失败替代检测。每项执行后恢复源码；最终副本与主工作树 src 逐文件比较一致。优化后恢复复跑目录 22 passed（41.84 秒）、采集 9 passed（42.58 秒）；输入/提交/恢复最大规模分别为 94/4833/2981 ms。

| 变异 | 错误改动 | 捕获测试 |
| --- | --- | --- |
| M1 | 不过滤暂停来源 | `probe_inputs_are_source_scoped_filtered_and_readonly` |
| M2 | 不过滤退役请求 | 同上 |
| M3 | 坏材料直接跳过 | `probe_inputs_reject_corrupt_required_rows_and_material_without_partial_output` |
| M4 | 缺控制行当作 retire_requested=0 | 同上 |
| M5 | 每次新建 SourceProbe，绕开共享单飞锁 | `acquisition_singleflight_single_slot_and_cancellation_never_publish_half_round` |
| M6 | 发布半段再占一个 commands 槽 | 同上 |
| M7 | 不保存采集轮次，取消后复用编号 | 同上 |
| M8 | 一轮内重复调用完整采集，模拟按组重复请求 | `acquisition_shared_sources_use_real_quota_and_keep_native_api_idle` |
| M9 | 不回读变化成员的最终 member_control，保留旧版本/指令 | `round_nonempty_unknown_execution_history_survives_target_change_and_reopen`（含逐字段对照） |
| M10 | 不回读变化成员的最终 memberships，保留旧 priority/applied | 同上 |

## C-1 整体证据对应

| 要求 | 当前实现及直接验证范围 |
| --- | --- |
| C1 / I-C3 共享采集 | `glm_probe_inputs` 同事务输入、SourceProbe 按源采集；网络请求数/selector/credential 断言、容量/损坏/暂停过滤用例 |
| C2 / I-C2 发布与取消 | 单槽/单飞/RAII、当前目录重读、共用 `apply_glm_cycle`；半轮取消、跳号、响应期控制与成员变更、重放/重开测试 |
| C3 原 GLM 与 Keep | A 的冻结参考与 4 项属性测试继续运行；事务小步 Keep/未知/临期/pin/退役及独立 SQL 模型，网络 PBT 补链路映射 |
| C4 / I-C1 原子性 | 同事务保存组/必要目标/许可/恢复；多组 SQL 故障 PBT、晚组/唤醒写失败、deferred-FK 不确定提交、关闭 fence 测试 |
| C4 / I-C5 / I-C6 历史和 no-op | 旧成功回执、非空 responded/uncertain 历史和消费状态逐字段检查；稳定 no-op/不新操作、仅恢复 NotReady 提前时间、实际 10000 恢复用例 |
| C5 / I-C4 资格与 lease | 最终 member/source/group 版本、当前 managed/绑定/ready/状态/可用读数、三方截止交集；固定身份/时限负向、128-case PBT 和实际不同源响应时间 |
| 发送安全与 dry_run | 既有 planner/prepare/fence/unknown-write/降旧回读门禁不变；五种远端写零票据、预先准备票据发送层拦截、真实采集驱动双组轮转与 GLM 不重启用 |
| schema/容量/副作用 | v4 DDL 和公开 wire=1 不变；SourceProbe/原 GLM 算法/认证不改；新脚本核对旧提交/许可函数逐字一致；私有临时库及随机回环测试 |

前两小步证据见[事务审核](c1-tx-audit.md)和[发布审核](c1-publication-audit.md)，其测试均由最终全量回归重新执行。C-1 三小步合计新增 39 项测试、6 项各 128 cases PBT、30 项最终定向变异（8+12+10，不重复计早期复跑）。结合本步最终变异、完整回归、release、三小步独审和下列五维度复核，C1–C5 及 I-C1 至 I-C6 的退出条件通过，无未关闭阻断或审核建议；这不代替 C-2/C-3 或整个功能验收。

## 五维度复核与边界

- 一致性：C1–C5 连接沿用确认方案，未增加运行循环或火山正向许可。
- 风格：复用原 SourceProbe、read_material、Config、publish/mutate 和错误枚举，无新依赖或协议实现。
- 正确性：请求前完成输入读取，网络无 Core 锁，完成后重新核目录；失败/取消不发布半轮，完整提交后整体替换。
- 性能：按来源而非成员读取，有限扫描/原单源 10 秒超时；写后仅回读变化字段且逐字段对照完整快照。输入、事务与恢复分别有最大规模预算，不承诺整轮网络只需 10 秒。
- 可维护性：手工批次入口只留测试包装，正式采集只走一个提交 helper；新增网络闸门仅在测试中。

本步未访问生产 config、数据库、运行端口或供应商，未重启、部署、推送或合并。完整功能仍有 C-2/C-3、SGM-4 至 SGM-7 及相关组合验收待完成。
