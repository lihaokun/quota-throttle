# SGM6 B 事前契约清单

2026-10-19，事前提取基线b3a5fe6。以下清单先依据既定架构、三页计划和A1/A2已验收契约独立抽取，不从后来代码现状回填预期。独审者先只读这些契约提供十节边界，主设计在[细化](../../design/source-groups-token-management/subplans/sgm-6-browser-panel-b.md)补固定参数。用户确认f206984后进入实施，实际证据见[B实施审核](b-implementation-audit.md)。

## 1 契约源

架构G4–G6/I1–I8、sgm-6-three-page-panel、A2总体§2–5、A2.2§3–9、既有PanelSnapshot/Operation/Token/Usage schemas与workflow§4.4/6。B是三页消费与必要静态入口；不接CLI/run，不改策略/账本/DDL/transport，不部署。

## 2 Schema和数值

所有控件列明字段来源、版本、可空语义和单位。ID以正十进制字符串保持，local/native不同；配置、成员、策略、运行revision不可混用。JSON数字逐schema核safe域，不能猜“整数都可Number”；使用字段缺失/非法不能变0。新增仅UiConfig schema1的dry_run及nullable公开NewAPI根地址，不能将管理origin当调用地址。

## 3 枚举

来源类型、原操作kind/phase、成员生命周期、观察状态、owner、扫描状态和错误码来自原schema机械生成的共享常量。未知枚举为协议错误；无Claude来源类型、火山临期或假成功phase。模型null与同名字串不同身份。

## 4 流程步骤

U1资源/config；U2请求和消费；U3单生命周期；U4年龄/投影/文本渲染；U5稳定操作与恢复；U6来源/组；U7key分页和身份；U8查询/模型图；U9敏感复制；U10启动/失败。实现须有同名Step及HTTP/JS对应映射。

## 5 行为

目录201仅保存；成员planned后再显式Ensure；check(false)也是写。持久202保留原ID/phase，丢回执继续查同ID；retry不换ID或payload。C2 succeeded/0只表示设置保存。来源按凭据、组按newapi_group、成员按source/group核对，名称不是身份。创建模型默认真实发现，成功不混fallback。新key固定issuer、单组且两协议共用；原管理员历史仍沿原owner，不暗改default。

## 6 时间和生命周期

三页仅一个document刷新所有者；切页/隐藏/恢复/BFCache不能叠加轮询或恢复过期标志。age至少header+RTT+单调经过；10秒缓存/30秒观察/reset端点沿原规则只降级，不重算策略。所有结果绑定generation/query/实体，旧响应不能覆盖新筛选或草稿。abort不证明服务端停工或未提交。原生分页的读取时刻和本地身份分别展示。

lane.owner与结果generation分开：过期ticket的finally仍归还自己的owner，不清别人的槽。state失败立即使旧观察过期，同publication成功不复活。GLM相关来源严格按A1.4集合，相关样本先到期使整组current/临期/pin_release降级，非相关缺样本不误伤。operation回执只按ticket因果，不以updated_at/phase作全序；op-only恢复和create_token未知target分别建立首回执绑定。usage滚动anchor只在启动查询时冻结，5秒state刷新不能持续废弃长查询。

## 7 PBT不变量

至少8组各128：时间只降级；任意完成顺序只发布当前代次；单pump/lane守恒；opID/载荷/phase恢复；目录类型/版本和唯一关系；分页缺失不删/local-native隔离；统计联合守恒/稳定颜色；文本及secret生命周期。独立oracle与可重放seed，不能只用静态HTML或被测投影自证。

固定反例覆盖独审提出的五类边界；模型筛选整体Any、null Unknown、字符串Exact，不能混合null与字面unknown。未初始化、初始化空统计、零token但有消费行分开，零最大值无除零；火山周期hour查询空区间/已到界不发，客户端不重置。公开根末段v1拒绝，不静默删其他路径前缀。

## 8 性能和容量

H1/H2连接/截止/四槽及原usage单槽不放宽。单浏览器state/work两lane、16个跟踪ID、100项身份/原生页、25/50项目录/明细DOM页；最多一份state/current usage。完整32MiB响应处理上限不意味着所有合法结果能返回。所有模型/unknown/零桶保留，不以Top-N或删测试达标。新增Node/浏览器独立性能与原31release+4debug全部保留。

## 9 安全和副作用

静态导航与API分别定义，但固定Host/Origin/Fetch Metadata以及API X-QT-Panel/JSON写保护保持，旧API不作fallback。文本只进入textContent/value，动态值不进HTML、可执行属性或class名。上游表单视为敏感，secret只在显式reveal短时持有；无URL/日志/持久缓存，sessionStorage只存16个非敏感opID。复制拒绝不自动重取；清DOM不声称清系统剪贴板或物理内存。只用隔离fixture/随机端口，不影响run。

## 10 跨实现和退出

字段/枚举生成、UiConfig schema、旧schema原样、Step与精确非空清单、核心冻结机械核对。Rust普通回归、Node基本/PBT、真实Chromium/Firefox及性能/变异分别记录；B的API mock不冒充C的真实worker/NewAPI双协议链路。实际可执行变异至少12，恢复重编/重载及独审五维度通过才实施退出；设计审核不算实现完成。

Step 5 验证记录（不修改上述事前预期）：2026-10-19 全部门禁通过，默认 Rust debug/release 各 744/0/35，Node 51/0、8×128 PBT，浏览器 72/72，17 个有效变异与全部性能通过；253 文件最终 SHA 一致。独审 critical 0、unresolved 0，五维度通过，见[最终审核](b-audit-report.md)与[逐轮证据](b-implementation-audit.md)。C/SGM7 未完成，未操作现网。
