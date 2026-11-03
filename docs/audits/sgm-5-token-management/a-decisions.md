# SGM5 A 审核处置

日期：2026-10-18，基线eb0b727；范围为原生令牌接口，不是完整访问key管理交付。

## 设计审处置

1. 历史创建恢复不能复用“有效期必须未来”的校验。拆为固定关联名/单组/秒值域结构校验，以及仅新建入口和最终准入使用的未过期校验。查回允许现已过期、耗尽或账号余额不足；不重新创建。
2. 原生search底层为LIKE，不能把查询名字称为字节精确。返回单条时额外检查name逐字相等；多条就是Ambiguous，不挑最新ID。加入大小写折叠反例。
3. 剩余额度允许消费后的安全负数，使用量仍要求非负；不把合法legacy耗尽key拒成损坏，更不把缺失数值默认为0。

以上均在实现前同步细化及事前expectations，独立复核关闭设计阻断。

## 实现审处置

通用transport允许任意合法2xx包络，SGM5 A细化只接受200。token层新增精确status检查，对201/202保持InvalidResponse/Uncertain，写前读取失败再映射NotSent；没有改变旧传输语义。新增profile/groups/create/list四阶段负向矩阵；明确success=false仍为RemoteRejected/ResponseReceived。

原生缺失只在专用正ID token GET、200/false/固定缺失文本及data缺省/null时放行；普通GET及写请求仍不能把false当缺失。机械脚本仅剥离新增factory、lookup及可选固定owner插入后，逐字比较旧client与transport（忽略空行），并冻结其余管理实现/测试。

取消组合测试只证明A调用链取消后仍可请求；64槽全部释放引用原transport测试的available_permits断言，不把一次取消测试夸大成完整资源计数证明，不新加诊断API。文档的三个请求上限指业务请求，登录/401恢复另计；token页循环≤100，groups迭代则受1MiB正文约束。

首轮clippy发现CreationLookup空分支携带248字节投影，One改为Box持有；语义与schema不变。清理测试未用import并补available_groups实际边界测试，最终clippy只剩6项旧警告。

## 实测和范围

原生探针第一次将渠道禁用请求误写为不存在路径，在9项检查后404；改用仓库已有POST status/batch并先回读，未改变业务期望。随后两个全新隔离实例各13项通过，所有自建进程正常退出。第一次Python导入产生的__pycache__移动到本次临时调研目录，之后使用-B，仓库不提交生成缓存。

A不提供账号创建/充值或组配置写，不挂HTTP/CLI，不保管访问key明文。普通签发账号及其额度/角色生命周期、组注册/就绪/引用保护、稳定操作/重开对账分别属于后续B/C及发布组装；不能以A的网络ACK替代它们。新token创建实际默认启用，只有持久查回确认后上层才可允许复制。

最终身份恢复检查发现：一次profile校验不能阻止401后重登换成不同principal。新增两个回归先在旧实现得到0 passed/2 failed，再按[修正方案](../../fixes/newapi-token-fix-auth-principal-drift.md)将expected_user_id固定到每个token业务请求，每次prepare核Session ID并固定实际header；默认旧请求None不受影响。修正后newapi模块108 passed/0 failed/2 ignored。此前全量/9项变异结果保留为修正前证据，最终验收重跑，不冒称它们已经覆盖新修正。

收尾独审无新增代码阻断；文档已精确区分业务不重试与明确401后的单次认证重放，完整key仅对上层显式返回与HTTP/解析期间暂持原文，以及原生Bearer GET self和cookie POST create两条错header实证，不夸大秘密生命周期或探针覆盖。

运行验收及最后独审结论见[A审核](a-audit.md)。
