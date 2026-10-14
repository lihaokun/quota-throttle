# 来源目录审核问题处置

日期：2026-10-14。子计划：sgm-1-catalog-store。以下保留初审处置；初审完成不代表后续复审所发现的问题已被当时的测试覆盖。

## 复审后修正

用户要求重新审核后，独立复审与隔离变异确认了满容量的重复来源错误、C1 的重复组/secret 断言不足，以及 C6/C7 漏检反序和内部明文 Debug。处理方案与验证结果记录在[容量边界与 PBT 修正](../../fixes/catalog-fix-capacity-and-pbt.md)。

只移动三处容量检查并加强测试；不顺带重构重复父查询、序列化拷贝或凭据访问接口。后两类是非阻断改进点，不作为本次修复完成的隐含范围。

## 初审问题处置

| 问题 | 处置与验证 |
| --- | --- |
| 持久 discovery 缺少 auth 时复用旧配置默认 Bearer | 改为直接 typed from_str，并把校验后序列化的 Value 与原始 Value 比较；删除 auth 的回读/重开回归必须 CorruptData |
| 私有 SQL 列被写成 BLOB 时误归类 Storage | 结构化 InvalidColumnType/FromSqlConversionFailure/IntegralValueOutOfRange 映射 CorruptData；settings_json/raw_key 分别回归 |
| 只读身份探查 Busy 被归为外库 | 显式 1 秒 busy_timeout，NotADatabase 才映射外库，其余保留结构化错误；DELETE-mode BEGIN EXCLUSIVE 测试通过 |
| 旧 CSV 先去重才检查容量 | 规范化前检查原始逗号分隔项 ≤1000，两层 header 在复制前各检查 ≤64；1001 个重复模型拒绝，1000 个接受 |
| 备份清理失败无法说明目标残留 | CatalogError 增加 incomplete_backup；身份不符不删除，删除失败/无法检查明确标记；注入 remove 失败和身份不符用例通过 |
| 已有备份 sidecar 可能被 SQLite 接管 | 在创建目标之前拒绝其已有 WAL/SHM/journal；保留用户原文件，增加回归 |
| 旧导入属性测试随机维度不足 | 独立扰动渠道查询顺序并比较摘要；随机模型去空去重和两层 header 顺序；不改变原预期 |
| 模板导入分支未直接测试 | 增加匹配模板成功、group/type/base_url 各自不匹配拒绝、实际 models 胜过旧模板且 discovery 保留的用例 |
| 新非平凡 helper 的细化条目不足 | 补充 commit、verify_schema/snapshot/receipt、read_rows/material、cleanup、错误映射等 pre/post、失败与终止论证 |

错误映射采用固定 catalog 错误域和结构化 kind/安全 ID，没有实现泛化的错误上下文框架；这是同一模块内部组织方式，不改用户功能或 HTTP 契约。备份残留标志实现了细化已要求的“不完整文件提示”，不是新的业务状态。

## 测试准备问题记录

首轮新增测试未进入业务断言：tempfile 默认目录权限受 umask 影响，并非保证 0700，安全打开检查正确拒绝。查明 tempfile 实现后，仅将自建测试目录改为 Builder 显式 0700；权限安全条件、断言和负面用例均保留。失败种子保存在 proptest-regressions/catalog/tests.txt。

首次属性测试编译还遇到 prop_assert 对内嵌 format 捕获变量的宏展开限制；改为先计算 Debug 字符串再断言，未改变检查内容。机械脚本首次使用 jq 的 $defs 字段语法错误，改为带引号字段访问；没有跳过校验。

## 既有警告与范围约束

Clippy 的 6 条警告位于未修改的 newapi/orchestrator/quota/status 模块，类型为 sort_by_key 建议及文档列表缩进。本子计划不顺带修改这些模块；新增 catalog 没有 Clippy 警告。

本轮只提交目录基础模块。没有接受生产状态漂移、跳过测试或绕过契约来换取“通过”；也没有修改线上服务来验证新代码。
