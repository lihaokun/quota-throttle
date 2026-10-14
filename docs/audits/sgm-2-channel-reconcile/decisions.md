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
