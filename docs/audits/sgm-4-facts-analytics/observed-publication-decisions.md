# 火山观测持久发布审核处置

日期：2026-10-18，基线 ce23dc7。本步 O4 的审核、夹具修正和范围裁剪记录。全量验收结果见[审核报告](observed-publication-audit.md)。

| 项目 | 根因与处置 | 验证入口 |
| --- | --- | --- |
| 旧请求跨撤错或重置 | 若过期队列直接丢弃，请求后续成功会被误认作新请求。仍持久化相位，只过滤计数输出，并用同事务规范事实 MIN 补齐未入队历史的首次时间；原 cutoff/period 不放宽 | old_queued_and_unqueued_failures、reset_history |
| 序号耗尽静默漏记 | 首审发现 INSERT OR IGNORE 同时忽略 CHECK，不仅唯一冲突。改成只忽略 native_log_id 冲突，CHECK 失败令外层 B1/B3 整事务回滚 | sequence_exhaustion、late_source_binding 故障注入 |
| 退休成员旧许可残留 | group_work 有意排除退休成员，不等于提供者的完整所有权。scope 改从完整 snapshot memberships 按 source 类型取得，不改变 group_work 或选择规则 | retired_members_are_removed_only_by_their_provider |
| 多组提交故障位置 | 原全局触发器在第一组写入时就失败；新增仅第二组 memberships/member_execution 更新失败，证明前组写入也回滚 | second_group_failure_rolls_back_prior_group_targets |
| 仅组归属改善 | 新增 source 已知、group 未知，随后组映射匹配的轨迹，确认不再入队/重计 | group_only_attribution_does_not_reenqueue |
| 密集秒边缘 | 2000 条同秒消费验证 raw-fact keyset；完整分钟由十万消费性能门禁覆盖 | observed_usage_exact_edges_budgets_and_overflow_fail_closed |
| 测试夹具错误 | 首次 ingest helper 从 Scanned 状态调用仅供 Scanning 的 load_scan_ticket，改读账本 generation。新增不支持类型夹具误写存储 JSON 外层，改为 $.settings.upstream；退休测试第二次使用旧 GLM cycle，改为新 cycle=2 | 原业务断言全部保留；未放宽生产 decoder、重放拒绝或选择器 |
| 变异揭示的中间断言缺口 | 漏存 request 相位的变异在“重复 S 与重复 F 同批”的末态断言中存活，错误清零和错误加一抵消。独立逐事件 PB2 已捕获该变异；确定性轨迹也须检查重复成功后、后续失败前的计数 | PB2；最终补强的 persistent_success_and_retries 红绿回归 |
| 范围外格式 | cargo fmt --all 意外格式化5个未参与模块。逐文件证明等于 rustfmt(基线) 后仅撤回自己的格式变化；后续显式文件列表且 skip_children | 机械门禁冻结 config/main/orchestrator/quota/status |
| 方案复杂度 | 保留一个 ledger/单写者/原 Runtime/选择器/执行屏障；只加待处理引用、请求相位、周期过渡三表，无第二计数镜像、代理、NewAPI 补丁、定时器或额外依赖 | 独立静态复审、冻结 DDL/依赖/算法门禁 |

独立审核者只读，不运行测试或访问生产。最终执行证据由主线程记录，不以静态通过替代全量、性能或变异。
