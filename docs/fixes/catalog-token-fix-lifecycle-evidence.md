# 令牌持久生命周期反向证据校验

2026-10-18，SGM5 B首次本地实现审核发现；未接运行服务、未部署。按用户连续实施授权修正，不新增状态、服务或数据库。

## 一 现象与复现

显式登记一个既有token后，仅把managed_tokens.deleted_at改为created_at，首次实现的verify仍可接受。没有删除操作、没有严格缺失收据，却会投影为已删。用独立临时库固化损坏重开回归，先运行旧实现红例，再修复。

同类反向边界一并核对：已确认创建的全部尝试丢失，或逻辑revision与完整命令历史不一致。它们分别违反“Never不可绑定”和“每个新命令只递增一个版本且不删历史”；新增测试会区分这些情形，不用模拟生产数据。

## 二 根因

首次验证只从已有收据指向身份：有Delete收据时要求deleted_at一致。缺少身份终态反向指向充分证据的检查，因而当收据本身缺失时前向循环没有任何项目可拒绝。仅检查字段类型和非空关系不能证明生命周期可达。

## 三 参考实现对照

B细化T5B.6规定删除必须严格缺失回读后原子保存收据/终态，创建Never不能确认；T5B.4规定逻辑命令版本逐次增加、操作历史保留。既有渠道层也区分持久执行证据与成功收据。本修正不改变A原生协议、SQL事务或通用操作语义，不需要修改NewAPI。

## 四 修正方案

verify按本地身份核对反向证据：deleted_at必须恰有该token的成功Delete目标/同刻missing收据；created已绑定必须有非NotSent创建尝试及原创建成功收据。核对该身份的操作版本历史与当前revision，不接受没有命令依据的版本或重复版本；不新增可失配缓存列。

## 五 正确性论证

登记只产生未删除身份；正常删除在一事务内同时写严格缺失收据和deleted_at，所以增加反向要求不拒绝正常路径。正常创建确认前必须经过prepare，且任何可信NotSent不能让已物化创建成立；保留至少一条非NotSent尝试是可达状态的必要条件。所有新命令仅加1且不删操作，因此版本链可独立核对。SQL或内存校验错误在open只读阶段拒绝，不修补或改写损坏库。

## 六 测试同步

增加无删除证据、丢失创建尝试、无对应历史版本的损坏重开测试；先验证旧实现被拒绝断言失败。保留正常创建/删除确认后重开、历史回执、复杂迁移、PBT及容量测试。另修正legacy夹具缺组（保留断言），把创建PBT的group/expiry/关联预期从原始命令独立构造，并补全部载荷字段的幂等冲突测试和丢expiry变异。

| 类型 | 用例 | 当前结果 |
| --- | --- | --- |
| 回归 | `t5b_reopen_requires_reverse_lifecycle_evidence`：三种不可达历史均须拒绝 | 旧实现0 passed/1 failed，明确接受了[0,1,2]；修正后通过；`58aa5f8` |
| 保持 | `t5b_confirmed_prepared_create_and_delete_survive_reopen` | 正常确认/重开不降级，通过；`58aa5f8` |
| 新增/强化 | `t5b_pb2_creation_delivery_reopen_and_expired_lookup_model`及`t5b_every_payload_field_participates_in_idempotency` | 独立输入预期及字段冲突通过；丢expiry等10类编译变异均被捕获；`58aa5f8` |

修正后定向25 passed/0 failed；变异源码还原后25 passed/0 failed。最终debug/release各603 passed/0 failed/22 ignored，20项独立性能全部通过；完整证据见[B验收](../audits/sgm-5-token-management/b-audit.md)，不用定向结果替代全量。

## 七 代码更新

限定catalog/tokens/verify.rs及必要的新v8约束；新增回归在token_migration_tests.rs，PBT/夹具调整在token_tests.rs。保留旧DDL、A请求、策略、账本和运行入口。修正与验证已完成，本地提交`58aa5f8`。

| 文件 | 位置及改动 | 状态 |
| --- | --- | --- |
| `src/catalog/tokens/verify.rs` | `check`反向核对删除、创建及逻辑版本链 | 已改：`58aa5f8` |
| `src/catalog/schema_v8.sql` | token目标逻辑版本唯一约束 | 已改：`58aa5f8` |
| `src/catalog/token_migration_tests.rs` | 三类不可达生命周期回归 | 已加：`58aa5f8` |
| `src/catalog/token_tests.rs` | 正常重开、独立创建预期、字段冲突及legacy夹具 | 已同步：`58aa5f8` |

## 八 文档更新

同步B细化的生命周期不变量、b-expectations及b-decisions；新增入口同步机械清单。最终红绿、变异、全量与独立审核结果记录于B验收。本记录在修正代码前建立；2026-10-18上述实现、测试及文档三轨完成，本地验收通过，以下文档/入口同步于`58aa5f8`，本次仅回填提交号。

| 文档或入口 | 更新 | 状态 |
| --- | --- | --- |
| `subplans/sgm-5-token-management-b.md` | 反向证据、逻辑历史连续性及边界 | 已同步 |
| `b-expectations.md`、`b-decisions.md` | 独立预期及审核问题处置 | 已同步 |
| `scripts/contract_audit/sgm5_b.sh` | 精确27项清单含新增回归 | 机械检查通过 |
| `b-audit.md` | 红绿、变异、复审与全量证据 | 独审0未关闭；最终debug/release/性能均通过 |
