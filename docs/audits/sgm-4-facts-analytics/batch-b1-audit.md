# SGM-4 B1 事务入账审核记录

日期：2026-10-17。实施基线 `9595837`，实现提交 `fcda329`。B1 实现、独立静态审核、默认全量回归、定向变异及性能验证完成，本地退出条件通过。B2/B3、C、运行接线和部署不在本次交付内。

## 实现范围

依据 [B1 细化](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b.md)和[事前清单](batch-b1-expectations.md)，新增应用库 v5 的状态、两个采集方向检查点、规范事实及分钟联合用量表。没有新数据库服务、后台采集器、原生数据库直读或策略发布。

原始行先由 A2 解析/批内去重；事务中核票据和永久历史，只有持久库中真正新增的事实进入 A3/A4。事实、联合单元、检查点和 revision 同事务提交。原生规范字段不覆盖，已知身份按历史核验，旧未知身份不会因新映射出现而被重开自动修改。私有 metadata 使用闭合 version=1 codec，不给 NativeFact 增加通用 serde。

v4 只读核验与一致备份先于迁移，保留实例 ID、旧业务表、策略和执行恢复。已有 v5 在 open 的只读事务中按主键逐行验证全部事实，再经联合键索引双流核对完整投影；独占 guard 下后续无账本写入，不重复全扫。分钟 NULL 维度通过表达式唯一索引归一，不把内部 0/空串当公开身份。

旧 v1–v4 DDL、GLM/火山算法、NewAPI 生产认证、worker、CLI、面板与依赖不变。旧测试 helper 只移除新增 v5 对象以还原真实旧库；两个正向“当前 schema”检查承接 v5，旧备份/失败版本及逐字段断言保留。

## 独立静态审核

审核者 `sgm4_b1_contract_audit` 先独立抽取契约期待，再检查基线 diff、新增文件及必要局部声明；不运行测试、不修改文件、不访问生产。最终静态结论：无阻断代码问题，反馈已复审关闭。动态结果由下节单独记录，不由静态结论代替。

| 核对项 | 结论 |
| --- | --- |
| 事务与唯一身份 | ✅ 验票在写前；完整规范字段比较，新增投影与检查点成组提交；未知提交沿 Store poisoned 处理 |
| 归属及 NULL | ✅ 退休映射保留；已知 source/group 核对永久映射；合法未知状态保留；NULL 单元唯一 |
| codec | ✅ version/字段集合、整数/字节边界、元数据无通用 wire 能力，固定错误不泄漏载荷 |
| 重开核验 | ✅ 全事实逐行及联合索引双流，内存不随全历史事实数增长 |
| 迁移 | ✅ 保留旧复杂 fixture、备份和独立 prepared 恢复期望；当前版本检查不会放松旧版本备份检查 |
| 测试与机械门禁 | ✅ 24 个 B1 入口、3 个单独性能入口精确核验；六类 PBT 均 128 cases，参考模型独立 |

首轮发现的问题是错误分类和验证覆盖缺口，处置见[决策](batch-b1-decisions.md)：完整持久字段独立模型、随机规范冲突/晚溢出、双 lane/空页/重放、已知身份反向损坏、缺表/大数/晚行损坏、合法 JSON 的 16 KiB 边界等均已补齐。旧历史非法映射由 Fact 错误修为 CorruptData，容量超限仍为 LimitExceeded。

## 已执行专项与初期失败

新增 24 项入口，其中 21 项普通测试、3 项独立 ignored 性能测试；普通部分包含 6 项各 128 cases 的 PBT。

- PB1：分批、顺序、重复、重开，与独立 ID 集合及完整规范字段逐项对照，不只比较总量。
- PB2：随机规范字段冲突、晚溢出、事实/单元/检查点 SQL 故障及真实 deferred commit 失败，核全账本图像与 poisoned。
- PB3：source/group/channel 身份各异、同源跨组、退休与未知，核 source/group/native_group 及重开不变。
- PB4：直接从随机原输入计算联合单元，四种含未知项的边际与总体守恒。
- PB5：两个 lane 的独立状态模型、空页、重复事实、旧票据、fault 和重启，逐步核 generation/page/phase/revision。
- PB6：复用既有 B5 非空历史，随机 v1–v4、材料、策略及未决执行；原始字段图像与独立恢复期望核对。

加强后及提交前复跑 `sgm4_b1.sh` 均为 **39 passed / 0 failed / 4 ignored**（最后一次 39.02 秒），包括 A 的 18 项普通测试和 B1 的 21 项普通测试。性能 runner 模拟回归 8 项通过。额外伪造空清单、全部 ignored 清单，B1 入口均以 exit 1 拒绝。clippy 返回 0，仅既有 6 条排序/文档警告，本批新增警告已修正。

初期编译曾调用未启用的 prepare_cached，已改为现有 prepare，没有加依赖/功能开关。加强后的大数 fixture 曾同时改了 Idle 的 history 行，在目标调用前触发 CHECK（38 passed/1 failed/4 ignored）；已限定只改 recent，保持原边界断言，随后上述专项通过。原失败记录不冒充生产错误或变异检测。

## 最终动态退出记录

默认全量保留原并发（移除 RUST_TEST_THREADS，不指定 test-threads）：debug/release **各 407 passed / 0 failed / 9 ignored**，分别 425.15/315.37 秒。9 个 ignored 为原有 1 项和显式单独运行的 8 项性能入口，没有把失败用例忽略。全量执行期间未重编译其正在执行的测试二进制。

串行 release 性能门禁 **8 passed / 0 failed**（6.04 秒），预算均未放宽：

| 本批门禁 | 实测 | 原预算 |
| --- | --- | --- |
| 1024 事实/10000 历史映射入账 | 114 ms | 5 秒 |
| 100000 事实及联合投影的重开核验 | 460 ms | 10 秒 |
| 非空复杂 v4→v5 迁移/备份 | 22 ms | 5 秒 |

既有门禁同时通过：GLM 输入/事务/恢复 14/1158/841 ms，复杂历史迁移 19 ms，v3 策略迁移 18 ms，A 最大批次 25 ms，火山最大候选选择 5 ms。性能入口及模拟回归已纳入现有 CI 调用路径；本次未推送，未触发远端 CI。

资源趋势另行在无全量测试/编译并发时采样：原 10 万条夹具再次重开 445 ms，进程最大 RSS 12136 KiB；双隔离副本仅扩大同形夹具到 20 万条并调整独立期望，重开 838 ms，最大 RSS 12200 KiB，整体夹具运行 1.55 秒。两组都核完整记录总量，20 万样本只记录趋势，不替代或放宽主树 10 万条的 10 秒门禁。RSS 是整测试进程的 /usr/bin/time 采样，且该夹具使用简单 metadata；支持本次流式趋势，不冒称所有数据形状的硬内存上限。趋势副本随后恢复，src 与主树逐文件一致。

最终 clippy 返回 0，仅既有 6 条警告；`cargo build --release --locked --offline` 返回 0（19.61 秒）。构建只在开发 worktree，没有替换或重启运行服务。

## 定向变异

独立源码和独立 target 下执行下列八项。每项均成功编译后由指定断言捕获（exit 101，1 failed）；源码逐项还原，最后与主树 src 比较一致，不以编译失败计数。

| 编号 | 故意错误 | 捕获测试（catalog::store::tests::ledger 下） |
| --- | --- | --- |
| M1 | 持久重复事实再次投影，INSERT OR IGNORE 掩盖重复插入 | smoke_ingest_reopen_and_replay |
| M2 | 同 ID 异值删除旧行后覆盖 | same_identity_conflicts_in_every_field_without_partial_writes |
| M3 | 按 request_id 合并不同消费日志 | properties::pb1_cross_batch_dedup_replay_and_reopen |
| M4 | 从历史映射排除退休成员 | retired_mapping_group_mismatch_and_newly_resolvable_unknown_survive_open |
| M5 | 联合投影漏 token_id | properties::pb4_joint_cells_match_independent_raw_model |
| M6 | 检查点写入前拆开事务提交事实/投影 | late_overflow_and_sql_failure_roll_back_facts_cells_and_checkpoint |
| M7 | NULL 单元索引丢失 UNIQUE | null_cell_unique_constraint_and_metadata_redaction_are_enforced |
| M8 | 吞掉累计溢出并保留旧单元数值 | late_overflow_and_sql_failure_roll_back_facts_cells_and_checkpoint |

## 五维度与发布边界

- 一致性：实现限 B1 存储契约；ReachedEnd 只是本次遍历结束，不能变成完整遥测或火山许可。
- 风格：复用 A、CatalogStore、固定错误与 commit；没有第二份身份登记或批次收据框架。
- 正确性：唯一身份、NULL 索引、原子提交、已知归属与未知保留均有独立对照；错误变异被捕获，默认全量无回归。
- 性能：入账有界，历史核验逐行/双流；计时预算只在串行 release 入口执行，阈值未放宽。
- 可维护性：SQL 读写、私有类型/codec、核验和测试分开；旧模块改动限版本承接、helper 复用及测试 fixture。

本轮未访问生产配置、密钥、数据库或运行端口，未重启、部署、推送或合并。应用库 v5 本地实现不等于已对运行实例做迁移。
