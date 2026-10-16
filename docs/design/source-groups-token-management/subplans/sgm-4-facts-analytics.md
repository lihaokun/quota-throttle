# SGM-4 统一事实账本与用量统计细化

日期：2026-10-16。状态：用户已确认先修测试门禁并澄清本批边界，再进入 A 实施；本批尚无 Rust 实现，B/C 尚未完成细化。代码基线 `dc952b3`。本文件不授权部署，不表示事实能力已具备火山路由许可条件。

依据：[已确认架构](../architecture.md)、[事实能力核验](../../../research/source-groups-facts-capability-research.md)、[SGM-3 B 生产者义务](sgm-3-source-policies-b.md#32-可信事实批次)。完整目标仍包括事实持久化、统一采集、各维度图表、最终结果归并及火山策略组合，不以第一批纯函数完成代替整体交付。

## 1 分批范围

| 批次 | 交付 | 仍需满足的门槛 |
| --- | --- | --- |
| A 待实施 | 日志白名单解析、批内身份去重、历史归属适配、统一分钟用量增量 | 纯函数/边界/PBT/变异、机械检查、独立契约审；不进行 IO |
| B 后续细化 | 应用库事实/投影/检查点事务、管理 API 分页和补采、图表查询 | 实例替换与历史归属、跨批去重、分页移动/迟到/缺口、容量/流控、迁移；不直读 NewAPI 数据库 |
| C 后续细化并与 SGM-3 C-3 合流 | 可信终态、来源周期历史、错误代次隔离及路由发布 | 纯失败请求可靠结束证据、同源成功优先、流式取消、状态/目标/许可同事务及发布；未知不签发正向许可 |

SGM-3 C-2 管理动作、SGM-5 key 管理、SGM-6 三页面、SGM-7 迁移发布继续保留在[实施计划](../implementation-plan.md)，不由本子计划替代。

A 只增加 `src/ledger/` 及模块声明，不修改既有 `recent_logs`、面板图表、GLM 算法、worker、catalog DDL 或运行入口。新代码是 B 的元数据解码及每笔新增事实的投影运算，不建第二个采集器或账本。B 必须在全局去重后，把本批产生的增量与事实/检查点同事务提交。

## 2 现有能力和复用

- `catalog::{SourceId,GroupId,ChannelId}`：复用身份类型；不另定义来源种类或组协议枚举。
- `catalog::JSON_SAFE`：复用可公开表示的计数上限；身份类型仍按既有十进制字符串语义处理，不能把 ID 当浮点数。
- `serde_json`、标准库 `BTreeMap`：解析和稳定排序；现有 `proptest` 用于测试，不增加依赖。
- 后续 B 复用 `ManagementTransport` 的响应大小限制、会话恢复和统一预算，历史请求使用 `Backfill`，不逐来源或逐 key 轮询。
- 旧 `status::aggregate_usage` 已丢失来源/key 联合维度，`recent_logs` 是最近记录视图且有缺字段默认值，均不能作为新账本的权威输入；本批不改它们以免影响现行服务。

## 3 A 批私有数据结构

这些类型只供 crate 内的 ledger 使用，不增加公开 JSON、SQL 或 HTTP 契约。带原生元数据的类型不提供生产环境 `Debug`、`Display` 或 `Serialize`；错误只返回下面的固定错误码。

| 类型 | 字段和约束 |
| --- | --- |
| NativeFact | `log_id` 为正 i64；`kind` 为 Consume/Error；`occurred_at` 为 UTC epoch 秒，0..=JSON_SAFE；`channel_id: Option<ChannelId>`、`token_id: Option<i64>`，原生 0 表示未知，负数非法；`model/native_group: Option<String>`；`input/output` 为非负安全整数且合计≤JSON_SAFE；`is_stream: bool`；可空 `request_id`；RelayMetadataState |
| RelayMetadataState | Absent、Parsed(RelayMetadata)、Unusable。只描述元数据可解析程度，任何值都不意味着请求已结束 |
| RelayMetadata | 可空 HTTP 状态码、错误类型/代码、已尝试渠道序列、流式分类。只提取需要的字段，不保留原始 `other` 或错误正文 |
| StreamObservation | Missing、Done、Eof、ClientGone、Other。只归一化实测的 `ok/done`、`ok/eof`、`error/client_gone` 组合，其余是 Other；不是 Success/Unavailable |
| ChannelAttribution | `channel_id/source_id/group_id` 和该渠道历史归属的 `native_group`；同一 channel 必须唯一。由 B 从不可复用的历史成员映射构造，不能拿当前展示名代替历史身份 |
| AttributedFact | NativeFact 加可空 `source_id/group_id`、AttributionIssue；未知来源/组仍保留事实。无复制来源或全局计数 |
| UsageCellKey | UTC 分钟起点、channel_id、source_id、group_id、原生 group、token_id、model；未知维度为 None，不与空字符串展示标签混用 |
| UsageTotals | input、output、consumption_rows，非负且各自≤JSON_SAFE，input+output≤JSON_SAFE；行数不是最终请求数 |
| UsageDelta | 按 UsageCellKey 排序的非空消费单元及总体 UsageTotals；仅代表传入新增事实的观测增量，不带 coverage、ready、余额或请求成功率 |

数值严格读取 JSON 整数；不接收浮点、数字字符串或布尔替代。NativeFact 的模型/原生组字段允许 null、缺失、空字符串表示未知；合法非空值保留原文，不 trim、不改大小写、不改模型名。每个最多 256 UTF-8 字节，不含控制字符；非字符串、超界或含控制字符时，仅该标签降级为 None，并把 RelayMetadataState 置为 Unusable，仍保留有效身份、时刻和 token。它们不是拒绝整条消费的计量必需字段，也不回填默认模型/组。不套用受管组的 `valid_group`，因为历史原生组未必符合新建组命名规则。

`request_id` 缺失/null/空串为 None；非空最多 64 字节且只接受 ASCII 字母数字、`_`、`-`，不推算请求开始或结束。不是此格式时保留消费，置 None 并使元数据 Unusable；禁止用原生 log ID 补成请求 ID。

可选元数据约束：原始 `other` 最多 16 KiB；错误类型/代码最多各 128 字节且无控制字符；HTTP 状态码若有必须是 100..599 整数；`admin_info.use_channel` 若有必须是最多 128 个正 i64 的十进制字符串，保留顺序及重复，不把一次请求重试拆成多个请求。任一已知字段格式非法或超过上界，整份可选元数据置 Unusable，原始文本不保留。未知字段忽略，不能凭名字包含 `token` 就累加。

`other` 缺失/null/空串为 Absent；有效 JSON object 为 Parsed，已知可选字段缺失仍为 None/Missing；其他形状或 JSON 错误为 Unusable。该兼容处理只适用于非计量元数据：消费的身份、时刻、token 等必需字段损坏仍拒绝，不填默认零。

这些 16 KiB、128 次尝试、256 字节标签和 128 字节错误代码限制是本地内存/解析保护预算，不是探针测得的 NewAPI 最大合法值。超界会损失相应维度或终态元数据，必须作为未知保留消费，不能宣称完整错误遥测。request_id 的 64 字节兼容上限同样不是固定生成长度：已核验版本生成 23 位 UTC 时间（14 位年月日时分秒+9 位纳秒）、8 位前缀和8 位随机字符，共39位；前缀是构建路径哈希的8位十六进制，取不到构建信息时为8位字母数字，随机后缀也是字母数字，均落在允许集合内。[生成逻辑](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/common/utils.go#L268)、[随机字符集](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/common/str.go#L41)

## 4 错误和容量

`FactError` 为私有固定枚举：InvalidShape、InvalidNumber、LimitExceeded、ConflictingIdentity、InvalidAttribution、ArithmeticOverflow。不得携带原生行、模型名、路径、响应正文或底层 JSON 错误链。

单批最多 1024 行/事实，每份映射最多 10000 条；先检查长度再分配处理容器。映射和事实的容量互相独立，不能因为只有几条消费就跳过映射唯一性校验。容量是处理块大小，不是整个历史区间上限；B 可多批事务处理，持久检查点不能因块已满就跳过剩余记录。

## 5 A 批函数细化

### A1 decode_native_row

签名：`decode_native_row(row: &serde_json::Value) -> Result<Option<NativeFact>, FactError>`。

前置：输入是内存 JSON，不进行 IO。先要求 object 且 `type` 是非负整数；type 不为 2/5 返回 None，只表示非本账本类型，不表示“查询完整”。type=2/5 则检查 log ID、时间、channel/token ID、两个 token 计数和 is_stream 必需字段；缺失或类型不符报固定错误。Error 行两个 token 必须为 0，异常的非零值拒绝，不加入消费统计。

随后解析模型/原生组和可选请求/relay 元数据；只按 §3 白名单输出。任何计量字段错误返回 Err，无部分事实。模型/组标签非法时，该字段为 None；request_id 非法时为 None；任一发生都使元数据为 Unusable，合法的其他标签仍保留。非计量元数据错误返回相同计量事实及 Unusable，不掩盖消费，也不把不确定状态默认为成功。空消费 0+0 合法，仍是一条消费记录；缓存、quota、倍率和积分完全不参与 token 加算。

正确性论证：必需字段验证在构造结果之前完成，保证 NativeFact 数值不变量；元数据降级不修改计量字段，保证未知终态不变成零消耗。输入字段有限，可选列表有界，无 IO 和共享写入。

### A2 decode_native_batch

签名：`decode_native_batch(rows: &[serde_json::Value]) -> Result<Vec<NativeFact>, FactError>`。

先检查容量，再逐行调用 A1；None 不进入账本，Err 使整批失败。将有效行按 log_id 放入 BTreeMap：同 ID 的完整规范字段相同则保留一次，不同则 ConflictingIdentity。返回按 log_id 排序的向量；这只是确定性处理顺序，不是请求完成顺序或策略结果游标。忽略的原生字段不同不构成冲突，因为它们不参与账本语义。

前置责任：一批只来自同一个已绑定的 NewAPI 数据集。A 不验证实例身份，B 须按架构实现 instance/log ID 的跨批去重及替换门禁，不能以 `catalog_instance_id` 自动证明原生数据库从未被替换。

正确性论证：处理前 k 行时 map 恰好包含其规范事实集合，重复同值不改变集合，冲突直接失败；遍历有界输入后得到完整批内去重集合。不持久化，不宣称跨批有效一次。

### A3 attribute_batch

签名：`attribute_batch(facts: &[NativeFact], history: &[ChannelAttribution]) -> Result<Vec<AttributedFact>, FactError>`。

先验证两份容量、全部映射 ID/字符串及 channel 唯一性，重复映射即使内容相同也拒绝。再检查事实按完整规范字段有效且 log_id 唯一；外部手造的非法事实不能绕过 A1。每条事实按 channel 查询历史映射：

- 没有 channel 或查不到：source/group 都为 None，issue=UnmappedChannel，仍保留 token、原生组、模型和访问 token ID。
- 查到且日志原生组与历史原生组一致：使用稳定 source/group ID，issue=None。
- 查到但原生组缺失或不一致：source ID 仍由不可复用的 channel 确定，group ID 为 None，issue=GroupUnresolved；保留日志原生组，不改贴当前组。

返回顺序与事实输入一致。history 的真实性属于 B 的同事务读取责任；本函数不根据 source 名、secret、当前启用状态或模型名猜归属。退休成员仍可进入 history。未知后来解决时，B 必须显式重投影并事务搬移分项，本批不暗中把旧事实再次加算。

正确性论证：映射唯一性先于查找保证单 channel 单归属；只有已验证的组对应关系才分配 group ID。每个输入恰好产生一个输出，故未知和退休身份不会让消费丢失或跨来源串账。循环次数由两份有界输入决定。

### A4 usage_delta

签名：`usage_delta(facts: &[AttributedFact]) -> Result<UsageDelta, FactError>`。

先校验容量、所有事实/归属形状与 log ID 唯一性；重复 ID 一律拒绝，防止绕过 A2，不在投影层再悄悄去重。source=None 时 group 必须为 None；issue=None 时 source/group 均有值且原生组非空；GroupUnresolved 要求 source 有值、group 为空；UnmappedChannel 要求二者为空。

仅 Consume 行产生增量。用 `occurred_at - occurred_at % 60` 取得 UTC 分钟起点，并用 §3 的完整联合维度作为 key。调用 A5 加到对应单元与总体；Error 行不增加 token 或 consumption_rows。即使来源暂停/退休、消费是取消或 EOF，都按已记录 token 计量，不检查来源可用性，不增加/清除失败计数。

结果按 key 的派生 Ord 稳定排序（时间、渠道、来源、组、原生组、token、模型；Option 使用 Rust 的 None 在前）。每条消费属于唯一单元；分项求和等于总体；总体是同一批事实的另一视图，不允许再次与分项相加。没有消费时单元为空、总体为 0，仅表示这批没有消费，不代表采集完整或来源还有额度。

前置责任：B 只把持久去重后真正新增的事实交给本函数；跨批重放由同一事实账本防重，不要求 A 在内存保留所有历史 ID。

正确性论证：初始总体和单元均为零；每次仅将一条消费的 input/output/1 加到一个单元和总体，保持分区求和等式。失败时丢弃本地临时结果，调用者得不到半批增量。UTC 分钟算术对非负安全整数不溢出；不按主机时区变更归属。

### A5 add_usage

签名：`add_usage(left: UsageTotals, right: UsageTotals) -> Result<UsageTotals, FactError>`。

先核两输入不变量，再逐字段 checked_add，最后核范围及合计。全部通过才返回新值，不原地更新传入值；失败为 ArithmeticOverflow。用于 A4，也供 B 在事务内更新累计单元时复用，禁止依赖 SQLite SUM 的隐式数值转换或饱和截断。

正确性论证：每个字段只在可表示且符合安全范围时生成；不变量验证后输入与输出均是精确整数。没有循环、IO 或可见部分更新。

## 6 时间与展示责任

分钟投影是稀疏联合投影，不为无记录的分钟填“完整零用量”。B 的图表查询再按用户指定固定时区的小时/自然日组合，保留模型维度；源重置周期继续使用来源策略的独立边界，不能把图表午夜当作重置。

选择分钟作为投影粒度，是因为图表时区和火山 reset_minute 均以分钟表示；原始秒级 occurred_at 仍保留在事实表，不因投影丢失。B 须定义非分钟对齐查询边缘从原始事实补算的规则，不把整个分钟强塞入范围。A 不增加“只支持最近 1024 条”的产品限制。

## 7 并发和副作用

A 全部为纯函数，输入借用/局部所有权，不读时钟、不启动 task、不获取 catalog 锁、不访问网络或数据库。不同批次可独立计算，但 B 的事实/投影/检查点写入仍归既有单写者，不因纯函数可并发而新增写者。

不创建 FinalOutcome、VolcanoEvidence、BackendReadiness 或 RoutingPermit；不暴露 `verified=true` 等假生产者入口。解析到 `Done` 也不能绕过 C 的终态归并。B/C 的 IO、状态事务及分布式约束必须在各自细化中补齐，本批无跨进程接口。

## 8 测试和退出

事前十节清单见 [A expectations](../../../audits/sgm-4-facts-analytics/batch-a-expectations.md)。至少覆盖：类型/边界、批内重复/冲突、未知历史、跨组共享来源、未知模型、取消/EOF 计量、联合维度分项守恒、容量与整数溢出。PBT 每项至少 128 cases，参考值不得调用被测解析/投影函数生成。

机械检查限定新模块及模块声明、验证共享 ID/常量复用和 A1–A5 步骤标记；旧 GLM/worker/DDL/面板及运行文件零修改。新增模块无公开 JSON，故本批 JSON Schema 检查不适用；这不是免除 B/C 或最终面板公开 schema。容量/正确性断言留在普通测试，5秒计时预算使用独立串行性能入口，并同步入口测试清单和CI，不给默认并行PBT夹墙钟门槛。独审按 workflow §6 独立抽取契约与 diff 对照；全量回归、clippy、release 和定向变异完成后才可宣布 A 验收。

## 9 实施前审查与未解决依赖

A 的分支、退出、容量、纯函数调用契约与正确性论证已列出；无 SQL 迁移或后台接线。用户已确认先修测试门禁、补齐以上两处澄清再进入 A 编码，当前没有 A 对应实现或测试通过记录。

B 仍必须解决：同一托管 NewAPI 数据集身份/替换、offset 分页移动、迟到事实重扫、采集覆盖的精确定义、历史重归属、分钟边缘查询、保留期和事务迁移。不能把当前纯函数向量当成永久账本，也不能把批内去重冒充这些条件通过。

C 的纯失败请求结束证据仍未解决；当前研究已否定错误条数、当前 RetryTimes 和自由文本结束行的通用推断。原生能力扩展或 NewAPI 最小日志补丁需单独确认，不是此 A 批的隐含前提或授权。
