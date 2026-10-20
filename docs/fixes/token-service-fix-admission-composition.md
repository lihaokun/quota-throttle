# 令牌服务准入与幂等组合修正

2026-10-18，SGM5 C首次实现静态审核发现，尚未接运行或部署。沿连续实施授权，先固化回归再修正，不改原GLM/火山策略或传输预算。

## 一 现象与复现

正向准入只取第一个本地eligible成员；原生已禁用它而第二个成员正常启用时，新key被错误阻止。C提交还在B幂等查询前读取目标身份/owner配置；重用operation_id但指定不存在的另一个token时先NotFound而不是IdempotencyConflict，历史重放也可能被当前身份缺失阻止。

组完整扫描把每行30秒fresh当配置身份条件；真实backend仍在60秒窗口内时，首页行超过30秒可被误报BindingDrift。配置/正向准入还漏了已有火山auto_ban=0约束。以上都可用私有临时目录和假原生接口复现，无需真实上游。

## 二 根因

把“找到首个本地候选”当成“找到一个实际可用候选”；在组合层增加了比B幂等更早的易变前置检查；混淆稳定配置扫描与正向单次实时观察；复用identity_matches时漏读其只验证auto_ban属于0/1，而非火山必须0的callee边界。

## 三 参考实现对照

原B submit_token_operation先canonical/replayed_operation后查身份；原planner允许GLM保留原生禁用状态，并在火山auto_ban!=0时安排字段修正；原ManagedChannels完整分页和原backend lease为60秒。C不能扩大这些证明的含义，不通过修改原策略或全局观察TTL解决。

## 四 修正方案

正向路径在同一30秒frame内有限遍历本地eligible候选；status非1或priority未收敛可试后项，身份/模型/火山auto_ban漂移及网络失败仍终止。保持完整原预算和期限，不承诺大目录一定在lease内发现候选。

B增加复用原canonical/replayed_operation的只读入口，C先处理已有操作的相同/冲突载荷，再对新操作查configured owner。无第二摘要算法。组配置核验提取纯helper以测试旧行时刻，配置本身不套30秒行龄，最终仍检查原backend/epoch；token单候选新鲜性与30秒TTL不变。仅Volcano追加auto_ban=0，GLM原语义不变。

## 五 正确性论证

有限候选集合每轮消耗一个且所有请求同frame，成功必须有独立原生启用证据；跳过不可用候选不跳过漂移。先规范幂等保证重放/冲突不依赖当前资源存在；新请求仍校固定owner。完整配置扫描不等于实时可服务性，正向路径独立重读保持时效，60秒backend门禁未放宽。没有新的状态表或许可权威。

## 六 测试同步

| 类型 | 用例 | 状态 |
| --- | --- | --- |
| 回归 | 首候选原生禁用、第二候选可用，创建可以发送 | 已固化，旧实现失败 |
| 回归 | 已有operation_id的异载荷不存在目标仍冲突；owner缺失仍保留历史重放 | 冲突及Create缺issuer均有成功编译红例，修正后组合通过 |
| 回归 | 31秒配置观察不误报漂移，正向TTL仍严格 | 已固化，旧实现失败 |
| 回归 | 火山auto_ban=1拒绝配置/正向；GLM允许原值 | 已固化，旧实现失败 |
| 回归保护 | C全部定向/PBT、全量debug/release、变异/性能 | C42普通含6组PBT通过，14类编译变异捕获；最终debug/release各645/0/26 ignored，23项release性能及旧大组debug门禁通过 |

修正前定向为0 passed/4 failed，0.78秒，四项均成功编译后命中预期断言；日志`/tmp/quota-throttle-sgm5-c-admission-red.log`。此项保留红例证据，不计作通过。

四项修正后定向4 passed/0 failed，0.56秒。复核同类入口还发现Create wrapper在构造命令前要求当前issuer，需在同一提交临界区对已有Create操作采用不可变持久owner后复用同一canonical判重；仅新ID要求当前issuer，历史owner不用于新发请求。追加缺issuer的Create同载荷/异载荷/新ID/其他kind同ID回归，再跑红绿。

Create追加红例成功编译后0 passed/1 failed，0.57秒，命中当前NotReady而非返回历史view；`/tmp/quota-throttle-sgm5-c-create-replay-red2.log`。首次红例命令的测试模块路径编译错误已另记，未算作捕获。

## 七 代码更新

| 文件 | 计划改动 | 状态 |
| --- | --- | --- |
| `worker/token_service.rs` | 候选遍历、配置helper、火山字段核验 | 已验收，先红后绿；`abaafc4` |
| `worker/token_service/execution.rs`、`catalog/tokens.rs` | 原B只读重放入口与C先后顺序 | 已验收，Create持久owner重放同临界区；`abaafc4` |
| `worker/token_service_tests.rs`及其coverage/edges子模块 | 假接口与组合回归 | 已验收，含火山正向门禁/GLM保留原值；`abaafc4` |

## 八 文档同步

C细化/expectations/c-decisions已同步具体分支、有限候选和配置时效边界；B最终发送校验说明同步为等价的原子栅栏组合。2026-10-19实施、回归/全量及独审验收完成，最终原生组合8项检查通过；代码与契约提交`abaafc4`，见[C审核](../audits/sgm-5-token-management/c-audit.md)。
