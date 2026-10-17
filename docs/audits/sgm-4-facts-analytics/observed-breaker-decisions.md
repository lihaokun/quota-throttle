# 观测型熔断审核处置

日期：2026-10-17，O1–O3。用户确认观测型熔断并要求成功有 token 流清零计数，不再要求 NewAPI 补丁。实际运行证据见[审核记录](observed-breaker-audit.md)。

| 审核项 | 处置 |
| --- | --- |
| 原方案把失败终态证据作为开发阻塞 | 以新细化替代：只按明确上游失败观测防抖，承认重试稍后成功前可能提前熔断；不修改 NewAPI，不阻塞独立 key/页面工作 |
| 成功清零与锁存是否冲突 | 字段分离；Success 无条件清当前有效的连续计数，已熔断时也清，但不自动解除 error_exhausted、token 原因或暂停；原状态机已满足，不重写算法 |
| 参数/403 错误测试遍历生产常量，可能共同漏项 | 使用独立的完整契约字面表验证，生产仍只有一份分类表 |
| 随机状态码与多重条件可能让 PBT 全走 None | HTTP 状态加权，并在每个 case 必测六种正 Unavailable、四类随机正 input-only token 活动；保留独立负向决策表 |
| 事实没有 provider/channel_type，不能自行保证未验证适配器不分类 | 明确 O4 可信 catalog 同时核验 Volcano/type 8 的调用前置条件；不给事实增加冗余字段，不冒称 O1 已实现 gate |
| 私有 State/Decision 的 Display 负 trait 不齐 | 补齐 Debug/Display/Serialize/Deserialize 负断言 |

独审先抽契约后读 diff，复审上述反馈已关闭，O1–O3 静态 0 unresolved；代理没有运行测试。没有改 GLM、原状态机算法、DDL/依赖、计量或线上服务。机械门禁通过重命名归一化比较，核原算法和三个旧测试文件的断言未变。

不把类型名字或纯函数测试当持久完成：RequestObservationState 由后续 catalog 同事务保存，O4 还需重启去重、period/error_epoch/fence、采集检查点、源 gate 与成员许可的组合验证。当前没有宣称火山运行调度或整套来源分组已上线。
