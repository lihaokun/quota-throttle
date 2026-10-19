# SGM6 B 三页管理界面细化设计

2026-10-19，设计基线b3a5fe6，实施基线f206984，分支research/source-groups-token-management。A1/A2已验收；本稿覆盖三页浏览器界面、同一listener的静态资源及必要的安全界面配置，已在设计独审和用户确认后完成实施验收，见§13。不接现行CLI/run，不部署，不修改GLM/火山策略、账本、NewAPI传输或DDL。

## 1 范围和复用

实现架构G4–G6：来源管理、组与访问Key管理、数据三个页签。沿现有HTML/CSS/原生JavaScript方式，不引入前端框架、CDN、独立前端服务、推理代理或服务端HTTP任务库。页面切换只改变单一document的视图；不承诺多个浏览器标签页共用一个客户端轮询器。

复用A1 PanelSnapshot、A2.2全部业务wire与worker入口。旧status.rs只参考布局和交互意图，不复制其innerHTML插值、本地时区归桶、历史永不变化或双setInterval逻辑；旧服务完全冻结。页面不直连NewAPI管理端，不把后端当前配置或观测当写授权。

实现文件拟为src/panel_http/ui.rs与ui/中的index.html、panel.css、app.mjs、client.mjs、core.mjs、render.mjs、charts.mjs、contract.mjs。core承载纯状态/投影，client只做有界HTTP，app拥有一个生命周期和副作用，render/charts仅消费投影。contract.mjs从已有schema机械生成枚举/版本/容量常量，不再手抄第二份枚举；生成器不进入运行服务，不造通用schema解释器。JS只校验并白名单投影实际消费字段，完整wire另以原schema交叉验证。

测试使用Node 22的内置test runner，dev-only fast-check 4.10.2及@playwright/test 1.64.1；实施纳入精确package-lock，npm ci复现。生产资源不导入npm包，不需要Node运行或打包服务；Cargo依赖保持不变。版本依据为[fast-check发行记录](https://github.com/dubzzz/fast-check/releases/tag/v4.10.2)及[Playwright发布包元数据](https://registry.npmjs.org/@playwright/test/1.64.1)。设计阶段未安装依赖或浏览器；确认后的实施验证见§13。

## 2 界面和动作边界

公共顶部为三个页签、采样/连接状态、显式刷新及折叠操作进度。来源和组选择保留在内存；URL fragment只允许sources/groups/data，不放key、配置或请求正文。

| 页面 | 首屏及详情 | 显式动作 |
| --- | --- | --- |
| 来源 | 类型/名称筛选，25条一页；GLM窗口及观测年龄；火山暂停/耗尽原因、周期计数、重置时间、额度覆盖说明；详情显示所属组和完整模型 | 新增、暂停/恢复、删除归档、模型发现/设置；火山策略与撤错 |
| 组与Key | 组列表与当前组成员，50条一页；策略首选/固定、期望与观察、同步状态；本地身份与按owner原生页分区展示 | 建组、显式核验/注册、加入来源/显式Ensure、移除成员、GLM固定/取消；生成/登记/启停/删除/复制访问key |
| 数据 | 整体或访问key、24小时/30天/单日、固定UTC偏移；输入/输出/总token、模型堆叠柱、可分页完整模型/原生token明细、账本进展 | 改筛选、按日钻取、手动刷新；查询不修改采集器 |

所有实体主键保持字符串，显示名称不用于身份认领。同源来源只一份额度/错误/用量；成员的渠道/priority按组独立。组类型必须选glm或volcano；无组删除/就地换类型、逐访问key绑定、任意priority/status编辑、火山临期或积分换算。

来源“删除”必须确认其实际含义：请求全局停用并归档，保留历史，不撤销供应商凭据。移除成员仅影响该成员，不删除共享来源。暂停恢复与耗尽原因分别展示；“恢复调度”不承诺立即可用。组selected_member_id标为“策略首选”，不是当前所有推理必经渠道；ready/backend_ready均标注采样时含义。

## 3 最小HTTP增量

### 3.1 静态资源

同一PanelServer明确白名单GET/HEAD：/、/assets/panel.css及上述固定.mjs资源。/只返回HTML，不接受任意文件路径或SPA路径回退；未知资源404，白名单资源非GET/HEAD为405 Allow: GET, HEAD，query（含空问号）400。HEAD Content-Length与GET一致且无body。

现有request::authorize已只对/api/admin/v1/要求X-QT-Panel；因此无需放宽或重写H1授权。静态导航仍经过原严格framing、固定Host、可选Origin一致性、Fetch Metadata和截止；API仍必须X-QT-Panel: 1，写请求仍必须Origin和JSON。跨站导航被拒时提示通过本地地址栏/SSH隧道访问，不增加CORS例外。

Response仅增加私有固定ContentType枚举Json/Html/Css/JavaScript；原API默认Json、错误wire/状态/Allow均不变。继续no-store、nosniff、CORP same-origin、no-referrer、DENY。HTML增加CSP：
default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'none'
不用inline script/style/事件属性、eval、远端字体或外部请求；图表采用canvas与固定CSS。静态资源编译内嵌，启动时构成Arc bytes，不逐请求读文件/查询worker；总资源<=1MiB、单文件<=512KiB，已有16连接和5/10/5/45秒预算保持。

### 3.2 界面配置

新增GET /api/admin/v1/ui-config，无query，沿原API鉴权；非GET返回405 Allow: GET，HEAD不执行业务。返回schema1、dry_run:boolean、public_api_base_url:null|绝对URL；见panel-ui-config.schema.json。是启动时安全标量，不是权限快照，不含owner是否可用、原配置、上游URL、凭据、用户余额或签发材料。

dry_run必须来自同一个worker.is_dry_run()。public_api_base_url只能由可信启动组合显式提供，不从Host/Origin或worker私有配置猜测；未提供时null，页面显示“调用地址未配置”，禁用地址复制而不是复制管理面地址。它是NewAPI根地址，不含协议的/v1后缀；UI只派生OpenAI兼容base=root+/v1、Claude ANTHROPIC_BASE_URL=root，两者使用同一访问key，不生成两把key或两组。首版仅提供接入字段说明，不承诺自动生成所有版本的OpenCode完整配置。

拟新增UiOptions::new(Option<String>)->Result<UiOptions, StartupError>，纯验证http/https、原输入和规范化结果均UTF-8<=8192 bytes、无user/password/query/fragment、原输入无前后空白，规范化尾斜杠。末个非空路径段百分号解码后为v1（ASCII大小写不敏感）则拒绝，不能生成/v1/v1或偷偷剥离其他反代前缀；首版不支持根路径末段本就名为v1的歧义配置，调用者须提供不含协议后缀的公开根。无效为新增内部StartupError::InvalidUiConfig，不回显原URL。serve_ui(self, worker, options, shutdown)沿同一supervisor；原serve保留为默认options的薄包装。ui-config启动时编码一次且<=16KiB，GET不占业务槽、不访问DB/网络。SGM7才决定CLI配置如何传options，本批不改config.toml或CLI。

## 4 字段消费和公开类型

| 消费者 | 唯一字段来源/语义 |
| --- | --- |
| 来源行与修改版本 | catalog.sources[].source及retire_requested/model_count；config_revision用于来源操作，retired历史只读 |
| GLM额度 | glm.sources按source_id连接，同时匹配source_revision；state、observed_at_ms、five_hour/weekly的percentage/reset_at_ms、level；不存在不补0 |
| GLM组提示 | glm.groups的current/evaluated_at_ms/imminent_members/pin_release，结合catalog.groups的policy_revision/pinned_member_id；不在JS重算decide/临期/pin释放 |
| 火山 | catalog.sources[].volcano.policy/runtime；runtime=null未初始化，coverage_end=null不能称完整零消费；used_input/output是本周期已观测值 |
| 组成员 | membership原视图加control_revision/status_directive/retire_requested/create_state/dirty/next_attempt_at/error_code；操作用control_revision，绝不用applied_revision |
| 操作 | OperationView wire3原kind/phase/target_type/target_id/target_revision/total_targets/confirmed_targets/error_code/updated_at |
| 本地key | TokenIdentityView wire1的token_id、native_token_id、owner_user_id/owner_scope、group_id/explicit_group、revision/create_state/deleted_at |
| 原生key | NativeTokenView原token_id是原生ID；owner_scope+owner_user_id+原生ID三维匹配才可补充本地行。未加载/失败不表示删除 |
| 统计 | UsageSnapshot原query、ledger和nullable usage；同一次响应的total/models/tokens/buckets及bucket.models，不拿边际统计拼图 |
| 运行模式/调用地址 | 仅新UiConfig；配置读取失败时保留只读页面，禁用写和复制，不从backend_ready推导dry_run或issuer能力 |

消费前检查版本、所用字段存在与类型、枚举、ID及容量。ID以长度/字典序核正十进制且<=9223372036854775807，不经Number。revision、Totals、运行计数、时间等按各原schema的JSON_SAFE域验证；原生remaining_quota允许负数，百分比允许非整数。金额/积分不换算为token。safeAdd每次检查整数及范围，任何报表守恒/溢出错误拒整份结果，不静默丢行。

A1时间为毫秒，业务/统计时间为秒。日期格式化前检查Date可表示域和秒转毫秒的安全范围；超域显示原始数值及单位，不生成Invalid Date或改写查询。查询模型使用原标签，不trim；catalog模型的trim/去重仍交后端。Source模型编辑用JSON字符串数组，避免以换行/CSV分割而改写合法模型名；只限制整请求1MiB和原元素数量，不误套统计模型256-byte限制。姓名/模型含__proto__等时用Map索引，不写普通对象原型。

新增跨Rust/浏览器共享类型仅UiConfig，归架构本节增量。其余为浏览器私有、不可成为业务权威：
- ViewState：当前页/筛选、一个已接受state、一个当前usage、当前本地身份页和每owner一个原生页、非敏感草稿、最近操作；列表分页不无限累积页面。
- RequestTicket：唯一ticketId、lane、generation、query/owner/page/entity key、sentMono、abortController；lane.owner持当前实际客户端请求的ticketId，独立于结果generation。只有匹配当前期望的generation/key可发布结果，但过期ticket仍负责释放自己占用的lane。
- SnapshotAge：captured_at_ms、headerAgeMs、sentMono/receivedMono、同一publication已判的过期标记。
- PendingIds：sessionStorage键qt.pending.v1，{schema_version:1,operation_ids:[...]}，最多16个原operation ID，无payload/凭据/模型/URL；新动作发送前持久保存。存储失败则本次持久写不发送，提供复制操作ID并说明修复存储后再试；不回退弱随机ID。
- SecretSlot：最多一份reveal值及对应本地ID/generation/30秒单调截止，独立于ViewState、日志和恢复记录。JS/系统剪贴板不承诺物理擦除。

## 5 统一刷新和显示年龄

一个document只有一个递归setTimeout pump，1秒唤醒做过期降级和due选择，不使用各页setInterval。最多同时一个state fetch和一个work-lane fetch；这里仅是客户端在途计数，不是服务端真实任务数证明。

| 任务 | 规则 |
| --- | --- |
| ui-config | 初始先读；失败按5/10/20/30秒退避，成功后不周期读，BFCache恢复重读 |
| state | 初次/恢复立即，成功后5秒；失败5/10/20/30秒退避；不重叠、不补发错过的ticks |
| 当前usage或火山详情模型统计 | 可见且当前页需要时首次/改筛选读取，之后60秒；每次只一个查询。历史也允许迟到补采，不能称永远冻结 |
| 本地身份页 | 进入Key区、切cursor、相关操作后显式读取；不后台遍历100页 |
| 原生key页 | 显式点击刷新/翻页/切owner才读；无逐key GET、无自动全量扫描 |
| 操作查询 | work空闲时轮转跟踪ID，每ID最短5秒；单次跟踪累计60秒未终结则标“仍待确认”，转手动继续查询，ID仍保留 |
| 写/核验/发现/复制 | 只有用户动作；work忙按钮禁用，保留非敏感草稿，不堆积隐式队列 |

work周期任务需让位给显式用户动作；一个pending desired-read只保留最新筛选，旧read可abort，写不因切内部页签自动abort。每次abort也增加generation；迟到结果丢弃，不因Promise结束声称后端槽已释放。queue_full/query_cancelled按原错误显示，重试读取最早5秒；没有自动写重试。HTTP超时客户端state为12秒、work为40秒（覆盖body/发送开销，不放宽服务端预算），结果解码后再次核自身deadline和ticket。

lane转移固定为：空闲时把owner设为新ticketId；abort/隐藏/换筛选仅使结果generation失效并调用AbortController，owner不清。请求链完成或抛错的finally只在lane.owner===ticketId时清owner，不要求generation仍有效；清后才可启动最新desired-read。旧finally不能清除别的owner。隐藏时的在途读沿此规则settle；在途写保留owner，若隐藏期间收到合法回执仅更新非敏感追踪，不写DOM。恢复早于旧finally时先标过期和记录待刷新，仍等owner归还；不会永久busy或并发接替同一lane。deadline通过abort中断浏览器fetch/reader，最终链settle归槽；不靠无主Promise.race提前伪造实际请求结束。

hidden/pagehide：停止pump、不发新请求，abort只读、失效所有读ticket，清全部敏感表单/reveal，旧页面标过期；已发写仍可能执行，只保留可恢复ID。pageshow/visible：先使旧显示失效，再重取ui-config/state，随后按当前页恢复due，不复用冻结前的fresh标记。页面销毁清timer/listeners/引用；pagehide不能被当作可靠最终事务通知，不发sendBeacon写操作。离线恢复同visible处理。

快照age上界=headerAge+完整RTT+接收后monotonic时长；缺/非法header、负单调差、页面恢复均失效。同captured_at_ms的重复缓存结果只能使已知age增加或维持，不以新收到时间续TTL。state任何网络/协议/HTTP非成功/超时失败立即将现有观察标过期，即使原age仅4秒；同publication随后成功也不能消除该标记，只有新publication重新验证。age>=10000ms整份观察过期；目录历史仍可只读展示但注明旧快照。

GLM源样本检查captured-observed非负、source_revision及总年龄<=30000ms；组先要求原current=true、cycle_id与整轮一致且captured-evaluated及总评估年龄合法，再检查每个“相关来源”的原样本年龄。相关集合精确沿A1.4：当前组有channel、lifecycle非retired、retired_at=null且未请求退休的成员，其来源未暂停、未退休、未请求退休，按source_id去重。只要任一相关样本已过30秒/缺失/版本不符，整组current、imminent_members、pin_release一起降级；原Unavailable的近期样本可满足年龄条件，不把查询失败推导为0或一律使组不current。非相关的暂停/未绑定/退休来源缺样本不错误影响组；空相关集合仍检查评估年龄。恰30秒包含，超过才stale；不根据新lookahead重新生成临期。

以captured_at_ms+age上界检查reset；窗口到界不再显示该窗口新鲜，原imminent成员周reset到界使组current/临期/pin_release一起降级。火山next_reset_at到界只显示“等待服务端新周期”，不客户端清零/解除耗尽。只降级原观察，不提升unknown/unavailable/stale。backend_ready始终文案“采样时就绪”，不成为前端放行依据。

新201/202回执可能先于下一次state；只显示收据与“等待状态快照更新”，不把旧快照的实体或revision改成推测新值。

## 6 表单 提交和恢复

所有写用固定route/action builder构造白名单body，schema_version=1；表单无owner user_id、priority、status、backend_ready等通用字段。必要null按A2.2显式发送。数字从严格十进制文本转safe integer，空串不是0；字符串不使用JS trim去模拟Rust的Unicode规则。默认/合法域以原schema与server校验为准。

打开编辑时冻结实体ID、相应版本和初始数据。刷新只提示“对象已有新版本”，不覆盖草稿、不自动采用新revision重发。409需用户重新载入并确认；切换实体、来源类型或owner使旧读ticket无效。没有发行人配置的布尔推断，生成是否允许最终由原callee判定。

| 动作族 | 请求与恢复 |
| --- | --- |
| 创建来源 | 显式选GLM/火山；GLM可填入config.example的现有三个URL样例，火山地址留空。默认discover，manual/fallback需JSON模型数组；policy默认null/3/60/480。整个来源配置表单视为敏感，不持久化；送出时清DOM，只在当前fetch局部持body。201显示model_origin；失败不自动重发。丢回执后用户重填凭据、显式再次提交，duplicate_source原ID用于核对，不能按名称认领或声称新配置已覆盖 |
| 创建组/成员 | 类型必选；同类型可选来源来自当前state。201仅保存。丢回执按newapi_group或source/group唯一关系核对；冲突不覆盖。planned成员拿到新state.control_revision后由用户显式Ensure，不自动串第二个写 |
| 暂停/恢复/来源归档/模型同步发现 | source config_revision；归档显示全部受影响组并确认。models读取返回的source_revision与草稿匹配后才编辑 |
| 移除/Ensure成员 | member control_revision，Ensure是原显式enabled意图，不保证当前可路由 |
| GLM固定/取消 | group revision+policy_revision，membership_id或null；只显示原pin/释放信息 |
| 火山策略/撤错 | source config_revision+runtime revision；未初始化设置用0，撤错须>=1；UI不自己清超限/暂停或推进重置 |
| 核验/注册组 | 显式POST check，register真假都属写，200 ready=false保留error；不混入自动刷新 |
| token创建/启停/删除 | 创建固定issuer，不提供owner选择，group revision；后两者用本地ID+identity revision。expires_at=null或秒，不在重放前重新套当前日期校验 |
| token登记/复制 | 登记用明确owner scope+原生ID；复制只用本地ID+revision，二者不混同 |

持久动作在客户端校验后用crypto.randomUUID生成qt_前缀ID（39字符），将ID先存sessionStorage，再发送同一不可变body。无随机能力/存储空间/16槽满则不发送，并解释原因。提交fetch后任何异常保守标为“待核对”并保留ID，不能从HTTP错误码推出所有动作都未写；自动GET同一ID，NotFound不判未执行。收到202只采用原OperationView，不把phase改成成功。

当前document可在明确人工操作下重发尚无回执的同ID同payload；绝不能更新revision/body却沿用该ID。刷新/崩溃后恢复记录只剩ID，所以只查询，不重发未知payload；提供手工输入operation ID恢复入口。sessionStorage损坏/不可用须显示恢复受限，不清其他应用存储。Succeeded/Superseded看过后可清该ID；Failed/Unknown保留到用户显式移除追踪或重试，说明移除追踪不取消后端。浏览器/标签丢失sessionStorage时不承诺自动找回所有历史操作。

GET op回执只按单work lane和ticket的因果归属接收，不将updated_at、target_revision或phase排序；updated_at秒级可相同，retry可重新进入执行phase。同一ID不并发查回/重试，失效ticket结果不发布；当前ticket返回的合法同秒新phase必须接收。

首回执核operation_id、原schema、kind→target_type关系及ID/revision合法性：普通已知目标提交还核原kind和目标ID；create_token仅知道kind=create_token/target_type=token，不拿输入group_id或group revision冒充新token target_id/revision，首次合法回执才绑定该token。sessionStorage恢复/手工输入只有opID，首次仅核上述公开关系后建立内存(kind,target_type,target_id)绑定；后续同ID不得变更该绑定，若漂移显示协议错误，不改成新操作。

Succeeded且total_targets=0的C2设置只显示“设置已保存”；带targets按原confirmed数量和phase显示；Unknown/Partial/Waiting不写“已生效”。显式POST retry仍用原ID，不是新操作；retry丢响应继续GET原ID。分页和state不含完整操作列表，不能伪造历史全景。

## 7 key列表和敏感复制

本地页after与原生owner/page各有独立ticket、时间和错误。每owner最多保留当前100项，不因空页/失败把历史身份删除。交集匹配(owner_scope,owner_user_id,native_id)，不按名称或masked_key；未匹配显示“该原生页未加载/未登记/绑定需核验”等原事实，不合成为原子状态。原生分页没有观察时间：仅标“本页收到于…”，不用created_at冒充。

数据页选本地key时，只有native_token_id非null且有效才生成Exact原生筛选；pending身份禁止查询而非回退Any。历史已删除身份仍可查询原生ID的统计。本地页未加载的key可按原生ID显式输入查询，并标明是原生ID，不自动拉完所有页。

reveal只由点击发起；若work忙、dry_run、版本缺失则不发。先清旧SecretSlot，绑定token/revision/generation。成功响应必须同本地ID、固定schema和合法key；页面/窗口已隐藏、切换、已过40秒请求截止则丢弃，不写DOM或剪贴板。

收到有效key后尝试Clipboard.writeText；只有Promise成功才显示“已复制”。不支持/拒绝时仅使用本次key展示30秒只读手动复制框和“复制已取得的key”按钮，不再发reveal。第二次点击可提供新的用户手势；没有自动网络重试。成功、30秒截止、关闭框、换页/换key、hidden/pagehide均清节点和引用；迟到Promise不能复活已清slot。不能取消已经发生的系统剪贴板写入，也不承诺擦除JS字符串或系统剪贴板历史。

## 8 数据和分模型柱形图

区间在真正取得work lane、即将启动查询时，以最新且未过期state.captured_at_ms的整秒冻结滚动截止to，并标“截至状态采样时间”，不用浏览器时钟猜服务端当前时间。后续5秒state刷新不改该查询key/generation，也不废弃仍在途的查询；只有用户改变筛选/页签、生命周期失效或相关火山period变化才废弃结果。60秒刷新到期时再取新的anchor。默认固定UTC+08:00，可选-12:00..+14:00的整数分钟偏移；这是图表口径，不改变来源重置时区。

- 近24小时：[to-86400,to)，hour；源epoch极早时from夹到0。
- 近30天：该固定偏移下今天起点减29天至to，day；不足30天的epoch域from夹0。
- 日钻取：[选中日边界, 次日边界)，过去日完整24小时，今天to=min(次日,to)，未来/空区间不查询；所有边界为秒及半开区间，最长31天。
- 火山详情固定grain=hour，按runtime.period_start至min(next_reset_at,to)读取source Exact+其余Any，只在该详情展开时占用同一usage通道。runtime缺失、to<=period_start或已到next_reset_at时不发非法/伪新周期查询，显示未初始化/等待服务端新周期；旧周期已有报表只能明确标为旧周期。查询key包含source revision、period_id和完整区间；结果不能改A1运行计数。自然日图与来源周期分开，不要求采集不同瞬间的二者相等。

每次查询冻结key={from,to,offset,grain,四维filter}及generation；返回query须精确等于发送值，ledger/usage作为同一个整体校验接收。null表示未初始化；失败/超限保留“旧查询结果”且不能挂在新筛选标题下。query变化清当前可视报表，等待新结果；同key刷新失败可灰显旧报表及读取时间。scanned不是供应商完整保证，不新增complete=true。

prepareChart校验全报表safe totals及models/tokens/buckets、各桶models的守恒；null model有独立身份，不与字符串“unknown/null/未归属”合并。颜色由带类型的模型key确定，筛选和刷新不重排颜色。保留所有模型/零桶，图表以bucket.models堆叠total token；输入/输出在数值与tooltip列明，不将金额或quota当token。legend/明细分页50项，分页只限DOM，不裁掉图表数据；模型筛选整体→Any、null身份→Unknown、字符串身份→Exact原字符串，均发新的相应查询，不能隐藏部分堆叠却保留误导总量。

usage=null显示“账本尚未初始化”，不绘制零统计；已初始化且无消费返回原零桶/空模型，显示“此区间暂无已观测消费”并保留进展/缺口。已初始化有消费行但token总量全0时，精确显示consumption_rows及所有模型/桶，不称无请求；纵轴最大值为0则所有柱高为0、刻度为0，不做除零或添加假token。仅绘图比例为浮点，原数值和safeAdd仍保持整数。

Canvas显示所有桶，CSS宽<=4096、逻辑高320、DPR截到[1,2]；大数据只稀疏画轴标签，不截桶或Top-N。tooltip及明细用textContent和原精确整数；图下有按桶/模型的可键盘读取表格。颜色可能重复，名称/数值仍是身份；不承诺1024模型都有可区分色。

## 9 函数级细化和正确性论证

### U1 资源和配置
UiOptions::new按§3验证规范化；UiConfig::from_worker仅取dry_run及已验证公开base，失败在serve启动前固定错误返回。try_ui_response(head, bundle)->Option<Response>只匹配静态表和ui-config，方法/query失败固定400/405；无匹配None交原A2，返回路径也遵守H1绝对deadline。serve_ui仍使用原supervisor、同一worker与关闭barrier；respond仅按固定MIME输出，API Json分支保持。有限白名单和有界编码终止；没有DB/网络写，且不改变API鉴权，所以静态可达不意味着业务可越权。

### U2 请求和消费
requestJson(route, body, ticket)->Result<validated projection, ClientError>检查同源白名单route、JSON序列化UTF-8<=1MiB、generation/deadline，fetch固定X-QT-Panel、JSON写头、cache:no-store、credentials:omit、redirect:error；Origin/Host由浏览器设置。流式读最多32MiB后停止，UTF-8/JSON失败固定协议错误，不回显原body；对成功/两类错误schema按预期family投影，未知错误码为通用“不兼容响应”。收到/解码后再核ticket和deadline；任何迟到结果只丢弃。循环每次消费chunk且有限字节/截止；错误不生成默认空数组或零值。消费字段声明机械对照schema，非消费字段不进入ViewState。

### U3 生命周期
start/stop/switchPage/onVisibility/onPageShow及pump(now)->effects由一个App拥有timer和两个lane。start幂等；每次pump最多启动两个有限操作且先检查可见/截止；finally按owner ticketId归槽而不是按generation，过期请求也归还自己的槽，不能释放另一个owner。切换只替换desired读、使旧结果generation失效，不清仍在途的owner；stop递增epoch并清timer/listeners/secret，owner由settle释放。只有pump能安排下一次定时，所以重复事件不创建第二轮询；每次有限扫描<=16个op和固定任务类终止。网络不能证明服务端取消，故保留A2背压/未知回执语义。

### U4 展示年龄和投影
acceptState(previous, raw, ageHeader, sent, received)验证A1字段/关系并建立§5年龄锚；同采样沿更保守age及已失效标记，失败立即降级。deriveView(snapshot, now, selection)用Map连接有限目录，按类型、版本、源/评估/精确相关来源集合的年龄及reset边界只降级；相关样本先过期必传播到组标签，非相关缺样本不能误传播。复杂度O(sources+groups+memberships)，不调用策略或探针。renderFrame仅分页选择后创建DOM，所有动态值作为文本/表单value，class和属性名来自固定枚举，不用innerHTML、outerHTML、insertAdjacentHTML、document.write、内联事件或字符串定时器。DOM分页和Map键防止输入成为代码/原型；局部重绘不重置焦点或编辑草稿。

### U5 操作恢复
buildAction(action,draft,current)->body按§6路由矩阵、类型关联和冻结版本构造；缺版本/跨类型/已归档拒绝，server仍是最终验证者。submitPersistent先检查lane、UUID及sessionStorage原子更新，再发一次不可变请求；失败保持跟踪并转查询，不自动换ID。pollOperation按原op、当前ticket及首回执/已绑定两路校验展示，失败保持已知phase不推成功，不按秒级updated_at/phase猜全序；retry只显式发原ID。restorePending只解码16个合法ID，不接受payload；任何未知存储版本/损坏不执行写。有限ID集合及一次调用一次请求保证终止和边界，稳定ID与原持久摘要共同防逻辑重复；不承诺浏览器崩溃后能恢复未持久化的目录请求。

### U6 来源和组
openSource/openGroup/openModels建立草稿及版本快照，当前state变化只显示冲突。createSource读取DOM到局部body、提交即清敏感字段；模型模式由用户明确选择、结果原model_origin展示。createGroup/planMembership只处理原201，不自动Ensure；ensure/retire/pin/policy/check均经U5或其固定非operation分支。版本关联来自§4且API是唯一写者，故UI不绕源级暂停/组类型。丢响应恢复按§6唯一关系，查不到不证明未写；各有限列表遍历终止。

### U7 身份和分页
loadIdentities(after)/loadNative(scope,page)经work单槽取一页；validatePage核query归属、游标推进和容量，mergeIdentity用三维Map，只对当前已加载页补充原生观察。成功不删除页面外身份，失败保留独立错误，最多一个本地页与两个原生页。buildUsageFilter(identity)仅返回Any（明确整体选择）或Exact(native_id)，pending返回错误；所以既不误删也不把本地ID误用到统计。生成/登记/启停/删除固定对应A2.2，不出现owner回退。

### U8 查询和图表
makeUsageQuery(mode,anchor,offset,filter)取得lane时冻结anchor，按§8整数日期边界及Any/Unknown/Exact三态构造，区间合法才发；UTC偏移不依赖浏览器本地Date.setHours。acceptUsage核generation/query与全部有界安全数字；prepareChart以同报表的联合项校验，并分未初始化、空统计、全零token和正值四路映射，不另查SQL或拼边际；max=0不作除法。有限<=745桶、1024模型、4096原生ID、65536桶模型项，safeAdd单调累加且不超域；错误整份拒绝。drawChart只用已验证几何和静态颜色，表格保留精确数值及unknown身份；分页不改变守恒。

### U9 复制
revealAndCopy(identity,generation)前置显式点击/有效版本/非dry-run/当前lane空闲；错误固定提示、无自动重取。成功仅通过SecretSlot，任一代次/可见性/时限失配直接丢弃；writeClipboard成功后清slot，失败才提供同一key的短时手动框。clearSecret是幂等清引用/DOM，不写local/sessionStorage或日志，不声称清系统剪贴板。有限请求和30秒截止保证本模块持有时间有界，生命周期事件只可缩短，迟到Promise不能重建slot。

### U10 启动与能力失败
bootstrap只安装一次事件监听，初始加载ui-config/state；缺fetch/AbortController/ES modules时HTML给静态不兼容说明，不降级到旧API。无crypto随机能力只禁持久写，无Clipboard提供显式手动复制；存储不可用只禁不能可靠跟踪的持久新动作。任何初始化失败保持安全只读错误屏和手动重载入口。已执行动作不因bootstrap失败推定撤销；无新的后台守护进程或隐式重试队列。

## 10 验证计划和实施顺序

设计确认后依次实现U1静态/config与U2–U4纯模型刷新，U5–U7管理表单/key，U8–U9图表和复制，最后U10/浏览器组合；是同一个B子计划的实现顺序，不新增产品阶段或重复确认同一已批准细化。每组按workflow先测试/审核再继续。C另验证真实worker→隔离NewAPI→双协议→统计全链路，不能以B的浏览器mock当C通过。

PBT至少8组各128 cases：年龄只降级/边界、代次乱序、单pump/lane轨迹、op ID/载荷/phase恢复、目录唯一性与类型版本、owner分页缺失与ID隔离、模型/桶/原生token守恒与颜色、恶意Unicode/文本和敏感生命周期。oracle用独立时钟/Map/整数模型；不以被测投影作期望。Node test及fast-check输出seed/shrink路径；失败保留回归。

固定反例必须包括：null模型与字面unknown同时存在并分别查询；abort后旧finally/恢复先于settle/在途写隐藏；评估1秒旧但相关样本29.5秒旧再经过1秒、以及暂停/无channel来源缺样本；age4秒时刷新503立即过期且同publication成功不复活；同秒phase改变及retry回执行态、仅opID恢复首回执、create_token返回目标ID不同于输入groupID；耗时超过5秒的usage不被每轮state刷新饿死；已初始化0token但consumption_rows>0；火山period边界/空区间；公开根末段v1拒绝。

Rust HTTP组合保留原80/4清单全部断言，新增静态GET/HEAD、MIME/CSP、错误Host/Origin、资源白名单/query、API缺头仍403、ui-config默认/显式地址/dry_run和关闭测试。旧A2.2 schema/业务route不变；新增配置schema正负例、API响应消费字段/枚举生成校验，源树冻结检查不以只保留测试名冒充旧回归保留。

真实浏览器首轮Chromium+Firefox，固定1280x900及390x844视口：真实导航与脚本加载、跨源攻击、DOM注入、页签与筛选乱序、hide/show、pagehide/pageshow或BFCache恢复、重复点击/重载后同ID查回、分页失败/未登记、复制成功/权限拒绝/迟到清理。测试中可用Playwright拦截明确的API fixture模拟乱序，但必须单独标注；静态资源/CSP至少通过实际Rust listener测试。不得把字符串快照或仅mock函数当真实浏览器证据。浏览器或测试依赖缺失时门禁失败，不silent skip；下载/安装只进任务隔离缓存，不启动正式服务。

新增独立性能：Node最大已接受PanelSnapshot解析+投影<=1秒；65536桶模型项图表数据准备<=500ms；固定浏览器最大图完整绘制<=2秒；100行DOM页渲染<=500ms。记录CPU/Node/浏览器版本、输入形状和字节数，普通套件无墙钟断言，保留原31release+4debug预算。不把单个fixture说成任意标签/所有设备的耗时证明。

至少12种实际可执行变异：静态绕API头、JSON当HTML、unknown填0、重复pump、丢generation、隐藏后复活、改revision自动重发、retry新op、原生缺页删身份、本地ID查usage、模型Top-N/颜色漂移、secret进storage/迟到DOM、缺事后deadline（可超过12）。Rust变异须真实编译、JS变异须语法/模块加载成功；精确单失败且核对目标断言，恢复后重编/重载并复验，环境错误和空跑不算捕获。

最终运行Rust默认debug/release、Node普通/PBT、两个浏览器、全部原/新增独立性能、格式/lint/build、机械清单及独审五维度；各套件精确非空清单在实施首批冻结并防ignore增减。CI新增独立UI测试job（锁定Node22/npm ci/浏览器），不删除原Rust门禁；CI资源/总timeout不足另如实说明，不缩案例数。

## 11 分布式契约和假设

连接仍同源单HTTP请求，浏览器不接推理流；失败/断开不抹操作。server 5/10/5/45秒、4业务槽、1usage槽保持；浏览器12/40秒是独立等待界限。读退避有限且合并，写自动重试为0；跨请求不承诺FIFO/exactly-once。sessionStorage只恢复非敏感ID，不是服务端状态库；隐藏/恢复事件可重复，生命周期幂等。客户端双lane不证明服务端只有两项实际工作，背压依A2真实许可。

依赖固定同源回环/SSH隧道、支持的浏览器API、原wire schema有效及单worker生命周期。多标签全局选主、离线写队列、PWA/Service Worker、公网登录和多用户权限不在范围。正式fermat部署仍是SGM7全新初始化，本轮不导入或切换线上数据。

平台依据：[Fetch禁止网页任意设置Host/Origin](https://fetch.spec.whatwg.org/#forbidden-request-header)、[visibilitychange](https://developer.mozilla.org/en-US/docs/Web/API/Document/visibilitychange_event)、[pageshow](https://developer.mozilla.org/en-US/docs/Web/API/Window/pageshow_event)、[Clipboard.writeText的权限和Promise](https://developer.mozilla.org/en-US/docs/Web/API/Clipboard/writeText)、[randomUUID](https://developer.mozilla.org/en-US/docs/Web/API/Crypto/randomUUID)、[textContent](https://developer.mozilla.org/en-US/docs/Web/API/Node/textContent)。这些说明平台边界，不代替本批真实浏览器测试。

## 12 设计退出

- [x] 范围、现有接口和字段来源已核对。
- [x] 私有状态、错误/取消/副作用、逐函数callee与有限循环已列。
- [x] 独立§4.4审核0个未关闭设计阻断；UiConfig metaschema、7正例/23负例通过，见[设计审核](../../../audits/sgm-6-three-page-panel/b-design-audit.md)。
- [x] 用户于2026-10-19确认f206984后开始实现；不改运行配置或正在服务的实例。

实施及逐模块审核记录见[B实施审核](../../../audits/sgm-6-three-page-panel/b-implementation-audit.md)。设计退出不等于实施退出；最终回归、浏览器、性能和变异证据分别记录。

## 13 实施退出

2026-10-19，U1–U10 已完成，默认 Rust debug/release 各 744 passed、0 failed、35 ignored；Node 51 项及八组各 128 PBT、Chromium/Firefox 四项目 72 项交互、全部原/新增性能和 17 个有效变异通过。机械 schema/字段/枚举/清单、核心及旧断言冻结、格式/default clippy/build 和最终源码 SHA 核验通过。两位独审分组/交叉复核，最终 critical 0、unresolved 0，workflow 五维度通过，见[实现审核](../../../audits/sgm-6-three-page-panel/b-audit-report.md)。

依赖与浏览器已仅为实施测试安装，生产仍无 Node/框架/CDN。上述是 B 内部验收，不代表 C 的真实 NewAPI 双协议/统计全链路或 SGM7 运行部署已完成；本地现网保持不变。
