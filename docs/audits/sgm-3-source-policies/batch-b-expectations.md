# SGM-3 第二批事前契约清单

状态：B 批事前清单，待用户确认细化后编码；未执行本批测试。基线 `2ec8b50`。本表从 B 契约抽取，不列实现位置或填预期通过。A 的已验收范围和测试保持不变。

## 1 契约源

- [B 批细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies-b.md)：状态、证据、周期、存储/迁移、接线门禁。
- [SGM-3 主细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies.md)：A/B/C 边界与 GLM 不变。
- [架构](../../design/source-groups-token-management/architecture.md)：G1/G2/G3/G6、共享来源、独立组选择、三态与两个原因。
- [需求](../../research/source-groups-token-management-research.md)：火山无临期、软 token 上限、每日 01:00 UTC+8、人工撤错。
- [目录迁移承接](../sgm-1-catalog-store/migration-scope-review.md)：版本/公开格式分离、逐步迁移/备份、缺失不能当初始化。
- [workflow](../../workflow.md) §4.4、§5、§6。

## 2 Schema 字段

新增为应用私有 SQL v4，不新增公开 JSON；CatalogSnapshot/OperationView wire=1。旧三份 DDL 不改。

| 表/类型 | 必需字段和约束 |
| --- | --- |
| volcano_runtime | source_id、kind=volcano、revision、period_id/start/next、used_input/output、consecutive_failures、error_exhausted/limit_exhausted、error_epoch/cutoff、evidence_cursor/coverage_end；源类型复合 FK，PK 唯一 |
| 未初始化来源行 | revision=0、无周期、全部计数/原因/epoch=0、cutoff/cursor/coverage 空；不提供 Available 业务结论 |
| 已初始化来源行 | revision/period_id/error_epoch≥1；端点齐全有序；计数、合计、游标与时间满足 JSON_SAFE 范围；原因与当前策略一致 |
| group_policy_state | group_id/kind 复合 FK、revision、active/pinned、regime、cursor_order/member；kind 专属字段约束；成员归属按组验证 |
| VolcanoEvidence | 源/周期/error_epoch、from/to cursor、绝对输入/输出、扫描上界、最多 1024 个归并 Outcome；不可从前端 JSON 构造 |
| FinalOutcome | first_event_at≤completed_at；Success/Unavailable/Ignored；生产者保证请求/来源唯一终态 |
| PeriodTransition | 旧起点/原 next、当前新起点/next、旧规则；不逐日补造空周期 |

SQL CHECK/FK、严格解码和数据核验一起验证；不得只核表名/行数。私有状态输出不包含 credential、selector、prompt、原生错误正文。

## 3 枚举值

- SourceKind、MembershipLifecycle、Regime 复用已有定义，不创建另一份提供者枚举。
- VolcanoState 只有 Paused/Available/Exhausted，派生而不落库。
- OutcomeKind 只有 Success/Unavailable/Ignored；未知终态不是 Unavailable，根本不应进入结果列表。
- GroupPolicyState 复用 GLM 状态或携带火山 preferred/cursor，不给火山添加 pin/临期字段。
- 消费结果区分 Applied/Duplicate/Stale/Gap；Gap 在存储入口映射既有 NotReady。
- 内部更新只有 Tick/Evidence/ClearError/ReplacePolicy；返回 SourcePolicyWrite 不含凭据或远端确认。
- 错误复用 PolicyError 和 ControlErrorCode，新增公开枚举/schema 不在本批范围。

## 4 流程步骤

实现沿用 `// Step Bn:` 标记，本批机械脚本检查定位，再由独审核对语义。

| 步骤 | 契约 |
| --- | --- |
| B1 | 规则/输入/可表示性校验，再计算固定 UTC 偏移边界 |
| B2 | 仅未初始化行显式初始化；旧状态不能重新清零 |
| B3 | 已到旧边界先推进；常数时间跳过多日，保留旧边界过渡 |
| B4 | 证据周期/epoch/游标门禁；整批副本应用绝对用量和有序终态 |
| B5 | 撤错只清错误原因/连续次数，提升 epoch，持久保留 token 和暂停 |
| B6 | 改规则先处理旧到期；保留起点/用量，重评上限和降阈值 |
| B7 | 先验证身份唯一性，再做资格过滤与粘滞/稳定轮转 |
| B8 | 旧库只读验证/原始版本备份/v4 事务回填，最后发布当前版本 |
| B9 | 状态写事务中校验 source/runtime 或 group/policy revision，变更才提升版本 |
| B10 | 重开核对全部状态行和外键归属，缺失/损坏拒绝，不修补 |

## 5 行为契约

- 同源跨多组只有一份用量、连续失败和原因；组首选互不覆盖。
- 暂停优先于两个耗尽原因；错误/上限可同时成立。成功清连续计数但不自动解除已锁存原因。
- 无上限不触发规则 2；缓存不二次累加；提高上限不能清规则 1，撤错不能清规则 2。
- 降低失败阈值可立即锁存；升高阈值不能代替人工撤错。GLM 阈值/算法完全不改。
- Ignored 不增加也不清连续次数；其他来源成功不能清本来源次数。
- clear 的同秒边界保守排除旧事件；旧周期、旧 error_epoch、重复/重叠投影不得重复累计。
- 首选可用则保持；不可用按稳定位置轮转；全员不可用仍保留游标；恢复来源不抢占。
- 尚无事实覆盖不能显示成查到零余额；本批策略结果不等于渠道已切换或真实上游健康。

## 6 时序和状态契约

- 日边界为 [start,next)，next 严格未来；默认 01:00 UTC+8；不依赖主机时区。
- 同刻 Tick→Clear/改配置，先推进旧到期边界；改时刻不重写已关闭历史、不立即凭空恢复。
- 时钟回拨不倒退 period_id、不重复重置；撤错/编辑若不能满足当前周期或 cutoff 边界则失败，不挪动旧状态凑合法。
- period/error_epoch 变化使旧批次失效；同周期计数绝对值不下降；接受游标和状态同事务。
- 非法批次中途失败全部回滚；版本冲突无写入；提交不确定沿用毒化，不发布成功。
- 重新打开保存原原因/游标；第一次策略 Tick 才补算错过周期，不能在 open 隐式用时钟清额度。
- v3→v4 只追加必要状态结构和默认行；多级迁移逐次事务，不声称一笔回滚全链。
- C/SGM-4 必须把事实检查点、历史过渡、运行状态和渠道意图组合提交；B 的单独存储测试不算该条件通过。

## 7 属性测试不变量

每项至少 128 cases。参考模型独立保存标志、周期和轮转位置，不调用被测算法计算期望。

| 编号 | 随机空间 | 必须断言 |
| --- | --- | --- |
| B1 双原因 | 暂停、错误/成功/忽略序列、上限/阈值编辑、撤错 | 派生三态符合暂停优先与 OR；原因清除边界独立；同源多组不重复计数 |
| B2 日边界 | 全部合法偏移/分钟、边界前后、跨日/多日、回拨和规则修改 | start≤now<next、next 对齐新规则；重置幂等、改规则不丢旧用量/起点；与有界逐日参考对拍 |
| B3 投影消费 | 批次分割/重放/游标缺口、epoch/周期改变、迟到结果、非法项 | 重复不累加；旧代次不覆盖；缺口需重建；绝对 token 与归并结果顺序一致；错误无部分提交 |
| B4 稳定轮转 | 多组不同成员 ID、共享源、暂停/耗尽/恢复、成员移除、无候选 | 当前可用不切；只在本组选择；从旧游标继续；恢复不抢；来源/成员 ID 不混用 |
| B5 持久保真 | 有区分性的 GLM/火山来源、组状态、v3 目录/操作/执行历史、重开 | 私有材料/旧收据/映射逐字段保留；新状态重开一致；缺行/错组不能被默认修复 |

确定性回归另覆盖：两个原因并存撤一个、失败/成功/失败、正好三次、cap 恰好到达、cutoff 同秒、未初始化/缺行、退休历史、来源/成员 ID 不相等、批次 0/1024/1025 项、整数溢出、DDL/状态写故障回滚。

定向变异至少包含：重置解除暂停、clear 清 token、旧 epoch 仍接受、重复批次再累加、改时刻清零、恢复抢占当前首选、缺运行行自动补零、v4 迁移篡改私有材料。每项须有断言失败证据，源码/target 双隔离。

## 8 性能契约

- 来源/组/成员沿用现有容量；一份证据最多 1024 项，超界在持久写前拒绝。
- 跳过多年使用常数次算术；组选择最多一次稳定排序和线性扫描，不逐源 HTTP、不逐日插空记录。
- 新表按 SourceId/GroupId 主键访问，读/核验不保存原始日志或无界历史；历史属于后续统一账本。
- 最大管理规模的纯选择测试加宽松 5 秒预算；迁移/重开单独记录耗时，沿用既有忙等/备份上界。
- 业务参数相同且无状态变化不重复提升 revision；不为本批添加后台任务或轮询周期。

## 9 安全与副作用

测试使用独立私有 TempDir 和假材料，不读生产 config/数据库，不请求供应商或运行服务，不启动 NewAPI。

允许：source_policy 的 B 代码/测试；catalog v4、严格核验、源/组默认行与策略事务及测试；本批文档/机械脚本。禁止：旧 DDL、GLM 决策、管理认证/HTTP、worker 发布逻辑、CLI/面板、Cargo 依赖的无关修改。

新增初始化状态必须明确未就绪，不能漏行时重新默认可用。当前 public wire 不扩字段；所有保存成功只表示本地持久化，不伪造 operation 远端收据。

## 10 跨实现一致性与退出记录

固定时区公式与独立的有界日期枚举对照；三态与简化参考状态机对照；轮转与独立环模型对照；持久化与纯状态值逐字段对照。GLM A 的冻结 golden、既有目录/协调/认证回归均继续通过。

退出需补全实际测试、PBT/变异、机械脚本、全量回归、release、独立 subagent 契约审及五维度复核。当前均未执行，不预填通过。真实日志终态、历史账本组合和最终路由是 C/SGM-4 的单独门禁；它们未完成时不得部署自动火山调度。
