# SGM-4 A 实现与审核记录

日期：2026-10-17。实现基线 `f0b8295`，代码提交 `dddf28d`。A1–A5 纯函数已实现，专项与独立静态审核通过；完整退出暂未通过，默认 debug 回归暴露了既有认证测试竞态。B/C、整体功能及部署均未完成。

## 实现范围

依据 [A 细化](../../design/source-groups-token-management/subplans/sgm-4-facts-analytics.md)及[事前十节清单](batch-a-expectations.md)，新增 `src/ledger/{types,decode,mod}.rs`：

- 白名单解析原生 Consume/Error 日志，计量字段严格校验；非法可选标签或元数据降级，保留消费。
- 批内按 log ID 去重；同 ID 异值拒绝，request ID 不用作消费去重。
- 先校验全部历史映射，再归属来源/组；未知或组不匹配不会丢失用量。
- 生成 UTC 分钟×渠道×来源×组×原生组×token×模型的稀疏联合增量；只计消费行，缓存不重复累加，取消/EOF 已记录用量照计。
- 精确检查 input/output/消费行数及 token 合计，不溢出、不饱和、不返回半批结果。

只在 main 增加模块声明；未修改 catalog DDL、GLM、worker、认证、面板、依赖或运行入口。带元数据类型不提供 Debug/Display/Serialize/Deserialize，错误只有固定码。无 IO、后台任务、跨批状态、终态判定或火山正向许可。

## 路径 B 独立契约审核

审核者 `sgm4_a_contract_audit` 从契约独立抽取要求，再读取本批 diff；新增文件按完整新增 diff 核验，没有以动态测试结果替代静态判断，也未运行测试或访问生产。

| 契约 | 静态结论 |
| --- | --- |
| 字段、枚举、共享身份与常量 | ✅ 使用 catalog ID/JSON_SAFE；私有类型与固定错误保持边界，无新公开 schema |
| A1 严格核心与元数据降级 | ✅ other 只接受原生字符串；未知字段不留存，错误不会泄漏载荷 |
| A2 规范身份 | ✅ 完整规范事实相等才去重；冲突不覆盖，排序不作为终态游标 |
| A3 历史归属 | ✅ 未用映射也先校验；来源可确认而组不匹配时只保留来源 |
| A4 联合计量 | ✅ 重验手造事实/归属/重复 ID；每条消费只进入一个单元，Error 不加行数 |
| A5 算术与原子返回 | ✅ 输入、逐项加法和 token 合计均检查，局部结果不对外泄出 |
| 副作用与后续责任 | ✅ 无 IO/数据库/路由接线；不冒充跨批去重、coverage 或最终结果 |

首轮发现 1 项 ❌ 和 2 项 ⚠️，复审全部关闭：

1. P1 随机空间偏窄：扩展安全数值、核心字段缺失及各种错误表示、合法/非法 metadata、标签降级和秘密哨兵；期望独立计算。
2. 普通测试入口可能空集通过：加入精确 19 项 manifest，另核唯一 ignored 性能项；空筛选、缺 PBT、普通测试被忽略均拒绝。
3. 最大容量 fixture 压力不足：改为 1024 个不同联合单元、10000 条历史映射，每行 16 KiB other、128 次尝试、256 字节标签及 128 字节错误标签；计时不含夹具构造。

复审结论为无未解决静态审核项，不等于全量回归通过。[处置记录](decisions.md)保留三项关闭依据及退出阻断。

## 路径 A 与专项验证

新增 19 个测试入口：12 个确定性普通测试、6 个各 128 cases 的 PBT、1 个独立 ignored 性能测试。最后一次机械检查普通部分为 **18 passed / 0 failed / 1 ignored**（0.37 秒）。

P1 覆盖解析严格性及降级；P2 覆盖排列/重复/冲突；P3 覆盖历史映射；P4 覆盖独立逐条联合参考值及模型/来源/组/token 四种分项守恒；P5 覆盖不重叠分批可加；P6 使用 i128 算术 oracle 覆盖溢出和 UTC 边界。参考模型不调用被测解析、归属或加算 helper 生成期望。

确定性测试另外覆盖所有核心字段、1024/1025 行、10000/10001 映射、16 KiB other、128/129 次尝试、39 位原生 request ID、标签与请求 ID 字节上界、历史 auto 组、零 token 消费、非零 Error tokens、手造非法结构、同请求不同 log ID、缓存重复计费及取消/EOF。

`scripts/contract_audit/sgm4_a.sh` 通过：步骤、共享类型、纯函数边界、基线零改动范围、格式、测试清单与普通执行均符合期待。另用假 cargo 返回空清单及全部 ignored 清单，均被入口以 exit 1 拒绝，未把零测试当成功。

`cargo clippy --locked --offline --all-targets` 返回 0，仅保留既有 6 条排序/文档缩进警告，本批无新警告。性能 runner 的 8 项模拟回归通过；真实 release 性能结果在下节记录。

## 定向变异

独立源码和独立 target 下逐项修改，每项均成功编译，并由指定测试断言失败捕获（exit 101，1 failed）；不以编译错误充数。10 项完成后逐项还原，副本 src 与主工作树逐文件比较一致。

| 编号 | 故意错误 | 捕获测试（ledger::tests 下） |
| --- | --- | --- |
| M1 | 缺 prompt_tokens 默认零 | core_fields_are_required_strict_and_bounded |
| M2 | 把 Error 计入消费行 | error_and_unrelated_rows_cannot_become_consumption |
| M3 | 丢弃未知模型的消费 | label_degradation_keeps_usage_and_other_valid_dimensions |
| M4 | 消费 input/output 双倍累加 | properties::p4_joint_projection_and_all_marginals_conserve_usage |
| M5 | 按 request_id 合并不同消费日志 | stream_and_retry_metadata_never_reclassify_usage |
| M6 | 先过滤无流量历史映射再校验 | history_validation_precedes_filtering_and_preserves_unknown_groups |
| M7 | 同 log ID 异值直接覆盖 | duplicate_conflicts_check_every_canonical_field |
| M8 | 额外累加 cache_tokens | stream_and_retry_metadata_never_reclassify_usage |
| M9 | 超界时饱和截断 | overflow_in_late_cell_returns_no_partial_delta |
| M10 | 丢弃 client_gone 消费 | stream_and_retry_metadata_never_reclassify_usage |

## 全量回归与退出限制

使用默认测试并发（移除 RUST_TEST_THREADS，不设置 test-threads），编译 jobs=2、nice=15、ionice=3；测试运行期间没有重编译同一 target。所有网络测试使用随机回环端口及假材料，不接运行端口或供应商。

- debug：**385 passed / 1 failed / 6 ignored**，385.85 秒。失败为 `newapi::transport::tests::authentication_waiters_are_bounded_and_cancelled_login_opens_circuit`，`tests.rs:715` 期望假服务已记录 1 次登录，实际 0 次。单独重跑同样失败。
- 基线对照：在隔离副本撤销唯一的 main 模块声明，使有效源码恢复 `f0b8295`，该单测首次通过、下一次执行复现相同 0/1 断言失败（372 filtered out）。所以问题并非由 ledger 调用造成。
- 测试在每个请求上只 poll 1 毫秒，首个登录 future 随后放进 Vec 不再继续 poll，却把“已占队列槽”当作“假服务已收到登录”。二者没有完成同步；本批未改认证实现、测试断言或超时。
- release：**386 passed / 0 failed / 6 ignored**，252.93 秒。默认并发执行全部普通测试，没有通过筛选避开认证用例；单次通过不消除已复现的竞态。
- 独立性能：串行 release 的 **5 项全部通过**（4.81 秒）。新账本最大合法批次耗时 **28 ms**，预算仍为 5 秒；既有 GLM 输入/事务/恢复为 15/1270/980 ms，复杂历史迁移 23 ms，v3 策略迁移 17 ms，火山最大候选选择 2 ms。未放宽任何原阈值。
- `cargo build --release --locked --offline` 返回 0（27.57 秒），仅构建开发 worktree 产物，未替换正在运行的程序。

6 个 ignored 包含原有 1 个用例和 5 个独立性能入口，不是删除或忽略此次失败。认证测试需要独立小步改为显式到达同步，再重新通过默认 debug/release 回归；不能用专项或 release 单次通过代替 debug 失败的处置。

## 五维度复核

- 一致性：A1–A5 与事前契约一致；B 的跨批事务、历史重归属及采集完整性没有被纯函数结果替代。
- 风格：复用现有 serde_json、BTreeMap、catalog ID 和 proptest；无新依赖。
- 正确性：核心字段严格、可选维度保留未知、完整映射先验唯一性、重复身份和算术错误整批拒绝；定向错误均被测试捕获。全量旧测试失败仍是退出阻断。
- 性能：有界输入，map 查找而非 n×m 扫描；普通测试核最大容量，墙钟预算只在独立串行 release 入口核验。
- 可维护性：解码、私有类型、纯投影与测试分离；没有为消除 dead_code 警告而添加假调用、第二采集器或缓存框架。

未读取生产配置、密钥、数据库或日志，未请求运行端口，未重启、部署、推送或合并。
