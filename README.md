# quota-throttle

一个独立的小守护进程：轮询智谱 GLM Coding Plan 的用量 API，通过 **new-api 的管理 API** 用 `priority` **钉住单把活动 key**，让 opencode / Claude Code 的流量连续打在同一把 key 上（prompt 缓存能持续命中）；当这把 key 的 **5 小时**或**每周**窗口用量逼近阈值（默认 95%）时，自动把活动 key 切到下一把还有额度的 key。

它**不碰 new-api 本体**，只是个外挂脚本。转发、多 key 兜底、撞 429 反应式切换仍由 new-api 负责；这个进程只补上 new-api 缺的那一环——**由外部集中决定「现在走哪一把 key」**，既预防式在撞墙前主动切，又保住缓存局部性。

```
        智谱 quota API（每把 key 的 5h/周 已用%）
                    │  本进程轮询
                    ▼
        quota-throttle
                    │  钉住活动 key：PUT /api/channel 把它 priority 抬到最高
                    │  活动 key ≥95% → 把下一把有额度的 key 抬为新活动
                    ▼
        new-api 管理 API
                    │
                    ▼
        new-api 路由层（优先打最高 priority 的渠道 + 429 反应式兜底）
                    ▲
                    │ base_url 指这里
            opencode / Claude Code
```

## 为什么用 priority 钉住单把，而不是加权分散

new-api 默认在同组同模型的健康渠道里**按 weight 加权随机**，每个请求可能落到不同 key。对无状态的 `chat/completions` 这本身没问题，但如果上游 **prompt 缓存是按账号/key 隔离的**，请求分散会让 Claude Code / opencode 那种「系统提示 + 长上下文大量复用」的缓存命中率下降 → 成本和首 token 延迟变差。

所以本程序把「选哪把 key」的决策收回来，用**三档 priority** 钉住单把：

| 档位 | 默认 priority | 含义 |
|------|--------------|------|
| active | 100 | 所有正常流量都走它（缓存连续命中） |
| standby | 10 | 有额度、平时不碰，只作 429 兜底目标 |
| exhausted | 0 | 逼近/超阈值，最后手段 |

**关键：用 priority 而不是 weight=0。** new-api 优先路由最高 priority 的渠道；活动 key 万一撞 429，它能沿 priority 阶梯自动跌到还有额度的 standby 渠道。而 `weight=0` 会把渠道从选择集里抹掉，破坏这层反应式兜底。

**切换策略（能不换就不换，护缓存）**：当前活动 key 只要 `pct < throttle(95)` 就继续钉住；到 95% 才切走，在有额度的其余 key 里挑 **`pct` 最低（剩余最多）** 的当新活动，让它撑最久、切换最少。活动 key 一旦被切走就没流量，`pct` 停在高位直到窗口重置（一次性大跌），天然不回跳，不需要滞回防抖。

## 前置：先把 new-api 侧准备好

1. **一把智谱 key = 一个独立渠道**。不要把多把 key 塞进一个渠道内部轮询——那样无法单独对某把 key 调 priority。多个渠道放同一分组、配同一模型名即可。
2. 在 new-api 后台先手动验证：opencode 改 `base_url` 指向 new-api 能正常出结果，且撞 429 会自动切下一个渠道。**这步不涉及本程序**，是基础设施验证。
3. 在后台【个人设置 / Profile】生成 **系统访问令牌**（管理员身份，区别于发给 opencode 的调用令牌）。

## ⚠️ 必须用 F12 核实的几处（版本差异所在）

new-api 的管理 API 没有稳定的官方文档，路由和 header 随版本变。打开后台，F12 → Network，手动点一次「编辑渠道 → 改优先级 → 保存」，核对：

- **渠道路径**：多为 `GET /api/channel/{id}` 和 `PUT /api/channel`，填进 `channel_path`。
- **priority 字段**：确认渠道对象里确实有 `priority`，且**路由确实优先取最高 priority**（抓一次 `/api/channel` 列表看字段名）。绝大多数版本如此，但值得亲自确认——这是本程序赖以工作的前提。
- **额外 header**：不少版本需要 `New-Api-User: <管理员 user id>`，漏了会 401。抓到就填进 `[[new_api.extra_headers]]`，不需要就删掉那段。

同理，智谱用量响应的字段名（`success` / `data.limits[].type` / `percentage` / `nextResetTime`）也请用一把真实 key 手动打一次
`https://open.bigmodel.cn/api/monitor/usage/quota/limit`（Header: `Authorization: <key>`）核对；不一致就改 `src/quota.rs` 里的 `Raw*` 结构体。

- **团体/企业套餐必读**：该接口对团队套餐通常要求带 org/project selector header（z.ai 版是 `Bigmodel-Organization` / `Bigmodel-Project`），**漏了会返回 `success:true` 但 `limits` 为空**——用量恒判 0%、永不切换。启动后日志若出现 `limits 为空` 的 WARN 就是这个原因。F12 抓后台用量页那条 quota 请求，把它带的 selector header 原样填进 `[[zhipu.extra_headers]]`。
- **认证/反爬**：社区反馈不一——有的场景 `Authorization: <key>` 直连即可，有的场景该端点有反爬（校验 UA/浏览器指纹），headless 会被挡。请先用你的真实 key 直连打一次确认能拿到数据，再上本程序。

### 关于用量数据的更新延迟

`percentage` 是实时还是有滞后，**智谱官方和社区都没有公开文档**。社区监控工具的默认刷新间隔从 10 分钟到几十秒不等，说明没人拿到过确切数字。唯一可靠的办法是**自己测**：拿一把 key 打一批已知量的请求，然后每几秒 poll 一次这个端点，看 `percentage` 多久才动——这个实测延迟就是 `poll_interval_secs` 的下限（比它还密没意义）。

## 用法

```bash
cp config.example.toml config.toml
# 编辑 config.toml，dry_run 先保持 true

cargo run --release -- config.toml     # 空跑：只打印决策，不真调 new-api
# 看日志确认活动 key 选择、切换判断、窗口识别都对，再把 dry_run 改成 false 正式生效
```

日志级别用 `RUST_LOG` 控制，例如 `RUST_LOG=debug cargo run -- config.toml`。

## 设计要点

- **单活动 key + priority 钉住**：护住 prompt 缓存局部性；切换靠调 priority，不动 `status`，避免和 new-api 自带的「渠道失败自动禁用/自动测试重启用」逻辑打架。
- **粘滞切换**：活动 key 撑到 95% 才换；挑新活动时要求 `pct < restore(90)` 多留余量 → 切换更少。活动 key 被切走后无流量、pct 不回落，天然不横跳。
- **窗口独立**：任一监控窗口达阈值即视为该 key 满，触发切换。5h 墙和周墙取最大使用率一起看。
- **429 兜底不丢**：非活动 key 保留在 standby 档，活动 key 预测漏判先撞墙时，new-api 反应式沿 priority 阶梯自动兜底。
- **鲁棒**：单把 key 查询失败只 warn 跳过本轮，不参与决策也不动其 priority；活动 key 查询失败时保持不变，不因瞬时抖动丢缓存。改 priority 用「GET 渠道 → 只改 priority → PUT 回」，整体搬运，对 new-api 字段差异不敏感。幂等下发：priority 没变就不重复 PUT。

## 已知边界 / 待你决定

- **吞吐上限不变，但更集中**：一把 key 会被灌到 ~95% 才换下一把。要保证有**足够多的 key 覆盖一个 5h 窗口**，否则全灌满只能等重置。总额度不够时，钉不钉都会 429。
- **5h 与周窗口的区分**靠 `nextResetTime` 升序排（5h 重置更早 → 排前）。这是照社区脚本的启发式，请用真实响应验证；若智谱返回里有更明确的窗口标识字段，改用那个更稳。
- **多进程**：本进程是集中式的（一个进程管所有 key），这正是它相对「每个 opencode 进程各跑一份插件」的优势。别在多台机器上各跑一份指向同一批 key，否则状态会打架。
- **合规**：多个**个人** Coding Plan 拿来做 key 池扛团队/多实例用量，可能违反智谱套餐条款（已有封号先例）。实验室规模的正规做法是企业版或按量 API。
- **模块可复用**：`src/quota.rs` 的 `QuotaProbe` 与 new-api 解耦。将来若迁到 litellm-rs 或自研网关，这块原样搬走即可。

## License

MIT
