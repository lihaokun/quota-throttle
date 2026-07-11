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
}

pub type Shared = Arc<RwLock<StatusSnapshot>>;

/// 读快照。锁 poison 时用 into_inner 恢复——避免一次 panic 让看板永久黑屏。
fn read_snap(snap: &Shared) -> StatusSnapshot {
    snap.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 写快照（整体覆盖，不做增量合并，避免陈旧字段残留）。
pub fn publish(snap: &Shared, value: StatusSnapshot) {
    *snap.write().unwrap_or_else(|e| e.into_inner()) = value;
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
 :root{--bg:#0b0d12;--card:#151922;--card2:#1b202b;--line:#252b38;--txt:#e8eaef;--dim:#8b94a3;
       --ok:#3ecf8e;--warn:#f5b942;--bad:#f2555a;--accent:#5b8cff}
 *{box-sizing:border-box}
 body{margin:0;padding:32px 28px;background:
      radial-gradient(1200px 600px at 20% -10%,#1a2030 0%,transparent 60%),var(--bg);
      color:var(--txt);min-height:100vh;
      font:14px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,"PingFang SC","Microsoft YaHei",sans-serif}
 .wrap{max-width:1080px;margin:0 auto}
 header{display:flex;align-items:baseline;gap:12px;margin-bottom:6px}
 h1{margin:0;font-size:22px;letter-spacing:-.01em}
 .tag{font-size:11px;color:var(--dim);border:1px solid var(--line);border-radius:5px;padding:2px 7px}
 .sub{color:var(--dim);margin-bottom:22px;font-size:13px}
 .chips{display:flex;gap:12px;flex-wrap:wrap;margin-bottom:22px}
 .chip{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:11px 16px;
       font-size:13px;display:flex;align-items:center;gap:8px}
 .chip .k{color:var(--dim)} .chip .v{font-weight:600}
 .dot{width:8px;height:8px;border-radius:50%;box-shadow:0 0 0 3px rgba(62,207,142,.12)}
 .grid{display:grid;gap:14px}
 .card{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:18px 20px;
       transition:border-color .3s,background .3s}
 .card.act{border-color:rgba(62,207,142,.45);background:linear-gradient(180deg,rgba(62,207,142,.06),transparent 60%),var(--card)}
 .card.dead{opacity:.72}
 .chead{display:flex;align-items:center;gap:10px;margin-bottom:16px}
 .name{font-size:16px;font-weight:650}
 .tier{padding:2px 10px;border-radius:999px;font-size:11px;font-weight:700;letter-spacing:.04em}
 .t-active{background:rgba(62,207,142,.16);color:var(--ok)}
 .t-standby{background:rgba(139,148,163,.16);color:#aeb6c2}
 .t-exhausted{background:rgba(242,85,90,.16);color:var(--bad)}
 .t-unknown{background:rgba(245,185,66,.16);color:var(--warn)}
 .meta{margin-left:auto;color:var(--dim);font-size:12px;font-variant-numeric:tabular-nums}
 .win{display:grid;grid-template-columns:1fr 1fr;gap:22px}
 @media(max-width:640px){.win{grid-template-columns:1fr}}
 .wlab{display:flex;justify-content:space-between;align-items:baseline;margin-bottom:7px}
 .wlab .l{color:var(--dim);font-size:12px}
 .wlab .p{font-size:17px;font-weight:700;font-variant-numeric:tabular-nums}
 .bar{position:relative;height:9px;background:#20252f;border-radius:999px;overflow:hidden}
 .fill{height:100%;border-radius:999px;transition:width .6s cubic-bezier(.4,0,.2,1)}
 .thr{position:absolute;top:-3px;bottom:-3px;width:2px;background:var(--bad);opacity:.6;border-radius:2px}
 .rst{margin-top:6px;color:var(--dim);font-size:12px;font-variant-numeric:tabular-nums}
 .err{color:var(--bad);font-size:12px;margin-top:4px}
 .foot{margin-top:20px;color:var(--dim);font-size:12px}
</style></head><body><div class="wrap">
<header><h1>quota-throttle</h1><span class="tag">智谱 GLM Coding Plan</span></header>
<div class="sub" id="sub">加载中…</div>
<div class="chips" id="chips"></div>
<div class="grid" id="grid"></div>
<div class="foot" id="foot"></div>
</div><script>
const pad=n=>String(n).padStart(2,'0');
const clock=ms=>{const d=new Date(ms);return `${pad(d.getMonth()+1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`};
// 倒计时比时间戳有用：直接看还有多久重置
const left=ms=>{if(!ms)return'—';let s=Math.floor((ms-Date.now())/1000);if(s<=0)return'即将重置';
  const d=Math.floor(s/86400);s%=86400;const h=Math.floor(s/3600);const m=Math.floor(s%3600/60);
  return d?`${d}天${h}小时后重置`:h?`${h}小时${m}分后重置`:`${m}分后重置`};
const hue=(p,thr)=>p>=thr?'var(--bad)':p>=thr*0.8?'var(--warn)':'var(--ok)';
const TIER={active:'ACTIVE',standby:'STANDBY',exhausted:'耗尽',unknown:'未知'};

function win(label,pct,reset,thr){
  if(pct==null) return `<div><div class="wlab"><span class="l">${label}</span></div>
     <div class="err">查询失败</div></div>`;
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

  const act=d.keys.find(k=>k.channel_id===d.active_channel_id);
  const thr=d.throttle_threshold;
  document.getElementById('sub').textContent=
    `任一窗口达 ${thr}% 即切换 · 挑新活动 key 要求低于 ${d.restore_threshold}%`;

  document.getElementById('chips').innerHTML=`
   <div class="chip"><span class="dot" style="background:${d.new_api_healthy?'var(--ok)':'var(--bad)'}"></span>
     <span class="k">new-api</span><span class="v">${d.new_api_healthy?'健康':'不可达'}</span>
     <span class="k">${d.new_api_base}</span></div>
   <div class="chip"><span class="k">活动 key</span>
     <span class="v" style="color:var(--ok)">${act?act.name:'无 · 全部无额度'}</span></div>
   ${d.dry_run?'<div class="chip" style="border-color:rgba(245,185,66,.5)"><span class="v" style="color:var(--warn)">dry_run</span><span class="k">只打印决策，不真改 new-api</span></div>':''}`;

  document.getElementById('grid').innerHTML=d.keys.map(k=>`
   <div class="card ${k.tier==='active'?'act':''} ${k.tier==='exhausted'?'dead':''}">
     <div class="chead">
       <span class="name">${k.name}</span>
       <span class="tier t-${k.tier}">${TIER[k.tier]||k.tier}</span>
       <span class="meta">渠道 #${k.channel_id} · priority ${k.priority??'—'}</span>
     </div>
     ${k.error?`<div class="err">${k.error}</div>`:''}
     <div class="win">
       ${win('5 小时窗口',k.five_hour_pct,k.five_hour_reset,thr)}
       ${win('每周窗口',k.weekly_pct,k.weekly_reset,thr)}
     </div>
   </div>`).join('');

  document.getElementById('foot').textContent=`最后更新 ${clock(d.updated_at)} · 每 5 秒自动刷新`;
}
tick(); setInterval(tick,5000);
</script></body></html>"##
        .to_string()
}
