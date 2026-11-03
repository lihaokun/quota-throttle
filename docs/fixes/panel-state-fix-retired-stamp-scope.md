# 面板已提交判定的退休成员范围修正

2026-10-19，SGM6 A1首次实现验收，尚未接HTTP/运行服务。先记录根因与方案，再独立复核后修正。

## 一 现象与复现

fermat隔离复验17 passed/1 failed，7.06秒。新增退休路径完成真实worker的禁用/回读/成员退休，再提交新GLM轮；`s6a1_retirement_missing_members_and_new_binding_invalidate_display`第一条`original.glm.groups[0].current`断言失败。日志`/tmp/quota-throttle-sgm6-a1.LAtp1k/targeted-green.log`保留，名称不代表通过。

## 二 根因

缓存producer从`applied.round.groups[].work`取stamp；原`execution::group_work`的SQL明确只载入`lifecycle!='retired'`。consumer却从包含历史退休成员的PanelCatalog取整组集合，因此退休之后两边永远不等。设计误把“完整策略输入集合”写成了“完整目录历史集合”，callee前提读错；不是时效预算或业务退休错误。

## 三 参考契约

原group_work、compute/apply_glm_round和decide协作处理全部未退休成员；其中暂停、请求退休、无channel等仍留在work供过滤，已经退休者根本不传入。CatalogSnapshot则永久保留历史身份。两个集合各有职责，不能为展示把退休成员重新送入旧策略。

## 四 修正方案

stamp固定为完整的未退休成员集合，consumer仅按lifecycle过滤已退休者，与原producer一致；仍包括暂停、请求退休、无channel和非临期成员，禁止只比较当前可选项。显示目录继续包含全部历史身份。未退休→退休或真实集合增减令旧判定过期；新判定对已退休历史元数据不再因无关变动失效。

只在`GroupInputStamp::from_catalog`增加该集合过滤，不改原GLM/worker执行/DDL。设计与expectations同步这个callee边界。原失败的“退休后新判定应current”断言保持；矩阵中“已经退休的历史时间改变也应stale”属于错误预期，按新明确语义改为不失效，并另测当前成员退休导致失效。

## 五 正确性论证

两端由同一lifecycle谓词定义输入域，原成功事务中的新缓存可与同版本目录相等；当前成员新增、绑定、控制版本、来源暂停等仍改变该集合或stamp字段，不能洗白真正过期的判定。退休历史仍出现在目录输出，只有不参与算法的历史字段退出展示资格比较。没有新查询、缓存权威或策略分支。

## 六 测试同步

保留退休后新判定应current的红例；增加/明确已经退休元数据不影响、当前成员退休/移除影响、无channel到绑定影响。所有既有3组PBT/只读/时间/取消/提交断言继续运行；变异专门移除这一scope过滤应重新命中原红例。初轮夹具半开区间错误单列，不算本问题红例。

最终结果：18项A1普通测试及3组各128 cases PBT通过；移除scope过滤的M8成功编译后精确0/1、命中原current断言。源码恢复且实际重新编译后18/0；默认debug/release各663/0/30，机械/全部性能/clippy/build通过。首次恢复工具复用旧变异二进制的失败单列于[A1审核](../audits/sgm-6-three-page-panel/a1-audit.md)，不掩盖或当作生产红例。

## 七 代码同步

仅改`src/catalog/panel.rs`的consumer集合筛选及对应矩阵断言，不改producer算法。独立方案复核通过后已实施；新矩阵还覆盖请求退休后新轮仍current、真正退休后旧轮stale、最后新轮current。全部验证通过，实现提交`b4116ef`。

## 八 文档同步

A1细化、事前expectations和decisions把“全组”精确限定为原策略完整输入域，同时保留目录全历史。最终审核说明旧失败、修正与独立捕获，不把这个显示修正描述为调度策略改变。
