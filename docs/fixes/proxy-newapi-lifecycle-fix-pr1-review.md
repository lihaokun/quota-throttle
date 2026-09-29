# 修正方案 — PR #1（缓存池 / 渠道生命周期）合并前 review 修复：H1 / H2 / M1 / M2

> workflow §7 流程。来源：2026-09-22 对 `LittleS321/quota-throttle#1`（负载均衡 + 临期优先，
> commits 675ba3c..796d189）的合并前 review。四条经核实的正确性缺陷 + 测试不稳定，由维护者
> 修后合入；其余发现开 issue 跟踪。代码 commit：**f078d37**（proxy/router）、**d2e33ca**（newapi/orchestrator）。

## 第一部分：现象与复现

| # | 现象 | 触发条件 / 频次 | 出错代码路径（PR 头 796d189） | 预期 vs 实际 |
|---|---|---|---|---|
| **H1** | 全员限速时客户端收到合成 `502 "全部候选渠道转发失败"`，而非上游 429（含「已达到…使用上限」原文） | **无缓存命中 + 合格渠道 < 3 把**（1–2 把 key 的部署，每个新对话首请求）；必现 | `proxy.rs:449-456` 评分阶段：非预算打满时 `resp.bytes()` 排干后丢弃 → `tried.push` → 下轮 `choose` 得 `NoEligible` → `break` → `final_resp=None` → `proxy.rs:486` 合成 502 | 预期：交回最后一次上游响应（循环注释「中途候选耗尽 → 交回最后响应」）；实际：合成 502，429 体丢失 |
| **H2** | ① new-api 重启后 10s 冷却窗口内面板 channels=0 / quota=-1（F0 声称已消除的症状）；② 该窗口内点「弃用」→ **健康渠道被打弃用标志**，下次启动对齐删渠道 | 重登失败或冷却中（10s 窗口）/ root 密码改坏（持续）；② 需用户在窗口内操作，低频但后果静默 | `newapi.rs:273-278` `send_authed` 重登失败时 `Ok(resp)` 原样返回 401；`list_channels` 对 `{"success":false}` `.json()` 成功、`extract_items` 无 `data` → `Ok(空 map)`；`orchestrator.rs:629-641` `still_exists = m.values().any(..)` 对空 map 得 false → 放行弃用 | 预期：401 恢复不了 → 管理调用报错（`main` 的 `remove_key` 是硬失败）；实际：读接口返回空集，闸门被绕过（绕过口是 PR review #5 新加的） |
| **M1** | 面板「恢复」报「渠道已建好，但在 new-api 里解析不到它的 id」；下次启动后该 key 仍显示弃用、刚建的渠道**被删除** | 恢复流程建渠道后 `list_channels` 恰好失败（`/api` 限流 429 的非 JSON 体 / 网络抖动）；低频 | `orchestrator.rs:731` `.ok()` 吞掉失败 → config 仍 `deprecated=true` 无 `channel_id` → 下次启动 `plan_channel_ops`（`newapi.rs:114-121`）对弃用 key 的同名渠道发 `ChannelOp::Delete`，`sync_channels:746-752` 无条件执行 | 预期：瞬时失败重试；仍失败则明说「config 未改、渠道已建、再点一次恢复」；实际：一次抖动 → 恢复静默回滚 |
| **M2** | 管理员在 new-api UI 手删一个受管渠道后：该渠道上的对话全部 400；评分最高时**所有新对话**撞 400，直到重启 | 运行中外删渠道；低频但波及面 100% 新对话 | rc.20 `middleware/distributor.go:47-52`：指定渠道 `GetChannelById` 失败 → **400**（非重试码）。`proxy.rs:467-472` `Ok(resp)` 兜底臂不看状态码即 `record()` → 对话钉死；探针合格集不知渠道已删 → `choose` 继续选它；400 不 `cool()` 无自愈 | 预期：旧拓扑下 new-api 自动跳过已删渠道（无感）；实际：代理拓扑下变成硬失败 |
| **flaky** | `cargo test proxy::` 4 个集成用例 ~30% 概率随机挂（`left: 502 right: 200` / `left: 429 right: 200`） | 并行或串行均可复现；12 轮 3 挂 | `proxy.rs` tests：mock 上游单次 `read` 即回包、关连接（客户端可能还在写体 → RST），且 9 个 mock 响应均无 `Connection: close`（reqwest 复用已关连接） | 预期：确定性；实际：连接错误被当成换道信号，断言随机失败 |

**最小复现**（均已固化为回归测试，见第六部分）：H1 = `无命中_两渠道全429_交回上游429而非502`；M2 = `非重试错误_原样交回_不记池不换道` + `快照提取_按渠道存在性与启用状态过滤`；H2/M1 为管理面路径，靠代码走读 + 第五部分论证（无 mock new-api 基础设施，e2e 留待真环境）。

## 第二部分：根因分析

- **H1**：症状是 502；根因是评分阶段「留住响应」与「换道」两个动作耦合在预算判断上——只有预算打满分支存 `final_resp`，候选耗尽分支没有对应处理。修复消除根因（每个可重试响应先存再换道），非绕过。
- **H2**：症状是面板空 / 误弃用；根因是 `send_authed` 的错误边界不完整：把「401 且恢复失败」当成普通响应交给业务解析层，而业务解析层对 `{"success":false}` 的处理是「无 data = 空」。第二根因：冷却判定只看时间不看会话代次，并发 401 的输家即使别人已重登成功也拿裸 401。修复消除两个根因。
- **M1**：症状是恢复回滚；根因是「渠道已建、config 未落」的撕裂态没有任何补救路径（AddChannel 不回 id，只能列表解析；列表失败即放弃）。修复：重试缩小撕裂概率 + 报错明示恢复路径（再点一次恢复走「删残留再重建」自愈）。**注**：不是根因消除（撕裂态仍可能出现，只是极低概率 + 可见 + 可自愈），因 rc.20 `AddChannel` → `BatchInsertChannels` + `ApiSuccess(nil)` 不返回 id，无法在建渠道原子操作内拿到 id。
- **M2**：症状是 400 钉死；根因是代理的可用性视图只有探针（智谱额度）一个来源，对 new-api 侧渠道实况（存在 / 启用）零感知，而指定渠道机制恰恰把「渠道不存在」暴露成一个非重试码。修复在视图层补上渠道实况（面板 5s 刷新的 `channels` 表），并把 `record()` 收紧到 2xx。
- **flaky**：根因是 mock 不遵守 HTTP/1.1 连接语义（不读完请求、不声明短连接）。

## 第三部分：参考实现对照

- **new-api rc.20 `middleware/distributor.go:35-56`**（已拉源码核对）：`ContextKeyTokenSpecificChannelId` 存在 → `strconv.Atoi` 失败 / `GetChannelById` 失败 → `abortWithOpenAiMessage(400, MsgDistributorInvalidChannelId)`；`Status != Enabled` → **403**。⇒ 400 是渠道级信号，但 400 同时也是 malformed 请求的码（PR review #4 因此把 400 移出重试矩阵，正确）。两者无法从状态码区分，只能靠「渠道是否存在」的旁路数据判断——这决定了 M2 修在 `RouteView` 而不是 `retryable()`。
- **new-api rc.20 `controller/channel.go` `AddChannel`**：`model.BatchInsertChannels` 后不返回 id ⇒ M1 只能按名再列。
- **本机 `.newapi/one-api.db`**：`tokens.name` 仅普通索引 `idx_tokens_name`；`channels` 为 `PRIMARY KEY (id)` 无 AUTOINCREMENT（与本次修复无关，已记入 issue）。
- **`main` 分支 `orchestrator.rs:537-565` `remove_key`**：`set_channel_priority` 失败 = 硬失败无兜底 ⇒ H2 的绕过口确为 PR 引入。

## 第四部分：修复方案

| # | 修什么 | 为什么这样修 |
|---|---|---|
| H1 | `proxy.rs` 评分阶段：`final_resp = Some(resp); tried.push(id); if score_sends >= SCORE_SENDS { break }`——先存再判预算，不再排干 | 候选耗尽 / 预算打满两条出口都能拿到最后响应；不排干只损失一条连接回池（重试路径，可忽略） |
| H2 | `newapi.rs`：`AuthState.generation`（登录成功 +1）；`send_authed` 记发请求时的代次，401 后 `try_relogin(seen_gen)`：代次已变 → 直接用新会话重试；否则冷却检查 + 登录；**任何未恢复的 401 一律 `bail!`**（Token 模式同样）。`orchestrator.rs` `deprecate_key`：`m.is_empty() \|\| any(..)` 空表也按「还在」 | 错误边界补全：401 不再流入业务解析；并发输家不再吃冷却。闸门加固是纵深防御（send_authed 修后 list 已不会返回空，但空表本身就不是「渠道没了」的证据） |
| M1 | `newapi.rs` 新增 `resolve_channel_id_by_name`（0s/1s/3s 三次）；`add_key` / `restore_key` 改用之，失败文案明说 config 状态与恢复动作 | 见第二部分；恢复路径「再点一次」由 `restore_key` 步骤 ②（删残留再重建）天然承接 |
| M2 | `router.rs` `RouteView::from_snap`：`channels` 非空时 `eligible ∩ {c.id \| c.enabled}`；`proxy.rs` `Ok(resp)` 臂仅 `is_success()` 时 `record()` | 在选路前剔掉不存在 / 禁用的渠道（`choose` 的 pin / 命中 / 评分三层都以 `eligible` 为基，一处过滤全覆盖；affinity 等待期的 `still_eligible` 复用同函数也随之生效）；`channels` 为空不过滤，退回探针合格集（面板拉取失败不能让代理拒绝服务） |
| flaky | tests：`read_request`（读到 `\r\n\r\n` + Content-Length 体）、`mock_response`（`connection: close`）、`auth_line`；5 个 mock 统一改用；抽 `two_channel_fixture` 供新回归用例 | 遵守 HTTP/1.1 语义，每请求新连接，确定性 |

**修改后的预期行为（复现走一遍）**：H1 两渠道全 429 → 发送 2 次 → `NoEligible` → 交回第 2 次的 429（体原样）。H2 冷却窗口内 `list_channels` → `Err("列出渠道失败: HTTP 401（管理会话失效，重登未成功：…冷却中…）")` → 面板 `unwrap_or_else` 记 debug 保持上一轮值、`deprecate_key` 硬失败「未做任何改动」。M1 列表 429 → 1s 后重试成功 → 正常落 config。M2 面板 5s 内刷新 `channels` → 已删渠道出视图 → 命中查询 `eligible.contains` 失败 → miss → 评分选别的渠道 → 成功后 `record` 迁移。

## 第五部分：正确性论证

- **根因消除**：H1 / H2 / M2 / flaky 见第二、四部分；M1 明确标注为「缩小 + 可见 + 可自愈」而非消除（上游 API 限制）。
- **不变量保持**：
  - `route_llm`「客户端收到的要么是某次上游响应原样、要么是无任何上游响应时的合成错误」——H1 修后更严格成立（此前候选耗尽分支违反）。
  - `RouterState` 池条目「只指向曾成功服务过该对话的渠道」——M2 修后成立（此前非 2xx 也写）。
  - `send_authed`「返回 `Ok` ⇒ 响应非 401」——新不变量，H2 修后成立；所有 14 个调用点均只在 `Ok` 后解析 JSON，无调用点依赖拿到 401 响应。
  - 「config.toml 唯一数据源」「面板出错不影响切换循环」「代理不因面板数据缺失拒绝服务」（`channels` 为空不过滤）均保持。
  - `deprecate_key` 闸门语义「查实渠道确实没了才放行」——H2 修后对 `Ok(空)` 也成立。
- **无回归**：
  - H1：全渠道 429 既有用例（`全渠道429_预算打满_交回429且无透传多发`，期望 4 次发送 + 429）仍过；差别仅在交回的是最后一次（渠道 2）而非命中阶段（渠道 1）的 429，与注释语义一致。
  - H2：200 路径逐字节等价；Token 模式 401 由「返回空集」变「报错」——这是修正而非回归（配置错误本就该报错）。10s 冷却语义不变。
  - M1：成功路径多 0 次等待（首次不 sleep）；仅失败路径多 4s。
  - M2：`channels` 为空时行为与修前完全一致；非空时只减不增合格集，且只剔除 distributor 必回 400/403 的渠道。既有 `快照提取_fields映射` 用例（channels 空）仍过。
  - 全量：102 用例通过（PR 头 97 + 2 flaky + 3 新增）。

## 第六部分：测试用例清单

| 类型 | 用例描述 | 状态 |
|------|---------|------|
| 回归 H1 | 无缓存命中 + 两合格渠道全 429 → 交回上游 429 与原始体，发送恰 2 次，不透传客户端 token，不记池 | 已加：`proxy::tests::无命中_两渠道全429_交回上游429而非502` @ f078d37 |
| 回归 M2 | 非重试码 404 原样交回、不换道（发送 1 次）、不记池 | 已加：`proxy::tests::非重试错误_原样交回_不记池不换道` @ f078d37 |
| 回归 M2 | `from_snap`：渠道在且启用留下、在但禁用剔除、不在表里剔除；`channels` 为空不过滤 | 已加：`router::tests::快照提取_按渠道存在性与启用状态过滤` @ f078d37 |
| 回归 flaky | 原 4 个集成用例改 mock 后 `--test-threads=8` 12 轮 0 失败（修前 6 轮 3 挂） | 已验：f078d37 |
| 新增 H2 | 401 且重登失败/冷却 → `send_authed` 返回 Err；并发 401 输家用新会话重试成功 | 待加：需 mock new-api 管理面（现无此基础设施），e2e：`run` 中 kill new-api + 立即重拉，观察日志「重登未成功」不再伴随 channels=0 |
| 新增 M1 | 建渠道后 list 首次 429 → 1s 后重试成功 | 待加：同上需 mock 管理面；逻辑为纯重试循环，走读覆盖 |
| 举一反三 | `add_key` 与 `restore_key` 同款 `.ok()`（前者后果轻：留野生渠道）——已一并改用重试解析 | 已改 @ d2e33ca |

## 第七部分：代码更新清单

| 文件 | 函数 / 行号 | 改动概述 | 状态 |
|------|------------|---------|------|
| `src/proxy.rs` | `route_llm` 评分阶段（原 449-456） | 先存 `final_resp` 再换道，不排干 | 已改：f078d37 |
| `src/proxy.rs` | `route_llm` `Ok(resp)` 臂（原 467-472） | 仅 2xx `record()`，否则 debug 日志原样交回 | 已改：f078d37 |
| `src/proxy.rs` | `retryable` 文档注释 | 说明 400「渠道不存在」由视图层过滤 | 已改：f078d37 |
| `src/proxy.rs` | tests：`read_request` / `mock_response` / `auth_line` / `two_channel_fixture` + 5 处 mock + 2 新用例 | 修 flaky + 回归 | 已改：f078d37 |
| `src/router.rs` | `RouteView::from_snap` | 按 `snap.channels`（非空时）过滤 `eligible` | 已改：f078d37 |
| `src/router.rs` | tests 新增 `快照提取_按渠道存在性与启用状态过滤` | 回归 | 已改：f078d37 |
| `src/newapi.rs` | `AuthState` + `new()` | 加 `generation` | 已改：d2e33ca |
| `src/newapi.rs` | `send_authed` / `try_relogin(seen_gen)` / `do_login` | 401 未恢复 bail；代次判断；登录成功 +1 | 已改：d2e33ca |
| `src/newapi.rs` | 新增 `resolve_channel_id_by_name` | 三次退避重试 | 已改：d2e33ca |
| `src/orchestrator.rs` | `add_key` / `restore_key` | 改用重试解析；失败文案明示恢复路径 | 已改：d2e33ca |
| `src/orchestrator.rs` | `deprecate_key` `still_exists` | 空表按「还在」 | 已改：d2e33ca |

## 第八部分：文档更新清单

| 文档路径 | 要改什么 | 状态 |
|---------|---------|------|
| `docs/fixes/newapi-401-relogin.md` | 第四部分「失败/冷却中则原样返回 401 响应」→ 改为报错；第五部分并发段补代次机制 | 已改（本 commit） |
| `CLAUDE.md` | 「会话失效自愈」条目补「401 恢复不了一律报错 + 代次」；「缓存池代理」条目补「代理视图按 channels 过滤 + 只 2xx 记池」 | 已改（本 commit） |
| `docs/design/cache-pool/architecture.md` | §2「缓存命中（渠道合格且未冷却）」与 §3「≤2 次换道」已与实现（冷却不挡命中 / 最多 6 次发送）漂移——**不在本次范围**，随 issue 跟踪 | 待改（issue） |
