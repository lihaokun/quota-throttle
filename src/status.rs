//! 状态查询接口 + HTML 看板。
//!
//! orchestrator 每轮 tick 末尾把「每把 key 的 5h/周 用量、档位、priority、当前活动 key、
//! new-api 健康」整体写入共享快照；本模块把它暴露为：
//!   GET /api/status  → JSON（opencode 插件等外部消费者也读这个）
//!   GET /            → 自包含 HTML 看板（内联 CSS/JS，无外部 CDN）
//!
//! 设计原则：**看板是附属，绝不拖垮主循环**——bind 失败只降级记 error，切换循环照常跑。

use serde::Serialize;
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

#[derive(Debug, Clone, Serialize, Default)]
pub struct KeyStatus {
    pub name: String,
    pub channel_id: i64,
    /// 5 小时窗口已用%；None = 本轮未取到
    pub five_hour_pct: Option<f64>,
    /// 每周窗口已用%
    pub weekly_pct: Option<f64>,
    /// 窗口重置时间（epoch ms）
    pub five_hour_reset: Option<i64>,
    pub weekly_reset: Option<i64>,
    /// 监控窗口取最大（决策依据）；None = 查询失败（**不是 0%**）
    pub max_pct: Option<f64>,
    /// "active" | "standby" | "exhausted" | "unknown"
    pub tier: String,
    /// 本工具最近成功下发的 priority
    pub priority: Option<i64>,
    /// 查询失败原因
    pub error: Option<String>,
}

/// new-api 侧的渠道实况。补的是我们看不见的盲区：
/// 本工具只改 priority、**从不碰 status**——渠道一旦被 new-api 自动禁用，
/// priority=100 也不会有流量，而我们毫无察觉。
#[derive(Debug, Clone, Serialize, Default)]
pub struct ChannelState {
    pub id: i64,
    pub name: String,
    /// status == 1
    pub enabled: bool,
    /// 原始 status（1=启用；2=手动禁用；3=自动禁用 …）
    pub status_raw: i64,
    /// new-api 中的**权威** priority，用于和本工具下发值对账
    pub priority: Option<i64>,
    pub weight: Option<i64>,
    /// new-api 内部虚构计费额度（按「按量付费倍率」记账）。**对包月编码套餐无意义**，
    /// 故看板不展示；保留在 JSON 里仅供参考。
    /// 「这把 key 消耗了多少」看 key 卡片的智谱 5h/周 百分比（权威）；
    /// 「流量怎么分布」看请求流水表。new-api 不提供按渠道的 token 汇总。
    pub used_quota: i64,
    pub auto_ban: Option<i64>,
    pub models: String,
    pub group: String,
}

/// 一条真实请求（仅用于推导「最后一次请求」，不进快照）。
#[derive(Debug, Clone, Serialize, Default)]
pub struct RequestLog {
    /// epoch **秒**（注意与 snapshot.updated_at 的毫秒不同单位）
    pub created_at: i64,
    /// 实际服务的渠道 id
    pub channel: i64,
    pub channel_name: String,
    pub model_name: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub quota: i64,
    /// 耗时（秒）
    pub use_time: i64,
    pub is_stream: bool,
    /// 哪个调用令牌发起的（如 "opencode"）
    pub token_name: String,
}

/// 某渠道的实时指标 —— 回答「有没有连上 / 烧多快」。
///
/// ⚠️ **必须独立于 KeyStatus**：决策循环每轮整体重建 `keys`，若把这些字段塞进 KeyStatus
/// 会被冲掉。两个循环写的字段必须严格不相交。
#[derive(Debug, Clone, Serialize, Default)]
pub struct LiveMetric {
    pub channel_id: i64,
    /// 最近 **60 秒**该渠道的请求数（窗口由 new-api 服务端固定）。> 0 ⟺ 流量正在走这把 key
    pub rpm: i64,
    /// 最近 60 秒该渠道的 tokens
    pub tpm: i64,
    /// 该渠道最后一次请求的时间（epoch 秒）
    pub last_request_at: Option<i64>,
    pub last_request_model: Option<String>,
}

/// 时序点（new-api 按小时聚合好的 token 用量）
#[derive(Debug, Clone, Serialize, Default)]
pub struct UsagePoint {
    /// 小时桶起点（epoch 秒）
    pub hour: i64,
    pub tokens: i64,
    pub count: i64,
}

/// 按模型的用量
#[derive(Debug, Clone, Serialize, Default)]
pub struct ModelUsage {
    pub model: String,
    pub tokens: i64,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct StatusSnapshot {
    pub updated_at: i64,
    pub dry_run: bool,
    pub throttle_threshold: f64,
    pub restore_threshold: f64,
    pub new_api_base: String,
    pub new_api_healthy: bool,
    pub active_channel_id: Option<i64>,
    pub keys: Vec<KeyStatus>,
    /// 客户端应连的地址（= new_api_base + /v1）
    pub client_endpoint: String,
    /// new-api **内部虚拟余额**（root 用户）。它按「按量付费倍率」给包月套餐虚构记账，
    /// 一旦见底会**直接挡住转发**（「预扣费额度失败」），跟智谱额度毫无关系。
    /// 低于阈值时看板告警，避免无声卡死。
    pub newapi_user_quota: i64,
    // —— 以下为**面板循环**独占写的字段（与上面决策字段严格不相交）——
    /// new-api 侧渠道实况（合并进 key 卡片；不在 keys 里的即「野生渠道」）
    pub channels: Vec<ChannelState>,
    /// 每渠道实时指标（rpm/tpm/最后请求）
    pub live: Vec<LiveMetric>,
    /// 近 24 小时 token 时序（按小时）
    pub hourly: Vec<UsagePoint>,
    /// 按模型的 token 用量
    pub model_usage: Vec<ModelUsage>,
}

pub type Shared = Arc<RwLock<StatusSnapshot>>;

/// 读快照。锁 poison 时用 into_inner 恢复——避免一次 panic 让看板永久黑屏。
fn read_snap(snap: &Shared) -> StatusSnapshot {
    snap.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// **字段级原子更新**：在写锁内就地修改。
///
/// 决策循环与面板循环各自 read-modify-write，但两者写的字段**严格不相交**
/// （决策：keys/active/阈值/健康；面板：channels/live/hourly/model_usage/内部余额），
/// 加上写锁互斥 ⇒ 不会互相覆盖对方的更新。
pub fn update(snap: &Shared, f: impl FnOnce(&mut StatusSnapshot)) {
    let mut g = snap.write().unwrap_or_else(|e| e.into_inner());
    f(&mut g);
}

/// 常驻状态服务。bind 失败 → 记 error 并**返回**（降级），主循环不受影响。
pub async fn serve(addr: String, snap: Shared) {
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            error!(addr = %addr, error = %e, "状态看板监听失败，已降级（切换循环照常运行）");
            return;
        }
    };
    info!("状态看板已启动 → http://{addr}");
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let s = snap.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, s).await {
                        debug!(error = %e, "看板连接处理失败");
                    }
                });
            }
            // accept 出错不退出监听循环
            Err(e) => debug!(error = %e, "accept 失败"),
        }
    }
}

async fn handle_conn(mut stream: TcpStream, snap: Shared) -> std::io::Result<()> {
    // 上界 1 KiB：请求行 + 头足够，防畸形请求撑爆内存
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");

    let (status, ctype, body) = match path {
        "/api/status" => {
            let json =
                serde_json::to_string(&read_snap(&snap)).unwrap_or_else(|_| "{}".to_string());
            ("200 OK", "application/json; charset=utf-8", json)
        }
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", render_html()),
        "" => (
            "400 Bad Request",
            "text/plain; charset=utf-8",
            "bad request".to_string(),
        ),
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found".to_string(),
        ),
    };

    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Connection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// 自包含看板：内联 CSS/JS，不引用任何外部 CDN。
fn render_html() -> String {
    r##"<!doctype html>
<html lang="zh"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>quota-throttle</title>
<style>
 :root{--bg:#0b0d12;--card:#151922;--line:#252b38;--txt:#e8eaef;--dim:#8b94a3;
       --ok:#3ecf8e;--warn:#f5b942;--bad:#f2555a;--accent:#5b8cff}
 *{box-sizing:border-box}
 body{margin:0;padding:32px 28px;background:
      radial-gradient(1200px 600px at 20% -10%,#1a2030 0%,transparent 60%),var(--bg);
      color:var(--txt);min-height:100vh;
      font:14px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"PingFang SC","Microsoft YaHei",sans-serif}
 .wrap{max-width:1100px;margin:0 auto}
 header{display:flex;align-items:baseline;gap:12px;margin-bottom:6px}
 h1{margin:0;font-size:22px;letter-spacing:-.01em}
 .tag{font-size:11px;color:var(--dim);border:1px solid var(--line);border-radius:5px;padding:2px 7px}
 .sub{color:var(--dim);margin-bottom:22px;font-size:13px}
 h2{font-size:12px;margin:30px 0 12px;color:var(--dim);font-weight:600;
    text-transform:uppercase;letter-spacing:.09em}
 .chips{display:flex;gap:12px;flex-wrap:wrap;margin-bottom:22px}
 .chip{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:11px 16px;
       font-size:13px;display:flex;align-items:center;gap:8px}
 .chip .k{color:var(--dim)} .chip .v{font-weight:600}
 .dot{width:8px;height:8px;border-radius:50%}
 .copy{cursor:pointer;border:1px dashed var(--line);border-radius:6px;padding:3px 9px;
       font-family:ui-monospace,SFMono-Regular,monospace;font-size:12px;color:var(--accent)}
 .copy:hover{border-color:var(--accent);background:rgba(91,140,255,.08)}
 .grid{display:grid;gap:14px}
 .card{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:18px 20px;
       transition:border-color .3s,background .3s}
 .card.act{border-color:rgba(62,207,142,.45);background:linear-gradient(180deg,rgba(62,207,142,.06),transparent 60%),var(--card)}
 .card.dead{opacity:.7}
 .card.off{border-color:rgba(242,85,90,.5);background:linear-gradient(180deg,rgba(242,85,90,.08),transparent 60%),var(--card)}
 .chead{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
 .name{font-size:16px;font-weight:650}
 .tier{padding:2px 10px;border-radius:999px;font-size:11px;font-weight:700;letter-spacing:.04em}
 .t-active{background:rgba(62,207,142,.16);color:var(--ok)}
 .t-standby{background:rgba(139,148,163,.16);color:#aeb6c2}
 .t-exhausted{background:rgba(242,85,90,.16);color:var(--bad)}
 .t-unknown{background:rgba(245,185,66,.16);color:var(--warn)}
 .badge{padding:2px 8px;border-radius:6px;font-size:11px;font-weight:700}
 .b-on{background:rgba(62,207,142,.13);color:var(--ok)}
 .b-off{background:rgba(242,85,90,.22);color:var(--bad)}
 .cid{color:var(--dim);font-size:12px;font-variant-numeric:tabular-nums}
 .live{display:flex;align-items:center;gap:9px;margin:12px 0 16px;font-size:13px;
       font-variant-numeric:tabular-nums;color:var(--dim)}
 .live b{color:var(--txt)}
 .pulse{width:8px;height:8px;border-radius:50%;background:var(--ok);
        box-shadow:0 0 0 0 rgba(62,207,142,.6);animation:p 1.6s infinite}
 @keyframes p{0%{box-shadow:0 0 0 0 rgba(62,207,142,.5)}70%{box-shadow:0 0 0 7px rgba(62,207,142,0)}100%{box-shadow:0 0 0 0 rgba(62,207,142,0)}}
 .idle{width:8px;height:8px;border-radius:50%;background:#3a4150}
 .win{display:grid;grid-template-columns:1fr 1fr;gap:22px}
 @media(max-width:640px){.win{grid-template-columns:1fr}}
 .wlab{display:flex;justify-content:space-between;align-items:baseline;margin-bottom:7px}
 .wlab .l{color:var(--dim);font-size:12px}
 .wlab .p{font-size:17px;font-weight:700;font-variant-numeric:tabular-nums}
 .bar{position:relative;height:9px;background:#20252f;border-radius:999px;overflow:hidden}
 .fill{height:100%;border-radius:999px;transition:width .6s cubic-bezier(.4,0,.2,1)}
 .thr{position:absolute;top:-3px;bottom:-3px;width:2px;background:var(--bad);opacity:.6;border-radius:2px}
 .rst{margin-top:6px;color:var(--dim);font-size:12px;font-variant-numeric:tabular-nums}
 .meta{margin-top:16px;padding-top:13px;border-top:1px solid var(--line);
       color:var(--dim);font-size:12px;display:flex;gap:16px;flex-wrap:wrap;font-variant-numeric:tabular-nums}
 .err{color:var(--bad);font-size:12px;margin-top:4px}
 .warn{color:var(--warn);font-size:11px}
 .chart{display:flex;align-items:flex-end;gap:3px;height:110px;padding:0 2px}
 .cb{flex:1;background:linear-gradient(180deg,var(--accent),rgba(91,140,255,.35));
     border-radius:3px 3px 0 0;min-height:2px;transition:height .5s;position:relative}
 .cb:hover{background:var(--accent)}
 .cb span{display:none;position:absolute;bottom:100%;left:50%;transform:translateX(-50%);
          background:#0b0d12;border:1px solid var(--line);border-radius:6px;padding:4px 8px;
          font-size:11px;white-space:nowrap;margin-bottom:6px;z-index:9}
 .cb:hover span{display:block}
 .xax{display:flex;justify-content:space-between;color:var(--dim);font-size:11px;margin-top:8px}
 .tbl{width:100%;border-collapse:collapse;font-size:13px}
 .tbl th{text-align:left;color:var(--dim);font-weight:500;font-size:11px;text-transform:uppercase;
         letter-spacing:.05em;padding:0 12px 9px 0;border-bottom:1px solid var(--line)}
 .tbl td{padding:10px 12px 10px 0;border-bottom:1px solid rgba(37,43,56,.55);font-variant-numeric:tabular-nums}
 .tbl tr:last-child td{border-bottom:none}
 .empty{color:var(--dim);font-size:13px;padding:6px 0}
 .foot{margin-top:26px;color:var(--dim);font-size:12px}
</style></head><body><div class="wrap">
<header><h1>quota-throttle</h1><span class="tag">智谱 GLM Coding Plan</span></header>
<div class="sub" id="sub">加载中…</div>
<div class="chips" id="chips"></div>
<div class="grid" id="grid"></div>
<div id="wild"></div>
<h2>近 24 小时用量</h2>
<div class="card"><div id="chart"></div></div>
<h2>按模型</h2>
<div class="card"><div id="models"></div></div>
<div class="foot" id="foot"></div>
</div><script>
const pad=n=>String(n).padStart(2,'0');
const clock=ms=>{const d=new Date(ms);return `${pad(d.getMonth()+1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`};
const left=ms=>{if(!ms)return'—';let s=Math.floor((ms-Date.now())/1000);if(s<=0)return'即将重置';
  const d=Math.floor(s/86400);s%=86400;const h=Math.floor(s/3600);const m=Math.floor(s%3600/60);
  return d?`${d} 天 ${h} 小时后重置`:h?`${h} 小时 ${m} 分后重置`:`${m} 分后重置`};
const ago=sec=>{if(!sec)return null;const s=Math.floor(Date.now()/1000)-sec;
  if(s<60)return `${s} 秒前`; if(s<3600)return `${Math.floor(s/60)} 分钟前`;
  if(s<86400)return `${Math.floor(s/3600)} 小时前`; return `${Math.floor(s/86400)} 天前`};
const kfmt=n=>n>=1e6?(n/1e6).toFixed(2)+'M':n>=1e3?(n/1e3).toFixed(1)+'k':String(n||0);
const hue=(p,thr)=>p>=thr?'var(--bad)':p>=thr*0.8?'var(--warn)':'var(--ok)';
const TIER={active:'ACTIVE',standby:'STANDBY',exhausted:'耗尽',unknown:'未知'};
const LOW=10000000;

function win(label,pct,reset,thr){
  if(pct==null) return `<div><div class="wlab"><span class="l">${label}</span></div><div class="err">查询失败</div></div>`;
  const w=Math.max(0,Math.min(100,pct));
  return `<div>
    <div class="wlab"><span class="l">${label}</span><span class="p" style="color:${hue(pct,thr)}">${pct}%</span></div>
    <div class="bar"><div class="fill" style="width:${w}%;background:${hue(pct,thr)}"></div>
      <div class="thr" style="left:${thr}%" title="切换阈值 ${thr}%"></div></div>
    <div class="rst">${left(reset)}</div></div>`;
}

async function tick(){
  let d;
  try{ d=await (await fetch('/api/status',{cache:'no-store'})).json(); }
  catch(e){ document.getElementById('sub').textContent='连不上 quota-throttle（进程没跑？）'; return; }
  if(!d.keys||!d.keys.length){ document.getElementById('sub').textContent='等待首轮采集…'; return; }

  const thr=d.throttle_threshold;
  const act=d.keys.find(k=>k.channel_id===d.active_channel_id);
  const chOf=id=>(d.channels||[]).find(c=>c.id===id);
  const lvOf=id=>(d.live||[]).find(l=>l.channel_id===id);

  document.getElementById('sub').textContent=
    `任一窗口达 ${thr}% 即切换 · 挑新活动 key 要求低于 ${d.restore_threshold}%`;

  const q=d.newapi_user_quota;
  document.getElementById('chips').innerHTML=`
   <div class="chip"><span class="dot" style="background:${d.new_api_healthy?'var(--ok)':'var(--bad)'}"></span>
     <span class="k">new-api</span><span class="v">${d.new_api_healthy?'健康':'不可达'}</span></div>
   <div class="chip"><span class="k">活动 key</span>
     <span class="v" style="color:var(--ok)">${act?act.name:'无 · 全部无额度'}</span></div>
   <div class="chip"><span class="k">客户端连接</span>
     <span class="copy" title="点击复制" onclick="navigator.clipboard.writeText('${d.client_endpoint||''}');this.textContent='已复制';setTimeout(()=>this.textContent='${d.client_endpoint||''}',900)">${d.client_endpoint||'—'}</span></div>
   ${(q!=null&&q>=0&&q<LOW)?'<div class="chip" style="border-color:var(--bad)"><span class="v" style="color:var(--bad)">new-api 内部余额即将耗尽</span><span class="k">见底会挡住转发（与智谱额度无关）</span></div>':''}
   ${d.dry_run?'<div class="chip" style="border-color:rgba(245,185,66,.5)"><span class="v" style="color:var(--warn)">dry_run</span><span class="k">只打印决策，不真改 new-api</span></div>':''}`;

  // —— 合并卡片：智谱用量 + new-api 渠道状态 + 实时指标，一把 key 全在这 ——
  document.getElementById('grid').innerHTML=d.keys.map(k=>{
    const c=chOf(k.channel_id), l=lvOf(k.channel_id);
    const disabled = c && !c.enabled;
    const mism = c && c.priority!=null && k.priority!=null && c.priority!==k.priority;
    const on = l && l.rpm>0;
    const lastTxt = l&&l.last_request_at ? `最后请求 ${ago(l.last_request_at)}${l.last_request_model?` (${l.last_request_model})`:''}` : '暂无请求记录';
    return `
   <div class="card ${k.tier==='active'?'act':''} ${k.tier==='exhausted'?'dead':''} ${disabled?'off':''}">
     <div class="chead">
       <span class="name">${k.name}</span>
       <span class="tier t-${k.tier}">${TIER[k.tier]||k.tier}</span>
       <span class="cid">渠道 #${k.channel_id}</span>
       ${c ? (c.enabled ? '<span class="badge b-on">启用</span>'
             : `<span class="badge b-off">已被 new-api 禁用</span>`) : ''}
     </div>
     ${disabled?`<div class="err">status=${c.status_raw} · priority 不起作用，流量不会来这把 key</div>`:''}
     ${k.error?`<div class="err">${k.error}</div>`:''}

     <div class="live">
       <span class="${on?'pulse':'idle'}"></span>
       <span><b>${l?l.rpm:0}</b> req/min</span><span>·</span>
       <span><b>${kfmt(l?l.tpm:0)}</b> tok/min</span><span>·</span>
       <span>${lastTxt}</span>
     </div>

     <div class="win">
       ${win('5 小时窗口',k.five_hour_pct,k.five_hour_reset,thr)}
       ${win('每周窗口',k.weekly_pct,k.weekly_reset,thr)}
     </div>

     <div class="meta">
       <span>priority <b style="color:${mism?'var(--warn)':'var(--txt)'}">${k.priority??'—'}</b>${mism?` <span class="warn">（new-api 侧是 ${c.priority}，不一致！）</span>`:''}</span>
       ${c?`<span>分组 ${c.group||'—'}</span><span>auto_ban ${c.auto_ban?'开':'关'}</span><span style="opacity:.7">${c.models||''}</span>`:''}
     </div>
   </div>`}).join('');

  // 野生渠道：new-api 里有、但不在我们管辖的 keys 里 —— 可能偷偷接到流量
  const mine=new Set(d.keys.map(k=>k.channel_id));
  const wild=(d.channels||[]).filter(c=>!mine.has(c.id));
  document.getElementById('wild').innerHTML = !wild.length ? '' : `
    <h2>野生渠道（不在 config.keys 里，我们不管它）</h2>
    <div class="card"><table class="tbl"><thead><tr><th>渠道</th><th>状态</th><th>priority</th><th>分组</th><th>模型</th></tr></thead><tbody>${
      wild.map(c=>`<tr><td><b>${c.name}</b> <span class="cid">#${c.id}</span></td>
        <td>${c.enabled?'<span class="badge b-on">启用</span><div class="warn">可能接到流量</div>':'<span class="badge b-off">禁用</span>'}</td>
        <td>${c.priority??'—'}</td><td style="color:var(--dim)">${c.group||'—'}</td>
        <td style="color:var(--dim);font-size:12px">${c.models||'—'}</td></tr>`).join('')}</tbody></table></div>`;

  // 近 24h 时序
  const hs=d.hourly||[];
  if(!hs.length){ document.getElementById('chart').innerHTML='<div class="empty">近 24 小时暂无用量</div>'; }
  else{
    const max=Math.max(...hs.map(p=>p.tokens),1);
    document.getElementById('chart').innerHTML=
      `<div class="chart">${hs.map(p=>`<div class="cb" style="height:${Math.max(2,p.tokens/max*100)}%">
          <span>${new Date(p.hour*1000).getHours()}:00 · ${p.tokens.toLocaleString()} tokens · ${p.count} 次</span></div>`).join('')}</div>
       <div class="xax"><span>${new Date(hs[0].hour*1000).getHours()}:00</span>
         <span>峰值 ${kfmt(max)} tokens/小时</span>
         <span>${new Date(hs[hs.length-1].hour*1000).getHours()}:00</span></div>`;
  }

  // 按模型
  const ms=d.model_usage||[];
  document.getElementById('models').innerHTML = !ms.length ? '<div class="empty">暂无数据</div>'
    : `<table class="tbl"><thead><tr><th>模型</th><th>tokens</th><th>请求数</th><th>占比</th></tr></thead><tbody>${
      (()=>{const tot=ms.reduce((a,m)=>a+m.tokens,0)||1;
        return ms.map(m=>`<tr><td><b>${m.model}</b></td>
          <td>${m.tokens.toLocaleString()}</td><td style="color:var(--dim)">${m.count}</td>
          <td><div class="bar" style="height:6px;width:120px"><div class="fill" style="width:${m.tokens/tot*100}%;background:var(--accent)"></div></div></td></tr>`).join('')})()
      }</tbody></table>`;

  document.getElementById('foot').textContent=
    `决策更新 ${clock(d.updated_at)} · 面板每 ${5} 秒刷新（智谱用量按 poll_interval_secs 轮询）`;
}
tick(); setInterval(tick,5000);
</script></body></html>"##
        .to_string()
}
