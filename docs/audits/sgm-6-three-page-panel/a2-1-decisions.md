# SGM6 A2.1 设计审核处置

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
