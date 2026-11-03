# 来源目录容量边界与属性测试修正

日期：2026-10-14。基线：2797dbe。实施提交：163f97b。范围：sgm-1 的局部修复与测试增强，用户已确认上一轮复审建议。按 workflow §7 局部错误路径推进；本文保留八部分用于同步复现、测试和文档，不开启新架构或子计划。

## 1 现象与复现

来源数量达到 1000 后，重新提交已有凭据，实际得到 InvalidInput 和空 source_id；契约要求 DuplicateSource 和原 SourceId。路径是 create_source → insert_source → capacity 先返回，重复查询未执行。分组和成员的容量检查存在同类顺序问题。

最小回归：用已有 legacy(1000) 假数据导入，记下第一条 SourceId，再 create_source 同一 key，断言 DuplicateSource、原 ID，以及快照/secret 数量不变。正式测试命名为 regression_duplicates_at_capacity；修复前应失败，修复后通过。

此外，复审在临时副本中分别将导入 position 反序、将 SecretString::Debug 改成输出原始 key，两次变异都未被原 82 项测试识别。当前实现并没有这两个错误；它们证明 C6/C7 的测试断言不足。

## 2 根因

- 容量限制适用于新增记录，却放在重复身份识别之前，遮盖了应返回的重复错误及复用 ID。
- C1 参考模型遇到重复组时不再调用被测方法，只检查公共 source 数量，没有核对私有 secret 数量与对应关系。
- C6 使用跨来源相同的模型/selector 等值，未沿成员引用核对 order、priority、source/group 及实际凭据，因此错配可能仍满足断言。
- C7 只格式化外层固定脱敏容器，未直接检查 SecretString 的 Debug/Display；指纹只查字段名而未查敏感值。

## 3 参考行为对照

这不是供应商算法或 NewAPI 接口故障，不以外部转发实现作为参考。对照既有 sgm-1 细化 §4.2 的“重复凭据返回已有 SourceId”，以及独立 BTreeMap/集合参考模型：先辨认已存在身份；仅在确实新增时检查容量。数据库唯一约束仍保留作为最终保护。

## 4 修复方案

仅调整 insert_source、insert_group、insert_membership 的容量检查位置：输入/引用有效且无重复身份后、INSERT 之前检查容量。来源仍返回原 ID，不增加表或更改 schema。非重复新增在满容量时仍拒绝。

增强原有 5 个 PBT，不通过增加重复用例数量代替断言质量：

- C1/C2 实际调用重复组创建，检查 source/secret 数量、唯一引用和 key 对应关系。
- C4/C6 为每个来源生成不同备注、模型、header 名和值及优先级；沿 channel → member → source → material 核对完整输入，再检查重开/重放。
- C7 逐个测试凭据 Debug/Display、settings/upstream/quota/validated 容器和错误文本；检查真实指纹值不在普通快照中。
- 加确定性顺序/脱敏回归；在隔离副本重新运行反序、明文 Debug、selector 错配、额外 secret 四种变异，要求对应 PBT 失败。

不顺带修改冗余查询、序列化拷贝、凭据可见性、CI 或外部调度；不推进 sgm-2。

## 5 正确性论证

重复检查和容量检查仍处于同一 IMMEDIATE 事务，调换只读检查不会引入并发插入窗口。重复路径不执行 INSERT，不消耗容量；新身份继续通过原容量门禁及 SQL 约束。错误返回不发布新的实体，事务失败路径不变。

测试预期来自独立输入和参考集合，不使用被测函数计算期望。逐来源可区分数据与关联断言能拒绝保留总量但错配身份/顺序的结果；直接格式化敏感内部类型能拒绝被外层脱敏遮挡的泄漏。PBT 不代替穷尽证明，四种变异只代表已复核的回归类别。

## 6 测试清单

| 类型 | 用例 | 状态 |
| --- | --- | --- |
| 回归 | 满容量的重复 source/group/member 与全新身份拒绝；退休凭据复用 ID | 已加 regression_duplicates_at_capacity；旧实现失败、修复后通过 |
| 回归 | 三个不同来源的导入顺序、priority、关联与材料保持 | 已加 regression_import_keeps_per_source_mapping_and_order，通过 |
| 回归 | SecretString Debug 和 Display 固定脱敏 | 已加 regression_secret_debug_and_display_are_redacted，通过 |
| PBT | C1/C2、C4/C6、C7 补齐独立断言，维持每项 128 cases | 原 5 个属性测试均通过，未削弱其他原有断言 |
| 变异 | 反序、明文 Debug、selector 串源、额外 secret 必须分别被对应 PBT 检出 | 四种均被对应属性测试检出，见下表 |
| 全量 | 既有测试、机械脚本、release 和 Clippy | 85 项通过；机械脚本、release 通过；Clippy 仅 6 条未改旧模块警告 |

变异副本使用自己的源码及 target 目录，不再与开发分支共用构建缓存。真实 config、NewAPI 及生产服务不在测试范围。

本次验证使用临时副本，先跑正常代码得到 85 项通过，再每次只注入一种错误并单独执行目标 PBT。下面的退出码 101 是预期的测试断言失败，不是编译失败。每次注入后恢复，最终源码与开发分支逐文件 cmp 一致；恢复后的完整 85 项测试再次通过。

| 注入的错误 | 命中的属性测试 | 观测失败 |
| --- | --- | --- |
| import position 改为 n-1-position | property_c4_c6_import_replay_and_conversion | 缩减到 n=2，order 实际 1、期望 0 |
| SecretString Debug 输出原始 key | property_c7_no_secret_in_snapshots_or_debug | 实际 PRIVATE_…、期望 [REDACTED] |
| 全部来源引用 cfg.keys[0] 的 selector | property_c4_c6_import_replay_and_conversion | k1 来源得到 X-k0-0 / k0-0-a，与自身输入不符 |
| plan_membership 额外复制一条 secret | property_c1_c2_reference_model | 最小单次加入操作，secret 数量实际 2、期望 1 |

这四项是已实际执行的定向变异，不代表穷尽全部可能错误。此次走 workflow §7 修复及 §5.2 复核；修复前已有独立复审，不将初审报告冒充成修复后的新一轮独立审核。

## 7 代码更新清单

| 文件 | 修改 | 状态 |
| --- | --- | --- |
| src/catalog/store.rs | 三处容量检查移到重复判断之后 | 已实施：163f97b |
| src/catalog/tests.rs | 固化回归、加强 C1/C6/C7，保持既有测试 | 已实施：163f97b |

## 8 文档更新清单

| 文档 | 修改 | 状态 |
| --- | --- | --- |
| sgm-1 细化 | 明确重复身份优先于容量拒绝，补测试 helper 契约 | 已同步 |
| expectations | 补完整字段/直接敏感类型断言和复核结果 | 已同步 |
| audit-report / decisions | 保留初审历史，纠正“无未解决项”的过满结论，记录此次实际验证 | 已同步 |
| 本文 | 回填测试结果和实施提交 | 已同步：实施提交 163f97b，记录随文档提交保存 |
