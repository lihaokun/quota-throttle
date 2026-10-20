# SGM6 B 实施审核记录

2026-10-19，实施基线为用户已确认的 f206984。B 已完成实施及内部验收，独立审核 critical 0、unresolved 0，五维度通过；见[最终审核](b-audit-report.md)。没有部署、重启、切换实际 run，也没有改 CLI、调度策略、账本、NewAPI 传输或 Cargo 依赖。

## U1 静态入口和界面配置

已实现编译内嵌的固定资源白名单、固定 MIME 与 HTML CSP、GET 和 HEAD 语义，以及同一个 worker 提供的 dry_run 和显式公开调用根地址。配置和资源在 accept loop 前构建一次；原授权、连接截止、业务槽和关闭等待关系保留。

执行 `cargo test --locked panel_http:: -- --test-threads=1`：78 passed，0 failed，4 ignored；其中 U1 新增 5 条测试全部通过。4 条 ignored 仍是原独立性能测试；原完整 80 条清单还包括该过滤器以外的 3 条 panel_admin 测试，不能据此宣称完整清单已验收。首次沙箱执行有 3 条 TCP 测试因 BindFailed 失败；获得随机回环端口测试权限后重新编译执行通过。

独立审核 0 critical、0 unresolved，仅覆盖 U1。审核发现并关闭：静态 POST 测试须先带合法 Origin 才验证 405，同时保留无 Origin 的 403；公开根地址校验须覆盖全部路径的合法字符和百分号转义，而非仅校验末段。已补非末段非法转义、裸路径非法字符和合法 `%25` 反例。

本机 Python JSON Schema FormatChecker 没有注册 uri。前一轮配置 schema 正负例证明形状与显式 pattern，不单独证明完整 URI 语义；本次 Rust URL 检查及实际负例补足该部分。最终机械检查仍需保留这一能力边界。

## U2 至 U4 请求和刷新基础

已实现 schema 派生常量与消费字段声明、白名单路由、有界 HTTP 读取、ID 字符串和安全数值检查、双 lane 真实 ownership、单生命周期 pump，以及快照和 GLM 相关样本的年龄降级。后续已与三页面调度组合。

本组交接时 `npm run test:ui` 24 passed、0 failed、0 skipped，包含 4 组 fast-check PBT，各 128 cases。独立审核 0 critical、0 unresolved。审核期间补入：retry 的原响应为 200；序列化后真正发送前再次检查截止；拒绝响应头或迟到响应时也取消未消费 body；不同采样时间不能按墙钟大小排序；服务端 stale 样本不能复活为 current；从未采样的 unknown 与过期观察分开。

本沙箱中 Node 默认测试子进程隔离只报告文件级成功，没有具名断言结果。因此测试命令明确使用 Node 22 的 `--experimental-test-isolation=none`，实际输出每条具名测试；最终清单检查必须拒绝空跑、文件级成功冒充以及 skip/ignore 漂移。该变更不减少测试用例或 PBT 次数。

## U5 至 U7 管理表单和操作恢复

来源、组和 Key 页面已组合。按配置、成员、组策略、运行版本分别冻结草稿；创建来源的模型默认发现；目录保存与显式 Ensure 分开。sessionStorage 仅存最多 16 个操作 ID；同页重发冻结载荷，重载只查原 ID。身份页与两种 owner 的原生页分别保留，缺页不解释为删除。

独立审核 0 critical、0 unresolved。关闭的问题包括：旧 failed 回执阻挡重试丢响应恢复、迟到失败污染新页、身份草稿未提示新版本、原生旧页误挂新页标题、切组未使旧读取失效、背压后读取缺 5 秒冷却，以及把周期起点误称覆盖起点。Key 操作完成提示明确要求分别刷新身份与原生页，不承诺目录 state 包含身份。

第一组真实浏览器组合为 24 passed：Chromium/Firefox × 1280×900/390×844，各 6 例。包含实际 Rust listener 的 HTML、模块和 CSP，以及明确标注的 API fixture 交互。首次运行发现浏览器定时器作为对象方法调用会触发 Illegal invocation，改为包装函数后通过；这不是仅凭 Node 测试得出的兼容结论。

## U8 至 U10 用量和敏感复制

按原始模型身份绘制完整堆叠图，图例和明细分页不截断图数据；null 模型与字面 unknown 分开。查询冻结采样时间，校验全部安全整数、模型/原生 Key/桶总计及桶模型边际。未初始化、无消费、消费行存在但 token 为零分别展示。

SecretSlot 仅短时持有显式 reveal 的值；剪贴板未确认则显示同一次取得的只读值，不重新请求。隐藏、换页、换身份、成功或到期清除。缺必要浏览器能力或初始化失败时只读提示与重载，不回退旧接口。

独立代码审核 0 critical、0 unresolved。已补展示年龄跨过火山重置边界时的发起/迟到接收/旧图标记、同色模型图例、换身份清理、能力失败清理，以及消费行零但 token 非零的拒绝。

第二位独审从契约重新检查后，又关闭五项遗漏：显式 usage 进入同一个最新 desired-read 并优先于周期操作查询；补类型/名称筛选、顶部刷新和进度折叠；显示独立 quota 样本年龄并允许归档模型只读查看；严格检查恰 100 项必须返回最后 ID 游标；加入真正由异源浏览器页面发起的请求测试。恢复事件用 pagehide/pageshow 显式注入覆盖，不冒称实测了浏览器 BFCache 驻留。

复制的三种浏览器用例显式注入 Clipboard Promise 成功、拒绝或迟到，验证真实页面的等待、提示与清理；不冒称向宿主系统剪贴板写入后读取验证，也不把注入拒绝当作浏览器原生权限弹窗实测。

## 机械交叉验证和 CI

32 份合成报文通过原完整 JSON Schema：24 份实际 body/query builder 输出、3 份显式查询或重试样例、5 份响应/快照/配置消费投影，共覆盖 21 类定义，并验证同数量额外字段反例。另注册仅用于 UiConfig 的显式 HTTP URI 检查，验证 6 个非法公开根地址，不依赖缺失的可选 format checker。A2.2 实测捕获 3233 份报文通过原 schema，含全部 28 种错误上下文；其中 1949 份实际 Rust 回包、13 个响应族均通过前端消费检查。

UI 清单脚本核对具名实际执行结果、八组 PBT、生成常量、步骤和危险 DOM 接口；本轮核心/schema/旧断言冻结使用显式 `--freeze`，避免把开发基线提交作为未来每次 CI 的历史依赖。新增独立 UI CI job，Node 22.21.1、npm ci、两个浏览器和串行 UI 性能入口；Rust 原有命令均保留，timeout 从 15 分钟增至 45 分钟。setup-node 的 v7 用法依据其[官方仓库](https://github.com/actions/setup-node)。尚未推送，因此没有声称 GitHub CI 已执行通过。

完整报文 JSON Schema 交叉验证是本次本地机械门禁；CI 的 UI contracts 步骤覆盖生成常量、字段声明和具名用例，不冒称已运行 wires Python 检查。浏览器使用独立冻结清单与实际结果 reporter：普通门禁要求 18 例×4 项目，性能要求 1 例×4 项目，拒绝空跑、skip、xfail、重试及视口漂移；定向调试明确标为 partial。

## 已完成的收尾门禁

| 门禁 | 实际结果 |
| --- | --- |
| npm ci | 固定 lock 离线复现通过，5 个安装包，未运行安装脚本 |
| 冻结和清单 | 核心/schema/旧断言冻结；原 80 条加 6 条 UI Rust 清单精确匹配；U1–U10 与危险 DOM 检查通过 |
| Node | 51 passed、0 failed、0 skipped；8 组 fast-check，各 128 cases |
| 浏览器交互 | 72/72，两个浏览器和两个视口完整通过；独立 reporter 核验实际执行结果 |
| 新增性能 | Node 两项、浏览器四项目全部通过；见下方实测值 |
| 原性能 | 31 项 release 与 4 项 debug 全部通过；runner 回归 24 项通过 |
| Rust debug 全量 | 独立顺序复验 744 passed、0 failed、35 ignored、0 filtered，994.94s；旧失败用例原样全部通过 |
| Rust release 全量 | 冻结源码复验 744 passed、0 failed、35 ignored，940.94s；35 项的构成和独立执行见下文 |
| 有效变异 | 隔离副本 17 项全部捕获并恢复：15 项 JS、2 项 Rust；每次恰 1 个目标断言失败，恢复恰 1 项通过 |
| release build | 通过；未启动或部署生成的二进制 |
| Clippy | 默认 all-targets 通过；新增面板无 warning。额外尝试 -D warnings 被 9 处既有未改文件的 lint 警告阻挡，未改冻结模块或加 allow 掩盖 |

冻结输入记录为 `/tmp/quota-throttle-sgm6-b-final-source.sha256`，共 253 个文件；debug/release 全量和隔离变异结束后，主执行者与独审分别核验全部未变化。主要日志前缀为 `/tmp/quota-throttle-sgm6-b-`，完整浏览器报告在 `browser-8LUBqH/report.json`，性能浏览器报告在 `browser-Ji64KS/report.json`（均同此前缀）。

普通 Rust 套件共 779 项，默认执行 744 项；35 个 ignored 为原有 34 项加唯一新增浏览器 fixture 进程入口，并非将失败功能测试移入 ignored。原性能的 31 release 与 4 debug 是独立执行清单，不与 35 个 ignored 简单一一相加；浏览器入口已由真实浏览器 runner 显式启动和关闭。

性能环境为 Linux、Intel i9-12900KF、Node 22.21.1。最大容量 PanelSnapshot 20,179,526 bytes，解析及投影 171.69ms（预算 1000ms）；745 桶、1024 模型、4096 原生 ID、65,536 联合项、256-byte 模型标签的 22,544,573-byte 报表准备 361.66ms（预算 500ms）。四个浏览器项目最大绘图 flush 加两帧 613ms、100 行 DOM 布局 166ms、来源详情完整绘制 790ms；分别在 2000/500/2000ms 预算内。来源图确认为真实可见 canvas，二次刷新复用它，不用 null 等值假绿。以上是固定输入和环境的实测，不推广为任意标签/设备的耗时保证。

首次模型图准备为 524ms，未通过 500ms 预算；优化固定字段列表复用和已验证标签的身份键构造后，通过同一预算，随后还加强了桶、模型、原生 ID 和标签容量，未删数据或提高门槛。

## 失败尝试和复验边界

首次完整 debug 为 740 passed、4 failed、35 ignored，release 为 744 passed、0 failed、35 ignored。debug 失败涉及自身可执行文件不存在和 LocalBinding；当时同一 target 发生交叉重编译，症状与测试二进制被替换相符，但没有保留当时的 proc 链接，因此将其记为编排原因判断，不当作内核级直接证明。停止交叉重编译后，9 项相关预检与跨进程锁 1 项均原断言通过；这些用例在下一次冻结版本完整 debug 中也全部通过。

第二次完整 debug 为 743 passed、1 failed、35 ignored，1165.91s。唯一失败为旧 `h1_http_rejections_are_safe_and_head_never_has_body` 在 `src/panel_http/tests.rs:233` 对 `cache.get(...).unwrap()` 得到 `SnapshotUnavailable`；完整日志保存在 `final-debug-concurrent-attempt.log`（同上述前缀）。该测试只初始化一次真实时钟快照，连续九次建立临时 worker/SQLite 和 HTTP 请求后检查同一缓存；未变更的缓存会在 10 秒年龄边界清除自身。该证据定位到快照有效期断言，不是 HTTP 403/400/405 或正文断言失败；与并发 release/隔离编译下耗时超限相符，但未记录失败瞬间年龄，不能把负载归因说成已证明的唯一原因。其他重任务全部结束后，原用例定向复验 1 passed、0 failed，2.35s；未改代码、断言、默认线程数或测试集合，随后单独完整 debug 为 744 passed、0 failed、35 ignored、0 filtered，994.94s。旧 H1 用例、预检与跨进程锁在该完整运行中全部通过；最终日志为 `final-debug.log`，没有用定向绿代替全量结论。

早期 16 个变异确实逐项捕获并恢复，但曾在开发工作树执行，不与最终冻结源的验证混算。现有 runner 改用 `/tmp` 白名单源码副本、独立 target 和 SHA 记录，不复制真实配置/密钥/数据库；加入原 ID retry 的实际出站变异。最终 17 项全部通过，证据位于 `/tmp/quota-throttle-sgm6-b-mutations-gYOd2q`：15 项 JS 均先通过语法/模块加载再命中语义断言，2 项 Rust 变异及恢复均实际编译；每案基线/变异/恢复分别恰 1 项通过/失败/通过。独审核对了所有日志和副本 SHA，未用环境错误或空跑代替捕获。

浏览器新增竞态用例最初因嵌套 label 的精确定位超时；改为可访问 combobox 角色后四项目通过，再跑完整清单。跨源用例初稿错误期待 no-cors 返回 opaque，实际被 CORP 拒绝；修正为两类 fetch 都被拒绝，并等待下一轮真实目录快照证明零写入。完整 72/72 结果包含修正后的用例。

## 测试环境

开发依赖固定为 fast-check 4.10.2 和 Playwright 1.64.1，已生成 package-lock。npm 缓存位于 `/tmp/quota-throttle-sgm6-b-npm-cache`。Chromium 156.0.8078.4 和 Firefox 157.0 位于 `/tmp/quota-throttle-sgm6-b-browsers`；二者已实际启动并运行上述组合。未在本机安装系统软件或改变生产依赖；CI 的系统依赖安装只面向临时 runner。

镜像缺中文字体，最终浏览器门禁使用 `/tmp/quota-throttle-sgm6-b-fonts.Cjs4Vf/fonts.conf` 指向临时 Noto CJK 字体；不增加生产资源或修改系统字体。字体 SHA256 为 `2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b`。

## 复验入口与编排

以下命令均在本开发 worktree 执行。完整 Rust 双 profile 应顺序运行；任何使用同一 Cargo target 的检查、浏览器 fixture 编译或变异不得在其间重新链接测试二进制。独立性能也不与完整回归/编译争抢资源。变异 runner 自己创建白名单源码副本和独立 target，不操作现网。

```sh
node scripts/contract_audit/sgm6_b_check.mjs --freeze --rust
cargo test --locked --offline
cargo test --release --locked --offline
npm run test:browser
npm run test:ui-performance
npm run test:browser-performance
bash scripts/tests/performance-gates-regression.sh
bash scripts/tests/performance-gates.sh
node scripts/contract_audit/sgm6_b_mutations.mjs
cargo clippy --locked --offline --all-targets
cargo build --release --locked --offline
```

`--freeze` 是本批针对 f206984 的范围/旧断言审计，不是未来所有开发都必须仍以该提交为基线。完整报文检查入口分别为 `sgm6_a22_wires.py`、`sgm6_b_wires.mjs` 和 `sgm6_b_wires.py`，仅接受隔离临时目录；实际产物在 `/tmp/quota-throttle-sgm6-b-rust-wires.ucDcwU` 和 `/tmp/quota-throttle-sgm6-b-wires.Zh0v6e`。这些目录含合成凭据和 fixture，不含实际运行材料。

## 实施退出与下一步

冻结版本完整 Rust 双 profile、Node/PBT、浏览器、全部性能、schema/清单、17 项有效变异、格式/default clippy/build，以及结束 SHA 均已通过。第二独审最终核验完整日志和 253 文件 SHA，critical 0、unresolved 0；workflow §5.2 五维度通过，B 实施退出。

下一步是 C 的真实 worker 和隔离 NewAPI 双协议/统计组合验收，B 的 API fixture 浏览器测试不能替代它。SGM7 才接运行和 fermat 全新部署；此次没有上传源码、操作 fermat 或改变本地现网。
