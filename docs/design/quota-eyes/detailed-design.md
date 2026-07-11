# 细化设计 — quota-eyes（切换层的用量读取）

## 1. 范围

**目标**：修复智谱用量读取，使切换层能拿到每把 key 的真实 5 小时 / 每周窗口百分比，从而支持预防式（95% 阈值）切换。

**覆盖模块 / 函数**：
- `config.rs`：`ZhipuConfig`（精简）、`KeyMapping`（新增 per-key selector）、`ResolvedKey`（透传）
- `quota.rs`：`QuotaProbe::new`、`QuotaProbe::query`（鉴权修复 + per-key header）；**删除** `probe_query` 及 probe 模式
- `orchestrator.rs`：仅调用点适配，**决策逻辑不动**
- `config.toml` / `config.example.toml`：配置值

**不在范围**：orchestrator 的 active/standby/exhausted 决策与滞回、priority 三档、new-api 托管与建渠道——均已实测通过，本次不碰。

## 2. 与已有代码的复用点

| 复用项 | 是否需适配 |
|--------|-----------|
| `QuotaProbe` 的 monitor 查询骨架 + `Raw*` 反序列化结构 | 保留，仅改鉴权与 header 组装 |
| `unit`/`number` 分窗逻辑（`unit=3&number=5`→5小时；`unit=6&number=1`→每周） | **原样保留**——已用真实响应验证正确 |
| `nextResetTime` 升序排序分窗 | 保留为回退路径（字段缺失时） |
| `HeaderKV` 类型 | 直接复用，无需包装 |
| orchestrator 的决策/滞回/幂等下发 | 不改 |
| reqwest client | 不改 |

## 3. 错误处理策略

- **错误模型**：沿用 `anyhow::Result`。
- **跨模块传播**：`QuotaProbe::query` 返回 `Err` → `orchestrator::tick` 捕获后 `warn` 并**跳过该 key**（不进 `pct` map ⇒ 不参与本轮决策、不改其 priority）。此行为已存在，本次不变。
- **`success=false`**：`bail!` 并带上智谱 `msg`（例如缺 selector 时的「当前用户不存在coding plan」），便于用户定位配置问题。
- **`limits` 为空**：不 `bail`，改为 `warn`（提示「可能缺 org/project selector」）并返回空 `QuotaStatus`——上游 `max_watch_pct` 得 0，该 key 会被当作「有额度」。**已知风险**：若长期缺 selector，会误判为永远有额度。以 warn 提示 + 文档说明缓解（不引入静默失败的硬失败，避免单 key 配置问题拖垮整个循环）。

## 4. 数据结构定义

### 4.1 `KeyMapping`（修改；跨模块共享——config → orchestrator → quota）

```rust
pub struct KeyMapping {
    pub name: String,
    pub zhipu_api_key: String,
    pub channel_id: Option<i64>,
    /// 【新增】该 key 查询用量时附加的 selector header。
    /// 团体套餐必需（Bigmodel-Organization / Bigmodel-Project）；
    /// 不同 key 可能属于不同组织/项目，故按 key 配置而非全局。
    pub quota_headers: Vec<HeaderKV>,   // #[serde(default)]
}
```
- **不变量**：为空 ⇒ 该 key 是个人套餐或无需 selector；非空 ⇒ 逐条附加到用量请求头。

### 4.2 `ResolvedKey`（修改；跨模块共享）

新增 `quota_headers: Vec<HeaderKV>`，由 `main::resolve_keys` 从 `KeyMapping` 原样透传给 orchestrator，再传给 probe。

### 4.3 `ZhipuConfig`（修改；模块私有）

- **删除**：`mode` / `probe_url` / `probe_model`（probe 探测模式撤除——已被真百分比方案取代）
- **保留**：
  - `quota_url`：默认值改为带 `?type=2`（团体额度）。个人套餐用户去掉该查询参数。
  - `extra_headers`：**全局默认 selector**，作为 per-key 未配置时的兜底（两把 key 同组织时只写一遍）。

### 4.4 `QuotaStatus` / `WindowStatus`（不变）

沿用 `five_hour` / `weekly`（`percentage`、`next_reset_time`）。

## 5. 模块细化

### 5.1 config.rs

#### 5.1.1 `ZhipuConfig` 精简
- **功能**：去掉 probe 相关三字段；`default_quota_url` 返回带 `?type=2` 的 URL。
- **实现思路**：纯字段增删，无逻辑。`quota_url` 允许用户写完整含 query string 的 URL ⇒ **不引入新的 `type` 配置字段**（最小改动原则）。
- **正确性**：trivial（纯声明式配置）。

#### 5.1.2 `KeyMapping.quota_headers`
- **功能**：per-key selector。
- **实现思路**：`#[serde(default)]` ⇒ 旧配置（无此字段）仍可加载，向后兼容。
- **正确性**：trivial。

#### 5.1.3 `ResolvedKey` 透传
- **调用关系**：callers: `main::resolve_keys`；callees: 无。
- **实现思路**：构造 `ResolvedKey` 时 `quota_headers: k.quota_headers.clone()`。
- **正确性**：trivial（字段搬运）。

### 5.2 quota.rs

#### 5.2.1 `QuotaProbe::new(cfg: &ZhipuConfig) -> Self`
- **功能**：构造探针，持有 url 与**全局兜底 header**。
- **实现思路**：删去 probe 三字段的读取；其余不变。
- **正确性**：trivial。

#### 5.2.2 `QuotaProbe::query(&self, api_key: &str, key_headers: &[HeaderKV]) -> Result<QuotaStatus>`
- **功能描述**：查询单把 key 的 5 小时 / 每周窗口已用百分比。
- **调用关系**：callers: `orchestrator::tick`；callees: `reqwest`、`RawLimit::to_window`。
- **签名变更**：新增 `key_headers` 参数。
- **实现思路**（推导链）：
  1. `GET self.url`（url 已含 `?type=2`）。
  2. **鉴权**：`Authorization: Bearer <api_key>` —— **本次核心修复**。原实现用裸 key（无 `Bearer`），是「无 coding plan」的成因之一。依据：CodexBar zai.md 文档 + 真实响应验证。
  3. 附加 `Accept: application/json`、`Content-Type: application/json`、`User-Agent`。
  4. **header 组装顺序**：全局 `self.extra_headers` 先加，**再加** `key_headers` ⇒ per-key 覆盖全局（reqwest 同名 header 后者追加/覆盖，语义上以 per-key 为准）。
  5. 反序列化 `RawResponse`；`success == false` ⇒ `bail!(msg)`。
  6. 取 `data.limits`，**只留 `TOKENS_LIMIT`**（`TIME_LIMIT` 是 MCP 搜索次数，非用量窗口）。
  7. **分窗**：优先按 `unit`/`number`（`3/5`→`five_hour`，`6/1`→`weekly`）；两字段皆缺则回退到 `nextResetTime` 升序（早者为 5 小时）。
  8. `limits` 为空 ⇒ `warn`（提示缺 selector），返回空 `QuotaStatus`。
- **分支覆盖**：网络失败 → `Err`；`success=false` → `Err`；`data` 缺失 → `Err`；`limits` 空 → `Ok(空)` + warn；正常 → `Ok(含窗口)`。
- **退出点**：以上 5 条全覆盖。
- **显式假设**：
  - 智谱返回 `data.limits[]`，每项含 `type`/`percentage`/`unit`/`number`/`nextResetTime`（**已用真实响应验证**：level=max，5h=unit3/num5，周=unit6/num1）。
  - `?type=2` 为团体额度作用域（文档 + 实测：不带则「无 coding plan」）。
- **正确性论证**：查询为无副作用的纯读取；分窗由 `(unit, number)` 唯一确定（两组值互斥），故 `five_hour` / `weekly` 至多各被赋值一次；回退路径仅在两字段全缺时触发，与主路径互斥。

#### 5.2.3 删除 `probe_query`
- 连同 `ZhipuConfig` 的 probe 字段一并移除。理由：真百分比可得 ⇒ 探测（二值信号 + 消耗额度）无必要。

### 5.3 orchestrator.rs

#### 5.3.1 `tick()` 调用点适配
- **改动**：`self.probe.query(&k.zhipu_api_key)` → `self.probe.query(&k.zhipu_api_key, &k.quota_headers)`。
- **不变**：`max_watch_pct`、active 粘滞选择、standby/exhausted 分档、幂等 priority 下发、查询失败跳过——全部保持。
- **正确性**：仅参数透传，决策语义不变。

### 5.4 配置文件

- `quota_url = "https://open.bigmodel.cn/api/monitor/usage/quota/limit?type=2"`
- 每个 `[[keys]]` 下：
  ```toml
  [[keys.quota_headers]]
  key = "Bigmodel-Organization"
  value = "org-..."
  [[keys.quota_headers]]
  key = "Bigmodel-Project"
  value = "proj_..."
  ```

## 6. 完整性自检 checklist

- [x] 所有函数实现思路推导连续（`query` 8 步无跳步）
- [x] 所有分支已覆盖（网络失败 / success=false / data 缺失 / limits 空 / 正常）
- [x] 所有退出点已刻画（4 条 Err + 2 条 Ok）
- [x] callee 引用显式（reqwest 发送、`to_window` 构造）
- [x] 无循环需终止性论证（`limits` 遍历为有界 for）
- [x] 上游事实显式列出（响应字段结构、`type=2` 语义——均经真实响应验证，非推测）

## 7. 验证计划

1. `cargo build` 零 warning。
2. **真实读取**：用真 key + per-key selector 跑一轮，日志应出现两把 key 的真实 5h/周 百分比（预期 zhipu-1 ≈ 2%/33%，zhipu-2 ≈ 1%/15%）。
3. **dry_run 决策验证**：确认 active 选中用量更低者、另一把置 standby。
4. **真跑**：`dry_run=false`，核对 new-api 中 priority 实际变为 100 / 10。
