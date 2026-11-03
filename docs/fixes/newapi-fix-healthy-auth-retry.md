# 管理操作重试不应重登健康会话

源码基线：`2c65f20`，修复前方案：`f644310`。来源：2026-10-15 外部 SGM 审核的 SGM-2 F1。状态：用户确认后已实施并验证，代码提交 `3b1bbae`；没有修改运行服务。按 workflow §7 修复现有认证恢复行为，不新增子系统。

## 1 现象与复现

`ReconcileWorker::retry` 对非 succeeded 操作先重排本地意图、清观测缓存，再调用 `NewApiClient::retry_management_authentication`。后者转交 `ManagementTransport::retry_authentication`；修复前该入口不检查是否熔断，直接清 failed 并调用 refresh_session。

在正常 Session、generation 未变化、没有并发刷新时，每次操作重试都会额外 POST `/api/user/login`。操作可能只是渠道漂移、管理限流或写入结果未知，与认证无关。额外请求消耗共享 HTTP 预算，也可能触发服务端独立的登录限流；没有通过真实服务压测这一风险。

最小回归用例可追加到 `src/newapi/transport/tests.rs`，直接复用现有 Fake、Reply、read：

```rust
#[tokio::test]
async fn healthy_session_retry_does_not_login_again() {
    let fake = Fake::start(|call| match call.path.as_str() {
        "/api/user/login" => Reply::json(json!({"success":true,"data":{"id":42}})),
        "/api/channel/1" => Reply::json(json!({"success":true,"data":{}})),
        _ => panic!("unexpected fake endpoint"),
    }).await;
    let client = fake.transport(false);
    client.send(read(), RequestClass::Control).await.unwrap();
    for _ in 0..3 {
        client.retry_authentication().await.unwrap();
    }
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|r| r.path == "/api/user/login").count(), 1);
}
```

执行入口：`cargo test --locked --offline healthy_session_retry_does_not_login_again`。该用例已加入源码并先在未修实现上实际运行：断言失败，登录次数为 4、预期为 1，退出码 101。修复后保持同一断言通过，初始化登录 1 次、显式重试新增 0 次。

范围不包括 Token 模式登录：当前 Token 分支本来就不登录；成功历史操作也已在 worker.retry 提前返回。不能将上述现象描述成所有认证模式下每次 retry 都必然登录。

## 2 根因分析

直接根因是将“允许显式解除认证熔断”实现成“每次显式操作重试都刷新认证”。操作是否需要重新观察，与传输会话是否已经失败，是不同状态，只有传输层掌握当前认证事实。

`refresh_session` 的 generation 判断用于合并同一次过期会话的自动恢复，不是健康检查。retry 取当前 generation 后调用它，未发生并发刷新时 generation 相等，因而不会早退。

现有测试验证了认证失败熔断、Token 不登录、显式恢复和 bootstrap 不重复 setup，但缺少“健康 Session 下非认证重试不登录”的断言；worker 重试夹具主要用 Token，未覆盖该入口的会话模式。

不能只根据 OperationView.error_code 决定是否恢复：多个操作共用一个 transport，当前认证状态可能与某个历史操作的错误类型不同。修复应落在 transport 的恢复入口，不给 worker 增加第二份认证状态。

## 3 参考行为对照

这不是供应商路由或数值算法缺陷，按 workflow §7.1 对照已有内部契约，不另引入外部认证库。

- SGM-2 细化 §5.6 已要求“认证熔断只允许此次显式恢复再尝试一次，失败仍关闭”。条件是认证熔断，而不是任意操作重试。
- `refresh_session` 已有 refresh 单飞锁、generation 复核和共享预算；应复用，不另做会话管理器。
- `token_401_never_logs_in_and_opens_circuit` 与 `failed_login_latches_until_explicit_retry_and_does_not_setup` 是必须保留的现有行为。

同样的健康 Session 输入，修复前 retry 发登录；修复后保持会话/代次不变，返回成功，不发送 HTTP。真正 failed 的输入才进入恢复路径。

## 4 修复方案

### 行为边界

| 单飞锁内的当前状态 | 显式 retry 的行为 |
| --- | --- |
| failed=false，Session/Token/Pending 任一模式 | 返回成功；不登录、不 setup、不改变 generation。Pending 的首次登录仍由既有 send/bootstrap 负责 |
| failed=true，Token | 清认证熔断，不登录，不使用 root 凭据；下一次业务请求仍由现有 Token 认证 |
| failed=true，Session 或 Pending | 在现有单飞锁下尝试登录一次；成功后恢复会话，失败或取消保持熔断 |

逻辑调用仍先占用既有 64 槽请求限额，满时可返回 QueueFull；“不额外消耗预算”指不产生 HTTP 准入扣费，不是绕开逻辑调用上限。

### 函数与调用关系

保留 worker.retry 的业务顺序、公开签名和错误传播。生产变更限定 `src/newapi/transport.rs`：

1. 从 `refresh_session` 提取私有 `refresh_session_locked(observed_generation, refresh_guard)`，承接其现有锁内逻辑。guard 为对已持有异步锁 guard 的借用；helper 不再次获取 refresh 锁。普通 refresh_session 继续“获取锁→调用 helper”，自动 401 的重放上限和首次登录入口不变。
2. retry_authentication 先调用既有 register_request，再取得 refresh 单飞锁；在短 auth 锁内读取 failed。未熔断立即返回，不调用 helper。
3. 熔断且 Token 时，只清 failed 并返回。熔断且 Session/Pending 时，记录 generation，复用现有 RefreshCancellation 作为显式恢复的取消护栏，再清 failed 并调用锁内 helper。登录成功在同一个 auth 临界区更新 Session/generation 并清 failed，避免登录等待期间旧代次的在途 401 留下熔断标记。
4. 显式恢复护栏覆盖清 failed 之后的预算等待和网络等待。成功才解除该护栏；错误/取消通过已有 fail_generation 保持该代次熔断，不回滚或覆盖较新的成功代次。
5. 从状态判断到登录结果处理均持有同一 refresh 锁；不采用“先清 failed、解锁，再重新排队登录”的窗口。auth 同步锁必须在预算申请或 await 前释放，保持现有 Budget→auth 锁序。

helper 的 pre 是 caller 持有 refresh_guard 且外层逻辑调用受限；post 沿用原刷新结果：generation 已更新则返回成功，failed/Token 拒绝，合法 Session/Pending 最多一次登录，错误不泄漏凭据。所有遍历仍由现有调用/重放上限约束，不引入后台重试循环。

并发恢复成功后，等待中的显式 retry 在锁内看到未熔断，均为 no-op。若登录失败，仍保持 failed；后续另一个显式 retry 可以再尝试一次，不承诺把多个明确的失败重试合并成一次动作。普通后台请求不能主动清除已经存在的熔断来发起登录；已经在途的合法登录成功，以新会话/代次提交结果。

不调整上游调度、NewAPI 限流参数、数据库 schema、接口路径或启动方式；不顺便替换所有 unwrap/unreachable，不内联 SecretRef，不修改字段 no-op 的 outcome 名称。

## 5 正确性论证

根因消除：healthy 分支在登录 helper 之前返回，故非认证操作重试不再产生额外登录。判断使用同一个 transport 的实际 failed，不依赖可能过期的操作错误码。

不变量保持：逻辑调用限额先于异步等待；网络请求仍逐次经过原预算；refresh 锁串行化状态判断和登录，auth 锁不跨 await。成功时同步更新会话/代次/failed，使登录期间的旧失败标记不污染新会话；成功之后的旧 fail_generation 因代次不同而无效。Token 不进入登录 helper；worker 的持久意图、unknown 创建票、成功历史收据和缓存处理均不改。

取消安全：显式恢复在清 failed 前建立代次护栏，取消或失败后只重新锁住同一代次；成功刷新已经更新 generation，旧护栏不能毒化新会话。未熔断返回和 Token 清旗标分支无 await 间隙。

无回归的证据义务：初始化、401 单飞、第二次 401 熔断、Token 显式恢复、setup 隔离、共享预算和 worker 原重试语义必须保留，以下测试通过后才能报告修复完成。

## 6 测试用例清单

| 类型 | 用例与独立断言 | 状态 |
| --- | --- | --- |
| 必须回归 | 上述健康 Session 连续 retry，登录总数保持初始化的 1 次；修复前红、修复后绿 | 已验证：healthy_session_retry_does_not_login_again |
| 入口回归 | Session 模式 worker 对非认证失败操作重试多次；login 增量为 0，仍能重新观察远端修复 | 已验证：f1_worker_session_non_auth_retry_and_success_history_do_not_login |
| 状态保护 | unknown 创建不获得第二次 POST；succeeded 重试保留收据/缓存且不登录；superseded 不重放 | 三项 Session worker 测试及原回归通过 |
| 真实熔断 | failed Session 显式恢复成功一次，后续 retry 不再登录；恢复失败仍 failed，普通请求不再发登录 | 已验证：f1_failed_session_recovers_once_and_failed_recovery_stays_latched |
| 并发 | 健康会话多路 retry 无登录；熔断恢复成功时并发等待者不追加登录；登录失败后仍拒绝普通请求 | 已验证：8 路并发恢复及健康重复调用；另验证失败熔断 |
| 代次边界 | 登录等待期间旧代次失败先到、登录成功后旧失败再到，两者都不能使成功的新会话保持/变为 failed | 已验证：受控注入旧 fail_generation，覆盖两个时间点 |
| 取消 | 等待 refresh、清旗标后的预算等待、登录已发出三处取消；无锁/槽泄漏，不能把失败会话当健康 | 三阶段通过；另验证 64 个等待者上限与释放 |
| Token/Pending | Token 永不 root 登录；健康 Pending 的 retry 不初始化，随后的正常 send 仍按原路径登录；bootstrap/setup 不扩权 | 新身份测试与既有 Token/初始化回归通过 |
| PBT | 健康 Session 下随机重复次数 N∈0..64，至少 128 cases；HTTP 登录增量=0，generation 和 HTTP admissions 不变 | f1_pbt_healthy_retries_preserve_session_and_http_budget，128 cases 通过 |
| 定向变异 | 删除 healthy no-op；放掉 Token 隔离；恢复失败后误清熔断；取消时丢失保护 | 四项均被测试断言捕获，见下表 |
| 总体验证 | 当前所有测试、改动范围检查、clippy、release；旧测试断言不削弱 | 288 passed、1 ignored；范围/格式检查与 release 通过，clippy 仅原 6 条警告 |

测试只使用已有 Fake/临时目录/随机回环端口和假凭据；不访问真实服务或供应商。新增 11 项测试，包括上述 1 项 PBT。全量命令为 `cargo test --locked --offline -j 2 --quiet -- --test-threads=4`，288 passed、0 failed、1 ignored，306.43 秒。原有 ignored 原生 NewAPI 测试本轮未单独运行，不以 fake 代替真实部署验收。

Clippy 指出新增取消测试中的显式 drop 锁作用域不够清晰，已改成独立词法块，断言未变；随后重跑全部 10 项 f1_ 测试与 clippy/release，通过且无新增警告。`rustfmt --check`、`git diff --check` 及基线受保护文件检查通过；业务代码改动仅限认证传输层。

变异使用 `/tmp/quota-throttle-auth-retry-mutations.bpGt0h` 的独立源码和独立 target。正常基线先通过，逐项注入并恢复；以下退出码 101 均来自预期测试失败，不是编译错误。恢复后三个修改文件与开发分支逐文件一致，正常构建产物没有被变异覆盖。

| 变异 | 捕获测试 |
| --- | --- |
| M1 健康会话仍进入登录 | healthy_session_retry_does_not_login_again，4≠1 |
| M2 绕过 Token 分支隔离 | f1_pending_and_token_retry_preserve_initialization_and_identity，Token 身份被错误换成 Session |
| M3 恢复失败后清掉熔断 | f1_failed_session_recovers_once_and_failed_recovery_stays_latched |
| M4 关闭显式恢复的取消保护 | f1_cancelled_recovery_preserves_latch_and_releases_all_resources，预算等待取消后 failed 错误为 false |

本轮按 workflow §7 修复和 §5.2 五维度复核：实现符合已确认状态表；复用原锁/预算/取消护栏；回归与边界断言通过；健康重试不增加 HTTP，等待仍受 64 槽限制；无新类型系统、依赖或数据库。外部独立审核是本缺陷的发现来源，本轮未额外宣称完成新的 §6 独立审核。

## 7 代码更新清单

| 文件 | 修改 | 状态 |
| --- | --- | --- |
| src/newapi/transport.rs | 健康 no-op、复用锁内登录 helper、显式恢复取消护栏 | 已实施：3b1bbae |
| src/newapi/transport/tests.rs | 最小回归、并发/失败/取消/Token/Pending 与 PBT | 已实施：3b1bbae |
| src/reconcile/worker/tests.rs | 复用现有 fixture 增加 Session 模式重试验证 | 已实施：3b1bbae |

没有修改 worker 生产逻辑、NewApiClient 包装、catalog、GLM 或 CLI；没有借本修复扩展可选清理范围。

## 8 文档更新清单

| 文档 | 更新 | 状态 |
| --- | --- | --- |
| 本修正方案 | 根因、函数契约、回归与实施记录 | 已回填 3b1bbae 的实际结果 |
| SGM-2 细化 | retry/refresh 的健康与失败分支及取消范围 | 已同步已实现契约 |
| SGM-2 expectations | 健康 no-op、并发/取消与预算验证项 | 已记录局部验证完成，不勾选整个 SGM-2 退出 |
| 实施准备计划 | F1 完成后回到 SGM-3 后续批次；不改变最终功能范围 | 已同步 |

外部审核的 P1 文档承接另见[目录迁移增量核对](../audits/sgm-1-catalog-store/migration-scope-review.md)。P1 不要求改本修复的生产代码，也不将历史 SGM-1 报告改写成对当前 HEAD 的全量审核结论。
