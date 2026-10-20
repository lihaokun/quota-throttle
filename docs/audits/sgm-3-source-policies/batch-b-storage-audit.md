# SGM-3 B 存储小步审核

范围：v4 两张策略状态表、B8–B10 存储入口/验证、基础迁移与重开。实施基线 `0b52e50`。纯算法 B1–B7 保持上一小步实现，本轮不改变 GLM、控制/执行算法或运行入口。复杂旧目录、操作收据和执行历史的随机迁移组合 PBT B5 仍待下一小步，不把基础验证当成完整 B 退出。

## 契约与独立审核

依据：[B 细化](../../design/source-groups-token-management/subplans/sgm-3-source-policies-b.md)、[事前 expectations](batch-b-expectations.md)和 workflow §5/§6。`sgm3_b_storage_audit` 先独立抽取契约，再审核基线 diff/新文件 diff；没有使用主代理测试结论或审核报告。审核及增量复核均无阻断项。

| 契约 | 本轮结果 |
| --- | --- |
| v4 形态及必需行 | 明确区分 revision=0 占位与已初始化；缺行不能等同未初始化；SQL CHECK、严格解码和跨表校验互补 |
| 来源写入 | 同一来源仅一行；显式初始化幂等；退休/退役拒写；两种 revision 复核；用量/原因/游标同事务 |
| 组选择保存 | GLM 和火山复用原状态类型；成员必须属于本组；允许同组历史引用；相同内容不提升版本 |
| 时序与回滚 | 先推进旧周期，再处理动作；旧证据可携合法重置提交；非法/Gap/冲突回滚；提交不确定沿用毒化 |
| 迁移与重开 | 原版本只读核验后备份；追加 v4 保留实例；旧 DDL 不改；打开不读时钟重置状态 |
| 副作用 | 仅本地 SQL；没有新的 ControlCommand、远端收据、渠道意图或 worker/CLI/面板接线 |

独审两项非阻断测试建议均已补充：最终退休后重开的历史读取/拒写，以及仅配置变化时 source/runtime revision 的分离。复核确认补充充分。决策与覆盖精度见[决策记录](batch-b-storage-decisions.md)。

## 自动验证

机械入口：`bash scripts/contract_audit/sgm3_b_storage.sh`。检查代码改动白名单、旧 DDL/纯算法及运行入口零 diff、types 仅数据库版本变化、B8–B10 标记、共享枚举与无网络/时钟、格式和本轮测试。公开 CatalogSnapshot/OperationView wire 仍为 1，无新增公开 JSON schema。

新增 15 项测试，其中 2 项 PBT 各 128 cases：

- `storage_pbt_shared_runtime_and_group_reopen`：独立初始化并逐字段维护运行状态，随机用量/错误/撤错；两个组独立切换选择，每步重开后完整对拍。
- `storage_pbt_conflicts_and_bad_batches_preserve_all_state`：随机来源/运行/组版本冲突、非法证据、游标缺口和外组成员，验证失败后目录快照、私有材料、运行状态和组版本保持不变。

确定性覆盖：占位与同源共享、SQL NULL 约束、严格坏行拒绝、初始化/no-op、两原因及暂停、策略编辑/跨日重置、迟到/重复/缺口、三类 revision 耗尽、源/组默认行写失败、部分状态写失败、组合事务回滚、未知提交毒化、退休历史及基础 v3 迁移/DDL 失败/私有备份。

本轮曾有一项红测试：既有 `control::bump` 的耗尽错误是 InvalidInput，与 B 的 Storage 契约不符。已只在策略存储入口映射，不修改原控制层，也不放宽测试期待。15 项模块测试及机械门禁通过；`cargo clippy --locked --offline -j 2 --all-targets` 通过，仍为原有 6 条警告；`cargo build --release --locked --offline -j 2` 通过。

最终 `cargo test --locked --offline -j 2 --quiet -- --test-threads=4` 为 **317 passed、0 failed、1 ignored**，297.09 秒。旧目录、迁移、控制/执行、认证与 GLM 回归一起通过；既有 ignored 原生 NewAPI 集成未在本轮启动。全量假 HTTP 获准监听随机回环端口，只使用临时测试材料，不访问生产端口或真实 key。

## 隔离变异

源码与 target 分别位于独立副本 `/tmp/quota-throttle-sgm3-b-storage-mutations.MZMxEA` 及其 `target/`，没有复用开发工作树产物。补充测试后强制重新编译，15 项基线先通过；每项变异执行指定单项测试后恢复，均以断言失败和 exit 101 捕获，不把编译失败计为检出。恢复后逐文件比对一致，15 项再次通过。

| 变异 | 捕获测试与证据 |
| --- | --- |
| M1 缺运行行返回 None | missing_rows 回归：应为 Storage，却成功返回未初始化 |
| M2 v4 迁移改写上游配置 URL | migration_preserves_material：迁移前后私有设置不相等 |
| M3 跳过 runtime revision 门禁 | conflicts PBT：旧版本更新成功，不满足 is_err |
| M4 跳过成员所属组校验 | conflicts PBT：外组成员保存成功，不满足 is_err |
| M5 写运行状态时漏掉 cursor | shared_runtime PBT：重开后游标 None 不等于参考 Some |
| M6 仅改配置也提升 runtime revision | config_only 回归：Runtime revision=2，不等于原值 1 |

本表证明本小步断言可捕获这些偏移，不替代下一步的复杂历史迁移组合。

## 五维度自审与边界

- 一致性：与 B 的占位形态、类型隔离、版本和时序约定相符；应用库 v4 与公开 wire 版本分离。
- 风格：复用 CatalogStore/commit/错误类型/严格目录解码和原纯算法，无新增依赖。写入口保持 crate 可见，非测试构建暂允许本模块未接线入口的 dead_code，C 再接单写者。
- 正确性：写入使用 IMMEDIATE 事务；缺行/错误类型/非法历史拒绝；错误后不发布候选。SQL 与解码均校验占位及已初始化状态，防止 NULL 绕过。
- 性能：运行/组按主键读写；全库核验先限界来源/组 ID 数量；无原始日志、重复账本或逐日循环。基础迁移测试使用 5 秒宽松预算，既有规模回归继续运行。
- 可维护性：新代码集中于 catalog/policy；open/insert 只追加必要状态行为；旧版本 fixture 仅移除新增 v4 表以保持真实的历史版本形状，不改旧字段/断言。

未读写生产配置、生产数据库或 NewAPI 库，未启动/重启/部署服务。下一步补复杂历史迁移组合 PBT B5 与 B 的整体验收；C/SGM-4 的事实检查点、历史账本、渠道意图原子发布仍未实现。
