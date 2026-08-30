# 调研 — 同一 NewAPI key 的 Claude 下游接入

## 1. 目标澄清

Claude 是 NewAPI 对下游暴露的另一种请求格式，不是新的上游提供商，也不是一个新的出口组。
现有 NewAPI 访问 key 应当同时可调用 OpenAI 与 Anthropic 接口；格式变化不得要求新 token、
新 group 或复制上游渠道。

## 2. NewAPI v1.0.0-rc.20 源码事实

- `router/relay-router.go` 原生注册 `POST /v1/messages`，使用 `RelayFormatClaude`。
- `middleware/TokenAuth` 对该路径接受 Anthropic 的 `x-api-key`，仍解析同一 Token。
- `relay/channel/openai/adaptor.go::ConvertClaudeRequest` 调
  `service::ClaudeToOpenAIRequest`，将 system/messages/tools/tool_result 等转换为 OpenAI 请求。
- Custom(type 8) 的请求 URL 直接使用渠道完整 `base_url`，因此转换结果会送到现有智谱
  `.../chat/completions` 渠道。
- OpenAI adaptor 的响应层会把普通/流式 OpenAI 响应重新转换为 Anthropic JSON/SSE。

结论：现有唯一的 type 8 渠道已经同时服务两种下游格式。

## 3. 2026-08-30 本机实测

环境：当前 NewAPI、唯一 token `opencode`（group=default）、7 个 type 8 智谱渠道，无 type 14
或 `-cc` 渠道。

1. 同一 `opencode` key 请求 `/v1/messages`：HTTP 成功，返回 `type=message`、Anthropic usage。
2. 同一 key 发流式请求，携带 `tools` 与 `cache_control`：完整收到
   `message_start → content_block_* → message_delta → message_stop`。
3. 同一 key 带 Anthropic headers 请求 `/v1/models`，按 Anthropic 形状返回现有渠道的 9 个模型。
4. 生成请求均落在现有活动渠道，不需要新的 token/group/channel。

## 4. 方案结论

采用单渠道方案：

```text
OpenAI downstream ─┐
                   ├─ same NewAPI token/group ─→ same active type-8 channel ─→ Zhipu
Claude downstream ─┘
```

否决旧的双渠道方案：它为每把上游 key 创建 `-cc` type 14 渠道，再创建 Claude 专用 token/group，
把下游格式错误地固化进凭据和调度状态；不仅重复，还让用户切换请求格式时必须换 key。

## 5. 边界

- `/v1/messages/count_tokens` 在该 NewAPI 版本没有专用路由；此前实测 Claude Code 遇到 404
  会退化为本地估算，不影响生成请求。
- `cache_control` 可被请求解析和转换链接受；智谱 Coding OpenAI 口的缓存命中由上游实际 usage
  决定，不承诺与 Anthropic 原生口完全相同的显式缓存语义。
- 未来智谱/火山出口分组属于独立功能，不应重新与下游协议绑定。
