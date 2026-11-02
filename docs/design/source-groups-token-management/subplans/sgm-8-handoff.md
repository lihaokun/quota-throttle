# SGM-8 梳理轮：交接给 Codex（2026-11-01）

> 用户决定：后续工作由 **Codex 实现**，Claude 改任**监管者**（审查设计与代码、跑独立测试，不改代码）。本文自包含，Codex 只需读本文及其中列出的文件。
> 工作区：`/home/haokun/projects/quota-throttle/.worktrees/source-groups-research`（与 Claude 共享，不要改写历史、不要 stash）。
> 分支：`research/source-groups-token-management`，对应 PR #5（https://github.com/lihaokun/quota-throttle/pull/5）。**本轮后续所有改动都合进 PR #5**，不另开 PR。
> 最新审查安排（2026-11-02用户）：**改用subagent独立审核，不用等Claude**；原Claude审查为历史记录，C1/C2/C3停点仍保留。
> 接手进度：I10/D1已确认，B1的C2已通过。**B2旧提交 `9619729` 的C2意见已修正：后端16项测试及七个反向变异通过；V1确定复现后局部修复，永久浏览器回归能杀死两项回退；全量Rust803/0、Node60/0、浏览器full96/96。** 专项独审与证据见 `docs/audits/sgm-8-5-balance/b2-c2-fix-review.md`。修正随本地新提交交C2，下一步按新SHA复核，再进入B3；旧失败报告保留，B3–B8未实现，未推送或部署。

## 0. 铁律（违反任何一条都要停下来）

- 流程：`docs/workflow.md`。**不确认不实现，不跳过设计直接写码；调研靠查源码 / 实测，不靠猜。**
  新功能：调研 → [用户确认] → 架构 → [确认] → 细化 → [确认] → 审查 → 逐模块实现 + 测试 + 审核。每个 [确认] 都要停下来等用户。
- 回复用户用中文。
- 提交信息**不加** `Co-Authored-By` 等署名。
- 推送只用显式 refspec：`git push origin research/source-groups-token-management:research/source-groups-token-management`（本地 upstream 误设为 origin/main，**禁止裸 `git push`**）。`main` 受保护，只能走 PR。
- 不用裸 `git stash` / `git stash pop`（stash 栈与其他会话共享）；不用 `git checkout` 强行清树。
- 鉴权值不得进日志、输出或提交：智谱 key、火山 key、NewAPI token、root / 管理员 / 签发账号密码、opencode MCP key。配置文件含密码，不要 cat；需要临时文件时用 0600 并在用完后 shred。
- **任何「查不到用量 / 余额」的路径，默认值必须是「未知」而不是 0。**
- fermat 是共享机器：只碰 lihaokun 的文件和进程；**永远不碰** `~/apps/quota-throttle-sgm/{config.toml,state/,native/}`；fermat 上的 3000/3001 端口属于别人，SGM 用 13000/13001。
- 本机：不要动主仓库（`/home/haokun/projects/quota-throttle`）里未提交的 `src/{config,main,orchestrator,status}.rs` 等改动，只在上面的 worktree 里工作。

## 1. 必读文件

| 文件 | 内容 |
|---|---|
| `docs/workflow.md` | 流程规范，尤其 §4.2.1、§4.3.1–4.3.3、§4.4、§5.2 |
| `CLAUDE.md`（worktree 内） | 项目血泪坑 |
| `docs/design/source-groups-token-management/subplans/sgm-8-cleanup.md` | **本轮架构 v6**（v3–v6 均已确认；§2.9 已过 §4.4 第 1 项）。§2 架构变更、§2.6 不变量 I1–I10、§2.8 目标映射、§3 各批文件、§5 假设、§6 实施顺序、§7 审查记录 |
| `subplans/sgm-8-1-small-fixes.md` | 第 1 步细化（已实现，作为写法范例：规则 R、I8、T1–T12） |
| `subplans/sgm-8-5-balance.md` | **第5批 v3 C1 完成**；D1 已确认，§10 处置和最终报告、§11 停点 |
| `docs/research/source-groups-token-management-research.md` 末节 | 本轮调研 |

## 2. 原部署与接手基线（`7e5cf97`，已推送，PR #5 的 CI 三项全绿）

> 2026-11-02 B1 局部进度：余额策略持久化、v8→v9 迁移及回归/PBT 已提交为 `94362c4`，提交后的 C2 独审通过，剩余问题 0。实施和三项变异证据见 `docs/audits/sgm-8-5-balance/b1-implementation-review.md`，C2 见同目录 `audit-report.md`。
>
> B2原实现为 `9619729`，C2发现四类逃逸与浏览器红门禁；这些意见现已按用户要求修正，具体实现、失败留存和全量验证见修正记录。不要把旧14项/92项数字当作当前清单；当前B2补测16项、UI完整96项。提交后仍需固定SHA复核。

- 已完成：架构 v6（`2d48438`）；**第 1 步**（`7e5cf97`）：提交被拒就地反馈（规则 R：带状态码且不是 500 / 504 的错误 = 后端没有记录）、轮询 404 说明、`QueueFull` 警告带 `job`、pb2 断言修正、T1 按 8 类提交钉住 I8。
- 已部署 `7e5cf97`：
  - 本机（3000 / 3001，`quota-throttle-sgm.service`）：bin sha `c7acbfe3…`，回滚用 `~/apps/quota-throttle-sgm/bin/quota-throttle.bak-20261101-194951`。
  - fermat（13000 / 13001）：bin sha `a21866f7…`，回滚用同目录 `quota-throttle.bak-20261101-195427`。
  - 两边 new-api 子进程都未重启（serve 复用已运行的 new-api）。
- 接手时未提交的 `sgm-8-cleanup.md`、`sgm-8-5-balance.md`、本文及 C1 材料，已随 B1 提交 `94362c4` 保存。
- 正在采集：**`QueueFull` 按任务的 24 小时基线**，起点本机 2026-11-01 19:49:52。2026-11-02 19:50 之后执行：
  `journalctl --user -u quota-throttle-sgm.service --since "2026-11-01 19:49:52" --until "2026-11-02 19:49:52" | grep 来源分组维护暂未完成 | grep -o 'job="[a-z_]*"' | sort | uniq -c`
  结果记入第 2 批细化，作为「修改前」基线。
- 本机环境备注：签发账号 `qtsigner` 的 new-api 内部余额于 2026-11-01 被手工直写 SQLite 加到约 1e15（应急）；identity #1 `opencode-glm` 是「从未创建」的身份，删除会被拒（第 4 批的「放弃」解决）。

## 3. 后续工作（按架构 §6 顺序）

### A. 第 5 批：签发账号内部余额（架构 §2.9，记作第 1b 步）

- 细化 v3 技术独审/Step0 文本核对已通过，I10 与 D1 均获用户确认，进入逐模块实现；不能据作者处置或技术结论直接部署/推送。
- 下一步：
  1. **已完成（2026-11-02）**：把 v1 交用户确认。已确认的四处偏差：
     - 快照只加一个根字段 `balance`；
     - `dispatch` 增加 `deadline` 参数；
     - 周期观测用 `Observe` 类请求；
     - guard 为假时报 `not_ready`。
     另有两处细节：改设置时库忙返回 503；锁定期内的拒绝不覆盖「结果不确定」记录。
  2. **C1 已完成**：D1 具体配套已获用户确认；最终报告完整列输入 SHA 与各路结果，不需再等 Claude。
  3. B1的C2已通过；B2的测试缺口和V1已修正，专项独审、反向验证及最终全量门禁通过，修正提交后停C2复核。旧128例PBT仍只证明限定性质，不能冒称整批PBT完整。新SHA通过后才进入B3，继续剩余B3–B8、T1、夹具/清单/PBT与机械项；不直接推送或部署。
  4. 同批更正 `CLAUDE.md` 中「new-api 管理面没有调用户余额的 API」一条（细化 §7）。
- 要点：
  - 补足函数严格按 7 步（含 1b）。锁顺序为「所有者异步互斥 → Core（短暂）→ 槽位」，Core 与槽位不跨 `.await`。
  - 结论按 `DeliveryEvidence` 划分，≥500 一律视为不确定。
  - 封锁在互斥内设、在 1b 查。
  - `add_user_quota` 拒绝 `user_id ≤ 0`。
  - 改设置不经 `mutate`。
  - 快照字段名不能含 `settings`。
- 部署：带 **v8→v9 迁移**，打开库时自动执行并先备份；回滚要换回旧二进制，并用 `catalog-v8-before-v9-*.sqlite3` 恢复。
  - 本机首轮会把 `qtadmin`（约 1e8）补到 1e15。
  - **fermat 首轮会把签发账号从约 1e12 补到 1e15，这是对 fermat new-api 的写，部署 fermat 前必须再问用户。**

### B. 第 2 批：去许可 + 后端检查不阻塞（尚无细化）

- 范围（架构 §1 第 2 项、§2.1、§2.2、§2.6、§3 第 2 批、§6 第 2 步）：
  - 删除 `RoutingPermit` 全链路，许可消费方清单见 §2.1。
  - **资格规则**：只有组决策持久记录的活动成员（GLM `active` / 火山 `preferred`）能高于备用优先级，其余最多到备用优先级。
  - 火山启用指令规则（§2.1）。
  - `ledger_ready` 只删新鲜度那一半。
  - 升级屏障改从配置取备用优先级。
  - **GLM 固定前置条件恢复 main**（§2.1、§2.7 末段）：固定要求成员在最近一次已提交、且仍有效的评估合格集内。「仍有效」与面板 `current` 判据相同（`reconcile/worker/panel.rs:167-191`），需要给 `CommittedGroupAssessment` 加合格集，`submit_operation` 签名随之改变。
  - **`not_ready` 加可选 `reason` 枚举**（契约扩展，§2.1）：GLM 为用量未知 / 已越线 / 无有效评估；火山为已暂停 / 已耗尽 / 运行态未初始化 / 未到周期开始 / 非 type 8 / 无渠道。面板据此显示具体原因。
  - **后端检查不阻塞**（§2.2）：
    - 用 `backend_gen` 代际比较，检查期间不撤销、不持锁；
    - 失败结果总要发布；
    - `observations.clear()` / `wake_pending` 只在就绪状态真正变化时执行。
  - **栅栏只在相关变化时递增**（§2.2「相关变化」定义），细化里逐个 `mutate` 调用点标注归类。
- 约束：
  - 保持 I1–I8；
  - 凡改写路径都要扩充第 1 步的 T1；
  - 部署后对比 `QueueFull` 基线、切换次数、对齐写入次数、面板「后端未就绪」闪动（§4）。

### C. 其余第 1 批（依赖第 2 批的锁 / lane 调整）

- **Key 准入只留两条硬条件**（§2.3）：签发账号身份与角色正确、组已在 new-api 登记。
  - 去掉「余额 >0」，**必须在第 5 批上线之后**。
  - 补上导入 Key 的 `cross_group_retry` 核对，使 I2 对导入 Key 同样成立。
  - 从未发出的创建按条件改绑当前组版本。
- **建组即注册**：必须保持 I8。要么在提交组之前注册，要么组已提交时不以错误响应返回（例如 201 并带注册结果）；T1 要扩充。
- **组自动核验**（§2.4）：
  - 只升不降（I6），带退避；
  - 人工拒绝或 ratio / alias 不一致是终止错误；
  - 需要「已持 lane」的核验变体；
  - 会改变 `groups_runtime_probe.py:354` 的断言。
- **成员卡四分类**（§2.5）：预期停止 / 待启用（附原因）/ 同步失败 / 真正意外停用；priority 不一致只在已启用且非待同步时提示。

### D. 第 3 + 4 批（共用一次 schema **v10** 迁移，必须同批上线）

- **火山精简**（§1 第 3 项）：
  - 删死代码与只写不读的表；
  - 删 `error_epoch` / 证据游标 / Stale-Duplicate-Gap；
  - 去掉火山的耗尽 priority、`source_disabled`、`enable_epoch`（还需改 `execution.rs:177-180` 与 `verify_execution`）；
  - `cursor` 并入「上一个首选」；
  - 观测队列 + 请求阶段表改为每源一次索引查询。
- **火山组固定**（§2.7）：
  - 语义同 GLM；命令前置条件为「此刻是候选」。
  - 策略轮规则 1–4。
  - 解除记录持久化，GLM 共用；「知道了」= `set_group_pin(null)`。
  - 组版本只在 `pinned` 改变时递增。
  - `PANEL_SCHEMA_VERSION` 2 → 3（第 5 批先把它从 1 升到 2）。
- **放弃未创建的 Key**（§0 第 4 项）：加「放弃」标记，保留操作记录。本机 identity #1 就是实际案例。
- 需同步修订的设计文本与审计脚本清单见架构 §3 末尾。

## 4. 每一步的固定流程与交给监管者的检查点

1. 写细化（模板 `docs/workflow.md` §4.3.3；范例 `sgm-8-1-small-fixes.md`）→ **停，交用户确认**。
2. **检查点 C1**：用户确认后由subagent独立做§4.4审查（第2–6项；架构有改动含第1项），修到通过，修订记录写进细化。2026-11-02用户已替换审查方式，无须等Claude。
3. 实现与测试。每个行为修正都要配回归测试，并做变异验证（撤回修复，测试必须失败）。变异要在**单独的副本**里做，用**单独的 `CARGO_TARGET_DIR`**（见 §6）。
4. 跑完 §5 的全部门禁。
5. **检查点 C2**：本地提交（先不推送），把以下材料交监管者：
   - 提交 SHA；
   - 门禁输出的通过 / 失败数字；
   - 变异验证记录；
   - 与细化的偏差。
   监管者在该 SHA 上独立跑全量测试并做代码审查，修到通过。
6. 推送到 PR #5（显式 refspec），等 CI 三项（test / panel-ui / groups-runtime，约 17 分钟）全绿。
7. **检查点 C3**：部署前问用户。本机可在用户同意后直接部署；**fermat 每次都要用户明确同意**，带迁移或会写 new-api 的批次尤其如此。

汇报格式：做了什么、提交 SHA、门禁结果（带数字）、与设计的偏差、需要用户决定的问题。

## 5. 门禁

```bash
cargo test --release                      # 全量，默认并行度（不要降并发）
npm run test:ui                           # node 单元测试
npm run check:ui                          # 契约生成检查 + 清单 + DOM sink（CI 跑）
npm run test:browser                      # 4 个项目，用例清单见 sgm6_b_browser_tests.json
python3 scripts/contract_audit/sgm7_check.py   # CI 跑
rustfmt --edition 2021 --check <改动的 .rs>    # 这些文件在 HEAD 上是格式干净的
git diff --check
```

清单要同步：
- `scripts/contract_audit/sgm6_b_node_tests.txt`：node 测试名精确一致。
- `sgm6_b_browser_tests.json` 与 `tests/ui/reporter.test.mjs` 中的总数（B2 C2修正后 24 × 4 = 96；旧92项报告为历史记录）。
- `sgm7_tests.txt`：`runtime::tests::` 等测试名，按字典序。
- `sgm6_b_mutations.mjs`：永久变异，「变更前」子串必须唯一。

## 6. 已知坑（本轮实际踩过）

- **变异副本不能与工作区共用 `CARGO_TARGET_DIR`**：cargo 按相对路径计算产物哈希，副本的变异二进制会覆盖工作区产物，并被当成最新的用掉。表现是全量测试出现的失败行号对应副本文件，而不是工作区文件。已经发生时，先 `touch src/main.rs` 再重跑。
- **SQLite 对 `UPDATE x = x` 不写任何页**：要验证「写入会被发现」的变异，必须做真实写入（例如建一张表）。
- **catalog 为 WAL 模式**：证明「没有写入」不能只比较主文件字节，要比较快照，以及主文件 + `-wal`。
- 变异或失败运行可能留下 `proptest-regressions/**` 种子文件，提交前检查 `git status`，不要误提交。
- `src/panel_http/ui/contract.mjs` 是**生成文件**：先改 `docs/design/source-groups-token-management/schemas/*.schema.json`，再运行 `node scripts/contract_audit/sgm6_b_contract.mjs` 重新生成；新 record 族与枚举要在生成器的 `shapes` / `enums` 里登记。
- 快照文本不能含子串 `settings` 等（`panel_http/tests.rs:572-580`）。
- 新管理路由必须在 `panel_http/admin/tests/support.rs` 的 `record_wire` 里登记，否则测试会 panic。
- 管理错误体必须带 `existing_source_id`（只有 HTTP 层错误不带），浏览器测试的 mock 也一样。
- `ControlResult` 路径会把一切 `CatalogError`（含 Busy）变成 `Storage`（500）。凡是「什么都没写」的失败，要显式映射为 503 / 409 / 422，否则面板会误记「结果待核对」。
- 日志断言测试：并行测试下 tracing 调用点的兴趣缓存有竞态，在 `with_default` 内先调用 `tracing::callsite::rebuild_interest_cache()`（见 `runtime/tests.rs` 的 `control_warning_names_the_maintenance_job`）。
- firefox-mobile 的剪贴板用例（`API fixture fulfilled clipboard promise…`）在全量负载下偶发 5 秒超时；单独重复 20 次全部通过。全量门禁遇到它时重跑一次，不要改测试。
- 历史审计脚本 `sgm6_a1.sh`、`sgm6_a21.sh`、`sgm6_a22*`、`sgm6_b_check.mjs --freeze`、`sgm7_check.py --freeze` 都钉在旧 base 上，CI 不跑，HEAD 上本来就不全通过；不必维护，在 PR 说明里列出即可。
- new-api 全局限流约 2 次 / 秒（`/api` 360 次 / 180 秒），登录接口另有 20 次 / 20 分钟的限流。任何改动都不要引入逐渠道轮询或反复登录。

## 7. 部署配方

**本机**（只在用户同意后执行）：
```bash
W=/home/haokun/projects/quota-throttle/.worktrees/source-groups-research; I=~/apps/quota-throttle-sgm/bin
# 工作区干净、HEAD 为要部署的提交
cargo build --release --locked                       # 在 $W 下执行，确认输出里有 Compiling quota-throttle
ts=$(date +%Y%m%d-%H%M%S); cp -p $I/quota-throttle $I/quota-throttle.bak-$ts
install -m 0700 $W/target/release/quota-throttle $I/quota-throttle.new && mv -f $I/quota-throttle.new $I/quota-throttle
systemctl --user restart quota-throttle-sgm.service
# 验证：systemctl --user status、journalctl --user -u quota-throttle-sgm.service（应看到「new-api 已在运行」「来源分组服务已启动」）、
# curl -s 127.0.0.1:3001/assets/app.mjs 含新代码、curl -s -o /dev/null -w '%{http_code}' 127.0.0.1:3000/api/status 为 200
```

**fermat**（每次都要用户明确同意；ssh 别名 `fermat`，Tailscale）：
```bash
git -C $W archive <sha> | ssh fermat 'rm -rf ~/apps/quota-throttle-build && mkdir -p ~/apps/quota-throttle-build && tar -x -C ~/apps/quota-throttle-build && cd ~/apps/quota-throttle-build && cargo build --release --locked'
ssh fermat 'I=~/apps/quota-throttle-sgm/bin; ts=$(date +%Y%m%d-%H%M%S); cp -p $I/quota-throttle $I/quota-throttle.bak-$ts; install -m "$(stat -c %a $I/quota-throttle)" ~/apps/quota-throttle-build/target/release/quota-throttle $I/quota-throttle.new; mv -f $I/quota-throttle.new $I/quota-throttle; systemctl --user restart quota-throttle-sgm.service'
# 验证 13001 / 13000，new-api pid 不变
```

**回滚**：换回 `.bak-<ts>` 再重启。若该批带迁移（第 5 批 v9、第 3+4 批 v10），还需在服务停止时，用 state 目录里的 `catalog-vN-before-vN+1-*.sqlite3` 恢复目录库，因为旧二进制打不开新版本库。fermat 上恢复目录库属于碰 `state/`，必须先得到用户同意。

## 8. 待用户决定的事项

1. v1/I10 已确认，v3 技术独审与 Step0 文本已通过；D1 的 60s 业务/70s 客户端/75s 连接具体配套已确认，当前逐模块实现，C2/C3 停点保留。
2. 第 5 批部署到 fermat 的时机（首轮会写 fermat 的 new-api）。
3. 之前悬而未决、细节以用户为准：是否把 `panel-ui` / `groups-runtime` 设为 main 的必需检查；是否停用旧渠道 #8。
