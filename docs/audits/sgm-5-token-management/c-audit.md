# SGM5 C 组与访问 key 服务审核

2026-10-18开始，2026-10-19完成本地验收，基线`bc8f33e`。设计/实现独审0个未关闭问题，定向、变异、最终全量、独立性能与隔离原生组合均通过，C内部服务满足退出条件。工作分支`research/source-groups-token-management`，未接现行CLI/面板或部署；代码提交在提交后回填。

## 实现边界

DB仍为v8，旧wire、DDL和依赖不变。组注册只在一致的倍率别名基线上补缺项，权限只合并签发账户所属account_group的special配置；它不是user_id级授权。已有歧义或部分失败后的歧义拒绝自动修复，不覆盖无关倍率/权限。

Group.ready只表示配置核验，不等于当前有额度或活跃渠道。完整原生组扫描排除未知启用渠道；正向key操作另查当前策略许可和新鲜实际候选，火山核auto_ban=0，GLM保留原值。引用报告包含未决创建、显式组补认和未知继承范围，恒为已知本地计数、恒不授权组删除；本批没有组删除或换类型API。

issuer与legacy独立可选，固定身份、同原生实例及同一预算；缺issuer仅阻止新签发，不回退管理员。复用原worker单写者、lane、commands、publication和run，A执行有限HTTP、B持久意图/证据；没有第二调度器、计时器、状态数据库或操作phase。所有发送gate只读原子版本/时间，不在预算锁里取Core或查SQL。prepare后的所有分支保留票据交给锁外结算/取消guard。

## 独立审核及修正

审核者先从架构/A/B独立提取十节，再审C设计及diff。编码前明确了ready引导环、部分option写、Enable写前身份/spec、自己的epoch+2、post-commit票据、超窗诊断、legacy-only、dry_run和shutdown边界，设计§4.4通过。

实现首轮发现首候选遮蔽后续可用成员、C提前查询身份破坏幂等、配置行龄误报漂移、遗漏火山auto_ban；按[修正记录](../../fixes/token-service-fix-admission-composition.md)先红后绿，四项旧实现0/4失败，修正后4/0。复核同类Create入口后，再以缺issuer历史重放的0/1红例补齐同一临界区持久owner判重，随后进入19项组合绿色结果。未放宽原GLM规则、观察TTL或backend lease。

首轮SQL计数usize不能FromSql及测试子模块路径错误均为编译错误，单独保留，不计作红例/变异捕获。引用计数改为有界u32，测试模块加显式path，断言不变。三个未使用的访问器和重复期限计算已删除，不用压警告属性保留冗余。

最终独立静态复核为0个未关闭问题。审核者未运行编译或测试；执行证据由主代理记录，不由静态通过推导。十节结论如下：

| 契约维度 | 独立静态结论 |
| --- | --- |
| 契约源 | 架构、A/B/C细化及处置一致，保留原worker和状态权威 |
| Schema | DB v8/旧wire不变，新报告为白名单且引用计数不授权删除 |
| 枚举 | 复用原owner、操作、错误和状态，不按下游协议分组 |
| 步骤 | T5C.1–6对应实现，机械入口核精确步骤/测试集合 |
| 行为 | 双别名一致、无关配置保留、普通签发及legacy兼容符合契约 |
| 时序 | 固定epoch+2、纯原子gate、取消结算和严格回读闭环；delay不解除Unknown |
| 不变量 | 六组PBT未见生产决策充当expected；假认证、过期状态、快照的证据边界已注明 |
| 资源 | 共用预算/有界候选，无第二轮询器；实测性能另验 |
| 副作用 | 未见秘密进入普通投影/持久化；旧策略、账本、DDL和运行入口有冻结检查 |
| 组合 | A能力/B事实/C准入分工一致；原生策略fixture不冒充完整运行验收 |

审核者另复核机械与变异脚本：精确45入口/3 ignored排除空跑，变异必须实际改变源码、成功编译且命中单项失败，最后恢复对照并跑正例；未发现假通过阻断。此结论只关闭静态审核，不提前关闭未完成的执行门禁。

## 测试证据

当前新增45项入口：42普通、2独立性能、1显式原生组合。6组PBT各128 cases：组map保留、冲突拒绝、引用/版本、当前owner/组/候选、交付/重开、legacy隔离。预期来自原始输入、固定状态与请求次数，不调用生产决策作oracle。

PBT5的expired参数只验证原生Expired状态；另有实际过去expires_at的历史意图/尝试回归，证明无当前路由/余额仍能查回过期创建。PBT6比较CatalogSnapshot；另用非空账本、原生token77与当前auto归属不同的轨迹比较全部旧用户表及旧非token操作行。假服务按请求头回显身份，不能充当凭据主体隔离的实证；真实双账号与原生组合单独验证。

| 检查 | 已得到的结果 |
| --- | --- |
| 原生组定向 | 8 passed/0 failed/1 ignored，含2组PBT |
| 目录/B辅助定向 | 6 passed/0 failed/1 ignored，79.42秒，含1组PBT |
| 四项组合红例 | 0 passed/4 failed，0.78秒；成功编译后断言失败 |
| 四项修正绿例 | 4 passed/0 failed，0.56秒 |
| Create追加红例 | 0 passed/1 failed，0.57秒；首次路径编译错误不计入 |
| worker组合 | 19 passed/0 failed，63.72秒，含3组PBT |
| 全部C定向 | 首轮41/0/1 ignored，45.42秒；增加火山正向门禁及强化证据栅栏后，最终基线42/0/1 ignored，43.81秒；变异还原后42/0/1 ignored，62.30秒 |
| 原生组能力探针 | 新实例4项检查通过，证实两倍率别名缓存不互相跟随及account_group权限范围 |
| 原生A+B+C组合 | 首轮79.97秒通过；最终源码复验1 passed/0 failed，82.40秒，8项组合断言通过，独立进程均已退出 |
| 最终机械 | 两份schema/枚举、6个Step/PBT、旧代码冻结与精确45入口/3 ignored通过；测试清理后再次通过 |
| 性能runner自检 | 首轮8项通过；补旧计时隔离后12项通过，最终清单为23项release及大组单项debug |
| 变异 | 独立副本14类全部成功编译且被对应单项断言捕获；源码还原与工作树/初始副本对照通过，最后42项正例重新通过 |
| clippy/release build | 清理后clippy通过，保留基线7条及独审接受的参数数量1条提醒；release build通过，48.94秒 |
| 首轮默认debug | 644 passed/1 failed/25 ignored，1261.87秒；唯一失败为未改动的control_tests.rs:791原5秒断言，不是业务值断言；完整失败日志保留 |
| 原阈值单项诊断 | 三次均1 passed/0 failed，完整case耗时1.10/1.13/1.17秒 |
| 第二轮默认debug | 再次观察同一旧用例失败后中止自己的重复测试，保留未完成日志；不能作为完整结果 |
| 计时隔离后的最终机械/clippy/build | 精确45个C入口及旧case/预算对照通过，runner12项通过；clippy16.16秒、原7+接受1提醒，release build0.34秒 |
| 修正后默认debug | 645 passed/0 failed/26 ignored，1194.94秒；默认并发，原普通容量测试未忽略 |
| 最终默认release | 645 passed/0 failed/26 ignored，957.10秒；编译7分08秒，默认测试并发 |
| 最终独立性能 | release23 passed/0 failed，57.99秒；大组debug单项1 passed/0 failed，1.12秒 |
| 本批性能实测 | 万项组配置24ms、万身份引用2ms；旧大组操作release103ms/debug214ms，原5秒均保留 |

14类变异分别为：跳过倍率别名一致性、丢弃无关权限、ready不提升版本、漏未决创建引用、delay只比ID不比完整证据、取消记NotSent、最终gate接受新epoch、复制跳过后置检查、Enable跳过固定spec检查、只取首候选、放行火山auto_ban、先查owner再判重、Create重放改用当前owner、允许分离管理预算。每轮均有一项实际运行的失败断言；没有将编译失败、空跑或ignored算入。

收尾独立的[旧计时门禁修正](../../fixes/catalog-fix-group-operation-performance-gate.md)只改测试编排；C生产源码未再变。原普通容量用例继续执行全部功能断言，独立性能从22增至23项并额外保留该项debug预算。runner自检12项通过，设计与实现独审均0未关闭；两个wrapper精确文本及旧case归一化字节对照防止改变原fixture/预期。其零预算负向成功编译后0/1断言失败，源码恢复对照及正例通过，原5秒实测227/252ms；另计1类测试门禁变异，不计入C的14类。

原生组合使用固定rc.20/SHA256、新临时库/账号/假上游和随机端口。真实本地预检核PID、工作目录、监听、版本及恢复配置；组从禁用成员配置核验后，经原协调器启用。普通key完成持久创建查回、同key OpenAI/Anthropic、指定渠道后缀拒绝、整组停用不越组、启停/删除回读、历史引用保留。策略许可为受控依赖fixture，不调用真实GLM额度、不替代SGM7全系统验收；A前批15项原生检查没有冒称本批重新全部执行。

新性能预算只覆盖：9999项原生组配置的值解析并补足到10000项，以及10000条imported身份、无token操作历史的引用查询，各独立release≤5秒；不含网络或打开库的历史校验，不宣称任意历史规模有相同上界。

## 复现与安全范围

机械入口`scripts/contract_audit/sgm5_c.sh`；默认debug/release `cargo test --locked --offline`；独立性能`scripts/tests/performance-gates.sh`。原生脚本`scripts/probes/newapi_groups_probe.py`与`newapi_token_service_probe.py`均只读指定已核验二进制，启动新私有实例，拒绝3000/3001，最后仅停止自己进程。

最终日志前缀`/tmp/quota-throttle-sgm5-c-complete-`，顺序验证脚本退出0；初轮失败和中止日志分别保留为`-final-debug.log`和`-final-debug-recheck.log`。原生组合首轮目录`/tmp/quota-throttle-sgm5-c-combination-vdyq87tf`、最终目录`/tmp/quota-throttle-sgm5-c-combination-5aoqfiww`；C变异副本`/tmp/quota-throttle-sgm5-c-mutants.lHQZMw`、旧计时负向副本`/tmp/quota-throttle-sgm5-c-control-gate.FPFH1S`。临时证据可能被系统清理；源码、断言与脚本随提交保留。测试未读取生产配置/DB/日志/key，未请求真实供应商、重启服务、推送、合并或部署。仅只读使用已核验的现有二进制文件。

原生脚本生成的两份未跟踪pyc已移到`/tmp/quota-throttle-sgm5-c-probe-cache.CrDtcm`保留，不作为源码提交；提交前范围/schema/入口机械复验通过。最终执行证据经独立审核者只读核对，与报告一致。

## 五维度及未完成范围

- 一致性：A/B/C职责和既有wire/schema保持；组配置事实与实时发送资格分开，历史创建事实不当当前可用。
- 风格：复用现有发布/持久操作/传输，不新增恢复服务；脚本与测试延续已有模式。
- 正确性：幂等优先、当前票据、纯原子gate、取消保守结算、严格查回和敏感返回后置检查；最终全量、14类C变异及独立原生组合通过。
- 性能：同预算、有界候选、无每key后台轮询；23项release及大组debug独立预算通过。60秒backend与30秒正向窗口保留，大目录可能需重试，不保证容量上限下的单轮活性。
- 可维护性：来源策略/账本/旧HTTP冻结，局部辅助函数有契约与回归；仅补遗漏的旧测试隔离，不扩大业务修改。前端、公开HTTP及运行/部署仍未完成。

保留B的未知状态写边界：丢失可信终止证据可永久Unknown，不能以匹配GET强行解锁。倍率别名歧义也不自动覆盖修复。默认26项ignored中23项性能及本批原生组合已单独执行；两个既有opt-in（旧原生组合/百万历史趋势）本批未重跑。所有结果是本地验收，未触发远端CI。
