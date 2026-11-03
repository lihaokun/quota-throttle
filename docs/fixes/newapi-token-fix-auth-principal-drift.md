# 令牌请求重登后的固定账号修正

2026-10-18，SGM5 A实施期发现并修复，未接入运行服务。用户已授权连续实现及审核修正；本修正不增加账号服务、缓存或生产操作。

## 一 现象和最小复现

新接口先核验profile.user_id=41，但实际POST遇401后通用transport允许重登返回任何正ID。若返回42，prepare_request将New-Api-User改为42，cookie也为42，原生可在42名下创建并仅返回success=true。新建ACK无ID/owner，调用方无法发现错签。

回归用例`t5a_bound_owner_survives_reauthentication`用原Fake复现41→401→42，预期AuthFailed/NotSent、仅一个被401拒绝的创建请求、无42创建；41→401→41仍允许原恢复。第二用例验证Bearer业务请求固定41，不使用错误静态header999。均为回环假服务。

## 二 根因

一次profile验证不是之后会话恢复的身份约束。业务guard目前只覆盖调用方版本和expiry，transport会在每次attempt重新选择Auth，但请求未携带期望owner。检查“同一账号不得外部并发改角色/凭据”不能替代对本地可观察的principal ID漂移的拒绝。

## 三 参考实现对照

本地transport的refresh_session_locked只检查登录id>0；prepare_request按Session(id)写New-Api-User。固定NewAPI的authHelper比较session/access-token的id和该header；两者都42就放行，只有header固定41才能拒绝cookie42。参考：[固定版本鉴权](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/middleware/auth.go)。这不是原生缺陷，不改NewAPI。

## 四 修正方案

ManagementRequest增加私有expected_user_id，new默认None。仅ManagedTokens统一send把owner.user_id附到每个业务请求。prepare_request每次执行时：若Session ID与期望不等，AuthFailed/NotSent；实际New-Api-User始终覆盖为期望ID（Bearer亦如此）。登录请求本身仍不带此约束。

保留原预算、认证次数、guard与重试语义；不再额外发profile。没有expected_user_id的全部旧请求保持原行为。B/C仍负责业务版本guard及托管身份生命周期。

## 五 正确性论证

初始登录和每次401恢复均重新prepare，故Session(42)不能发送owner41请求。即使builder构造后cookie jar被并发重登更换，固定header41与cookie42不匹配，原生在业务handler之前拒绝；下一次prepare再拒绝不同Session ID。正常41→41不改变恢复行为。Bearer也绑定已知期望header，不能被静态配置改认owner。旧请求None不进入新增条件，原header/认证行为逐字保留。

同ID的外部角色升降竞态仍受既有部署假设约束；本修正不新增角色缓存，也不声称阻止管理员在别处修改账号。

## 六 测试同步

- 先运行两个新回归，保留旧实现红例证据。
- 修正后回归、全部newapi测试、默认debug/release全量、schema/Step/冻结检查及clippy/build。
- 在独立副本重新运行原9种变异，并加“漏固定header”“漏Session ID检查”两类负向验证。
- 原18项独立性能最后串行执行，原生探针继续仅独立临时实例。

## 七 代码更新

仅ManagementRequest私有字段/绑定方法、prepare_request的条件及token统一send；测试新增在tokens/tests.rs。旧client/transport归一比较只扩充明确插入的剥离，不放宽冻结边界。

代码、两个新增回归及机械清单已随SGM5 A提交于`6ece728`；逐项核对无遗漏，旧请求默认None保持不变。

## 八 文档更新

已同步SGM5细化、事前expectations、架构类型约束、a-audit/a-decisions及机械清单。此前576项全量和9项变异通过为修正前证据，不冒称修正后验证。

上述契约与验收文档也包含在`6ece728`；本次文档提交仅回填已产生的实现哈希，不改代码或验证条件。

最终修正后验证：旧实现两条回归0 passed/2 failed；修正后NewAPI模块108 passed/0 failed/2 ignored，默认debug/release各578 passed/0 failed/20 ignored。11类有效编译变异（含本修正两项）均被捕获，还原源码后23项普通令牌测试通过；18项独立性能、精确机械检查、clippy/release build及独立复审通过，0个未关闭问题。原生隔离探针15项通过，新增分别为Bearer GET self和cookie POST create的错header拒绝；没有请求真实供应商或改运行服务。完整证据及保留边界见[A审核](../audits/sgm-5-token-management/a-audit.md)。
