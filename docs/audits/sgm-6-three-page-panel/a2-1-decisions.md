# SGM6 A2.1 审核处置

2026-10-19，基线`ea7cf33`。本轮仅设计与schema，生产代码、旧HTTP、CLI和在线服务未改；PBT/变异/全量为实现阶段门禁，不能用设计审核代替实跑。

## 已确认范围

1. 独立审核先从架构/三页/A1提取A2十节预期，再查既有worker入口，未从新实现倒推契约。A2.1只交付HTTP边界、单一状态发布和GET state；A2.2才接目录、控制、token、模型和用量，需另写逐路由wire。
2. 回环/固定authority及Origin、自定义头、JSON和无CORS共同保护本地API。没有nonce/session/login表；明确这不是对本机恶意用户的身份认证。不同端口SSH转发必须显式配置唯一browser origin，不能信任请求反推。
3. 保留A1原JSON和原时间，新增年龄header只用于展示。current/backend_ready描述采集时刻，B只允许降级旧数据，不借bool授权写入或续期。缓存过期/时钟异常清除entry，防止前跳后回拨复活旧数据。

## 独审问题与修订

| 问题 | 处置 |
| --- | --- |
| 独立publication父任务异常后会丢失blocking句柄，无法兑现实际回收 | 删除该父task和第二publisher槽；唯一serve supervisor直接持有ActivePublication直到join。timeout只标expired/清cache；正常shutdown回收，强制abort例外明确 |
| raw query没有区分无问号/空问号，ASCII范围含控制字节 | 固定Option<String>的None/Some("")；整个target先拒SP/CTL/DEL/nonASCII，query仅可见ASCII |
| 十节清单第10节被退出描述替代 | 恢复跨实现一致性，退出单列；未来浏览器验证明确交B，不伪造已完成 |
| 未解析请求的满槽503正文可能违反HEAD承诺 | 满槽直接关闭，不读请求、不发送正文、不创建等待任务 |
| state方法错误缺精确Allow契约 | 已知state的405带Allow: GET；HEAD不发送body |
| 临期ID总量的容量估算缺前提 | 明确依赖A1每membership仅一个组、临期仅本组成员；最坏容量夹具限真实A1合法域 |
| connect及客户端重试责任含混 | server无主动connect，timeout N/A；服务端0次重试，客户端次数/退避由B细化 |

请求framing、统一cache、32MiB完整响应和A2.1/A2.2拆分未引入新业务状态、写库连接或任务表。独立§4.4复审已通过，三个设计问题及四项建议全部关闭；目标映射、函数细化、六条完整性、正确性论证、接口一致性与可实施性均通过。这只是设计准入，不是实现验收。

静态校验：错误schema的metaschema验证、15个合法枚举样例与6个非法结构/值样例通过；文档diff检查通过，src/Cargo/scripts相对基线零修改。本轮没有运行应用、PBT、变异、性能或浏览器测试，也未连接fermat/在线服务。后续按本契约实施A2.1并完成全部动态门禁。

## 实现阶段处置

以下为用户确认设计后的实现阶段，基线`80ba157`；上文“本轮仅设计”保留为历史记录，不代表当前仍未实现。最终证据集中于[实现审核](a2-1-audit.md)。

1. **Ready截止漏洞修复**：独审发现timeout_at先poll Ready future、全部缓冲body无timer及body起点偏后。先写八节[RCA](../../fixes/panel-http-fix-ready-deadline.md)，三个新增回归成功编译后全红，再实施显式IO边界检查和head完成时刻传递。保留原预算；不把超时当抢占内核调用，也不声称能撤回已发送字节。
2. **测试错误与生产缺陷分开**：首轮慢writer的16字节缓冲误配17字节期望，只修字面量，原大小/时限断言保留。它不算生产deadline漏洞的红例。测试中的刻意panic用于验证poison与blocking失败，不是未预期失败。
3. **容量证明补齐**：最大关联目录分别使用GLM/火山/混合类型，保持成员与组及来源类型一致。独立字段宽度包络再覆盖全部可选状态、最长枚举和数值宽度，允许互斥最大值保守叠加但不能充当业务快照；性能仅宣称固定三夹具，不声称穷尽所有状态。
4. **验收oracle补强**：独审发现常量自引用和整除分片的两个盲点；现有入口加入固定预算字面断言，并增加3000字节分片真正跨越head上限。35个测试入口不变，完整验收使用补强后源码；未改生产逻辑来迎合测试。
5. **保持职责范围**：main只有mod声明，没有运行入口。新HTTP仅GET state，A2.2业务调用、B页面与SGM7组合仍需后续细化/实现；不新增业务状态、独立writer或新调度器，不改GLM/火山策略。

独审最终静态结论0个未关闭问题，动态证据另行核验。最终机械、12种变异及还原重编、默认debug/release各696/0/32 ignored、29release+4debug性能、clippy/build均通过；确切耗时、证据路径、失败记录和五维度见[实现审核](a2-1-audit.md)，不以静态通过替代动态结果。
