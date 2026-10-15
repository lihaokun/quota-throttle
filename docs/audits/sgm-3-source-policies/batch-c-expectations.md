# SGM-3 C-1 事前契约清单

状态：用户已确认 C-1；事务适配小步（C3、C4 的本地状态/目标部分）已完成测试与独审。方案基线 `8113696`，实施基线 `fce5187`。本表从 [C 方案的 C-1 细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies-c.md)抽取，不根据实现填位置或预填通过。C1/C2/C5、许可发布/资格唤醒及完整 C-1 退出仍待后续；C-2/C-3 也不在本小步范围。

## 1 契约源

- [架构](../../design/source-groups-token-management/architecture.md) G2/G3/G6、单写者/本地提交先于发布、接管与保密边界。
- [SGM-3 主细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies.md) GLM 等价、Keep 和发布边界。
- [B 细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies-b.md) 持久组状态与事务 helper。
- [SGM-2 细化](../../design/source-groups-token-management/subplans/sgm-2-channel-reconcile.md) 发送 fence、显式启用消费、未知写与确认收据。
- C-1 细化 §1–8，以及 workflow §4.4/§5/§6。

## 2 Schema 与字段

数据库仍为 v4，公开 CatalogSnapshot/OperationView wire=1；不新增 SQL 表、ControlCommand 或公开 JSON 字段。

| 内部结构 | 必需字段/约束 |
| --- | --- |
| worker 配置与采集器 | Arc<Config> 生命周期只读；单个 SourceProbe 的异步单飞锁；不持 Core 等网络 |
| last_glm_cycle | 成功发布后更新的进程内轮次；重复/旧轮次不能续 lease；重启为空 |
| GlmPolicyCommit | 完整新 GLM permit map、轮次、变化 ID 集合/安全计数；提交前构造，提交后无失败查询 |
| PolicyRefreshReport | 轮次、preview_only、变化/获许可/等待计数；无凭据或远端成功声明 |
| RoutingPermit | 现有 source/group/priority/时限外增加 membership_id/member_revision；非前端输入 |

原成员表、组策略表和旧回执按字段核验，不仅核行数。私有 credential、selector、完整配置/URL 错误正文不得进入日志/Report；新增输入不能反序列化为前端授权。

## 3 枚举与错误

复用 SourceKind、Regime、StatusDirective、CreateState、ControlErrorCode；不复制提供者枚举。quota 失败为 Unavailable；缺状态行/错类型是 Storage，不补默认选择。旧轮次为 RevisionConflict；单飞/容量占用为 QueueFull；非法配置/时间为 InvalidInput。未知提交毒化，不能作为普通 rollback 再发布。

## 4 流程步骤

| 标记 | 必须行为 |
| --- | --- |
| C1 | 同一读事务组装来源视图/版本/材料，去重并过滤暂停/退役，随后释放锁 |
| C2 | 单飞采集完整一轮；取消不发布半轮；当前目录重新读取；提交后一次替换许可 |
| C3 | 原 GLM 纯函数驱动每组；None/未入结果成员 Keep；未 managed 不自动接管 |
| C4 | 一事务保存组状态及实际变化的受管 priority；稳定 no-op 不写、不新增操作；资格恢复仅唤醒已有 NotReady 待办 |
| C5 | 从最终目标版本构造有界许可，检查成员身份、资格、观测/预检交集及发送屏障 |

## 5 行为契约

- 原 GLM 阈值、临期、pin/自动释放、未知保留语义不改；两个组使用同一 SourceId 的同轮结果但独立选择。
- 普通 priority 变化不重置 enable_epoch/enable_consumed；已完成 Enabled 的有效含义仍是 Preserve。
- 未 managed 的 imported/planned 记录不产生新 operation_targets，status/dirty/applied/priority 保持原值；屏障仍看到可能路由的旧成员。
- 历史成功回执、执行结果和 manual_paused 不被策略刷新覆盖；只有变化成员的未完成旧操作可按现有规则 supersede。
- 旧许可失效→新许可有效可同事务提前已有 NotReady 待办的 next_attempt_at；稳定资格不重复 wake，不清终态错误，不增加目标 revision 或操作。
- dry_run 不持久策略/目标、不发布真实许可；所有远端写入口均拒绝，包括不依赖许可的创建/禁用。读取可用于预览。
- C-1 不改火山 Runtime/组状态/目标、不消费事实、不做日重置，不签发火山许可，不用测试 fixture 冒充 producer。

## 6 时序契约

- Core 锁内完成计算/事务，HTTP 在锁外；publication 先失效，commit 后才替换完整许可并公布新 epoch。
- 暂停/移除/修改来源发生于 probe 等待期时，以提交时当前目录为准；旧 source revision 的 quota 只作为未知，不回写旧配置。
- SQL 任一阶段失败整轮回滚，旧缓存保留；commit 不确定后禁止新发送。进程在 commit 后、发布前退出时，重启许可为空，重新采集后才能授权。
- 许可绑定最终 member/source/group revision；重复轮次和过期/未来观测不得延长授权。
- lease 截止取 source 观测时间、预检有效期和本次发布预算的交集；不会把 cycle.completed_at 当成每个来源刚采集。
- pending/unknown 写与降旧回读屏障继续优先；本地 fence 不承诺撤回已经发出的请求。

## 7 属性测试与故障矩阵

每项 PBT 至少 128 cases；参考模型不得调用被测提交 helper 计算期望。

| 编号 | 随机空间 | 独立期望 |
| --- | --- | --- |
| C-P1 Keep 与 no-op | 多组 priority Some/None、暂停、未绑定、未 managed、原版本/回执、资格恢复 | 稳定未改成员全字段不变；只提升实际变化目标；恢复仅改约定调度字段；操作数不随 Tick 增长 |
| C-P2 共享 GLM 等价 | 独立来源/成员/渠道 ID、quota/临期/pin/未知、组状态 | 冻结原 GLM 参考算完整状态和目标；同源一轮一次请求 |
| C-P3 原子发布 | 多组故障注入位置、旧许可、commit 不确定、重开 | 状态+目标全旧或全新；不出现未提交许可；未知提交关闭发送 |
| C-P4 时序与授权 | probe 等待中暂停/加成员/改模型、取消、旧轮次、lease 边界、member revision | 旧来源读数不授权；旧成员许可不能复用；不扩宽已消费 GLM 启用权 |

确定性回归：零来源/零组、组未 ready、全部 quota 失败、全部不合格、无变化的相邻轮次、未来/过期时间、版本耗尽、已有成功/unknown 执行历史、每处 SQL trigger 失败、dry_run 创建/禁用/提升均零请求、未接管导入成员不变、暂停先提交而慢 probe 后返回、最大目录容量/超界。

事务小步单列 C-P1 的 Keep/接管/no-op 参考模型和 C-P3 的 SQL 原子回滚子集，各至少 128 cases；完整许可时序不计入本步通过。最大规模用 1000 来源、100 组、10000 成员实际写入/二次 no-op，事务应用预算 10 秒。低层 dry_run 拒写与只读计算本步覆盖，远端 dry_run 门禁仍留 worker 小步。

定向变异至少覆盖：Keep 填 0、no-op 仍增版本、提交前发布许可、逐成员发布暴露半批、移除 member revision 绑定、unknown 当健康、丢失 consumed 使 GLM 重启用、每组重复采集、dry_run 漏禁用写。每项须由断言/编译期负 trait 检出，源码和 target 双隔离。

## 8 性能契约

来源/组/成员容量沿用既有常量；一轮网络来源数不超过活动 GLM Source 数，不按组/成员重复。所有 SQL 在一轮有限集合上运行，no-op 不写逐行状态，也不清空所有观测制造额外 GET。Core 锁内没有 await 或管理 HTTP；最大目录组装/提交单列宽松性能预算和耗时，不能用小 fixture 隐藏规模问题。

不新增轮询周期、补采队列或无限重试；commands 保持 64 上限、probe 单飞，忙时返回 QueueFull。保留当前串行 SourceProbe 的有限最坏时限；过期观察不因慢轮次补成新 lease。

## 9 安全与副作用

只用私有临时库、假 key、随机回环 fake API；不读生产 config/数据库，不请求供应商或运行服务，不执行 up/run/down/sync。业务代码改动严格限于 C-1 的事务适配、worker/permit 和 Config 校验可见性；旧 DDL、A/B 纯算法、认证和 HTTP 协议实现不改。

constructor/旧测试夹具签名调整不能放宽旧断言。未授权 imported 成员不得被周期更新伪装为 managed。正式接入 CLI/部署仍由 SGM-7 明确确认。

## 10 退出与跨实现核对

原 GLM 冻结参考、SQL 全字段模型、提交/缓存事件轨迹与 fake HTTP 请求记录相互对照。需机械白名单/标记/共享枚举/公开 schema 检查、本批与全量回归、PBT/变异、release、独立契约审和五维度复核全部通过，才算 C-1 完成。

事务适配小步的实现和验证见[审核记录](c1-tx-audit.md)，只覆盖 C3/C4 的本地 SQL 范围，不代表整个 C-1 通过。C-1 通过不能代替 C-2 命令语义、C-3/SGM-4 事实能力验证或生产部署授权。全量预检运行期间不并行重编译同一 target。
