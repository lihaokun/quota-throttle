# HTTP就绪IO的绝对截止时间修正

2026-10-19，A2.1实现阶段，基线`80ba157`；先记录根因和方案，再独立复核后修正。尚未接CLI或在线服务。

## 一 现象与复现

独立代码审核发现read_head、read_body、respond依赖timeout_at，却没有显式检查已经到期的时刻。新增三个回归分别使用已就绪完整head、已就绪/全部缓冲body、可立即写入的writer，并把deadline设为当前或更早时刻；应拒绝读成功或不开始发送。修改生产代码前先运行这些用例。

实际红例：fermat隔离目录`/tmp/quota-throttle-sgm6-a21.fOECri/deadline-red.log`，成功编译49.45秒后精确3项0 passed/3 failed，0.00秒。head/body本应Timeout却返回成功，writer在到期后写出了响应；原断言均保留。

首轮原28入口为25 passed/1 failed/2 ignored，4.05秒；唯一失败是慢writer测试将16字节缓冲的期望字面量误写为17字节`HTTP/1.1 500 Inte`。已仅将字面量改为16字节`HTTP/1.1 500 Int`，原缓冲大小、绝对时限和len=16断言保留。该夹具错误不算本次deadline实现红例。

## 二 根因

锁定Tokio 1.52.3的Timeout::poll先poll内部future；内部返回Ready即成功，再有Pending才poll timer。因而timeout_at不是对Ready IO的事前授权检查，完全缓冲的body甚至不进入timeout。write_all也可能连续完成Ready写入，不再次让外层timer先行。

此外body deadline在authorize之后取now，未固定在head完成边界；任务被调度暂停时可延长约定的body窗口。45秒总连接deadline同样不能仅依赖外层timeout在Ready路径上抢先。

## 三 参考与契约

本地锁定源码`tokio-1.52.3/src/time/timeout.rs:211–217`已核对poll顺序。A2.1定义head从accept起5秒、body从head完成起10秒、write最多5秒且不超总连接45秒，数据分块不能续期；保持这些原预算和已有所有权边界。

## 四 修复方案

read_head在读前、完整head解析/返回边界检查绝对期限；返回head完成Instant供body复用。read_body在已缓冲返回及每次IO前后检查同一deadline，非法/过期不能成功返回。serve_connection立即保存head完成时刻，不从authorize结束重新起表；进入路由前再核总连接界限。

respond开始时和每个写片段/flush前后检查唯一截止；将write_all替换为有界进度循环，每次poll-ready写后也检查时间。到期、零写或IO失败直接关闭，不追加第二个错误响应。无法撤回已在deadline前发起的单次系统调用，但不得在到期后开始新IO或把迟到读取当成功。

新增边界Instant属于本模块私有读取结果，不改A1 wire、业务策略、worker、数据库或依赖。原超时响应规则不变：读取超时408，发送/总连接到期只关闭。

## 五 正确性论证

显式时间比较先于应用层IO操作与成功返回，消除Ready分支越过外层timer的路径；相同deadline贯穿每一轮，分块与authorization不会延长它。write循环每轮正进度或退出，zero-write退出，规模受有界header/body约束。不能禁止检查之后OS抢占或抢占已进入的系统调用；保证迟到读取不返回成功、到期后不发起下一应用层操作、不回滚已发送字节。

## 六 测试同步

保留原慢IO/截断/UTF-8/HEAD及全部PBT；三个新增Ready红例保持。增加head完成后的延迟消费、分块途中到期/READY返回时到期、发送阶段切换时到期的覆盖。至少一类变异移除显式检查，必须编译后被新红例捕获。真实初轮失败与修复后定向/默认全量/性能结果分别记录。

## 七 代码同步

已按独立批准的方案修改`src/panel_http/request.rs`、`src/panel_http.rs`和相应tests；增加head完成时刻传递、读/写/flush边界检查及途中Ready用例。缓存年龄及publication的显式age检查不改，旧核心代码冻结。M11/M12去掉显式截止后均成功编译并被原Ready回归捕获；恢复全树并重编译56.19秒，33 passed/0 failed/2 ignored、3.09秒。最终全量与提交结果见[A2.1审核](../audits/sgm-6-three-page-panel/a2-1-audit.md)。

## 八 文档同步

A2.1细化与expectations同步完整head结果携带完成时刻、Ready边界显式检查及系统调用不可抢占的精确保证。审核/decisions保留本问题与慢writer字面量错误，容量证据补齐独立记录，不与deadline根因混同。
