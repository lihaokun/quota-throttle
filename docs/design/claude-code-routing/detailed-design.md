# 细化设计 — claude-code-routing

> 上游文档：`architecture.md`（模块边界与不变量）、`../../research/claude-code-routing-research.md`（外部事实）。
> 细化阶段新增的源码级事实（v1.0.0-rc.20，补调研）：
> · `POST /api/token/:id/key` → `{data:{key}}` **返回完整 key**（`controller/token.go` GetTokenKey）——令牌打码问题不存在了，令牌 key **每次 sync 现取现用，不落 config**。
> · `POST /api/token/`（AddToken）搬运 `Group` 字段、服务端生成 key、name ≤50、
>   `unlimited_quota:true` 时不校验 remain_quota（`controller/token.go:170-228`）。
> · 令牌路由 `router/api-router.go:232-243`：`GET /api/token/`（列表，key 打码）、
>   `POST /api/token/`（建）、`POST /api/token/:id/key`（取完整 key）。均为 UserAuth。
> · `GET/PUT /api/option/`（RootAuth）读写 option 表（`router/api-router.go:186-190`），
>   group ratio 以 config 名 `group_ratio_setting` 注册（`setting/ratio_setting/group_ratio.go`）。

## 1. 范围

| 模块 | 函数 | 性质 | 对应架构 |
|---|---|---|---|
| config.rs | C1 `ClaudeChannelTemplate`（+`channel_name`） | 新增，trivial | §2.1 |
| config.rs | C2 `Config::validate` 扩展 | 修改，non-trivial | §2.1 不变量 |
| config.rs | C3 `ResolvedKey.claude_channel_id` | 修改，trivial | §2.2 |
| newapi.rs | N1 `ChannelParams` + 两个 From | 新增，trivial | §3.2 |
| newapi.rs | N2 `create_channel` 泛化签名 | 修改，trivial | §3.2 |
| newapi.rs | N3 `plan_channel_ops` 纯函数 | 新增，non-trivial | §3.2 |
| newapi.rs | N4 `sync_channels` v2 → `SyncOutcome` | 修改，non-trivial | §3.2 |
| newapi.rs | N5 `ensure_group` | 新增，non-trivial | §3.2 |
| newapi.rs | N6 `ensure_claude_token` | 新增，non-trivial | §3.2 |
| main.rs | M1 `resolve_keys` v2 | 修改，small | §3.3 |
| main.rs | M2 `cmd_sync`/`cmd_up` 铺设 + 接入块打印 | 修改，medium | §3.3 |
| main.rs | M3 `cmd_run` 的 claude map 构建 | 修改，small | §3.3 |
| orchestrator.rs | O1 `tick` 下发段双渠道写 | 修改，non-trivial | §3.4 |
| orchestrator.rs | O2 `key_index_of` 归一化 | 新增，trivial | §3.4 |
| orchestrator.rs | O3 `pin` 入口归一化 | 修改，trivial | §3.4 |
| orchestrator.rs | O4 `remove_key` 双渠道降档 | 修改，non-trivial | §3.4/D7 |
| orchestrator.rs | O5 `add_key` 双渠道创建 | 修改，non-trivial | §3.4 |
| orchestrator.rs | O6 快照新字段 | 修改，trivial | §2.3/2.4 |
| status.rs | S1/S2 结构体与 `tracked_channels` | 修改，trivial | §2.3/2.4 |
| status.rs | S3 HTML 五处 | 修改，medium | §3.5 |

不实现（明确排除）：model_mapping、令牌 key 持久化到 config、客户端配置托管、SQLite 依赖。

## 2. 与已有代码的复用点

- 建渠道 payload 形状 `{mode:"single", channel:{...}}` 原样复用（type 14 无额外必填字段，
  `model/channel.go` 无 models 正则校验）——N2 只是参数化。
- 改 priority 的「GET→改字段→PUT 剔 status」路径原样复用（O1/O4 只是多调一次）。
- `apply_headers` / 成功判定（`success==true` 或 HTTP ok）/ `extract_items` 兼容层全部复用。
- `list_channels` 的 name→id 兼容解析复用（N4/N6 都靠它）。
- 看板命令通道 `Command` / `dispatch` / CSRF / oneshot 回执完全不动（Pin/RemoveKey 语义扩展
  在 orchestrator 入口归一化完成，HTTP 层零感知）。
- toml_edit 机制（append_key/remove_key）不动——claude 渠道 id 不落 config，每次按名解析。
- 测试风格复用：纯函数中文单测（decide/plan_channel_ops 同款）。

## 3. 错误处理策略

**setup 层（sync/up，D6：不受 dry_run 门控）**：
- openai 渠道创建失败 → **硬错**终止 sync（与现状一致：基础设施不齐不该进循环）。
- claude 渠道创建失败 → **warn 降级**：该 key 的 claude 侧无人受管，openai 侧照常入池。
  （claude 是叠加层，它坏了不该连累现有链路。）
- `ensure_group` 失败 → **warn 不阻断**。理由：group ratio 只影响计费倍率，不影响路由隔离
  （隔离由 token group + abilities 保证，I4 不依赖它）；无证据表明缺注册会断转发，硬错反而
  可能因版本差异卡死整个 sync。失败信息里提示验收项（实测 H4）。
- `ensure_claude_token` 失败 → **warn + 打印人工指引**（new-api UI 建令牌绑 claude group）。
  CC 接入块仍打印，只是 AUTH_TOKEN 一栏写指引而非 key。

**运行期（tick）**：
- 双渠道 priority 下发**部分失败**：成功侧记 `applied`，失败侧下轮重试（幂等）；看板 cc 对账
  字段会把不一致晒出来（S3）。绝不让 cc 失败影响主渠道下发顺序与语义。

**dry_run 语义**：priority（含 O4 的降档 PUT）受门控；渠道/令牌/group 铺设不受门控（D6）。

## 4. 数据结构定义

### 4.1 `ClaudeChannelTemplate`（config.rs，跨模块共享：config→newapi/main/orchestrator）

```rust
pub struct ClaudeChannelTemplate {
    pub channel_type: i64,   // serde "type"，默认 14
    pub base_url: String,    // 默认 "https://open.bigmodel.cn/api/anthropic"
    pub models: String,      // 默认含 [1m] 变体的全列表（见 config.example.toml）
    pub group: String,       // 默认 "claude"
    pub name_suffix: String, // 默认 "-cc"
    pub token_name: String,  // 默认 "claude-code"
}
impl ClaudeChannelTemplate {
    pub fn channel_name(&self, key_name: &str) -> String;  // format!("{key_name}{}", self.name_suffix)
}
```

类型不变量（C2 校验）：`name_suffix` trim 后非空；若 `channel_template` 也配了，则
`claude.group ≠ openai.group`。生命周期：配置块存在即开启（D4）。

### 4.2 `SyncOutcome`（newapi.rs → main.rs）

```rust
pub struct SyncOutcome {
    pub primary: HashMap<String /*key 名*/, i64>,
    pub claude:  HashMap<String /*key 名*/, i64>,  // 值 = <name>-cc 渠道 id
}
```
**按 key 名索引而非渠道名**——resolve_keys 不需要再知道 suffix（降低耦合）。

### 4.3 `ChannelParams`（newapi.rs 私有）

```rust
struct ChannelParams<'a> { channel_type: i64, base_url: &'a str, models: &'a str, group: &'a str }
impl From<&ChannelTemplate> / From<&ClaudeChannelTemplate> for ChannelParams
```

### 4.4 `ChannelOp`（newapi.rs 私有，plan 的输出）

```rust
enum Kind { OpenAi, Claude }
enum ChannelOp<'a> {
    Skip   { name: String, kind: Kind },
    Create { name: String, kind: Kind, params: ChannelParams<'a>, key: &'a str },
    Missing{ name: String },  // openai 侧无模板且渠道不存在（现有 warn 语义）
}
```

### 4.5 既有结构扩展（字段追加，serde/JSON 向后兼容）

- `ResolvedKey` + `claude_channel_id: Option<i64>`
- `KeyStatus` + `claude_channel_id: Option<i64>`
- `StatusSnapshot` + `claude_endpoint: String`（claude 模板未配时为空串，前端隐藏 chip）
- `AddKeyOk` + `claude_channel_id: Option<i64>`

## 5. 模块细化

### 5.1 config.rs

#### C1 `ClaudeChannelTemplate`（trivial：纯数据 + 一行 format，无分支无副作用）

#### C2 `Config::validate` 扩展
- 功能：启动时拦截 §4.1 两条不变量。
- 调用关系：`Config::load` → validate（现有）。
- 实现思路：现有阈值校验后追加：
  1. 取 `self.new_api.channel_template`（Option）与 `channel_template_claude`（Option）。
  2. 若 claude 存在且 `name_suffix.trim().is_empty()` → bail（提示：cc 渠道名会与主渠道同名，
     sync 按名匹配会互相误判）。
  3. 若两者都存在且 `openai.group == claude.group` → bail（提示：分发器不按格式过滤，同 group
     两种格式渠道会互抢流量做跨格式转换）。
  4. 通过 → Ok。
- 分支覆盖：4 条路径（无 claude / 有 claude 无 openai 模板 / suffix 空 / group 冲突）全列。
- 正确性论证：
  - 前置：Config 已反序列化成功（load 保证）。
  - 论证：bail 条件恰为两条不变量的否定；`load` 在 validate Err 时返回 Err ⇒ 违反不变量的
    配置不可能进入运行期；不 bail 时两不变量成立。
  - 后置：Ok ⇒ 不变量成立；Err ⇒ 进程带原因退出，零副作用。
  - 副作用论证：纯校验，无写。

#### C3 `ResolvedKey` 扩展（trivial：加字段；所有构造点在 M1/O5 同步补 None/Some）

### 5.2 newapi.rs

#### N1 `ChannelParams` + From（trivial：字段搬运）

#### N2 `create_channel` 泛化
- 功能：按 `ChannelParams` 建渠道（payload 与现有逐字段相同，仅来源参数化）。
- 调用关系：N4/O5 → create_channel；callee：POST `{channel_path}`（契约同现状：
  `{mode:"single",channel:{...}}` 包裹、success 判定）。
- 实现思路：签名改 `create_channel(&self, name: &str, key: &str, priority: i64, p: &ChannelParams)`；
  payload json! 中 `type/base_url/models/group` 改取 `p.`，其余不动。现有调用点（O5 的 openai
  分支）改传 `(&tpl).into()`。
- 正确性论证：payload 形状不变（对照 json! 逐字段），仅取值来源变化 ⇒ 对 new-api 的请求体
  与改造前逐字节等价（同模板下）。trivial 级别但因为是「现有唯一建渠道入口」，改动须保守。

#### N3 `plan_channel_ops`（纯函数）
- 功能：给定 keys、现有渠道名集合、两模板，产出渠道操作计划（不执行）。
- 调用关系：N4 调用；零 IO。
- 实现思路（推导连续）：
  1. `existing: &HashSet<String>`（渠道名集合）。
  2. 外层 `for k in keys`（有限，终止性显然）：
     a. openai 侧：`k.name` ∈ existing → `Skip{OpenAi}`；∉ 且模板 Some → `Create{OpenAi,
        params=tpl.into(), key=&k.zhipu_api_key}`；∉ 且模板 None → `Missing`（沿用现有 warn 语义）。
     b. claude 侧：模板 None → **不产出任何 op**（功能关）；Some → 名 `tpl.channel_name(&k.name)`
        ∈ existing → `Skip{Claude}`；∉ → `Create{Claude, key=&k.zhipu_api_key}`。
  3. 收集返回 `Vec<ChannelOp>`。
- 分支覆盖：a 的三分支 + b 的二分支全列如上；两个 key 同名不在防御范围（现有
  sync 同样不防，config 层面 name 即主键，append_key 已挡重复名）。
- 显式假设链：existing 与 new-api 实际渠道名一致（N4 调 list_channels 刚拉取）；两模板
  group 不同（C2 已保证）；`k.name + suffix` 不会恰好等于**另一把 key 的主渠道名**——
  ⚠️ 这个未防！补充：C2 追加第三条校验：任一 `keys[i].name + suffix ≠ keys[j].name`（i,j 任意）。
  （否则 zhipu-1-cc 会与名为 zhipu-1-cc 的主渠道撞名，plan 误判已存在。）C2 论证同步补一条。
- 正确性论证：
  - 前置：C2 三条不变量成立。
  - 论证：plan 只依据 existing 集合做属于判定 ⇒ 每个名字至多产出一个 op；Create 的名字
    全部 ∉ existing ⇒ 执行期不会重名创建；Skip/Missing 与现有语义逐条对应。
  - 后置：ops 覆盖每把 key 的两侧（openai 恒有 op，claude 视模板）。
  - 副作用：无（纯函数）。
- 单测：① 全新建（双渠道两个 Create）② 全已存在（两个 Skip）③ 无 claude 模板（仅 openai op）
  ④ openai 无模板 + 渠道缺（Missing）⑤ 混合（key1 Skip / key2 Create×2）⑥ suffix 撞主渠道名 → C2 拒绝。

#### N4 `sync_channels` v2
- 功能：按计划执行创建，返回 `SyncOutcome`。
- 调用关系：M2 → sync_channels → list_channels/N2；O5 不走它（单 key 场景直接建）。
- 实现思路：
  1. `let existing = self.list_channels().await?`（callee 契约：Ok=完整 name→id，Err 上抛）。
  2. `let names: HashSet = existing.keys().cloned().collect()`；`plan = plan_channel_ops(...)`。
  3. `for op in &plan`：Create → `create_channel`：
     - `Kind::OpenAi` 失败 → **bail**（错误信息带渠道名）；
     - `Kind::Claude` 失败 → `warn!` 继续；
     Skip/Missing → Missing 打 warn（现状文案），Skip 打 info（现状文案）。
     （循环终止：plan 有限；无重试。）
  4. 若执行过 Create → 重新 `list_channels().await?` 得最新映射；否则用第 1 步的。
  5. 组装 `SyncOutcome`：对每把 key，`primary[k.name] = latest[k.name]`（**必然存在**：要么
     本来就在 existing，要么刚建完；若刚建的解析不到 → warn 并缺项——现有代码同款容错）；
     claude 侧同理但允许缺项（创建失败的 key 无 cc id）。
  6. 返回 Ok(outcome)。
- 退出覆盖：Ok（全成/部分 claude 失败）；Err（list 失败 / openai Create 失败）。
- 正确性论证：
  - 前置：已鉴权；C2 不变量成立。
  - 论证：幂等性——重复执行时第 1 步 existing 已含目标名 ⇒ 全 Skip ⇒ 不发任何写请求；
    I2 的「claude group 只含 -cc 渠道」由 Create 只携带模板 group（C2 保证 ≠ default 组）
    保持；openai 硬错语义与现状一致（对照现有代码逐行）。
  - 后置：Outcome 中每个 primary 条目对应一个真实存在的渠道 id。
  - 副作用：new-api 侧新增渠道（且仅 plan 中 Create 的那些）；无本地状态写。
- dry_run：不门控（D6）。

#### N5 `ensure_group(group: &str)`
- 功能：把 group 注册进 group ratio 配置（ratio=1），幂等。
- 调用关系：M2 → ensure_group；callee：GET/PUT `/api/option/`（RootAuth，apply_headers 已覆盖）。
- 实现思路：
  1. GET `{base}/api/option/` → `data: [{key,value}...]`（`extract_items` 或 data 数组直取）。
  2. 在列表中找 `key == "group_ratio_setting"`；找不到 → 找 legacy `"GroupRatio"`；都没有 →
     `bail!("该 new-api 版本的 option 列表里没有 group 配置键…")`（M2 侧 warn 放行）。
  3. 解析 value（JSON 字符串）：
     - `group_ratio_setting` 形如 `{"group_ratio":{...},"group_group_ratio":{...},...}`
       → 目标子对象 = `group_ratio`；
     - legacy `GroupRatio` 直接是 map → 目标子对象 = 根。
  4. 目标子对象已含 `group` → Ok（幂等早退）。
  5. 插入 `group: 1.0` → 序列化回原形状 → PUT `{"key":<原名>,"value":<新 JSON 串>}`。
  6. 回读一次（再 GET）断言 group 在 → Ok；不在 → bail（PUT 未生效）。
- 分支覆盖：无键/两形态/已含/未含/回读失败 全列如上。
- 错误：所有失败 bail；**M2 对 ensure_group 的 Err 一律 warn 不阻断**（§3）。
- 正确性论证：
  - 前置：已鉴权管理员；option 列表可读。
  - 论证：只做「读-判-补一个键-写回」，不改既有键值（步骤 5 只 insert 目标 group，JSON
    其余部分原样搬运）⇒ 对现有 default/vip/svip 倍率零扰动；幂等由步骤 4 早退保证，
    重复调用零写请求。
  - 后置：Ok ⇒ 该版本配置里 group 已注册 ratio 1；Err ⇒ 调用方 warn 且路由隔离不受影响（I4
    由 token group 保证，与本函数无关）。
  - 副作用：option 表一行（或零行，幂等早退时）。

#### N6 `ensure_claude_token(name, group) -> Result<String>`
- 功能：按名找到（或建出）绑定 group 的令牌，返回**完整 key**。
- 调用关系：M2 → ensure_claude_token；callee：GET `/api/token/?p=0&page_size=100`（UserAuth，
  打码无妨，只用 id/name/group/status）、POST `/api/token/`、POST `/api/token/:id/key`。
- 实现思路：
  1. list tokens → items 找 `name` 相符：
     - 找到多条同名 → bail（提示人工清理；同名令牌语义不明）。
     - 找到一条：`group` 字段 ≠ 目标 group → bail（提示：同名令牌属别的 group，请改名或人工处理）；
       `status != 1`（禁用）→ bail 提示启用；否则记 `id`。
     - 没找到 → POST `/api/token/`，payload `{name, expired_time:-1, unlimited_quota:true,
       remain_quota:0, group, model_limits_enabled:false}`（AddToken 契约见文首）→ 再 list
       一次取 `id`（AddToken 响应不含 id）；仍找不到 → bail。
  2. POST `/api/token/{id}/key` → `data.key`（即完整 key，GetTokenKey）→ Ok(key)。
  3. ⚠️ 不缓存：每次 sync 现取（key 不会变，但省掉一切持久化状态）。
- 分支覆盖：多条/一条不符/一条禁用/一条符合/无→建→找到/建后仍找不到/key 取失败 全列。
- 正确性论证：
  - 前置：已鉴权；令牌数未达上限（AddToken 校验，超限会失败并 bail 如实上报）。
  - 论证：返回的 key 必来自 GetTokenKey 对**刚按名确认过的 id** 的响应 ⇒ key 与 name/group
    绑定关系由 new-api 权威保证；创建路径的 group 经 AddToken 的 `Group: token.Group` 搬运
    （文首源码事实）进入持久层。
  - 后置：Ok(key) ⇒ 该 key 即可用于 CC 的 `ANTHROPIC_AUTH_TOKEN`；Err ⇒ 调用方 warn + 指引。
  - 副作用：new-api 侧至多新增一个令牌；本地零状态。

### 5.3 main.rs

#### M1 `resolve_keys(cfg, primary: &HashMap<String,i64>, claude: &HashMap<String,i64>)`
- 实现思路：现有循环体每个 key 追加 `claude_channel_id: claude.get(&k.name).copied()`；
  claude map 无此名（未建/失败/未配模板）→ None（**静默**：sync 阶段已 warn 过，此处不重复刷屏）。
- 正确性论证：trivial（查表填字段）；None 语义 = 该 key 无 claude 侧（I4：显式失败而非错路由）。

#### M2 `cmd_sync` / `cmd_up` 扩展
- 实现思路（两函数同构，抽 `setup_claude(api, cfg) -> ()` helper）：
  1. `claude_tpl = cfg.new_api.channel_template_claude.as_ref()`；None → 直接返回（D4）。
  2. `ensure_group(&tpl.group)` → Err 则 warn（文案含「不影响路由隔离，仅计费倍率」）。
  3. `ensure_claude_token(&tpl.token_name, &tpl.group)` → Ok(key) 或 Err→warn。
  4. 打印接入块（info! 多行）：
     - `ANTHROPIC_BASE_URL = {cfg.new_api.base_url}`（无尾斜杠）
     - `ANTHROPIC_AUTH_TOKEN = <key 或「⚠️ 令牌获取失败：…请到 new-api UI 手动建/复制」>`
     - 「其余 env（ANTHROPIC_DEFAULT_*_MODEL 等）与你现在的直连配置完全一致，不用改」
- 正确性论证：setup 失败均降级（§3），cmd_up 后续 run_loop 不受影响（ResolvedKey 的
  claude id 来自 N4 结果，与令牌是否就绪无关——最坏情况 CC 侧 401，openencode 侧无感）。

#### M3 `cmd_run` 的 claude map
- 实现思路：`list_channels()` 已返回全部渠道名→id；配了 claude 模板时对每把 key 用
  `tpl.channel_name(&k.name)` 查 map 组装 claude map；未配 → 空 map。
- 正确性论证：trivial 查表；渠道不存在 → None（warn 一次提示「先跑 sync」）。

#### M4 `print_mapping` 扩展
- 每把 key 一行变两列：`zhipu-1 → #3 (cc: #7)`；cc 缺 → `(cc: 未建)`。

### 5.4 orchestrator.rs

#### O1 `tick` 下发段双渠道写
- 功能：对每把 key 把同一 target priority 写到主渠道与 cc 渠道。
- 调用关系：tick 内循环 → set_channel_priority（callee 契约不变）。
- 实现思路（在现有 for k in &keys 循环内改）：
  1. target 计算逻辑**逐字不动**（active/eligible/pct 三分支 + 查询失败 continue）。
  2. 查询失败 `continue` 的语义升级说明：跳过的是**整把 key**（两个渠道都不动——状态未知
     不动它，与现状对单渠道的语义一致）。
  3. 对 `let chans = [(k.channel_id, false), (k.claude_channel_id, true)]`中 Some 的项：
     - `applied.get(&cid) == Some(&target)` → continue（幂等，双渠道各自记账）；
     - dry_run → `info!(name, channel_id=cid, cc=<bool>, priority, "dry_run: 将设 priority")`
       + `applied.insert(cid, target)`；
     - 否则 PUT → 成功 insert + info；失败 `error!`（不中断，另一渠道照发）。
  4. 循环终止：keys 有限 × ≤2 渠道。
- 分支覆盖：target 三分支（不动）× 每渠道三分支（跳过/dry/真发）全列；cc None → 只主渠道。
- 正确性论证：
  - 前置：decide() 不变（身份=主 id）；I3 成立。
  - 论证：两渠道的 target 来自同一次计算 ⇒ 同值（I1 的写入端）；applied 按渠道 id 独立记账
    ⇒ 单侧失败下轮仅重试失败侧（幂等），最终 I1 成立（看板对账可见中间态，S3）；
    cc 渠道写失败不改变主渠道执行顺序 ⇒ 现有行为（对 openai 侧）与改造前逐字节一致。
  - 后置：tick 结束时，本轮成功下发的渠道 applied == 目标分层。
  - 副作用：new-api 渠道 priority PUT（受 dry_run 门控）。
- 回归保证：decide() 现有 17 个单测零改动即回归通过。

#### O2 `key_index_of(channel_id) -> Option<usize>`（trivial：`keys.iter().position(|k|
k.channel_id == id || k.claude_channel_id == Some(id))`）

#### O3 `pin` 入口归一化（trivial：入口处 `let Some(idx) = self.key_index_of(id)` else Err；
后续判据全部用 `self.keys[idx].channel_id`（主 id）——last_eligible/last_pct 的键就是主 id，
I3 保持）

#### O4 `remove_key` 双渠道降档
- 实现思路：
  1. `key_index_of(id)` 归一化 → 找不到 → Err（现有文案）。
  2. 最后一把 key 防呆（现有）。
  3. **先全部降档再动 config**：对 `[primary, cc?]` 逐个 `set_channel_priority(cid,
     priority_exhausted)`；`!dry_run` 门控照旧（dry_run 跳过 PUT）；任一失败 → Err「未做任何
     改动」（此时可能已压了部分渠道——方向安全：少接流量，而非多接）。
  4. config remove（现有）→ `keys.retain`（按 key 索引摘除）→ `applied.remove` 两 id →
     active/pinned 匹配主 id 则清（现有逻辑，键改主 id）。
- 分支覆盖：无 cc（现状路径）/有 cc 全成/部分失败/全失败/dry_run 五路全列。
- 正确性论证：
  - 论证（D7）：若只压主渠道，cc 渠道以 priority=100 孤儿般继续吃光 CC 流量而用量无人盯——
    双压后两 group 内该 key 都跌到最低档，429 兜底优先选还有余量的 standby（与单渠道时代
    同构）；「先降档后摘除」的顺序保证失败时**不会**出现「config 已删但渠道还挂着高优先级」
    的最坏中间态（对照现有 remove_key 的顺序论证，只是把单 PUT 变多 PUT）。
  - 副作用：≤2 个 priority PUT（dry_run 门控）+ config.toml 删一条 + 进程内状态摘除。

#### O5 `add_key` 双渠道创建
- 实现思路（在现有 ① 探活 ② 建渠道 ③ 写 config ④ 热加载骨架上扩 ②'）：
  1. 前置校验/探活/命名检查**不动**（含 append_key 的同名拒绝）。
  2. openai 建渠道（N2）→ 失败 Err「未做任何改动」（现状）。
  3. claude 模板 Some → 建 `<name>-cc`（N2）→ 失败仅 `warn!`（§3 降级语义）。
  4. `list_channels()` 一次 → 主 id 解析不到 → Err（现状文案）；cc 解析不到 → warn + None。
  5. `append_key` 写 config（不动；[[keys]] 无渠道 id 字段）。
  6. `keys.push(ResolvedKey{ claude_channel_id, .. })`；AddKeyOk 带 `claude_channel_id`。
- 正确性论证：探活挡在录入口的血泪不变量不动；claude 失败不阻塞入池（该 key 用量调度照常，
  仅 CC 侧无此 key——I4 显式失败）；对照现有 add_key 逐步等价（openai 路径逐行相同）。

#### O6 快照字段（trivial）：KeyStatus 填 `claude_channel_id`；
`claude_endpoint = claude 模板 Some 时 base_url 去尾斜杠，否则 ""`。

### 5.5 status.rs

#### S1/S2（trivial）：结构体字段见 §4.5；`tracked_channels` 改为
`keys.iter().flat_map(|k| [Some(k.channel_id), k.claude_channel_id]).flatten().collect()`。

#### S3 HTML 五处（列出精确改动点，均为模板字符串内改）：
1. **chips 双 endpoint**：现有「客户端连接」chip 标签改 `opencode`；`d.claude_endpoint`
   非空时并列第二个 chip，标签 `Claude Code`，同样可复制。
2. **key 卡片认领 cc 渠道**：`.chead` 内主 `渠道 #id` 后追加
   `k.claude_channel_id!=null` 时 `<span class="cid">CC #id</span>`；对应 `chOf(cc_id)`
   的 enabled 徽标（复用 `badge b-on/b-off` 文案「CC 渠道被禁用」）。
3. **meta 行对账**：cc 存在时追加 `CC P={cc.priority}`；与主 `k.priority` 不等 →
   `<span class="warn">（联动不一致！）</span>`（I1 的可视化，模板沿用现有 mism 写法）。
4. **live 行**：`lvOf(k.claude_channel_id)` 存在且 rpm>0 → 追加分段 `CC: N req/min`
   （pulse 判定改为「主或 cc 任一 rpm>0」）。
5. **野生渠道过滤**：`mine` 集合加入 `k.claude_channel_id`；**addmsg** 成功文案
   `渠道 #${j.channel_id}${j.claude_channel_id!=null?` · CC #${j.claude_channel_id}`:''}`。
- pin/unpin/删除按钮继续用主 id（O2/O3 已归一化，双 id 也能用，但 UI 统一发主 id）。
- 正确性论证：全部为展示层改动，读写均经现有快照字段；不新增 HTTP 端点/命令类型。

## 6. 完整性自检 checklist

- [x] 所有函数实现思路推导连续（C2/N3/N4/N5/N6/O1/O4/O5 逐步显式；trivial 项标注理由）
- [x] 所有分支覆盖（各 non-trivial 函数已列全部分支；target 三分支明确「不动」并说明为何不动）
- [x] 所有退出点刻画（N4 双 Err 路径、N5/N6 bail 条件、O4 部分失败方向安全性均已写）
- [x] callee 契约引用（create_channel/option/token/priority 各端点的请求-判定契约均标注文首源码事实）
- [x] 循环终止性（N3/N4/O1：有限集合 × 常数因子；无重试循环）
- [x] 显式假设链（C2 新增第三条不变量补 suffix 撞名漏洞；N3 假设链逐条列；O1 的「查询失败=整 key 跳过」语义显式化）

## 7. 测试计划

**单元（cargo test，纯函数）**：
- config：C2 三条拒绝路径 + 未配块 None + channel_name；append_key 回归（现有 6 个不动）。
- newapi：N3 plan_channel_ops 六个用例（§5.2 N3 列）。
- orchestrator：decide/peak 现有 22 个测试**零改动必须全过**（回归红线）。

**端到端（实测，任务 #4）**：architecture.md §7 五条验收 + H3（令牌 group 路由域）、
H4（group_ratio 注册形态按 N5 两候选实测）、H6（count_tokens 404 影响）逐项落。
