# SGM-8 B2：C2 验证缺口修正

2026-11-02；基线 `d41c4fb`（产品源码 `9619729`）。用户已明确要求“修正”，本方案落实已有 C2 意见；不新增业务策略、依赖或部署许可。

## 1. 现象与复现

四个错误变异（两个profile入口丢deadline、初始Pending延长deadline、截止>=改>、amount窄化i32）均编译后通过原14项测试，证据见 `docs/audits/sgm-8-5-balance/b2-c2-audit-report.md`。原实现尚未发现对应缺陷；问题是错误修改不能被测试捕获。另有独立浏览器门禁91/1，Firefox-mobile点击启用后密钥展示未隐藏，根因待独立确定复现。

## 2. 根因

F1a：profile入口测试只用即时响应，没有阻塞等待；低层预算PBT绕过入口。F1b：finish可再推进40s，缺少原end约束。F2：现有到期用例要么真实时钟已越界，要么预算始终阻塞，未触及空闲且恰到期的先Ready分支。F3：精确请求体只用17，cap正例只计数。

## 3. 交叉核对

以已批准细化 §5.3、T-B4/T-B5/T-B15 为权威；保留现有 ManagementTransport 的公开入口、真实 Fake HTTP、共享管理预算与原始错误证据。此次不改变第三方API行为；此前逃逸patch就是修复后的反向验证输入。

## 4. 修正方案

- 新增不会推进虚拟时间的 `finish_at` 测试helper：只在已到原end后让任务完成；有有限poll次数，不能无限等，也不能用“最终完成”替代原截止。
- 两个公开profile入口分别覆盖业务预算、Pending refresh锁、Pending登录预算阻塞，检查原end退出、NotSent、无HTTP与槽位归还。
- 初始Pending、64认证等待者、等锁者、401原截止等待使用精确结束判定；已准入HTTP/登录仍允许按原10s窗口完成，保持原测试。
- 从公开until入口验证空闲预算的截止前、恰到期、截止后；恰到期和之后必须NotSent/QueueFull、零handler，正例证明fixture有效。
- 扩充合法大金额/大ID矩阵，逐请求核对完整JSON和入口计数，保留原非法输入、guard和连接失败断言。
- 浏览器V1先用独立合成fixture确定复现；未证实前不改产品、不放宽断言/时限或以重试刷绿。确定根因后在本文补方案与回归证据。

## 5. 正确性与无回归论证

新helper不改变生产代码，只收紧观测时间；100000次yield上界避免错误变异导致挂死，冻结时钟保证断言不受墙钟调度影响。profile矩阵沿实际公开入口及真实预算/认证路径验证接线。恰到期测试区分>=与>；大整数JSON等值直接对照输入，不经过窄化变换。旧PBT和所有断言保留，不要求所有路径新建大型随机状态机。

## 6. 用例与变异

保留14个原测试及128例PBT，新增公开profile等待矩阵和公开until截止分区测试；加强原Pending/64槽及payload用例。四项逃逸必须在独立源副本与独立target中编译后失败，恢复后基线通过。全量Rust默认并行；Node/契约/SGM7/浏览器原full门禁必须执行，保留失败材料。

## 7. 实施清单

Rust侧修改 `src/newapi/transport/balance_tests.rs`，调用层生产行为不改。V1实际修改 `src/panel_http/ui/{app,render}.mjs`、永久浏览器回归及精确清单/Node fixture，方案与确定复现见下文。独立subagent复核测试与变异，提交后按C2停点交审，不进入B3。

### V1 根因与局部修复补充（实施前确认，2026-11-02）

独立合成Firefox实例已确定复现：在启用按钮的pointerdown中触发真实 `queryUsage` 或纯身份数据重绘，`render.guard` 的 `replaceChildren` 移除了仍在激活中的按钮；随后click落在DIV，Playwright click正常完成，但 `app.action` 未调用。控制场景click→action→secret隐藏正常，直接busy时调用action也先清secret，故不是readOnly分支漏清。原失败trace与同机制吻合，但不把缺少pointer事件的旧trace冒称完整因果证据。复现材料 `/tmp/sgm8-b2-browser-probe-results.json`。

`button:active` 的更短候选经实测不适用：Firefox Space和Chromium可信touchStart时不匹配，仍会丢节点。采用经过双引擎验证的单引用方案：renderer最多保留一个 `{button,kind,id,released}`；primary pointer和Space/Enter在已启用按钮上开始记录。只有包含该按钮的guard暂不重建，也不更新stamp。正常click capture仅清引用，真实事件继续分派；不在pointerdown执行动作，不缓存旧view model/业务请求。up标released；cancel、drag、焦点离开、window blur释放；无click的释放由现有tick开头清理后正常paint。suspend清引用，destroy移除新增监听，不增加第二个timer。

`app.action`/readOnly/work协议保持：授权看当前状态，busy/stale/dry_run不能因保DOM而获得写权限；busy时明确动作仍能清secret，但不会开写确认或排队。取消手势零action，下一paint能显示最新数据。改动限render.mjs、app.mjs三个生命周期/pump挂点与回归测试/精确清单。双引擎确定实验覆盖pointer+data/work、Space/Enter、拖动、blur和stale/dry_run，原guard回退会明确失败。新增真实浏览器回归应复现中途重绘，检查未提前激活、真实click到达、取消不激活及刷新最终完成；原密钥隐藏断言和时限保留。

关联调度边界：仅保DOM仍可能让周期work抢在已开始的手势前占槽，违反原面板§5“work周期任务需让位给显式用户动作”。在tick完成settle/secret过期/paint/state检查之后、选择新work之前，用单引用的只读getter判断正在交互则本轮不启动新周期work。已在途work和显式queryUsage不受此判断影响，不消费due/desired，不抢占、不排队、不改action权限。双引擎4场景/60断言证实state/expire继续、周期work延后、手势后下一tick恢复；显式work仍使确认被拒且secret清理，见 `/tmp/sgm8-b2-yield-probe-results.json`。

## 8. 文档同步

保留原失败审查报告为历史；新增修正执行记录，同步 expectations、细化§10及handoff的当前状态。只把已运行的证据写为通过，明确B2覆盖与整个第5批PBT范围的区别。

### 执行结果（2026-11-02）

已完成Rust补测与V1局部交互修复。focused16/0；Rust全量默认并行803/0（35既有ignored）；Node60/0，check:ui/SGM7通过；浏览器原full一次96/96。七项Rust变异均编译后被精确断言捕获、恢复16/0；永久浏览器case分别杀死DOM保护和周期yield回退，两引擎最终恢复partial基线通过。历史Node fixture接口失败和CDP清理失败均保留并记录修正原因，未放宽原断言/时限。证据与独立审查见 `docs/audits/sgm-8-5-balance/b2-c2-fix-review.md`。

本方案随修正提交保存，具体SHA见该提交与交接记录；本地提交后按C2固定SHA复核，未推送或部署。B3–B8未开始，整个第5批PBT不据此宣称完整。
