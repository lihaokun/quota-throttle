# quota-throttle

一个独立的小守护进程：轮询智谱 GLM Coding Plan 的用量 API，通过 **new-api 的管理 API** 用 `priority` **钉住单把活动 key**，让 opencode / Claude Code 的流量连续打在同一把 key 上（prompt 缓存能持续命中）；当这把 key 的 **5 小时**或**每周**窗口用量逼近阈值（默认 95%）时，自动把活动 key 切到下一把还有额度的 key。

转发、多 key 兜底、撞 429 反应式切换仍由 new-api 负责；这个进程补上 new-api 缺的那一环——**由外部集中决定「现在走哪一把 key」**，既预防式在撞墙前主动切，又保住缓存局部性。

它还能**替你把 new-api 起和配好**（可选）：`up` 子命令会在 new-api 不在跑时，按平台从 GitHub release 下载 new-api 二进制（sha256 校验）、原生进程启动、首启自动建管理员，再按你的 key 列表建好渠道——零前置一条命令拉起整套。不想让它管 new-api，删掉配置里的 `[new_api.manage]`、用 `run` 子命令即可，退回纯外挂模式。

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

## 两种用法

**A. 让本工具全托管（推荐，零前置）** —— 配上 `[new_api.manage]` 和 `[new_api.channel_template]`，其余交给 `up`：

```bash
cp config.example.toml config.toml   # 填智谱 key；dry_run 先保持 true
cargo run --release -- up config.toml
```

`up` 会：new-api 不在跑就下载 release 二进制（sha256 校验）+ 启动 → 首启自动建管理员（`root` / `root_password`，密码需 ≥8 位，**首启后请登录 UI 改掉**）→ 按 key 列表建渠道 → 进入切换循环。停： `... down config.toml`。

> 一把智谱 key = 一个独立渠道，本工具自动一一建好。只有 **≥2 把 key** 时「切换」才有意义。

**B. 你自己已有 new-api** —— 删掉 `[new_api.manage]`，填 `admin_token`（后台【个人设置】生成的系统访问令牌）或 root 账号，用 `run`（只解析已有渠道并切换）或 `sync`（先建/对齐渠道）。这步先在后台手动验证过 opencode 指向 new-api 能出结果、撞 429 会自动切渠道。

## ⚠️ 必须用 F12 核实的几处（版本差异所在）

new-api 的管理 API 没有稳定的官方文档，路由和 header 随版本变。打开后台，F12 → Network，手动点一次「编辑渠道 → 改优先级 → 保存」，核对：

- **渠道路径**：多为 `GET /api/channel/{id}` 和 `PUT /api/channel`，填进 `channel_path`。
- **priority 字段**：确认渠道对象里确实有 `priority`，且**路由确实优先取最高 priority**（抓一次 `/api/channel` 列表看字段名）。绝大多数版本如此，但值得亲自确认——这是本程序赖以工作的前提。
- **额外 header**：不少版本需要 `New-Api-User: <管理员 user id>`，漏了会 401。抓到就填进 `[[new_api.extra_headers]]`，不需要就删掉那段。

同理，智谱用量响应的字段名（`success` / `data.limits[].type` / `percentage` / `nextResetTime`）也请用一把真实 key 手动打一次
`https://open.bigmodel.cn/api/monitor/usage/quota/limit`（Header: `Authorization: <key>`）核对；不一致就改 `src/quota.rs` 里的 `Raw*` 结构体。

- **认证（已核对 opencode 插件源码，可 headless）**：直接 `Authorization: <裸 key>`（**无 `Bearer` 前缀**）即可，本程序已这么做，并附带一个明确 `User-Agent`。社区那句「只认浏览器 cookie / 反爬」是另一条路——那是拿**网页会话 cookie**（`bigmodel_token_production`）去打；用 **API key** 直连 headless 是通的（opencode-mystatus、opencode-glm-quota 两个插件都这么干）。
- **团体/企业套餐**：现有能跑通的实现里，quota 请求都**没带** org/project header，所以多半不需要。但若你 F12 发现自己的团队 key 必须带某个 selector（如 `Bigmodel-Organization` / `Bigmodel-Project`），塞进 `[[zhipu.extra_headers]]` 即可。启动后若日志出现 `limits 为空` 的 WARN，就往这个方向查。

### 关于用量数据的更新延迟

`percentage` 是实时还是有滞后，**智谱官方和社区都没有公开文档**。社区监控工具的默认刷新间隔从 10 分钟到几十秒不等，说明没人拿到过确切数字。唯一可靠的办法是**自己测**：拿一把 key 打一批已知量的请求，然后每几秒 poll 一次这个端点，看 `percentage` 多久才动——这个实测延迟就是 `poll_interval_secs` 的下限（比它还密没意义）。

## 子命令

| 子命令 | 作用 |
|--------|------|
| `up`   | 配了 `[new_api.manage]` 时下载/启动 new-api → sync 建渠道 → 进入切换循环 |
| `sync` | 只建/对齐渠道并打印 `name → channel_id`，不进循环 |
| `run`  | 假设 new-api 已在跑，只解析已有渠道并进入切换循环 |
| `down` | 停掉本工具托管的 new-api |

省略子命令按 `run` 处理。先保持 `dry_run = true` 空跑，看日志确认活动 key 选择、切换判断、窗口识别都对，再改 `false` 正式生效。日志级别用 `RUST_LOG` 控制（如 `RUST_LOG=debug`）。

> 首启凭据：托管模式下本工具用 `root` / `root_password` 创建 new-api 管理员（新版 new-api 不再有默认 root/123456）。**首启后请登录 UI 改密码**，或改用你自己生成的 `admin_token`。

## 设计要点

- **单活动 key + priority 钉住**：护住 prompt 缓存局部性；切换靠调 priority，不动 `status`，避免和 new-api 自带的「渠道失败自动禁用/自动测试重启用」逻辑打架。
- **粘滞切换**：活动 key 撑到 95% 才换；挑新活动时要求 `pct < restore(90)` 多留余量 → 切换更少。活动 key 被切走后无流量、pct 不回落，天然不横跳。
- **窗口独立**：任一监控窗口达阈值即视为该 key 满，触发切换。5h 墙和周墙取最大使用率一起看。
- **429 兜底不丢**：非活动 key 保留在 standby 档，活动 key 预测漏判先撞墙时，new-api 反应式沿 priority 阶梯自动兜底。
- **鲁棒**：单把 key 查询失败只 warn 跳过本轮，不参与决策也不动其 priority；活动 key 查询失败时保持不变，不因瞬时抖动丢缓存。改 priority 用「GET 渠道 → 只改 priority → PUT 回」，整体搬运，对 new-api 字段差异不敏感。幂等下发：priority 没变就不重复 PUT。

## 已知边界 / 待你决定

- **吞吐上限不变，但更集中**：一把 key 会被灌到 ~95% 才换下一把。要保证有**足够多的 key 覆盖一个 5h 窗口**，否则全灌满只能等重置。总额度不够时，钉不钉都会 429。
- **5h 与周窗口的区分**已按智谱返回的 `unit`/`number` 字段精确判定（`unit=3&number=5` → 5小时；`unit=6&number=1` → 每周，核对自 opencode-glm-quota 源码）。若某版本没这俩字段，自动回退到「按 `nextResetTime` 升序，早的当 5h」的启发式。
- **多进程**：本进程是集中式的（一个进程管所有 key），这正是它相对「每个 opencode 进程各跑一份插件」的优势。别在多台机器上各跑一份指向同一批 key，否则状态会打架。
- **合规**：多个**个人** Coding Plan 拿来做 key 池扛团队/多实例用量，可能违反智谱套餐条款（已有封号先例）。实验室规模的正规做法是企业版或按量 API。
- **模块可复用**：`src/quota.rs` 的 `QuotaProbe` 与 new-api 解耦。将来若迁到 litellm-rs 或自研网关，这块原样搬走即可。

## License

MIT
