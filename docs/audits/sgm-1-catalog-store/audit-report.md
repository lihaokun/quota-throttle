# 来源目录与状态库实施审核

日期：2026-10-14。子计划：sgm-1-catalog-store。实现基线：27a03b7。

结论：本子计划通过。路径 A 机械验证、路径 B 独立契约审核及五维度复核均完成，没有未解决的阻断项。此结论只覆盖独立目录库，不代表整个共享来源功能已经完成或获准部署。

## 路径 A 机械验证

| 命令或检查 | 结果 |
| --- | --- |
| cargo test --locked --offline -j 2 | 82 passed，0 failed；原有 58 项保留，新增 24 项 |
| 属性测试 | 5 个 property 函数覆盖 C1–C7，每个至少 128 cases，另重放保留的失败种子 |
| bash scripts/contract_audit/sgm1.sh | 通过：实际序列化快照、JSON schema、8 个非法变异、编译枚举与 schema/SQL CHECK、P1–P14、受保护模块 diff |
| cargo build --release --locked --offline -j 2 | 最终候选增量构建通过；未运行或部署产物 |
| cargo clippy --locked --offline --all-targets -j 2 --message-format=short | 完成；catalog 无警告，6 条警告全部位于未修改旧模块 |
| rustfmt --edition 2021 --check src/catalog/mod.rs | 通过；没有对旧模块做无关格式化 |
| git diff --check 27a03b7 | 通过 |

性能用例覆盖 1000 来源导入、100 组和 10000 成员容量；导入与快照各自带 <10 秒断言。属性用例包括共享来源、类型与渠道唯一性、幂等重放、真实 SQLite trigger 故障回滚、独立渠道乱序、随机模型及两层 header 顺序、敏感字段不泄漏。

机械脚本依赖本机 cargo、rustfmt、rg、jq 和 check-jsonschema；当前已实际运行。未扩充 CI 配置，不能据此声称所有远端 runner 已安装独立 schema 校验工具；现有 CI 的 cargo test 会执行新增 Rust 测试。

## 路径 B 独立契约审核

审核者：独立 subagent sgm1_contract_audit。先读架构/细化/schema/expectations 独立提取契约，再检查 27a03b7 到工作树的 diff；未给实现位置列，未直接读取完整业务源码。以下为独立审核最终反馈的整理，不将主审核的测试运行冒充独立执行。

| 分类 | 独立核对结果 |
| --- | --- |
| ✅ 安全打开 | 私有权限/链接检查、OS 锁、只读身份识别先于持久 pragma；Busy 保留为 Busy |
| ✅ 事务与未知提交 | 来源/secret、完整导入/marker 同事务；提交后才发布；未知提交毒化句柄 |
| ✅ 幂等导入 | 显式 ID 优先、歧义全量拒绝、实际模型及原顺序保留；相同摘要只返原收据，不复活旧数据 |
| ✅ 严格回读 | 原始 JSON 与规范值比较；拒绝丢字段/默认鉴权/模型修补；SQL 列类型损坏为 CorruptData |
| ✅ 历史身份 | channel 唯一覆盖退休记录，复合 FK 约束同类型，有效 source/group 与 position 唯一 |
| ✅ 备份 | 拒绝已有目标及 sidecar，有界 SQLite Backup；只清理本次同身份文件，残留明确标记 |
| ✅ Schema 与泄漏防护 | 快照白名单，敏感容器 Debug 脱敏；真实输出参与 schema 和负例校验 |
| ✅ 属性和边界 | C1–C7、独立渠道乱序、随机模型/header、模板匹配及冲突、容量和性能断言覆盖 |
| ✅ 副作用与文档 | main 仅注册模块，config 仅 Serialize，六个既有运行模块零修改；新增 helper 契约与错误字段已同步 |

最终反馈：没有未解决的 ❌ 或 ⚠️ 契约偏离。首轮提出的问题及处理见 [decisions](decisions.md)。独立审核自行执行了 diff 空白检查、受保护模块零修改检查和 bash -n；没有重复运行会产生临时数据库/构建产物的测试，没有修改文件、索引或生产状态。

## 五维度复核

- 一致性：本模块未接线到 up/run/sync/HTTP，不将本地 planned/imported 写入误报为线上状态生效。
- 风格：Rust snake_case、中文边界说明、既有类型与模型规范化函数复用；不改旧代码风格。
- 正确性：先完整校验，再事务写入，提交后返回；约束与严格解码同时防止错认历史和静默默认值。
- 性能：有界管理规模、批量导入事务、只读快照白名单、无逐实体网络请求；备份循环和锁等待有界。
- 可维护性：types、legacy 纯转换、store、SQL schema 和测试分离；无新路由代理或通用插件框架。

## 验证边界

所有测试使用显式 0700 临时目录和假凭据。未读取生产 config.toml/数据库/日志，未请求真实供应商或 NewAPI，未修改渠道、token、进程或监听地址。

源码实现 Unix 私有权限和本机单写者边界；不承诺网络文件系统、多控制节点或同 UID 恶意替换防护。备份不是磁盘加密。真实磁盘断电未作硬件故障注入，事务失败用例通过 SQLite trigger/延迟约束与重开检查验证。

下一子计划 sgm-2 才实现外部渠道协调器、状态回读和操作队列；来源启停、火山判据、token 管理、三页面及生产迁移尚未完成。
