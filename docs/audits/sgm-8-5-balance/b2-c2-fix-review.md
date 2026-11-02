# B2 C2 修正执行记录

2026-11-02，修正基线 `d41c4fb`，产品Rust仍为 `9619729`，原C2失败报告保留。用户已要求修正；方案见 `docs/fixes/sgm8-b2-fix-verification-gaps.md`。**专项修正、独立审查、反向验证和全量门禁全部完成，随本次本地提交交C2复核。** 不冒称该新SHA的提交后C2已执行。

## F1–F3：测试缺口

只修改 `src/newapi/transport/balance_tests.rs`，没有改动调用层Rust实现。

- `finish_at` 在原end只yield、不推进时间，有100000次poll上界；不能用“最多再等40s”掩盖漏传或延长截止。原Pending、64槽、401等待和共享登录等待者均使用精确判定；已准入HTTP仍用原finish完成窗口。
- `balance_profile_entrypoints_keep_deadline_through_all_waits` 从两个公开profile方法进入业务预算、Pending refresh锁、Pending登录预算六组合，断言原end/NotSent、无HTTP、许可及队列归还。
- `balance_public_deadline_before_equal_after_with_idle_budget` 从四个公开until入口核对空闲预算的截止前、恰到期、之后；恰到期/之后零准入/零HTTP，正例确认handler可达。
- 原非法输入测试增加cap、超i32金额/ID、i64最大ID的完整JSON等值断言，保留原校验、guard和连接失败断言。

focused由14→16项，**16/0**，原128例未选中waiter PBT原样保留。不把新增定向覆盖称为全域/整个第5批PBT完整。

独立reviewer `/root/b2_fix_test_review` 先做静态审核，再在独立源/target验证原四项逃逸；全部编译后被精确测试捕获。另验证Observe、Control单独丢deadline和ID窄化，合计七项均exit101、0/1，非编译错或外部kill。初始/最终恢复重编译均16/0，179个复制输入恢复哈希一致，测试跨九轮不变。报告与原始证据：`/tmp/sgm8-b2-fix-independent-_2o_1naf/{independent-review.txt,results.json,restored-verification.json}`。该副本验证Rust测试/实现；同期面板修复另行验证，不把早先复制的UI资产冒称最终UI输入。

## V1：确定根因与最小面板修复

独立reviewer `/root/b2_fix_browser_diagnosis` 用随机localhost合成fixture确定复现：pointerdown触发真实queryUsage或纯数据paint，原guard移除按钮；click未到app.action，因此没有清secret。正常click与busy直接调用action的控制场景排除readOnly本身漏清。原91/1历史trace与机制吻合，但缺少pointer日志，不声称唯一证明该次历史因果。

修正使用一个 `{button,kind,id,released}`，只延迟包含该按钮区块的replaceChildren且不更新stamp；真实click/cancel/blur等清理，up未伴click由既有tick清理。suspend清引用、destroy移除新增九个监听；无新timer/重试/业务队列，不在pointerdown执行业务。周期work在已开始手势期间让位，secret过期和state检查仍继续，due不消费；显式/已在途work不被抢占，action仍按最新readOnly/stale/dry_run判定。

CSS-only候选被真实Firefox Space和Chromium可信touch反例否定后才采用单引用方案。DOM保护部分有20场景/140断言、周期优先级另有双引擎4场景/60断言。详细报告 `/tmp/sgm8-b2-fix-browser-analysis.md`。独立未参与设计的 `/root/b2_fix_ui_independent_review` 五维通过，额外只读Node探针29断言通过；报告 `/tmp/sgm8-b2-fix-ui-review.md`。原有匿名监听清理边界为基线既有，新增监听均可移除，不扩大为新框架。

永久回归 `active controls survive repaint and periodic work yields without early activation` 检查pointer/周期tick、显式busy、Space数据重绘、取消手势；Chromium-mobile再走可信CDP触屏。检查节点保留、未提前激活、当前权限、secret清理/保留、零写、取消后下一paint刷新。原失败用例与其全部断言/时限保留，浏览器清单23→24、92→96，精确reporter计数同步。

独立副本实际执行同一永久case：Firefox基线通过→删DOM guard失败→恢复通过→删tick yield失败→最终恢复通过，均原断言/时限/零重试。最终Firefox-mobile、Chromium-mobile（含touch）各一次partial1/1。证据：`/tmp/sgm8-b2-permanent-browser-mutations.md`、`/tmp/quota-throttle-sgm6-b-browser-b2-permanent-hU9EPi`，82文件证据哈希校验通过，最终14项输入与主树一致。partial只证明回归/变异有效，不替代96项完整门禁。

## 保留的失败与修正理由

1. 首轮Node为57/3：旧隔离fixture缺新增renderer.clearInteraction/settleInteractions接口。补fixture接口并加入交互清理/周期让位断言，未删除或放宽旧断言；随后60/0。原日志 `/tmp/sgm8-b2-fix-node-first.log`。
2. 新触屏子场景首次body断言均完成，但finally在touchEnd后再次touchCancel被CDP拒绝；整次仍记失败。修正测试清理为仅touchActive时cancel且始终detach，所有expect文本未改。旧失败日志、trace、cleanup-only.patch与两版test均保留于永久变异目录；修正后两引擎最终基线通过。

## 门禁

| 项目 | 当前结果 | 证据 |
|---|---|---|
| Rust focused | 16/0，含128例PBT | `/tmp/sgm8-b2-fix-focused.log` |
| Rust full默认并行 | 803 passed / 0 failed / 35既有ignored，0 filtered，699.46s | `/tmp/sgm8-b2-fix-full-rust.log` |
| Node / check:ui | 60/0；精确清单、8组PBT、契约及DOM检查通过 | `/tmp/sgm8-b2-fix-node-final.log`、`/tmp/sgm8-b2-fix-check-ui.log` |
| 浏览器定向 | 原失败用例+新回归四项目8/8（partial8/96），后续可信touch清理修正另有两引擎最终partial基线 | `/tmp/sgm8-b2-fix-browser-focused.log`及永久变异目录 |
| SGM7 | 通过，26精确Rust名称；15 C/4浏览器仅清单核对 | `/tmp/sgm8-b2-fix-sgm7.log` |
| 浏览器full96 | 一次96/96，0失败/跳过，原断言/时限/零重试，fixture正常退出 | `/tmp/sgm8-b2-fix-browser-full.log`；`/tmp/quota-throttle-sgm6-b-browser-f6Blx7/report.json` |

机械范围核对：生产Rust/依赖/schema与修正基线逐字不变；src仅一个Rust测试文件与两个UI模块变动，浏览器精确24名称×4项目=96。rustfmt、JS语法与diff检查通过。V1是用户“修正”所含红门禁的关联修复，范围扩展已在修正文档中明确，不沿用原B2“UI零修改”的历史范围声明。

本轮没有部署或改动运行配置/数据库。B3–B8未开始，后续仍按C2固定提交复审；修正后的模块审核不冒称提交后的C2已通过。
