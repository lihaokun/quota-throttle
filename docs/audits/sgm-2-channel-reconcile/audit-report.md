# 管理传输与渠道接口第一批审核

日期：2026-10-15。范围：sgm-2 的 P1–P7。结论：本批实现可进入下一批开发；SGM-2 整体尚未完成，不能部署新协调模式。

这是第一批的阶段记录。后续 P8–P10 已完成第二批验证，见[状态库与持久操作审核](batch-2-audit.md)；本页末尾的剩余事项按第一批完成时记录，当前剩余门禁以 expectations 与第二批报告为准。

## 独立契约审核

独立审核者先读取设计与 expectations 前十节，再对照 f30e549 后的修改及新增文件 diff；没有以主 agent 的测试通过记录代替判断。仓库缺少 workflow 引用的 subagent prompt 模板，本次按 §6.5 的契约提取、diff 对照、问题分级与复核要求执行。

| 契约 | 结果 |
| --- | --- |
| 共享发送预算，登录及 401 重放计入，在途最多 2、背景最多 1 | ✅ 显式共享预算，普通请求不持有认证锁等待网络 |
| 有界入口、控制优先、取消释放 | ✅ 修正后包含等待 refresh 的请求，最多 64 个逻辑调用 |
| 单飞认证、token 不回退 root、刷新不 setup | ✅ 代次复核与刷新锁职责明确；先释放业务槽再登录 |
| 严格成功正文与未知写分类 | ✅ 业务 POST/PUT 及显式 setup 的不可解析回执均为未知结果 |
| 有界分页、完整性/覆盖标记 | ✅ 页码、总数、单页数量及 ID 顺序/重复校验；不只读首屏 |
| 受管字段更新、单组禁用创建、状态回执边界 | ✅ 未知字段保留，key/status 移除；不把 ACK 计成协调完成 |
| GLM 与火山边界、只读自动恢复预检 | ✅ GLM 创建沿用 auto_ban；火山为 0；NativeRecoveryCheck 不是完整 readiness |
| 敏感包装和生产隔离 | ✅ 新私有正文无 Debug/Serialize，不启用生产协调器；测试使用假凭据及自建进程 |

审核发现的四处问题已修复并由独立审核者复核：

1. 认证等待曾绕过发送队列容量。增加共享逻辑 permit，覆盖 send/bootstrap/retry_authentication，补 64 槽与取消登录回归。
2. New-Api-User 曾追加在静态旧值之后。改为 HeaderMap::insert，测试会话身份覆盖与 token 身份保留。
3. POST/PUT 的 malformed、缺 success 或超限回执曾与读失败同为 invalid_response。按写语义改为 transport_unknown，覆盖六个组合且不重复请求。
4. 显式 setup 曾绕过上述分类。unauthenticated 统一分类，补无登录/无重试回归，独立补审确认关闭。

## 测试与机械检查

- 普通完整测试：120 通过，0 失败；原生测试默认忽略，需显式指定测试二进制。原有 85 项全部保留。
- 新增 5 项 PBT，各 128 cases：预算所有子时间窗、错误脱敏、分页完整元组、字段保留完整对象、自动恢复配置未知值。
- 隔离原生验收：使用固定 v1.0.0-rc.20 二进制，全新临时库、假管理员/key、随机端口；写入前确认监听 PID 归属，退出仅终止自建 Child。覆盖旧客户端登录/列表/priority、新接口禁用创建、双组独立字段、启停回读、option 和 101 条分页。
- 原生二进制 SHA-256：90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205。分页 fixture 只写本测试新建的临时库，不读写生产库。
- `bash scripts/contract_audit/sgm2_batch1.sh` 通过：P1–P7 定位、共享枚举、旧认证入口移除、禁止破坏性接口、保护模块 diff 与旧目录真实输出 schema。
- release 构建通过；Clippy 保留 6 个既有风格警告，无新增模块警告。

定向变异在独立临时副本及独立 target 运行，未污染正常源码和构建缓存：

| 故意错误 | 捕获它的测试 |
| --- | --- |
| 业务发送绕过预算 | concurrent_401_singleflight_releases_slots_and_counts_login_budget：发送轨迹时间断言失败 |
| 第一页直接退出 | real_http_scan_reaches_second_page_and_reports_partial_coverage：完整性断言失败 |
| 更新时漏删 key | patches_preserve_unknown_identity_and_native_state_ownership：完整对象 PBT 失败 |

## 五维度复核与剩余门禁

一致性：本批不写目录 v2，不实现持久回执，不产生 applied_revision。风格与维护性：复用已有 HTTP/模型工具，只抽管理传输与渠道接口，无通用工作流引擎。正确性：审核问题已闭环，字段/响应均严格解析。性能：共享预算、响应上限和有界扫描；不重复探测来源模型。安全：不动 GLM 决策、生产配置或运行进程。

R8 尚缺将随机并发、401、429、多身份合并为真实发送轨迹的属性测试；目前这些交错由固定场景覆盖。R10 的错误脱敏有 PBT，但私有包装不实现 Debug/Serialize 当前靠静态审核，尚缺误加 Debug 的机械防退化门禁。这两项仍是 SGM-2 整体退出前的待办，不以本批通过豁免。

P8–P16、完整 BackendReadiness、OperationView 真实输出 schema、目录迁移与协调恢复、其余 PBT/变异及新模式双协议验收仍待后续批次。
