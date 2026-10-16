# 目录库关闭后因继承文件锁而暂时无法重开

日期：2026-10-16。状态：新发现问题的修正方案，待确认；尚未修改生产实现。发现于[默认并行测试门禁修复](sgm-fix-parallel-performance-gates.md)的 debug 全量回归。运行服务未访问、重启或部署。

## 一 现象与复现

默认并行 `cargo test --locked --offline --quiet` 返回 359 passed/1 failed/5 ignored，317.86秒，退出101。失败为 `property_c4_c6_import_replay_and_conversion` 在 `src/catalog/tests.rs:984` 的 `drop(store)` 后立即 `CatalogStore::open`，返回 `CatalogError::AlreadyOpen`，不是耗时断言。失败种子 `4817466a9e39c74d5eb80053fe3765c3e42ec1eef842c4c5147285a9b9d0dad0` 已保留在 `proptest-regressions/catalog/tests.txt`；单独回放128 cases通过（8.98秒），说明不能只靠该随机输入稳定复现并发调度。

当前构造使用 `File::try_lock`，Store 持有 `_lock: File`；销毁没有显式 unlock。独立单线程 Rust+fork 门闩程序在私有临时目录验证：父进程 drop 原文件后、子进程仍活着持有继承描述符时，重开文件 try_lock 返回 WouldBlock；父进程先显式 unlock 再 drop，则子进程仍活着时就能重新取得锁。结果为：

```text
explicit_unlock=false blocked_while_child_alive=true acquired=false
explicit_unlock=true blocked_while_child_alive=false acquired=true
```

实跑程序：`/tmp/quota-throttle-lock-inheritance.XR8cef/repro.rs`，只创建两个测试锁文件，两个门闩子进程均已回收。这个实验验证继承锁机制，不假称已在 CatalogStore 完成组合回归。一个可固化为实际 Store 回归的确定性输入是：取得 Store 后克隆其锁描述符模拟继承，保留克隆，drop Store，再断言同目录重开成功；当前实现预期失败。

## 二 根因分析

`AlreadyOpen` 来自独立打开的锁文件仍被某个描述符持锁。把“销毁当前 File”视作“内核锁一定释放”不成立：同一打开文件描述的克隆/继承句柄尚在时，关闭父句柄不会释放锁。当前库的生命周期处理缺少显式释放。

全量套件存在并发进程创建（例如本地预检调用 ss），能产生 fork 到 exec/关闭句柄之间的继承窗口。外部进程产生该窗口是与实测和代码吻合的原因推断；本轮没有跟踪第一次失败时具体哪个子进程持有该描述符。独立实验已确认足以导致相同现象的内核机制。

## 三 参考行为

Rust File 文档明确：锁在本文件及所有重复/继承句柄都关闭，或调用 unlock 时释放；Unix实现使用 flock。[File::try_lock / unlock](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)

Rust 先运行类型本身的 Drop，再依声明顺序析构结构体字段。因此不能简单在 `CatalogStore::drop` 开头解锁：那会早于数据库连接关闭，违反整个数据库资源使用期被锁保护的条件。[析构顺序](https://doc.rust-lang.org/reference/destructors.html)

## 四 拟议修复

将成功获得的文件锁放进最小的内部持锁对象，析构时显式解锁；Store保持先销毁连接、后销毁持锁对象的字段顺序。仅在 try_lock 成功后构造持锁对象，打开/验证/迁移失败的局部变量析构也要保证连接先关闭。失败拿锁仍立即 AlreadyOpen，不加 sleep/retry，不改变单写者门禁。

析构不 panic；解锁失败时仍关闭本句柄，不能声称后续一定可重开。不得增加数据库登记、新服务、DDL或新依赖。正常运行服务的二进制仍保持不变，本修改只在开发分支验证。

## 五 正确性论证义务

1. 根因消除：本持有者关闭时显式释放其内核锁，不再依赖无关继承句柄的关闭时刻。
2. 不变量保持：数据库连接整个存活期均受锁保护；不能先解锁后关闭连接；未成功持锁的入口不得执行解锁逻辑。
3. 无回归：同时打开第二个 Store、跨进程竞争、外库/损坏库拒绝、失败后的资源释放仍符合原契约。不能把所有 AlreadyOpen 当作瞬态而绕过。

## 六 测试清单

| 类型 | 用例 | 状态 |
| --- | --- | --- |
| 必须回归 | 实际Store持锁描述符被克隆后，原Store关闭但克隆存活，重开成功；先证未修实现失败 | 待实现 |
| 保持排他 | 原Store仍活着时第二次open必须AlreadyOpen，不因别的失败open解掉第一把锁 | 待验证 |
| 错误清理 | 验证/迁移等成功持锁后的错误退出，连接及锁都正确释放，数据不被修补 | 待验证 |
| 顺序 | 数据库连接释放先于文件锁释放；保持原析构/错误路径资源顺序 | 待验证 |
| 原回归 | 保存的导入PBT、所有目录/迁移/执行恢复测试及默认debug/release全量 | 待验证 |
| 负向 | 撤掉显式unlock，确定性回归必须失败；扩大为无条件解锁应被排他用例捕获 | 待验证 |

## 七 代码更新清单

计划仅修改 `src/catalog/store.rs` 内部锁所有权及相应 catalog 测试；不改实际调度策略、NewAPI管理HTTP、CLI/面板、DDL或依赖。当前只有失败种子被测试写入并保留，生产实现未变。确认后先加确定性回归，再实施修复和审核。

## 八 文档更新清单

修复后同步 SGM-1 的持锁/关闭生命周期和 expectations，保留后续v4迁移承接；回填本方案的红绿、负向与完整回归证据。性能门禁修复记录保留本次debug失败，不用之后一次绿测覆盖它；整体CI可放行需等待此问题处理。
