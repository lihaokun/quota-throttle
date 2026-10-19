# SGM7 运行交付验收

2026-10-19，基线ab59a88，分支research/source-groups-token-management。本报告区分静态审查、隔离测试和实际部署；未完成项不标通过。

## 范围与冻结

一个新serve入口组合原worker/统一采集/三页面，旧up/run及本地现网不变。Catalog/DDL/wire、GLM/火山算法和依赖锁不改。测试及部署目标为fermat的全新私有目录，不读取或复制现网配置、key、数据库，不操作原3000/3001。

本地与fermat的src、Cargo.toml、Cargo.lock、config.example.toml规范化tar SHA256一致：`ca80e7e087c55cefc15d3cd3f9831eece072fff6dfc2087482a6a02d708fa95a`。后续C夹具/文档修正未改Rust源码。fermat测试目录为`/tmp/quota-throttle-sgm7-empJic`；Rust/Cargo1.97.1，Node22.22.1，原生固定rc.20 SHA256为`90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205`。

## 自动检查与测试

| 门禁 | 实际结果与证据 |
| --- | --- |
| default debug | 768通过/0失败/35独立入口项，523.19秒；full-debug-tests.log |
| default release | 768通过/0失败/35独立入口项，318.52秒；full-release-tests.log |
| 归档PBT种子 | 首次源码白名单未包含5个已跟踪种子文件，后补齐并对全部对应14项用同一冻结debug/release二进制重放，分别14/0（180.69秒）、14/0（83.31秒）；regression-seeds.log |
| 新runtime/协调回归 | 精确清单25项（含1项原planner），三组128-case PBT；group-token-cooperation-tests.log，完整debug覆盖 |
| 机械冻结 | 本地sgm7_check.py --freeze通过，R1–R5/共享类型预算/精确可运行清单/旧核心零diff；仅核C声明清单，不替代C执行 |
| initializer / C清理夹具 | fermat各10/0；initializer-tests.log、probe-harness-tests.log |
| Node | 52/0，原8组PBT保留；node-contract-tests.log |
| B真实浏览器 | Chromium/Firefox×桌面/移动共72/72；browser-suite.log；API fixture套件不是完整原生C |
| full Schema | 3316实际合成报文、18请求+13响应、60根反例、28错误码；2000实际响应过前端；32 UI生成报文及URI反例；wire.qHCjDV/uiwire.YKP7YV |
| 原生渠道 | 默认ignored显式运行1/0，65.58秒；native-channel-contract.log |
| 原生token A+B+C | 同Key两协议、普通角色隔离、启停删除/历史8项检查通过并停止自建进程；native-token-contract.log |
| 性能runner回归 | 24/0；performance-runner-tests.log |
| 独立性能/容量趋势 | 31项release全部通过（34.60秒）及4项debug全部通过；20万/100万历史趋势1测试通过（103.65秒）；UI Node两项231.98/1000ms、471.23/500ms；四浏览器性能4/4，145.43秒，原预算不变 |
| clippy | 默认门禁退出0；当前Rust1.97.1共12个warning，其中本批2个仅测试布局/独立PBT比较表达式的风格提示，其余原逻辑；未声称-D warnings通过 |
| SGM6 C真实全组合 | 最终attempt12完整15/15及四浏览器4/4，scope=full、apiMocks=false、success=true，controller/native均实际停止；e2e-attempt-12.log及/tmp/quota-throttle-sgm7-combination-r8dr5yie/report.json |

35项默认ignored不等于跳过验收：31项独立release性能，另外原生渠道、token组合、浏览器fixture、历史容量趋势各有显式入口；4项原debug性能也单独保留。

## 有效编译变异

独立源码/target：`/tmp/quota-throttle-sgm7-mutations.r8MQ5k`。四案都真实重新编译，精确1测试失败，无编译错误或空跑：允许旧keys（Config拒绝消失）；允许原生亲和（settings_ready正例失败）；新空账本当Storage（Err(Storage)≠Ok）；注册期间允许自动火山发布（预期QueueFull却收到Ok，Frame随后失效）。原始源码恢复、逐文件diff一致、重新编译后runtime筛选26/0。日志m01–m04及restored.log保留。

## 失败/中断轨迹保留

全部位于fermat临时目录，不以失败轮冒称全量通过：attempt1/2在普通签发账号创建失败（原生密码上界）；attempt3新空账本误判Storage；attempt4定时发布打断组注册；attempt5同类问题打断签发；attempt6夹具错误要求初始化后永久首选第一个；attempt7未把合法snapshot_unavailable识别为启动等候；attempt8用于SIGTERM清理实证，attempt9为换修订夹具主动中断。后两轮非零/probe_cancelled，控制器与原生进程均确认停止。完整最终轮另填。

attempt10已通过11项（含双协议普通/流式、隔离、账本、成功清计数、错误熔断/撤错、共享软上限），后因夹具在clear_error成功后仍读旧runtime revision而遇409。新增只读等待新revision，不重发clear；另收紧切换判据为所选成员及全部本组成员的实际priority回读/dirty收敛，而非只看选择意图。attempt11为更换该夹具主动中断，退出与清理正常；最终attempt12完整通过全部15项和4浏览器，未忽略失败、跳项或复用前轮成功片段。

## 双重审核与交付状态

设计及R1–R5、自动提交协调经独立审查，代码0 critical/0 unresolved；一致性、风格、正确性、性能资源边界、可维护性五维通过。脚本独审指出的误绿/漏项及缓存调用约束按decisions.md收紧，最终独审回执也为critical 0/unresolved 0。测试数量不能代替实际C或部署证明。

已部署release SHA256：`b60093070a43a87ad12619bc2113758fa08a1cc8e39f8d9e0563bcd882dae666`；安装前及安装后均核对一致。代码提交为`5a2ccfddcfd178863f2b41bbc72c5f2174623188`。

CI新增独立groups-runtime job，保留原Rust、性能和UI jobs；此次等价测试在fermat执行，不声称GitHub Actions已运行。未push、未merge。

## fermat 全新部署验收

2026-10-19 UTC 12:22完成实际启停恢复验证。目标`/home/lihaokun/apps/quota-throttle-sgm`，用户服务`quota-throttle-sgm.service`为enabled、active/running；用户Linger=yes。面板`127.0.0.1:13001`，NewAPI端口13000（原生实际通配监听，不冒称回环）。没有迁移旧配置/业务库，没有触碰原3000/3001服务。

实际effective unit：FragmentPath为`/home/lihaokun/.config/systemd/user/quota-throttle-sgm.service`，无drop-in，KillMode=process、TimeoutStopUSec=3min、Restart=on-failure、UMask=0077、NoNewPrivileges=yes。目录0700、配置与两数据库0600；进程UID/exe/cwd/listener、原生SQL_DSN/LOG_SQL_DSN=local、SQLITE_PATH以及ERROR_LOG_ENABLED=true均已核验。

只stop/start本次新控制器：MainPID从49666变为57601；NewAPI PID49733保持不变且停止期间继续监听13000；应用库schema8及catalog_instance_id保持。重启后backend_ready=true，首页200及CSP、ui-config调用根、来源/组/访问Key空目录再次验证。第一次只读验证脚本的CSP字符串受SSH外层引号影响而断言失败；检查到实际响应头正确后，改为本地quoted heredoc原样传Python并完整复验通过，没有因此更改服务配置或放宽CSP。

SGM7实现、完整隔离验收、独审、正式新部署及恢复门禁均完成。新服务为空真实来源/访问Key，用户尚未提供两把火山Key；真实NewAPI+模拟上游的组合通过不等于真实供应商推理已验收。后续注册用户提供的来源并以实际model list核验，不能擅取旧服务或历史聊天中的密钥。
