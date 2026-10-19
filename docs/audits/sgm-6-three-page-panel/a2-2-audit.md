# SGM6 A2.2 管理业务接口实现审核

2026-10-19，分支 `research/source-groups-token-management`，实现基线 `bcb5700`。已确认的26个新管理动作与原state接口组合完成内部验收：机械、六组PBT、默认双profile、性能、16种有效编译变异、恢复重编和最终独审全部通过。A2.2完成不代表B/C页面、SGM7运行组合或部署完成。

## 实现范围

HTTP使用逐路由DTO、固定错误码和白名单响应，不开放通用ControlCommand或任意NewAPI patch。目录、操作恢复、模型发现、token身份及查询继续调用原worker；没有第二调度器、第二写库连接、持久HTTP任务表、新依赖或DDL。GET state保留原唯一publisher与缓存字节读取。

四个业务许可贯穿handoff、blocking池排队、实际执行、编码和结果消费。HTTP超时不归还后台任务许可，不承诺回滚。正常关闭先禁准入，join连接、真实业务和publisher后，再等原查询槽真正空闲；强制终止不承诺此生命周期。

GLM既有策略、火山既有状态机和单key单组语义不变。OpenCode与Claude仍是同一访问key支持的下游协议。新代码未接CLI、旧status或现行run；没有重启、部署、真实key/配置/数据库访问、推送或合并。

## 分层验证记录

| 阶段 | 实际结果 |
| --- | --- |
| H2.1严格形状复验 | 13 passed / 0 failed，含PB1的128 cases；日志 `/tmp/quota-throttle-sgm6-a22-input-green.log` |
| fermat中H1与H2.1 | 46 passed / 0 failed / 2 ignored，4.32秒；只包含桥接前源码 |
| 第一批桥接 | 6 passed / 0 failed，19.77秒，含PB6的128 cases；编译3分39秒；日志 `/tmp/quota-throttle-sgm6-a22-bridge-tests.log` |
| 本地沙箱TCP尝试 | 24 passed / 6 failed；六条在bind时因Operation not permitted失败，不当业务红例；日志 `/tmp/quota-throttle-sgm6-a22-h2-all.log` |
| 授权回环后的首轮 | 37 passed / 1 failed / 2 ignored，32.72秒；失败在火山夹具初始化 |
| 夹具修正后 | 41 passed / 0 failed / 2 ignored，32.92秒；日志 `/tmp/quota-throttle-sgm6-a22-admin-local2.log` |
| 首轮实产出schema | 3296正例、18请求和13响应定义、60根结构负例、28业务错误context、6旧schema规范化一致；目录 `/tmp/quota-throttle-sgm6-a22-wires.L9W91b` |
| 性能runner自测 | 原24项通过；实际性能另跑 |
| 最终机械 | 80/4精确清单、14 Step、H1正文和核心冻结通过；实产出3281正例、31定义、60负例、28错误context、6旧schema一致；fixture目录 `/tmp/quota-throttle-sgm6-a22-wires.kw0gLY`；44 passed / 0 failed / 2 ignored，32.88秒 |
| 最终clippy | all-targets通过，13.46秒；只有原8条生产及1条A1测试警告，无新增warning |
| 默认debug全量 | 739 passed / 0 failed / 34 ignored，969.08秒，未缩减默认测试并发或PBT cases；日志 `/tmp/quota-throttle-sgm6-a22-final-debug.log` |
| 默认release全量 | 739 passed / 0 failed / 34 ignored，672.58秒；release重编译5分10秒；日志 `/tmp/quota-throttle-sgm6-a22-final-release.log` |
| 31项release性能 | 31 passed / 0 failed / 0 ignored，43.54秒；用量21,778,135字节编码99ms；1,048,576字节DTO解析按整毫秒输出0ms，不等于零耗时 |
| 4项debug性能 | 各1 passed / 0 failed / 0 ignored；原控制、导入、组快照、GLM纯容量入口及原预算保持；日志 `/tmp/quota-throttle-sgm6-a22-final-performance.log` |
| release build | 实际重编译35.21秒通过；日志 `/tmp/quota-throttle-sgm6-a22-final-build.log` |

`admin`过滤额外命中既有`t5a_signer_readiness_never_falls_back_to_admin`，41通过包含1旧测试和当时40条新增常规测试，不能全称新增。最终清单80项：35旧H1、45新A2.2，其中4项为独立性能；6组PBT各128 cases。

schema校验去掉根anyOf，按路由专属$defs验证；六份嵌入旧schema逐一比较。原始JSON重复键、UTF-8字节域及跨字段语义由Rust测试验证。首轮机械运行期间补充过测试和脚本，不作为最终冻结证据；必须从头复跑。一次机械调用因output.rs模块声明排序未符合rustfmt退出，随后仅格式化该文件。

## 独立审核与处置

独审者`sgm6_a2_contract_audit`先从批准契约抽取边界，再复核增量。H2.1和桥接/映射静态审核各为0个生产实现阻断；未把未运行的动态用例算作通过。

| 问题 | 处置 |
| --- | --- |
| Serde隐式接受额外unit字段、对象数组、标量枚举对象 | 保留6条红例，按[严格形状修正](../../fixes/panel-admin-fix-strict-json-shapes.md)增加局部map/string门禁；不改共享enum或schema |
| 新夹具误写models及manually_paused | 按批准manual_models和既有manual_paused修正夹具，生产拒绝规则保持 |
| 火山初始化时间1000导致UTC+8周期起点为负 | HTTP启动前准备失败；改用当前SystemTime，原非负周期规则不改；撤错推进runtime revision的预期沿原error epoch语义 |
| publisher取Core可能掩盖漏join业务 | 增加publisher不取Core的独立停机测试，HTTP超时后仍要求serve等待真实业务 |
| advance会yield，原Ready用例可能先消耗timeout | 手动pin/poll admit至Pending，推进截止后必须成功send，再poll；M16专门删除事后截止检查 |
| 复制后的state允许503，不能证明正常快照不泄密 | 改为必须200且有真实catalog.sources；M9另注入复制进入普通cache |
| 机械脚本未冻结H1正文 | 仅去除精确worker/connection薄适配，再与基线tests.rs全文比较 |
| 两个新增clippy常量profile断言警告 | 去除非必要构建模式断言；原runner固定release，1秒/2秒实际墙钟断言不变，不抑制lint |

主执行者已读取锁定Tokio 1.52.3的timeout.rs:217先poll value、clock.rs:280 yield_now，确认Ready用例补强依据。新token、legacy-only管理、local/native ID隔离、操作重放、发现成功权威/失败不覆盖/繁忙不兜底、unknown/null以及真实SQLite尾部已有定向覆盖。PB5采用独立BTreeMap/整数累加oracle；PB6检查真实许可、队列和句柄守恒。

## 最终门禁

- [x] 冻结源码的80/4清单、14个Step、schema及核心冻结。日志 `/tmp/quota-throttle-sgm6-a22-final-mechanical.log`；测试二进制重编译39.50秒。
- [x] 16种变异真正编译，各捕获1条失败；临时源码恢复后真正重编译。
- [x] 默认debug与release全量。
- [x] 原29加新增2个release性能、原4个debug性能。
- [x] 最终clippy all-targets与release build，无新增警告。
- [x] 独审critical=0、unresolved=0，五维度全部通过。
- [ ] 本地提交及提交号回填；不推送、合并或部署。

最终159份src/Cargo/config.example文件的SHA256清单在 `/tmp/quota-throttle-sgm6-a22-final-source.sha256`，之后未改这159份编译输入；审核文档与脚本另行维护。默认773项回归及其余门禁均基于该冻结源码。clippy完整日志 `/tmp/quota-throttle-sgm6-a22-final-clippy.log`。

34个ignored包括已另行执行的31个性能入口，以及原3个opt-in（大历史SQL趋势、原生渠道契约、原生token组合）。本批不把假TCP或默认全量称作真实供应商、浏览器或原生双协议重新验收。

16案变异在 `/tmp/quota-throttle-sgm6-a22-mutations.mlEUkL` 完成；runner.sh与仓库脚本SHA256均为 `ad7552ff9f162ff467038098b5bf8603a0a5efbd16abb311bd42e58a5a978def`。仅修改临时副本；每案均成功编译、精确0 passed / 1 failed / 0 ignored。脚本拒收编译失败、空跑、超时及常见端口权限/磁盘/FD环境错误；主执行者与独审另逐案核验了下面的实际失败语义。

| 变异 | 实际捕获 |
| --- | --- |
| M01漏dry_run | 错误进入handoff等待，result_unconfirmed不等于dry_run |
| M02混用版本类别 | expected_policy_revision变成7，独立预期为9 |
| M03非issuer签发 | 临时callee改legacy后POST tokens回422，正常契约要求202；工作树旧核心未改 |
| M04原生ID冒充本地ID | native101被当成本地身份，reveal回404而非200 |
| M05重复JSON覆盖 | 重复schema_version的原始body被错误接受 |
| M06重复query覆盖 | 重复owner_scope被错误接受 |
| M07超时提前归槽 | 4个真实Core阻塞job存活时可用槽错误为4，预期0 |
| M08关闭丢句柄 | publisher不阻塞的用例发现serve在真实业务结束前返回 |
| M09复制进入普通cache | 复制后state虽200，却缺真实catalog.sources；先被A1形状门禁捕获，未走到后一行密钥文本断言 |
| M10发现混fallback | 发现成功的两项模型被错误附加fallback |
| M11未知填零 | 未初始化usage被改成零total和空数组对象，预期null |
| M12重试换op | 既有成功操作的retry回404，不是200原收据 |
| M13对象数组放行 | reveal/retry/group三个数组body错误成功 |
| M14标量enum对象放行 | kind/auth/scope/grain四种对象形式错误成功 |
| M15unit额外字段被忽略 | discover附额外字段被错误接受，与RCA红例同源 |
| M16去事后截止检查 | 手动poll构造的同时Ready回复变成not_found，预期result_unconfirmed |

每轮恢复后显式touch；最终全src逐字比较、Cargo.toml/lock/config.example比较及两份159文件hash全部一致。restored.log真实重编译1分20秒后44 passed / 0 failed / 2 ignored，32.85秒；总流水线exit 0。恢复结果不是复用旧变异二进制，也不把预期变异失败计成生产回归失败。

## 最终五维度结论

独立审核者逐案核对16份失败日志与恢复证据，并复核双profile、性能、机械、clippy/build及源码hash，最终critical=0、unresolved=0。一致性：逐路由、schema、ID/版本/owner和原callee一致；风格：无新lint、遵循原模块边界；正确性：基本/边界、六PBT、完整回归及有效变异通过；性能：31+4预算保持，模型标签借用，响应有界；可维护性：仅窄worker入口和私有HTTP桥接，不引入第二业务权威。

本次后来同步完整源码到fermat被权限审查拒绝，未绕过；后续使用已获准的本地随机回环端口、临时数据库和假上游。早期fermat与后续本地结果分开标注。fermat正式部署仍是全新初始化；B/C页面和SGM7运行组合不由本批代替。
