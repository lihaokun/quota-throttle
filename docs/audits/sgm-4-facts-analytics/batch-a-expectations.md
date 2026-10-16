# SGM-4 第一批事前契约清单

日期：2026-10-16。事前清单基线 `dc952b3`；2026-10-17 用户确认后基于 `f0b8295` 实施（含此前测试门禁和目录锁修复）。以下要求保留作为事前期待，实际执行结果见[审核记录](audit-report.md)，不作为整个 SGM-4 的验收结论。

## 1 契约源

- [SGM-4 A 细化](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics.md)：私有类型、A1–A5、边界与后续依赖。
- [架构](../../design/source-groups-token-management/architecture.md)：联合事实投影、共享来源、未知不为零、历史身份、原生转发不改。
- [隔离能力核验](../../research/source-groups-facts-capability-research.md)：type=2/5、重试不是独立请求、消费不是最终成功、控制台文本不可信。
- [SGM-3 B](../../design/source-groups-token-management/subplans/sgm-3-source-policies-b.md)：FinalOutcome/VolcanoEvidence 的真实生产者义务，A 不满足也不绕过。
- [workflow](../../workflow.md) §4.4、§5、§6。

仓库当前没有 workflow 提到的 `templates/contract-audit/expectations-template.md`；沿用既有 SGM-3 清单的十节结构，不为此安装或更新工作流。

## 2 Schema 字段

本批只有 crate 内私有类型；无 SQL 迁移、公开 JSON 或 HTTP。实现前不得自行新增表或变更 CatalogSnapshot/OperationView wire。

| 类型 | 期待字段/约束 |
| --- | --- |
| NativeFact | 原生 log/request 身份、Consume/Error、秒级时间、channel/token、原生 group/model、input/output、is_stream、可解析状态；缺计量字段拒绝 |
| RelayMetadata | 仅状态码/错误代码与类型/尝试渠道序列/流式分类；无原始 other、正文、路径、key、selector、鉴权 |
| ChannelAttribution | 稳定 channel/source/group ID 和历史原生组；映射唯一性在过滤前验证 |
| AttributedFact | 保留完整事实，稳定或空 source/group，归属问题分类；未知不丢消费 |
| UsageCellKey | UTC 分钟×channel×source×group ID×原生 group×token×model，空维度独立于真实字符串 |
| UsageTotals/Delta | input/output/消费行数；各项和 token 合计不超 JSON_SAFE；总体与联合分项守恒 |

ID 使用已有类型/整数精确值；不以 f64 或展示名归属。归属缺失不得使用 0 构造合法 SourceId/GroupId。带元数据类型不提供生产 Debug/Display/Serialize，测试夹具只能使用假材料。

## 3 枚举值

- FactKind：Consume、Error；仅映射原生 2、5，其他类型不计量。
- RelayMetadataState：Absent、Parsed、Unusable，不含 Ready/Verified。
- StreamObservation：Missing、Done、Eof、ClientGone、Other，不复用成功/不可用状态机枚举。
- AttributionIssue：None、UnmappedChannel、GroupUnresolved；组合约束按细化 A4。
- FactError：InvalidShape、InvalidNumber、LimitExceeded、ConflictingIdentity、InvalidAttribution、ArithmeticOverflow；错误无自由文本载荷。
- SourceKind、来源 ID 和组 ID 不另抄一份；不生成 FinalOutcome 或策略许可。

## 4 流程步骤

| 标记 | 检查 |
| --- | --- |
| A1 | 类型筛选→必需字段严格校验→白名单元数据解析/降级→规范事实 |
| A2 | 批容量→逐行解析→同 ID 同值去重/异值拒绝→确定性排序 |
| A3 | 完整映射校验→事实校验→历史 channel 归属→组不匹配保留未归属 |
| A4 | 完整事实/归属/唯一性校验→仅消费→UTC 分钟联合分组→整体/分项精确加算 |
| A5 | 输入不变量→checked_add→合计/范围校验→返回新值 |

实现采用 `// Step An:` 标记。辅助校验 helper 的契约与调用顺序也须可追踪，不能只有标记而缺分支。

## 5 行为契约

- channel/token字段必需，0作为未知；模型/原生组/request_id允许缺失为未知，不能丢弃对应消费；计量必需字段缺失不默认0。
- 消费只加 prompt/completion；不叠加 cache_tokens、quota、倍率或积分；Error 必须零 token 且不增加消费行数。
- 同源跨组的不同渠道消费各计一次；分组是实际使用维度，不通过来源副本数复制消费。
- 同原生日志 ID 同规范值在 A2 去重，不同值整批冲突；A3/A4 额外拒绝重复 ID。
- source 可从不可复用历史 channel 确定；原生组不一致时不能按当前成员组强贴 group ID。
- 模型/组原文不 trim 或重写；未知模型仍有独立分项，不能叫作某个默认模型。
- 模型/组为非字符串、超256字节或含控制字符时，该标签为None，元数据Unusable，保留有效token及其他合法标签；计量核心字段损坏仍拒绝。可选元数据上限是本地保护值，不冒充上游合法最大值。
- 流式取消/EOF 的已记录 token 照计；解析成功不等于最终成功或可用，不修改任何错误计数。
- 空增量只表示输入没有消费，不宣布完整零用量、剩余额度或路由可用。

## 6 时序与状态契约

- A 没有持久状态或后台任务；所有 Err 都没有 IO 或可见半批结果。
- log ID 排序只是规范输出，不作为请求结束排序或 SGM-3 的投影游标。
- 同一批必须属于同一个原生数据集；实例验证、跨批去重、替换/恢复不由调用者布尔值替代。
- B 必须先全局去重，再计算本批新增事实增量，并与事实/检查点同事务提交；重放不能再次累计。
- 历史归属读取和未知后续重归属属于 B；A 不读当前状态推测旧历史。
- 分钟投影用 UTC；秒级原始时间保留。自然日展示和火山重置是不同责任，不依赖本机时区。

## 7 属性测试不变量

各项至少 128 cases；参考模型使用独立集合/直接求和和明确字段，不调用被测 helper 生成期望。

| 编号 | 随机空间 | 断言 |
| --- | --- | --- |
| P1 解析边界 | 合法数值/类型、缺失/null/字符串/浮点/布尔、可选元数据形状 | 核心字段严格，未知元数据不改变消费；错误不泄漏随机秘密哨兵 |
| P2 去重与排列 | 规范事实排列、同值重复、单字段冲突、无关字段差异 | 排列/同值重复不变；语义冲突整批失败；无关字段不进入规范事实 |
| P3 历史归属 | 源/组 ID 与 channel ID 不相同、多组共享源、未知/退休、重复映射、组缺失/不匹配 | 无跨源猜测，无丢失；组未知不阻断已知来源统计；先验唯一性不能被无流量映射躲过 |
| P4 联合分项 | 多时间/模型/来源/组/token、消费与错误、未知维度 | 每条消费只属于一个联合单元，整体=模型和/来源和/组和/token 和，未知项参与守恒 |
| P5 分批可加 | 将全局唯一事实划分成不重叠块，块顺序/尺寸变化 | 按联合 key 合并各块增量等于一次计算；明确不声称重叠块可以直接相加 |
| P6 数值与时间 | 分钟边界前后、0/MAX 安全值、累计溢出、0-token 消费 | UTC 分桶稳定、零 token 仍计消费行、溢出拒绝不饱和、不返回半结果 |

确定性用例至少包括：1024/1025 行、10000/10001 映射、16 KiB other 边界、128/129 尝试、标签/请求 ID 上界；固定版本39位正常请求ID（十六进制/随机前缀均可），非法标签只降级该维度且计量相等；原生 2/5/其他类型；缺失 tokens；非零 Error tokens；同请求不同 log ID 不错误合并；same-source fail→success 不由 A 判终态；取消/EOF 与正常消费相同计量；缓存不加两次；组名称 `auto` 的历史记录不被新建组规则丢弃。

定向变异至少包含：缺字段默认零、将错误当消费、丢未知模型、复制共享来源计数、按 request_id 去重消费、先过滤后验映射、相同 ID 异值覆盖、额外加缓存、溢出饱和、取消消费被丢。变异用源码/target 双隔离，每项提供断言捕获证据。

## 8 性能契约

- 输入最多 1024 行和 10000 历史映射，超限先拒绝；嵌套元数据上限按 §2/细化执行。
- A2/A3/A4 使用有界 map，复杂度分别 O(n log n)、O((m+n) log(m+n))、O(n log n)，不做 n×m 扫描或全量历史常驻缓存。
- 固定最大合法批次及10000映射的容量/正确性检查属于普通测试；5秒预算在独立串行release性能测试中断言，并加入强制入口/CI清单。不得在计时内等待网络、构建或随机外部进程。
- 容量是处理块约束；B 必须持久化分页进度并处理后续块，不得截断图表历史。

## 9 安全与副作用

允许修改新 `src/ledger/`、`src/main.rs` 的单个模块声明、本批测试/文档/机械脚本。禁止修改旧 DDL、catalog/worker/GLM 逻辑、HTTP 管理认证、面板、生产配置/数据库/日志或 Cargo 依赖。

测试使用内存假 JSON/映射，不连接 NewAPI 或供应商，不调用运行端口。普通错误输出固定枚举，无原始载荷；带秘密哨兵的未知字段必须被丢弃。A 不读取凭据，不建立实例登记，不新增服务或推理代理。

## 10 跨实现对照和退出记录

隔离探针的消费/错误/流式字段作为兼容 fixture，但不把探针客户端观察到的 HTTP 终态塞进生产输入。联合投影与独立逐条参考计算对照；模型/来源/组/token 分项均须包含未知项后与总体相等。

路径 A 计划：新增机械脚本检查 A1–A5、共享类型/常量、零修改文件、计时断言、测试/构建；无公开 JSON，Schema 检查为 N/A（本批无公开载荷，B/C/面板另验）。路径 B 计划：按 workflow §6 独立 subagent 从契约抽期待，对照基线 diff，主 agent 关闭 ❌ 和处置 ⚠️，保留 audit/decisions。

退出还需全量 Rust 回归、clippy、release 和五维度复核。2026-10-17 更新：A 已实现，18 项普通测试、6×128 PBT、10 项变异、机械及独立静态审核通过；全量 debug 的既有认证测试竞态仍阻断完整退出，动态结果见[审核记录](audit-report.md)。本批无公开 JSON，Schema 检查为 N/A；后续公开接口仍须验 Schema。B/C、整体 SGM-4 和生产发布保持未完成。
