# SGM5 C 设计与审核处置

2026-10-18设计开始，2026-10-19完成实施验收，基线`bc8f33e`。保留设计/实施阶段的处置过程，最终结果见[c-audit](c-audit.md)。

1. 保留原架构：group.ready指注册/授权/成员路由配置已核验，允许禁用成员引导旧策略；真正正向token操作另查当前许可与status=1，避免首次启用循环。
2. 不新增v9或组删除/换类型服务。引用报告只覆盖已知本地引用，含未决创建与空/auto未知范围，恒不授权删除；后续页面不能把零计数当全原生无引用。
3. 固定源码与新临时实例实证两个倍率别名缓存互不跟随。新组只从语义一致且与注册键集合一致的基线有界双写、完整回读；预先歧义和崩溃后无证据歧义都拒绝自动修复。已注册组不为整理别名改倍率。special权限作用于account_group，不宣称user_id级；不动全局usable及已有倍率。
4. 独立设计审发现资格失败未说明note节流、Enable缺少写前固定身份/spec校验、prepare自己的epoch推进未明确，均在编码前补齐。Waiting+Never保留显式retry边界；不给未知创建放宽重发。
5. 原transport在预算mutex内调用guard，因此不允许C的guard取Core查SQLite。沿原原子publication fence：同Core prepare提交后核B发送资格，固定epoch+2及窗口；任一状态写先封旧fence，最终guard只读原子与时间。这是B最终校验的等价组合，不另存许可权威。
6. 超窗处理仅新增B.note_token_delay窄入口：同状态/版本、无倒退时固定NotReady刷新调度，不接收远端观察或返回新Work，不能绑定/成功/解unknown。历史查回不要求未来expiry或活跃来源；未配置TokenOwners时run也不计算token唤醒。
7. prepare后的失败不能丢票据：同Core返回ticket及check_result，锁外立即建立drop guard，再处理零发送NotSent；poison闭锁等待B恢复，不能在持Core时析构重锁。另明确check_group(register=false)仍是本地写、dry_run拒绝；shutdown沿原预算等待边界，不冒称10秒总上界。

设计审结论：独立审核者先从架构/A/B抽取十节、后审C稿及上述修订，§4.4六项通过，0个未关闭设计阻断；据此进入后续实现和独立验收，不以设计审代替动态测试。

8. T5C.1实施时发现partial后同目标已出现在registered，不能因此豁免写注册的一致基线。独立复核同意：register_for入口始终核两alias与注册集合一致，即使目标已存在；纯register=false只读路径不变。设计/expectations先同步，补首写成功次写失败后同目标重试零PUT回归，不新增状态表。
9. 首次定向编译暴露rusqlite不支持直接FromSql到usize；引用计数改用有界u32（SQL转换自带负数/溢出拒绝），schema值域不变。该轮未运行测试，不计作测试失败或通过；保留初轮日志。
10. 组配置核验必须排除同组未知启用渠道；独立复核同意复用完整scan_group(group,None)，因为Some(required)可提前返回。要求完整分页、已知绑定ID齐全、所有启用渠道身份/单组/type/models一致；未知禁用不计就绪也不接管。正向单token只复核候选依赖此核验与原无外部写者前提，不新增后台扫描。
11. TokenOwners的issuer与legacy改为独立可选、至少配置一方。独立复核确认这是保留旧key管理的最小兼容细化；缺普通签发身份不回退管理员，不改变成功操作历史读取/retry，未完成缺身份操作仍按NotReady节流。测试覆盖legacy-only/issuer-only/全空与绝无管理员创建。
12. worker四项组合红例成功编译后0/4失败，修正后4/0通过；同类Create历史owner入口另补回归。扩展测试首次编译的路径属性使子模块从worker目录解析，补显式相对path，不改断言；该次编译失败不计入红例或变异捕获。
13. 最终独立静态复核十节/五维度没有未关闭问题；审核者未代跑测试。14类编译变异全部被断言捕获，还原副本与工作树对照一致，42项正例再通过。普通测试、性能与实际原生组合的结果仍分别记录，不能互相替代。
14. clippy首轮无错误，新增测试两个if同排和回调类型复杂度提醒：只将独立if分行，并给原Arc回调命名类型别名，未改条件、断言或运行逻辑。生产positive_admission含self共8参的风格提醒保留，不加allow、不为此新增包装/模式层；独审认可职责清楚且保持最小改动。基线7条提醒不在本批清理。变异完成后只有上述两项测试可读性清理，最终全量/机械/clippy使用清理后源码。
15. 首轮默认全量644/1/25 ignored，唯一失败为旧大组操作5秒计时；单项三次1.10/1.13/1.17秒通过，第二轮又观察同用例失败后中止并留证。按已确认的性能分离方案补漏，先写[八节修正](../../fixes/catalog-fix-group-operation-performance-gate.md)并独审再改测试编排。共享case保留原夹具、计时点和全部值断言，原5秒同时约束独立debug/release；不是放宽预算或减少默认并发。C本体仍45入口，另加1个旧用例性能入口；总性能23 release+该项debug，总ignored26。没有额外生产源码修改，最终结果另验。
16. 最终默认debug/release各645 passed/0 failed/26 ignored，23项release性能及大组debug预算、机械/clippy/build、隔离原生8项组合全部通过。旧计时另1类零预算编译变异已捕获并恢复；原两项opt-in未重跑，远端CI未触发。C内部服务退出，不宣称页面或运行部署完成。
