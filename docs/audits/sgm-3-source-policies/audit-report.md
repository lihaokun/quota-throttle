# SGM-3 第一批实现审核

范围：共享 GLM quota 采集与原策略分组适配。基线 `e1a9279`，用户确认细化后实施。机械验证、独立契约复审、全量回归与构建均通过，第一批 A 完成。本批不代表整个 SGM-3 或三页面功能完成。

## 契约与独立审核

契约来源为[细化 §1–5、§7–8](../../design/source-groups-token-management/subplans/sgm-3-source-policies.md)、架构 G2/I1/I2/I3/I8 和[事前 expectations](expectations.md)。B/C 未细化的事实采集、状态持久化和发布接线不在本批实现范围。

workflow §6.5 独立审核者 `sgm3_batch_a_audit` 先抽取契约，再审核基线 diff 与新文件新增 diff，未读取主代理测试结论或实现位置列。模板目录缺失，按 §6.5 明文流程执行，未为此安装额外框架。

| 检查 | 结论 |
| --- | --- |
| 原 GLM 规则及 caller 行为 | 通过。冻结函数比对一致；保留 None→旧 active、active 优先 priority、pin_release 和临期规则 |
| 来源共享与组隔离 | 修复后通过。非 retired source/group 唯一性先于候选过滤；组/成员/来源/target 身份明确 |
| 未知和版本处理 | 通过。失败、缺失和不同 revision/cycle 不进入已知 quota；不是 0% |
| 原生状态与暂停 | 通过。暂停/退役意图高于 pin；不以原生 status=2/3 重写 GLM 算法 |
| PBT 的独立性 | 修复后通过。冻结原算法，分离来源/成员/渠道 ID，跨组共享变化有独立断言 |
| 副作用边界 | 通过。无数据库迁移、管理 API、模型周期查询、CLI 接线或运行服务修改 |

首审 R1/R2 阻断项与一项非阻断 fixture 建议均已处理，见[决策记录](decisions.md)。复审结论为无剩余阻断项；测试执行结果由下面的路径 A 单独报告，不冒称独审运行了全量测试。

## 自动验证

入口：`bash scripts/contract_audit/sgm3_batch_a.sh`。检查本批改动白名单、main 仅模块声明、四个原算法函数未改、测试参考与基线等价、P1–P7、共享枚举和纯适配器边界、格式及本批测试。公开 JSON/schema 与旧数据库未变，新增 JSON Schema 校验不适用。

新增 17 项测试，其中四项 proptest 各至少 128 cases：完整链路等价、组隔离与共享源传播、真实假 HTTP 一源一次、未知/旧版本等价于缺失。包含配置鉴权传递、TOKENS_LIMIT/CREDIT_LIMIT、超时后继续其他源、取消轮次、空输入、溢出、容量、暂停/退役/未绑定和原生状态边界。

第一轮网络测试因沙箱不允许监听而失败；放行随机回环监听后使用原断言通过。环境权限失败产生的临时种子不作为业务缺陷提交。测试不使用实际服务端口或真实 key。

重新编译后的机械门禁通过，本批 17 项测试全部通过；`cargo test --locked --offline --quiet -- --test-threads=4` 最终为 **277 passed、0 failed、1 ignored**，303.90 秒。原有 ignored 原生 NewAPI 集成未在本批运行，本批没有修改管理执行层。`cargo clippy --locked --offline --all-targets` 通过，仍只有原来的 6 条 sort_by_key/文档缩进警告，无新增警告；`cargo build --release --locked --offline` 通过。

## 定向变异验证

在独立临时源码副本 `/tmp/quota-throttle-sgm3-mutations.eFHZwp` 先跑通过 17 项本批测试，再逐项注入、执行、恢复；没有修改开发分支源码以运行变异。六项均在测试断言处失败，不以编译失败充数。恢复后与开发分支相关源码逐文件相同。

| 变异 | 捕获测试 |
| --- | --- |
| M1 预防/耗尽阈值 `<` 改为 `<=` | golden_thresholds_degraded_unknown_and_pin |
| M2 decide 返回 None 时清旧 active | golden_thresholds_degraded_unknown_and_pin |
| M3 未知 quota 填 0 | a4_unknown_and_stale_observations_equal_absence |
| M4 对同源重复请求 quota | a3_real_http_one_query_per_source_across_groups |
| M5 接受外组成员 | a2_group_isolation_with_shared_source |
| M6 用 MembershipId 查 SourceId 的 quota | a1_complete_glm_chain_matches_frozen_reference |

M3–M6 均由 PBT 收缩到最小反例；变异种子留在临时副本，不回写正式回归集。

本轮变异副本曾复用开发分支的 target 目录，源码恢复后 Cargo 命中了残留的 M6 测试产物，使一次后续门禁误跑变异程序。源码逐文件比对无差异；已使源码时间戳失效以强制重编译，重新执行本批和全量回归，残留产物运行不计入验收。以后变异副本必须使用独立 target，不能只隔离源码。

## 五维度自审与交付边界

- 一致性：实现对应 A 的类型和 P1–P7；新增构造/时钟辅助函数契约已同步。未提前实现火山账本或持久表。
- 风格：复用既有 ID、配置、QuotaProbe 和 GLM 决策，不重定义来源类型或阈值。
- 正确性：独审缺陷补有回归；合法时间保留旧规则，不可表示的上游时间差在适配器返回 InvalidInput。
- 性能：同源跨组复用单轮；容量有界、网络不持目录锁；最大目录输入纯计算测试限制 5 秒。保持原串行 quota 查询，不在本批另建调度框架。
- 可维护性：新生产代码限于 probe/glm 与小型公共类型；旧 tick 仅提取 priority 分支，HTTP、applied、dry_run 与快照位置不变。

来源探针和组决策目前只具备显式调用入口，尚未装配到 up/run。下一批为火山状态机和持久化细化；真实采集、访问 key、页面及生产切换仍需后续子计划验收。
