# 目录库关闭后因继承文件锁而暂时无法重开

日期：2026-10-16，更新：2026-10-17。用户已确认，实施基线 `6a9db05`。本修复已完成红绿回归、8个新测试入口（含子进程辅助入口及128-case PBT）、双profile默认并行全量、两个变异、性能门禁、构建及独审。发现于[默认并行测试门禁修复](sgm-fix-parallel-performance-gates.md)的 debug 全量回归。运行服务未访问、重启或部署。

## 一 现象与复现

默认并行 `cargo test --locked --offline --quiet` 返回 359 passed/1 failed/5 ignored，317.86秒，退出101。失败为 `property_c4_c6_import_replay_and_conversion` 在 `src/catalog/tests.rs:984` 的 `drop(store)` 后立即 `CatalogStore::open`，返回 `CatalogError::AlreadyOpen`，不是耗时断言。失败种子 `4817466a9e39c74d5eb80053fe3765c3e42ec1eef842c4c5147285a9b9d0dad0` 已保留在 `proptest-regressions/catalog/tests.txt`；单独回放128 cases通过（8.98秒），说明不能只靠该随机输入稳定复现并发调度。

修复前构造使用 `File::try_lock`，Store 持有 `_lock: File`；销毁没有显式 unlock。独立单线程 Rust+fork 门闩程序在私有临时目录验证：父进程 drop 原文件后、子进程仍活着持有继承描述符时，重开文件 try_lock 返回 WouldBlock；父进程先显式 unlock 再 drop，则子进程仍活着时就能重新取得锁。结果为：

```text
explicit_unlock=false blocked_while_child_alive=true acquired=false
explicit_unlock=true blocked_while_child_alive=false acquired=true
```

实跑程序：`/tmp/quota-throttle-lock-inheritance.XR8cef/repro.rs`，只创建两个测试锁文件，两个门闩子进程均已回收。这个前期实验只验证继承锁机制；后续已固化实际Store回归：取得Store后克隆其锁描述符模拟继承，保留克隆，drop Store，再断言同目录重开成功。该回归在未修复实现上实际失败，修复后通过。

## 二 根因分析

`AlreadyOpen` 来自独立打开的锁文件仍被某个描述符持锁。把“销毁当前 File”视作“内核锁一定释放”不成立：同一打开文件描述的克隆/继承句柄尚在时，关闭父句柄不会释放锁。旧实现的生命周期处理缺少显式释放。

全量套件存在并发进程创建（例如本地预检调用 ss），能产生 fork 到 exec/关闭句柄之间的继承窗口。外部进程产生该窗口是与实测和代码吻合的原因推断；本轮没有跟踪第一次失败时具体哪个子进程持有该描述符。独立实验已确认足以导致相同现象的内核机制。

## 三 参考行为

Rust File 文档明确：锁在本文件及所有重复/继承句柄都关闭，或调用 unlock 时释放；Unix实现使用 flock。[File::try_lock / unlock](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)

Rust 先运行类型本身的 Drop，再依声明顺序析构结构体字段。因此不能简单在 `CatalogStore::drop` 开头解锁：那会早于数据库连接关闭，违反整个数据库资源使用期被锁保护的条件。[析构顺序](https://doc.rust-lang.org/reference/destructors.html)

## 四 修复方案与内部契约

将成功获得的文件锁放进最小的内部持锁对象，析构时显式解锁；Store保持先销毁连接、后销毁持锁对象的字段顺序。仅在 try_lock 成功后构造持锁对象，打开/验证/迁移失败的局部变量析构也要保证连接先关闭。失败拿锁仍立即 AlreadyOpen，不加 sleep/retry，不改变单写者门禁。

析构不 panic；解锁失败时仍关闭本句柄，不能声称后续一定可重开。不得增加数据库登记、新服务、DDL或新依赖。正常运行服务的二进制仍保持不变，本修改只在开发分支验证。

内部 `CatalogLock(File)` 无Clone、无公开接口。`acquire(file) -> Result<CatalogLock>` 的输入是新打开的锁文件；仅try_lock成功才构造，WouldBlock仍为AlreadyOpen，其余为Storage。失败只关闭这个未持锁句柄，不触碰其他持有者。Drop对自己持有的文件调用unlock，忽略析构阶段错误并让File正常关闭，不打印路径或私有材料。循环与重试均为零。

Store字段仍按 `conn`、`_lock`、其他元数据排列；`open` 在SQLite probe/连接之前构造成功持锁对象，错误返回依靠局部变量逆序析构。故已构造Store与构造前错误路径都先关闭数据库资源再释放锁。不支持fork子进程继续使用继承的SQLite连接/Store；测试只保留原始File副本模拟未使用的继承描述符，不复制持锁对象。

本修复保证实际执行Drop的关闭/错误退出路径；SIGKILL、abort等不执行析构，仍由OS关闭本进程句柄，若另有继承描述符尚未关闭就可能继续持锁。不能把本次修复扩大成“任意强制终止都能瞬间重开”的保证。

关闭顺序测试利用SQLite `sqlite3_create_function_v2` 的临时函数析构回调：连接关闭期间独立文件描述符仍必须拿不到目录锁，Store销毁后才能拿到。不修改持久schema；FFI只在测试中，Box仅交给xDestroy释放一次，回调不panic。[SQLite函数析构契约](https://www.sqlite.org/c3ref/create_function.html)

## 五 正确性论证

1. 根因消除：本持有者关闭时显式释放其内核锁，不再依赖无关继承句柄的关闭时刻。
2. 不变量保持：数据库连接整个存活期均受锁保护；不能先解锁后关闭连接；未成功持锁的入口不得执行解锁逻辑。
3. 无回归：同时打开第二个 Store、跨进程竞争、外库/损坏库拒绝、失败后的资源释放仍符合原契约。不能把所有 AlreadyOpen 当作瞬态而绕过。

## 六 测试清单

| 类型 | 用例 | 状态 |
| --- | --- | --- |
| 必须回归 | 实际Store持锁描述符被克隆后，原Store关闭但克隆存活，重开成功；先证未修实现失败 | lock_drop_releases_despite_duplicated_descriptor旧实现0/1、AlreadyOpen、退出101；新实现通过 |
| 保持排他 | 原Store仍活着时第二次open必须AlreadyOpen，不因别的失败open解掉第一把锁 | 独立文件描述符竞争与实际子进程探针通过；关闭旧副本不解掉新Store的锁 |
| 错误清理 | 验证/迁移等成功持锁后的错误退出，连接及锁都正确释放，数据不被修补 | guard返回Err且副本存活、实际Store拒绝高版本后可再拿锁且文件逐字不变，通过；原迁移故障测试亦随全量回归通过 |
| 顺序 | 数据库连接释放先于文件锁释放；保持原析构/错误路径资源顺序 | lock_remains_held_during_sqlite_connection_close通过；检查真实SQLite关闭回调 |
| 新PBT | 随机打开/关闭/复制/释放句柄序列，独立布尔所有者模型核排他；旧副本可跨重开存活 | lock_pbt_owner_and_duplicate_lifetimes，128 cases通过 |
| 原回归 | 保存的导入PBT、所有目录/迁移/执行恢复测试及默认debug/release全量 | 最终debug默认并行368 passed/0 failed/5 ignored，365.33秒；release同为368/0/5，239.28秒 |
| 负向 | 撤掉显式unlock，副本存活回归必须失败；把guard放到conn之前释放，真实关闭顺序用例必须失败 | 两个最终源码变异均编译成功，各0 passed/1 failed、退出101，测试各0.05秒；分别捕获AlreadyOpen和提前解锁 |

独审建议已纳入：跨进程入口成功后输出唯一握手标记，父端同时核退出码和标记，另实际运行一个错误exact名称并断言不能算通过。失败获取不构造guard由静态结构及正常竞争验证；不宣称对新打开但未持锁的独立FD调用unlock会解掉另一个owner的锁，这不符合flock的打开文件描述语义。

执行记录：最初新回归在旧实现上0 passed/1 failed（0.06秒），随后8个新测试通过（5.51秒，含128-case PBT）。握手补强前的debug全量368/0/5（337.08秒）保留为阶段记录，不替代最终365.33秒那一轮。两次全量均未设置RUST_TEST_THREADS或test-threads参数，仅限定编译jobs=2并使用nice 15/ionice idle。变异在 `/tmp/quota-throttle-lock-mutants.nXvMVB` 的独立源码与target中执行，恢复后src与主树逐文件一致；没有在主树试写错误实现。

最终主树命令为 `env -u RUST_TEST_THREADS CARGO_BUILD_JOBS=2 nice -n 15 ionice -c 3 cargo test --locked --offline --quiet`，release增加`--release`。两种profile均包含旧失败种子的回放，5个ignored仍是原1项与4个独立性能入口，无新增忽略用例。

独立性能入口4 passed（4.76秒），六预算实测：GLM输入15ms、事务1377ms、恢复886ms、基础迁移14ms、复杂迁移22ms、火山选择3ms；脚本入口回归8项通过。clippy all-targets退出0，仅原有6条warning；release build退出0（20.42秒）。diff/格式检查通过，源码白名单证明除store.rs及catalog/tests.rs外的业务源文件、DDL和依赖零改动。远端CI未触发，本记录仅声明本地验证结果。

## 七 代码更新清单

实现仅修改 `src/catalog/store.rs` 内部锁所有权及 `src/catalog/tests.rs` 的8个测试（含一个只供父测试启动的子进程入口）。未改实际调度策略、NewAPI管理HTTP、CLI/面板、DDL或依赖；上轮失败种子保留。源码变化发生在开发分支，未替换运行二进制。

## 八 文档更新清单

已同步SGM-1持锁/关闭生命周期与expectations，保留v4迁移承接。[独立审核](../audits/sgm-1-catalog-store/lock-lifecycle-audit.md)无未解决静态项，两条建议的[处置](../audits/sgm-1-catalog-store/lock-lifecycle-decisions.md)已完成。性能门禁修复中的旧debug红例保留并链接本次闭环，不覆盖历史结果。

五维度复核：契约保持成功持锁/连接先关闭/失败保守；风格复用标准File与局部RAII；正确性由确定性红绿、PBT、实际子进程、关闭回调及双profile回归支持；性能保留全部六个原预算；维护上没有新增服务、依赖、配置、重试或PID登记。仅当前锁修复退出，SGM-4及后续功能仍未完成。
