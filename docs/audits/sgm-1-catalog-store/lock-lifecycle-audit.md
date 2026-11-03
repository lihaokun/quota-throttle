# 目录锁生命周期修复独立审核

日期：2026-10-17。基线 `6a9db05`。独立审核者 `catalog_lock_lifecycle_audit` 先从[修复方案](../../fixes/catalog-fix-inherited-lock-release.md)、SGM-1锁/open/Drop条款和expectations时序节抽取期待，再只读diff及必要局部依赖；不运行测试、不以主代理动态结果背书、不访问运行实例。

| 期待 | 静态核对 |
| --- | --- |
| 成功持锁后才构造对象 | ✅ try_lock成功后才构造CatalogLock，错误映射不变，没有失败guard解锁路径 |
| 数据库关闭前不释放锁 | ✅ conn在_lock之前，无CatalogStore提前unlock的Drop；构造前局部资源逆序释放，构造后字段顺序释放 |
| 析构保守且无panic | ✅ unlock返回值被忽略，随后File正常关闭；不sleep/retry，不伪造下一次open成功 |
| 重复FD存活重开 | ✅ 使用实际Store和真实try_clone，验证非空快照、旧副本关闭不影响新owner；能捕获删除显式unlock |
| 真实关闭顺序可观测 | ✅ SQLite xDestroy时独立FD仍拿不到锁；注册成功且事前未回调，排除注册失败/立即析构假象 |
| 测试FFI安全 | ✅ Box只由xDestroy回收一次，回调无assert/unwrap；核对了本地SQLite契约和rusqlite关闭实现，不改持久schema |
| 错误清理与排他 | ✅ guard错误返回和实际高版本拒绝有测试；构造失败完整顺序另外由局部源码检查证明，不冒称所有分支已动态注入 |
| PBT参考独立 | ✅ 128-case操作序列驱动独立owner布尔模型，OS查询只作实际值；副本可跨owner代际 |
| 跨进程验证防假绿 | ✅ 子入口完成行为检查才输出唯一标记；父端同时核退出码/单个标记，实际错exact名的负向检查拒绝零测试成功 |
| 范围 | ✅ 无DDL、公开API、迁移算法、调度或依赖变更 |

结论：0个❌生产偏移；两项⚠️建议已复审关闭，无未解决静态问题。动态红绿、完整回归和具体变异结果须以修复记录为准，本报告没有独立重跑它们。

这不表示SGM-4已实现，也不授权部署。处置见[决策记录](lock-lifecycle-decisions.md)。
