# SGM6 B 设计审核

2026-10-19，基线b3a5fe6。三页管理界面的函数细化通过workflow §4.4设计审核，0个未关闭设计阻断。此结论不是实现完成；用户确认后才开始编码。

## 产物

- [B细化](../../design/source-groups-token-management/subplans/sgm-6-browser-panel-b.md)：三页范围、静态资源与安全配置、字段来源、刷新/失效、操作恢复、key/复制、统计及U1–U10函数论证。
- [十节事前预期](b-expectations.md)：来自已确认架构与A1/A2契约，未从B实现回填。
- [UiConfig schema](../../design/source-groups-token-management/schemas/panel-ui-config.schema.json)：固定schema1/dry_run/public_api_base_url，拒额外字段。
- [处置记录](b-decisions.md)：初审5阻断和4建议的逐项关闭依据。

## 设计复核

独审者sgm6_b_design_audit先独立读取指定契约，再审拟稿；没有编码或操作服务。初审指出模型null筛选、lane owner、相关GLM样本过期、state失败失效和操作回执顺序/首绑定五项阻断，随后针对性复审确认全部关闭。

§4.4对应：G4–G6目标与原模块复用明确；私有状态与唯一新增共享UiConfig归属明确；U1–U10各有前置、成功/失败/取消、副作用和有限循环论证；跨模块ID/版本/owner与时间沿原wire；HTML导航不放宽H1业务API；三页生命周期、恢复和背压有明确边界。无新调度器、任务库、推理代理或公网认证扩展。

## 静态检查

临时目录 /tmp/quota-throttle-sgm6-b-design.Bv5OeX 保存check.py。主执行者和独审分别执行，UiConfig metaschema、7正例/23负例全部通过，包括缺字段、额外敏感字段、错版本/类型、userinfo/query/fragment、协议后缀v1及超长等结构场景。schema不替代Rust完整URL/UTF-8字节和规范化检查，后者列为实施测试。

git diff --check通过；src、Cargo.toml/lock、config.example.toml、.github和scripts相对b3a5fe6零改动。未运行或声称通过B的PBT、性能、浏览器或端到端测试，也没有把旧A2.2的739测试数字算成本批成绩。

## 实施门禁与下一步

确认后按同一B子计划依次实现基础资源/模型/刷新、管理与key、图表/复制和浏览器组合。至少8组各128 cases PBT、12类有效变异、精确非空清单、Rust双profile、Node、Chromium/Firefox、原31release+4debug及新增性能、lint/build和最终独审才算实现完成。C负责真实worker/NewAPI双协议全链路，SGM7负责fermat全新部署；现行本地服务未变更。
