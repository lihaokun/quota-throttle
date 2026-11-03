# SGM-4 B3 查询与补归属事前契约清单

日期：2026-10-17。设计基线 `9d6a67d`，实施基线 `603f83f`。用户已确认，第一小步 B3.1–B3.2 见[查询计算审核](batch-b3-query-audit.md)，B3.3–B3.7 已完成[只读组合/补归属验收](batch-b3-completion-audit.md)。以下十节是实现前提取的契约，保留原期待；执行结果写入审核，不从实现反推或削减期待。

## 1 契约源

- [B3 函数细化](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b3.md)：完整字段、步骤、并发/恢复与固定资源预算。
- [架构](../../design/source-groups-token-management/architecture.md)：G5/G6、单应用库/单写者、永久身份、统一计量与服务隔离。
- [B 总体](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b.md) §2–4、§7：一致查询、分钟边缘、补归属、容量和取消。
- [A 规范事实](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics.md)：白名单、未知标签、精确整数、历史映射及联合投影守恒。
- [B2](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b2.md)：持久采集进度不是完整性证明，不能以查询启动新的采集。
- [产品需求](../../research/source-groups-token-management-research.md)：近 24 小时、近 30 天及按天查看小时；整体/per-key 与模型堆叠口径一致。
- [workflow](../../workflow.md) §4.3/4.4/6：函数论证、事前十节、PBT、机械检查及实现后的独立契约审。

## 2 数据结构与字段

查询结果必须携带相同的区间、显式固定时区、ledger_revision、总计与时间桶/模型分项，消费行数不叫最终请求数。未知模型/来源/组/token 保留独立身份，不能用展示字符串替换 None。本批只提供内部类型，不公开 NativeFact 或凭据；HTTP JSON/schema 留在对应页面接口小步。

补归属仅允许更新派生 source_id/group_id/attribution_issue 与关联分钟投影及必要 ledger_revision，不能覆盖原生字段、日志 ID、token、模型、时间、输入输出或采集检查点。

| 类型 | 字段与检查 |
| --- | --- |
| UsageQuery / UsageFilter | window/utc_offset_minutes/grain/filter；filter 含 source/group/token/model；严格值域、无隐式主机时区 |
| UsageSnapshot | query/state/usage；LedgerState 来自同一读事务，未初始化 iff usage=None |
| UsageReport / UsageBucket | total/models/tokens/buckets；桶 from/to/total/models，范围连续，分区各自守恒 |
| AttributionCursor | instance/after_id/through_id 私有，只有 Store 构造；0≤after≤through，正 ID 可超过 JSON_SAFE |
| AttributionBatch / Report | checked/changed/ledger_revision 与 next 或 pass_finished；Report 另含 preview_only，结束不代表未知为零 |
| UsageReadTarget / UsageReader | 私有路径/instance/Arc guard 与只读连接，连接先销毁，不能从客户端 JSON 构造 |

v1–v5 DDL、公开 CatalogSnapshot/OperationView schema 不改。B3 内部带标签/事实/路径的类型无 Debug/Display/serde，wire schema 暂不适用，不是免除未来页面 schema。

## 3 枚举与常量

复用 SourceId/GroupId、UsageTotals、AttributionIssue、LedgerState/ScanProgress、JSON_SAFE 与安全错误，不另定义提供者类型、请求终态或路由许可。新 UsageGrain 仅 Hour/Day，DimensionFilter 仅 Any/Unknown/Exact；AnalyticsError 为 Ledger/Control/InvalidQuery/LimitExceeded/ArithmeticOverflow/Cancelled/TimedOut/Busy，具体载荷按 B3 §4，不带自由文本。

预算单处定义：31 天/745 小时桶或 32 日桶、256 行 SQL 页、1000000 扫描行、1024 模型/4096 token/65536 桶模型项、查询单槽/10 秒/读 busy_timeout=0、补正 256 行；MAX_HISTORY 复用 A 的 10000。时区范围 −720..=840，ID 正 i64，计量仍安全整数。

## 4 流程步骤

| 标记 | 必需顺序 |
| --- | --- |
| B3.1 | 完整输入校验→分钟/边缘互斥分区→显式时区桶 |
| B3.2 | 过滤→容量先验→精确累计全部分区→稳定全结果 |
| B3.3 | usable/guard target→只读连接→同事务身份/状态→有界 keyset 扫描→关闭 |
| B3.4 | 查询单槽→可取消短 Core→阻塞任务→实际退出释放槽→发布前 usable/截止检查 |
| B3.5 | 固定上界→同事务读最多 256 事实/永久映射→单调细化/检查游标 |
| B3.6 | 撤旧单元→加新单元→仅改归属→必要 revision→commit→发布 next |
| B3.7 | commands/repair 单飞→dry_run 或一批→提交后内存游标；不接定时/页面 |

这些标记标在对应入口，不能仅在模块说明中凑齐计数。

## 5 行为契约

- 相同过滤/窗口/快照下，总计等于模型分项和各时间桶之和；多 key 汇总使用同一快照，未知 token 不丢失。
- 只计 type=2 的 input+output；错误记录不计 token，零 token 消费仍计 consumption_rows。不把缓存、积分、倍率或重试次数再次加算。
- 完整分钟读联合投影，非分钟边缘按原始秒精确补算；同一分钟内的短窗也必须只算一次。半开区间和固定 UTC 偏移不依赖主机时区或火山 01:00 重置。
- 同名/改名/退休来源、成员或删除访问 key 不改变稳定历史身份；组归属不根据当前 token 组或展示名重写。
- 查询纯本地，无 NewAPI/供应商请求、无页面乘数轮询、无按查询触发补采。
- 空桶至多表示没有已观测消费，不宣称完整零用量；扫描完成或相同分页计数不授予完整性。
- 不能以 Top-N 截断冒充全部模型总量；容量/溢出/超时失败不得发布部分图表或默认零。
- 补归属只解未知项，已知身份不能改贴；历史映射包括退休成员。每批撤旧、加新与改事实同事务，重复执行不得增量累加。

## 6 时序和状态

查询不跨 await 持有 Core/写事务；一致快照与有界取消需证明不会把长查询挂在控制路径。只读连接不能创建/迁移库或成为第二写者；生命周期须跟随原 catalog 锁，不允许替换目录后旧读者混读新库。

READ_ONLY/query_only、WAL、busy_timeout=0；只读任务持同一 Arc guard，原 CatalogLock 不 Clone。仅裸 File 副本存活仍应按旧修复立即解锁；实际查询/target 存活则阻止新 Store，最后连接关闭后才解锁。query future 取消后，已启动阻塞任务仍持唯一查询槽直到真实退出；取消先于 handle 注册、两条 SQL 间取消、队列尚未启动均有负例。普通写不使旧一致快照无效，但 poisoned 后不能发布健康查询结果。

补归属通过现有单写者，事实归属/分钟计数/账本 revision 只在成功事务后发布，不修改采集检查点；确定失败整批回滚，未知提交沿 poisoned 停写和重开核验。游标不能因映射晚到、重启或中途取消永久遗漏旧未知事实；查询看到补正前或后的一致状态，不得见到半次搬移。

游标每遍冻结最大事实 ID、逐页检查所有事实，无法解析也推进；遍历结束下一次从 0 重扫，重启丢内存游标也重扫。不要求遍历期间 ledger_revision 不变；changed=0 不写 ledger_revision。错误事实可补归属但不动分钟用量，零 token 消费按行数保留。dry_run 不推进游标或写表，query 不调用 mutate，不改 publication/input_epoch；repair 沿 mutate fence 但不改策略、许可、input_epoch。

## 7 属性与反例测试

至少六组 PBT，每组不少于 128 cases；参考值用独立逐事实过滤和整数求和，不调用被测投影/分桶/归属 helper。

1. 时间窗口分区：随机秒边界、同一分钟短窗、整分钟/小时/自然日、正负偏移和整数边界；不重不漏。
2. 多维守恒：混合模型、未知维度、共享来源、多组、多 token、错误/零消费；总体与桶/模型/per-key 同口径。
3. 历史身份：改名/退休/删除引用、同名 token、原生组不匹配、后到映射；不串账、不丢历史。
4. 补正原子性：批次边界、重放、失败注入、commit unknown、重开；原生事实不变、总量不变、投影守恒。
5. 补正进展：游标/映射到达顺序/迟到事实/多轮重启；未知可再次被检查，有限静止输入与映射下最终补齐可解析项。
6. 查询与写并发：采集/补正写交错、取消/超时/队列满；同 revision 全结果、控制写可进展、无锁/槽泄漏。

确定性负例至少覆盖 None 与字面“未知”模型不合并、输出数量超界、最后一个单元溢出、已有归属不许重写、撤旧单元变零时删除、零 token 消费不可误删、查询不发网络/不改状态，以及旧读者与目录 guard 生命周期。

## 8 性能和资源

预算按 §3 及 B3 §3.2；扫描计数在 Rust 过滤之前，超限整请求失败。token=Any 使用时间索引，Exact/Unknown 使用现有 token/time 索引；按所选索引范围实际读出的行计数，不能只数匹配行。精确达到上限允许额外探测一个 next 存在性，不返回额外事实。局部页/输出缓冲有界，不能先全量取出再判断。source/group/model 的高选择性或零匹配过滤仍受原始扫描预算；索引测试防止每页从头扫描，禁止无界 SQL 聚合/排序。

独立串行 release：10 万消费、20 模型/100 token、31 天和秒边缘，整体日桶查询、单 token 小时查询各≤5秒；256 行/10000 映射补正≤1秒。20 万/100 万历史另记耗时、扫描数、峰值内存和 WAL 趋势，不当历史上限。普通测试默认并发，性能入口纳入既有 runner/CI；并发控制写用 barrier 核读事务存活期间真实提交，不以查询退出后可写充数。

## 9 安全和副作用

仅在开发 worktree 使用合成临时应用库；不读生产配置/数据库/日志/key/环境，不请求供应商，不重启/部署/合并/推送。GLM/火山策略、原生转发、认证、CLI、现行面板及 boot 均不改。本批不引入新数据库服务、账本、轮询器或推理代理；标签作为数据，不拼 SQL、不进诊断或通用 Debug/serde。

## 10 对照和退出

用户已确认。实施后执行范围 diff、步骤/类型/负 trait/测试入口清单（空清单或全 ignored 必拒绝）、默认 debug/release 全量、clippy/build、独立性能与至少 10 种可编译定向变异，变异方向见 B3 §7。现有目录锁、B1 入账、B2 采集和业务测试全部保留，不通过改变测试并发或清空复杂夹具掩盖失败。

再按 workflow §6 独立抽契约审 diff，记录审核/decisions 并做五维度复核。全部门槛通过才能宣布 B3 本地验收；三页面和部署仍是后续子计划。

第一小步历史范围：当时先固定为 ledger/analytics 计算与 QueryControl 的无 SQL 部分，不改 catalog/worker。PB1/PB2 及增量拆分、取消/错误轨迹各 128 cases；安全 trait/输入/整数/未知项/最大输出负例；最大输出累计独立串行 release≤5秒。累计错误必须锁存并丢弃局部工作集，不能忽略 Err 后 finish 得到部分成功。至少捕获时间上界、边缘双算、负偏移截零、漏 token 过滤、未知模型合并和放宽容量六个变异；当时 SQL/目录锁/恢复变异仍待后续小步，不能提前勾选。

第一小步历史执行结果：14 普通入口和 4 组 PBT 通过；10 个变异被捕获、反馈复审关闭；当时 debug/release 各 459 passed/0 failed/12 ignored；11 项串行性能通过，新最大输出 118 ms。第一步证据未涵盖 PB3–PB6 持久/并发部分；这些部分随后已在 B3.3–B3.7 验收中执行，见文首最终审核。
