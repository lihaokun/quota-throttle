# 火山观测型熔断与成功 token 清零

日期：2026-10-17。用户已确认改用观测型熔断，并明确“观测到成功有 token 流后清空计数”。此决定替代此前要求证明每个请求最终失败的前提，不修改 NewAPI，不增加推理代理，不影响 GLM 策略。开发沿现有分支连续推进；运行部署仍不在授权范围。

## 1 业务规则与取舍

规则作用于 SourceId，而非成员渠道、组或下游协议。同一来源跨组共享失败计数；同一请求在不同来源分别计数。默认三个不同请求的明确上游不可用观测触发错误耗尽，同请求同源的重试只增加一次。

同源出现有实际 token 的成功消费观测，连续失败计数立即清零；不能让第三个错误接在此前两次失败和一次成功之后。成功还可以覆盖同请求已经观测到的失败；已经有成功观测的同请求，迟到错误不再次计数。另一来源的成功不清本源计数。重放同一成功也不能清掉其后的新失败。

失败计数与耗尽原因分开：成功清计数，包括已经熔断时的计数；不自动解除已锁存的 error_exhausted，不清 token 上限原因或手动暂停。错误原因仍由人工撤销或日重置解除。旧 period/error_epoch/cutoff 的无效观测沿既有状态机排除，不用迟到旧成功抹掉新一轮失败。

这是观测防抖，不是请求终态保证。先见到第三次错误、稍后才见到同源成功时，可能已经触发熔断；后续成功清计数，但不自动撤销已锁存原因。采集不到的实时流式 chunk 不在观察范围，不承诺首个 chunk 到达即更新面板。

## 2 本步范围与后续接线

O1–O3 先提供统一 ledger 中的分类、单请求观测合并和给既有 VolcanoRuntime 的内部结果类型；没有另一套计数器或内存全局账本。既有状态机的 Success 分支已清零计数，不重写该算法。

O4 后续 catalog/worker 接线须在现有应用库保存请求去重进度、观测处理位置，并与 Runtime、周期历史、来源 gate、各组目标和许可同事务发布；重启不能丢失去重状态。必须复用 B1/B2 的唯一事实采集，处理分页、补采、晚到归属和人工撤错；不能用本步无 IO 测试冒充持久或运行接线已完成。访问 key 管理和三页面不再被“必须修改 NewAPI”阻塞。

本步不改 v1–v5 DDL、NativeFact/消费计量、采集、管理 HTTP、CLI、UI、GLM、组选择和旧状态存储格式。旧内部 FinalOutcome 改称 OutcomeObservation，completed_at 改称 observed_at，防止名字继续宣称终态；只有内部 Rust 类型及其调用夹具机械更名，不引入兼容别名或重复枚举。OutcomeKind 和 VolcanoEvidence 继续复用。

## 3 分类边界

输入为已通过 A1/A3 校验的 AttributedFact；入口仍自行调用 validate_attribution，坏形状报固定 FactError。要求已知来源、有效 request_id；未知身份不凑数，不以 log_id 伪造 request_id，消费计量不受本步是否分类的影响。可信 catalog 调用方必须同时核验来源为 Volcano、上游适配类型属于已验证路径（当前为 Custom/type 8）；不从模型名、组名或下游协议猜测。AttributedFact 不含 provider/channel_type，因此此适配范围门禁属于 O4 调用方义务，不冒称本纯函数已检查这些字段。

| 原生日志 | 观测结果 |
| --- | --- |
| Consume，input+output>0，非流式，元数据缺省或合法且无显式失败 | Success（有 token 的成功活动） |
| Consume，input+output>0，流式且 stream 为 Done/Eof/ClientGone，无其他明确失败标签 | Success；已有 token 活动说明来源实际提供过输出，不把 EOF/客户端主动取消当来源失败，也不宣称请求完整成功 |
| Consume，0+0；或流式状态缺失/Other；或元数据 Unusable | 不输出健康/失败观测，仍正常计量 |
| Error，Parsed 元数据，error_type=openai_error，HTTP 401/429/500/502/503/504 | Unavailable；已绑定来源的上游错误，不是管理接口错误 |
| Error，Parsed 元数据，openai_error，HTTP 403 且 code 为 invalid_api_key/authentication_error/permission_denied | Unavailable；其他 403 不猜供应商原因 |
| 参数/上下文/内容拒绝/取消等排除 code，任意状态码 | 不输出；排除项优先于状态码 |
| 其余错误、new_api_error、缺失或损坏元数据 | 不输出，不把 unknown 变成失败或成功 |

固定排除 code：invalid_request_error、invalid_request、invalid_parameter、context_length_exceeded、content_filter、content_policy_violation、context_canceled、context_cancelled、client_gone、request_cancelled。字符串精确匹配，不解析自由文本、大小写折叠或猜不同供应商报文。401/429/5xx 的当前 Custom/OpenAI 适配路径来自已核验固定版本；隔离探针的 503 样本实际为 openai_error/unavailable。尚未验证的其他适配类型保持不分类，后续扩展需独立样本。

成功的合法元数据不得含非空 error_type/error_code 或 status_code>=400；非流式也不得携带矛盾流式状态。流式 Eof/ClientGone 的 token 活动只是清防抖计数的依据，不改变既有消费统计口径或取消分类。token 指 input+output，不计算缓存倍率/积分，不要求 is_stream=true；因此普通响应同样能清零。

## 4 私有类型

- ObservationCandidate：source_id、request_id、occurred_at、kind；request_id 保留规范身份，kind 只会是 Success/Unavailable。无 Debug/Display/serde。
- RequestObservationState：source_id、request_id、first_event_at、phase。phase 为 Failed 或 TokenFlow；不存在行表示尚无有效观测，不另存 Unseen。无 Debug/Display/serde。
- ObservationDecision：next 状态及可空 OutcomeObservation。无公开序列化或日志特征。
- OutcomeObservation：first_event_at、observed_at、kind；时间是已观测日志的时刻，不是证明请求开始/结束的时间。既有 VolcanoEvidence 的批次/代次/游标契约保留，outcomes 是去重后的观测转移。

身份为正 i64、request_id 沿 A 的格式/64 字节上限；时间为 0..=JSON_SAFE。first_event_at 是已见有效事件的最早时间，必须≤本次 emitted.observed_at。源和请求身份由状态携带并校验，调用方不能把来源 A 的历史传给 B。

## 5 函数规约与论证

`classify_observation(fact) -> Result<Option<ObservationCandidate>>`（O1）：前置为可信调用方已经核验 §3 的来源/适配范围；先核规范事实与归属，未知 source/request 返回 None；依 §3 白名单分类。只在有有效种类时复制有界 request_id；不读取正文、不访问时钟/网络/数据库。不知道提供者或适配类型的调用方不得使用结果签发策略许可。

论证：A 校验保证数字和元数据形状，白名单按消费与错误分支互斥；排除项先于 HTTP 状态，避免把下游/参数问题算作来源不可用。未分类返回 None 而非 Success，使未知不会清已有失败；正 token 条件排除空消费。

`observe_request(previous: Option<&RequestObservationState>, candidate) -> Result<ObservationDecision>`（O2）：校验 candidate 数字/请求 ID/kind，校验 previous 数字/身份/phase；身份不匹配返回 InvalidAttribution，输入不变。取最早有效事件时间。首次 Unavailable→Failed 并输出 Unavailable；Failed+Unavailable 不输出；首次 Success 或 Failed+Success→TokenFlow 并输出 Success；TokenFlow 后任意重复成功/迟到错误均不输出。返回 next；调用方仅在持久事务提交后保存它，不能先推进内存去重状态。

论证：每个状态的 phase 只能从不存在到 Failed/TokenFlow 或 Failed→TokenFlow，不能回退。故每个 request/source 最多输出一次失败与一次成功；重复错误不加次数、重复成功不反复清零；不同 request/source 不能复用状态。先检查、后构造使失败原子。只有有界字符串及常数次分支，无全局 map、无清理/驱逐策略。

`apply_outcome` / `apply_evidence`（O3）：参数类型机械改为 OutcomeObservation，校验 first_event_at≤observed_at，既有 period/error_epoch/cutoff/coverage/游标防重放不变。Success 清零 consecutive_failures，Unavailable 增加并按阈值锁存，Ignored 保持；成功不解除两种耗尽或暂停。批次最多 MAX_POLICY_OUTCOMES，旧 caller 的纯夹具同步名称，不放宽任何原断言。

论证：已有 B 状态机证明继续成立，仅输入语义从最终结果改为受信任且去重的观测转移。成功清零与错误锁存写不同字段，因此同时满足用户的新条件和原原因隔离规则。O4 未落地前不对生产开放该内部接口。

## 6 验证与退出

确定性测试：实际探针形状；非流式/Done/Eof/ClientGone 正 token；零 token/未知/显式失败不清；403 白名单、排除 code 优先；F/F/S/F 的计数为 1/2/0/1；三失败锁存后成功变为 count=0/error_exhausted=true；同请求多渠道/跨组去重、其他源隔离、重复成功不能清新失败；损坏/跨身份输入失败原子、负 trait。

至少 3 组各 128 cases PBT：分类与独立决策表；跨 source/request 的重复/乱序观测合并；合并后实际 Runtime 与独立计数/锁存参考逐步对照。参考不调用生产分类/合并 helper。保存随机失败，不削弱原状态机/GLM 测试。

机械门禁冻结 DDL、依赖、collector/worker/GLM/CLI/NewAPI；核 Step O1/O2/O3、共享枚举、私有 trait、精确测试清单和普通测试不全 ignored。独立串行 release 性能：连续处理 100000 个小事实的分类/单请求合并≤5秒；正常测试不加墙钟门槛。至少 6 种可编译变异：删正 token 条件、成功不清零、重复错误计数、重复成功再清零、跨源混用、成功误解除耗尽、参数错误误计（实际至少取六种）。默认 debug/release 全量、已有性能 runner、clippy/build及独立契约审后记录真实完成边界。

## 7 本步执行状态

O1–O3 [本地验收通过](../../../audits/sgm-4-facts-analytics/observed-breaker-audit.md)：11 项普通（3 组各 128 cases PBT）及 1 项性能，9 种变异；debug/release 各 507 passed/0 failed/16 ignored，14 项独立性能、机械与静态独审通过。十万次分类/合并 41 ms；旧状态机算法及旧测试断言仅机械更名，GLM/DDL/依赖/运行服务均不变。O4 持久、版本及调度发布仍须组合验收，没有新增在线入口。
