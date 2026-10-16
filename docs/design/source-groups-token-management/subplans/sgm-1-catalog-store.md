# 来源目录与状态库细化设计

子计划：sgm-1-catalog-store。日期：2026-10-14。状态：实现及双路径验收完成，见[审核报告](../../../audits/sgm-1-catalog-store/audit-report.md)。尚未接入运行服务。

版本范围：本文保留 SGM-1 的 v1 设计与当时验收约束，不表示当前 `CatalogStore::open` 仍只支持 v1。SGM-2 已按其 §4.2/§5.4 和 v3 持久日志补充，引入合法旧库校验、私有备份、逐版本事务升级，以及打开后的执行恢复；source/member 插入也在原事务中追加控制/执行侧表。SGM-3 B 随后追加 v4 来源/组策略状态，数据库当前为 v4，公开快照仍为 wire=1。具体承接和受影响路径见[迁移增量核对](../../../audits/sgm-1-catalog-store/migration-scope-review.md)。

下文 P3/P4 的“不迁移高版本、不修复性补损坏表”仍成立；合法低版本的升级已由 SGM-2 明确扩展，不能把旧阶段叙述当作当前 open 无持久副作用的承诺。生产导入和部署授权边界不变。

## 1 范围

实现共享 Source、Group、Membership 的本地目录及 SQLite 持久化；支持新增来源/组、建立计划成员、幂等导入旧 GLM 配置映射、安全快照和一致性备份。对应[架构](../architecture.md) G1、G6，并为 G2 保留旧渠道 ID、成员顺序和探针参数。

本步不连接 NewAPI 或真实供应商，不修改渠道，不接管调度，不添加三页面，不实现火山运行状态机。来源启停/删除、成员绑定渠道/退役、revision 更新和操作队列在 sgm-2 统一实现，不能先暴露不具备外部回读的管理入口。迁移 CLI 和生产切换在 sgm-7；本步只实现其纯转换与事务落库函数。

新增 src/catalog/{mod.rs,types.rs,store.rs,legacy.rs,schema_v1.sql} 和模块测试；main.rs 仅注册模块。config.rs 仅为 HeaderKV、ModelDiscoveryConfig、ModelDiscoveryAuth 增加必要的 Serialize derive，不改变解析或默认值。Cargo 文件只增加下述依赖。orchestrator.rs、quota.rs、newapi.rs、boot.rs、status.rs 和现有测试期望均不改。

验收清单：[expectations](../../../audits/sgm-1-catalog-store/expectations.md)。JSON 输出契约：[catalog snapshot schema](../schemas/catalog-snapshot.schema.json)。SQL schema 已按本设计实现，后续变更须递增数据库版本。

## 2 与已有代码的复用点

| 复用项 | 本步用法与约束 |
| --- | --- |
| Config / KeyMapping / ChannelTemplate | 输入已经加载的内存配置；本模块不调用 Config::load，不读取默认 config.toml |
| ModelDiscoveryAuth / ModelDiscoveryConfig / HeaderKV | 共用原类型，仅补 Serialize；不重定义 bearer、authorization_raw、x_api_key 枚举 |
| normalize_models_csv | 对旧渠道实际 models 做 trim、去空、首次出现去重；空结果拒绝导入，不用旧模板盖掉实际目录 |
| reqwest::Url 与 header 类型 | 仅解析验证 URL/header，无 HTTP；错误不能打印待验证值 |
| serde / serde_json / sha2 | 已有依赖；私有设置序列化、凭据敏感指纹和导入摘要，不自制加密 |
| rusqlite =0.40.2 | 新运行依赖，default-features=false，features=[bundled,backup]；版本本机已有缓存，后续锁入 Cargo.lock |
| proptest =1.11.0 / tempfile =3.27.0 | 新开发依赖，用于属性测试及独立临时目录；不进入运行逻辑 |
| std::fs::File::try_lock | 目录内固定锁文件的非阻塞排他锁；本机 Rust 1.95 支持，所需最低版本 1.89 |

外部契约：rusqlite 事务提供提交/回滚、busy_timeout 可显式限定等待；使用 backup 功能保证含 WAL 的一致性备份。[Connection 文档](https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html)

进程锁由成功持锁的内部 CatalogLock 持有，不依靠可能遗留的 PID 文件；try_lock 失败立即返回，不无限等待。连接关闭后显式unlock再关闭File，避免无关继承/重复描述符拖延重开。[File 文档](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)

本仓库未包含 workflow 所述 templates/contract-audit 目录；按 workflow §6.3 原有十节结构创建 expectations，不为此安装或替换项目工作流。

## 3 错误处理策略

内部采用 Result<T, CatalogError>，调用者只能根据稳定 kind 和无敏感值的上下文处理。不把 rusqlite/serde/URL 原始错误字符串连同用户输入直接透传，不记录 SQL 参数。

| kind | 条件 | 已有状态 |
| --- | --- | --- |
| invalid_input | 值域/配置/URL/header 错误，空导入，ID 或整数溢出 | 不变 |
| duplicate_source / duplicate_group / duplicate_membership / channel_conflict | 唯一性冲突，包括已退休的渠道身份 | 当前事务回滚 |
| not_found / type_mismatch / retired | 引用缺失、来源组类型不同、引用已归档 | 不变 |
| import_conflict / import_requires_empty | 导入摘要不同，或首次导入前已有目录实体 | 不变 |
| foreign_database / unsupported_schema / unsafe_path / already_open | 非本应用数据库、高版本、路径或权限不安全、锁被占用 | 不更改已有库或修复其权限 |
| unsupported_platform | 当前平台无法执行本步 Unix 私有权限契约 | 不创建状态库；不改变原有 CLI |
| busy | SQLite 锁等待超过 1 秒 | 回滚当前事务；调用方决定稍后重试 |
| storage / corrupt_data | IO/SQL 失败、持久数据解码或跨字段约束损坏 | 不把部分结果当成功；必要时弃用连接 |
| commit_unknown | 提交时报 IO 错误，无法证明是否提交 | 将句柄标为不可继续使用；重开后按唯一键/导入标记对账，不盲目再插入 |
| backup_failed / backup_exists | 备份失败或目标已存在 | 源库不变，目标不视为成功备份 |

错误格式化只包含 kind、固定 catalog 错误域、可安全公开的实体 ID，以及 incomplete_backup 布尔标志；不含凭据、指纹、header 值、URL 查询参数、配置内容或私有设置 JSON。incomplete_backup=true 表示失败目标可能仍残留或无法确认身份，调用方必须提示检查，不能当作成功备份。未知枚举/字段拒绝解码，不回退成 glm/default/零值。

## 4 数据结构定义

共享实体见架构“第一子计划共享类型补充”。本节固定值域及模块私有存储类型。所有外部传入字段先完整验证再写事务；结构字段使用 deny_unknown_fields。

### 4.1 共享类型值域

- SourceKind：glm / volcano；MembershipLifecycle：planned / imported / created_disabled / ready / retired，唯一 Rust 定义放 types.rs。sgm-1 不对后面三个状态提供写入方法。
- SourceId、GroupId、MembershipId、SecretRef：内部值 1..i64::MAX，各自独立 newtype。公开 ID 为十进制字符串；解析时拒绝前导零、负值和溢出。SecretRef 仅内部使用。
- name：trim 后 1..128 个 Unicode 字符；note：0..2048 个字符，保留内容；拒绝 NUL。newapi_group：1..64 位 ASCII 字母、数字、下划线、短横线或点，禁止 auto；不能包含逗号/空白，大小写保持原值。
- revision 初始为 1；schema_version 为 1。公开 revision、order、priority、token 上限限制在 JSON 安全整数范围内；越界拒绝，不截断。order 从 0 起，组内递增；sgm-1 不重新排序存量。
- VolcanoPolicy：daily_token_limit=None 或 1..9007199254740991；failure_threshold=1..100，默认 3；reset_minute=0..1439，默认 60；utc_offset_minutes=-720..840，默认 480。时区为固定 UTC 偏移，不承诺夏令时规则。
- channel_id：可空正 i64，公开时为字符串；imported/created_disabled/ready 必须非空，planned 必须为空。retired 可保留原 channel_id 或为空（取消未建渠道的计划）。
- observed_status 可空，非空为 NewAPI 1/2/3；observed_priority、applied_revision 可空。sgm-1 全部创建/导入记录这三项为空，不冒充已回读。
- retired_at 可空，非空为非负 UTC epoch 毫秒；成员 retired 当且仅当 retired_at 非空。来源退休标记仅为后续生命周期保留，sgm-1 新建/导入均为空。

### 4.2 来源配置与凭据

SourceDraft = {name, note, settings: SourceSettings, credential: SecretString}。SourceSettings 是以 type 区分的 glm/volcano 结构，不能给 GLM 配火山 token 判据，也不能给火山配 GLM quota selector。

UpstreamProfile = {channel_type: 正 i64, base_url: String, models: Vec<String>, model_discovery: Option<ModelDiscoveryConfig>}。models 规范化后必须非空，模型名不允许逗号。新目录的 upstream 与 discovery URL 必须为带主机的 http/https，拒绝 userinfo、fragment、首尾空白；不把 URL 规范化后静默改写原路径。base_url/discovery 禁止 query 中携带凭据，新来源使用独立 credential 字段；本步统一拒绝它们的 query，GLM quota_url 保留现有 type=2 等 query。

GlmQuotaProfile = {quota_url, global_headers: Vec<HeaderKV>, key_headers: Vec<HeaderKV>}。header 名和值用原 reqwest 类型校验，顺序和两层结构保持不变。尤其不将现有“先全局 header，再 per-key header”的构造改成新的字典合并算法。quota_url 禁止 userinfo/fragment，允许 query。

SecretString 包装非空、无首尾空白及 CR/LF/NUL 的上游凭据。不实现 Serialize，Debug/Display 固定为 [REDACTED]。SourceDraft、内部 SourceRecord、LegacyImportPlan 的 Debug 同样不能递归输出敏感配置。凭据明文仅由 store 内部入库/读取函数访问，不提供通用明文快照。

私有 SecretRecord = {secret_id, raw_key}；与 SourceRecord 同事务插入。SecretRef 指向这条记录。SQLite 和备份包含上游明文凭据，安全边界是本机私有目录/文件权限，不宣称磁盘加密；下游访问 key 不在此库保存。

credential_fingerprint = SHA256(固定域前缀 quota-throttle/source-key/v1 加 NUL 加原始 credential 字节)。同一凭据不能通过改名、换组或换 endpoint 再建来源；冲突返回既有 SourceId，要求复用。指纹不公开、不记日志。不同供应商偶然使用完全相同密钥字符串会被保守拒绝，首版不提供强制重复开关。不能据此识别供应商账户下不同 key 共享额度的情况。

### 4.3 私有 SQL schema

单库绑定一个 NewAPI 部署；更换部署必须走后续迁移流程，不把原 channel_id 认成新实例的渠道。

| 表 | 字段 | 关键约束 |
| --- | --- | --- |
| catalog_meta | singleton=1、schema_version=1 | 单行；与 PRAGMA user_version 一致 |
| source_secrets | secret_id、raw_key | 正整数 AUTOINCREMENT 主键；raw_key 非空 |
| sources | source_id、kind、name、note、secret_id、credential_fingerprint、settings_json、config_revision、manual_paused、retired_at | AUTOINCREMENT；secret FK 且 UNIQUE；指纹 UNIQUE 含退休行；kind CHECK；revision>0；布尔 CHECK |
| source_groups | group_id、newapi_group、name、kind、revision、ready | AUTOINCREMENT；newapi_group UNIQUE；kind CHECK；布尔 CHECK |
| memberships | membership_id、source_id、group_id、kind、channel_id、position、lifecycle、desired_priority、observed_status、observed_priority、applied_revision、retired_at | AUTOINCREMENT；channel_id UNIQUE 含退休行；有效 source/group 组合及组内 position 的部分唯一索引；生命周期 CHECK |
| legacy_import | singleton=1、digest、receipt_json | 标记与所有导入行同事务提交；无更新接口 |

memberships.kind 是数据库内部校验字段，不在 MembershipView 重复公开；通过 (source_id,kind) 和 (group_id,kind) 复合外键约束同类型，两张父表建立对应 UNIQUE。所有外键使用 RESTRICT，不级联删除来源或历史。sgm-1 不提供硬删除入口。

settings_json 使用 {version:1, settings:SourceSettings}，由已有 serde_json 编解码且经过 4.1/4.2 校验。数据库读取不是信任边界豁免，非法持久值返回 corrupt_data。所有 INSERT/查询参数绑定，不拼接用户文本。

本步不预建事实、火山运行周期、操作队列或 token 表；它们在对应子计划以递增 schema 迁移加入。避免先制造空表或假零用量。

### 4.4 导入与公开快照

LegacyChannel = {id, name, group, channel_type, base_url, models_csv, priority}，是调用方提供的无凭据渠道描述，不代表持续有效的回读确认。不得通过“名字的 HashMap 最后一个覆盖前一个”消除歧义。

LegacyImportPlan 私有字段包括统一的 newapi_group、按旧 keys 顺序的 SourceDraft/channel_id/priority 和规范摘要。摘要覆盖全部持久导入内容，使用确定字段顺序的 serde 结构计算，不包含 source_path、当前时间或偶然查询顺序。模型序列等按本契约规范化后再计算。原始配置和摘要均不打印。

ImportOutcome = Applied 或 AlreadyApplied，附同一组 GroupId / SourceId / MembershipId 收据。只用于进程内控制，不是 HTTP 输出。

CatalogSnapshot 的公开字段严格取 schema 白名单；不直接给内部数据库行 derive Serialize。sources/groups/memberships 均按各自稳定 ID 升序，数组可空。快照包含历史退休身份，供后续对账；不是可恢复配置导出，也不包含当前 token 消耗或来源可用状态。

## 5 模块细化

### 5.1 公共调用与底层假设

Store 持有唯一 SQLite Connection 和私有锁文件句柄，写方法要求 &mut self。连接不向外暴露；不让异步任务在事务内等待网络。未来由架构定义的单写者任务拥有 Store，本步只有同步本地调用。

被调契约统一编号，下面每个函数均引用：

- C1 std 文件系统：create_new 原子拒绝已存在路径；元数据检查不写内容；try_lock非阻塞。仅关闭单个File不保证继承/重复句柄对应的锁释放，因此持锁对象Drop显式unlock。路径所在父目录可信、同 UID 无恶意并发替换是前提；不防御本机同用户攻击者。
- C2 rusqlite：BEGIN IMMEDIATE 成功取得写事务；唯一/外键/CHECK 错误不能当成功；提交前退出回滚；提交失败按 §3 处理，不能一概声明未提交。
- C3 serde：严格类型序列化/反序列化，未知枚举/字段报错；SecretString 无 Serialize，不被普通输出间接序列化。
- C4 validate_source / validate_group：按下文纯校验，成功给出规范数据；失败不修改输入或存储。
- C5 既有 normalize_models_csv：trim、去空、保留第一次出现顺序，不访问网络；空集由调用方拒绝。
- C6 SQLite backup：step 返回 More/Busy/Locked/Done 或错误；只有 Done 才可认为复制完成。不得使用可能无限等待的重试循环。

性能约束：单次导入最多 1000 来源；单库最多 1000 来源、100 组、10000 成员，退休行计入上限。超限拒绝而不是截断；限制是首版管理规模保护，后续扩容需独立评估。循环遍历有限输入，备份另有明确步数上界。

### 5.2 validate_source 和 validate_group

签名：validate_source(SourceDraft) -> Result<ValidatedSource>；validate_group(GroupDraft) -> Result<ValidatedGroup>，Validated 类型字段私有，仅这两个函数构造。

前置：来自调用方的未可信输入。后置：名称、枚举、URL、headers、模型、凭据、火山策略均满足 §4；或返回 invalid_input，零副作用。

步骤：先检查大小和值域；依据 settings variant 分支检查 GLM/火山专属字段；按 C5 规范模型并拒绝空结果；逐项解析 URL 和 header，错误转换为固定字段名，不输出值；检查 key 后计算敏感指纹；构造 Validated 对象。Group 只做组名和类型检查，不访问 NewAPI 注册组。每个输入列表设上限（模型 1000、每层 header 64），越界在循环前拒绝。

正确性论证：所有能入库的值必须先通过 C4，按枚举分支检查排除跨类型字段混用；循环按有限模型/header 列表推进必终止。每个失败出口发生在事务之前，只有内存对象构造，不可能改变来源/组或路由。

### 5.3 CatalogStore::open

签名：open(path: &Path) -> Result<CatalogStore>。路径必须显式指定，不能缺省落到生产目录，也不接受 :memory: 或 SQLite URI；测试也使用真实临时目录。

前置：Linux/Unix 私有目录，文件系统支持 SQLite 本地锁；部署方保证不是网络文件系统。本步非 Unix 返回 unsupported_platform，不影响原有未接入目录库的 CLI。

步骤：

1. P1 路径和权限检查。绝对路径、固定文件名 catalog.sqlite3；直接父目录若不存在只创建这一层（0700），不递归创建祖先；存在时必须真实目录、无组/其他用户权限。拒绝最后一层目录或文件的符号链接，已有数据库/锁/WAL/SHM 必须普通文件、无多重硬链接且权限不宽于 0600。失败不 chmod 用户旧文件。
2. P2 打开目录内 .catalog.lock 并 try_lock，持有至 Store 释放；锁冲突立即 already_open。不存在的 DB 用 create_new 和 0600 预建，禁止 truncate；本次新建标志保存在内存。
3. P3 只读检查已有库的 application_id/user_version/catalog_meta；识别常量 APPLICATION_ID=0x51545343。不是本次新建且无正确标识时返回 foreign_database；高 schema 返回 unsupported_schema，不迁移、不设置 WAL。上次初始化失败遗留空文件报 foreign_database，由人工辨认后处理，不自动删除。
4. P4 对已识别库或本次新建库建立 RW 连接，busy_timeout=1000ms、foreign_keys=ON。新库在单事务创建 schema_v1.sql 和标识；任一失败不返回 Store。已存在 v1 检查标识、表版本与必要索引/外键，不“修复性”重建缺失表。
5. P5 设置并回读 journal_mode=WAL、synchronous=FULL；检查 quick_check、foreign_key_check 和本模块必要的跨字段约束。失败关闭连接，保留文件供诊断，不静默初始化空库。

正确性论证：C1 的排他锁排除第二个受管写者；P3 在任何持久 pragma 之前拒绝外库，避免把 NewAPI 数据库当状态库初始化。C2 保证新 schema 原子出现；P5 未通过则没有可用 Store 句柄。副作用仅限显式私有目录及其中本应用的 DB/锁/WAL/SHM；拒绝已有外库时最多留下本应用锁文件，不修改外库内容。

句柄Drop依字段声明顺序先关闭连接，再销毁CatalogLock显式unlock，不删除锁文件，也不在CatalogStore::drop提前解锁。只有try_lock成功才构造持锁对象；构造前/后的错误退出均保持数据库资源先于锁释放。解锁失败时析构不panic，File仍关闭，后续open仍按OS结果判断。没有PID存活登记、延时重试或放宽AlreadyOpen。详见[锁生命周期修复](../../../fixes/catalog-fix-inherited-lock-release.md)。

### 5.4 create_source 和 create_group

签名：create_source(&mut self, draft: SourceDraft) -> Result<SourceView>；create_group(&mut self, draft: GroupDraft) -> Result<GroupView>。

前置：Store 未处于 commit_unknown 状态；无活动事务。后置：恰好新增一条来源及其私有凭据，或一条组；ID 唯一、revision=1、未退休。新 source.manual_paused=false 仅代表没有人工暂停意图，不声明来源已能推理；新 group.ready=false。

步骤：按 C4 完整校验 → BEGIN IMMEDIATE → 检查指纹或组名重复，存在则返回对应重复错误（来源附既有 ID）→ 仅对新身份检查容量 → 插入 secret/source 或 group → 从本事务构造安全 View → commit → 返回 View。冲突/构造失败回滚；commit IO 异常标记句柄不可继续写，返回 commit_unknown。新来源不创建成员、渠道或火山运行期零值。

正确性论证：来源/凭据在同一 C2 事务内，消除无主 secret 或无凭据 source；唯一索引作为检查后的最终防线；commit 之前不向调用者发布实体。失败分支按 §3 保留旧目录，唯一外部副作用是本库写入，无供应商/管理 API 调用。

### 5.5 plan_membership

签名：plan_membership(&mut self, source: SourceId, group: GroupId, standby_priority: i64) -> Result<MembershipView>。

前置：显式的来源/组 ID 和合法 priority。后置：新增一个 planned 成员，channel_id/observed/applied 均为空；既有成员和其他组不变。

步骤：校验 ID/priority → 事务中读取两端 → 缺失 not_found、源退休 retired、不同类型 type_mismatch、有效组合重复 duplicate_membership → 查本组历史最大 position，checked_add(1)，空组为 0 → 容量检查 → 插入 planned → commit。源暂停仍可建立计划关系，但不能由此启用，后续协调器须重查源状态。

正确性论证：同事务读取与复合外键保证两端类型一致，部分唯一索引保证仅一个有效成员；历史 position 不重排，新增不会改变原选择顺序。没有 channel_id 或外部调用，不能生成活动副本；它不是来源“已加入线上组”的回执。

### 5.6 build_legacy_import

签名：build_legacy_import(cfg: &Config, channels: &[LegacyChannel]) -> Result<LegacyImportPlan>，纯函数。

前置：已加载的旧 GLM 配置与调用方明确提供的渠道描述。后置：每把旧 key 恰好映射一个实际旧渠道，保留顺序、备注、凭据、quota 参数和渠道实际模型；无缺失或歧义，或者整批报错。

步骤：

1. P6 校验条目总数，建立按 ID 和名字的多值索引；重复 ID 即错，名字重复保留为歧义。不取任意一个，不静默跳过解析失败条目。
2. P7 按 cfg.keys 原顺序遍历：显式 channel_id 优先且必须存在；无 ID 时要求恰好一个同名渠道。两个 key 指向同一 channel_id 或使用同一凭据，拒绝整批，不能擅自合并不同 selector。已有配置的“未解析 key 跳过”不适用于迁移，因为迁移必须全量可回退。
3. P8 所选渠道必须同属一个明确非 auto 的单组；有模板时验证组/type/base_url 与模板相符。模型使用渠道实际 models，不用模板 fallback 重写。模板提供已加载的 model_discovery；无模板则保持 None，不在此猜测发现端点。
4. P9 先检查原始 models_csv 的逗号分隔条目数 ≤1000、两层 header 各 ≤64，再规范化/复制。每项创建 GLM SourceDraft：quota_url、global_headers 和 key_headers 分别保留，调用 C4 校验；name/note 按 §4 规范，输出顺序不变。记录实际 channel_id、priority，不把它们解释成已核对的 active/available。
5. P10 对规范导入结构固定序列化并计算摘要，构造字段私有的 Plan。任一错误不返回半份计划。for 循环最多 1000 次，每次消费一条，必终止。

正确性论证：P6/P7 的唯一解析使“配置 key → channel”是单射；P8 固定组确保旧路由归属不被迁移重写；P9 原样保留两层 header 和旧顺序，不改变 GLM 参数。C4 全量通过才进入 P10，Plan 可以作为下一函数的合法输入；函数只读内存，零文件/网络副作用。

### 5.7 import_legacy

签名：import_legacy(&mut self, plan: &LegacyImportPlan) -> Result<ImportOutcome>。

前置：Plan 来自 5.6；Store 可写。后置：第一次完整导入及标记原子提交；相同计划再导入返回同一收据，不改变现有目录；不同计划返回 import_conflict，不自动覆盖。

步骤：

1. P11 BEGIN IMMEDIATE，查询唯一 legacy_import。存在且摘要相同：校验收据引用仍存在，返回 AlreadyApplied，既不比较当前字段是否已被用户修改，也不重置暂停/退休。摘要不同：回滚报 import_conflict。
2. P12 无标记时要求目录表和 secrets 均为空，否则 import_requires_empty；插入唯一旧组，ready=false。
3. P13 按计划顺序插入 secrets/sources/memberships，沿用 create_source 的内部事务插入 helper，不嵌套事务。所有来源为 GLM、manual_paused=false；成员为 imported，channel_id 和 desired_priority 保留旧值，position 等于原序号，observed/applied 为空。
4. P14 将收据和摘要写入 legacy_import，同一事务 commit；commit 成功才返回 Applied。中途故障全回滚；提交不明按 §3 关闭句柄后重开，对标记与收据对账。

正确性论证：C2 将“全部实体存在”和“已迁移标记存在”绑定在一个提交点；不会出现重启补半批或复制渠道。摘要相等仅重放收据，不执行 UPDATE，故后续人工修改不会被旧配置复活。循环上界等于已验证 Plan 长度，唯一约束/外键在每次插入维护历史身份。原 config 和 NewAPI 均未被本函数打开或修改。

### 5.8 snapshot 与 read_source_material

签名：snapshot(&mut self) -> Result<CatalogSnapshot>；read_source_material(&self, id: SourceId) -> Result<SourceMaterial>。

snapshot 开启只读事务，分别按 ID 升序读三张公开实体表，严格解码/校验并以字段白名单构造 View；任一异常返回 corrupt_data，不发布部分列表。结束事务后返回 schema_version=1 的 Snapshot。不得 SELECT * 后把 secrets/settings_json 原样序列化。

read_source_material 仅为可信后端的后续 probe/reconciler 使用；按稳定 ID JOIN source_secrets，严格解码 settings；不存在/已退休返回错误。SourceMaterial={settings,credential} 不实现 Serialize，Debug 脱敏，不可作为 HTTP 响应。

正确性论证：同一读事务保证三个数组来自一致数据库状态；有限表行数保证查询遍历终止；显式白名单不能附带私有字段。source_id 与 secret FK 保证私有材料归属唯一。两函数均只读，无网络或业务状态写入。

### 5.9 backup_to

签名：backup_to(&self, destination: &Path) -> Result<()>。只备份本应用库，不提供恢复或备份 NewAPI 库的隐式入口。

前置：Store 无活动事务，目标为显式私有目录中的新文件，不与源路径相同。后置：成功目标可独立打开且具有相同 schema、目录、凭据和导入标记；源库不变；失败不发布成功文件。

步骤：校验父目录/目标权限、目标及其 WAL/SHM/journal 均不存在 → 在目标目录 create_new 保留目标名，0600 → 建立独立目标连接 → C6 Backup::new → 最多 10000 次 step(128)，Busy/Locked 累计最多 100 次，每次等待 10ms，且总尝试期限 10 秒 → Done 后退出 Backup、关闭目标连接、sync_all → 返回成功。任何错误释放句柄，删除的仅是本次 create_new 创建且身份未变化的失败目标；删除失败或无法核对身份返回 BackupFailed 且 incomplete_backup=true，不能删用户已有目标。

正确性论证：备份读取 SQLite 已提交视图而不是裸拷贝主文件，包含 WAL 中已提交数据；固定步数与重试上限保证循环结束。create_new 排除覆盖已有备份，错误清理目标由本次创建身份限定，源库只有读操作。10 秒是重试循环期限，不保证底层单次文件系统调用可被硬中断。

### 5.10 helper 与并发退出约束

以下 helper 是本步实现清单的一部分，不能以“内部函数”为由绕过失败规约。简单字段装配和唯一枚举的 as_str 为 trivial；其他 helper 的推导如下。

| 私有函数 | 前置与步骤 | 返回和副作用论证 |
| --- | --- | --- |
| parse_positive_id(text: &str) -> Result<i64> | 先按 §4.1 检查十进制字符/首零，再 checked parse，最后检查 >0；任一分支失败 invalid_input | 无状态写入；格式和值域两关同时成立才返回；遍历字符串长度有限 |
| prepare_private_directory(path: &Path) -> Result<PathBuf> | C1：检查绝对路径及父目录，已有叶目录拒绝 symlink/宽权限；仅缺失叶目录时用 mode=0700 创建；检查后返回规范路径 | 只可创建明确的一层目录，不递归建祖先；IO/权限异常立即返回，已有目录不 chmod |
| check_private_file(path: &Path, allow_missing: bool) -> Result<()> | C1：symlink_metadata；不存在依 allow_missing 返回或报错；存在须普通文件、nlink=1、无组/其他权限 | 元数据检查纯只读；不符拒绝，不跟随末端链接，不输出文件内容 |
| insert_source(tx: &Transaction, source: &ValidatedSource) -> Result<SourceView> | C2/C4：先检查指纹重复并返回旧 ID，再对新身份检查容量，插入 secret/source，checked 读取 ID 并构造 View | 不另开/提交事务；满容量不遮盖重复来源；外层回滚涵盖两次插入，消除孤儿 |
| insert_group(tx: &Transaction, group: &ValidatedGroup) -> Result<GroupView> | C2/C4：先检查组名重复，再对新组检查容量，插入 ready=false/revision=1，校验返回 ID | 只向外层事务写一条组；满容量仍返回 DuplicateGroup；不建立外部权限 |
| insert_membership(tx: &Transaction, row: &NewMembership) -> Result<MembershipView> | NewMembership 私有临时参数含两端 ID、kind、channel_id、position、lifecycle、priority；只接受 planned/imported，先校验值域、两端、退休及成员/渠道重复，再检查容量并插入 | 不提交；满容量不遮盖已发现的重复身份；复合 FK/唯一索引兜底；imported 仅允许 5.7 调用 |
| decode_source_view / decode_group_view / decode_membership_view(row: &Row) -> Result<View> | 读取显式命名的白名单列，检查 ID、类型、边界和生命周期字段配对；任一失败 corrupt_data | 不访问 secret/settings_json，不修改数据库；所有检查通过才构造完整 View |
| decode_settings(text: &str) -> Result<SourceSettings> | C3：直接从文本严格解码 version=1，再复用 C4 中不含 credential 的设置校验；将结果编码为 Value 与原始 Value 对比，捕获旧类型忽略字段或默认补值 | 无外部状态写入，错误不包含 text；未知字段/缺失字段/非法值报 corrupt_data，私有设置不会被默认值“修复” |
| From<rusqlite::Error> for CatalogError | 按结构化类型区分列解码损坏、busy/locked 和其他 storage；不解析或回显 raw message；提交失败由外层处理 | 固定 catalog 错误域和允许的 ID 是唯一常规上下文，不引入通用错误框架，不打印 SQL/参数或自行重试 |
| validate_url / validate_settings | URL 仅解析校验；设置按 GLM/火山分支先检查列表上限，再遍历 headers/models 和调用 URL 校验；无网络 | 返回规范化设置或 invalid_input；有限输入遍历终止；不修补持久值，持久解码另与原 Value 对比 |
| fingerprint / encode_settings | 前者按固定域前缀计算 SHA256；后者对已验证设置包装 version=1 后序列化 | 仅生成内存敏感值，供入库/摘要使用；不输出到日志/快照，序列化失败 storage |
| commit(tx, poisoned) | 消费事务执行 commit；成功返回；busy 依赖事务 Drop 回滚；其余失败设置 poisoned 并返回 commit_unknown | 无未知结果被当成功；毒化位只从 false 变 true，必须重开才能继续 |
| usable / capacity | usable 为单一毒化位判断，trivial；capacity 仅对模块内固定表名 COUNT 并比较上限 | 无写入；毒化返回 commit_unknown，达容量返回 invalid_input；表名不接受用户文本 |
| member_parents | 同事务查询两端，缺失/退休/类型不匹配逐分支拒绝，再解析唯一 SourceKind | 无写入；成功意味着当前事务内两端可建立关系；SQL 列解码错误为 corrupt_data |
| verify_schema | 查询元版本，遍历本版本有限 CREATE 语句，与 sqlite_master 原 DDL 精确比较 | 缺表/索引或约束弱化报 corrupt_data，不重建、不修改数据库；有限 DDL 集合保证终止 |
| map_probe_error | 身份探查的 NotADatabase 映射 foreign_database，其他错误复用结构化 From；只读连接也显式设置 busy_timeout=1000ms | 不把暂时 Busy/Locked 误报成外库；无状态写入、无原始错误回显 |
| read_rows / verify_snapshot | 前者在 LIMIT 数量保护下逐行解码；后者构建来源/组映射并核对每个成员的类型、渠道唯一、有效组合与顺序唯一 | 任意失败不发布部分快照；最多允许的行数或下一超限行后退出；集合不包含凭据 |
| verify_receipt | 先校验数量及 ID 唯一，逐对查询收据来源/成员/组关系存在；退休/改名不改变历史关系 | 只读，不把收据重放变成 UPDATE；最多 1000 对，缺失或错认则 corrupt_data |
| read_material(id, allow_retired) | usable 后 JOIN source/secret；公开调用拒绝退休，open 校验历史时允许退休；严格解码并核对 kind 与凭据指纹 | 返回只供后端的非序列化 SourceMaterial；无凭据日志，SQL 类型损坏报 corrupt_data |
| cleanup_failed_backup(target, identity, remove) | 比较当前 dev/ino/文件类型；只对本次目标调用 remove；缺失视为无残留；身份改变、无法检查或删除失败标记 incomplete_backup | 永不删除身份不符目标，返回固定 BackupFailed；remove 参数只为私有函数测试注入，生产使用 fs::remove_file |

表内非平凡 helper 的共同论证：调用前的 C1/C2/C3/C4 分别提供受控文件、活动事务或已验证类型；每一步显式检查排除非法中间值；只有完整成功路径构造输出，失败路径不自行提交。循环均遍历有界输入/结果，退出传播给 5.3–5.9 的资源释放或事务回滚路径。新增其他非平凡 helper 时，必须先补同等规约，不用 unwrap/panic 处理用户输入错误。

所有外部错误只向上返回 CatalogError；无后台重试或自动新建第二个库。数据库锁协议：目录进程锁先于 SQLite 事务；不存在逆序获取。Store 独占拥有连接，不实现 Sync，也不将连接 clone 给其他线程；&mut 方法保证同句柄事务不重叠。

单库写失败后不发布新快照；commit_unknown 句柄拒绝所有后续读写，必须重新 open。Drop 不依赖 async runtime。备份期间本 Store 无写入，其他受管进程被目录锁拒绝；SQLite 外部人工写入不是支持的运行模式。

## 6 完整性自检与验收

- [x] 非平凡操作列出校验、分支、事务、成功/失败/未知提交出口。
- [x] C1..C6 明确被调方承诺；循环输入、上限和备份终止条件已列出。
- [x] 身份、同类型、唯一性、历史映射、私有字段等跨模块不变量与架构一致。
- [x] 源/组的本地创建与外部生效区分；旧渠道导入不冒充已确认。
- [x] JSON 输出白名单 schema、十节 expectations 先于实现产出。
- [x] 用户确认细化后进入编码；实现前已复核 §4.4。此自检不代替实施后双路径审计。

最低测试集：非法/重复来源与组；同源多组；类型拒绝；跨组不串 channel；旧 key 顺序与显式 ID 优先；歧义名字/缺失渠道/混组/重复 credential 全量失败；重复导入及不同摘要；故障注入回滚；重开不重导；外库/高 schema 不修改；并发打开立即拒绝；WAL 备份及拒绝覆盖；JSON/Debug/错误凭据不泄漏。

复审补充：满容量时重复 source/group/member 仍返回重复错误，真正新增仍拒绝；退休来源的凭据仍返回旧 SourceId。C1 同时查 source/secret 数量、引用唯一与 key 对应关系，重复组必须真实调用 create_group。C6 逐来源生成可区分的输入，完整比较关联和材料；C7 直接格式化内部敏感类型，不只检查外层容器或敏感字段名。

测试 helper assert_legacy_fidelity 的前置为内存中的合法假配置、唯一渠道描述及实际导入收据；按原配置顺序独立找到渠道，再沿实际 member → source → material 引用对照 source/group/channel/order/priority、凭据、名字、备注、模型和完整 header 序列。模型期望用独立去空去重循环构造，不调用生产规范化函数。遍历上限为有限输入条目，任何差异立即断言失败；仅查询本次临时库，不修改目录、不访问网络。该 helper 供确定性回归与 C4/C6 共同使用，返回已核对快照用于重开/重放比较。

属性测试覆盖共享来源唯一、同类型与 channel 全局唯一、重复导入不变、事务失败等价于操作前快照、快照序列化无秘密。纯目录操作序列与 BTreeMap/集合参考模型交叉验证，每项至少 128 cases，不用改期望掩盖失败。

本步测试仅用 TempDir 和假凭据，不启动 NewAPI、不读取生产 config.toml。不改服务监听地址、进程或真实数据库。最终验证须同时包含机械检查、workflow §6 独立 subagent 审计、§5.2 五维度复核；尚未执行的验收保持未完成。
