# 本地托管预检独立审核

审核范围：分支 `research/source-groups-token-management` 相对 `713803f` 的本地托管预检实现。仅本轮增量，不代表 sgm-2 或整个 feature 退出。审核者先读契约与 expectations 的 Step 5 之前内容，再读 diff；未读取 expectations 的 Step 5 记录或其他审计报告。仓库缺少 `templates/contract-audit/`，采用 workflow §6.3/§6.5 的同等结构。

结论：本轮独立审核通过，0 个未解决问题。发现 1 个内部维护与预检发布的并发活性问题（F1），已修正并独立复验。未修改实现、测试或契约文件，未提交。

## 独立期待（代码审查前提取）

| 项 | 契约来源与独立期待 |
| --- | --- |
| E1 契约源 | architecture 的 BackendReadiness、单写者及本地控制面约束；sgm-2「已确认的本地托管预检」；workflow §6.5。复用 boot 配置/PID，不追加远端注册、数据库指纹或进程管理框架。 |
| E2 字段 | readiness 私有、非前端反序列化输入；包含严格版本、已关闭原生自动恢复、管理读权限、实例核验及有限时效；缺失/冲突必须不可用。 |
| E3 值域 | 常规版本仅固定受支持版本；仅 v0.0.0 可由运行中 exe 的已冻结 SHA-256 映射；未知版本/解析错误不 ready。错误只用固定安全原因。 |
| E4 流程 | URL/托管配置→PID/进程身份/唯一监听→共享预算的 status/option/channel 读取→再次核对 PID/启动时刻/身份→发布许可；顺序不能由测试布尔值绕过。 |
| E5 行为 | 回环 HTTP 与相同端口直接绑定实际 exe/cwd/listener；自动启用明确 false 才通过；管理能力读取严格解析；不 setup/bootstrap、不写 option/channel、不生产来源/组许可。 |
| E6 时序 | 整体检查≤20 秒；许可从开始计≤60 秒；时间回退失败；worker 等待前撤销旧 ready/清观测，旧 epoch 的迟到结果不覆盖新命令/输入；不持数据库事务或 Core 锁 await。 |
| E7 不变量 | 字段/版本/时间窗口至少 128 cases 的独立性质测试；故障、自动恢复冲突、身份漂移、失效时间均不能成为 ready；原生进程验证使用真实 producer。 |
| E8 性能 | PID≤32B、stat≤8192B、ss≤1MiB/3秒、exe hash≤256MiB；阻塞 IO 不阻断 async 截止，超时任务仅有界只读；复用共享 HTTP 总时限/正文/预算。 |
| E9 安全副作用 | 不访问生产配置/数据库/key/3000/3001；测试只用临时目录/假凭据/随机端口；只清理自建 ss/测试进程；不读进程环境/命令行；回环禁环境代理且非回环保留原行为；不接现行 up/run 新写者。 |
| E10 跨实现 | native 用固定构建身份真实核对；已有 boot、management transport、worker 的调用语义不退化；零数据库 schema/配置扩展；证据与未覆盖范围清楚区分。 |

## 审核结果

| 项 | 结论 | 独立核对 |
| --- | --- | --- |
| E1 | ✅ | boot 仅新增版本 getter；未增加配置项、实例登记、数据库表/指纹。原 CLI/调度器未接线。 |
| E2 | ✅ | `BackendCheck` 字段私有，无 Debug/Serialize；失败通过 `unavailable()` 返回不可用证据，不用错误默认值制造 ready。 |
| E3 | ✅ | `preflight.rs:295` 仅 canonical 版本直接接受；只有 v0.0.0 分支调用运行中 `/proc/PID/exe` 的冻结哈希核对。原生测试走该分支，未知版本和其他 v0 构建被拒绝。 |
| E4 | ✅ | `preflight.rs:285` 及 `:326` 串联前后进程核对、三项管理读和最终 PID 文件重读；实际原生测试改变自有临时 PID 后得到 LocalBinding。 |
| E5 | ✅ | `local_target` 要求直连回环 HTTP/同 URL/同端口；exe/cwd 及 `ss` 的唯一进程归属均核对。原生 automatic-enable=true 被拒绝且值未自动改变，false 后通过；fake 成功路径三项均为 GET。 |
| E6 | ✅ | 撤销发生在首次 await 前，旧结果不能覆盖新外部发布；修正后复用 input_epoch，使内部观察/延期不拒收合法检查，发布时再次清观测（F1）。60 秒窗口与前后时间检查正确；20 秒/3 秒截止与取消清理经代码核对。 |
| E7 | ✅ | 两项新 PBT 均显式 128 cases，覆盖 PID/stat/监听唯一性/许可窗口及页面结构；固定 fake 和真实原生 producer 实测均通过。F1 的内部延期/观察两分支与迟到外部发布回归均独立通过。 |
| E8 | ✅ | PID 32B、stat 8192B、ss 1MiB、hash 256MiB 均有代码上界；文件/hash 由阻塞线程执行；ss 管道有限读取且 `kill_on_drop` 仅持有其自己的子进程；管理读复用 transport 预算及正文限制。 |
| E9 | ✅ | 独立运行只使用假服务、自有临时数据及随机端口。指定二进制只读，未访问其同目录其他文件。强制代理环境下本地测试通过；非回环仍使用原 builder 分支。 |
| E10 | ✅ | native 成功 readiness 来自 `check_local_backend`，worker 入口来自 `refresh_backend`，删除了该路径手传 instance_verified=true 的替代证据。现有管理恢复与 worker 发布机制继续复用。 |

## 发现与处理

### F1：内部维护可拒收仍有效的预检（已修正，✅）

修正前位置：`src/reconcile/worker.rs:357`。`refresh_from` 保存撤销后的通用 publication epoch，并用 `mutate(Some(revoked))` 发布结果。与此同时，`mutate` 在普通观察/延期时也推进该 epoch，而 `defer` 不改变外部输入的 `input_epoch`。因此，只要检查期间执行一次正常后台轮次，合法结果就可能报 RevisionConflict，backend 留在 unavailable；撤销本身清空观察并通知后台，增加了此交错发生的机会。

最小轨迹：一个未绑定且待创建的受管成员 → 开始 refresh 并等待检查结果 → `reconcile_once` 由于未 ready 将成员延期 → 释放成功的检查结果 → 通用 epoch 已不同，发布被拒绝。此轨迹没有新命令、新预检或配置变更。应保留对新外部输入的拒收，同时允许观察/延期等内部维护；可复用已有 input_epoch，不需要新的身份框架。

这是本轮新增组合入口的活性问题；不表示已经发生生产故障。原迟到结果测试只覆盖新的 `publish_backend`，未区分内部维护。

修正后 `refresh_from` 在 Core 同锁内核对现有 `input_epoch == revoked`，内部观察/延期/结算不会改变该值；成功外部命令、新预检或其他可信输入仍会使旧检查失效。发布前先完成 `wake_pending`，随后更新 readiness 并清除检查期间积累的观察。没有新增代次计数器或身份框架，契约已同步。

独立复验 `worker_preflight_accepts_internal_progress_but_requires_fresh_observations`：1 通过，实际执行无 channel 的 defer 和已绑定成员的 observe 两个分支，断言 publication 变化、input_epoch 不变、成功发布、缓存清空及待办唤醒。重新运行 `worker_late_preflight_cannot_replace_newer_publication`：1 通过，确认旧结果仍不能覆盖较新外部输入。测试专用 ready fixture 仅位于 cfg(test) 模块，公开生产/native 路径仍使用真实 producer。

## 独立执行证据

- `cargo test --offline local_preflight -- --nocapture`：9 通过，含两项各 128 cases 的 PBT；用时 4.98 秒。第一次在普通沙箱内因回环 bind 被禁止而失败，提权后同一命令通过，未修改测试期望。
- `cargo test --offline worker_late_preflight_cannot_replace_newer_publication -- --nocapture`：1 通过。
- F1 修正后以 `--locked --offline` 独立运行新增内部维护交错用例及既有迟到外部发布用例：各 1 通过，分别 0.10 秒和 0.05 秒。
- 单独运行复用能力的 `recovery_preflight_is_read_only_and_never_defaults_unknown_to_false` 及 `r12_only_one_explicit_false_is_disabled`：各 1 通过，覆盖只读 option 请求及重复/非字符串/未知值拒绝。
- 将大小写 HTTP/HTTPS/ALL_PROXY 全部设为 `http://127.0.0.1:1` 并清空 NO_PROXY 后，独立运行 `local_preflight_uses_real_process_and_readonly_management_budget`：1 通过，1.03 秒。
- `QUOTA_THROTTLE_NEWAPI_TEST_BINARY` 显式指向用户既有的 `new-api-v1.0.0-rc.20` 二进制，运行 `cargo test --locked --offline native_managed_channel_contract_on_fresh_database -- --ignored`：1 通过，51.92 秒。测试先核对冻结 SHA-256，再以自有临时 cwd/数据库/PID、假管理员和随机非生产端口启动进程；真实检查包含 v0 构建映射、自动启用 true 拒绝、检查中 PID 变化拒绝及 worker 实际入口。退出只终止测试自己的 Child。
- `git diff --check 713803f` 通过。

证据范围：本报告没有重跑全 feature 测试或读取主机械门禁结果；OS 故障注入（例如真实阻塞的磁盘/ss）未逐项动态复现，其上界和取消路径以代码核对为依据。不包含生产配置检查、部署切换、sgm-3 至 sgm-7 或整个 sgm-2 的退出结论。

最终核对的源码 blob：worker `df063533884f`，worker tests `4ffda129cc8c`，preflight `b75b9e633e5f`，preflight tests `2fe899015ac0`，transport `3f6309995eb4`，native tests `169bd2a2751e`。新增机械脚本另行以 diff 核对：schema 使用实际测试输出，基线副作用检查限定允许文件，未把原生隔离测试或全 feature 退出冒充为已覆盖；其执行结果不计入本独审证据。
