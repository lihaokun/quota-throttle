# SGM6 A2.1 实现审核

2026-10-19，分支`research/source-groups-token-management`，实现基线`80ba157`。范围仅HTTP边界、A1统一状态发布及GET state；实现、机械、PBT、变异、最终全量与性能门禁全部通过。A2.1完成不代表整个SGM6或部署已完成。

实现提交`66037e8`；仅本地提交，未推送、合并或部署。

## 契约与范围

依据[A2.1细化](../../design/source-groups-token-management/subplans/sgm-6-http-boundary-a2-1.md)、[事前十节](a2-1-expectations.md)和workflow§5/6，新增`panel_http`，main只加模块声明。旧HTTP、worker、来源策略、目录DDL、Cargo及A1 schema均冻结；没有CLI接线、管理写路由、真实浏览器或运行服务变更。

请求解析、同源校验、单连接响应、bounded编码、cache和publication各自有明确边界。GET只取已发布Arc bytes；仅唯一supervisor调用A1，实际blocking任务结束前不释放active。没有第二调度器、写库连接、后台任务表或请求驱动探针。

## 独立审核与处置

独立审核者`sgm6_a2_contract_audit`先读契约抽出预期，再审增量代码；最终静态复审0个未关闭问题。动态结果由主执行者留存，独审不冒充亲自执行测试。

| 问题 | 处置与证据 |
| --- | --- |
| Ready IO可先于timeout计时器返回，已缓冲body绕过截止；body期限从authorize之后重起 | 先写[根因方案](../../fixes/panel-http-fix-ready-deadline.md)，3条红例全部失败，再加显式读写截止和head完成Instant。原回归保留 |
| 大目录夹具不足以证明全部字段最大宽度 | 合法GLM/火山/混合最大关联目录加独立字段宽度包络；后者不是合法业务状态或性能输入 |
| 固定预算测试引用同一实现常量，常量退化可能存活 | 现有3个入口直接断言16连接、各字节/字段上限及5/10/5/45秒、5秒周期、10秒TTL；不改35入口集合 |
| 超限head分片恰好先撞16384，未覆盖同一read跨界带delimiter | 现有用例增加3000字节分片，实际从15000跨界到带delimiter的末包；同时关闭两道防线的变异被3000分片断言捕获 |
| 慢writer夹具期望17字节却只允许16字节缓冲 | 只修正错误字面量，保留缓冲大小、len和时限断言；不改被测对象掩盖失败 |

完整处置见[a2-1-decisions](a2-1-decisions.md)。

## 机械与测试

精确清单35入口：33普通、2独立性能；4组各128 cases PBT涵盖消息分片、非法头组合、cache参考轨迹和安全编码。四个oracle从输入/模型推导，不以被测函数回填预期。

`scripts/contract_audit/sgm6_a21.sh`核对核心冻结、main单行模块声明、两性能入口增量、H1.1–H1.7、35/2精确集合、4PBT、实际worker输出schema、15错误code/状态、6错误结构负例与3快照负例。现有性能runner回归24项通过；完整性能和变异单独执行，不以空测试/编译失败算捕获。

初始定向25 passed/1 failed/2 ignored（慢writer夹具）；Ready红例0/3；修复后33 passed/0 failed/2 ignored，3.86秒。这些日志保留，后续结果不覆盖原失败记录。

最终机械检查通过，日志`/tmp/quota-throttle-sgm6-a21-final-mechanical.log`；等待artifact锁后复用恢复二进制，不把它宣称为第二次编译。首轮完整debug为696 passed/0 failed/32 ignored、706.19秒，运行前尚未补固定预算/3000分片，故仍对补强后的完整debug另行复验。

12类变异全部成功编译，各自只运行指定的1个测试，均0 passed/1 failed/0 ignored。临时目录`/tmp/quota-throttle-sgm6-a21-mutations.tXfGkB`保留逐项日志和`runner.sh`；runner的SHA256为`666864086d645443fc6fc11e8575cc0d11acca04cd9892b157c5b121e3c12f0f`。变异范围如下：

| 编号 | 注入的错误 | 捕获证据 |
| --- | --- | --- |
| M1–M2 | 放过重复头、非法Content-Length当0 | 严格head用例拒绝Host/CL歧义及非法长度 |
| M3–M4 | EOF当完整body、忽略Origin | 不完整读取及独立同源门禁断言 |
| M5 | 同时删head终止位置及纯parser的两个上限防线 | 3000字节分片实际跨界被错误接受 |
| M6 | 完成时刻重置publication起点 | 原start+10秒仍可读，违反TTL |
| M7 | 超时丢弃仍运行的active | 实际producer调用2次而应为1；不是只检查返回码 |
| M8–M9 | 发布显式expired任务、不清时钟失效entry | 迟到状态发布及回拨复活断言 |
| M10 | 去除编码writer上限 | 超限write错误成功 |
| M11–M12 | 去除Ready读取/发送前的显式截止检查 | 到期head仍成功、到期连接开始发响应 |

每轮还原后显式touch，防止mtime造成旧变异二进制复用。最终`restored.log`明确重编译56.19秒，随后33 passed/0 failed/2 ignored、3.09秒；整个src树及Cargo.toml/lock与工作树逐字一致。独审已直接复核12条编译/断言日志、恢复重编、全树比较及最终机械日志，0个未关闭问题。

最终证据目录为本机`/tmp/quota-throttle-sgm6-a21-evidence.gY748f`，远端为`/tmp/quota-throttle-sgm6-a21.fOECri`。最终debug重新编译53.67秒，release重新编译2分24秒；两轮都包含固定预算断言和3000分片。两份7文件SHA256清单与当前工作树一致。

| 最终门禁 | 结果 |
| --- | --- |
| 默认debug全量 | 696 passed/0 failed/32 ignored，802.29秒 |
| 默认release全量 | 696 passed/0 failed/32 ignored，642.88秒 |
| 29个release性能 | 29 passed/0 failed/0 ignored，32.64秒；新编码80/64/99ms，body按整毫秒打印为0ms，不等于零耗时 |
| 4个debug性能入口 | 各1 passed/0 failed/0 ignored；控制170ms、导入328ms/快照15ms、组快照43ms、GLM纯容量21ms，原预算不变 |
| 最终远端clippy all-targets | 12.50秒通过；8条既有生产、2条既有测试警告，无A2.1新增警告 |
| 最终release build | 0.28秒通过 |

完整回归不指定测试线程数；CARGO_BUILD_JOBS只限制编译，不降低默认测试并发。本地最终clippy也通过42.31秒，该工具链为8条既有生产和1条既有A1测试风格警告，不混作远端警告数量。32个ignored中29个性能入口已另行执行；3个原有opt-in（大历史SQL趋势、原生渠道契约、原生令牌组合）本批不重跑。没有把假TCP服务称作浏览器交互或真实双协议验收。

一次编排误向无参数的性能runner传入`release`，runner按原契约打印Usage并exit 2，未执行任何性能测试；该调用不算门禁通过或性能失败，也没有修改runner放行。随后使用原无参数入口完整串行执行29个release和4个debug门禁，结果见上表。

远端最终构建日志完成且确认该隔离目录无在跑命令后，本机长SSH客户端仍等待；仅关闭本次客户端，客户端返回255不作为测试结果。各门禁以完整结束日志和独立复核为据，没有停止任何运行服务。

## 容量证据

三类最大关联目录均有1000来源/100组/10000成员，来源类型、组类型、成员唯一性与临期ID归属合法；最长转义文本及长数值已覆盖。编码分别20,179,527、19,283,229、19,731,377字节。

独立字段包络覆盖退休、固定/释放、最长枚举、负优先级和可选结构；source/member/group/root为15152/726/1423/295字节，分别小于16KiB/1KiB/2KiB/64KiB预算。依赖每membership只属一组，临期ID总量最多10000；保守总上界27,114,336字节小于32MiB。互斥字段最大值叠加只是证明上界，不作schema正例或运行快照。

2秒编码、1秒body预算不变且通过。性能固定三种最大目录及指定body分块，不能扩称所有合法状态组合的耗时证明。

## 五维度审核与剩余范围

一致性：schema、七步、精确清单及核心冻结一致。风格：格式检查及lint通过，无新依赖。正确性：基本/边界、4组PBT、最终全量、12变异及恢复重编均通过，红例保留。性能：容量上界与独立预算通过，请求只clone Arc，不重复查询或编码。可维护性：解析、发布和服务职责分开，唯一supervisor持实际任务，没有第二业务写者或调度器。明确有序shutdown才保证drain，强制abort不宣称回收不可抢占系统调用；capture时状态不是当前写授权。

独立审核者已逐项核对最终全量、35入口、29release/4debug性能名称、12条变异及恢复日志、机械与源码哈希，最终0 critical/0 unresolved，五维度全部通过。最终build为成功构建检查，不虚构为再次编译；真正的恢复重编证据是`restored.log`。

A2.2还需逐路由wire、业务错误映射和worker调用的有界生命周期细化；之后才有B/C页面和SGM7运行组合。fermat本轮仅私有临时目录测试，正式部署仍是全新初始化；未访问真实key、配置或线上DB，未重启、推送、合并或部署。
