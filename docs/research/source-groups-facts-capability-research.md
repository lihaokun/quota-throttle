# 来源分组的用量与请求终态能力核验

日期：2026-10-16。承接 SGM-3 C-1，供 C-3 与 SGM-4 细化使用；本轮是隔离调研，不是事实模块或火山自动调度的验收。

现有 NewAPI 消费日志能提供按模型、来源、访问 key 和组统计的基础字段。但已核验的消费及错误日志，不能完整证明每个失败请求何时结束。必须把“记录了消耗”与“来源最终成功或失败”分开，不能直接把三条错误日志变成三次失败，也不能用等待时长补出终态。

## 核验范围

- 开发分支：`research/source-groups-token-management`，Rust 基线 `47206ca`。
- NewAPI：固定 `v1.0.0-rc.20`，源码提交 `6ce7305cd36f16506fb6a2c3c524a5a318539ba7`。
- 二进制 SHA256：`90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205`。
- 探针：[newapi_facts_probe.py](../../scripts/probes/newapi_facts_probe.py)。仅启动私有临时目录、新 SQLite、随机非 3000/3001 端口和回环假上游；管理写入前检查进程拥有端口。临时凭据和假上游 key 不写入结果。
- 两个来源 S/T，两个测试组；S 在两组各有一个渠道，T 仅在第二组作为低优先级备用。测试用管理员签发的临时 token，不把它当作 SGM-5 普通签发身份的隔离证据。
- 未读取生产配置、数据库、日志或真实 key；未请求供应商，未重启、部署或更改运行服务。

现有目录库已有来源与历史渠道身份，B 已有火山状态机，C-1 已有协调器单写者及事务后发布；这些继续复用。SGM-4 仍是唯一事实账本，不在策略模块另建错误计数采集器。

## 可复现结果

`type=2` 表示消费记录，`type=5` 表示错误记录。下表按一次客户端请求描述；来源命中序列来自假上游，HTTP 结果来自探针客户端，不能冒充后台采集器能看到的证据。

| 场景 | 客户端及上游观测 | 原生日志 | 对方案的约束 |
| --- | --- | --- | --- |
| 正常非流式 | HTTP 200；S | 1 条消费，input=10/output=2 | 消费维度及 token 可读取 |
| 同源失败后成功 | HTTP 200；S→S | 同 request/channel 的 1 条错误和 1 条消费 | 不能在第一次错误出现时固定失败终态 |
| 所有尝试失败 | HTTP 503；S→S→S | 同 request/channel 的 3 条错误 | 这是一个请求，不是三个独立请求；这些记录没有明确最终结束字段 |
| 换源后成功 | HTTP 200；S→T | S 错误、T 消费，同 request_id | T 成功不能清掉 S 的失败；T 被阻塞时，S 错误已经可查询 |
| 请求中改变重试次数 | HTTP 200；S→S | 1 条错误后出现同源消费 | 当前重试配置和已用渠道数不足以证明终态，详见下节 |
| 完整流式 | HTTP 200，有下游 DONE | 消费 10+2；`stream_status=ok/done` | 可观测原生流式结束分类 |
| 上游提前 EOF | 上游没有 finish/usage/DONE，下游仍得到 HTTP 200 和 DONE | 消费 9+2；`stream_status=ok/eof` | 下游 DONE 不等于上游完整结束；9+2 是该假请求的原生计数，不是供应商账单 |
| 客户端取消流式 | 收到数据后主动断开 | 消费 9+2；`stream_status=error/client_gone` | 已有 token 仍计量；消费行不能一律当成功，更不能把取消当上游不可用 |

另有一项日志边界测试：流式请求仍在途时，另一条请求的 URL 路径含编码换行，使临时 `server.log` 出现带该请求 ID 的伪造 GIN 结束行。因此不能仅解析普通控制台文本来认定请求结束。探针只在自建实例发送这条测试路径。

完整增强版连续两次通过 8 个场景及日志边界断言，两个自建 NewAPI 均退出：

- `/tmp/quota-throttle-facts-probe-_nabd_33/result.json`
- `/tmp/quota-throttle-facts-probe-0780yqw6/result.json`

结果文件只保留白名单元数据。临时路径可能被系统清理，复现以脚本和固定二进制为准：

```bash
python3 -B scripts/probes/newapi_facts_probe.py --binary /absolute/path/to/new-api-v1.0.0-rc.20
```

早期夹具有两次失败：第一次过早读取到错误行，消费行尚未出现；第二次只发一个流式 chunk，被原生转换器缓冲。现已分别改为等待明确期望的记录集合、发送两个 chunk 后才阻塞。增强版还将取消分类和日志插入结果改为明确断言；没有把固定等待时间当作请求终态。此处的等待超时仅判定探针失败。

## 为什么重试次数也不能替代结束标记

反例时间线：请求开始时 `RetryTimes=0` → 第一尝试被假上游阻塞 → 改为 1 并释放失败 → 第二尝试开始且继续阻塞 → 将配置改回 0。此刻读取到的配置为 0，日志中已用渠道数为 1，但请求仍在途，随后同源成功。

这否定了 `used_channels.len == current_RetryTimes + 1` 就是最终失败的推断，也说明请求开始时的配置未必固定该请求的实际重试上限。源码中循环及重试判断读取全局 `RetryTimes`；错误记录发生在是否继续重试的判断之前。未持久化的跳过重试条件也不能由当前配置完整还原。[固定版本 Relay 源码](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/relay.go#L174)

把重试全局关掉会改变现有 GLM 和请求行为，不是本项目已确认的方案，不能用它规避事实缺口。

## 采集设计需要保留的边界

消费和错误记录带 `request_id/channel/token_id/group/model_name` 等字段；错误不增加 token。管理日志列表使用时间及 ID 倒序、offset 分页，计数与取页不是同一个快照。因此“扫完这一页”不等于完整覆盖，后续仍需定义分页移动、补采、保留期和缺口处理。[日志结构及管理查询](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/log.go#L54)

固定版本的请求中间件生成服务端请求 ID，不沿用客户端头；前 23 位来自 UTC 时间与纳秒格式。它可作为后续撤错/重置时识别旧请求的候选依据，但需要验证版本、格式和时钟异常；它本身不是结束或覆盖证明。[请求 ID 中间件](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/middleware/request-id.go)、[时间及 ID 生成](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/common/utils.go)

GIN 格式直接包含路径等自由文本，本轮插入测试进一步说明：即使正则能识别外观正确的结束行，也不能证明它来自真实请求结束。[原生日志格式](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/middleware/logger.go)

这次核验限定于上述固定版本及实际检查的日志路径，不作“所有 NewAPI 版本、所有可用接口都没有终态能力”的扩大结论。

## 后续方案与待确认事项

业务规则保持不变：连续三个独立请求的来源最终不可用才触发规则 1，同源最终成功优先，其他源成功不清本源失败；token 上限仍是软上限，人工只撤销错误原因。GLM 策略不变。

| 路径 | 能解决什么 | 代价和限制 |
| --- | --- | --- |
| 先细化统一消费账本和模型统计 | 多来源共享计量、整体/per-key 图表；独立于失败终态归并 | 不能据此宣称完整 SGM-4 或火山自动调度完成，仍要解决分页与覆盖 |
| 核验原生结构化结束事件；必要时另行设计最小日志补丁 | 有机会补齐纯失败请求的可信终态 | 补丁需维护固定后端产物及升级验证，属于新增范围，需用户确认；本轮未修改 NewAPI |
| 将规则改成尝试级错误，或先计错后撤销 | 可以更早响应错误 | 改变已确认语义；可能在同源成功前提前耗尽，不采用为默认方案 |
| 解析控制台结束行或等待若干秒 | 无法排除伪造文本或长流式请求 | 不满足当前契约，不采用 |

建议先完成不依赖终态的消费账本细化，同时对“可靠结构化结束事件”单独确认范围；若需要修改 NewAPI，先出最小方案，不增加推理代理、不改原生路由。未经确认不以补丁、全局禁用重试或弱化规则作为实现前提。

另一个可能的采集选项是对本地托管 SQLite 做只读 ID 游标查询，它可帮助取得稳定读取快照，但不能补出不存在的结束事件。当前架构选定管理 API；改成直读原生数据库属于架构调整，必须先确认，本轮没有实施，也没有新增数据库身份登记。

下一次进入编码前，按 workflow 补齐所选小步的函数级设计、十节 expectations、PBT 不变量、机械门禁及独审范围。当前仅新增可复现探针与调研记录，没有改 Rust、DDL、运行入口或路由许可。

## 2026-10-17 B 方案的源码复核补记

本轮只复核同一固定提交的公开源码，未重新运行原生探针或访问运行实例；不把源码推导写成新的实测结果。

- 管理日志使用 `p/page_size`，页大小上限为 100；offset 为 `(page−1)×page_size`，返回 page/page_size/total/items。[分页实现](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/common/page_info.go)
- 所选 handler 接收时间、类型等筛选，未读取 ID 游标或快照参数；不能给它添加 since_id 后假定有效。[handler](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/log.go#L11)
- 时间两端包含，0 表示不启用相应过滤；Count 与 Find 分开。SQLite 管理列表保留原生 ID，但用户/key 视图和 ClickHouse 分支会分配展示 ID。type=0 不按类型过滤，适合一次全局采集再由 A 筛选。[日志查询与展示处理](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/log.go#L103)

由这些实现推导：固定时间窗和持久去重可以处理重复，但 offset 变化、同计数替换或较晚写入仍不能凭“扫到末页/重复两遍相同”证明完整。catalog_instance_id 也不是原生日志数据库身份凭证；不一致换库/回滚须保持为明确的恢复边界，不能靠另造本地 UUID 宣称解决。

下一小步形成 [B 总体方案与 B1 事务细化](../design/source-groups-token-management/subplans/sgm-4-facts-analytics-b.md)。B1 待确认，B2 原生多页/时间边界及可控变化场景仍需补验证；没有据此改变已确认的火山规则或切到直读 NewAPI 数据库。
