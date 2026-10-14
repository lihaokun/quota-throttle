# 来源共享功能实施准备计划

日期：2026-10-15。状态：[架构](architecture.md)已确认，sgm-1 完成，sgm-2 前两批已验证，协调执行未实现；不是授权部署清单。

## 当前完成与未完成

- 已有：[需求方案](../../research/source-groups-token-management-research.md)、[实现前审核](review.md)、已确认架构、可复现的隔离 NewAPI 契约探针。
- 已完成：[sgm-1](subplans/sgm-1-catalog-store.md) 的独立目录库及导入函数、[schema](schemas/catalog-snapshot.schema.json)、[审核与复审记录](../../audits/sgm-1-catalog-store/audit-report.md)。复审后的局部修复为 85 项测试及 release 构建通过，四种定向变异被 PBT 检出；不接管 CLI/运行服务。
- 已验证：58 个既有 Rust 测试通过；11 项原生 NewAPI 契约检查通过，隔离进程退出。
- 当前实施：[sgm-2 细化](subplans/sgm-2-channel-reconcile.md)、[操作 schema](schemas/control-operation.schema.json)、[事前 expectations](../../audits/sgm-2-channel-reconcile/expectations.md)。第一批管理传输与渠道接口完成测试和[独立审核](../../audits/sgm-2-channel-reconcile/audit-report.md)。第二批目录 v2 与本地持久意图完成[审核](../../audits/sgm-2-channel-reconcile/batch-2-audit.md)：当前全量 141 项普通测试及本批 3 项变异通过。协调执行和剩余整体验收仍待第三批。
- 未完成：sgm-2 第三批及整体验收；sgm-3 至 sgm-7 的细化/expectations 与实现，包括调度适配、用量采集、访问 key 管理、三页面 UI、生产迁移与发布。
- 不开展：逐访问 key 上游绑定、火山临期、积分换算、硬 token 限额、修改 GLM 算法、定制 NewAPI 推理路由。

## 分步实施顺序

下列 ID 为本 feature 子计划；sgm-1 已完成实现及验收，其余仍待各自进入编码前完成设计，不以此表代替契约。

| 子计划 | 范围 | 主要退出条件 |
| --- | --- | --- |
| sgm-1-catalog-store | 共享类型、来源/组/成员目录、状态库、幂等旧配置导入 | 类型/唯一性约束、迁移重复执行无重复来源、历史映射不复用、数据库权限/备份策略明确 |
| sgm-2-channel-reconcile | 原生 API 封装、全局流控、持久操作、禁用建渠道、源全局启停 | 跨组 priority 独立；暂停竞态/部分失败/未知 POST 对账；不使用裸 CopyChannel；批量状态回读 |
| sgm-3-source-policies | 共享 quota 采集、GLM 分组适配、火山组首选与双判据/重置 | GLM golden 等价；同源跨组仅一份计数；规则 1 去重/最终结果归并与遥测前提；人工撤销与重置 fencing |
| sgm-4-facts-analytics | 完整元数据采集、分页/补采/去重、时间×模型×token 投影 | 多页/迟到/重启/缺口测试；共享来源与整体不重复；图表分项合计一致；采集不饿死控制 |
| sgm-5-token-management | 普通签发身份、组注册权限、token CRUD/复制/操作对账 | 新 token 单组隔离并关闭跨组重试；旧 token 保持可用；组删除引用保护；创建结果关联唯一；默认快照和日志无凭据 |
| sgm-6-three-page-panel | 来源页、组与 Key 页、数据页、分模型堆叠柱 | 类型专属展示正确；GLM pin/临期保留、火山无临期；写操作显示 pending/failed；整体/per-key 同口径 |
| sgm-7-migration-release | 预检、备份、旧库导入、灰度、回退、发布说明 | 独立环境端到端通过；错误遥测和账号配额就绪；获得生产重启/切换窗口确认 |

sgm-3 的 token 上限及错误规则以 sgm-4 的事实接口为依赖；先定义接口和纯函数 fixture，待 sgm-4 完成后合流验收，不用 mock 通过冒充生产采集已完成。实现可按依赖调整先后，但不跳过各自前置契约。

## 每个子计划的 workflow 要求

1. 先写细化：范围、复用、数据结构、错误路径、并发责任、函数前后条件及非平凡函数论证。
2. 从契约独立提取十节 expectations，定义 schema/枚举/步骤、不变量、性能与副作用检查。
3. 只实现当前子计划范围，补基本、边界和属性测试；禁止修改期望掩盖失败。
4. 执行机械检查和 workflow §6 要求的独立 subagent 契约对照，记录 audit-report/decisions；此要求用于后续实施验收，本轮未冒充已完成。
5. 业务行为和文档同步后报告结果，再进入下一子计划。

## 第一批要锁定的实现细节

- rusqlite 的锁定版本、状态库 schema、ID 和 SecretRef 格式、一次性导入/导出方式。
- 组轮转顺序与 GLM 输入映射；不改 GLM 原规则或 default 历史 ID。
- NewAPI 错误事件分类、同 request/source 结果归并、采集完整边界和迟到处理。探针仅证明一例 503→备用成功可关联，不能代替完整错误分类验收。
- 管理 API 的关联命名、操作 ID、revision、回读集合、重试/退避与预算机械化测试。
- 普通签发身份初始化、可用组和内部余额检查；不可用时明确阻止生成，不能偷偷退回管理员 token。
- 火山受管请求的亲和/指定渠道规则预检与隔离负向用例；不得为此静默修改 GLM 或未纳管路由。

## 验证和部署隔离

测试使用随机或明确的非生产端口、全新数据库和假上游。不得将生产 config.toml 或数据库复制进普通测试目录、不得请求真实供应商来制造失败。

生产前预检需要确认 ErrorLogEnabled 不是无效的管理 option，而是当前版本实际启动环境 ERROR_LOG_ENABLED=true；已运行 NewAPI 如需变更必须经过部署窗口确认。本轮没有改环境或重启生产。

发布先备份敏感配置和两套数据库的一致性快照，记录渠道/来源映射；迁移保留原 token、URL 和 default 渠道。回退先停止新成员可路由状态，再回退程序和兼容配置，不自动删除用户新建来源或访问 key。

## 交付判定

只有完成全部必要子计划、无未解决的阻断项、隔离端到端与 GLM 回归通过，才能称为功能实现完成。此准备计划和当前探针通过不等于可以合并或部署。
