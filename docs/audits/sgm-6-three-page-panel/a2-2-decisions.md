# SGM6 A2.2 设计审核处置

2026-10-19，基线65a4235；本轮只设计，不把静态通过当实现完成。

## 已确认需求保持

GLM原策略不变，火山没有临期概念；每访问key绑定单一类型组且两种下游协议共用。来源凭据/用量/异常共用，组成员渠道和priority独立。沿原worker/store管理接口，不新建任务数据库或调度器。fermat是全新初始化，不导入旧配置/数据，不干预现行服务。

## 审核反馈和决策

| 问题 | 决策 |
| --- | --- |
| 现有async方法仍同步取Core，直接spawn async会阻塞主runtime | 4个全局permit覆盖handoff和真实blocking job；唯一supervisor保留JoinSet；不新建每请求runtime或通用任务registry |
| 原query_usage超时可结束外层等待，但内部读任务还活着 | 所有wrapper join后再等原query_slots许可，不close它、不持Core/commands；所有查询调用方先停止准入。只承诺有序关闭，runtime须存活 |
| 仅宣称blocking池至少8足够，忽略DNS/其他worker | 保持现有Tokio标准默认配置与进度假设，不把局部任务数当完整线程充分性证明 |
| 201丢失后默认发现可能失败，遮住已创建来源身份 | 先只读原凭据唯一索引，重复立即带SourceId；最终事务仍验唯一性，不增加幂等表/预览token或占位模型 |
| 响应next_page误允许0，ledger revision也过宽 | 按原DDL/state校验收紧为>=1，补idle/非idle、两lane顺序、未初始化null等机械关系；修正夹具并加反例 |
| valid_label仅写无NUL，不等于原char::is_control | 统计模型禁Cc且不trim；token名称另要求trim不变；catalog模型继续原禁逗号/NUL规则，不能一起收紧 |
| route返回BoundaryError却可能产生业务422，400/422分类含糊 | 私有AdminFailure只载两个既定wire；格式/未知/缺query为H1 400，已知ID/page语义为业务422，方法优先级与state旧行为分别确定 |
| job panic的internal不在AdminError集合 | sender销毁后，HTTP有效期限内回原H1 500/internal，过期504/关闭；supervisor继续drain，不添task→sender表 |
| “缺issuer不能复制”可能误伤legacy-only配置 | 仅按原owner/callee检查；新创建固定issuer，legacy管理/复制不增加全局issuer条件 |
| 原生分页没有观察时间字段 | B用请求/接收时刻标记刷新，不把created_at当观测时间，不新增伪原子快照 |
| 最大用量合法输入可能编码超过32MiB | 明确完整响应硬预算，超限整份失败、提示缩小区间/筛选；不截断/Top-N，不扩大为所有合法域必可返回的承诺 |

## 结论

独立§4.4最终复审0个未关闭设计阻断，结构、六条完整性、非平凡函数论证、跨模块类型及附录A通过。静态验证65正/44负、6份旧schema规范化一致、28业务code/状态表及实现冻结通过，详见[a2-2-design-audit](a2-2-design-audit.md)。用户确认后才开始编码；本轮无运行服务操作。
