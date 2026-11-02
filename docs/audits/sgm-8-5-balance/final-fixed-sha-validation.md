# SGM-8 第5批 B3–B8 最终固定SHA独立C2验证

结论：本次承担的固定SHA机械/功能/Rust性能门禁全部通过，未发现未关闭问题。核验对象为 `ef44c070bd967cfec602d8cbc4e11c08585ebbbc`；独立代码契约审查与F1关闭结论由仓库 `docs/audits/sgm-8-5-balance/final-contract-review.md` 提供，其结束清单中的全部src/tests/scripts/Cargo/schema字节与本提交一致。不能把本报告写成已部署或远端CI已通过；主任务另负责UI两项性能与最终文档汇总。

- 开始：2026-11-02T20:55:48.134146+00:00；结束：2026-11-02T21:11:26.925675+00:00。
- 工作树：`/home/haokun/projects/quota-throttle/.worktrees/source-groups-research`。
- 开始/结束HEAD均为 `ef44c070bd967cfec602d8cbc4e11c08585ebbbc`。
- 开始/结束只有审核文档 `docs/audits/sgm-8-5-balance/b3-b8-implementation-review.md` 已修改；本审未修改/提交/推送仓库文件。
- 冻结275个src、Cargo、schema、测试、脚本、UI清单及CI输入：逐文件hash前后相同，且与HEAD匹配；聚合SHA256 `cabae87ddbe2092e33314808dc5bb05b54ec6fa9c55438a1397f98a12b191718`。
- 输入清单：`/tmp/sgm8-final-c2-input-start.json`、`/tmp/sgm8-final-c2-input-end.json`。环境：`/tmp/sgm8-final-c2-environment.json`；命令与结果索引：`/tmp/sgm8-final-c2-commands.json`。

## 实际执行结果

| 门禁 | 本审实际结果 | 原始日志 |
|---|---|---|
| `cargo test --release`，默认并发 | **831 passed / 0 failed / 35 ignored / 0 filtered**；595.92s，release编译另2m17s；含两处最终harness修正 | `/tmp/sgm8-final-c2-rust.log` |
| `cargo build --release --locked` | 成功，20.10s | `/tmp/sgm8-final-c2-build.log` |
| Node全量 | **68/68**，0失败/跳过 | `/tmp/sgm8-final-c2-node.log` |
| `check:ui` | 精确68名称、U1–U10、8既有总体PBT组、DOM sink检查通过 | `/tmp/sgm8-final-c2-check-ui-unrestricted.log` |
| generator `--check` | 28枚举/51字段族与schema一致 | `/tmp/sgm8-final-c2-generator.log` |
| 实际wire重验 | **3386** Rust报文（20请求族/15回复族）；JS消费**2050**回复；UI43正例/25族/76余额反例 | `/tmp/sgm8-final-c2-wires.log` |
| SGM7 | 精确28个可运行名称，全部在本次full中各通过一次；R1–R5/共享类型预算检查通过 | `/tmp/sgm8-final-c2-sgm7.log`、`/tmp/sgm8-final-c2-rust-result.json` |
| 格式/差异 | 23个本提交改动Rust文件：rustfmt `--check --edition 2021 --config skip_children=true`通过；`git diff --check HEAD^`通过 | `/tmp/sgm8-final-c2-rustfmt.log`、`/tmp/sgm8-final-c2-diffcheck.log` |
| 探针无网络回归 | 新增余额探针2/2、组合探针11/11 | `/tmp/sgm8-final-c2-probe-unit.log` |
| 性能runner回归 | **24/24** | `/tmp/sgm8-final-c2-performance-runner.log` |
| CI指定Rust性能 | **31/31 release + 4/4 debug**，0失败；按原脚本原阈值串行，release段34.37s；不与full或UI性能并发 | `/tmp/sgm8-final-c2-performance.log` |

普通full的35项ignored没有冒称执行：其中31项性能现由独立性能入口另行执行。其余手动/fixture测试仍以各自既有或独立证据记账。当前本地release全量不冒称远端CI的debug全量已跑。

## 机械契约与范围

已按workflow §5.2/§6.5以及细化§4–9核对；用户取消中间模块停点的最新授权优先，部署前确认保留。

1. 三份schema均通过Draft 2020-12结构校验；snapshot `$id`/title/schema_version均为2，唯一新增根为`balance`，其余旧根属性及三份schema既有defs保持原样。九个新增严格对象的字段集合=required，`additionalProperties=false`；七个跨层字段族/三枚举由生成器导出，原公开错误集合仍28。
2. `BalancePolicy/Draft/BALANCE_CAP`、`QuotaUnit`、`TokenOwnerScope`和既有错误类型复用；周期60s、读20s、auto60s、年龄180000ms、封锁/退避共用600000ms正确。六项任务均在原JoinSet；B3七步及1b、观测/取消/策略/快照注释齐全。
3. Rust余额View/回复不省略nullable字段，内部ID显式`to_string()`。实际捕获的5组BalanceRefilled和1组BalancePolicySaved均有对应同pid/序号/请求族原request文件及sidecar；0缺失，不从回复反造请求；每份回复绑定真实原请求scope/amount或三设置/revision。实际refill样本4个after整数、1个after=null，user_id均字符串。
4. 对基线`7e5cf97`核零修改：Cargo依赖、现有配置类型、orchestrator/quota、source_policy、planner、worker policy、GLM round/publication、volcano publication、panel.css、初始化器。没有GLM/火山策略或供应商额度变化。
5. 当前产品Rust/探针与独立代码审查开始清单也相同；取消/截止/PBT的最终测试字节由本次full覆盖。没有以作者此前831通过代替本轮实际执行。

明细：`/tmp/sgm8-final-c2-mechanical.py`、`.json`、`.log`。

## 只读复核的已有证据

这些不是本审重新运行的测试，保留证据时点，不把旧组合测试称为F1修后浏览器门禁。

| 证据 | 复核结果 | 来源 |
|---|---|---|
| F1修后full浏览器 | **112/112**，28名称×4项目/viewport精确匹配；0失败/跳过/重试；fixture正常退出 | `/tmp/sgm8-balance-browser-final.log`；`/tmp/quota-throttle-sgm6-b-browser-ZPlYy1/report.json` |
| 最后完整JS变异 | **25/25**；每项baseline1/0、模块加载/语法成功、精确目标1项语义失败、恢复1/0；包括F1原金额回环变异 | `/tmp/sgm8-balance-js-mutations-unrestricted.log`；`/tmp/quota-throttle-sgm6-b-mutations-L4jXMQ` |
| 变异输入一致性 | **282个公开输入hash全部与本提交相同**；独立代码审查结束src/tests/scripts/Cargo/schema清单也零差异 | 上述目录`snapshot.sha256`；`/tmp/sgm8-balance-final-contract-input-end.json` |
| 手动原生add语义 | **10/10**，success=true，临时进程停止/私有材料移除；固定原生binary SHA256 `90c333eb7ace489d46e25cb687767c25c3e28584ce80384f111c03ff5f5b1205` | `/tmp/sgm8-native-balance-probe-fixed-final.log` |
| 真实原生组合 | **16/16 + 4/4**真实浏览器，apiMocks=false；首轮内部余额补足检查存在，控制器/Native都清理 | `/tmp/sgm8-balance-combination.log` |

原始报告/日志、3392个Rust wire及sidecar文件、125个变异日志的SHA256已封存于`/tmp/sgm8-final-c2-evidence.json`。已有Native/112浏览器没有重复启动。本审不访问真实config、运行数据库、密钥或本机/ker79已有服务。

## 最终可交付程序

- 路径：`/home/haokun/projects/quota-throttle/.worktrees/source-groups-research/target/release/quota-throttle`。
- SHA256：`7aeba806f454b56334d4ef1829b10c3b5d7821a83adb5be4a0805408a8685fde`。
- 构建命令：`cargo build --release --locked`；构建来源固定为上述SHA。
- 校验记录：`/tmp/sgm8-final-c2-binary.sha256`。

未部署、未替换本机或远端服务；部署许可仍需主任务保留的明确确认。

## 执行差异与证据边界

- `check:ui`首次sandbox执行的子Node进程返回0但输出无TAP名称，导致精确名称断言失败；日志`/tmp/sgm8-final-c2-check-ui.log`保留。相同源码、相同命令在与既有门禁一致的unrestricted环境通过，未修改脚本或绕过名称检查。
- 本审辅助机械脚本首轮只认可`ERR_ASSERTION`，遗漏fast-check有效的`Property failed`语义失败格式；读取原始反例后修正仅/tmp核验器，复核25项全部有效，首轮日志`/tmp/sgm8-final-c2-mechanical-first.log`保留。仓库产品/测试/契约未变。
- 本报告覆盖本审实际执行及明确标注的既有证据复核；UI性能由主任务在Rust性能结束后独立运行。没有把未执行项写成通过，也不提供部署授权。
