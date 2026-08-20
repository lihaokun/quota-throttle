# claude-code-routing 调研报告

> 目标：让 Claude Code（Anthropic 格式）也走本工具的智谱 key 池自动切换，与 opencode（OpenAI 格式）并存。
> 本文所有结论均标注来源（官方文档 / new-api 源码 / 本机实测），不凭记忆。

## 0. 现状与目标

- 现状链路：opencode →(OpenAI 格式)→ 本地 new-api（每把智谱 key 一个 Custom(8) 渠道，
  base_url=智谱 coding 口 `/v4/chat/completions` 全路径透传）→ 智谱。orchestrator 用
  priority 把单把活动 key 钉在最高档，护 prompt 缓存局部性。
- 新增链路：Claude Code →(Anthropic 格式 /v1/messages)→ **同一个** new-api → 智谱
  Anthropic 兼容口。同一把智谱 key 同时挂两种格式渠道，切换时两边 priority 联动。
- 用户已确认：两套并存（opencode 渠道保留）。

## 1. 智谱 Anthropic 兼容口

| 事实 | 值 | 来源 |
|---|---|---|
| Base URL | `https://open.bigmodel.cn/api/anthropic` | 官方文档 [1][2] |
| 请求路径 | `{base}/v1/messages` | 官方文档 [1]（curl 示例） |
| 鉴权 | 文档示 `x-api-key`；**Bearer 亦可用**（本机 Claude Code 当前就用 `ANTHROPIC_AUTH_TOKEN` → `Authorization: Bearer` 直连成功） | [1] + 本机实测 |
| 模型名 | 直接用智谱模型编码（`glm-5.3` 等）；**1M 上下文加后缀 `[1m]`**（如 `glm-5.2[1m]`） | [1][2] |
| 服务端模型映射 | 存在「界面上是 Claude 模型实际是 GLM」的服务端映射，但**本方案不依赖它**（客户端显式发 glm 名） | [2] |
| Claude Code 接入 | 官方教程用 `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` + 三个 `ANTHROPIC_DEFAULT_{SONNET,OPUS,HAIKU}_MODEL` | [2] |
| `/v1/messages/count_tokens` | 文档未提及，是否支持未知 | [1]（缺失） |

## 2. new-api（v1.0.0-rc.20 源码，`gh api repos/QuantumNous/new-api/contents/...`）

| 事实 | 值 | 来源（源码路径） |
|---|---|---|
| Claude 渠道类型码 | `ChannelTypeAnthropic = 14` | `constant/channel.go:18` |
| 上游 URL 拼法 | `{ChannelBaseUrl}/v1/messages`（另按需附 `?beta=true`） | `relay/channel/claude/adaptor.go` GetRequestURL |
| 上游鉴权头 | `x-api-key: <key>` + `anthropic-version`（缺省 2023-06-01） | 同上 SetupRequestHeader |
| `anthropic-beta` 头 | 从客户端请求**透传**上游 | 同上 CommonClaudeHeadersOperation |
| 请求体转换 | claude→claude **原样透传**（`ConvertClaudeRequest` 直接 return request）⇒ `cache_control`/system/thinking 语义不丢 | 同上 |
| 入口路由 | `POST /v1/messages` 存在（"claude related routes"） | `router/relay-router.go:88` |
| **无** count_tokens 路由 | `/v1/messages/count_tokens` 未注册 ⇒ CC 调它会 404（影响待实测） | 同上（缺失） |
| 入口令牌鉴权 | 同时接受 `Authorization: Bearer` 与 `x-api-key`（后者转成前者） | `middleware/auth.go:333` |
| 渠道选择 | 按 **(group, model)** 从 abilities 表选，**不按请求格式过滤** | `middleware/distributor.go` |
| 渠道 `models` 字段 | 纯逗号分割字符串，**无名称正则校验** ⇒ `glm-5.3[1m]` 可直接挂 | `model/channel.go:39,289` |
| group 计费 | `group_ratio_setting` 默认只注册 `default/vip/svip`；新 group 需显式注册，未定义行为未验 | `setting/ratio_setting/group_ratio.go` |
| 跨格式转换 | openai↔claude 双向转换存在（`ConvertOpenAIRequest` 等）⇒ 同池混挂会被转换 | `relay/channel/claude/adaptor.go` |

### ⚠️ 推论（本设计最重要的约束）

**分发器不按格式过滤** ⇒ 若 type-8 与 type-14 渠道把同名模型挂在同一 group，claude 格式
请求可能被路由到 openai 渠道（跨格式转换，毁缓存局部性 + 语义漂移），反之亦然。
**必须用 group 隔离两套渠道**，且令牌按 group 绑定（CC 的令牌只能看到 claude group）。

## 3. Claude Code 客户端（本机当前配置 = 实测基线）

当前直连智谱（本会话即跑在此配置上）：

```
ANTHROPIC_BASE_URL=https://open.bigmodel.cn/api/anthropic
ANTHROPIC_AUTH_TOKEN=<智谱 key>
ANTHROPIC_DEFAULT_OPUS_MODEL=glm-5.3[1m]     # sonnet/haiku 同
ANTHROPIC_MODEL=opus[1m]
CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
```

⇒ **迁移到本工具只改两个变量**：`ANTHROPIC_BASE_URL=http://127.0.0.1:3000`（本地 new-api）、
`ANTHROPIC_AUTH_TOKEN=<new-api 令牌>`。模型解析、effort、[1m] 后缀等全部不动。
实测证明智谱口接受模型名 `glm-5.3[1m]`（本会话就在用）。

## 4. 方案对比

| 方案 | 描述 | 结论 |
|---|---|---|
| **A. 同一 new-api + group 隔离（推荐）** | 每把 key 双渠道：`<name>`(type 8, group default, opencode 用) + `<name>-cc`(type 14, group `claude`, CC 用)；CC 用绑定 claude group 的专用令牌；切换时两渠道 priority 联动 | 改动集中：config 双模板、sync 建双渠道、orchestrator 双写、group/令牌确保。看板天然覆盖（列的是全部渠道） |
| B. 第二个 new-api 实例 | 隔离彻底，但 boot/orchestrator/panel 全部双实例化，改动面大 | 否决 |
| C. 靠模型名区分（不分组） | 两客户端要用**同名** glm 模型，模型名无法区分 | 不可行 |
| D. CC 直连智谱 + 工具改写客户端配置切 key | 每次切换要动客户端 env/重启，违背「不断流切换 + 护缓存」的立项目标 | 否决 |

## 5. 推荐方案（A）的待实测清单

实现期逐项验证（对本地 new-api v1.0.0-rc.20 + 真 key）：

1. **全链路**：CC → new-api `/v1/messages` → 智谱 claude 口，含 stream、thinking、
   `cache_control`（响应 usage 里 cache_creation/reads 字段非零即透传成功）。
2. **count_tokens 404** 对 CC 的实际影响（预期非致命，CC 会回退本地估算；当前直连智谱
   也未确认支持）。
3. **group 注册**：把 `claude` 写进 `group_ratio_setting`（option/config API）的确切调用；
   未注册时令牌选组/计费的行为。
4. **令牌**：创建/绑定 group=claude 的令牌；创建响应是否返回完整 key（列表是打码的，
   CLAUDE.md 已知；不行则提示用户从 UI 复制一次或读 SQLite）。
5. **429 兜底**：智谱 claude 口报 429/「达到上限」时 new-api 是否按预期 fallback 到
   standby 渠道（与 openai 口行为一致性）。

## 5.5 实测结果（2026-08-20 验收时回填）

| 项 | 结果 |
|---|---|
| H1 group 隔离 | ✅ 按设计成立：CC 令牌只路由到 claude group 的 -cc 渠道 |
| H3 令牌 group 路由域 | ⚠️ **多一道门**：group 还必须在 `UserUsableGroups`（用户可用组 option）里，缺了 TokenAuth 直接 403「无权访问 x 分组」（auth.go:421-435）。`ensure_group` 已改为两处都注册 |
| H4 group ratio 形态 | rc.20 走**旧形态** `GroupRatio`（平铺 map）；新形态 `group_ratio_setting` 作兜底保留 |
| H5 令牌完整 key | ✅ `POST /api/token/:id/key` 回完整值，每次 sync 现取 |
| `[1m]` 后缀（§1 假设） | ❌ **推翻**：智谱 anthropic 口不认 `glm-5.3[1m]`（1214），纯 `glm-5.3` 通；CC 客户端自己剥后缀发纯名。渠道模型表的 `[1m]` 变体降级为无害冗余 |
| CREDIT_LIMIT（计划外发现） | 智谱窗口 type 有两种计费模式（TOKENS/CREDIT），探针已改为都接受；另 selector 决定查哪个团队的额度，账号属多团队时按 key 所属团队配 |

## 6. 来源

- [1] 智谱官方文档「Claude API 兼容」：https://docs.bigmodel.cn/cn/guide/develop/claude/introduction
- [2] 智谱官方文档「Claude Code 接入教程」：https://docs.bigmodel.cn/cn/guide/develop/claude
- [3] new-api 源码 @ v1.0.0-rc.20：`constant/channel.go`、`relay/channel/claude/adaptor.go`、
  `router/relay-router.go`、`middleware/auth.go`、`middleware/distributor.go`、
  `model/channel.go`、`setting/ratio_setting/group_ratio.go`（经 GitHub contents API 逐文件核实）
- [4] 本机实测：用户当前 Claude Code 直连智谱的 env（见 §3）；本会话模型 `glm-5.3[1m]`
