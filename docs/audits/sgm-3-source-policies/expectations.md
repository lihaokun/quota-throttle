# SGM-3 来源策略事前契约清单

状态：第一批 A 事前清单已完成验证，详见[审核报告](audit-report.md)。基线 `e1a9279`。本表在编码前依据细化契约抽取；不回填实现位置，不以既有代码的缺陷作为新增需求。B/C 在各自编码前补充，当前清单不构成整个 SGM-3 验收。

B 的状态、迁移和持久化使用独立的[事前清单](batch-b-expectations.md)，当前尚未实现，不把 A 的通过记录套用到 B。

## 1 契约源

- [细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies.md) §1–5、§7–8：A 的范围、类型、逐函数语义与测试门禁。
- [架构](../../design/source-groups-token-management/architecture.md)：G2、来源全局暂停、组隔离、I1/I2/I3/I8 和第三子计划共享类型。
- [需求](../../research/source-groups-token-management-research.md)：GLM 不改策略；同源配额共享；协议不是组。
- [workflow](../../workflow.md) §4.4、§5、§6：设计确认、三层测试、双路径审核。

## 2 Schema 字段

A 的输入输出仅在进程内使用，不新增公开 JSON，JSON Schema 校验不适用。CatalogSnapshot、OperationView、SourceSettings 以及数据库 v1/v2/v3 不变。

| 类型 | 必需字段与约束 |
| --- | --- |
| ProbeInput | source_id、config_revision、quota profile、SecretString；不序列化凭据，Debug 脱敏 |
| ProbeCycle | cycle_id、完成时间、唯一 SourceId 的观测集合 |
| QuotaObservation | source_id、config_revision、cycle_id、观测时间、Available/Unavailable |
| GlmGroupState | active/pinned 为 MembershipId 或空；regime 复用旧 Regime |
| GlmGroupDecision | group_id、group_revision、cycle_id、state、eligible、imminent、pin_release、成员 priority Option |
| GlmPinRelease | membership_id、pct、limit；不把旧算法的整数标签误报为 channel_id |

成员输入复用 WorkSnapshot，包含 source_retire_requested 与 target.retire_requested；不能只靠 MembershipView.lifecycle 识别退役中状态。

不允许加入 provider balance、每组独立 quota 或访问 key 到上游的绑定字段。

## 3 枚举值

- SourceKind、MembershipLifecycle、Regime 复用已有唯一定义。
- 探测结果只有 Available(QuotaStatus) 与 Unavailable；失败不是空 QuotaStatus。
- PolicyError 只有 InvalidInput、Capacity、SequenceExhausted；不含原始正文、header、URL 或 credential。
- priority 的 None 为 Keep，不是 Disabled；本批不增加状态指令枚举。

## 4 流程步骤

实现使用 Rust `// Step Pn:`，机械脚本检查全部存在且独审核对所在语义。

| 步骤 | 契约 |
| --- | --- |
| P1 | 全量校验、唯一 SourceId、有界任务、轮次 checked_add；先校验后网络 |
| P2 | 每个来源一轮一次 quota 查询；单源失败不中断其他来源 |
| P3 | 完成整轮才返回；无凭据、无部分轮次混用 |
| P4 | 校验组/源归属，过滤暂停/退休/退役中/未绑定，稳定顺序 |
| P5 | 仅同轮次且匹配源 revision 的成功值进入 pct/weekly |
| P6 | 调原 GLM 选择，None 保留有效旧 active，正确处理 pin_release |
| P7 | active → eligible → known exhausted → Keep 的 priority 顺序 |

## 5 行为契约

- 正常档严格 `< throttle`；全员越预防线仍可使用 `< exhausted`；restore 和周临期只在原允许条件生效。
- 全部额度用尽保留仍有效的旧 active；全部查询失败不能证明已降级。
- active 查询失败仍获 active priority；未知非 active 保持不写，不混淆两种分支。
- pin 的未知、越界自动解除、暂停/成员移除清理分别测试；暂停不被 pin 绕过。
- 绑定后的 created_disabled 可以参与策略；不能以原生 status=2/3 重写 GLM 候选算法；未绑定不参与。
- 输入组不匹配、缺 source、类型错误、重复身份和超容量必须失败，不静默裁掉非法输入。
- 同源同组的非 retired 成员唯一性必须在暂停/未绑定/退役中筛选前校验；已 retired 历史成员可与新成员并存。
- 外部 weekly 时间与 now_ms 差值不可表示时返回 InvalidInput，不能整数溢出或改变合法时间语义。
- quota 请求保持原 URL/Bearer/selector 顺序与套餐解析，成功模型发现不在 quota 周期内。
- 没有生效确认：策略返回 active 不代表远端渠道已切换。

## 6 时序和状态契约

- 每轮一个编号；旧 revision、其他轮次、失败观测不作为当前成功值；历史展示缓存不能参与当前决策。
- 不持目录/worker 锁等待网络；同一采集器不重入；取消不发布半轮结果。
- 纯决策不改变目录、来源暂停、组 ready、原生渠道状态、旧 applied 或外部服务。
- A 不落组首选数据库，不接 CLI；B 持久化/C 发布完成前不宣称能接管运行服务。

## 7 属性测试不变量

使用 proptest，每项至少 128 cases，保存失败种子；不能把随机 for 循环冒充 PBT。

| 编号 | 生成空间 | 独立断言 |
| --- | --- | --- |
| A1 完整链路等价 | 成员顺序、quota 缺失/边界、weekly、active/pin、阈值和 priority | 经过身份映射，选择/合格/临期/pin_release/None 保留/priority 与冻结参考一致 |
| A2 组隔离 | 两组的独立成员、共享源、无关组成员增删和顺序变化 | 固定当前组输入，另一组独有的变化不改变当前组结果；共享源 quota 变化可影响两组 |
| A3 采集唯一 | 来源集合及随机重复组引用、成功/失败响应 | 每源恰好一次请求；结果键集合等于来源集合；组引用数不放大请求数；非法重复任务在网络前被拒绝 |
| A4 未知隔离 | cycle/revision 匹配与不匹配、失败/缺失组合 | 仅匹配成功值进入已知集；不匹配等价于缺失；非 active 未知为 Keep，active 未知仍保留 |

冻结参考与 golden 不调用被测适配器/新 priority helper 计算期望。定向变异须能击穿：阈值改等号、None 清 active、未知填 0、按成员重复查询、跨组混入。记录触发测试与失败输出，不提交变异代码。

A1 分离 SourceId、MembershipId 和 channel_id；A2/A3 同源跨组必须使用不同成员和渠道身份，不能仅修改 GroupId。A3 还验证每组 eligible 与成功来源对应，避免只检查请求数而漏掉全未知映射错误。

## 8 性能契约

- 输入上限复用 MAX_SOURCES/MAX_MEMBERSHIPS；超限先失败，不能先遍历发网络。
- n 个合法来源一轮 n 次 quota 请求，无自动重试；没有每组/每渠道额外请求。
- 一源失败仍完成其他来源；沿用 10 秒单请求总时限，测试用可控假端点验证有限退出。
- 最大容量的纯映射/选择测试附宽松的 5 秒 elapsed 断言；不把毫秒级 benchmark 当 CI 稳定性要求。
- 没有新增后台轮询器、每轮模型探测、NewAPI 管理调用或数据库写入。

## 9 安全和副作用

第一批允许变更：新 source_policy 模块/测试、orchestrator 的必要可见性和纯 helper 提取、main 模块声明、本子计划文档和本批机械脚本。禁止改 up/run 装配、Cargo 依赖、旧迁移、生产配置和现有部署文件。

机械脚本按已确认实现基线检查 diff；旧 SGM-1/2 门禁不放宽。对假 credential/selector 标记执行格式化泄漏检查，确保结果和错误不带密钥。

测试仅使用本机随机端口和假数据；不调用 3000/3001、不读取生产库、不重启实际服务。无网络依赖的测试离线运行。

## 10 跨实现一致性与验收记录

旧 GLM 与新组适配器是本批两条行为路径；冻结参考基于 `e1a9279`，覆盖调用方保留 active 和 priority 顺序，而非只验证 decide。旧 tick 的 HTTP、applied/dry_run 和快照位置须在 diff 审核中保持不变。

验证完成：新增 17 项基本/边界/属性测试；四项 PBT 各至少 128 cases；六项变异全部被捕获；机械门禁通过；全量离线回归 277 passed、1 ignored；release 通过；独立复审无剩余阻断项，五维度自审通过。过程中的修复、环境问题与构建缓存残留处理见审核报告和 decisions。B/C 和真实遥测合流另验，不以 A 的 mock 通过替代。
