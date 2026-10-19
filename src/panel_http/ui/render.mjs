import { formatTime, matchNative, safeAdd } from './core.mjs';

export function node(document, tag, text = null, className = null) {
  const element = document.createElement(tag);
  if (text !== null) element.textContent = String(text);
  if (className !== null) element.className = className;
  return element;
}
const labels = { glm: 'GLM', volcano: '火山', unknown: '尚未观测', stale: '观察已过期', unavailable: '查询不可用', observed: '已观测' };
export function createRenderer(document, handlers) {
  const byId = id => document.getElementById(id);
  const button = (text, fn, disabled = false, focusKey = '') => {
    const b = node(document, 'button', text); b.type = 'button'; b.disabled = disabled;
    b.dataset.focusKey = focusKey; b.addEventListener('click', fn); return b;
  };
  const text = (tag, value, cls) => node(document, tag, value, cls);
  const toolbar = (...children) => { const box = text('div', null, 'toolbar'); box.append(...children); return box; };
  const action = (name, title, id, disabled) => button(title, () => handlers.action(name, id), disabled, `${name}:${id ?? ''}`);
  // 视觉组件：状态药丸 + 占用仪表。动态宽度/颜色只经 CSSOM（el.style），不用内联 style 属性（受 CSP style-src 'self' 限制）。
  const kfmt = n => typeof n !== 'number' ? String(n ?? '—') : n >= 1e9 ? `${(n / 1e9).toFixed(2)}B` : n >= 1e6 ? `${(n / 1e6).toFixed(2)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}k` : String(n);
  const pill = (label, kind) => text('span', label, `pill pill-${kind}`);
  const gauge = (label, pct, sub = '') => {
    const box = text('div', null, 'gauge'), top = text('div', null, 'gauge-top');
    top.append(text('span', label, 'gauge-label'));
    if (pct == null) {
      top.append(text('span', '未知', 'gauge-pct muted')); box.append(top, text('div', null, 'gauge-track'));
      if (sub) box.append(text('div', sub, 'gauge-sub')); return box;
    }
    const clamped = Math.max(0, Math.min(100, pct)), color = pct >= 90 ? 'var(--bad)' : pct >= 70 ? 'var(--warn)' : 'var(--ok)';
    const value = text('span', `${pct}%`, 'gauge-pct'); value.style.color = color; top.append(value);
    const track = text('div', null, 'gauge-track'), fill = text('div', null, 'gauge-fill');
    fill.style.width = `${clamped}%`; fill.style.background = color; track.append(fill);
    box.append(top, track); if (sub) box.append(text('div', sub, 'gauge-sub'));
    return box;
  };
  const sourceStatus = (p, s, obs) => {
    if (s.retired_at !== null || p.retire_requested) return pill('归档', 'idle');
    if (s.manual_paused) return pill('暂停', 'warn');
    if (s.type === 'volcano') {
      const r = p.volcano?.runtime;
      if (!r) return pill('未初始化', 'idle');
      return r.error_exhausted || r.limit_exhausted ? pill('耗尽', 'bad') : pill('可用', 'ok');
    }
    const st = obs?.state;
    return st === 'observed' ? pill('观测中', 'ok') : st === 'stale' ? pill('观察过期', 'warn') : st === 'unavailable' ? pill('查询失败', 'bad') : pill('未观测', 'idle');
  };
  // 右上角 ⋯ 分类操作菜单（浮层）。开合用 classList，不用内联 style（守 CSP）。
  const collapse = pop => { for (const m of document.querySelectorAll('.menu.open')) if (m !== pop) { m.classList.remove('open'); m.querySelector('.menu-btn')?.setAttribute('aria-expanded', 'false'); } };
  const menu = groups => {
    const wrap = text('div', null, 'menu');
    const trigger = text('button', '⋯', 'menu-btn'); trigger.type = 'button';
    trigger.setAttribute('aria-haspopup', 'true'); trigger.setAttribute('aria-expanded', 'false'); trigger.setAttribute('aria-label', '操作菜单');
    const pop = text('div', null, 'menu-pop');
    for (const g of groups) {
      const items = g.items.filter(Boolean);
      if (!items.length) continue;
      pop.append(text('div', g.label, 'menu-group'));
      for (const item of items) { item.classList.add('menu-item'); item.addEventListener('click', () => { wrap.classList.remove('open'); trigger.setAttribute('aria-expanded', 'false'); }); pop.append(item); }
    }
    trigger.addEventListener('click', event => { event.stopPropagation(); collapse(wrap); trigger.setAttribute('aria-expanded', String(wrap.classList.toggle('open'))); });
    wrap.append(trigger, pop);
    return wrap;
  };
  document.addEventListener('click', event => collapse(event.target.closest?.('.menu') ?? null));
  document.addEventListener('keydown', event => { if (event.key === 'Escape') collapse(null); });
  function table(headers, rows) {
    const wrapper = text('div', null, 'table-scroll'), table = text('table'), head = text('thead'), line = text('tr');
    for (const title of headers) { const th = text('th', title); th.scope = 'col'; line.append(th); }
    head.append(line); table.append(head); const body = text('tbody');
    for (const row of rows) { const tr = text('tr'); for (const value of row) { const td = text('td'); if (value?.nodeType) td.append(value); else td.textContent = String(value); tr.append(td); } body.append(tr); }
    table.append(body); wrapper.append(table); return wrapper;
  }
  function pager(page, count, size, change, key) {
    const total = Math.max(1, Math.ceil(count / size)), current = Math.min(page, total - 1);
    return toolbar(button('上一页', () => change(Math.max(0, current - 1)), current === 0, `${key}:previous`), text('span', `${current + 1} / ${total} · ${count} 项`), button('下一页', () => change(current + 1), current + 1 >= total, `${key}:next`));
  }
  function sources(vm) {
    const root = byId('sources-content');
    // 仅当有意义的数据变化才重建列表（排除逐帧跳动的"样本年龄"），避免每 5 秒把整列表拆了重搭、进度条重闪。
    const stamp = JSON.stringify([vm.snapshot?.catalog, [...(vm.observation.sourceViews ?? new Map())].map(([k, o]) => [k, o?.five_hour?.percentage ?? null, o?.weekly?.percentage ?? null, o?.state ?? null, o?.level ?? null, o?.five_hour?.current ?? null, o?.weekly?.current ?? null, o?.five_hour?.reset_at_ms ?? null, o?.weekly?.reset_at_ms ?? null]), vm.observation.fresh, vm.sourceFilter, vm.sourcesPage, vm.sourceUsageId, vm.readOnly, vm.workBusy]);
    if (root.dataset.srcStamp === stamp) return;
    root.dataset.srcStamp = stamp;
    const previousDetail = byId('source-usage-content'); root.replaceChildren();
    root.append(toolbar(action('createSource', '添加来源', null, vm.readOnly)));
    const all = (vm.snapshot?.catalog.sources ?? []).filter(p => (!vm.sourceFilter.type || p.source.type === vm.sourceFilter.type) && p.source.name.includes(vm.sourceFilter.name)), start = Math.min(vm.sourcesPage, Math.max(0, Math.ceil(all.length / 25) - 1)) * 25;
    root.append(pager(vm.sourcesPage, all.length, 25, handlers.sourcePage, 'sources'));
    for (const p of all.slice(start, start + 25)) {
      const s = p.source, observation = vm.observation.sourceViews.get(s.source_id), archived = s.retired_at !== null || p.retire_requested;
      const card = text('article', null, vm.observation.fresh ? 'card' : 'card stale');
      const head = text('div', null, 'card-head');
      head.append(text('h2', `${s.name} · ${labels[s.type]} · #${s.source_id}`), sourceStatus(p, s, observation));
      card.append(head);
      if (s.note) card.append(text('p', s.note, 'muted'));
      const names = (vm.snapshot?.catalog.memberships ?? []).filter(m => m.membership.source_id === s.source_id).map(m => vm.snapshot.catalog.groups.find(g => g.group.group_id === m.membership.group_id)?.group.name ?? m.membership.group_id);
      const groupLabel = [...new Set(names)].join('、') || '尚未加入组';
      const intent = archived ? '归档中或已归档' : s.manual_paused ? '暂停意图' : '启用意图';
      const act = { usage: [], config: [], run: [], archive: [] };
      act.config.push(action('models', archived ? '只读查看历史模型' : '查看或编辑模型', s.source_id, vm.workBusy || !vm.observation.fresh), action('discoverModels', '重新发现模型', s.source_id, vm.readOnly || archived));
      act.run.push(action('setSourcePaused', s.manual_paused ? '恢复来源' : '暂停来源', s.source_id, vm.readOnly || archived));
      act.archive.push(action('retireSource', '归档来源', s.source_id, vm.readOnly || archived));
      const more = text('details', null, 'more'); more.append(text('summary', '详情'));
      if (s.type === 'glm') {
        const win = key => observation?.[key];
        const sub = key => { const w = win(key); return w ? `${w.reset_at_ms === null ? '' : `重置 ${formatTime(w.reset_at_ms, 'ms')}`}${w.current ? '' : (w.reset_at_ms === null ? '' : ' · ') + '旧观察'}` : ''; };
        const grid = text('div', null, 'gauge-grid');
        grid.append(gauge('5 小时', win('five_hour')?.percentage ?? null, sub('five_hour')), gauge('每周', win('weekly')?.percentage ?? null, sub('weekly')));
        card.append(grid);
        card.append(text('p', `套餐 ${observation?.level ?? '未知'} · ${labels[observation?.state ?? 'unknown']} · 样本年龄 ${Number.isFinite(observation?.age_ms) ? `${Math.ceil(observation.age_ms)} ms` : '未知'}`, 'meta'));
        more.append(text('p', `配置版本 ${s.config_revision} · ${p.model_count} 模型 · 所属组：${groupLabel} · 人工配置：${intent}（不等于所有远端成员已同步）`, 'meta'));
        card.append(more);
      } else {
        const v = p.volcano, r = v.runtime;
        if (r) {
          act.usage.push(button(vm.sourceUsageId === s.source_id ? '收起分模型用量' : '展开分模型用量', () => handlers.sourceUsage(vm.sourceUsageId === s.source_id ? null : s.source_id), false, `sourceUsage:${s.source_id}`));
          act.config.push(action('setVolcanoPolicy', '设置每日策略', s.source_id, vm.readOnly || archived));
          act.run.push(action('clearError', '撤销错误耗尽判定', s.source_id, vm.readOnly || archived));
          let total; try { total = safeAdd(r.used_input, r.used_output); } catch { total = '总数超出安全范围'; }
          const limit = v.policy.daily_token_limit;
          if (limit && typeof total === 'number') { const grid = text('div', null, 'gauge-grid'); grid.append(gauge('每日用量', Math.min(100, Math.round(total / limit * 100)), `${kfmt(total)} / ${kfmt(limit)} Token${vm.observation.fresh ? '' : ' · 已过期'}`)); card.append(grid); }
          card.append(text('p', `${p.model_count} 模型 · 所属组：${groupLabel} · 连续异常 ${r.consecutive_failures} · 下次重置 ${formatTime(r.next_reset_at)}${observation?.resetPending ? '（等待新周期）' : ''}`, 'meta'));
          if (r.coverage_end === null) card.append(text('p', '本周期尚无覆盖记录；累计数不能解释为已观测零用量或完整统计。', 'warning'));
          more.append(
            text('p', `每日 Token 上界：${limit ?? '未设置'} · 失败阈值 ${v.policy.failure_threshold} · 重置分钟 ${v.policy.reset_minute} / UTC 偏移 ${v.policy.utc_offset_minutes} 分钟`, 'meta'),
            text('p', `已保存累计输入 ${r.used_input} / 输出 ${r.used_output} / 总计 ${total}`, 'meta'),
            text('p', r.coverage_end === null ? '尚无覆盖记录。' : `周期起点 ${formatTime(r.period_start)} · 已记录覆盖末端 ${formatTime(r.coverage_end)}；不保证供应商日志完整。`, 'meta'),
            text('p', `错误判定 ${r.error_exhausted ? '是' : '否'} · 上界判定 ${r.limit_exhausted ? '是' : '否'}${vm.observation.fresh ? '' : '（已过期）'} · 配置版本 ${s.config_revision} · 人工配置：${intent}`, 'meta'),
          );
          card.append(more);
        } else card.append(text('p', '周期尚未初始化，不能视为零用量。', 'warning'));
        if (vm.sourceUsageId === s.source_id) { const detail = previousDetail?.dataset.sourceId === s.source_id ? previousDetail : text('div'); detail.id = 'source-usage-content'; detail.dataset.sourceId = s.source_id; card.append(detail); }
      }
      head.append(menu([{ label: '用量', items: act.usage }, { label: '配置', items: act.config }, { label: '运行', items: act.run }, { label: '归档', items: act.archive }]));
      root.append(card);
    }
    if (!all.length) root.append(text('p', vm.snapshot ? (vm.snapshot.catalog.sources.length ? '没有匹配此类型/名称的来源。' : '还没有来源，请添加。') : '等待目录快照。', 'muted'));
  }
  function groups(vm) {
    const root = byId('groups-content'); root.replaceChildren();
    root.append(toolbar(action('createGroup', '创建组', null, vm.readOnly), action('planMember', '把来源加入组', vm.selectedGroup, vm.readOnly || !vm.selectedGroup)));
    const all = vm.snapshot?.catalog.groups ?? [], start = Math.min(vm.groupsPage, Math.max(0, Math.ceil(all.length / 50) - 1)) * 50;
    root.append(pager(vm.groupsPage, all.length, 50, handlers.groupPage, 'groups'));
    for (const p of all.slice(start, start + 50)) {
      const g = p.group, a = vm.observation.groupViews.get(g.group_id), selected = vm.selectedGroup === g.group_id;
      const card = text('article', null, selected ? 'card card-sel' : 'card');
      const head = text('div', null, 'card-head');
      const title = button(`${g.name} · #${g.group_id}`, () => handlers.selectGroup(g.group_id), false, `group:${g.group_id}`); title.classList.add('title-btn');
      head.append(title, pill(g.ready ? '已配置' : '待核验', g.ready ? 'ok' : 'warn'), menu([{ label: '核验', items: [action('checkGroup', '核验配置', g.group_id, vm.readOnly), action('registerGroup', '注册并核验', g.group_id, vm.readOnly)] }, { label: '访问 Key', items: [action('createToken', '生成访问 Key', g.group_id, vm.readOnly)] }, { label: '引用', items: [action('groupReferences', '本地引用', g.group_id, vm.workBusy)] }]));
      card.append(head);
      const policy = g.type === 'glm' ? `首选 ${p.selected_member_id ?? '无'} · 固定 ${p.pinned_member_id ?? '无'}${a?.current ? ` · 临期成员 ${a.imminent_members.join('、') || '无'}${a.pin_release ? ` · 原固定释放 ${a.pin_release.membership_id} (${a.pin_release.pct}/${a.pin_release.limit})` : ''}` : ' · 评估已过期或未知'}` : `首选成员 ${p.selected_member_id ?? '无'}`;
      card.append(text('p', `${labels[g.type]} · ${g.newapi_group} · 版本 ${g.revision} · ${g.ready ? '采样时已配置' : '待核验'}`, 'meta'), text('p', policy, 'meta'));
      root.append(card);
    }
    if (vm.selectedGroup) {
      const members = (vm.snapshot?.catalog.memberships ?? []).filter(p => p.membership.group_id === vm.selectedGroup);
      const currentGroup = all.find(p => p.group.group_id === vm.selectedGroup);
      root.append(text('h2', `组 #${vm.selectedGroup} 的成员`));
      root.append(pager(vm.membersPage, members.length, 50, handlers.memberPage, 'members'));
      const memberStart = Math.min(vm.membersPage, Math.max(0, Math.ceil(members.length / 50) - 1)) * 50;
      for (const p of members.slice(memberStart, memberStart + 50)) {
        const m = p.membership, retired = m.retired_at !== null || p.retire_requested;
        const source = vm.snapshot.catalog.sources.find(s => s.source.source_id === m.source_id)?.source;
        const card = text('article', null, 'card compact');
        const head = text('div', null, 'card-head');
        const acts = [action('ensureMember', '显式启用 / Ensure', m.membership_id, vm.readOnly || retired), action('retireMember', '移除成员', m.membership_id, vm.readOnly || retired)];
        if (currentGroup?.group.type === 'glm') acts.push(action('setGroupPin', '固定该成员', m.membership_id, vm.readOnly || retired));
        head.append(text('h2', `#${m.membership_id} · ${source?.name ?? m.source_id}`), pill(retired ? '已退休' : p.dirty ? '待同步' : '已记录', retired ? 'idle' : p.dirty ? 'warn' : 'ok'), menu([{ label: '成员操作', items: acts }]));
        card.append(head, text('p', `channel ${m.channel_id ?? '未创建'} · 生命周期 ${m.lifecycle} · 期望优先级 ${m.desired_priority} / 观察 ${m.observed_priority ?? '未知'} · 观察状态 ${m.observed_status ?? '未知'} · 错误 ${p.error_code ?? '无'}${vm.observation.fresh ? '' : '（旧快照）'}`, 'meta'));
        root.append(card);
      }
      if (currentGroup?.group.type === 'glm') root.append(toolbar(action('unpin', '取消固定', vm.selectedGroup, vm.readOnly)));
    }
    root.append(text('h2', '访问 Key · 本地身份'));
    root.append(text('p', '每个访问 Key 同时支持 OpenAI 兼容格式与 Claude Messages 格式。生成固定使用发行人账号；历史管理员身份不迁移。', 'muted'));
    if (vm.config?.public_api_base_url) {
      const base = vm.config.public_api_base_url;
      const addr = text('div', null, 'card compact');
      addr.append(text('p', `OpenAI 兼容 base：${base}/v1`, 'meta'), text('p', `Claude ANTHROPIC_BASE_URL：${base}`, 'meta'), toolbar(button('复制 OpenAI 兼容地址', () => handlers.copyAddress(`${base}/v1`)), button('复制 Claude 地址', () => handlers.copyAddress(base))));
      root.append(addr);
    } else root.append(text('p', '调用地址未配置；管理面板地址不是调用地址。', 'warning'));
    const page = vm.identities.data;
    root.append(toolbar(button('刷新本地身份', () => handlers.identities(vm.identities.after), vm.workBusy, 'identities:refresh'), button('第一页', () => handlers.identities(null), vm.workBusy, 'identities:first'), button('下一页', () => handlers.identities(page.next_after), vm.workBusy || !page?.next_after, 'identities:next'), action('registerToken', '登记已有 Key', null, vm.readOnly)));
    if (vm.identities.error) root.append(text('p', vm.identities.error, 'warning'));
    if (page) for (const i of page.items) {
      const native = matchNative(i, [...vm.nativePages.values()].map(p => p.data)), deleted = i.deleted_at !== null;
      const card = text('article', null, 'card compact');
      const head = text('div', null, 'card-head');
      head.append(text('h2', i.display_name), pill(deleted ? '历史已删除' : native ? `原生 ${native.status}` : '需核验', deleted ? 'idle' : native ? 'ok' : 'warn'), menu([{ label: '访问', items: [action('reveal', '复制访问 Key', i.token_id, vm.readOnly || deleted || i.native_token_id === null), button('查看用量', () => handlers.identityUsage(i), i.native_token_id === null, `usage:${i.token_id}`)] }, { label: '状态', items: [action('enableToken', '启用', i.token_id, vm.readOnly || deleted), action('disableToken', '停用', i.token_id, vm.readOnly || deleted)] }, { label: '危险', items: [action('deleteToken', '删除', i.token_id, vm.readOnly || deleted)] }]));
      card.append(head, text('p', `local #${i.token_id} / native ${i.native_token_id ?? '待创建'} · ${i.explicit_group} · ${i.owner_scope} / owner ${i.owner_user_id} · 本地版本 ${i.revision}`, 'meta'));
      root.append(card);
    }
    for (const scope of ['issuer', 'legacy_admin']) {
      const p = vm.nativePages.get(scope), data = p?.data;
      root.append(text('h2', `原生 Key 页 · ${scope}`));
      root.append(toolbar(button('读取本页', () => handlers.native(scope, p?.page ?? 1), vm.workBusy, `${scope}:refresh`), button('上一页', () => handlers.native(scope, Math.max(1, (p?.page ?? 1) - 1)), vm.workBusy || (p?.page ?? 1) <= 1, `${scope}:previous`), text('span', `第 ${p?.page ?? 1} 页`), button('下一页', () => handlers.native(scope, (p?.page ?? 1) + 1), vm.workBusy || (p?.page ?? 1) >= 100 || !data || data.page * data.page_size >= data.total, `${scope}:next`)));
      if (p?.error) root.append(text('p', p.error, 'warning'));
      if (data) {
        if (data.page !== p.page) root.append(text('p', `请求第 ${p.page} 页；下面显示第 ${data.page} 页的旧观察，不是新页结果。`, 'warning'));
        root.append(text('p', `本页收到于 ${p.receivedLabel}；这是读取时间，不是账号创建时间。`));
        root.append(table(['原生 ID / 名称', '组 / 状态', '其他限制'], data.items.map(i => [`#${i.token_id} · ${i.name} · owner ${i.owner_user_id} · ${i.masked_key}`, `${i.explicit_group} → ${i.effective_group} · ${i.status}`, `quota 余额 ${i.remaining_quota} / 已用 ${i.used_quota}（不是 Token 统计） · 模型限制 ${i.model_limits_enabled ? i.model_limits : '无'} · 跨组重试 ${i.cross_group_retry ? '开启' : '关闭'}`])));
      } else root.append(text('p', '尚未读取，不据此判定 Key 不存在。', 'muted'));
    }
  }
  function operations(vm) {
    const root = byId('operations'); root.replaceChildren(text('h2', '本标签页操作追踪'));
    root.append(text('p', '只恢复保存的操作 ID；移除追踪不会取消后端操作。标签页存储丢失时无法自动找回全部历史。', 'muted'));
    if (vm.storageError) root.append(text('p', vm.storageError, 'warning'));
    root.append(button('输入操作 ID 恢复查询', handlers.restoreOperation, false, 'operations:restore'));
    for (const item of vm.operations) {
      const r = item.receipt, card = text('article', null, 'card');
      const status = r ? (r.phase === 'succeeded' && r.total_targets === 0 ? '设置已保存' : `${r.phase} · 确认 ${r.confirmed_targets}/${r.total_targets}`) : '结果待核对';
      card.append(text('code', item.id), text('p', status), text('p', item.error ?? (item.tracking ? '追踪中' : '自动追踪已暂停，请手动继续'), item.error ? 'warning' : 'muted'));
      card.append(toolbar(button('继续查询', () => handlers.operation('continue', item.id), vm.workBusy, `op:continue:${item.id}`), button('显式重试原操作', () => handlers.operation('retry', item.id), vm.readOnly || !r || ['succeeded', 'superseded'].includes(r.phase), `op:retry:${item.id}`), button('同 ID 同载荷再次提交', () => handlers.operation('resubmit', item.id), vm.readOnly || !!r || !item.canResubmit, `op:resubmit:${item.id}`), button('移除追踪', () => handlers.operation('dismiss', item.id), false, `op:dismiss:${item.id}`)));
      root.append(card);
    }
  }
  function frame(vm) {
    // Step U4: 动态值只进入文本/value；刷新不重建正在编辑的表单。
    const activeKey = document.activeElement?.dataset.focusKey;
    byId('compatibility').hidden = true;
    byId('connection').textContent = vm.observation.fresh ? `状态采样时${vm.snapshot.backend_ready ? '就绪' : '未就绪'} · ${Math.ceil(vm.observation.age)} ms` : '观察已过期或尚不可用';
    byId('refresh-state').disabled = vm.stateBusy;
    const toggle = byId('toggle-operations'); toggle.textContent = `操作进度（${vm.operations.length}）`; toggle.setAttribute('aria-expanded', String(vm.operationsOpen));
    byId('operations').hidden = !vm.operationsOpen;
    for (const page of ['sources', 'groups', 'data']) {
      byId(`page-${page}`).hidden = page !== vm.page;
      const button = document.querySelector(`button[data-page="${page}"]`);
      if (page === vm.page) button.setAttribute('aria-current', 'page'); else button.removeAttribute('aria-current');
    }
    const notice = byId('notice'); notice.replaceChildren();
    if (vm.notice) notice.append(text('p', vm.notice, 'warning'));
    if (vm.config?.dry_run) notice.append(text('p', 'Dry run：写入与敏感复制已禁用。', 'warning'));
    if (!vm.observation.fresh) notice.append(text('p', '目录仅供只读参考，旧观察不能表示当前可用。', 'warning'));
    if (vm.page === 'sources') sources(vm); if (vm.page === 'groups') groups(vm);
    operations(vm);
    if (activeKey) for (const element of document.querySelectorAll('[data-focus-key]')) if (element.dataset.focusKey === activeKey && !element.disabled) { element.focus({ preventScroll: true }); break; }
  }
  function clearForm() {
    const root = byId('editor'); for (const input of root.querySelectorAll('input,textarea,select')) { if (input.type === 'checkbox') input.checked = false; else input.value = ''; }
    root.replaceChildren(); root.hidden = true;
  }
  function form(title, fields, submit, onSubmit, onChange = null) {
    clearForm(); const root = byId('editor'); root.hidden = false; root.append(text('h2', title));
    const form = text('form'); form.autocomplete = 'off'; const inputs = new Map();
    for (const field of fields) {
      if (field.type === 'explanation') { form.append(text('p', field.label, 'warning')); continue; }
      const label = text('label', field.label), input = text(field.type === 'select' ? 'select' : field.type === 'textarea' ? 'textarea' : 'input');
      if (input.tagName === 'INPUT') input.type = field.type ?? 'text';
      input.name = field.name; input.autocomplete = field.type === 'password' ? 'new-password' : 'off';
      if (field.type === 'select') for (const [value, labelText] of field.options) { const option = text('option', labelText); option.value = value; input.append(option); }
      if (field.type === 'checkbox') input.checked = field.value === true; else input.value = field.value ?? '';
      if (field.readOnly) input.readOnly = true;
      if (field.required) input.required = true;
      input.addEventListener('change', () => onChange?.(field.name, input.value));
      inputs.set(field.name, input); label.append(input); form.append(label);
    }
    const warning = text('p', '', 'warning'); warning.id = 'editor-conflict'; warning.hidden = true; form.append(warning);
    const submitButton = text('button', submit, 'primary'); submitButton.type = 'submit'; submitButton.dataset.submit = 'true';
    form.append(toolbar(submitButton, button('关闭', () => { clearForm(); handlers.closeEditor(); })));
    form.addEventListener('submit', event => { event.preventDefault(); const values = Object.fromEntries([...inputs].map(([key, input]) => [key, input.type === 'checkbox' ? input.checked : input.value])); onSubmit(values); });
    root.append(form); root.scrollIntoView({ block: 'nearest' }); [...inputs.values()][0]?.focus();
  }
  function conflict(message, disabled) {
    const note = byId('editor-conflict'); if (note) { note.textContent = message ?? ''; note.hidden = !message; }
    const submit = byId('editor').querySelector('[data-submit]'); if (submit) submit.disabled = disabled;
  }
  function clearSecret() { const root = byId('secret'); for (const i of root.querySelectorAll('input,textarea')) i.value = ''; root.replaceChildren(); root.hidden = true; }
  function manualSecret(key, onCopy, onClose) {
    clearSecret(); const root = byId('secret'); root.hidden = false;
    root.append(text('h2', '手动复制访问 Key'), text('p', '剪贴板未确认写入。此处只读显示 30 秒；离开页面会清除。', 'warning'));
    const input = text('textarea'); input.readOnly = true; input.value = key; input.setAttribute('aria-label', '临时访问 Key');
    root.append(input, toolbar(button('复制已取得的 Key', onCopy), button('关闭并清除', onClose))); input.focus(); input.select();
  }
  return { frame, form, clearForm, conflict, clearSecret, manualSecret, table, text, button, toolbar, pager };
}
