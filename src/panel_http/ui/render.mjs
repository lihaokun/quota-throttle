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
    const root = byId('sources-content'), previousDetail = byId('source-usage-content'); root.replaceChildren();
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
      const memberships = (vm.snapshot?.catalog.memberships ?? []).filter(m => m.membership.source_id === s.source_id);
      const names = memberships.map(m => vm.snapshot.catalog.groups.find(g => g.group.group_id === m.membership.group_id)?.group.name ?? m.membership.group_id);
      if (s.type === 'glm') {
        const win = key => observation?.[key];
        const sub = key => { const w = win(key); return w ? `${w.reset_at_ms === null ? '' : `重置 ${formatTime(w.reset_at_ms, 'ms')}`}${w.current ? '' : (w.reset_at_ms === null ? '' : ' · ') + '旧观察'}` : ''; };
        const grid = text('div', null, 'gauge-grid');
        grid.append(gauge('5 小时', win('five_hour')?.percentage ?? null, sub('five_hour')), gauge('每周', win('weekly')?.percentage ?? null, sub('weekly')));
        card.append(grid);
        card.append(text('p', `套餐 ${observation?.level ?? '未知'} · ${labels[observation?.state ?? 'unknown']} · 样本年龄 ${Number.isFinite(observation?.age_ms) ? `${Math.ceil(observation.age_ms)} ms` : '未知'}`, 'meta'));
      } else {
        const v = p.volcano, r = v.runtime;
        if (r) {
          let total; try { total = safeAdd(r.used_input, r.used_output); } catch { total = '总数超出安全范围'; }
          const limit = v.policy.daily_token_limit;
          if (limit && typeof total === 'number') {
            const grid = text('div', null, 'gauge-grid');
            grid.append(gauge('每日用量', Math.min(100, Math.round(total / limit * 100)), `${kfmt(total)} / ${kfmt(limit)} Token${vm.observation.fresh ? '' : ' · 已过期'}`));
            card.append(grid);
          }
          card.append(text('p', `每日 Token 上界：${limit ?? '未设置'} · 失败阈值 ${v.policy.failure_threshold} · 重置分钟 ${v.policy.reset_minute} / UTC 偏移 ${v.policy.utc_offset_minutes} 分钟`, 'meta'));
          card.append(text('p', `已保存累计输入 ${r.used_input} / 输出 ${r.used_output} / 总计 ${total} · 连续异常 ${r.consecutive_failures}`, 'meta'));
          card.append(text('p', r.coverage_end === null ? '本周期尚无覆盖记录；累计数不能解释为已观测零用量或完整统计。' : `周期起点 ${formatTime(r.period_start)} · 已记录覆盖末端 ${formatTime(r.coverage_end)}；不保证供应商日志完整。`, r.coverage_end === null ? 'warning' : 'meta'));
          card.append(text('p', `已保存运行状态：${s.manual_paused ? '停用' : r.error_exhausted || r.limit_exhausted ? '耗尽' : '可用'} · 错误判定 ${r.error_exhausted ? '是' : '否'} · 上界判定 ${r.limit_exhausted ? '是' : '否'}${vm.observation.fresh ? '' : '（已过期）'}`, 'meta'));
          card.append(text('p', `${observation?.resetPending ? '等待服务端进入新周期 · ' : ''}下次重置 ${formatTime(r.next_reset_at)}`, 'meta'));
        } else card.append(text('p', '周期尚未初始化，不能视为零用量。', 'warning'));
        card.append(toolbar(action('setVolcanoPolicy', '设置每日策略', s.source_id, vm.readOnly || archived), action('clearError', '撤销错误耗尽判定', s.source_id, vm.readOnly || archived || r === null), button(vm.sourceUsageId === s.source_id ? '收起分模型用量' : '展开分模型用量', () => handlers.sourceUsage(vm.sourceUsageId === s.source_id ? null : s.source_id), false, `sourceUsage:${s.source_id}`)));
        if (vm.sourceUsageId === s.source_id) { const detail = previousDetail?.dataset.sourceId === s.source_id ? previousDetail : text('div'); detail.id = 'source-usage-content'; detail.dataset.sourceId = s.source_id; card.append(detail); }
      }
      card.append(text('p', `配置版本 ${s.config_revision} · ${p.model_count} 模型 · 所属组：${[...new Set(names)].join('、') || '尚未加入组'} · 人工配置：${archived ? '归档中或已归档' : s.manual_paused ? '暂停意图' : '启用意图'}（不等于所有远端成员已同步）`, 'meta'));
      card.append(toolbar(action('setSourcePaused', s.manual_paused ? '恢复来源' : '暂停来源', s.source_id, vm.readOnly || archived), action('models', archived ? '只读查看历史模型' : '查看或编辑模型', s.source_id, vm.workBusy || !vm.observation.fresh), action('discoverModels', '重新发现模型', s.source_id, vm.readOnly || archived), action('retireSource', '归档来源', s.source_id, vm.readOnly || archived)));
      root.append(card);
    }
    if (!all.length) root.append(text('p', vm.snapshot ? (vm.snapshot.catalog.sources.length ? '没有匹配此类型/名称的来源。' : '还没有来源，请添加。') : '等待目录快照。', 'muted'));
  }
  function groups(vm) {
    const root = byId('groups-content'); root.replaceChildren();
    root.append(toolbar(action('createGroup', '创建组', null, vm.readOnly), action('planMember', '把来源加入组', vm.selectedGroup, vm.readOnly || !vm.selectedGroup)));
    const all = vm.snapshot?.catalog.groups ?? [], start = Math.min(vm.groupsPage, Math.max(0, Math.ceil(all.length / 50) - 1)) * 50;
    root.append(pager(vm.groupsPage, all.length, 50, handlers.groupPage, 'groups'));
    root.append(table(['组', '类型与配置', '原策略观察', '操作'], all.slice(start, start + 50).map(p => {
      const g = p.group, a = vm.observation.groupViews.get(g.group_id);
      const policy = g.type === 'glm' ? `首选 ${p.selected_member_id ?? '无'} / 固定 ${p.pinned_member_id ?? '无'} · ${a?.current ? '当前观察' : '评估已过期或未知'}${a?.current ? ` · 临期成员 ${a.imminent_members.join('、') || '无'}${a.pin_release ? ` · 原固定释放 ${a.pin_release.membership_id} (${a.pin_release.pct}/${a.pin_release.limit})` : ''}` : ''}` : `首选成员 ${p.selected_member_id ?? '无'}`;
      return [button(`${g.name} (#${g.group_id})`, () => handlers.selectGroup(g.group_id), false, `group:${g.group_id}`), `${labels[g.type]} · ${g.newapi_group} · 版本 ${g.revision} · ${g.ready ? '采样时已配置' : '待核验'}`, policy,
        toolbar(action('checkGroup', '核验配置', g.group_id, vm.readOnly), action('registerGroup', '注册并核验', g.group_id, vm.readOnly), action('createToken', '生成访问 Key', g.group_id, vm.readOnly), action('groupReferences', '本地引用', g.group_id, vm.workBusy))];
    })));
    if (vm.selectedGroup) {
      const members = (vm.snapshot?.catalog.memberships ?? []).filter(p => p.membership.group_id === vm.selectedGroup);
      root.append(text('h2', `组 #${vm.selectedGroup} 的成员`));
      root.append(pager(vm.membersPage, members.length, 50, handlers.memberPage, 'members'));
      const currentGroup = all.find(p => p.group.group_id === vm.selectedGroup);
      const memberStart = Math.min(vm.membersPage, Math.max(0, Math.ceil(members.length / 50) - 1)) * 50;
      root.append(table(['成员 / 来源', '同步观察', '操作'], members.slice(memberStart, memberStart + 50).map(p => {
        const m = p.membership, retired = m.retired_at !== null || p.retire_requested;
        const source = vm.snapshot.catalog.sources.find(s => s.source.source_id === m.source_id)?.source;
        const actions = [action('ensureMember', '显式启用 / Ensure', m.membership_id, vm.readOnly || retired), action('retireMember', '移除成员', m.membership_id, vm.readOnly || retired)];
        if (currentGroup?.group.type === 'glm') actions.push(action('setGroupPin', '固定该成员', m.membership_id, vm.readOnly || retired));
        return [`#${m.membership_id} · ${source?.name ?? m.source_id} · channel ${m.channel_id ?? '未创建'}`, `生命周期 ${m.lifecycle} · 期望优先级 ${m.desired_priority} / 观察 ${m.observed_priority ?? '未知'} · 观察状态 ${m.observed_status ?? '未知'} · ${p.dirty ? '待同步' : '已记录'} · 错误 ${p.error_code ?? '无'}${vm.observation.fresh ? '' : '（旧快照）'}`, toolbar(...actions)];
      })));
      if (currentGroup?.group.type === 'glm') root.append(action('unpin', '取消固定', vm.selectedGroup, vm.readOnly));
    }
    root.append(text('h2', '访问 Key · 本地身份'));
    root.append(text('p', '每个访问 Key 同时支持 OpenAI 兼容格式与 Claude Messages 格式。生成固定使用发行人账号；历史管理员身份不迁移。', 'muted'));
    if (vm.config?.public_api_base_url) {
      const base = vm.config.public_api_base_url;
      root.append(text('p', `OpenAI 兼容 base：${base}/v1`), text('p', `Claude ANTHROPIC_BASE_URL：${base}`));
      root.append(toolbar(button('复制 OpenAI 兼容地址', () => handlers.copyAddress(`${base}/v1`)), button('复制 Claude 地址', () => handlers.copyAddress(base))));
    } else root.append(text('p', '调用地址未配置；管理面板地址不是调用地址。', 'warning'));
    const page = vm.identities.data;
    root.append(toolbar(button('刷新本地身份', () => handlers.identities(vm.identities.after), vm.workBusy, 'identities:refresh'), button('第一页', () => handlers.identities(null), vm.workBusy, 'identities:first'), button('下一页', () => handlers.identities(page.next_after), vm.workBusy || !page?.next_after, 'identities:next'), action('registerToken', '登记已有 Key', null, vm.readOnly)));
    if (vm.identities.error) root.append(text('p', vm.identities.error, 'warning'));
    if (page) root.append(table(['本地身份 / 原生 ID', '组与状态', '操作'], page.items.map(i => {
      const native = matchNative(i, [...vm.nativePages.values()].map(p => p.data));
      return [`${i.display_name} · local #${i.token_id} / native ${i.native_token_id ?? '待创建'} · ${i.owner_scope} / owner ${i.owner_user_id}`, `${i.explicit_group} · ${i.deleted_at !== null ? '历史已删除身份（统计保留）' : native ? `原生页观察：${native.status}` : '该原生页未加载或绑定需核验'} · 本地版本 ${i.revision}`,
        toolbar(action('reveal', '复制访问 Key', i.token_id, vm.readOnly || i.deleted_at !== null || i.native_token_id === null), action('enableToken', '启用', i.token_id, vm.readOnly || i.deleted_at !== null), action('disableToken', '停用', i.token_id, vm.readOnly || i.deleted_at !== null), action('deleteToken', '删除', i.token_id, vm.readOnly || i.deleted_at !== null), button('查看用量', () => handlers.identityUsage(i), i.native_token_id === null, `usage:${i.token_id}`))];
    })));
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
