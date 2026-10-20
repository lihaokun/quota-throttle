# SGM6 B 审核处置

2026-10-19，设计基线b3a5fe6。设计阶段只细化和审核，不编码、安装依赖、上传源码或部署。用户已同意后续按需向fermat隔离临时目录上传测试必要源码，排除真实配置、密钥及数据库；本批未使用该授权，确认后的实施处置见下文。

## 保持的边界

三页仍在同一Rust服务、自包含HTML/CSS/原生JS内实现。一个访问key对应一个同类型组、两协议共用；共享来源一份额度/错误/流量，组成员渠道/priority独立。GLM原算法和火山状态机不改。旧status/run、策略、账本、DDL、NewAPI传输及Cargo依赖冻结。B不承担C的原生组合或SGM7发布。

必要增量仅静态资源、固定MIME/CSP、UiConfig(dry_run及显式公开NewAPI根地址)。现有H1已经只对/api/admin/v1/要求X-QT-Panel，静态导航无需绕过或放宽授权。配置不含issuer可用性猜测、上游材料或owner身份；未配置公开调用根时null，不能复制面板origin当调用地址。

测试依赖只提议dev-only fast-check4.10.2/Playwright1.64.1及锁文件；生产无框架/CDN/Node服务。本机只确认已有Node22，未发现PATH中的Chromium/Firefox/Playwright命令，不将浏览器测试称为可跳过。安装和实际兼容性在确认后的实施阶段验证。

## 独审反馈

| 初稿问题 | 最终决策 |
| --- | --- |
| null模型被一概要求Exact | 整体Any、null Unknown、字符串Exact原值；加入null与字面unknown并存的用例 |
| generation失效后finally可能不归lane或误清新请求 | lane.owner使用独立ticketId；abort只失效结果和取消fetch，settle按owner归还，归还前不接替；覆盖隐藏读/写与恢复先于finally |
| GLM评估新但相关样本已过期，组标签可继续新鲜 | 精确沿A1.4相关来源集合检查样本年龄，原Unavailable可满足时效；相关样本过期传播current/临期/pin_release，非相关缺样本不误伤 |
| state失败但旧快照未到10秒，仍可能显示新鲜 | 网络/协议/HTTP/超时失败立即过期；同publication成功不复活，新publication才重验 |
| updated_at/phase被隐含当作顺序，首回执身份不明 | 单lane+ticket因果接收；不按秒级时间/phase排序。op-only恢复先建绑定；create_token不把输入groupId/group revision冒充新token身份 |
| 火山周期查询grain/空区间/到界不清楚 | 固定hour；无runtime、空区间或到界不发伪新周期查询，等服务端推进，不清计数 |
| 5秒state刷新可能不停废弃长usage | anchor仅在真正取得lane启动查询时冻结；常规state更新不改当前query generation |
| null、无消费、零token有消费行混同 | 分支区分并保留行数和零桶；max=0不作除法、不添加假token |
| 公开根末段v1可能派生重复/v1/v1 | 拒绝该歧义配置，不隐式剥离合法反代前缀；原输入与规范化结果均验证UTF-8字节界限 |

原始独审先只读架构、三页计划、A1/A2契约提取十节预期，未看主设计或实现。针对性复审确认上述5个阻断和4项建议全部关闭；当前0个未解决设计阻断。详见[b设计审核](b-design-audit.md)，不能据此跳过用户确认或实施验收。

## 确认后的实施处置

用户确认 f206984 后执行同一 B 子计划；上文“未编码/未安装”仅记录设计阶段。实施证据见[实施记录](b-implementation-audit.md)，最终退出依据见[实现审核](b-audit-report.md)。没有新增产品决策或扩大到 CLI、C、SGM7。

| 实施审核发现或风险 | 处置及验证边界 |
| --- | --- |
| 原 retry 状态、冻结版本、首回执与未知操作恢复可能被客户端另造语义 | 沿原 wire 使用 retry 200、原操作 ID 与冻结 body；配置/成员/策略/运行 revision 分开；未知 target 由首份合法回执建立，回执不按 phase/墙钟排序 |
| 取消、旧 finally、隐藏与长 usage 可能错误释放槽或发布过期结果 | lane ownership 与 generation 独立；隐藏读取消但写保留安全回执；显式 usage 合并到最新 desired-read，定期 state 不令它饥饿 |
| 图表和来源状态可能把未知当零、边际拼联合、用前端重置周期 | 只用同一联合报表；独立校验总计与模型/桶/原生 token 边际；展示年龄跨周期后失效，等服务端推进；缺覆盖保持未知 |
| 第二独审发现少了来源筛选、顶部刷新、进度折叠、归档模型只读、quota 年龄和 100 项游标校验 | 全部按原设计补齐；增加普通/PBT 与四项目真实浏览器回归，未增加新产品范围 |
| 浏览器定时器接收者、迟到剪贴板、bootstrap 失败可能泄漏状态或使页面停摆 | 默认定时器用包装函数；SecretSlot 校验代次/可见性/到期，失败清理并安全只读；实际 Chromium/Firefox 交互通过 |
| Node 子进程空跑、浏览器漏项、变异没编译可能冒充通过 | Node 明确无进程隔离并比对 51 项实际 TAP；浏览器冻结 18×4/性能 1×4 清单，拒绝 skip/xfail/retry；17 个变异在私有副本实际加载/编译、精确目标失败并恢复 |
| 完整 schema 与 UI 轻量消费校验有不同覆盖范围 | 原 schema 另校验 32 份合成报文、3233 份 Rust 实测报文；1949 份实际回包再过前端消费。不声称 CI 已含本地 Python wires 门禁 |
| Node 首次图表性能超过 500ms | 复用固定字段列表、避免已验证模型标签的重复编码；保持完整数据与原预算，再加强容量 fixture 后通过 |
| 默认 clippy 通过，但严格 -D warnings 遇到既有 lint | 保留 9 处未改文件警告，不加 allow、不越界重构核心；严格模式失败如实记录，不能称其通过 |
| 默认全量受并发重编译/负载干扰 | 保留每轮原始失败；变异改到独立源码和 target；默认全量原断言、原集合复验。H1 TTL 的并发负载归因只作推断，不用定向绿代替全量退出 |

没有降低 PBT 次数、性能预算、安全校验或原测试断言。所有本次接受的范围限制均显式保留：API fixture 不等于真实 NewAPI 双协议链路；pagehide/pageshow 注入不等于 BFCache 驻留实测；实测容量耗时不等于任意设备保证；清除页面密钥不承诺擦除系统剪贴板。
