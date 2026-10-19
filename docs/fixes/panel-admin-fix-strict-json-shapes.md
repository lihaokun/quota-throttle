# 管理请求严格 JSON 形状修正

2026-10-19，A2.2实现阶段，基线bcb5700。用户已确认的契约要求对象、字符串枚举及任意层未知字段严格校验；本修正只落实该规则，不改变wire、默认值、错误码或业务策略。当前新接口未接CLI/在线服务。

## 一 现象与复现

第一版输入DTO直接使用Serde derive。发现三类隐式拓宽：internally tagged unit variant忽略额外字段；普通struct及internally tagged enum可从数组解码；字符串枚举还接受对象形如{"issuer":null}。这些形式均不符合已批准schema，必须400。

修复前h2_unit_model_mode_rejects_unknown_fields及h2_unit_usage_modes_reject_unknown_fields成功编译2分56秒后0 passed/2 failed，0.00秒，原预期保持。日志/tmp/quota-throttle-sgm6-a22-unit-mode-red.log。随后四条h2_serde_回归成功编译37.82秒后0 passed/4 failed，0.00秒，分别确认顶层/嵌套对象数组、tagged mode数组和四种标量枚举对象被接受；日志/tmp/quota-throttle-sgm6-a22-shape-red.log。

## 二 根因

Serde的通用数据模型比本API JSON形状宽。容器deny_unknown_fields不会改变unit分支的访客选择；InternallyTaggedUnitVisitor会用IgnoredAny消费额外成员。普通struct visitor含visit_seq，JSON数组因此按字段顺序解码；TaggedContentVisitor也有visit_seq。外部tag的enum visitor同时允许字符串和单键对象。

只改unit为struct不能封住数组入口，只检查顶层首字节不能保护nested对象，也不能先把JSON读成Value再筛键，因为那会覆盖重复字段。

## 三 锁定源码对照

已阅读Cargo锁定源码：
- serde/serde_derive 1.0.228：enum_internally.rs的Style::Unit/Struct分支；serde/private/de.rs:2990–2995的IgnoredAny循环，以及812–856的TaggedContentVisitor::visit_seq。
- serde_json 1.0.150：de.rs:1836–1853将数组/对象分别交给visit_seq/visit_map；1881–1897的enum同时接受对象/字符串。
- serde_derive 1.0.228：struct_.rs:70–92为普通struct生成visit_seq。

不升级依赖，不修改共享业务enum的全局Deserialize，不另写完整JSON parser。

## 四 修正方案

输入层增加object<T> helper：只调用deserialize_map，visit_map再用MapAccessDeserializer交原derive，原始KV次序与重复键不丢。顶层json<T>经此helper并检查Deserializer.end；所有嵌套对象（含Settings/Selection/Filter）明确使用object。可空discovery用Option<MapOnly<T>>，header数组用Vec<MapOnly<T>>，保留本来合法的models/header array域。

Selection::Discover与Filter::Any/Unknown改为空struct variant，继续同tag、snake_case及deny_unknown_fields，手写同义Default。Group.kind、Discovery.auth、Register.owner_scope及usage.grain先String::deserialize，再以StringDeserializer映射原enum，拒绝对象形态。

## 五 正确性论证

每个schema object节点先进入deserialize_map，故数组/标量不能进入derive的visit_seq；合法map仍交原字段visitor，重复/未知字段拒绝保持。空struct以零允许字段集拒额外成员。标量enum只有从String进入匹配，合法枚举值与旧类型完全一致。

默认discover/any/policy/auth保持；查询模型、catalog模型和token名称的不同值域保持。JSON语法仍由同一Serde解析，完整body后检查end。全部修正发生在worker调用前，失败固定400且无副作用、不反射正文。

## 六 测试

保留两条unit红例和四条shape红例，补默认/显式空分支合法、逐对象节点数组替代、合法数组域、四个标量enum形状、任意层未知/重复键以及原字段/语义400/422边界。PB1随机额外字段仍必须拒绝。有效编译变异须覆盖恢复unit形式、漏object门禁或漏scalar枚举门禁；恢复后重新编译。

## 七 代码清单

已按独审确认方案修改src/panel_http/admin/input.rs的输入helpers与根DTO字段标注、input/source.rs、input/usage.rs的对应字段/enum；测试在admin/tests.rs，另补Settings数组及错误码400检查，保留原accepted.is_empty断言。Cargo版本、旧策略及原store/worker行为不改。首次修复后定向重编译55.03秒，13 passed/0 failed/0 ignored、0.01秒，含PB1的128 cases；日志/tmp/quota-throttle-sgm6-a22-input-green.log。全量/变异/最终提交仍随A2.2验收回填。

## 八 文档同步

A2.2细化与十节继续维持原严格形状/未知字段拒绝契约，不放宽schema。实施审核保留此Serde陷阱、锁定源码和两轮红例。helper是现有H2.1解码实现细节，不增可见类型或产品选择。

最终回填：原6条回归保留；M05重复键Value覆盖、M13对象数组、M14标量enum对象、M15unit额外字段四种有效编译变异均命中原断言。恢复重编1分20秒后44/0/2；完整debug/release各739/0/34、机械schema、性能、lint/build及最终独审全部通过，证据见[A2.2实施审核](../audits/sgm-6-three-page-panel/a2-2-audit.md)。
