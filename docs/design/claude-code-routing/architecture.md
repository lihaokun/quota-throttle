# claude-code-routing 架构

> 调研依据：`docs/research/claude-code-routing-research.md`（所有外部事实的来源与实测编号引用本文 §7 的 H 编号）。
> 已确认需求：Claude Code（Anthropic 格式）走本工具的智谱 key 池自动切换，**与 opencode 两套并存**。

## 0. 范围

**做什么**：同一把智谱 key 在 new-api 里挂两个渠道——现有 OpenAI 格式渠道（opencode 用）+
新增 Claude 格式渠道（type 14 → 智谱 anthropic 口，Claude Code 用）；切换循环对两渠道
**priority 联动**。Claude Code 侧只改两个环境变量接入本地 new-api。

**不做什么**：
- 不改 `quota.rs`（用量探针）、`boot.rs`（进程托管）、`decide()` 决策纯函数、`[[keys]]` 配置 schema。
- 不做 model_mapping（客户端显式发 glm 模型名，见 D3）。
- 不管理 Claude Code 的客户端配置文件（只在 sync/up 时打印接入说明）。
- 不引入 SQLite 依赖（令牌完整 key 优先从创建响应拿，见 §3.3）。

## 1. 核心流程

### 1.1 接入期（sync / up 子命令）

```
sync/up
 ├─ ensure new-api 进程（现有逻辑不动）
 ├─ sync_channels：每把 key
 │   ├─ 渠道 "<name>"   type 8  group default    （已存在则跳过 —— 现有逻辑）
 │   └─ 渠道 "<name>-cc" type 14 group "claude"  （新增；已存在则跳过）
 ├─ ensure_group("claude")        注册到 group_ratio_setting（ratio 1）
 ├─ ensure_claude_token           名为 "claude-code"、group=claude、无限额度令牌
 └─ 打印 Claude Code 接入说明（两个 env + 令牌 key）
```

### 1.2 运行期（切换循环，每轮 tick）

```
智谱用量探针（不变）→ decide()（不变，身份 = 主渠道 id）
 → 对每把 key 算出目标 priority（不变）
 → 下发：主渠道 id 与 <name>-cc 渠道 id **写同一个 target**   ← 唯一的运行期改动
 → 快照：KeyStatus 带上 claude_channel_id；新增 claude_endpoint
```

### 1.3 数据面（转发路径，new-api 负责，本工具只摆 priority）

```
opencode     ──OpenAI 格式──► new-api ─┐
                                      ├─ group "default"：type 8  渠道 ──► 智谱 coding 口
Claude Code  ──Anthropic /v1/messages─► new-api ─┤
                                      └─ group "claude"  ：type 14 渠道 ──► 智谱 anthropic 口
两个 group 各自内部按 priority 阶梯路由 ⇒ 单活动 key 语义在两侧同时成立
```

## 2. 数据结构（跨模块共享）

### 2.1 `ClaudeChannelTemplate`（config.rs，新增）

字段：
- `channel_type: i64` — new-api 渠道类型码，默认 **14**（Anthropic，`constant/channel.go:18`）
- `base_url: String` — 默认 `https://open.bigmodel.cn/api/anthropic`（new-api 自动拼 `/v1/messages`）
- `models: String` — 逗号分隔模型名，**含 `[1m]` 后缀变体**（Claude Code 实际发出的就是
  `glm-5.3[1m]` 这类名字，调研 §3 实测）
- `group: String` — 默认 `claude`。⚠️ **不得**与 OpenAI 渠道同 group（H1：分发器不按格式过滤）
- `name_suffix: String` — 渠道名后缀，默认 `-cc`；claude 渠道名 = `<key name> + name_suffix`
- `token_name: String` — CC 专用令牌名，默认 `claude-code`

类型不变量：
- `group ≠ [new_api.channel_template].group`（启动时校验，违反即拒绝启动——同 group 会跨格式
  互抢流量，是本设计要消灭的故障模式）
- `name_suffix` 不得为空（否则 claude 渠道名与主渠道同名，sync 按名匹配会互相误判）
- 对任意两把 key：`keys[j].name ≠ keys[i].name + name_suffix`（细化阶段补：否则某把 key 的
  cc 渠道名会与另一把 key 的主渠道名撞车，sync 按名匹配会把别人的主渠道误认成自己的 cc 渠道）

生命周期：`[new_api.channel_template_claude]` 表**存在** = 功能开启；不存在 = 完全走现有行为
（D4：功能开关即配置块本身）。

### 2.2 `ResolvedKey`（config.rs，扩展一个字段）

- 新增 `claude_channel_id: Option<i64>` — 该 key 的 claude 渠道 id；`None` = 无 claude 渠道
  （未配模板 / 建渠道失败 / 未 sync），该 key 的 claude 侧不受管。

类型不变量：
- **主 `channel_id` 是唯一的决策身份**：active / pinned / eligible / applied 等所有调度状态
  仍以主渠道 id 为键。claude 渠道是「从属执行器」，不参与决策（D2）。
- 同一 key 的两个渠道 priority 恒相同（见 I1）。

### 2.3 `KeyStatus`（status.rs，扩展一个字段）

- 新增 `claude_channel_id: Option<i64>` — 仅供看板关联展示（key 卡片上认领 `-cc` 渠道，
  使其不被「野生渠道」误报，并显示 cc 渠道实况）。

### 2.4 `StatusSnapshot`（status.rs，扩展一个字段）

- 新增 `claude_endpoint: String` — Claude Code 应填的 `ANTHROPIC_BASE_URL`（= new_api.base_url，
  如 `http://127.0.0.1:3000`）。与现有 `client_endpoint`（opencode 用，`{base}/v1`）并列展示。

## 3. 模块划分与功能规约

### 3.1 config.rs

功能：加载 `[new_api.channel_template_claude]`；`validate()` 增加 §2.1 两条不变量校验；
`ResolvedKey` 扩展字段；`append_key`（面板加 key 写回 config）**不动**——`[[keys]]` schema
没变，claude 渠道 id 靠 sync 按名解析，不落 config。

- Requires：TOML 合法；两模板 group 不同名（若都配了）。
- Ensures：`channel_template_claude = None`（未配）时，下游所有行为与现状逐字节一致。

### 3.2 newapi.rs

功能规约（新增/改动）：

- `create_channel` 泛化为「按模板建渠道」：现有签名收 `(tpl, name, key, priority)`，claude
  模板用同一 payload 形状（`{mode:"single", channel:{...}}`，字段同名）。两模板共用实现。
- `sync_channels(keys, openai_tpl, claude_tpl, standby)`：每把 key 确保两个渠道存在
  （各自模板缺省则跳过对应渠道），返回 `primary: HashMap<name, i64>` +
  `claude: HashMap<name, i64>`。幂等：按渠道名匹配，已存在不动（与现有语义一致）。
- `ensure_group(group)`（新增）：确保 group 已注册进 group_ratio_setting（ratio=1）。
  ⚠️ 具体 API 形态属实现期实测项（H4）。
- `ensure_claude_token(name, group)`（新增）：按名找令牌；没有则建（group 绑定、无限额度、
  永不过期）。**返回完整 key**；若 API 只回打码 key（H5），返回 None 并让调用方降级提示。
- 副作用：以上均为 new-api 管理面的写操作，**不受 dry_run 门控**（与现有「sync 建渠道不
  门控、运行期 priority 才门控」的分工一致，D6）。

### 3.3 main.rs

- `cmd_sync` / `cmd_up`：调新的 `sync_channels`；配了 claude 模板则 `ensure_group` +
  `ensure_claude_token`；打印 CC 接入说明（`ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`
  两个 env，注明「其余 env 不动」）。
- `resolve_keys`：填 `claude_channel_id`（按 `<name>-cc` 从 claude map 取；取不到 = None 并 warn）。
- `cmd_run`：同样按名解析 claude 渠道 id（list_channels 已返回全部渠道，无新调用）。

### 3.4 orchestrator.rs

- **tick 的 priority 下发段**（唯一运行期改动）：对每把 key 算出 target 后，对
  `[主 id, claude_channel_id?]` 两个渠道各走一遍现有「幂等检查 + PUT + 记 applied」路径。
  `applied` 按渠道 id 各自记账（主、cc 互不干扰，I1）。
- `pin` / `remove_key`：入参 channel_id **同时接受主渠道与 cc 渠道 id**（看板两处都可能发起），
  统一解析到所属 key 再执行。`remove_key` 把**两个渠道**都压到 exhausted 档再放手（D7，
  防孤儿高优先级渠道继续吃流量——现有血泪教训的 cc 版）。
- `add_key`：配了 claude 模板时**探活通过后建两个渠道**（openai 失败即终止不动 config；
  claude 失败仅 warn、不阻塞该 key 入池——它只是少一路客户端，用量调度照常）。
- `decide()` / `eligible_set()` / 探针 / 档位逻辑：**零改动**。

### 3.5 status.rs

- `KeyStatus.claude_channel_id` / `StatusSnapshot.claude_endpoint`（§2.3/2.4）。
- `tracked_channels`：把 cc 渠道 id 也纳入（面板给 cc 渠道拉 rpm/tpm）。
- HTML：key 卡片认领 cc 渠道（渠道 #id 徽标 + enabled/priority 对账，复用现有合并逻辑）；
  「野生渠道」过滤集合加入 cc id；chips 行并列展示两个客户端 endpoint；加 key 成功回执
  带上两个渠道 id。

## 4. 接口规约

| 调用方 → 被调方 | 数据 | 协议约定 |
|---|---|---|
| main → newapi | `sync_channels(keys, tpl_openai, tpl_claude, standby)` → `(primary, claude)` 两张 name→id 表 | 缺哪个模板就不建哪类渠道；按名幂等 |
| main → newapi | `ensure_group(g)` / `ensure_claude_token(name, g)` → `Option<key>` | key 为 None = API 只回打码值，调用方打印 UI 取 key 的指引 |
| orchestrator → newapi | `set_channel_priority(id, p)`（对主/cc 渠道各调一次，同一 target） | 沿用「GET→只改字段→PUT 且剔 status」的既有约定 |
| status/orchestrator → 快照 | 新字段 `claude_channel_id` / `claude_endpoint` | 决策循环写决策字段、面板循环写面板字段的现有分工不变 |
| 看板 → orchestrator | `Command::Pin/RemoveKey` 的 channel_id 语义扩展为「主或 cc」 | 解析到 key 后一切如旧；不新增命令类型 |

## 5. 关键设计决策

| # | 决策 | 理由（否决项） |
|---|---|---|
| D1 | **同一 new-api 实例 + group 隔离**（claude 渠道独立 group，CC 令牌绑定该 group） | H1：分发器按 (group, model) 选渠道、不按格式过滤；同 group 混挂必跨格式互抢。双实例（B）改动面大；模型名区分（C）不可行（两客户端用同名模型）；改写客户端配置（D）违背不断流立项目标 |
| D2 | **决策身份仍是主渠道 id**，claude 渠道只是从属执行器 | `decide()` 及其全部单测零改动；回归风险集中在「下发循环多写一个 id」这一处 |
| D3 | **不用 model_mapping**，客户端显式发 glm 名（`ANTHROPIC_DEFAULT_*_MODEL`，含 `[1m]`） | 这是智谱官方教程模式，且用户当前直连已在用（实测基线）。用 claude-* 名 + 映射则要枚举 Claude Code 可能发出的全部模型名（主模型 + haiku 后台 + /model 选择器），集合不可控、漏一个即「无可用渠道」 |
| D4 | 功能开关 = `[new_api.channel_template_claude]` 配置块存在与否 | 不配 = 现有用户零感知；配置即全功能（幂等 sync 会补齐 group/令牌/渠道） |
| D5 | claude 渠道名 = `<name>-cc`（后缀可配），令牌名单独可配 | 与现有「sync 按名幂等」机制天然兼容，无需新增持久化状态 |
| D6 | 建渠道/group/令牌属 sync（setup 类）**不受 dry_run 门控**；运行期 priority 受门控 | 与现有分工完全一致：dry_run 管的是「调度决策生效与否」，不管基础设施铺设 |
| D7 | `remove_key` 把主/cc 渠道**都**压到 exhausted 档 | 只压主渠道的话，cc 渠道以 priority=100 孤儿般继续吃光 Claude Code 流量，而用量已无人盯 |

## 6. 架构正确性论证

goal → 模块映射：
- **G1 Claude Code 流量只走智谱 anthropic 口，且绝不跨格式** → newapi（group/令牌确保）
  + config（group 不变量校验）。论证：token group=claude ⇒ 分发器候选集 = claude group 的
  abilities = 仅 -cc 渠道（H1 + D1）；-cc 渠道 type 14 ⇒ claude→claude 原样透传（H2）。
- **G2 切换对两个客户端同时生效** → orchestrator 下发循环。论证：同 key 双渠道写同一
  target priority（I1）⇒ 各 group 内部的 priority 阶梯形态与单渠道时代完全相同。
- **G3 缓存局部性不降级** → 同 G2：每个 group 内仍只有一个最高档渠道吃全部该格式流量。
- **G4 不破坏现状** → config（D4 开关）+ decide() 零改动（D2）。未配 claude 模板时，
  所有新代码路径不触达。
- **G5 可运维** → 看板认领 cc 渠道（不误报野生）、双 endpoint 展示、token/group 自动铺设。

关键假设（承接调研 §5 实测清单）：
- **H1** 分发器按 (group, model) 选渠道、不按格式 —— 已源码证实（`middleware/distributor.go`）。
- **H2** 智谱 anthropic 口接受 `x-api-key`（new-api 上游头）与 `glm-5.3[1m]` 模型名 ——
  官方文档 + 用户当前直连实测。
- **H3** 令牌绑定 group=claude 后，其路由域即该 group（root 可建任意 group 令牌）——
  实现期实测；若 root 令牌建组受限，降级路径：把 claude group 加进用户可用组再建。
- **H4** `ensure_group` 的确切 API（option/config 形态随版本变）—— 实现期实测。
- **H5** 令牌创建响应是否返回完整 key —— 实现期实测；不返回则打印 UI 指引（不引 SQLite 依赖）。
- **H6** new-api 无 `/v1/messages/count_tokens` 路由对 CC 非致命（回退本地估算）—— 实现期实测。

模块级 invariant 列表：
- **I1 同 key 双渠道 priority 恒相同** — 维护方：orchestrator 下发循环（同一 target 双写；
  单侧失败时 applied 不更新，下一轮重试 ⇒ 最终一致；看板 priority 对账会把不一致晒出来）。
- **I2 claude group 内只有 `-cc` 渠道** — 维护方：sync 只按模板建渠道（模板 group 固定）；
  人工在 new-api 里往 claude group 塞别的渠道属越权操作，看板「野生渠道」区可见。
- **I3 决策身份唯一（主渠道 id）** — 维护方：decide()/eligible/pin 全链路不感知 cc id；
  Pin/RemoveKey 入口处把 cc id 归一化到主 id。
- **I4 cc 渠道缺失 ⇒ 显式失败而非静默错路由** — claude group 内无渠道时 CC 请求报
  「无可用渠道」；绝不会回退到 default group（token group 隔离，H1/H3）。

preservation 论证：I1 由 tick 的双写 + applied 幂等共同保持；I2 由 sync 的唯一建渠道入口
保持；I3 由 orchestrator 入口归一化保持；I4 由 new-api 分发语义（H1）保持，本工具无需代码。

## 7. 验收（联动实测，承接任务 #4）

1. `sync` 后：每把 key 有 `-cc` 渠道（type 14 / group claude / models 含 `[1m]` 变体）、
   `claude-code` 令牌存在且 group=claude、claude group 已注册 ratio=1。
2. CC 按 §1.1 打印的两个 env 接入：能对话、能 stream、`/effort` 生效；
   响应 usage 含 cache 字段且重试命中（验证 `cache_control` 全链路透传，H2）。
3. 切换联动：人为把活动 key 推过 throttle（或临时调低阈值）⇒ 看板观察到**两个**渠道
   priority 同时变化；CC 与 opencode 流量同时切到新活动 key（live rpm 佐证）。
4. `remove_key` 后该 key 的两个渠道 priority 均为 exhausted 档，无孤儿高优先级渠道。
5. 看板：key 卡片认领 cc 渠道、野生渠道区**不**出现 `-cc`；H6（count_tokens 404）观察
   CC 行为无异常。
