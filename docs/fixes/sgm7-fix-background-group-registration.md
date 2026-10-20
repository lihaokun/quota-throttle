# 运行集成中的后台发布与组注册冲突

2026-10-19，SGM7 实施中发现，连续收尾授权内修复；本地现网及 fermat 既有服务不变。

## 1 现象和复现

fermat 隔离轨迹 `/tmp/quota-throttle-sgm7-combination-62fnu8w3` 已通过真实启动和model list来源创建。两个成员真实创建为disabled后，显式注册组失败。原生日志显示11:05:51 PUT option后没有继续完成别名写入，下一轮核验直接失败；只读库核对发现GroupRatio含新qt-volcano-one，原生别名仍未持久化，应用组ready=false。

原生注册需要多个有序读取/写入并共享1req/s预算。生产组合同时运行5秒火山/补归属、10秒GLM和collector，而前期SGM5原生测试是显式逐步驱动，没有这一组周期交错。预期是正常后台工作不持续打断用户启动注册，人工新命令仍可撤销过时操作。

## 2 根因

每次原mutate都会推进publication，publish还推进input_epoch。check_group的原生请求用开始时的Frame.guard逐次核验；后台即使是空补归属或续期发布也可令它失效。严格注册流程因此在两个原生倍率字段之间退出，而后续不会猜哪个别名是权威。这是实际运行组合缺少协调，不是应放松guard或自动覆盖未知原生状态。

## 3 原实现对照

`newapi/groups.rs::register_for` 的顺序为一致基线→写主倍率→核对中间态→写别名→回读→补可用组权限，契约保持。`worker::check_group` 已持lane，能排除coordinator/token round，却没有排除policy/collector/repair的后台提交。原`refresh_from`仍须一次撤销→检查→发布，不能以去掉epoch或延长期限解决。

## 4 最小修复

worker增加私有 `automatic_updates: tokio::sync::RwLock<()>`，不是第二数据库、队列或业务状态：

- check_group在取得原lane后try_write，成功后才capture Frame，直到原生核验及ready提交结束才释放。失败返回原QueueFull，无原生写。
- 新refresh_runtime_backend也try_write，原组合预检期间不被自动提交打断；旧refresh_backend不变。
- attempt5已通过三组注册及独立首选，但首次create_token被同类自动publication撤销，按原T5C进入waiting，需要显式Retry。因此同一锁补到每个token_step（已有round lane内，先try_write后Work/Frame，绝不包整个64项round），以及reveal_token（commands→try lane→try_write→Frame至返回）。不改Catalog状态机/准备票据/未知写规则；自动拒绝在Work加载和mutate前，不污染操作。
- GLM应用、火山发布、补归属、collector的ledger_mutation只在本地提交段try_read。失败返回原QueueFull/Busy，且必须在调用mutate前返回，不推进epoch、不写库。采样/日志网络等待不占该read guard。
- 人工source pause/retire、目录和其它显式控制不取这个锁，仍能立即推进原fence；不承诺阻止人工冲突或网络未知结果。
- 仅register=true开始前要求原backend lease至少剩30秒，不足则NotReady且零原生写，等待正常预检刷新，不延长lease。register=false保留原有效lease即可的条件。30秒只是起步余量，不保证多步请求一定在预算内完成；网络/429/人工撤销仍保持未知和严格拒绝，不猜别名权威。

每个取得的guard只随实际函数存活；async guard可跨await，原业务/关闭supervisor仍持有实际函数句柄。所有新获取都是try而非额外等待队列：拿不到就沿原有限周期/显式重试，不死锁；原lane先于独占guard，后台不拿lane，不产生反序。

## 5 正确性

后台提交先持read，注册/完整预检持write，故在独占期间没有自动mutate发生；Frame不会因这些无关定时工作失效。手动控制不参与新锁，仍按原epoch使旧Frame失效，安全性没有通过忽略版本换取活性。原生故障或显式冲突造成未知中间态时仍保持未就绪，不自动重写别名。

后台错过一轮只延后采集/发布，不伪造完整日志或零用量；Native推理继续，记录由原checkpoint/去重补采。火山限额仍为软上界、错误判定仍为观测语义。GLM选择、火山状态机、priority及DB v8/wire均不变。

## 6 测试

保留fermat失败轨迹。新增受控真实check_group等待期间的后台提交拒绝/epoch不变、人工控制仍可撤销、guard释放后继续，以及短lease零原生写反例；collector和补归属分别验证未入mutate。随后重新跑真实来源→禁用渠道→注册→原策略启用→Key的C轨迹和完整原测试，不改旧断言掩盖问题。

进一步加入日志页已收到后提交遇锁的精确交错，验证checkpoint/epoch/事实不变，释放后补采去重。token真实调用加入同样自动拒绝与人工撤销回归。C在明确waiting且带暂态error时模拟已有UI的显式Retry，始终使用原operation_id；waiting无错误代表等待回读，unknown不自动Retry，更不另建key绕过。不改变产品的显式重试语义。

## 7 代码清单

worker字段及初始化、新runtime预检薄入口；token_service::check_group；policy两个发布入口；collection::ledger_mutation；analytics::repair_attribution_once；对应测试。source_policy算法、catalog DDL及NewAPI原生注册算法零修改。

## 8 文档清单

本修复记录、SGM7细化/expectations/decisions、架构运行组合增量及最终审核同步。旧SGM5逐步验收记录保留，不冒称它覆盖了新运行周期交错。
