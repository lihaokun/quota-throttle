# SGM5 A 原生令牌接口审核

日期：2026-10-18，基线eb0b727。A原生令牌接口本地验收通过。最终重登检查发现的固定owner缺口已按[修正方案](../../fixes/newapi-token-fix-auth-principal-drift.md)完成红绿及独立静态复审；修正后全量、变异、构建和独立性能通过，0个未关闭问题。退出范围仅A，不代表完整SGM5或页面交付。

代码、测试、契约及验收记录本地提交：`6ece728`。后续文档提交只回填此实现哈希，未推送、合并或部署。

## 实现范围

固定owner的普通签发与legacy管理员读取/操作路径，复用已有NewApiClient、独立会话、共享ManagementBudget及最终发送guard。提供安全分页投影、单条查找、历史创建关联查回、单组创建、启停、单ID删除和显式复制。新创建只允许普通身份，旧空组仅显示继承账号组；不重签旧key。

新令牌创建无ID ACK且不支持禁用创建；A不自行认领或做业务重试，明确401后仅原transport允许认证恢复并重放一次。查回区分0/1/多条，唯一项必须逐字匹配关联名及固定设置。过期后仍能找回，额度负数合法。完整key对上层仅通过无Debug/Serialize的RevealedToken显式返回，不持久保存；HTTP正文/解析期间也会暂持原文。列表即使收到原文也重新打码，普通安全view不包含未知原生字段。

transport仅增加私有正ID token单条GET的严格缺失识别，以及可选expected_user_id绑定；token统一send附固定owner，每次prepare核Session ID并固定实际header，覆盖Bearer及重登。既有请求默认None且保持原路径/包络校验。token层拒绝其他2xx作为正常ACK，写前失败NotSent、写后不确定保持Uncertain；不混淆明确拒绝的ResponseReceived。未新增数据库、后台任务、登录实现或预算。

## 测试和独审

新增24项入口：23普通（3组各128 cases PBT）、1独立性能。PB1用独立字符序列构造掩码并验证身份、负额度和legacy继承组；PB2独立枚举创建payload，验证单组/跨组关闭/输入拒绝及零请求；PB3以独立状态写轨迹验证owner/ID漂移不产生写。预期值不调用生产mask或payload构造。新增账号绑定回归先复现旧实现0 passed/2 failed，再验证同账号恢复、异账号不重发及Bearer header固定。

确定性覆盖缺字段/非法页、精确缺失窄例外、0/1/多条创建匹配、过期历史、大小写反例、身份/余额/可用组拒绝、双会话不串cookie且共享预算、guard在前置读取和401后失效、实际写拒绝/超时/畸形响应、201/202、过期前最终发送、取消不重试、状态响应漂移、显式复制及错误脱敏。64槽计数依赖被保留并重跑的原transport测试，A只添加组合验证。

workflow §6独审先读架构与细化独立列十节expectations，再核对清单和diff；历史prompt模板缺失，使用正文要求。设计的过期恢复/关联名/负额度，以及实现的200边界均已关闭。[处置记录](a-decisions.md)保留原因及验证边界。

## 运行证据

| 验证 | 当前结果 |
| --- | --- |
| 编译检查 | cargo check通过，7.53秒 |
| 第一轮定向 | 19 passed / 0 failed，3.87秒；随后补两个边界用例及等价枚举装箱 |
| 修正前newapi全模块 | 106 passed / 0 failed / 2 ignored，19.87秒 |
| 修正后newapi全模块 | 108 passed / 0 failed / 2 ignored，22.98秒，覆盖原认证/预算/渠道/日志接口 |
| 原生隔离探针 | 初始两次各13项通过；扩展cookie POST create、Bearer GET self错header后15项通过；固定二进制hash，自建进程均退出 |
| 修正后机械检查 | 精确24入口、空/全ignored/漏项负例、schema/枚举/Step、旧client/transport内容保留及算法/DDL/依赖冻结通过；性能runner自检8项通过 |
| 修正后clippy | all-targets通过，20.62秒；仅6项既有警告 |
| 修正前默认debug全量 | 576 passed / 0 failed / 20 ignored，945.76秒 |
| 修正前默认release全量 | 576 passed / 0 failed / 20 ignored，676.22秒 |
| 修正前变异 | 9类捕获；还原21 passed/0 failed，2.31秒 |
| 修正前release构建 | 成功，45.44秒；开发树产物未安装或启动 |
| 修正后默认debug全量 | 578 passed / 0 failed / 20 ignored，928.76秒 |
| 修正后默认release全量 | 578 passed / 0 failed / 20 ignored，638.03秒 |
| 修正后release构建 | 成功，46.90秒；开发树产物未安装或启动 |
| 修正后变异 | 11类全部捕获；还原源码后23 passed / 0 failed，4.68秒 |
| 修正后独立性能 | 18 passed / 0 failed，43.91秒；新增最大100行令牌页解析15ms，预算2000ms |

性能在其他测试和构建全部结束后串行运行，不混入默认并发预算。原17项预算未放宽；最大GLM轮次2681ms、观测发布目录4305ms、十万条总体查询144ms、万条操作迁移293ms均通过原断言。

默认忽略的20项中18项由独立性能入口执行；另外两个既有opt-in入口（原生组合及百万历史趋势）本批不重跑。本批15项原生令牌探针是单独入口，不冒充这两个既有测试已执行。

修正后11类变异均已成功编译并由目标测试失败捕获：明文投影、管理员冒充issuer、忽略token owner、接受大小写折叠关联名、开启跨组重试、忽略最终guard、扩大缺失例外、接受202回执、过期阻断历史查回、漏Session owner检查、漏固定owner header。每项实际运行一个目标测试并出现0 passed/1 failed，编译错误不计捕获；最后逐文件核对源码已还原，并通过全部23项普通令牌测试。

复现：`scripts/contract_audit/sgm5_a.sh`、默认debug/release `cargo test --locked --offline`、`scripts/tests/performance-gates.sh`；原生入口 `scripts/probes/newapi_token_probe.py --binary <已核验二进制绝对路径>`。日志在`/tmp/quota-throttle-sgm5-a-*`，变异副本及脚本`/tmp/quota-throttle-sgm5-a-mutants.nIjopy`，原生报告和固定公开源码在`/tmp/quota-throttle-sgm5-research.KfAD2N`。临时证据可能被系统清理。

## 五维度与剩余边界

- 一致性：一个新key一个组，协议不分组；A只承认原生证据，不冒充持久业务完成。
- 风格：复用现有单实例管理预算、认证恢复、发送guard和安全错误；窄缺失例外不建立新传输框架。
- 正确性：固定身份和响应ID、严格页/字段/回执；创建无ID不认领，历史恢复允许已过期。
- 性能：固定业务请求数及有限页；无按key周期请求，100行接近8MiB解析单独测量。
- 可维护性：原算法/账本/DDL/运行层冻结；B/C的持久化与组装责任明确，未引入未使用的服务或存储表。

只在开发worktree及私有临时目录操作，没有读取真实配置/数据库/日志/key，没有请求供应商、重启、部署、推送或合并。SGM5 B/C、三页面和运行发布仍未完成。
