# 调研 — 上游 `/models` 驱动的模型目录

## 1. 问题与目标

当前 `ChannelTemplate.models` 在进程启动时载入，并在面板热添加 key 时继续复用。运行中若
`config.toml` 增加新模型，旧进程仍会用旧字符串创建渠道；2026-08-28 的 `zhipu-6` 因此没有
`glm-5.3` ability，虽然 priority=100，真实请求仍只能落到旧渠道并 429。

目标：模型集合以每把上游 key 实时返回的 `/models` 为准；配置字符串只做故障 fallback，
`up/sync` 同时对账已有渠道，不再“同名存在即跳过”。

## 2. 已核实的外部事实

### 2.1 智谱 Coding Plan

- `GET https://open.bigmodel.cn/api/coding/paas/v4/models`
- 鉴权：`Authorization: Bearer <智谱 key>`
- 2026-08-28 实测 HTTP 200，返回 OpenAI 形状 `data[].id`，包含 `glm-5.3`、
  `glm-5.3-flash` 等 10 个模型。
- 请求不生成内容，无生成 token 消耗。

### 2.2 火山自定义 APIG

- `GET <base>/models`
- 鉴权：`Authorization: <原始 key>`（不是 Bearer）
- 2026-08-28 实测 HTTP 200，返回相同的 `data[].id` 形状，包含 13 个模型。

两者证明模型发现应抽象为“URL + 鉴权方式 + 上游 key”，不能硬编码为智谱专用函数。

## 3. 当前代码能力与缺口

已有：

- `ChannelTemplate.models` fallback 字符串；
- `sync_channels()` 能创建缺失渠道，但已存在渠道直接 Skip；
- `set_channel_priority()` 已有安全的 GET→改字段→去 status→PUT 模式；
- add-key 路径在探活后创建渠道，但使用不可变的进程内 `self.cfg`。

缺口：

- 无 `/models` 客户端和响应校验；
- 无模型规范化（trim、去空、去重）；
- 无已有渠道 models 漂移检测/更新；
- 无“发现失败时保持已有值”的降级语义；
- 模型列表与 channel 创建耦合在静态模板字符串里。

## 4. 方案比较

| 方案 | 描述 | 结论 |
|---|---|---|
| A. `/models` 权威、配置 fallback | sync/add 时发现；成功即对账，失败保留已有或用于新建 fallback | 推荐：自动、可恢复、兼容旧配置 |
| B. 配置永远权威 | 继续人工维护 models | 否决：已经导致线上漂移 |
| C. 发现结果与配置取并集 | 自动结果 + 所有手填项 | 暂缓：会把拼错/已下线模型永久暴露；以后由显式 include 实现 |
| D. 收到未知模型请求时临时探测 | 数据面懒发现 | 否决：首请求失败、并发与缓存复杂，污染热路径 |

## 5. 推荐方案与边界

采用 A：

1. 每次 `up/sync`、面板新增 key、用户手动“刷新模型”时发现；不进入 60 秒 quota 循环。
2. 每把 key 独立发现，避免不同套餐/权限的模型集合被错误合并。
3. 成功且非空时，发现结果是该渠道的基础模型集合。
4. 失败时：已有渠道完全不动；新建渠道使用配置 fallback。
5. 同步只更新真实发生漂移的渠道，保护 new-api `/api` 限流预算。
6. 本 feature 不实现 UI include/exclude；它们在 group UI 子计划中叠加为
   `effective = discovered + include - exclude`。

## 6. 关键假设

- H1：`data[].id` 是上游当前允许该 key 调用的模型 ID。
- H2：HTTP 200 但模型数组为空不是合法成功，必须视为失败。
- H3：发现结果是该上游 key 的基础模型目录；下游协议特有别名不由自动发现凭空生成。
- H4：models 顺序不影响路由语义；比较时按集合，写入时保留上游首次出现顺序。
- H5：上游 key 属敏感信息，任何错误和日志不得包含请求头或 key。
