# SGM6 A2.2 设计审核

2026-10-19，基线65a4235。独立§4.4设计复审通过，0个未关闭设计阻断；本轮只交付待用户确认的细化与schema，尚未编码。A2.1已完成的动态门禁不作为A2.2验收结果。

## 事前契约和范围

审核者先只读architecture、三页计划、A2总体和已验收H1，独立提取[十节预期](a2-2-expectations.md)，再查必要既有worker/DDL/查询接口，最后对照[细化](../../design/source-groups-token-management/subplans/sgm-6-admin-business-a2-2.md)及两个新schema。没有从新实现倒推预期，因为本批还没有实现。

方案共27个method/path组合（含原state，新增26个），请求复用16种body定义与2种分页query定义，业务错误28码/9种HTTP状态。仍复用原单写者、operation ID、版本/发布栅栏、token owner和查询；4槽handoff只是传输与句柄所有权，不是新业务任务系统。

## 独审结果

| 检查 | 结论 |
| --- | --- |
| 目标与架构 | G4–G6覆盖；每key单组/双协议、来源共享、组内独立priority、原GLM/火山规则不改 |
| 逐路由wire | 输入、版本类别、callee、成功状态、dry_run与恢复分别声明；目录201、持久202、核验/查询/复制不混同 |
| 类型和schema | 旧wire复用，新类型归属已同步architecture；原始JSON/字节/归一化和关系校验不冒充全由schema完成 |
| 函数完整性 | H2.1–H2.7的分支、退出、callee前提、资源/循环和正确性论证通过；两个配置getter明确trivial |
| 生命周期 | 一个supervisor持publication及业务句柄；HTTP超时不归业务槽，全部wrapper退出后才等原query槽，runtime进度前提明确 |
| 安全与副作用 | 无通用patch、任意owner、明文普通缓存、第二writer或新调度器；旧CLI/status/运行服务冻结 |
| 附录A | 连接、deadline、重试、交付/顺序、失败、状态及背压全部显式，强制abort不冒称drain |

独审修订包括：页码/ledger revision及状态形状按原DDL收紧；统计标签、catalog模型、token名称三种域分开；400/422与两种错误wire闭合；job panic走原H1 internal；legacy复制不增加全局issuer条件。完整处置见[decisions](a2-2-decisions.md)。

## 静态验证

证据位于本机/tmp/quota-throttle-sgm6-a22-design.zhQOeN。两个schema metaschema验证通过；按路由选定$defs而不是总anyOf，65个正例/44个负例全部符合预期。负例必须为validator exit 1且有Schema validation errors，不能把工具失败算拒绝。覆盖默认发现/明确fallback、权限字段拒绝、各revision、分页、unknown/native ID、明文隔离，以及新增页码/状态/控制字符边界。

6份嵌入旧schema经jq -S规范化后逐项一致（字段/值无变化，不声称原文件排版字节相同）。28个业务code与文档9组状态集合一致；git diff检查通过，src、Cargo、scripts及公开配置示例相对基线零修改。没有运行Rust/PBT/性能、真实供应商、fermat或在线服务。

初轮61正/30负曾通过，但独审发现字段值域覆盖不足，不能据此视为完整设计域证明；原目录保留在/tmp/quota-throttle-sgm6-a22-design.zhQOeN-pre-review。之后修正静态样例中的next_page=0和未初始化revision=0为原合法值1，并新增反例及catalog模型保留原域正例；没有放宽预期让错误schema过关。

首次validator无法把process-substitution pipe当schema真实路径，实际是工具加载失败，未计入正负例。改用真实临时schema文件后验证；原失败保存在validator-pipe-failure.log，未改接口契约迁就工具。样例只验证结构/字段约束，不是关联目录执行轨迹，更不是实现输出或PBT。

## 确认与后续

设计可进入用户确认。确认后按内部模块顺序实施A2.2，补精确测试清单、至少6组128 cases PBT、至少12种有效编译变异/恢复重编、默认全量、31项release及原4个debug性能门禁、clippy/build与独立五维度；这些动态工作本轮均未执行。

B/C三页面与组合、SGM7运行接线及fermat全新初始化仍在后续。现行本地服务不在本批修改/重启范围；本轮未推送、合并或部署。
