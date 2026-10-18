# SGM5 B 持久令牌操作审核

2026-10-18，基线`ec17fc9`。B本地验收通过：独立设计审及实现复审0个未关闭问题，定向、机械、10类编译变异、clippy/build、最终debug/release全量及20项独立性能均通过。只完成持久操作层，不代表C组合、页面或部署完成。

实现、测试及契约本地提交`58aa5f8`；提交前文档同步后机械检查再次通过。独立收尾复核对照最终日志确认完成声明准确；后续本次回填仅涉及文档，不改已验收源码。

## 实现范围

同一个CatalogStore追加v8，不另建数据库或操作服务。原control_operations拥有phase/error/幂等ID；三个侧表分别保存身份、目标/历史收据、执行尝试。当前OperationView wire3增加token kind/target，CatalogSnapshot及NativeTokenView不变。原生ID与本地ID明确分开，不保存完整key或原生状态缓存。

创建未知只按稳定关联查回；成功ACK仍非成功。当前版本及证据栅栏绑定单条回读，旧失败重试不能重新阻塞新版本，已确认成功不会因重开降级。状态写的不确定性不被匹配GET洗白；严格删除缺失为单调终态，历史身份不重认。复制入口只返回私有身份供C再做当前准入，并不返回明文。

B不发HTTP、不接worker/CLI/面板，不创建账号、改余额或组配置。只为A共享枚举补Deserialize用于SQL解码，其HTTP行为/认证/预算均冻结；既有ExecutionOutcome仅开放同catalog内的columns复用。GLM/火山/账本算法、旧DDL、依赖及现行服务未改。

## 设计与实现独审

审核者先从架构、A/B/C边界独立抽十节，再审B细化及新增diff；未先看实现构造预期。设计阶段关闭旧失败重试、成功恢复、有效读取时间/代次、legacy标签和最终发送接口等问题。实现阶段发现删除反向证据缺口、夹具缺组及PBT预期同源，均按[处置记录](b-decisions.md)与[修正方案](../../fixes/catalog-token-fix-lifecycle-evidence.md)处理。

反向证据修正先红后绿：旧实现接受无删除收据、丢创建尝试、无历史版本三种状态，回归0 passed/1 failed；修正后这些状态在只读重开验证拒绝。版本链只约束逻辑命令，证据/尝试序号仍可有间隙、同命令仍可安全多次尝试，没有新增状态或恢复服务。

新增27项入口：25普通、2独立性能；5组PBT各128 cases。PB1从规范JSON与原始命令独立核摘要和完整spec；PB2从输入组/expiry/独立关联构造事实，不用生产create_spec生成预期；PB3独立状态轨迹与历史回执；PB4强制交叉已有账本token77并比较全部旧表；PB5在原复杂历史fixture上比较迁移前镜像、备份和重开。测试夹具缺组与删除父记录前未处理外键子记录均修正，保留原业务断言。

补充覆盖同phase/同时间的证据变化、confirmed但无ACK后直接开始下一写、所有创建载荷字段/状态载荷的幂等冲突、pending64/列表100/身份10000边界、准备/结算/确认/重试/重开恢复的SQL失败回滚及原poisoned停写。poisoned入口测试不冒充本批重新实现COMMIT未知注入，原commit实现和既有相关测试保留。

## 验证记录

| 项目 | 结果 |
| --- | --- |
| 编译检查 | cargo check通过，17.31秒 |
| 生命周期旧实现红例 | 0 passed/1 failed，0.51秒；明确列出accepted states [0,1,2] |
| 修正后定向 | 25 passed/0 failed，99.96秒 |
| 机械检查 | schema/枚举/Step、精确27入口及空/全ignored/漏项负例通过；旧算法/DDL/HTTP及关键原控制函数冻结通过 |
| 性能入口自检 | 8项通过，新增2项独立性能登记到完整集合 |
| 独立静态复审 | 0个未关闭问题；迁移/身份/回执、逆向历史和PBT修正通过 |
| clippy | all-targets通过，1分25秒；6项既有警告与1项已评估的同步枚举布局提示 |
| 首轮debug全量 | 456 passed/147 failed/22 ignored，1118.01秒；回溯逐项分类145个socket EPERM、1个旧wire2预期、1个原5秒计时断言；保留失败证据，最终debug复验见下 |
| 首轮release全量 | 457 passed/146 failed/22 ignored，792.47秒；145个socket EPERM、1个旧wire2预期，原5秒组操作通过；不是最终验收 |
| release构建 | 成功，1分18秒；开发树产物未安装或启动 |
| 编译变异 | 10类全部捕获，逐项确认为成功编译后精确1测试失败；源码还原比对后25 passed/0 failed，103.15秒 |
| 最终机械/clippy/build | 机械27入口通过；clippy 20.65秒、仍为已记录7项提示；release build成功0.47秒 |
| 最终debug全量 | 603 passed/0 failed/22 ignored，1417.52秒；默认测试并发、允许隔离回环且无其他自有编译任务；原5秒组操作断言未改并通过 |
| 最终release全量 | 603 passed/0 failed/22 ignored，1067.43秒；与debug相同范围、默认并发，未跳过旧回归 |
| 最终独立性能 | 20 passed/0 failed/0 ignored，81.58秒；完整固定清单串行执行，新增身份基线459ms、旧操作迁移574ms，各低于原5秒阈值 |

最终顺序验证脚本正常退出0，之后仅同步文档；没有改动已验收Rust源码或测试阈值。

变异依次覆盖漏删除证据、漏创建尝试、漏版本历史、丢创建expiry、摘要漏enabled、匹配GET洗白未知、旧失败重试、旧读取忽略证据、发送忽略证据、重开误判NotSent。变异及所有初轮任务结束后，仅把旧C2测试的wire预期2同步为3；生产实现与全部B测试未再改动。最终验证允许隔离回环，并串行运行机械/clippy/build/debug/release/性能，不叠加自己的其他构建或测试。

默认忽略的22项中20项走独立性能入口；两个既有opt-in（原生组合/百万历史趋势）本批不重跑。B未改原生HTTP，不把A前批15项原生探针说成本批重新执行。

复现入口：`scripts/contract_audit/sgm5_b.sh`、默认`cargo test --locked --offline`及release、`scripts/tests/performance-gates.sh`。正式日志`/tmp/quota-throttle-sgm5-b-green.log`、`-mechanical.log`及后续同前缀；最终全量为`-final-debug.log`、`-final-release.log`，性能为`-performance.log`。变异独立副本与顺序验证脚本位于`/tmp/quota-throttle-sgm5-b-mutants.19Ly2K`。初始targeted日志曾交错，仅供排错，不作验收证据；正式后续每轮单独日志。临时证据可能由系统清理。

首轮沙箱EPERM产生的8个未跟踪PBT种子文件在两轮退出后移动至`/tmp/quota-throttle-sgm5-b-sandbox-regressions.eZDOnA`，可恢复；不混入功能提交，不删除原有受版本控制的回归种子。

## 五维度与剩余边界

- 一致性：B仅持久事实/意图，不把Succeeded创建事实当当前就绪；C仍承担A调用、真实准入、取消后结算和当前复制门禁。
- 风格：复用原事务、错误、操作命名空间、交付证据与版本模式；只有必要的token专属表，无新恢复框架。
- 正确性：前后双向历史检查，逐token串行、完整版本栅栏、未知交付及严格回读；失败原子回滚。
- 性能：有限身份/页/候选、pending部分索引、历史流式校验；无后台网络轮询，20项独立预算通过。新证据仅覆盖10000条imported身份且无token操作/attempt历史的verify+投影，以及指定10000条旧控制操作迁移。未测10000条created身份的完整历史成本，也不承诺无上限历史在5秒内完成；阈值保持原5秒。
- 可维护性：旧策略/账本/HTTP及DDL冻结，旧夹具只适配v8降级/当前版本边界并复用复杂历史，不放宽原断言。

保留限制：未知一般状态写若永远取不到原尝试终止证据，会保持未决；无强行解锁。原生实例不被替换/回滚、ID不复用及单写者假设仍必要。C组/签发身份就绪和worker组合、三页面及运行发布尚未完成；本批没有重启、推送、合并或部署。
