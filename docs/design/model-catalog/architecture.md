# 架构设计 — 上游模型目录同步

> 调研：`docs/research/model-catalog-research.md`

## 1. 核心流程

```text
up / sync / AddKey
  ├─ 读取 channel template + 当前 upstream key
  ├─ ModelDiscoveryClient::discover(url, auth, key)
  │    ├─ 2xx + 非空 data[].id → Discovered(models)
  │    └─ 网络/鉴权/格式/空列表失败 → Unavailable(error)
  ├─ 新渠道
  │    ├─ Discovered → 用发现模型创建
  │    └─ Unavailable → 用 template.models fallback 创建
  └─ 已有渠道
       ├─ Discovered 且集合漂移 → GET channel → 只改 models → PUT → 回读
       ├─ Discovered 且集合相同 → 零写入
       └─ Unavailable → 保持原值，warn
```

发现不进入 quota tick，也不进入面板 5/10 秒刷新循环。

## 2. 核心数据结构

### 2.1 `ModelDiscoveryConfig`（config → model catalog）

```rust
pub struct ModelDiscoveryConfig {
    pub url: String,
    pub auth: ModelDiscoveryAuth,
}

pub enum ModelDiscoveryAuth {
    Bearer,            // Authorization: Bearer <key>
    AuthorizationRaw, // Authorization: <key>
    XApiKey,           // x-api-key: <key>
}
```

- consumer：当前 `ChannelTemplate`，未来迁移到 `api_groups[]`。
- `model_discovery` 缺失：若模板是已知的智谱 Coding 官方地址，则自动补官方 `/models` +
  Bearer；其它 Custom 上游保持关闭，绝不猜 URL/鉴权。
- URL 必须是绝对 `http/https` URL；空白或其它 scheme 启动失败。

### 2.2 `ModelCatalogResult`（module-private）

```rust
enum ModelCatalogResult {
    Discovered(Vec<String>),
    Unavailable(anyhow::Error),
}
```

`Discovered` 不允许空列表；模型 ID 已 trim、过滤空串、按首次出现去重。

## 3. 模块划分与功能规约

### 3.1 `config.rs`

- 为渠道模板增加可选 `model_discovery`。
- 保留 `models` 字段，语义改为“发现关闭或失败时的新渠道 fallback”。
- 校验 URL 与 auth 枚举；旧智谱配置自动迁移为发现开启，其它旧配置保持原行为。

### 3.2 `model_catalog.rs`（新增）

- 只负责外部 `/models` 请求、鉴权头、响应解析与规范化。
- 不知道 new-api channel、priority、group 或 config 文件。
- 错误必须脱敏；不得把 key、Authorization 值或完整 Request Debug 输出。

### 3.3 `newapi.rs`

- `sync_channels()` 为每个 key/模板解析模型来源。
- 已存在渠道不再无条件 Skip：发现成功后比较并仅在漂移时更新 models。
- 模型更新复用 GET→只改 models→去 status→PUT，并回读断言；不动 key/priority/status/group。
- 同一 `(key name, discovery URL, auth)` 在一次 sync 中最多请求一次。

### 3.4 `orchestrator.rs`

- AddKey 在建渠道前调用同一发现能力，不再依赖进程启动时缓存的 `models`。
- 发现失败不阻断新增：按 fallback 创建并把降级信息返回/记录。
- quota 决策、active/pin/priority 逻辑完全不变。

### 3.5 `status.rs`（本阶段最小改动）

- 本阶段不新增模型编辑 UI。
- AddKey 成功信息可带 `models_source = discovered|fallback`，为后续 UI 留契约。

## 4. 接口规约

| 调用方 → 被调方 | 输入 | 成功输出 | 失败行为 |
|---|---|---|---|
| config → catalog | URL/auth/key | 合法发现请求参数 | 配置非法启动失败 |
| catalog → newapi sync | 发现配置 + key | 非空规范化模型列表 | existing 保持；create fallback |
| newapi sync → channel API | channel id + models | 仅 models 改变且回读一致 | 返回错误，不伪报成功 |
| orchestrator AddKey → catalog | 新 key | 建渠道模型列表 | fallback，不清空 |

## 5. 关键设计决策

1. **发现成功即权威**：避免配置和真实能力长期漂移。
2. **失败不覆盖 existing**：网络抖动不能删掉线上 ability。
3. **配置 fallback 只服务新建**：没有已有状态可保留时仍可录入 key。
4. **集合比较、稳定写序**：上游仅改变顺序时不产生 new-api PUT。
5. **非周期发现**：模型发布频率远低于 quota 变化；事件触发足够且保护管理 API 预算。
6. **每 key 发现**：不假设同一 provider 的所有 key 权限完全一致。

## 6. 正确性论证

### goal → 模块映射

- G1 新渠道不再使用过期进程内模型字符串 → `model_catalog` + AddKey。
- G2 已有渠道随上游目录收敛 → `sync_channels` 模型对账。
- G3 发现故障不破坏服务 → `Unavailable` 的 existing-keep/fallback 分支。
- G4 不再次吃满 new-api `/api` 限流 → 集合差异门控 + 非周期触发。

### 模块协作论证

catalog 在 H1/H2 下产出某 key 的非空真实模型集；newapi 对新渠道直接使用该集合，对已有渠道
仅在集合不等时原子更新并回读。故发现成功后所有受管渠道最终与各自 key 的目录一致。发现失败时
existing 分支无写入，新渠道仍有显式 fallback，因此不会因一次外部故障清空线上模型或阻塞录入。

### 模块级 invariant

- I1：任何渠道写入的 models 非空。
- I2：模型对账不改变 key、priority、status、group。
- I3：发现失败时已有渠道零写入。
- I4：每次 sync 中每个唯一发现目标最多一次外部 GET。
- I5：日志与错误中不含上游 key。

维护责任：catalog 保证 I1/I5 的发现侧；newapi 更新函数保证 I1–I4；orchestrator 仅消费结果，
不自行拼接模型字符串。

## 7. 验收

1. parser：空/重复/空白 id；错误 JSON；空 data；合法列表。
2. config：旧配置兼容；三种 auth；非法 URL 拒绝。
3. sync：相同零写、漂移单次 PUT、发现失败 existing 不动、新建走 fallback。
4. add-key：运行中修改 fallback 不影响成功发现；新渠道包含最新 `/models` 项。
5. 真实智谱：新增临时 key 后渠道包含 `glm-5.3` 与 `glm-5.3-flash`；priority/status 不变。
6. 管理 API 调用量：稳态仍为零模型同步写入。
