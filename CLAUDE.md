# quota-throttle

> 轮询智谱 GLM Coding Plan 用量，通过 new-api 用 `priority` 钉住单把活动 key、逼近额度时自动切到下一把；并能替用户托管 new-api（下载二进制 / 启动 / 建渠道）。目标：护住 prompt 缓存局部性 + 预防式绕开撞墙。

## 项目结构

```
src/
  main.rs          — 子命令 up/down/sync/run 编排
  config.rs        — TOML 配置加载
  quota.rs         — 智谱用量探针（“眼睛”）
  newapi.rs        — new-api 管理 API 客户端（登录/建渠道/改 priority）
  boot.rs          — 下载 + 托管 new-api 原生进程
  orchestrator.rs  — 控制循环：按用量选活动 key、重排 priority
docs/
  workflow.md      — 开发工作流程规范（@docs/workflow.md）
config.example.toml / config.toml(gitignored)
```

## 技术栈

- 语言：Rust 2021
- 构建：Cargo（本机 rustc 1.95 nightly）
- 依赖：tokio、reqwest(rustls,cookies)、serde/serde_json、toml、tracing、anyhow、sha2
- 测试：目前**以端到端实测为主**（对真实 new-api + 真 key 驱动），单元测试待补

## 常用命令

```bash
cargo build --release
cargo run --release -- up config.toml     # 起 new-api + 建渠道 + 切换循环
cargo run --release -- sync config.toml    # 只建/对齐渠道并打印 name→channel_id
cargo run --release -- down config.toml    # 停 new-api
# 托管的 new-api 数据在 ./.newapi/（SQLite=one-api.db、二进制、日志、PID）
```

## 代码风格

- 命名：snake_case
- 注释语言：中文
- 改 new-api 渠道字段用「GET → 只改目标字段 → PUT」整体搬运，对版本差异鲁棒

## 参考实现（交叉检验用，勿凭记忆猜其行为——查源码/实测）

- **new-api**（`QuantumNous/new-api`，旧名 `Calcium-Ion/new-api`）：管理 API、渠道类型码、setup 契约。源码用 `gh api repos/QuantumNous/new-api/contents/<path>?ref=<tag>` 拉。
- **opencode**（`sst/opencode`）：`zhipuai-coding-plan` provider 定义、config/auth 优先级。
- **models.dev**：opencode 的 provider 注册表（本机缓存 `~/.cache/opencode/models.json`）。

## 已知限制与注意事项（血泪，务必先读再动手）

- **用量读取（已解决，勿再走弯路）**：团体 coding plan 的用量**能用 API key 读**，三个条件缺一不可——
  ① url 带 **`?type=2`**（团队额度作用域；`type=3` 是团队小时用量）；
  ② **`Authorization: Bearer <key>`**（**必须带 Bearer**，裸 key 不行）；
  ③ 带 **`Bigmodel-Organization`** / **`Bigmodel-Project`** selector header。
  缺任一 → 返回「当前用户不存在coding plan」或 limits 空。org/project id 取法：浏览器开
  `https://bigmodel.cn/coding-plan/team/usage-stats` → F12 Network → 找 `quota/limit` 请求 → 抄这两个头。
  **selector 按 key 配**（不同 key 可能属不同组织/项目）。依据：CodexBar `docs/zai.md` + 实测。
  返回：`level`(如 max) + `limits[]`，`unit=3&number=5`→5小时窗口、`unit=6&number=1`→每周窗口、
  `TIME_LIMIT`(unit=5)=MCP 搜索次数（非用量窗口，须过滤）。
- **⚠️ 教训（本项目最大的坑不是技术，是流程）**：我曾因用错鉴权（裸 key、缺 type/selector）就断言「用量读不到」，
  进而设计出「推理探测」的弯路，被用户三次打断。**根因是跳过调研直接下结论**。
  凡是「某接口不行」的结论，必须先查官方文档 + 社区实现 + 实测三者交叉验证，再下结论。
- **智谱 coding 口** = `https://open.bigmodel.cn/api/coding/paas/v4/chat/completions`（`/v4` 不是 `/v1`，`/coding/` 不是普通 `/paas/`）。opencode `zhipuai-coding-plan` 用 `@ai-sdk/openai-compatible` 打 `{api}/chat/completions`。
- **new-api 渠道必须 Custom 类型(8)**：base_url 原样透传全路径。OpenAI 类型(1) 会拼成 `.../v4/v1/chat/completions` → 智谱 404。（类型码：OpenAI=1, Custom=8, Zhipu=16, ZhipuV4=26）
- **new-api 建渠道 payload** 要 `{mode:"single", channel:{...}}` 包裹；`channel` 是指针，平铺会 nil-panic 500。字段：`type/key/base_url/models(逗号串)/group/priority/weight/status`。
- **new-api PUT /api/channel 拒绝带 `status` 字段**的请求体（判 Invalid parameters）；改字段前必须 `obj.remove("status")`。GET 单渠道返回的 `key` 是空串，PUT 空 key 会保留原值（安全）。
- **new-api 首启无默认 root/123456**：需先 `POST /api/setup {username,password,confirmPassword,SelfUseModeEnabled}`（密码≥8位、用户名≤12）建管理员，再登录拿会话。
- **new-api 令牌 key 在列表里打码**（`aK1A****7H3Z`），真实值从 SQLite `tokens.key` 读；POST/PUT 到 `/api/xxx/` 要带**尾斜杠**（否则 307，reqwest 会自动跟随、urllib 不会）。
- **new-api release 有独立二进制**（linux/arm64/macos/win），自带 SQLite，`PORT` env 指定端口；默认只在 **401** 自动禁用渠道（429/耗尽不禁），耗尽报文是中文「已达到…使用上限」不撞其英文禁用关键词 → 恢复干净。
- **探测成本坑**：glm 是推理模型，`max_tokens:1` 挡不住思考（烧 ~660 token）；`thinking:{type:"disabled"}` 才压到 ~7 token。
- **认证**：智谱各口用 `Authorization: Bearer <裸 key>`（coding/推理口）；monitor 口社区脚本用裸 key（无 Bearer），但对团体 coding plan 无效。

## 工作流程

遵循 @docs/workflow.md。核心铁律（我此前反复违反，务必守住）：

```
新功能开发：调研 → [确认] → 架构 → [确认] → 细化 → [确认] → 审查 → 逐模块实现+测试+审核
每步操作：说明计划 → [等待确认] → 执行单步 → 报告结果 → [等待反馈]
```

**不确认不实现。不跳过设计直接写码。调研靠查源码/实测，不靠猜。**
