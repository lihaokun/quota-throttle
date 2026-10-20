# SGM-4 B3 只读组合及补归属审核记录

日期：2026-10-17。实施基线 `4aa89d6`，范围 B3.3–B3.7。源码分三笔本地提交：`3951d41`（ledger）、`c3e11b0`（catalog）、`1250445`（worker）。结合 B3.1–2 的[第一步审核](batch-b3-query-audit.md)，B3 本地退出验收通过；不代表来源分组整套功能或部署已完成。

## 实现边界

同一应用库的只读 WAL 事务覆盖身份、账本状态、完整分钟与秒级边缘；返回整体、模型、token、时间桶的一致结果。Any/Exact/Unknown token 分别走既有索引，有界 keyset 与过滤前扫描预算，不执行无界排序或 SQL 汇总。不创建库、不迁移、不触发采集/网络。

原 CatalogLock 只增加 Arc 共享生命周期，连接先关闭再释放 guard；裸 File 副本不成为持锁者。worker 查询单槽随真实阻塞任务退出释放，Core 只短持有，取消注册/SQLite interrupt、队列取消和固定 deadline 均有直接负例；发布前再次核 poisoned，不以普通写入令旧一致快照失效。

补归属每遍冻结 MAX ID、每批 256 行，永久映射包括退休成员；只细化未知身份、拒绝已知重贴。撤旧单元、加新单元、派生三列和必要 ledger_revision 同事务，消费原生字段与检查点不动。零 token 但有消费行的单元保留，错误行只补身份。提交成功后更新内存游标；重开/下一遍重扫，dry_run 不写或推进，不改来源/组策略、许可或 input_epoch。

本批无依赖/DDL/公开 schema 变更；未接 CLI、采集定时器、页面或火山自动调度。`pass_finished` 是遍历结束，不是数据完整/所有未知已解决。

## 测试范围与红例

本次新增 40 入口：37 普通、2 独立性能、1 显式趋势；包含 5 组各 128 cases PBT。整个 B3 合计 55 入口：51 普通、3 性能、1 趋势；9 组 PBT。查询参考值逐原始事实过滤求和；归属参考按独立身份表逐行核对，失败/重放与随机映射/重开轨迹不借被测 helper 生成预期。

保留的开发红例：tuple keyset 密集前缀 111218 VM steps；测试 Tokio/标准时基差异导致新请求误超时；测试编译中私有字段/方法名/SQLite main 参数类型不匹配。后续加强非空 runtime fixture 时把时间放在 epoch 初段，默认重置窗口下为非法输入，修为 86400 秒，未放宽生产校验或断言。

早期广义 `query_` 过滤意外选中既有 HTTP PBT，沙箱禁止临时 loopback 绑定，报 PermissionDenied；原测试与源码不变。环境回归种子保存到 `/tmp/quota-throttle-b3-readonly-sandbox-regression.txt`，不作为产品反例加入源码。完整回归单独获准绑定临时本地端口，仅假 key/临时数据库。

## 独审与五维度

只读组合、补归属分别由独立代理先抽契约再审 diff；反馈和处置见[decisions](batch-b3-completion-decisions.md)。复审静态项已全部关闭；代理未运行测试，不能替代下列执行证据。

- 一致性：覆盖 B3.1–7 的内部能力，不将账本观察口径或扫描完成提升为路由证据。
- 风格：保持中文步骤标记与私有类型；未引入通用 repository、第二账本或任务表。
- 正确性：身份/归属/计量原子性、稳定标签、错误无半结果、取消与锁生命周期均有正负例。
- 性能：256 行 SQL/补正批、有界结果、深 keyset；趋势另记读写交错与 WAL，不将 100 万当历史容量上限。
- 可维护性：复用现有 A3/A5、事务 poison、单写者与管理槽；只增加查询/补正入口，不复制 provider 策略。

## 执行证据

| 验证 | 当前实际状态 |
| --- | --- |
| 只读 worker 定向 | 7 passed；加入补正后的首次组合 12 passed |
| 目录只读 flags/关闭顺序 | 2 passed |
| 补归属最初定向 | 11 passed/1 性能 ignored；后续加强覆盖已纳入最终 37 项普通测试及全量执行 |
| 默认 debug 全量 | 496 passed/0 failed/15 ignored，534.28 秒；随后仅格式和 ignored 趋势测量入口调整，无生产逻辑变化 |
| 默认 release 全量 | 496 passed/0 failed/15 ignored，396.27 秒 |
| 独立串行性能 | 13 passed/0 failed，13.96 秒；100k 总体 120 ms、token 4 ms（预算各 5 秒），256 行/10000 映射补正 28 ms（预算 1 秒）；全部 11 项既有门禁保留并通过 |
| 200k/1m 趋势 | 最终两个独立进程各 1 passed；含快照内控制写分别 248/1143 ms；实际 scanned=200000/1000000；快照持有时 WAL 45352 bytes、11 frames/0 backfilled，释放后 TRUNCATE 至 0；含造数全程分别 12.32/65.42 秒 |
| 趋势内存 | 外部 GNU time 报告两个独立进程 Maximum RSS 分别 15356/15512 KiB，包含造数、不含编译；不是查询独占峰值或严格上界。早期同进程 `/proc` 的 15332/15304 KiB 反常下降记录保留，不用作可靠峰值或恒定内存证明 |
| 定向变异 | 16 项均编译后被断言捕获（各 1 failed/exit 101），恢复后正例通过；最终格式/测量入口同步后 src 逐文件比较一致 |
| 静态检查 | 最终 clippy all-targets 通过（11.63 秒），仅 6 项既有警告 |
| 机械/构建 | 精确 40 项清单、Step/范围/格式、空/全 ignored/漏项负例通过；37 普通全部通过，性能 runner 8 项入口回归通过；release build 通过 |

16 种变异为：删除 SQL interrupt、注册后不复查取消、颠倒读连接/guard 销毁顺序、状态/用量拆为不同快照、跳过扫描预算、错误日志计入用量、keyset 重读尾行、取消 token SQL 过滤、漏撤旧分项、no-op 增 revision、误删零 token 消费、错误冻结上界、跳过已知归属校验、每批重置游标、dry_run 写入、清空已有路由许可。三项直接由 PB3/PB4/PB5 捕获。整个 B3 累计 26 种变异执行，前 10 项保留于第一步审核。

全量保持默认并发，没有 RUST_TEST_THREADS/test-threads 覆盖；性能 runner 等全量与其他编译/变异结束后独立串行执行。未重编任何正在运行的同一测试可执行文件。15 个默认 ignored 项中，13 项性能单独执行，1 个趋势入口按两个规模独立执行；另 1 个显式原生 NewAPI 契约门禁未重跑，其 HTTP/渠道/CLI/boot 源码零变更，最近证据保留在 [B2 审核](batch-b2-audit.md)。未触发远端 CI。

日志位于 `/tmp/quota-throttle-b3-*`，变异副本 `/tmp/quota-throttle-b3-completion-mutations.rRV1MV`；临时文件可能被系统清理。复现入口 `scripts/contract_audit/sgm4_b3_completion.sh`、`scripts/tests/performance-gates.sh`、默认 debug/release cargo test，趋势另显式执行 `analytics_sql_large_history_trends -- --ignored --nocapture`。可将仅测试的 `QUOTA_THROTTLE_ANALYTICS_TREND_ROWS` 设为 `200000` 或 `1000000`，用 `/usr/bin/time -v` 包裹已编好的 release 测试可执行文件，为每个规模独立计量全程 RSS（包含造数、不含编译）。不重写第一步历史门禁或将历史原生测试结果称为本次执行。

## 运行隔离与剩余范围

所有改动在开发 worktree/分支 `research/source-groups-token-management`，没有读取生产配置/数据库/日志/环境/密钥，没有请求供应商或访问运行服务；没有重启、部署、推送、合并。

整个来源分组功能仍包含 SGM-3 C-2/C-3、SGM-4 C、访问 key 管理、三页面与发布接线。纯失败请求的可靠结束证据需要额外后端范围决定，不能以 B3 完成宣称这些已实现。
