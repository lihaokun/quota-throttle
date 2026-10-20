import { formatTime, matchNative, safeAdd } from './core.mjs';
import { barModel } from './charts.mjs';

export function node(document, tag, text = null, className = null) {
  const element = document.createElement(tag);
  if (text !== null) element.textContent = String(text);
  if (className !== null) element.className = className;
  return element;
}
const labels = { glm: 'GLM', volcano: '火山', unknown: '尚未观测', stale: '观察已过期', unavailable: '查询失败', observed: '已观测' };
// main 看板的数字/时间格式。
const kfmt = n => typeof n !== 'number' ? String(n ?? '—') : n >= 1e9 ? `${(n / 1e9).toFixed(2)}B` : n >= 1e6 ? `${(n / 1e6).toFixed(2)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}k` : String(n);
const hue = p => p >= 90 ? 'var(--bad)' : p >= 72 ? 'var(--warn)' : 'var(--ok)';
const left = ms => {
  if (!Number.isFinite(ms)) return '—';
  let s = Math.floor(ms / 1000); if (s <= 0) return '即将重置';
  const d = Math.floor(s / 86400); s %= 86400; const h = Math.floor(s / 3600), m = Math.floor(s % 3600 / 60);
  return d ? `${d} 天 ${h} 小时后重置` : h ? `${h} 小时 ${m} 分后重置` : `${m} 分后重置`;
};
const ago = ms => !Number.isFinite(ms) ? '未采样' : ms < 60000 ? `采样 ${Math.max(0, Math.floor(ms / 1000))} 秒前` : ms < 3600000 ? `采样 ${Math.floor(ms / 60000)} 分钟前` : `采样 ${Math.floor(ms / 3600000)} 小时前`;

export function createRenderer(document, handlers) {
  const byId = id => document.getElementById(id);
  const text = (tag, value, cls) => node(document, tag, value, cls);
  const button = (label, fn, disabled = false, focusKey = '', cls = null) => {
    const b = node(document, 'button', label, cls); b.type = 'button'; b.disabled = disabled;
    b.dataset.focusKey = focusKey; b.addEventListener('click', fn); return b;
  };
  const toolbar = (...children) => { const box = text('div', null, 'toolbar'); box.append(...children); return box; };
  const action = (name, title, id, disabled, cls = null) => button(title, () => handlers.action(name, id), disabled, `${name}:${id ?? ''}`, cls);
  const span = (value, cls) => text('span', value, cls);
  const tier = (label, kind) => span(label, `tier t-${kind}`);
  const badge = (label, kind) => span(label, `badge b-${kind}`);

  // 每秒都变的文字（倒计时、采样年龄）只改 textContent，不参与重建判定（见 updateLive）。
  const live = (kind, value, cls) => { const el = span('', cls); el.dataset.live = kind; el.dataset.v = String(value); return el; };
  function updateLive(vm) {
    const now = vm.snapshot ? vm.snapshot.captured_at_ms + vm.observation.age : NaN;
    for (const el of document.querySelectorAll('[data-live]')) {
      const value = el.dataset.v;
      const next = el.dataset.live === 'reset' ? left(Number(value) - now) : el.dataset.live === 'age' ? ago(vm.observation.sourceViews.get(value)?.age_ms) : '';
      if (el.textContent !== next) el.textContent = next;
    }
  }
  // 区块重建守卫：内容戳不变就不碰 DOM；必须重建时保住打开的 ⋯ 菜单与展开的详情。
  function guard(root, stamp, build) {
    if (root.dataset.stamp === stamp) return;
    const open = root.querySelector('.menu.open')?.dataset.key ?? null, expanded = new Set([...root.querySelectorAll('details[open]')].map(d => d.dataset.key));
    root.replaceChildren(); build(root); root.dataset.stamp = stamp;
    if (open !== null) for (const m of root.querySelectorAll('.menu')) if (m.dataset.key === open) { m.classList.add('open'); m.querySelector('.menu-btn').setAttribute('aria-expanded', 'true'); }
    for (const d of root.querySelectorAll('details')) if (expanded.has(d.dataset.key)) d.open = true;
  }

  // ⋯ 分类操作菜单（浮层）。开合用 classList，不用内联 style（守 CSP）。
  const collapse = keep => { for (const m of document.querySelectorAll('.menu.open')) if (m !== keep) { m.classList.remove('open'); m.querySelector('.menu-btn')?.setAttribute('aria-expanded', 'false'); } };
  const menu = (key, groups) => {
    const wrap = text('div', null, 'menu'); wrap.dataset.key = key;
    const trigger = text('button', '⋯', 'menu-btn'); trigger.type = 'button';
    trigger.setAttribute('aria-haspopup', 'true'); trigger.setAttribute('aria-expanded', 'false'); trigger.setAttribute('aria-label', '操作菜单');
    const pop = text('div', null, 'menu-pop');
    for (const g of groups) {
      const items = g.items.filter(Boolean);
      if (!items.length) continue;
      pop.append(text('div', g.label, 'menu-group'));
      for (const item of items) { item.className = 'menu-item'; item.addEventListener('click', () => { wrap.classList.remove('open'); trigger.setAttribute('aria-expanded', 'false'); }, {capture: true}); pop.append(item); }
    }
    trigger.addEventListener('click', event => { event.stopPropagation(); collapse(wrap); trigger.setAttribute('aria-expanded', String(wrap.classList.toggle('open'))); });
    wrap.append(trigger, pop);
    return wrap;
  };
  document.addEventListener('click', event => collapse(event.composedPath().find(node => node.classList?.contains('menu')) ?? null));
  document.addEventListener('keydown', event => { if (event.key === 'Escape') collapse(null); });

  function table(headers, rows) {
    const wrapper = text('div', null, 'tbl-wrap'), t = text('table', null, 'tbl'), head = text('thead'), line = text('tr');
    for (const title of headers) { const th = text('th', title); th.scope = 'col'; line.append(th); }
    head.append(line); t.append(head); const body = text('tbody');
    for (const row of rows) { const tr = text('tr'); for (const value of row) { const td = text('td'); if (value?.nodeType) td.append(value); else td.textContent = String(value); tr.append(td); } body.append(tr); }
    t.append(body); wrapper.append(t); return wrapper;
  }
  function pager(page, count, size, change, key) {
    const total = Math.max(1, Math.ceil(count / size)), current = Math.min(page, total - 1);
    return toolbar(button('上一页', () => change(Math.max(0, current - 1)), current === 0, `${key}:previous`, 'pbtn'), span(`${current + 1} / ${total} · ${count} 项`), button('下一页', () => change(current + 1), current + 1 >= total, `${key}:next`, 'pbtn'));
  }
  // main 的窗口组件：标签 + 大号百分比 + 进度条 + 重置倒计时。pct 为 null ⇒ 显示原因，绝不画成 0%。
  function win(label, pct, reset, missing, extra = null) {
    const box = text('div'), lab = text('div', null, 'wlab');
    lab.append(span(label, 'l'));
    if (pct === null) { box.append(lab, text('div', missing, 'err')); return box; }
    const value = span(`${Math.round(pct)}%`, 'p'); value.style.color = hue(pct); lab.append(value);
    const bar = text('div', null, 'bar'), fill = text('div', null, 'fill');
    fill.style.width = `${Math.max(0, Math.min(100, pct))}%`; fill.style.background = hue(pct); bar.append(fill);
    const rst = text('div', null, 'rst');
    if (extra) rst.append(span(`${extra}${reset === null ? '' : ' · '}`));
    if (reset !== null) rst.append(live('reset', reset));
    box.append(lab, bar, rst); return box;
  }
  const groupNames = (vm, sourceId) => [...new Set((vm.snapshot?.catalog.memberships ?? []).filter(m => m.membership.source_id === sourceId && m.membership.retired_at === null).map(m => vm.snapshot.catalog.groups.find(g => g.group.group_id === m.membership.group_id)?.group.name ?? m.membership.group_id))].join('、') || '尚未加入组';

  // 一个来源的用量视图（GLM：5 小时 + 每周；火山：每日额度）。组页成员卡与来源页卡共用。
  function usageView(vm, p) {
    const s = p.source, obs = vm.observation.sourceViews.get(s.source_id);
    if (s.type === 'glm') {
      const missing = obs?.state === 'unavailable' ? '查询失败' : obs?.state === 'stale' ? '观察已过期' : '尚未观测';
      const w = key => { const v = obs?.current ? obs[key] : null; return v?.current ? [v.percentage, v.reset_at_ms, missing] : [null, null, v ? '已过重置时刻，等待新观测' : missing]; };
      const [fp, fr, fm] = w('five_hour'), [wp, wr, wm] = w('weekly');
      const box = text('div', null, 'win'); box.append(win('5 小时窗口', fp, fr, fm), win('每周窗口', wp, wr, wm));
      const exhausted = (fp ?? 0) >= 100 || (wp ?? 0) >= 100, unknown = fp === null || wp === null;
      return {box, exhausted, unknown, unknownLabel: '未知'};
    }
    const v = p.volcano, r = v.runtime, limit = v.policy.daily_token_limit;
    if (!r) { const box = text('div', null, 'win one'); box.append(win('每日额度', null, null, '周期尚未初始化，不能视为零用量')); return {box, exhausted: false, unknown: true, unknownLabel: '未初始化'}; }
    const exhausted = r.error_exhausted || r.limit_exhausted;
    if (r.coverage_end === null) { const box = text('div', null, 'win one'); box.append(win('每日额度', null, null, '本周期尚无覆盖记录，累计数不能视为零用量')); return {box, exhausted, unknown: true, unknownLabel: '无覆盖'}; }
    let total; try { total = safeAdd(r.used_input, r.used_output); } catch { total = null; }
    const box = text('div', null, 'win one');
    const pct = limit && total !== null ? Math.min(100, total / limit * 100) : null;
    box.append(win('每日额度', pct, r.next_reset_at * 1000, total === null ? '总数超出安全范围' : '未设置每日上界，无法换算百分比', `${kfmt(total)} / ${kfmt(limit)} tokens`));
    return {box, exhausted, unknown: total === null, unknownLabel: '未知'};  // 未设每日上界只是换算不了百分比，来源本身可用
  }
  function sourceDetails(vm, p, key) {
    const s = p.source, more = text('details', null, 'more'); more.dataset.key = key; more.append(text('summary', '详情'));
    const archived = s.retired_at !== null || p.retire_requested, intent = archived ? '归档中或已归档' : s.manual_paused ? '暂停意图' : '启用意图';
    if (s.type === 'volcano' && p.volcano.runtime) {
      const v = p.volcano, r = v.runtime; let total; try { total = safeAdd(r.used_input, r.used_output); } catch { total = '总数超出安全范围'; }
      more.append(
        text('p', `每日 Token 上界 ${v.policy.daily_token_limit ?? '未设置'} · 失败阈值 ${v.policy.failure_threshold} · 重置分钟 ${v.policy.reset_minute} / UTC 偏移 ${v.policy.utc_offset_minutes} 分钟`),
        text('p', `已保存累计输入 ${r.used_input} / 输出 ${r.used_output} / 总计 ${total} · 下次重置 ${formatTime(r.next_reset_at)}`),
        text('p', r.coverage_end === null ? '尚无覆盖记录。' : `周期起点 ${formatTime(r.period_start)} · 已记录覆盖末端 ${formatTime(r.coverage_end)}；不保证供应商日志完整。`),
        text('p', `错误判定 ${r.error_exhausted ? '是' : '否'} · 上界判定 ${r.limit_exhausted ? '是' : '否'} · 配置版本 ${s.config_revision} · 人工配置：${intent}`));
    } else more.append(text('p', `配置版本 ${s.config_revision} · ${p.model_count} 个模型 · 人工配置：${intent}（不等于所有远端成员已同步）`));
    return more;
  }
  function sourceMenuGroups(vm, p, withArchive) {
    const s = p.source, archived = s.retired_at !== null || p.retire_requested, act = {usage: [], config: [], run: [], archive: []};
    act.config.push(action('models', archived ? '只读查看历史模型' : '查看或编辑模型', s.source_id, vm.workBusy || !vm.observation.fresh), action('discoverModels', '重新发现模型', s.source_id, vm.readOnly || archived));
    act.run.push(action('setSourcePaused', s.manual_paused ? '恢复来源' : '暂停来源', s.source_id, vm.readOnly || archived));
    if (withArchive) act.archive.push(action('retireSource', '归档来源', s.source_id, vm.readOnly || archived));
    if (s.type === 'volcano' && p.volcano.runtime) {
      if (withArchive) act.usage.push(button(vm.sourceUsageId === s.source_id ? '收起分模型用量' : '展开分模型用量', () => handlers.sourceUsage(vm.sourceUsageId === s.source_id ? null : s.source_id), false, `sourceUsage:${s.source_id}`));
      act.config.push(action('setVolcanoPolicy', '设置每日策略', s.source_id, vm.readOnly || archived));
      act.run.push(action('clearError', '撤销错误耗尽判定', s.source_id, vm.readOnly || archived));
    }
    return [{label: '用量', items: act.usage}, {label: '配置', items: act.config}, {label: '运行', items: act.run}, {label: '归档', items: act.archive}];
  }

  // —— 来源页：全部来源，一源一卡 ——
  function sources(vm) {
    const root = byId('sources-content');
    const views = [...(vm.observation.sourceViews ?? new Map())].map(([k, o]) => [k, o?.state ?? null, o?.current ?? null, o?.level ?? null, o?.five_hour ?? null, o?.weekly ?? null]);
    const stamp = JSON.stringify([vm.snapshot?.catalog, views, vm.observation.fresh, vm.sourceFilter, vm.sourcesPage, vm.sourceUsageId, vm.readOnly, vm.workBusy]);
    const previousDetail = byId('source-usage-content');
    guard(root, stamp, root => {
      const all = (vm.snapshot?.catalog.sources ?? []).filter(p => (!vm.sourceFilter.type || p.source.type === vm.sourceFilter.type) && p.source.name.includes(vm.sourceFilter.name));
      const start = Math.min(vm.sourcesPage, Math.max(0, Math.ceil(all.length / 25) - 1)) * 25;
      root.append(toolbar(action('createSource', '添加来源', null, vm.readOnly, 'sbtn')));
      if (all.length > 25) root.append(pager(vm.sourcesPage, all.length, 25, handlers.sourcePage, 'sources'));
      const grid = text('div', null, 'grid'); root.append(grid);
      for (const p of all.slice(start, start + 25)) {
        const s = p.source, archived = s.retired_at !== null || p.retire_requested, view = usageView(vm, p);
        const card = text('article', null, 'card');
        if (!vm.observation.fresh) card.classList.add('stale');
        if (view.exhausted || archived) card.classList.add('dead');
        const head = text('div', null, 'chead');
        const state = archived ? tier('已归档', 'standby') : s.manual_paused ? tier('暂停', 'unknown') : view.exhausted ? tier('耗尽', 'exhausted') : view.unknown ? tier(view.unknownLabel, 'unknown') : tier('可用', 'active');
        head.append(text('h3', s.name, 'name'), state, span(`${labels[s.type]} · #${s.source_id}`, 'cid'), menu(`source:${s.source_id}`, sourceMenuGroups(vm, p, true)));
        card.append(head);
        if (s.note) card.append(text('div', s.note, 'note'));
        card.append(view.box);
        const meta = text('div', null, 'meta');
        meta.append(span(`属于：${groupNames(vm, s.source_id)}`), span(`${p.model_count} 个模型`));
        if (s.type === 'glm') { const obs = vm.observation.sourceViews.get(s.source_id); meta.append(span(`套餐 ${obs?.level ?? '未知'}`), live('age', s.source_id)); }
        else if (p.volcano.runtime) meta.append(span(`连续失败 ${p.volcano.runtime.consecutive_failures}`));
        card.append(meta, sourceDetails(vm, p, `source-more:${s.source_id}`));
        if (vm.sourceUsageId === s.source_id) { const detail = previousDetail?.dataset.sourceId === s.source_id ? previousDetail : text('div'); detail.id = 'source-usage-content'; detail.dataset.sourceId = s.source_id; card.append(detail); }
        grid.append(card);
      }
      if (!all.length) root.append(text('p', vm.snapshot ? (vm.snapshot.catalog.sources.length ? '没有匹配此类型/名称的来源。' : '还没有来源，请添加。') : '等待目录快照。', 'empty'));
    });
  }

  // —— 组页（默认主页）：一次看一个组；成员卡 = 成员背后来源的用量 + 本组内的状态 ——
  function memberCard(vm, group, p, member, isSelected, isPinned) {
    const m = member.membership, s = p.source, retired = m.retired_at !== null || member.retire_requested, view = usageView(vm, p);
    const card = text('article', null, 'card');
    if (!vm.observation.fresh) card.classList.add('stale');
    const unexpectedOff = m.observed_status !== null && m.observed_status !== 1 && member.status_directive === 'enabled';
    if (isSelected && !retired) card.classList.add('act');
    if (view.exhausted || retired) card.classList.add('dead');
    if (unexpectedOff) card.classList.add('off');
    const head = text('div', null, 'chead');
    const state = retired ? tier('已移除', 'standby') : s.manual_paused ? tier('暂停', 'unknown') : isSelected ? tier('活动', 'active') : view.exhausted ? tier('耗尽', 'exhausted') : view.unknown ? tier(view.unknownLabel, 'unknown') : tier('备用', 'standby');
    head.append(text('h3', s.name, 'name'), state, span(m.channel_id === null ? '渠道未创建' : `渠道 #${m.channel_id}`, 'cid'));
    if (m.observed_status === 1) head.append(badge('启用', 'on'));
    else if (unexpectedOff) head.append(badge('已被 new-api 禁用', 'off'));
    else if (m.observed_status !== null) head.append(badge('已停用', 'off'));
    if (isSelected && view.exhausted) head.append(badge('耗尽', 'off'));
    else if (isSelected && view.unknown) head.append(badge(`用量${view.unknownLabel}`, 'info'));
    if (member.dirty) head.append(badge('待同步', 'info'));
    if (group.type === 'glm' && (vm.observation.groupViews.get(group.group_id)?.imminent_members ?? []).includes(m.membership_id)) head.append(badge('周额度临期', 'info'));
    if (group.type === 'glm' && !retired) head.append(isPinned ? action('unpin', '📌 已固定 · 取消', group.group_id, vm.readOnly, 'pbtn on') : action('setGroupPin', '📌 固定到这把', m.membership_id, vm.readOnly, 'pbtn'));
    const sourceGroups = sourceMenuGroups(vm, p, false).filter(g => g.label !== '用量' && g.label !== '归档').map(g => ({...g, label: `来源 · ${g.label}`}));
    head.append(menu(`member:${m.membership_id}`, [{label: '成员', items: [action('ensureMember', '显式启用 / Ensure', m.membership_id, vm.readOnly || retired), action('retireMember', '移除成员', m.membership_id, vm.readOnly || retired)]}, ...sourceGroups]));
    card.append(head);
    if (member.error_code) card.append(text('div', `同步错误：${member.error_code}`, 'err'));
    if (unexpectedOff) card.append(text('div', `status=${m.observed_status} · priority 不起作用，流量不会来这个来源`, 'err'));
    card.append(view.box);
    const meta = text('div', null, 'meta'), prio = span('priority '); prio.append(text('b', String(m.desired_priority)));
    if (m.observed_priority !== null && m.observed_priority !== m.desired_priority) prio.append(span(`（new-api 侧是 ${m.observed_priority}，不一致！）`, 'warn'));
    meta.append(prio);
    if (m.lifecycle !== 'ready') meta.append(span(`生命周期 ${m.lifecycle}`));
    meta.append(span(`${p.model_count} 个模型`));
    if (s.type === 'glm') { const obs = vm.observation.sourceViews.get(s.source_id); meta.append(span(`套餐 ${obs?.level ?? '未知'}`), live('age', s.source_id)); }
    else if (p.volcano.runtime) meta.append(span(`连续失败 ${p.volcano.runtime.consecutive_failures}`));
    card.append(meta);
    return card;
  }
  function identities(vm, root, group) {
    // 访问 Key 按组归属；不属于当前组（含历史/未归组）的 Key 收在下方折叠表里，操作一个不少。没有组时列出全部。
    root.append(text('h2', group ? `${group.name} · 访问 Key` : '访问 Key'));
    const card = text('div', null, 'card');
    if (vm.config?.public_api_base_url) {
      const base = vm.config.public_api_base_url;
      card.append(text('p', `每个访问 Key 只属于一个组，同时支持 OpenAI 兼容（${base}/v1）与 Claude（${base}）两种调用格式。`, 'note'),
        toolbar(button('复制 OpenAI 兼容地址', () => handlers.copyAddress(`${base}/v1`), false, 'copy:openai', 'pbtn'), button('复制 Claude 地址', () => handlers.copyAddress(base), false, 'copy:claude', 'pbtn')));
    } else card.append(text('p', '调用地址未配置；管理面板地址不是调用地址。', 'warn'));
    const page = vm.identities.data;
    card.append(toolbar(...[group ? action('createToken', '生成访问 Key', group.group_id, vm.readOnly, 'pbtn') : null, button('刷新本地身份', () => handlers.identities(vm.identities.after), vm.workBusy, 'identities:refresh', 'pbtn'), button('第一页', () => handlers.identities(null), vm.workBusy, 'identities:first', 'pbtn'), button('下一页', () => handlers.identities(page.next_after), vm.workBusy || !page?.next_after, 'identities:next', 'pbtn'), action('registerToken', '登记已有 Key', null, vm.readOnly, 'pbtn')].filter(Boolean)));
    if (vm.identities.error) card.append(text('p', vm.identities.error, 'warn'));
    const rowsOf = items => items.map(i => {
      const native = matchNative(i, [...vm.nativePages.values()].map(n => n.data)), deleted = i.deleted_at !== null;
      const status = badge(deleted ? '历史已删除' : native ? `原生 ${native.status}` : '需核验', deleted ? 'off' : native ? 'on' : 'info');
      const ops = menu(`identity:${i.token_id}`, [{label: '访问', items: [action('reveal', '复制访问 Key', i.token_id, vm.readOnly || deleted || i.native_token_id === null), button('查看用量', () => handlers.identityUsage(i), i.native_token_id === null, `usage:${i.token_id}`)]}, {label: '状态', items: [action('enableToken', '启用', i.token_id, vm.readOnly || deleted), action('disableToken', '停用', i.token_id, vm.readOnly || deleted)]}, {label: '危险', items: [action('deleteToken', '删除', i.token_id, vm.readOnly || deleted)]}]);
      return [i.display_name, status, `local #${i.token_id} / native ${i.native_token_id ?? '待创建'} · ${i.explicit_group || '未归组'} · ${i.owner_scope} · 版本 ${i.revision}`, ops];
    });
    const header = ['名称', '状态', '本地 / 原生 · 分组', '操作'];
    if (page) {
      const mine = group ? page.items.filter(i => i.explicit_group === group.newapi_group) : page.items, others = group ? page.items.filter(i => i.explicit_group !== group.newapi_group) : [];
      if (mine.length) card.append(table(header, rowsOf(mine)));
      else card.append(text('p', group ? '本页没有属于这个组的访问 Key。' : '本页没有访问 Key。', 'empty'));
      if (others.length) {
        const rest = text('details', null, 'more'); rest.dataset.key = 'other-keys';
        rest.append(text('summary', `本页其他组或未归组的访问 Key（${others.length}）`), table(header, rowsOf(others)));
        card.append(rest);
      }
    } else card.append(text('p', '尚未读取本地身份。', 'empty'));
    root.append(card);
    const more = text('details', null, 'more'); more.dataset.key = 'native-pages'; more.append(text('summary', '原生 Key 页（高级）'));
    for (const scope of ['issuer', 'legacy_admin']) {
      const p = vm.nativePages.get(scope), data = p?.data;
      more.append(text('h3', `原生 Key 页 · ${scope}`));
      more.append(toolbar(button('读取本页', () => handlers.native(scope, p?.page ?? 1), vm.workBusy, `${scope}:refresh`, 'pbtn'), button('上一页', () => handlers.native(scope, Math.max(1, (p?.page ?? 1) - 1)), vm.workBusy || (p?.page ?? 1) <= 1, `${scope}:previous`, 'pbtn'), span(`第 ${p?.page ?? 1} 页`), button('下一页', () => handlers.native(scope, (p?.page ?? 1) + 1), vm.workBusy || (p?.page ?? 1) >= 100 || !data || data.page * data.page_size >= data.total, `${scope}:next`, 'pbtn')));
      if (p?.error) more.append(text('p', p.error, 'warn'));
      if (data) {
        if (data.page !== p.page) more.append(text('p', `请求第 ${p.page} 页；下面显示第 ${data.page} 页的旧观察，不是新页结果。`, 'warn'));
        more.append(text('p', `本页收到于 ${p.receivedLabel}；这是读取时间，不是账号创建时间。`));
        more.append(table(['原生 ID / 名称', '组 / 状态', '其他限制'], data.items.map(i => [`#${i.token_id} · ${i.name} · owner ${i.owner_user_id} · ${i.masked_key}`, `${i.explicit_group} → ${i.effective_group} · ${i.status}`, `quota 余额 ${i.remaining_quota} / 已用 ${i.used_quota}（不是 Token 统计） · 模型限制 ${i.model_limits_enabled ? i.model_limits : '无'} · 跨组重试 ${i.cross_group_retry ? '开启' : '关闭'}`])));
      } else more.append(text('p', '尚未读取，不据此判定 Key 不存在。', 'empty'));
    }
    root.append(more);
  }
  // 组卡片排：钉住 → 当前组 → 上小时用量降序，最多 5 张；第 6 张是「更多 / 新建」。
  const more = {query: '', first: 0, size: 5};  // 「更多」菜单的搜索词、当前页首项、每页条数；跨区块重建保留
  const moreSize = pop => { const top = pop.getBoundingClientRect().top, view = document.defaultView?.innerHeight ?? 0; return Math.max(5, Math.floor((view - top - 150) / 42)); };
  function moreList(vm, pop, others, relayout = false) {
    const list = pop.querySelector('.glist'), pager = pop.querySelector('.gpager');
    const q = more.query.trim().toLowerCase(), hits = q ? others.filter(o => o.name.toLowerCase().includes(q) || o.newapi.toLowerCase().includes(q)) : others;
    // 每页条数只在打开菜单 / 窗口尺寸变化时按可用高度重算；翻页、滚动不改变它。
    if (relayout && pop.isConnected && pop.getBoundingClientRect().height > 0) more.size = moreSize(pop);
    const size = more.size, page = Math.min(Math.floor(more.first / size), Math.max(0, Math.ceil(hits.length / size) - 1)), pages = Math.max(1, Math.ceil(hits.length / size));
    more.first = page * size;
    list.replaceChildren(...hits.slice(page * size, page * size + size).map(o => {
      const b = button(null, () => handlers.selectGroup(o.id), false, `groups:more:${o.id}`, 'menu-item grow'); b.setAttribute('aria-label', `${o.name} · ${o.count} 个成员`);
      b.addEventListener('click', () => { const wrap = pop.closest('.menu'); wrap?.classList.remove('open'); wrap?.querySelector('.menu-btn')?.setAttribute('aria-expanded', 'false'); }, {capture: true});
      b.append(span(o.name, 'gr-name'), span(`${labels[o.type]} · ${o.count} 成员 · ${o.usage}`, 'gr-meta')); return b;
    }));
    if (!hits.length) list.append(text('div', '没有匹配的组。', 'empty'));
    pager.replaceChildren(button('上一页', () => { more.first = Math.max(0, more.first - size); moreList(vm, pop, others); }, page === 0, 'groups:more:prev', 'pbtn'), span(`第 ${page + 1} / ${pages} 页`), button('下一页', () => { more.first += size; moreList(vm, pop, others); }, page + 1 >= pages, 'groups:more:next', 'pbtn'));
  }
  function moreTile(vm, others) {
    const wrap = text('div', null, 'menu gtile gmore-wrap'); wrap.dataset.key = 'groups:more';
    const trigger = text('button', null, 'menu-btn gmore'); trigger.type = 'button'; trigger.dataset.focusKey = 'groups:more';
    trigger.setAttribute('aria-haspopup', 'true'); trigger.setAttribute('aria-expanded', 'false');
    trigger.append(span(`更多（${others.length}）`, 'gt-name'), span('搜索其余组 · 新建组', 'gt-line'));
    const pop = text('div', null, 'menu-pop gmore-pop');
    const create = action('createGroup', '＋ 新建组', null, false, 'menu-item'); create.dataset.write = '1';
    create.addEventListener('click', () => { wrap.classList.remove('open'); trigger.setAttribute('aria-expanded', 'false'); }, {capture: true});
    const search = text('input', null, 'gsearch'); search.type = 'search'; search.placeholder = '搜索组名 / new-api 分组'; search.setAttribute('aria-label', '搜索组'); search.autocomplete = 'off';
    search.value = more.query; search.dataset.focusKey = 'groups:more:search';
    search.addEventListener('input', () => { more.query = search.value; more.first = 0; moreList(vm, pop, others); });
    pop.append(create, search, text('div', null, 'glist'), text('div', null, 'gpager toolbar'));
    trigger.addEventListener('click', event => { event.stopPropagation(); collapse(wrap); const open = wrap.classList.toggle('open'); trigger.setAttribute('aria-expanded', String(open)); if (open) { moreList(vm, pop, others, true); search.focus({preventScroll: true}); } });
    wrap.append(trigger, pop); moreList(vm, pop, others);
    return wrap;
  }
  document.defaultView?.addEventListener('resize', () => { const pop = document.querySelector('.gmore-wrap.open .gmore-pop'); if (pop) pop.dispatchEvent(new Event('qt-relayout')); });
  function groupTiles(vm, root) {
    const snap = vm.snapshot, catalog = snap.catalog, byIdGroup = new Map(catalog.groups.map(p => [p.group.group_id, p]));
    const sourceName = new Map(catalog.sources.map(p => [p.source.source_id, p.source.name]));
    const info = id => {
      const p = byIdGroup.get(id), g = p.group, count = catalog.memberships.filter(m => m.membership.group_id === id && m.membership.retired_at === null).length;
      const m = catalog.memberships.find(x => x.membership.membership_id === p.selected_member_id);
      const total = vm.rank?.totals ? vm.rank.totals.get(id) ?? 0 : null;
      return {id, name: g.name, type: g.type, newapi: g.newapi_group, count, selected: m ? sourceName.get(m.membership.source_id) ?? null : null, usage: total === null ? '上小时 —' : `上小时 ${kfmt(total)}`};
    };
    const order = vm.groupOrder.filter(id => byIdGroup.has(id)), top = order.slice(0, 5).map(info), others = order.slice(5).map(info);
    const tiles = text('div', null, 'gtiles'); tiles.setAttribute('role', 'group'); tiles.setAttribute('aria-label', '选择组');
    const full = vm.groupPins.length >= vm.maxGroupPins;
    for (const t of top) {
      const tile = text('div', null, t.id === vm.selectedGroup ? 'gtile on' : 'gtile'), pinned = vm.groupPins.includes(t.id);
      const main = button(null, () => handlers.selectGroup(t.id), false, `group:${t.id}`, 'gtile-main');
      main.setAttribute('aria-label', `${t.name} · ${t.count} 个成员`); main.setAttribute('aria-pressed', String(t.id === vm.selectedGroup));
      main.append(span(t.name, 'gt-name'), span(`${labels[t.type]} · ${t.count} 成员`, 'gt-line'), span(t.selected ? `→ ${t.selected}` : '→ 无选择', t.selected ? 'gt-line gt-sel' : 'gt-line'), span(t.usage, 'gt-line'));
      const pin = button('📌', () => handlers.pinGroup(t.id), !pinned && full, `groupPin:${t.id}`, pinned ? 'gpin on' : 'gpin');
      pin.setAttribute('aria-label', pinned ? `取消钉住 ${t.name}` : `钉住 ${t.name}`); pin.setAttribute('aria-pressed', String(pinned));
      pin.title = pinned ? '取消钉住' : full ? `最多钉住 ${vm.maxGroupPins} 个` : '钉在最前面';
      tile.append(main, pin); tiles.append(tile);
    }
    if (others.length) { const tile = moreTile(vm, others); tile.querySelector('.gmore-pop').addEventListener('qt-relayout', () => moreList(vm, tile.querySelector('.gmore-pop'), others, true)); tiles.append(tile); }
    else { const tile = text('div', null, 'gtile gnew'), b = action('createGroup', '＋ 新建组', null, false, 'gtile-main gnew-btn'); b.dataset.write = '1'; tile.append(b); tiles.append(tile); }
    root.append(tiles);
  }
  function groups(vm) {
    const root = byId('groups-content');
    const views = [...(vm.observation.sourceViews ?? new Map())].map(([k, o]) => [k, o?.state ?? null, o?.current ?? null, o?.level ?? null, o?.five_hour ?? null, o?.weekly ?? null]);
    const groupViews = [...(vm.observation.groupViews ?? new Map())];
    const nativeStamp = [...vm.nativePages].map(([k, p]) => [k, p.page, p.error, p.receivedLabel ?? null]);
    let tilesRoot = byId('group-tiles'), bodyRoot = byId('group-body');
    if (!tilesRoot) { tilesRoot = text('div'); tilesRoot.id = 'group-tiles'; bodyRoot = text('div'); bodyRoot.id = 'group-body'; root.replaceChildren(tilesRoot, bodyRoot); }
    // 组卡片排只随「组本身」变化重建（名称、成员数、当前选择、排行、钉住、选中组）；写按钮的可用状态原地同步。
    const catalog = vm.snapshot?.catalog, sourceName = new Map((catalog?.sources ?? []).map(p => [p.source.source_id, p.source.name]));
    const tileModel = (catalog?.groups ?? []).map(p => [p.group.group_id, p.group.name, p.group.type, p.group.newapi_group,
      catalog.memberships.filter(m => m.membership.group_id === p.group.group_id && m.membership.retired_at === null).length,
      sourceName.get(catalog.memberships.find(m => m.membership.membership_id === p.selected_member_id)?.membership.source_id) ?? null]);
    const rankStamp = vm.rank?.totals ? [vm.rank.hour, [...vm.rank.totals]] : null;
    guard(tilesRoot, JSON.stringify([tileModel, vm.groupOrder, vm.groupPins, vm.maxGroupPins, rankStamp, vm.selectedGroup]), r => { if (tileModel.length) groupTiles(vm, r); });
    for (const b of tilesRoot.querySelectorAll('[data-write]')) b.disabled = vm.readOnly;
    const stamp = JSON.stringify([catalog, views, groupViews, vm.observation.fresh, vm.selectedGroup, vm.groupUsageMode, vm.identities.data, vm.identities.error, nativeStamp, vm.config, vm.readOnly, vm.workBusy]);
    const previousUsage = byId('group-usage-content');
    guard(bodyRoot, stamp, root => {
      const all = vm.snapshot?.catalog.groups ?? [];
      const current = all.find(p => p.group.group_id === vm.selectedGroup);
      if (!current) {
        if (vm.snapshot) root.append(toolbar(action('createGroup', '＋ 新建组', null, vm.readOnly, 'sbtn')));
        root.append(text('p', vm.snapshot ? '还没有组，先在「来源」页添加来源，再创建组。' : '等待目录快照。', 'empty')); if (vm.snapshot) identities(vm, root, null); return;
      }
      const g = current.group, a = vm.observation.groupViews.get(g.group_id);
      const sources = new Map(vm.snapshot.catalog.sources.map(p => [p.source.source_id, p]));
      const members = vm.snapshot.catalog.memberships.filter(p => p.membership.group_id === g.group_id).sort((x, y) => x.membership.order - y.membership.order);
      // 组标题：一行轻量标题（组名 + new-api 分组 / 核验状态 / ⋯ 组操作），当前选择已在组卡片与绿框成员卡上，不重复。
      const head = text('section', null, 'ghead'), right = text('div', null, 'ghead-right');
      head.setAttribute('aria-label', `当前组 ${g.name}`);
      right.append(span(`new-api 分组 ${g.newapi_group}`, 'ghead-meta'), badge(g.ready ? '已核验' : '待核验', g.ready ? 'on' : 'info'));
      const groupMenu = [{label: '核验', items: [action('checkGroup', '核验配置', g.group_id, vm.readOnly), action('registerGroup', '注册并核验', g.group_id, vm.readOnly)]},
        {label: '成员', items: [action('planMember', '把来源加入组', g.group_id, vm.readOnly), g.type === 'glm' && current.pinned_member_id ? action('unpin', '取消固定', g.group_id, vm.readOnly) : null]},
        {label: '引用', items: [action('groupReferences', '本地引用', g.group_id, vm.workBusy)]}];
      right.append(menu(`group:${g.group_id}`, groupMenu));
      head.append(text('h2', g.name, 'gname'), right);
      root.append(head);
      // 横幅：降级档 / pin 自动解除 / 未核验 / 旧评估
      if (current.regime === 'degraded') root.append(text('div', '降级档：本组全部来源都已越过预防线。正在把活动来源用到耗尽线，再流转到还有余量的那个。', 'ban ban-warn'));
      if (a?.pin_release) { const released = members.find(p => p.membership.membership_id === a.pin_release.membership_id); root.append(text('div', `📌 ${released ? sources.get(released.membership.source_id)?.source.name : `成员 #${a.pin_release.membership_id}`} 的固定已自动解除：用量 ${a.pin_release.pct}% 越过了合格线 ${a.pin_release.limit}%，已回到自动选择。`, 'ban ban-info')); }
      if (!g.ready) { const ban = text('div', null, 'ban ban-warn'); ban.append(span('本组尚未核验：new-api 分组或渠道绑定未确认，访问 Key 可能无法路由。'), action('checkGroup', '核验配置', g.group_id, vm.readOnly, 'pbtn')); root.append(ban); }
      if (g.type === 'glm' && a && !a.current) root.append(text('div', '本组评估已过期或尚未完成；下面的选择仅供参考。', 'ban ban-warn'));
      root.append(text('h2', `${g.name} · 成员`));
      const grid = text('div', null, 'grid'); root.append(grid);
      for (const member of members) {
        const p = sources.get(member.membership.source_id); if (!p) continue;
        grid.append(memberCard(vm, g, p, member, member.membership.membership_id === current.selected_member_id, member.membership.membership_id === current.pinned_member_id));
      }
      if (!members.length) grid.append(text('p', '本组还没有成员。用组名右侧 ⋯ →「把来源加入组」添加。', 'empty'));
      // 本组用量（单次按组过滤的查询；容器跨重建复用，图只在新结果到达时重画）
      const usageRow = text('div', null, 'hrow'), mode = vm.groupUsageMode;
      usageRow.append(text('h2', `${g.name} · ${mode === '30d' ? '近 30 天' : '近 24 小时'}用量`));
      const seg = text('div', null, 'seg');
      for (const [value, label] of [['24h', '近 24 小时'], ['30d', '近 30 天']]) { const b = button(label, () => handlers.groupUsageMode(value), false, `groupUsage:${value}`); if (value === mode) b.classList.add('on'); b.setAttribute('aria-pressed', String(value === mode)); seg.append(b); }
      usageRow.append(seg); root.append(usageRow);
      const usage = previousUsage?.dataset.groupId === g.group_id ? previousUsage : text('div');
      usage.id = 'group-usage-content'; usage.dataset.groupId = g.group_id; root.append(usage);
      identities(vm, root, g);
    });
  }

  // —— 用量报告（组页精简 / 数据页完整）：main 的柱状图 + 按模型表 ——
  function bars(chart, query, note = null) {
    const box = text('div', null, 'card'), plot = text('div', null, 'chart'), model = barModel(chart), offset = query.utc_offset_minutes, daily = query.grain === 'day';
    if (model.length > 60) plot.classList.add('dense');
    plot.setAttribute('role', 'img'); plot.setAttribute('aria-label', '按时间的 Token 用量柱状图，精确数值见下方表格');
    const stamp = sec => { const t = formatTime(sec, 's', offset); return daily ? t.slice(5, 10) : t.slice(5, 16); };
    for (const b of model) {
      const bar = text('div', null, b.zero ? 'cb zero' : 'cb'); bar.style.height = `${b.height}%`;
      bar.append(span(`${stamp(b.from)} · ${kfmt(b.total)} tokens · ${b.rows} 次`)); plot.append(bar);
    }
    const axis = text('div', null, 'xax');
    if (model.length) for (const i of [...new Set([0, Math.floor((model.length - 1) / 2), model.length - 1])]) axis.append(span(stamp(model[i].from)));
    box.append(plot, axis); if (note) box.append(note);
    return box;
  }
  function modelRows(chart, onlyThis) {
    const peak = Math.max(1, ...chart.models.map(m => m.usage.total));
    return chart.models.map(m => {
      const bar = text('div', null, 'bar thin'), fill = text('div', null, 'fill'); fill.style.width = `${m.usage.total / peak * 100}%`; bar.append(fill);
      const row = [m.model === null ? '未归属模型（null）' : m.model, kfmt(m.usage.total), `${kfmt(m.usage.input)} / ${kfmt(m.usage.output)}`, String(m.usage.consumption_rows), bar];
      if (onlyThis) row.push(button('只看此模型', () => onlyThis(m), false, `onlyModel:${m.model ?? ''}`, 'pbtn'));
      return row;
    });
  }

  function chips(vm) {
    const root = byId('chips'), snap = vm.snapshot;
    // 只放当前组的选择（各组的选择已在组卡片上），组再多状态条也不会变长。
    const groupsState = (snap?.catalog.groups ?? []).filter(p => p.group.group_id === vm.selectedGroup).map(p => {
      const m = snap.catalog.memberships.find(x => x.membership.membership_id === p.selected_member_id);
      return [p.group.group_id, p.group.name, m ? snap.catalog.sources.find(s => s.source.source_id === m.membership.source_id)?.source.name ?? null : null, p.pinned_member_id !== null];
    });
    const stamp = JSON.stringify([!!snap, snap?.backend_ready, vm.observation.fresh, groupsState, vm.config?.public_api_base_url ?? null, vm.config?.dry_run ?? null]);
    guard(root, stamp, root => {
      const chip = (...children) => { const c = text('div', null, 'chip'); c.append(...children); return c; };
      const ok = snap && vm.observation.fresh && snap.backend_ready, dot = span(null, `dot ${!snap ? '' : ok ? 'ok' : 'bad'}`);
      root.append(chip(dot, span('后端', 'k'), span(!snap ? '连接中' : !vm.observation.fresh ? '快照过期' : snap.backend_ready ? '就绪' : '未就绪', 'v')));
      for (const [, name, selected, pinned] of groupsState) root.append(chip(span(`当前组 ${name}`, 'k'), span(`${selected ?? '无选择'}${pinned ? ' 📌' : ''}`, selected ? 'v ok' : 'v')));
      if (vm.config?.public_api_base_url) {
        const address = `${vm.config.public_api_base_url}/v1`, copy = button(address, () => handlers.copyAddress(address), false, 'chip:copy', 'copy'); copy.title = '点击复制';
        root.append(chip(span('客户端连接', 'k'), copy));
      }
      if (vm.config?.dry_run) { const c = chip(span('dry run', 'v'), span('写入与敏感复制已禁用', 'k')); c.classList.add('warn'); root.append(c); }
    });
  }
  function operations(vm) {
    const root = byId('operations');
    const stamp = JSON.stringify([vm.operations, vm.storageError, vm.readOnly, vm.workBusy]);
    guard(root, stamp, root => {
      root.append(text('h2', '本标签页操作追踪'));
      root.append(text('p', '只恢复保存的操作 ID；移除追踪不会取消后端操作。标签页存储丢失时无法自动找回全部历史。', 'note'));
      if (vm.storageError) root.append(text('p', vm.storageError, 'warn'));
      root.append(toolbar(button('输入操作 ID 恢复查询', handlers.restoreOperation, false, 'operations:restore', 'pbtn')));
      for (const item of vm.operations) {
        const r = item.receipt, card = text('article', null, 'card');
        const status = r ? (r.phase === 'succeeded' && r.total_targets === 0 ? '设置已保存' : `${r.phase} · 确认 ${r.confirmed_targets}/${r.total_targets}`) : '结果待核对';
        card.append(text('code', item.id), text('p', status), text('p', item.error ?? (item.tracking ? '追踪中' : '自动追踪已暂停，请手动继续'), item.error ? 'warn' : 'note'));
        card.append(toolbar(button('继续查询', () => handlers.operation('continue', item.id), vm.workBusy, `op:continue:${item.id}`, 'pbtn'), button('显式重试原操作', () => handlers.operation('retry', item.id), vm.readOnly || !r || ['succeeded', 'superseded'].includes(r.phase), `op:retry:${item.id}`, 'pbtn'), button('同 ID 同载荷再次提交', () => handlers.operation('resubmit', item.id), vm.readOnly || !!r || !item.canResubmit, `op:resubmit:${item.id}`, 'pbtn'), button('移除追踪', () => handlers.operation('dismiss', item.id), false, `op:dismiss:${item.id}`, 'pbtn')));
        root.append(card);
      }
    });
  }
  function frame(vm) {
    // Step U4: 动态值只进入文本/value；刷新不重建正在编辑的表单；区块内容不变不重建（guard）。
    const activeKey = document.activeElement?.dataset.focusKey;
    byId('compatibility').hidden = true;
    // 快照过期不弹横幅：只把顶部这行刷新状态变黄（同时界面进入只读）。
    const ageText = vm.snapshot && Number.isFinite(vm.observation.age) ? `快照 ${Math.max(0, Math.floor(vm.observation.age / 1000))} 秒前` : null;
    const connection = vm.observation.fresh ? `每 5 秒刷新 · ${ageText}` : ageText ? `每 5 秒刷新 · ${ageText} · 已过期，暂时只读` : '正在连接后端…';
    const status = byId('connection');
    if (status.textContent !== connection) status.textContent = connection;
    status.classList.toggle('stale', !vm.observation.fresh);
    byId('refresh-state').disabled = vm.stateBusy;
    const toggle = byId('toggle-operations'), label = `操作进度（${vm.operations.length}）`;
    if (toggle.textContent !== label) toggle.textContent = label;
    toggle.setAttribute('aria-expanded', String(vm.operationsOpen));
    byId('operations').hidden = !vm.operationsOpen;
    for (const page of ['groups', 'sources', 'data']) {
      byId(`page-${page}`).hidden = page !== vm.page;
      const b = document.querySelector(`button[data-page="${page}"]`);
      if (page === vm.page) b.setAttribute('aria-current', 'page'); else b.removeAttribute('aria-current');
    }
    const notices = [vm.notice ? ['ban ban-info', vm.notice] : null, vm.config?.dry_run ? ['ban ban-warn', 'Dry run：写入与敏感复制已禁用。'] : null].filter(Boolean);
    guard(byId('notice'), JSON.stringify(notices), root => { for (const [cls, message] of notices) root.append(text('div', message, cls)); });
    chips(vm);
    if (vm.page === 'sources') sources(vm); if (vm.page === 'groups') groups(vm);
    if (vm.operationsOpen) operations(vm);
    updateLive(vm);
    if (activeKey && document.activeElement?.dataset.focusKey !== activeKey) for (const element of document.querySelectorAll('[data-focus-key]')) if (element.dataset.focusKey === activeKey && !element.disabled) { element.focus({ preventScroll: true }); break; }
  }
  function clearForm() {
    const root = byId('editor'); for (const input of root.querySelectorAll('input,textarea,select')) { if (input.type === 'checkbox') input.checked = false; else input.value = ''; }
    root.replaceChildren(); root.hidden = true;
  }
  function form(title, fields, submit, onSubmit, onChange = null) {
    clearForm(); const root = byId('editor'); root.hidden = false; root.append(text('h2', title));
    const f = text('form'); f.autocomplete = 'off'; const inputs = new Map();
    for (const field of fields) {
      if (field.type === 'explanation') { f.append(text('p', field.label, 'warn')); continue; }
      const label = text('label', field.label), input = text(field.type === 'select' ? 'select' : field.type === 'textarea' ? 'textarea' : 'input');
      if (input.tagName === 'INPUT') input.type = field.type ?? 'text';
      input.name = field.name; input.autocomplete = field.type === 'password' ? 'new-password' : 'off';
      if (field.type === 'select') for (const [value, labelText] of field.options) { const option = text('option', labelText); option.value = value; input.append(option); }
      if (field.type === 'checkbox') input.checked = field.value === true; else input.value = field.value ?? '';
      if (field.readOnly) input.readOnly = true;
      if (field.required) input.required = true;
      input.addEventListener('change', () => onChange?.(field.name, input.value));
      inputs.set(field.name, input); label.append(input); f.append(label);
    }
    const warning = text('p', '', 'warn'); warning.id = 'editor-conflict'; warning.hidden = true; f.append(warning);
    const submitButton = text('button', submit, 'sbtn'); submitButton.type = 'submit'; submitButton.dataset.submit = 'true';
    f.append(toolbar(submitButton, button('关闭', () => { clearForm(); handlers.closeEditor(); })));
    f.addEventListener('submit', event => { event.preventDefault(); const values = Object.fromEntries([...inputs].map(([key, input]) => [key, input.type === 'checkbox' ? input.checked : input.value])); onSubmit(values); });
    root.append(f); root.scrollIntoView({ block: 'nearest' }); [...inputs.values()][0]?.focus();
  }
  function conflict(message, disabled) {
    const note = byId('editor-conflict'); if (note) { note.textContent = message ?? ''; note.hidden = !message; }
    const submit = byId('editor').querySelector('[data-submit]'); if (submit) submit.disabled = disabled;
  }
  function clearSecret() { const root = byId('secret'); for (const i of root.querySelectorAll('input,textarea')) i.value = ''; root.replaceChildren(); root.hidden = true; }
  function manualSecret(key, onCopy, onClose) {
    clearSecret(); const root = byId('secret'); root.hidden = false;
    root.append(text('h2', '手动复制访问 Key'), text('p', '剪贴板未确认写入。此处只读显示 30 秒；离开页面会清除。', 'warn'));
    const input = text('textarea'); input.readOnly = true; input.value = key; input.setAttribute('aria-label', '临时访问 Key');
    root.append(input, toolbar(button('复制已取得的 Key', onCopy), button('关闭并清除', onClose))); input.focus(); input.select();
  }
  return { frame, form, clearForm, conflict, clearSecret, manualSecret, table, text, button, toolbar, pager, bars, modelRows, kfmt };
}
