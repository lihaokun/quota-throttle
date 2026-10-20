# NewAPI 原生恢复与人工暂停竞争实测

日期：2026-10-15。关联子计划：sgm-2-channel-reconcile。

结论：当前固定版本存在“测试先捕获自动禁用状态、随后人工暂停、测试成功后又启用”的竞争。只把该渠道 auto_ban 改成 0 不能解决；关闭全局原生自动启用的对照场景保持暂停。这支持 sgm-2 的只读部署预检，不授权修改生产 option。

## 复现条件

使用已有 new-api-v1.0.0-rc.20 二进制，只读执行，不重新构建。SHA256：90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205。

每个场景启动一套全新的临时 SQLite、随机端口、临时管理员/访问 token 和回环假上游。没有读取生产 config.toml、数据库、日志或 key，没有请求真实模型供应商，端口不是 3000/3001。脚本在首次写入前用 ss 核对监听 PID，并确认独立数据库存在。

先让假上游返回一次 401，使原生渠道进入 status=3。然后让后台批量测试进入假上游，停在成功响应之前；此时通过管理 API 写入并回读 status=2，再放行测试成功结果。最后观察实际状态和 test_time 推进，不用固定 sleep 猜测试是否完成恢复判断。

批量测试由管理 API 显式触发，执行原生后台测试/恢复路径；不是声称生产的定时测试已开启或已发生过这一竞争。

## 三个对照场景

| 原生自动启用 | 恢复阶段 auto_ban | 人工暂停已回读 | 测试成功后的状态 | 结果 |
| --- | --- | --- | --- | --- |
| true | 1 | 2 | 1 | 暂停被原生恢复覆盖 |
| false | 1 | 2 | 2 | 保持暂停 |
| true | 0 | 2 | 1 | 关闭渠道自动禁用仍不能阻止恢复 |

三个场景均符合断言，全部自建 NewAPI 进程及假上游服务器已停止。测试目录保留的是假数据；没有停止或重启生产进程。

## 源码对照

- 原生状态更新没有“当前是手动禁用则拒绝自动状态覆盖”的 CAS 条件：[model/channel.go](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/model/channel.go)。
- 恢复判断依赖全局 AutomaticEnableChannelEnabled、测试成功及此前取出的 status=3，不要求 auto_ban=1：[service/channel.go](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/service/channel.go)。
- 批量测试按先前渠道对象判断是否恢复，恢复判断之后才更新测试时间：[controller/channel-test.go](https://github.com/QuantumNous/new-api/blob/6ce7305cd36f16506fb6a2c3c524a5a318539ba7/controller/channel-test.go)。

## 复现入口与边界

[隔离探针](../../scripts/probes/newapi_pause_recovery_probe.py)：

```bash
python3 scripts/probes/newapi_pause_recovery_probe.py --binary /absolute/path/to/new-api-v1.0.0-rc.20
```

脚本只接受二进制路径，不接受生产实例地址或凭据；复用已验证的管理 API 测试客户端。退出输出只含状态、假实例位置/进程/任务标识和二进制摘要，不输出生成的密码或 key。

这三个场景证明了该固定版本中的具体竞争及一个有效前提，不是穷尽所有远端并发写入。sgm-2 仍必须对自己的过期请求/回执、未知创建、部分成功和重启恢复分别测试；不能因为关闭原生自动启用就移除这些防护。

生产预检如发现 AutomaticEnableChannelEnabled=true，只报告冲突、不自动关闭它；GLM 的正常 auto_ban、95%/restore/pin/临期规则保持原样。若需要改变实际配置，仍须发布阶段明确确认。
