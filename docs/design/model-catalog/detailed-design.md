# 细化设计 — `/models` 驱动渠道模型

> 架构：`docs/design/model-catalog/architecture.md`

## 1. 配置契约

渠道模板增加可选子表：

```toml
[new_api.channel_template.model_discovery]
url = "https://open.bigmodel.cn/api/coding/paas/v4/models"
auth = "bearer"
```

`auth` 支持 `bearer`、`authorization_raw`、`x_api_key`，缺省为 `bearer`。
没有 `model_discovery` 的旧智谱 Coding 模板会按已知官方地址自动补齐；非智谱 Custom 模板
保持原行为。`models` 保留为发现关闭或新建时发现失败的 fallback。URL 必须是绝对
`http/https` URL。

## 2. 发现与规范化

`ModelCatalogClient::discover(config, key)`：

1. 按 auth 类型设置一个鉴权 header；
2. GET 配置 URL；
3. 要求 2xx；
4. 解析 OpenAI 兼容响应 `data[].id`；
5. 对 id 做 trim、丢弃空值、按首次出现去重；
6. 结果为空即失败。

错误只包含 URL、HTTP 状态和响应结构摘要，绝不包含 key/header 值。

## 3. sync 状态机

每个 `(key.name, discovery.url, discovery.auth)` 在单次 sync 中只发现一次并缓存结果，
同一 key 的 OpenAI/Claude 两个渠道复用：

| 渠道状态 | 发现状态 | 动作 |
|---|---|---|
| 缺失 | 成功 | 用发现集合创建 |
| 缺失 | 失败/未配置 | 用模板 fallback 创建 |
| 已存在 | 成功且集合不同 | GET 完整渠道，只替换 models，去掉 status 后 PUT，再 GET 验证 |
| 已存在 | 成功且集合相同 | 零写入 |
| 已存在 | 失败/未配置 | 零写入 |

主渠道创建/对账失败为硬错误；Claude 渠道保持现有降级语义，只 warn。

模型比较忽略顺序、空白与重复，写入保留发现端首次出现的稳定顺序。更新前后验证
`priority/group/status` 不变；GET 返回的空 key 沿用 new-api 的“空 key 保留原值”契约。

## 4. AddKey

智谱额度探活通过后，分别调用与 sync 相同的“发现后创建”入口。这样新增 key 不使用进程启动时
可能已经过时的模型目录。发现失败只降级到 fallback，不阻塞录入；创建失败的主/Claude 分级
与现有逻辑一致。

## 5. 测试与验收

- catalog parser：合法、空白、重复、空 data、非法结构；
- model set：顺序/重复不触发漂移，真实增删触发；
- config：旧配置兼容、auth 默认与三种枚举、非法 URL；
- planner：已存在的渠道也携带模板/key，可进入对账；
- 全量单元测试；
- 合并/上线前用真实智谱 key 执行一次 `sync`，确认渠道包含 `/models` 最新集合且
  priority/group/status 未变。
