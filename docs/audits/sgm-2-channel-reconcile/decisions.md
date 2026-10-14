# 渠道协调器审核处置

日期：2026-10-15。此记录覆盖前两批、第三批计划器、持久日志、绑定/回读与 HTTP 执行器；SGM-2 未达到整体退出条件。

| 审核项 | 决策和状态 |
| --- | --- |
| 认证等待无界 | 已修复；共享逻辑 permit 覆盖等待认证/发送/在途，取消释放 |
| 静态用户头覆盖登录用户 | 已修复；会话 insert 覆盖，token 保留配置，真实 HTTP 回归 |
| 写回执解析失败未归类 unknown | 已修复；业务 POST/PUT 与 setup 共用分类，独立补审通过 |
| 64 槽容量口径 | 接受保守实现：64 个逻辑请求包含在途；实际发送仍最多 2、背景最多 1；细化已同步 |
| R8 随机发送轨迹覆盖 | 保留为整体退出门禁；本批预算 PBT 与固定 HTTP 并发/认证/429 用例不能冒充完整组合 PBT |
| R10 包装类型防退化 | 保留为整体退出门禁；当前无 Debug/Serialize 不等于测试能杀死误加派生的变异 |
| NativeRecoveryCheck 与完整 readiness | 分批边界：当前只检查自动启用；实例/版本/能力/策略许可后续合并，禁止将其直接当作 ready |
| 历史 sgm-1 机械脚本 | 不修改其固定基线以适配新功能；第一批脚本单独验证保护范围及旧快照真实 schema |
| 部署与生产开关冲突 | 未部署；不读取/修改生产开关、不重启服务。正式切换仍须单独确认 |

第一批验证命令：`cargo test --locked --offline`、`bash scripts/contract_audit/sgm2_batch1.sh`、`cargo build --release --locked --offline`。原生验收需显式设置 `QUOTA_THROTTLE_NEWAPI_TEST_BINARY` 后执行 `cargo test --locked --offline native_managed_channel_contract_on_fresh_database -- --ignored`，不得传入已有服务 URL 或生产数据库。

## 第二批处置

- 收据子行丢失可能伪装成功：已修复。增加私有 expected_targets、成员类型恰好一目标约束，读取/重开核对；独立复核及丢一行/全部行回归通过。
- 迁移随机空间偏 GLM legacy：已补混合提供者、多组、无渠道成员的确定性回归，不将局部随机测试夸大为所有场景的证明。
- R1 完整 ACK 状态机：仍属第三批门禁。本批只验证本地意图/幂等/代次/历史，不声称远端确认逻辑已实现。
- 历史脚本：sgm1.sh 与 sgm2_batch1.sh 保留其阶段基线，不为当前新模块放宽旧保护清单。当前第二批执行 sgm2_batch2.sh，覆盖新增 SQL/操作 schema，并保护第一批和运行策略模块。
- 第二批验证与变异详见[报告](batch-2-audit.md)。生产迁移、重启和切换仍未授权执行。

## 计划器与发送门禁处置

- 迟到退款可能突破 burst：已删除退款路径，最终检查后重新取时间补桶再扣额度；长 guard 准入轨迹回归通过，不声称物理发包硬实时。
- permit/peer 判定测试不足：补独立字段矩阵和随机唯一创建集合 PBT，GLM preserve 属性推进到字段更新后的下一步。
- 未知禁用创建是否阻挡其他来源：按恒 status=2、先绑定再启用及单写者前提给予无绑定 Never/May 豁免，但保留集合/身份检查；已绑定 unknown 不豁免。
- 跨普通 revision 的启用消费、退役中 peer、execution revision 与 fence 发布：明确为持久执行器接线前硬约束，尚未宣称实现。
- 验证与剩余范围见[当前审核](planner-audit.md)。本小步仅注册纯计划模块，旧运行循环不变。

## 持久日志处置

- W1 多票/多成员阶段误报：按整项操作的目标汇总，uncertain 优先 unknown，其次 prepared→applying；其他成员 ACK 不遮盖未决状态。跨成员、prepared 补偿回归及多票乱序 PBT 覆盖，独审复核关闭。
- W2 私有材料迁移 oracle 不全：逐 SourceId 比较全部 SourceSettings 与 credential，包含 GLM 不同模型、多 header，以及火山模型/日上限/重置字段；保留公共快照、实例 ID 和历史操作断言，独审复核关闭。
- 迁移版本：冻结 v1/v2 DDL，追加 v3。旧 v1 fixture 先移除新增 v3 表才能构造真实 v1；旧“当前数据库版本”断言改用 DATABASE_SCHEMA_VERSION，wire=1 等业务断言不变。历史阶段脚本未放宽，新增 sgm2_execution.sh。
- 迁移失败按步骤回滚：v1→v2 成功后 v3 失败保留 v2 和初始 v1 备份，不声称整条链原子回滚。
- 本小步没有网络发送、绑定和回读完成入口；实际 DeliveryEvidence 生产者与 worker 继续列为硬性未完成项。详见[独立审核](execution-journal-audit.md)。

## 绑定与回读处置

- C1 已物化 prepared 创建阻挡下一请求：绑定事务将其转 uncertain，并以 Resolved+绑定证明效果完成；不伪造 HTTP 响应、不修改唯一 prepared 索引。直接 bind→Enable、矛盾 NotSent 回滚、迟到回执与重开回归通过，独审复核关闭。
- W1 重复安全条件：pending 的创建豁免集中为一个 SQL 常量，终端错误仅由一个 Rust 分类函数定义。没有增加通用工作流框架，独审关闭。
- W2 测试边界：补实际代次/写/重试性质、消费授权 tuple、future/占用 ID/绑定回滚和 GLM 消费写失败回滚。更广的多成员 partial→再写→延期/重开网络组合继续作为 worker 阶段 R3–R7 门禁，不用本步各局部测试替代完整组合证明。
- 原生构建版本：固定测试二进制的内嵌 module/vcs 确认为 rc.20/6ce7305、modified=false，但 `/api/status` 返回 v0.0.0。测试在启动前校验冻结 SHA-256，再使用测试已核实的 canonical version；生产预检规则未放宽，此差异交由真实 producer 显式处理。
- 当前没有自动 worker 或生产接线。代码、测试与独审见[绑定/回读审核](confirmation-audit.md)，正常服务未被重启或修改。

## HTTP 交付与执行器处置

- W1/W2 等待饥饿与停止优先：自身缺许可时持久延期，只有可执行的降低/禁用才作为当前阻挡；共享源停用在 LIMIT 64 前排序。65 成员窗口回归通过。
- W3 旧错误覆盖新发布：成功外部发布记录 input_epoch，内部结算/观察不改该值；延期在同锁内校验，旧错误不能重新延迟新唤醒任务。实际排队 HTTP 的最终 guard 只读原子与时间。
- W4/W7 正负缓存：新完整扫描缺失/漂移时清除旧正结果；新鲜负结果避免热轮询。人工 retry 非成功操作后统一清缓存，避免修复后立即重试仍失败；不引入逐目标缓存索引，不改变 unknown/授权/历史收据。
- W5/W6 读取与后台恢复：GET 5xx 暂时失败，最多三次后延期；lane 占用仅等待，不退出后台。过期创建对象刷新 GET 失败同样持久延期，Storage 错误不吞。
- W8 重复组装：阻挡诊断复用已装配组与缓存，线性遍历且不重新查询 SQLite；只识别降低/禁用，实际执行仍重新校验完整组屏障。没有引入通用任务框架或新计数器。
- 新增 record_observed 只更新观测诊断/排序字段；四重 revision、批次回滚、零 applied/收据副作用经固定回归与 PBT 验证。目录入口保留 DuplicateSource 的原 ID，不扁平化为 Storage。
- 交付证据要求受信管理端的可识别完整应用回执，代理错误页仍 Unknown；前置 GET 的错误不能冒充 PUT 已发。管理错误不计入供应商耗尽状态。
- 两类夹具错误均按真实前提修正：私有临时目录权限设为 0700；刷新创建对象测试将假服务名称设置为持久 create_name，保留原断言。未放宽业务校验或删失败测试，独审复核通过。

最终主验证：`cargo test --locked --offline --quiet` 为 240 passed / 1 ignored（54.33 秒）；ignored 原生测试另行运行 1/1 通过（39.22 秒），使用已校验构建、全新临时数据库和非生产随机端口。`bash scripts/contract_audit/sgm2_worker.sh`、release 构建通过；clippy 只有 6 条既有警告，无新警告。新增 31 项中 3 项 PBT 各 128 cases。独立路径 B 的结论见 [worker-audit.md](worker-audit.md)，不以主测试代替独审。

定向变异在独立 `/tmp` 副本运行，每次清理该副本的本包构建产物、单独编译和运行对应回归，确认是测试断言失败而非编译失败；每次恢复源码，未修改开发 worktree。8/8 被捕获：

| 删除或错误替换的保护 | 捕获用例 |
| --- | --- |
| 最终发送 publication 校验 | worker_pause_during_budget_wait_prevents_stale_enable |
| 延期的 input_epoch 校验 | worker_old_read_failure_does_not_override_new_ready_publication |
| retry 缓存失效 | worker_explicit_retry_observes_repaired_id_without_waiting_for_cache_ttl |
| LIMIT 前共享来源停止排序 | worker_supplier_stop_is_prioritized_before_the_64_member_window |
| GET 5xx 暂时错误分类 | worker_read_5xx_retries_then_remains_recoverable |
| 前置 GET 失败覆盖为 NotSent | delivery_field_get_failure_never_marks_put_as_dispatched |
| record_observed 四重 revision 比较 | confirmation_pbt_observation_is_atomic_diagnostic_only |
| 代理响应与完整应用回执区分 | delivery_complete_proxy_errors_malformed_and_oversize_are_uncertain |

五维度结论：契约与实现同步；遵循现有私有 Store/原生管理 API 边界；正确性回归与独审发现已关闭；无逐 peer 整组重载、无网络持目录锁；共享错误分类、源 gate 和客户端预算，没有双写者。真实预检 producer、完整 R8/R10 及跨模块组合验收仍是退出门禁，本次不将 SGM-2 标为完成，不接线或部署现行服务。

## R8 R10 验收补齐

- A1 短冷却缺少独立 oracle：已补。每波四个请求分属两个身份且响应序列不同；记录头处理时的冷却事件和后续准入起点，与独立生成的 429 数量及固定 2 秒 Retry-After 核对。所有后续身份的准入必须满足截止，前已在途请求不误判。独审最终重跑通过。
- A3 脚本隐私覆盖不全：已补。除 r10_ 测试，显式运行现有 key/header/selector/指纹、真实 WorkTicket、实际 OperationView 和 ControlCommand 的三项回归，不用新类型断言代替这些实际输出检查。
- W1 证据范围：接受并写入契约。时间窗 oracle 是预算准入时刻，服务端次数与业务/登录路径用于对照；不声称逐请求跨网络时间配对或物理到达硬实时。64 槽、取消/超时仍保留既有边界测试；完整 worker 组合验收不由这四并发测试替代。负 trait 断言仅防指定类型误加能力，不是任意未来手写日志的信息流证明。
- 独审阅读边界：审核者曾收到包含 Step 5 历史的过宽输出，已在报告披露；本轮期待从原契约推导，证据由独立执行产生，未依赖主测试结果，不宣称完全盲读。

主验证：`cargo test --locked --offline --quiet` 为 245 passed / 1 ignored（55.40 秒）；新增 5 项中 2 项 PBT 各 128 cases。`bash scripts/contract_audit/sgm2_assurance.sh`、release 和 diff 检查通过；clippy 仅原有 6 条警告。生产源码只有 cfg(test) 的观测事件和测试模块声明增量，旧运行算法、数据库、依赖和配置均不变；原生隔离测试本轮未重跑，保持此前的独立结果，不冒充新增证据。

6 项变异在独立临时副本、独立 target 中逐项运行并恢复源码：

| 变异 | 预期捕获及实际结果 |
| --- | --- |
| 业务请求跳过预算 | R8 HTTP PBT 运行时失败：准入与服务端请求数不等 |
| 登录跳过预算 | 同上，登录没有被计入预算 |
| 不发布共享冷却截止 | R8 HTTP PBT 运行时失败：违反独立短 Retry-After 截止 |
| ManagementRequest 增加 Debug | 仓库负 trait 断言触发 E0283，编译拒绝 |
| RawChannelDocument 增加 Serialize | 仓库负 trait 断言触发 E0283，编译拒绝 |
| ExecutionEffect Debug 输出字段 | 内部 Debug PBT 运行时捕获哨兵泄漏 |

路径 B 见[独审报告](assurance-audit.md)，0 未关闭阻断项。五维度复核：本轮仅增强既有契约门禁；测试辅助宏无新依赖，事件仅测试编译可见；独立 oracle、字段哨兵与仓库变异共同验证测试有效性；有限虚拟时钟驱动，无新增生产开销；共用既有 Fake、预算、隐私回归，没有另建运行框架。

真实预检未实现。候选本地托管方案不写 NewAPI option，但会限制首个 producer 的部署范围，因此先向用户确认是否本版还需远程 NewAPI。尚未得到选择，不自动以本地方案替代原目标；其他安全独立验收可继续推进，SGM-2 和完整 feature 均不标完成。

## 执行器组合轨迹处置

- F1 一般未知写只测状态：增加字段 PUT 分支，保留匹配 GET、重开和重复 retry 后仍 Unknown、未完整确认、未归档的断言。首次 fields 夹具缺少模型更新后的新代次许可，实际停在 Waiting 而没有发 PUT；补发可信测试许可，并明确断言故障已被真实请求消费，保留失败种子和原断言，不更改生产行为。
- F2 屏障只检查远端值：增加实际 GET 返回页/实体的回读记录，并对照输入数组目标；旧首选部分降低为 50、目标 10、新首选 100 时仍须阻止提升。回读记录只来自返回的对象，不标记未返回的分页项目。
- F3 只查 PUT 的归属：全部非 GET 请求都要属于命令目标 ID 集合，意外路径立即失败；同名模型的 GLM 渠道不在任何火山操作写集合中。
- F5 暂停 oracle 从 SUT 反推：命令改从独立期望数组推进，反向核对 Source.manual_paused/类型；当前 revision 仅作为合法操作/许可输入。历史比较补充只读保存的逐目标 confirmed_at/receipt_json，不仅比较 OperationView 数量。
- F4 覆盖范围：保留既有同 ID 异载荷、错投/乱序 ACK、不同创建崩溃窗口、批量 changed/遗漏 ID、迁移回滚和私有材料测试。本轮增加真实 worker 组合证据，不声称四个新性质单独覆盖所有 R 项，更不代替真实预检、原生协议和部署互斥。
- 审核阅读边界：审核者在独立预期及初稿完成后调用工具时收到历史 agent 摘要，报告已披露，未用其替代本轮契约推导/独立执行，不宣称完全隔离阅读。

主验证：`bash scripts/contract_audit/sgm2_scenarios.sh` 全过，其中完整 `cargo test --locked --offline --quiet` 为 249 passed / 1 ignored、368.51 秒。新增四项 PBT 各 128 cases；增加真实 SQLite 重开/收据核对导致套件耗时上升，不缩减 cases 或关闭持久化换取速度。release 通过，clippy 保持原有 6 条警告。生产源码/依赖相对 4b4a79e 零差异，只有共享测试 fake、新测试与文档；旧测试断言未删除或放宽，Default 初始化与旧零值等价。默认忽略的原生测试本轮未重跑。

5 项变异在独立临时代码/target 中逐项编译运行后恢复，全部是运行断言失败：

| 变异 | 捕获证据 |
| --- | --- |
| 跳过 peer 提升屏障 | 旧首选仍为 50 时，新首选误升至 100；期望保持 10 的断言失败 |
| 快照隐藏一般 Unknown | 应仅确认 1/2，却得到 2/2，逐目标确认门槛失败 |
| MayHaveBeenSent 再次生成 CreateDisabled | 未进行唯一绑定、不能在有界轮次收敛，未知创建场景失败 |
| 后续命令 supersede 已成功历史 | 原 Succeeded 回执变为 Superseded，历史重放比较失败 |
| 未等全部成员退休即归档来源 | 部分拒绝仍发生 source.retired_at，提前归档断言失败 |

路径 B 的每项性质均取得独立 128-case 通过证据，初次夹具失败与定向复跑如实记录，见[组合审核](scenarios-audit.md)。五维度复核：验收契约已同步；复用既有 fake/worker/Store 无新框架或依赖；独立字段/请求/收据 oracle 与故障消费断言补足正确性检查；所有循环有界且无新增生产开销；新增场景独立文件，旧回归保留。真实预检范围仍待用户选择，整体目标保持未完成。
