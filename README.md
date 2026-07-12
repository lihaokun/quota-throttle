# quota-throttle

给智谱 GLM Coding Plan **多 key 池**做**预防式调度**的守护进程：轮询每把 key 的真实用量（5 小时 / 每周窗口），通过 new-api 用 `priority` **钉住单把活动 key**（让 prompt 缓存能连续命中），当它逼近额度上限时**在撞墙前**自动切到下一把还有额度的 key。

顺带把 new-api 也管了：`up` 一条命令自动下载 new-api 二进制、拉起进程、按 key 列表建好渠道，并提供一个状态看板。

```
        智谱 quota API（每把 key 的 5h / 周 已用%）
                    │  每 60s 轮询
                    ▼
        ┌──────────────────────┐        看板 :3001
        │   quota-throttle     │◀───────（每把 key 的用量 / 档位 / 活动 key /
        │  · 选活动 key         │         new-api 渠道实况 / 请求流水）
        │  · 写 priority        │
        │  · 托管 new-api 进程   │
        └──────────┬───────────┘
                   │ PUT /api/channel（只改 priority，从不碰 status）
                   ▼
            new-api :3000  ──────▶  智谱 coding 口
                   ▲                /api/coding/paas/v4/chat/completions
                   │ base_url 指这里
            opencode / Claude Code
```

## 为什么钉住单把 key，而不是加权分散

new-api 默认在健康渠道间**按 weight 加权随机**——每个请求可能落到不同 key。对无状态的 `chat/completions` 本身没问题，但智谱的 **prompt 缓存是按 key 隔离的**：请求分散会让 opencode / Claude Code 那种「系统提示 + 长上下文大量复用」的缓存命中率大跌，成本和首 token 延迟都变差。

所以本工具把「现在走哪把 key」的决策收回来，用**三档 priority** 钉住单把：

| 档位 | priority | 含义 |
|------|----------|------|
| **active** | 100 | 所有正常流量都走它（缓存连续命中） |
| **standby** | 10 | 有额度、平时不碰，只作 429 兜底目标 |
| **exhausted** | 0 | 逼近/超阈值，最后手段 |

**关键：用 `priority` 而不是 `weight=0`。** new-api 优先路由最高 priority 的渠道；活动 key 万一预测漏判先撞了 429，它能沿 priority 阶梯自动跌到还有额度的 standby 渠道。`weight=0` 会把渠道从选择集里抹掉，破坏这层反应式兜底。

**切换策略（能不换就不换，护缓存）**：活动 key 只要 `pct < 95%` 就一直钉着；到阈值才切走，在有额度的其余 key 里挑 **用量最低（剩余最多）** 的当新活动，让它撑最久、切换最少。

## 快速开始

```bash
cp config.example.toml config.toml     # 填智谱 key + org/project selector（见下）
cargo run --release -- up config.toml  # 起 new-api + 建渠道 + 进入切换循环
```

`up` 会自动：new-api 不在跑 → 按平台下载 release 二进制（sha256 校验）→ 启动 → 首启建管理员 → 按 key 列表建渠道 → 进入切换循环 + 起状态看板。

| 子命令 | 作用 |
|--------|------|
| `up` | 下载/启动 new-api → 建渠道 → 切换循环 + 看板 |
| `sync` | 只建/对齐渠道并打印 `name → channel_id`，不进循环 |
| `run` | 假设 new-api 已在跑，只解析渠道并进入循环 |
| `down` | 停掉本工具托管的 new-api |

数据（SQLite / 二进制 / 日志 / PID）都在 `./.newapi/`。日志级别用 `RUST_LOG` 控制。

## 状态看板

`http://127.0.0.1:3001`（`status_addr` 可配，留空则不启用）

- 每把 key：**5 小时 / 每周窗口进度条**（95% 处画阈值线）+ **重置倒计时** + 档位徽章 + 当前 priority；活动 key 的卡片绿色高亮
- **new-api 渠道实况**：是否被 new-api **自动禁用**（红行告警——我们只改 priority、从不碰 status，渠道一旦被禁 priority=100 也没用，这是唯一的盲区）、priority 与我们下发值**对账**、累计花费、`auto_ban`
- **最近请求流水**：每条请求**落在哪个渠道**（绿点=活动 key / 黄点=掉到兜底渠道）、tokens、耗时
- **用量图**：`近 24 小时`（实时，5 秒刷）· `近 30 天`（每天一根柱，**点某天下钻到它的小时视图**）
- 查询失败显示「查询失败」而非 0%（不会骗你说还有额度）

`GET /api/status` 是同数据的 JSON 接口（供 opencode 插件等外部消费者）。看板每 5 秒刷新只读进程内快照，**不给 new-api 增加任何负载**。

历史区间走 `GET /api/usage?start=<unix>&end=<unix>`（按需查，不进快照），**每 5 分钟刷新**——这正是 new-api 把用量落库的节奏（`DataExportInterval`），刷得再勤也拿不到更新的数。纯过去的某一天数据已不再变，拉一次冻住，根本不刷。

## ⚠️ 三个必须知道的坑（都是踩出来的）

### 1. 团体套餐读用量：三个条件缺一不可

```
GET  https://open.bigmodel.cn/api/monitor/usage/quota/limit?type=2   ← ① 必须带 ?type=2
Authorization: Bearer <api key>                                      ← ② 必须带 Bearer（裸 key 不行）
Bigmodel-Organization: org-...                                       ← ③ 团体必需的 selector
Bigmodel-Project: proj_...
```

缺任一 → 返回 `当前用户不存在coding plan` 或 `limits` 为空（会被误当成 0% 用量、永不切换）。

**org / project id 取法**：浏览器打开 `https://bigmodel.cn/coding-plan/team/usage-stats` → F12 Network → 刷新 → 找 `quota/limit` 请求 → 抄它带的这两个请求头。

**selector 按 key 配**（`[[keys.quota_headers]]`）——不同 key 可能属于不同组织/项目。个人套餐去掉 `?type=2` 和 selector 即可。

返回里 `unit=3 & number=5` = 5 小时窗口，`unit=6 & number=1` = 每周窗口；`TIME_LIMIT` 是 MCP 搜索次数（非用量窗口，须过滤）。

### 2. new-api 渠道必须用 Custom 类型(8) + 全路径 base_url

智谱编码套餐口是 `.../api/coding/paas/v4/chat/completions`（`/v4` 不是 `/v1`，`/coding/` 不是普通 `/paas/`）。new-api 的 **OpenAI 类型(1)** 会拼成 `.../v4/v1/chat/completions` → 智谱 **404**。必须用 **Custom 类型(8)**（原样透传 base_url 全路径）：

```toml
[new_api.channel_template]
type = 8
base_url = "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions"
```

### 3. opencode 接入：改 provider 的 baseURL，并清掉 auth.json 里的智谱 key

opencode 的 `zhipuai-coding-plan` 是 **OpenAI 兼容** provider（`@ai-sdk/openai-compatible`），默认直连 `https://open.bigmodel.cn/api/coding/paas/v4`。把它指向 new-api：

```jsonc
// ~/.config/opencode/opencode.jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "provider": {
    "zhipuai-coding-plan": {
      "options": {
        "baseURL": "http://127.0.0.1:3000/v1",   // 指向 new-api
        "apiKey": "<new-api 调用令牌>"            // 不是智谱 key！
      }
    }
  }
}
```

**同时要把 `~/.local/share/opencode/auth.json` 里的 `zhipuai-coding-plan` 条目清掉**（备份后置空即可）——否则 opencode 可能优先用 auth.json 里的智谱 key 去连 new-api，被拒 401。

**模型名**只能用 opencode 该 provider 的这几个：`glm-4.7` `glm-5.1` `glm-5.2` `glm-5-turbo` `glm-5v-turbo` `glm-4.6v` `glm-4.5-air`（**没有 `glm-4.6`**）。

## 设计要点

- **单活动 key + priority 钉住**：护 prompt 缓存局部性。切换靠调 priority，**不动 `status`**，避免和 new-api 自带的「失败自动禁用」逻辑打架。
- **窗口取最大**：5 小时墙和周墙同时盯，任一达阈值即切。
- **粘滞 + 余量**：活动 key 撑到 95% 才换；挑新活动时要求 `< 90%` 多留余量 → 切换更少。活动 key 被切走后无流量、用量不回落，天然不横跳，无需滞回。
- **鲁棒**：单把 key 查询失败只 warn 跳过本轮（不参与决策、也不动它的 priority）；活动 key 查询失败时**保持不变**，不因瞬时抖动丢缓存。全部 key 无额度时**保留原活动**，交给 new-api 的 429 兜底。
- **幂等下发**：priority 没变就不重复 PUT。稳态下对 new-api **零写入**。
- **看板绝不拖垮主循环**：监听失败只降级记 error；渠道/日志拉取失败退化为空列表。
- **恢复干净**：智谱耗尽返回中文「已达到…使用上限」，不撞 new-api 的英文自动禁用关键词（默认只有 401 触发禁用）；渠道全程 enabled，窗口重置后自动恢复。

## 已知边界

- **吞吐上限不变，但更集中**：一把 key 会被灌到 ~95% 才换下一把。要有**足够多的 key 覆盖一个 5h 窗口**，否则全灌满只能等重置。总额度不够时，钉不钉都会 429。
- **轮询间隔**：默认 60s。轮询间隔内活动 key 可能冲过阈值一点点（实测你的用量强度下 < 1%，而 95%→100% 有 5% 余量，够）。多实例并发时可压到 30s。
- **单进程集中式**：别在多台机器各跑一份指向同一批 key，状态会打架。
- **合规**：多个**个人** Coding Plan 拼 key 池扛团队用量可能违反智谱条款。团体套餐是正规做法。
- **模块可复用**：`src/quota.rs` 的 `QuotaProbe` 与 new-api 解耦，将来迁到别的网关可原样搬走。

## 开发

遵循 `docs/workflow.md`（半形式化 SDD 流程）。设计文档在 `docs/design/`；项目约定与踩过的坑记在 `CLAUDE.md` 的「已知限制」段。

```bash
cargo build --release
```

## License

MIT
