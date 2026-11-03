# 来源目录与状态库契约验收清单

子计划：sgm-1-catalog-store。日期：2026-10-14。状态：保留 Step 0 事前预期，实施后逐项核对通过。预期在基线 27a03b7 已提交，未从实现反向编造；修正和新增回归见 decisions.md。

版本范围补记：以下勾选是 SGM-1 当时 v1 范围的验收，不是当前 HEAD 的整体验收。后续 SGM-2 合法旧库迁移、控制/执行侧表及恢复语义见[迁移增量核对](migration-scope-review.md)与 SGM-2 expectations；本次不重写原 Step 0 或放宽历史脚本。“不自动迁移”不应解读为后续版本永远不能按已批准的迁移契约升级应用状态库，更不构成生产迁移授权。

## 1 契约源

- [主架构](../../design/source-groups-token-management/architecture.md)：G1/G2/G6、共享 Source/Group/Membership、不变量 I1/I2/I3/I6/I8、第一子计划共享类型。
- [子计划细化](../../design/source-groups-token-management/subplans/sgm-1-catalog-store.md)：§1 范围、§2 依赖、§3 错误、§4 类型与数据库、§5 函数契约、§6 验收。
- [快照 schema](../../design/source-groups-token-management/schemas/catalog-snapshot.schema.json)：唯一公开 JSON 输出白名单。
- [workflow](../../workflow.md)：§4.4 实现前审查、§5.2 五维度、§6 v2 双路径。

后续编码不得从实现反向修改此预期来掩盖失败；发现契约错误需记录原因并先更新设计，保持版本差异可追踪。

## 2 Schema 字段

| 对象 | 精确字段 | 类型与机械检查 |
| --- | --- | --- |
| 根 | schema_version, sources, groups, memberships | 版本固定 1；数组，不接受其他字段 |
| source | source_id, type, name, note, manual_paused, retired_at, config_revision | ID 字符串、枚举、文本、布尔、可空非负毫秒、正 revision |
| group | group_id, newapi_group, name, type, revision, ready | 单组名；不得 auto/逗号；ready 布尔 |
| membership | membership_id, source_id, group_id, channel_id, order, lifecycle, desired_priority, observed_status, observed_priority, applied_revision, retired_at | planned 无渠道；imported/created_disabled/ready 必有渠道；retired 与 retired_at 同时出现 |

所有层 additionalProperties=false；schema 不承担跨数组外键、i64 上界或唯一性证明，Rust/SQL/property 测试另验。用实现实际序列化输出校验，不用手工 fixture 代替输出证明；负样本验证未知字段/枚举、凭据字段、错误 null 和类型确实被拒绝。

## 3 枚举值

| 枚举 | 精确集合 | 唯一定义和引用 |
| --- | --- | --- |
| SourceKind | glm, volcano | src/catalog/types.rs；不在 store/legacy 重定义 |
| MembershipLifecycle | planned, imported, created_disabled, ready, retired | src/catalog/types.rs；schema/SQL 常量比对 |
| ModelDiscoveryAuth | bearer, authorization_raw, x_api_key | 复用 src/config.rs，不新增另一份发现鉴权枚举 |
| observed_status | null, 1, 2, 3 | 保留 NewAPI 观测语义，不能与业务三态混用 |

sgm-1 只产生 planned/imported；其余生命周期为 schema 约束及历史读兼容保留，不提前暴露状态修改接口。

## 4 流程步骤

代码使用 Rust 注释 // Step Pn:，机械检查要求每项出现在对应函数，不只比较注释总数。

| 步骤 | 预期 |
| --- | --- |
| P1 | 校验显式路径、私有权限、符号链接/硬链接 |
| P2 | 排他进程锁及仅新文件创建 |
| P3 | 外库/高版本在任何持久 pragma 之前拒绝 |
| P4 | SQLite 配置及新 schema 原子创建，旧 v1 不自动补表 |
| P5 | WAL/FULL 回读、完整性检查后才返回句柄 |
| P6 | 导入索引保留歧义；容量及重复 ID 检查 |
| P7 | 显式 ID 优先，名字必须唯一，不跳过无法解析项 |
| P8 | 同组和模板一致性；实际 models 优先 |
| P9 | 按原顺序构造 GLM 来源，保留两层 headers |
| P10 | 全量验证后生成确定性 Plan 与摘要 |
| P11 | 已导入先比摘要；同摘要只返回原收据 |
| P12 | 首次导入要求空目录，建待核对组 |
| P13 | 来源/凭据/成员成组插入；旧渠道 imported |
| P14 | 导入标记与所有实体同一提交点 |

## 5 行为契约

- [x] create_source 不复制同凭据来源；归档身份也不能被重复创建绕过。
- [x] 输入/引用有效时，满容量不遮盖重复 source/group/member；来源返回旧 ID，新身份仍受容量约束。
- [x] create_group 只建本地 ready=false 记录，不注册外部组。
- [x] plan_membership 校验存在、类型、退休、唯一性，允许对暂停源建立未生效计划但不启用。
- [x] 同一来源加入两个同类型组后只有一份 source/secret，两个独立 membership。
- [x] importer 不采用现有 CLI 的“缺失渠道跳过”行为；任意歧义/冲突全量失败。
- [x] 同计划重复导入不新增、不重置字段；不同计划和非空首次导入明确拒绝。
- [x] 新建来源不填写虚假的用量 0 或可用状态；火山仅保存策略配置。
- [x] unknown enum/字段及非法库内容报错，不转成 GLM/default/零值。
- [x] 快照不是备份；私有材料读取不具备 Serialize，也不供 HTTP 直接返回。
- [x] 备份包含 WAL 已提交数据；目标已存在时不覆盖。

## 6 时序和状态契约

- [x] 单 Store/进程锁先于 SQLite 事务；连接私有，写接口 &mut self，无网络等待。
- [x] commit 成功后才发布 View/收据；提交不明毒化句柄，重开后对账。
- [x] 来源/secret 成组提交，导入批次与 marker 成组提交；故障不留半份目录。
- [x] imported 不等于 ready；观测字段/applied_revision 为空，旧路由不变。
- [x] 已导入后修改/退休记录，重放旧计划不能恢复其旧值。
- [x] 退休渠道身份保持 UNIQUE，channel_id 不能改认其他来源或组。
- [x] 进程退出释放 OS 锁；遗留锁文件不误判为永久占用。
- [x] 外库/高 schema 拒绝前不切换 journal_mode、不写 schema、不清空目录。

2026-10-17锁生命周期修正：成功持锁对象在SQLite连接关闭后显式unlock；单个File关闭不作为继承/重复句柄全部消失的证明。未持锁失败路径不得解锁，仍保留活跃Store排他及拒绝外库/损坏库条件。新增确定性副本存活重开、真实关闭顺序、错误退出、跨进程排他与128-case随机句柄生命周期检查，见[修复记录](../../fixes/catalog-fix-inherited-lock-release.md)。原历史勾选不替代本轮完整验证。

2026-10-17 B3 设计增量（已确认、尚未实施验收）：独立只读查询拟共享同一 Arc guard，最后一个真实数据库使用者退出才解锁；Store 关闭而查询/target 活着时必须拒绝重开。原裸 File 副本、失败获取/验证退出、真实连接关闭顺序和跨进程排他回归全部保留；新取消路径不能提前释放 guard。具体见 [B3 expectations](../sgm-4-facts-analytics/batch-b3-expectations.md)，不将旧勾选视为新生命周期已经通过。

## 7 不变量和属性测试

使用 proptest，每项至少 128 cases；随机过程与独立 BTreeMap/集合模型对比。验证代码不得读取实现内部派生出的期望值。

| 编号 | 对任意合法操作序列恒成立 | 检查方法 |
| --- | --- | --- |
| C1 | 相同凭据最多一份 Source/Secret；多组只增成员 | 随机重复创建及跨组加入；实际重复组调用；核对 source/secret 数量、唯一引用和 key 对应关系 |
| C2 | 有效成员两端类型相同，有效 source/group 组合唯一 | 混合 GLM/火山生成器；成功/错误后均检查 |
| C3 | channel_id 永远不映射两个来源/组，含退休历史 | 导入映射置换、重复 ID、含退休行数据库约束 fixture |
| C4 | import(P); import(P) 与 import(P) 的目录及收据一致 | 随机顺序/备注/模型输入，重开后重放 |
| C5 | 事务提交前任一注入故障后，目录等价于操作前 | secret/source/member/marker 写入间故障；重开检查无孤儿 |
| C6 | 旧 key 顺序、关联、priority、凭据、模型与两层 headers 不漂移 | 每来源使用不同输入；沿 channel/member/source/material 比较完整元组；独立模型去重预期；渠道查询顺序扰动 |
| C7 | 快照、敏感对象自身 Debug/Display 与错误文本不泄露凭据/指纹/header | 直接检查 SecretString 及 settings/upstream/quota/validated 容器，检查敏感值（含指纹）而非仅字段名；哨兵仅放敏感字段 |

人工修改测试中的退休/暂停行只使用私有测试 fixture，不为测试方便添加危险的生产 setter。

## 8 性能契约

- [x] 最大来源 1000、组 100、成员 10000；超限错误不改变目录。
- [x] models≤1000、每层 header≤64；大输入在循环前拒绝。
- [x] SQLite busy_timeout 显式 1000ms；锁竞争测试不无限等候。
- [x] 备份最多 10000 steps；Busy/Locked 最多 100 次，重试循环期限 10 秒。
- [x] 标准临时目录中 1000 来源导入和一次快照在本地/CI 普通测试环境各有 elapsed<10s 的宽裕断言；不把此阈值当线上磁盘 SLA。
- [x] 源码确认没有每实体网络请求、没有事务内 await、没有全量快照中的私有配置复制。

## 9 安全与副作用

- [x] 运行依赖只新增固定版本 rusqlite；proptest/tempfile 仅 dev-dependencies。
- [x] 凭据仅在应用私有 SQLite/备份明文保存，权限检查有效；不虚称加密。
- [x] 错误、Debug、普通快照不回显敏感字段；参数化 SQL，不拼接用户值。
- [x] schema/application_id 门禁阻止误开 NewAPI/任意 SQLite 库。
- [x] 所有测试使用 TempDir/假凭据，不读真实 config.toml，不发真实网络请求。
- [x] main.rs 仅注册模块；config.rs 仅必要 Serialize derive；既有六个运行模块保持零业务改动。
- [x] 不修改生产数据库/渠道/token、进程、端口或启动参数，不自动迁移。
- [x] 备份错误只清理本次创建的失败目标，绝不覆盖/删除既有文件。

## 10 跨实现一致性

- [x] Rust 枚举/Serde 字段与 JSON schema 一致；SQL CHECK/外键与同一值域一致。
- [x] 公开 ID 字符串经 JSON 往返不丢精度；>i64::MAX 的十进制输入由 Rust 拒绝。
- [x] Store 查询结果和独立参考模型相同，不以 SQL 表自行计算的派生结果作唯一 oracle。
- [x] 既有 58 个 Rust 测试保留并通过；此数字是实施前基线，不是新模块验收数。

## Step 5 验证记录

初审 2797dbe 的 82 项通过不能代表所有已声明不变量均被充分测试；复审揭示了容量边界和 C1/C6/C7 盲区。此次按 §7 修复后为 85 项通过，原 5 个 PBT 每项仍为 128 cases，release 构建及机械检查通过。四种定向变异均被对应 PBT 检出，详见[修正记录](../../fixes/catalog-fix-capacity-and-pbt.md)。

Step 0 文档检查已执行：check-jsonschema 验证 metaschema 与设计样例通过；8 个变异负例（额外 credential 字段、auto 组、imported 无渠道、planned 有渠道、退休缺时间、未知来源类型、数字 ID、零 revision）全部被拒绝。Markdown 本地链接检查和 git diff --check 通过；src 与 Cargo 文件无变更。

路径 A 已执行：cargo test --locked --offline -j 2；bash scripts/contract_audit/sgm1.sh 校验实际快照、schema/SQL/编译枚举、负例、步骤和副作用；cargo build --release --locked --offline -j 2 通过。Clippy 仅有 6 条未修改旧模块警告。

初审和后续独立复审均已执行；后续复审纠正了初审结论。此次局部修复采用 §7 回归与 §5.2 复核，没有把旧独立审核结果充当修复后的新一轮审核。报告、问题处置和此次实测结果分别保留。
