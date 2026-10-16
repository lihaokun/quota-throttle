# SGM-4 B1 事务入账事前契约清单

日期：2026-10-17。基线 `4929e7c`。状态：从 B 方案的 B1 契约提取，待用户确认；没有 B1 实现、测试或独立审核通过记录。B2/B3 的网络/查询方案仍须后续细化，本清单不为它们代验收。

## 1 契约源

- [B 方案与 B1 函数细化](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b.md)：§2 前提、§4 私有字段、§5 函数及§8 退出。
- [A 契约](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics.md)：白名单、严格计量、身份/归属、联合分项与整数安全。
- [架构](../../design/source-groups-token-management/architecture.md)：一个应用库/单写者、原生转发不改、I1/I5/I6/I8。
- [能力核验](../../research/source-groups-facts-capability-research.md)：管理员 SQLite ID、offset/时间边界、缺终态/快照证明。
- [workflow](../../workflow.md)：§4.3/4.4、§5、§6；沿既有十节结构，不另装 workflow 模板。

## 2 Schema 字段

数据库新增 v5，冻结旧 DDL；公开 CatalogSnapshot wire=1 不变，无新增 HTTP JSON。

| 对象 | 必核字段及约束 |
| --- | --- |
| ledger_state | singleton、revision、initial_requested_from；未初始化为 NULL，不能当 0 用量 |
| ledger_checkpoints | 两个 lane、generation/window/page/phase/observed_at/fault；Idle/Scanning/Scanned/Stalled 的 NULL 组合与转移完整 |
| ledger_facts | 完整 A 规范身份、计量及 metadata；派生 source/group/issue；原生字段不可覆盖，未知归属可保留 |
| ledger_minute_usage | UTC 分钟与全部联合维度、input/output/consumption_rows；NULL 单元唯一，计数及合计≤JSON_SAFE |
| metadata codec | version=1、封闭 state/字段集合，≤16 KiB；Parsed 的列表/标签边界复用 A；坏持久值报损坏 |
| ScanTicket / LedgerCommit | 名称空间/lane/generation/window/page 与安全返回计数/revision；不带凭据或完整性授权 |

事实身份为单 catalog 名称空间内的原生 log ID；不能把 catalog UUID 当原生数据库未替换的证明。source/group 引用不得级联删除，永久成员归属另做事务校验。NULL 索引内部 0/空串与合法 Some 值不碰撞，不能泄漏为合法公开 ID。

## 3 枚举与类型

- NativeFact、FactKind、StreamObservation、AttributionIssue、FactError、SourceId/GroupId/ChannelId、JSON_SAFE 复用既有定义，不重复声明。
- ScanLane=Recent/History；PageEnd=More/ReachedEnd；ScanProgress 的 Scanned 仅表示遍历结束。
- LedgerError 固定结构，Catalog/Fact 错误不扩展原始文本载荷。
- fault 只使用 B §4 的八个固定采集原因；不生成 FinalOutcome/VolcanoEvidence/Ready/Verified。
- 带事实/标签类型无 Debug/Display/通用 serde，存储白名单 codec 不等于开放前端序列化。

## 4 流程步骤

| 标记 | 核对 |
| --- | --- |
| B1.1 | 旧库只读验证/一致备份→v5 单事务→严格核验→提交；新旧库及重开分支完整 |
| B1.2 | 初始化校验→状态/原值比较→只改起点和 revision；读取不修补缺行 |
| B1.3 | 代次/窗口比较→合法开始/继续→票据；旧响应不能进入新遍历 |
| B1.4 | A2 解析→事务验票→全历史校验→跨批判重→仅新增投影→事实/单元/检查点共同提交 |
| B1.5 | 匹配票据→仅诊断停滞状态→提交；不推进页或清事实 |
| B1.6 | codec、行读写、联合 key、错误和 commit helper 的范围/退出可追踪 |

## 5 行为契约

- 同 ID 同规范事实跨批/重开只计一次；同 ID 异值整批拒绝；同 request_id 不同 log ID 不能合并消费。
- batch 内去重调用 A2；返回 duplicates 含批内及持久同值重复，不把其他日志类型算进去。
- 永久 channel 历史全量先验，包含退休；不能以来源暂停、模型未知或组不匹配丢消费。
- 已知 source 按永久 channel 映射核验，已知 group 才额外要求原生组匹配；GroupUnresolved 和新映射出现后的旧未知事实在 B3 修正前仍合法，重开不得隐式改归属或拒绝合法未知行。
- 入账时只给新增事实做 A3/A4；重复事实不改派生归属，未来修正必须显式搬移联合单元。
- Error 不加 token/消费行；取消/EOF 计量不改；缓存、积分、quota 不额外累加。
- 空页/全重复页可推进合法扫描位置，但不宣布完整零用量或路由可用。

## 6 时序与状态

- 一条 IMMEDIATE 事务包含新事实、联合单元、对应检查点与 revision；失败不暴露成功进度。
- 唯一写连接、事务无 HTTP；B1 无 worker 周期接线或外部 NewAPI 写入。
- 失回执：旧票据被拒绝后重读状态；新遍历重扫依靠全局事实身份去重，不另造批次收据库。
- CommitUnknown 不伪装回滚，沿既有 poisoned 阻止继续写；恢复时检查持久结果。
- mark_scan_fault 是独立、显式诊断事务；不在失败事务中吞错误后提交前半段。
- 数据替换/回退不自动继承 ID 游标或换 UUID 重计；真实绑定和恢复验证属于 B2/SGM-7。

## 7 属性测试

每项至少 128 cases；独立 ID 字典、完整持久字段和逐行整数参考值，不调用被测投影或入账函数生成期望。

| 编号 | 随机空间 | 不变量 |
| --- | --- | --- |
| PB1 | 批划分、重复、顺序、重开、同请求多个 log | 最终事实集合唯一，重复不增加 token/消费行，重开一致 |
| PB2 | 规范字段冲突位置、晚单元溢出、SQL 故障点 | 已知失败的事实/单元/检查点/revision 全回滚；提交未知允许旧/新整批但不能半批，Store 保持 poisoned |
| PB3 | 历史 source/group/channel 各异、跨组共享、退休/未知/组不匹配 | 不跨源猜测、不丢消费；无流量坏映射也拒绝 |
| PB4 | 多分钟/model/token/source/group 及 NULL、零 token、Error | 每事实一个单元；模型/来源/组/key 各边际含未知后与总体守恒 |
| PB5 | lane/代次/页、失回执、旧 ticket、空页、故障标记、重启 | 票据逐字段匹配；诊断不推进；重放不重计 |
| PB6 | 合法 v1–v4 复杂目录/策略/未决执行、迁移失败/重开 | 原数据逐字段保真，新状态形态正确，坏库不先迁移掩盖 |

确定性还需：1024/1025 原始行、10000/10001 映射、NULL 唯一冲突、标签/metadata 字节边界、safe integer/i64/UTC 边界、坏 codec/派生归属、缺状态行、旧锁继承回归及真实 deferred commit 不确定性。

至少八项定向变异按 B §8：重复累计、覆盖冲突、request 去重、只读活动映射、漏联合维度、先提交检查点、NULL 重复、吞溢出。独立源码/target；编译成功后的断言失败才算检测。

## 8 性能与资源

- 每次事实≤1024、历史≤10000；工作量与本批相关，不能逐事实全扫历史或读全库到 Vec。
- 串行 release：最大入账≤5 秒，10 万事实及投影重开核验≤10 秒，既有复杂历史迁移≤5 秒；普通测试只核容量/内容。
- 事实按主键分块验证，投影核验用联合键部分索引双流；EXPLAIN 和更大数据趋势须证明未偷偷做全库内存 map。10 万不是产品上限。
- 保留原备份预算与失败语义；不自动清历史来绕开容量。不通过 max-page/max-row 截断宣称全量。
- 性能入口/CI 必须精确列入新增门禁，不能只靠 ignored 而未执行。

## 9 安全与修改范围

允许 ledger 私有持久 codec/必要 helper 可见性、catalog/ledger、新 v5 DDL 和相关 open/迁移核验接入、测试/文档/机械脚本。旧 DDL、GLM/火山策略、NewAPI 生产认证、worker 业务接线、CLI、面板和运行配置零修改。旧测试 fixture 只能显式移除新增 v5 对象还原旧版本，不清除复杂历史或放宽断言。

不接运行端口或供应商，不复制生产数据库/配置。错误固定码，新增类型负 Debug/serde 检查与秘密哨兵保留。元数据是私有存储，不通过普通快照泄漏。

## 10 对照与退出

路径 A 核 schema/索引、版本、流程标记、共享定义、范围、默认回归、clippy/release 和独立性能；本小步无公开 JSON，wire Schema=N/A，不豁免未来查询接口。

路径 B 按 workflow §6 给独立代理契约及 diff，对照事前期待，关闭 ❌/处置 ⚠️ 并记录审核和决策。最后执行五维度复核、回填真实提交与测试结果。当前这些全部是计划，不标已完成；B1 通过也不等于 B2/B3 或在线用量采集完成。
