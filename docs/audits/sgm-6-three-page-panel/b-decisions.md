# SGM6 B 设计审核处置

2026-10-19，设计基线b3a5fe6。本轮只细化和审核，不编码、安装依赖、上传源码或部署。用户已同意后续按需向fermat隔离临时目录上传测试必要源码，排除真实配置、密钥及数据库；本轮无需使用该授权。

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
