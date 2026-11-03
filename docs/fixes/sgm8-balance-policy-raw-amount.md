# B7 F1 未编辑金额回环修正（2026-11-03）

## 1. 问题与授权

最终独审者 `/root/balance_final_contract_audit` 确认并由主任务授权最小修正：USD比例500000，合法策略warn=100001/target=200003预填0.20/0.40，仅改开关或直接保存却发出100000/200000；合法策略1/2预填两个空串，关闭auto反而被输入校验阻止。本次不得改Rust/Cargo或产品wire。

## 2. 根因

openBalancePolicy用向下截到分的纯数字文本作为可编辑预填；旧buildAction无条件把全部USD文本重新换算。显示精度不足以回到任意合法raw整数，因此把“未编辑的原值”当作新输入会静默改金额，低于一分时还会将合法原值误作无输入。

## 3. 最小修复

仅改 `src/panel_http/ui/core.mjs` 的setBalancePolicy分支：通过原单位冻结与revision guard后，逐字段比较提交文字与 `balanceDollarsFloor(冻结raw,冻结unit)`。文字相同（含原预填空串）或非USD单位时保留冻结raw；仅文字不同的字段调用现有精确quotaFromDollars。最终金额不等式检查仍执行。app预填、readonly、disabled、context、schema/wire均不改变。

## 4. 正确性与边界

- 未改金额的checkbox-only/no-op使用原合法整数，避免显示舍入污染；一个字段修改不牵连另一个字段。
- 原预填空串的合法小金额可以保留；原预填非空而用户主动清空时仍视为编辑并报ActionError。
- 改动后又回到原展示文本时保留原raw，是本次明确允许的文字相等策略，不引入额外dirty标志。
- 单位或policy revision在打开后变化仍先拒绝，不能利用保留分支绕过冻结guard；非USD/null保持原disabled与原raw规则。
- 金额真被编辑时仍用BigInt规范十进制半上舍入，所有原格式/边界限制不变。

## 5. 改动清单

- `src/panel_http/ui/core.mjs`：逐字段policyAmount选择，附原因注释。
- `tests/ui/balance.test.mjs`：在原`balance submit success binds body and shows result`内扩展真实openBalancePolicy→submit→route/requestJson/validateReply回归，保留原8名称及总68 Node。
- `scripts/contract_audit/sgm6_b_mutations.mjs`：新增`balance-policy-reconverts-untouched-amounts`永久变异，撤回仅USD未编辑字段保留逻辑。
- 没有改Rust、生成contract、schema或其他产品源码；`/tmp/balance-b7-policy-roundtrip.patch`记录相对修前core的最小增量，修前core另保留在`/tmp/balance-b7-core-before-policy-roundtrip.mjs`。

## 6. 永久回归与修前失败

真实打开表单后从renderer接收的字段初值生成提交：100001/200003和1/2均覆盖开关true（no-op）与false（checkbox-only）；分别仅改warn或target；小金额仅改target；主动清空非空warn；打开后单位/版本变化。成功校验精确DTO及不进入pending，错误校验零请求及原框提示。

修前运行指定测试实际0通过/1 AssertionError：`/tmp/balance-b7-policy-roundtrip-before.log`，原值100001/200003被错误发送为100000/200000。未覆盖或删除失败证据。

## 7. 修后验证

- `/tmp/balance-b7-policy-roundtrip-node.log`：完整Node **68通过/0失败**，无新增隐形测试名。
- `/tmp/balance-b7-policy-roundtrip-check.log`：check:ui通过，生成契约、精确清单和DOM sink检查通过。
- `/tmp/balance-b7-policy-roundtrip-mutations.log`：余额永久变异 **8/8** 模块加载成功、每项一个指定语义失败、恢复重跑通过。证据目录 `/tmp/quota-throttle-sgm6-b-mutations-bvkJjx`；新变异明确命中精确原raw断言，而非TypeError或协议异常。
- git diff --check通过。当前默认JS变异总数25，本轮只宣称8项余额子集执行；历史全量23/24记录不被改写。
- 本次没有运行Rust/Cargo；完整112项真实Rust fixture浏览器门禁由主任务在此UI源码冻结后执行。

## 8. 独立复核与交付边界

原最终审查者已独立运行真实open→submit指定测试1/0，并独立验证56例未编辑回环矩阵：比例5e-324、1e-7、7.3、50、500000、1e20、1e308 × 策略1/2、100001/200003、近cap、默认值 × 2开关，全部通过；其消息明确F1可关闭、无新增问题。该独立结论来自审查者，不以作者自测冒认。最终契约正文§5.8.2/§6说明由主任务同步。

未提交/推送/部署、未读取生产config/DB/密钥或访问生产服务。本报告为F1修复闭环，不替代固定SHA C2与主任务最终完整门禁。
