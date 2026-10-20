//! 冻结 e1a9279 的 GLM 选择参考，仅测试使用；禁止改成调用生产 decide。
use crate::orchestrator::{Decision, PinRelease, Regime, WeeklyInfo};
use std::collections::HashMap;

fn imminent(weekly: &WeeklyInfo, now_ms: i64, lookahead_ms: i64, exhausted: f64) -> bool {
    if weekly.pct >= exhausted || weekly.max_pct >= exhausted {
        return false;
    }
    let until = weekly.reset_ms - now_ms;
    until > 0 && until <= lookahead_ms
}

fn eligible_set(
    ids: &[i64],
    pct: &HashMap<i64, f64>,
    throttle: f64,
    exhausted: f64,
) -> (Vec<i64>, Regime) {
    let under = |limit: f64| -> Vec<i64> {
        ids.iter()
            .copied()
            .filter(|id| pct.get(id).is_some_and(|p| *p < limit))
            .collect()
    };
    if !ids.iter().any(|id| pct.contains_key(id)) {
        return (Vec::new(), Regime::Normal);
    }
    let normal = under(throttle);
    if !normal.is_empty() {
        return (normal, Regime::Normal);
    }
    (under(exhausted), Regime::Degraded)
}

/// 选活动 key。**纯函数**（分支多、又是安全核心，必须可单测）。
///
/// 三层，从强到弱：
///   1. pin —— 只在合格集内生效。pin 是**优先级**，不是安全豁免：它能覆盖「粘滞 + 挑最低」
///      这个选择偏好，但**不能把自动逻辑判定为不合格的 key 拉上来用**。越线即自动解除。
///   2. 粘滞 —— 现任只要还合格就不换（护 prompt 缓存：能不换就不换）。
///   3. 挑选 —— 只在合格集内挑（keyrot-1：**临期 key 优先**，按周重置时刻升序——先烧快清零的；
///      无临期则回到旧策略：正常档优先要求 < restore，再挑 pct 最低）。
///
/// **抖动保护**：本轮查询失败的 key 不进合格集（不会被主动选中），但若它恰好是 current
/// 或 pinned，则**保持不变** —— 一次瞬时失败不该丢缓存，也不该抖掉用户的 pin。
/// 反过来说，解除 pin 必须基于「查到了 **且** 确实超线」这个正面证据。
///
/// keyrot-1 的 pin 说明：临期只影响合格集内排序，不放宽 pin 门限；正常档仍是 throttle，
/// 降级档仍是 exhausted，与旧版一致。
#[allow(clippy::too_many_arguments)]
pub(super) fn decide(
    ids: &[i64],
    pct: &HashMap<i64, f64>,
    weekly: &HashMap<i64, WeeklyInfo>,
    now_ms: i64,
    lookahead_ms: i64,
    current: Option<i64>,
    pinned: Option<i64>,
    throttle: f64,
    restore: f64,
    exhausted: f64,
) -> Decision {
    let (eligible, regime) = eligible_set(ids, pct, throttle, exhausted);
    let imminents: Vec<i64> = eligible
        .iter()
        .copied()
        .filter(|id| {
            weekly
                .get(id)
                .is_some_and(|w| imminent(w, now_ms, lookahead_ms, exhausted))
        })
        .collect();
    let mut pin_release = None;
    let done = |active, eligible, imminents, pin_release| Decision {
        active,
        eligible,
        imminent: imminents,
        regime,
        pin_release,
    };

    // 1. pin（只在合格集内）
    if let Some(p) = pinned.filter(|p| ids.contains(p)) {
        match pct.get(&p) {
            // 查询失败 → 保持 pin（抖动保护）
            None => return done(Some(p), eligible, imminents, None),
            Some(_) if eligible.contains(&p) => return done(Some(p), eligible, imminents, None),
            // 越线 → 解除 pin，落到自动逻辑。临期不放宽门限，仍按档位取。
            Some(&pp) => {
                let limit = match regime {
                    Regime::Normal => throttle,
                    Regime::Degraded => exhausted,
                };
                pin_release = Some(PinRelease {
                    channel_id: p,
                    pct: pp,
                    limit,
                });
            }
        }
    }

    // 2. 粘滞
    if let Some(c) = current.filter(|c| ids.contains(c)) {
        match pct.get(&c) {
            // 查询失败 → 保持（抖动保护）
            None => return done(Some(c), eligible, imminents, pin_release),
            Some(_) if eligible.contains(&c) => {
                return done(Some(c), eligible, imminents, pin_release)
            }
            Some(_) => {}
        }
    }

    // 3. 在合格集内挑。keyrot-1：临期者优先（EDF——周重置时刻升序，平手取 max_pct 低者，多留安全垫）。
    //    无临期 ⇒ 完全沿用旧策略（正常档 restore 优先，降级档最低）。
    let lowest = |cands: &[i64]| -> Option<i64> {
        cands
            .iter()
            .copied()
            .min_by(|a, b| pct[a].total_cmp(&pct[b]))
    };
    let active = if !imminents.is_empty() {
        imminents.iter().copied().min_by(|a, b| {
            let wa = &weekly[a];
            let wb = &weekly[b];
            wa.reset_ms
                .cmp(&wb.reset_ms)
                .then_with(|| wa.max_pct.total_cmp(&wb.max_pct))
        })
    } else {
        match regime {
            Regime::Normal => {
                // 优先挑余量更足的（< restore），让新活动 key 撑更久 → 切换更少
                let roomy: Vec<i64> = eligible
                    .iter()
                    .copied()
                    .filter(|id| pct[id] < restore)
                    .collect();
                lowest(&roomy).or_else(|| lowest(&eligible))
            }
            // 降级档全员已 ≥ throttle，restore 在这一档没有意义
            Regime::Degraded => lowest(&eligible),
        }
    };

    // 4. active == None ⇒ 合格集为空（全员 ≥ exhausted 或全查询失败）⇒ 调用方保留原活动
    done(active, eligible, imminents, pin_release)
}
