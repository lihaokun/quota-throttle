# 架构设计 — 协议无关的单渠道下游接入

> 调研：`docs/research/claude-code-routing-research.md`

## 1. 结构

```text
                   ┌─ POST /v1/chat/completions (OpenAI)
one NewAPI key ────┤
                   └─ POST /v1/messages (Anthropic)
                                  │
                         NewAPI format conversion
                                  │
                   one managed channel per upstream key
                                  │
                         Zhipu Coding Plan API
```

OpenAI/Anthropic 是下游协议；token/group 是访问与出口选择；渠道是上游凭据。三者不可混为一层。

## 2. 模块职责

- `config.rs`：只保留一个 `channel_template`；不再存在 Claude 渠道模板。
- `newapi.rs`：每把上游 key 只创建/对账一个渠道；不创建 group 或下游 token。
- `orchestrator.rs`：每把 key 只有一个 channel id 和一份 priority 状态。
- `main.rs`：打印两种下游 endpoint，并明确 `ANTHROPIC_AUTH_TOKEN` 复用现有 NewAPI key。
- `status.rs`：始终展示 Claude endpoint；不展示虚构的 CC 渠道或联动 priority。

## 3. 不变量

- I1：一把上游 key 恰好对应一个受管渠道。
- I2：OpenAI 与 Claude 下游请求使用同一个 NewAPI token/group。
- I3：两种下游请求自然共享同一个 active channel 和 priority。
- I4：quota、pin、95% 安全线只维护一份状态，不因下游格式分叉。
- I5：启用 Claude 下游不写 NewAPI option，不创建或打印任何新 token。

## 4. 兼容性

现有配置无需新增字段。Claude Code 只设置：

```text
ANTHROPIC_BASE_URL=<new_api.base_url>
ANTHROPIC_AUTH_TOKEN=<现有 NewAPI key>
```

若有人测试过旧双渠道草案，遗留的 `*-cc` 渠道与 `claude-code` token 不再由本工具管理；上线前
应在 NewAPI UI 中禁用或删除，防止它们作为野生渠道继续接流量。
