# 细化设计 — 同一 key 支持 OpenAI 与 Claude 下游

## 1. 删除错误的重复基础设施

- 删除 `ClaudeChannelTemplate` / `channel_template_claude`。
- 删除 `<name>-cc` 的规划、创建、解析和模型同步。
- 删除 `ensure_group`、`ensure_claude_token` 及完整 token 日志。
- 删除 `ResolvedKey.claude_channel_id` 和所有双 priority 写入。
- 删除看板中的 CC 渠道、CC priority、CC rpm 对账。

## 2. 保留与新增行为

- `sync/up/AddKey` 仍维护每把上游 key 的唯一渠道。
- `/models` 发现继续对账该唯一渠道。
- 面板实时指标继续使用一次 `recent_logs` 请求推导。
- `StatusSnapshot.claude_endpoint = new_api.base_url`，始终展示。
- 启动日志只打印配置方法，不读取或泄露 token：Claude 使用与 OpenAI 客户端相同的 NewAPI key。

## 3. 验收矩阵

| 输入 | 凭据 | 上游渠道 | 期望 |
|---|---|---|---|
| `/v1/chat/completions` | 现有 NewAPI key | 当前 active type 8 | OpenAI 响应 |
| `/v1/messages` 普通 | 同一 key | 同一 active type 8 | Anthropic message |
| `/v1/messages` 流式 + tools/cache_control | 同一 key | 同一 active type 8 | 完整 Anthropic SSE |
| `/v1/models` + Anthropic headers | 同一 key | 同一 group abilities | Anthropic 模型列表 |

单元回归同时要求：每把 key 的 planner 只产生一个 op；状态快照只含一个 channel id；priority
每轮每把 key 至多写一次；旧 `channel_template_claude` 不再出现在示例或文档中。
