import { Lanes, SnapshotAge, observations, backoff, PendingOperations, freezeDraft, buildAction, draftConflict, ActionError, ProtocolError, identityUsageFilter, operationId, decimal, id, modelLabel, modelFilter, allFilters, makeUsageQuery, volcanoUsageQuery, formatTime } from './core.mjs';
import { route, requestJson, ClientError } from './client.mjs';
import { createRenderer } from './render.mjs';
import { prepareChart, drawChart } from './charts.mjs';
import { ENUMS } from './contract.mjs';

// Step U3: 唯一递归 pump 拥有浏览器生命周期，不为每个分页创建定时轮询。
export class Lifecycle {
  constructor({ document, window, onTick, onSuspend, onResume, setTimer = (fn, ms) => globalThis.setTimeout(fn, ms), clearTimer = id => globalThis.clearTimeout(id) }) {
    Object.assign(this, { document, window, onTick, onSuspend, onResume, setTimer, clearTimer });
    this.attached = false; this.active = false; this.destroyed = false; this.timer = null; this.generation = 0;
    this.offline = window.navigator?.onLine === false;
    this.events = [
      [document, 'visibilitychange', () => this.visibility()],
      [window, 'pagehide', () => this.suspend()],
      [window, 'pageshow', () => this.visibility()],
      [window, 'offline', () => { this.offline = true; this.suspend(); }],
      [window, 'online', () => { this.offline = false; this.visibility(); }],
    ];
  }
  start() {
    if (this.destroyed || this.attached) return;
    this.attached = true;
    for (const [target, name, listener] of this.events) target.addEventListener(name, listener);
    this.visibility();
  }
  visibility() {
    if (this.destroyed || !this.attached) return;
    if (this.document.visibilityState === 'hidden' || this.offline) this.suspend();
    else if (!this.active) { this.active = true; this.generation++; this.onResume(); this.tick(this.generation); }
  }
  tick(generation) {
    if (!this.active || generation !== this.generation) return;
    this.timer = null;
    try { this.onTick(); } finally {
      if (this.active && generation === this.generation && this.timer === null) this.timer = this.setTimer(() => this.tick(generation), 1000);
    }
  }
  suspend() {
    if (!this.active) return;
    this.active = false; this.generation++;
    if (this.timer !== null) this.clearTimer(this.timer);
    this.timer = null; this.onSuspend();
  }
  destroy() {
    if (this.destroyed) return;
    this.suspend(); this.destroyed = true;
    for (const [target, name, listener] of this.events) target.removeEventListener(name, listener);
    this.events = []; this.attached = false;
  }
}

const previewOperationId = 'qt_00000000-0000-4000-8000-000000000000';
const explain = label => ({type: 'explanation', label});
const field = (name, label, value = '', type = 'text', options) => ({name, label, value, type, options});
export class SecretSlot {
  constructor({now, visible, show, clearView, writeClipboard, copied, setTimer = (fn, ms) => globalThis.setTimeout(fn, ms), clearTimer = id => globalThis.clearTimeout(id)}) {
    Object.assign(this, {now, visible, show, clearView, writeClipboard, copied, setTimer, clearTimer});
    this.generation = 0; this.slot = null; this.timer = null;
  }
  begin(tokenId, revision) { this.clear(); return {generation: this.generation, tokenId, revision}; }
  valid(slot) { return this.slot === slot && slot.generation === this.generation && this.visible() && this.now() >= slot.received && this.now() < slot.expires; }
  receive(binding, value, deadline) {
    // Step U9: 只有可见、当前且未超时的 reveal 可进入唯一短时槽。
    if (binding.generation !== this.generation || !this.visible() || this.now() >= deadline || value.token_id !== binding.tokenId || !/^sk-[A-Za-z0-9]{16,128}$/.test(value.key)) return false;
    const received = this.now(); this.slot = {...binding, key: value.key, received, expires: received + 30000, copying: false};
    const slot = this.slot; this.timer = this.setTimer(() => { if (this.slot === slot) this.clear(); }, 30000);
    void this.copy(slot); return true;
  }
  async copy(slot = this.slot) {
    if (!slot || !this.valid(slot) || slot.copying) return;
    slot.copying = true;
    try {
      await this.writeClipboard(slot.key);
      if (this.valid(slot)) { this.clear(); this.copied(); }
    } catch {
      if (this.valid(slot)) this.show(slot.key, () => this.copy(slot), () => this.clear());
    } finally { slot.copying = false; }
  }
  expire() { if (this.slot && !this.valid(this.slot)) this.clear(); }
  clear() {
    this.generation++;
    if (this.timer !== null) this.clearTimer(this.timer);
    this.timer = null; if (this.slot) this.slot.key = null; this.slot = null; this.clearView();
  }
}
export class PanelApp {
  constructor(window, { fetchImpl = window.fetch.bind(window), now = () => window.performance.now() } = {}) {
    this.window = window; this.document = window.document; this.fetchImpl = fetchImpl; this.now = now;
    this.lanes = new Lanes(now); this.age = new SnapshotAge();
    let storage; try { storage = window.sessionStorage; } catch { storage = {getItem() { throw new Error(); }}; }
    this.pending = new PendingOperations(storage, now);
    this.config = null; this.configDue = 0; this.stateDue = 0; this.configFailures = 0; this.stateFailures = 0;
    this.page = ['sources', 'groups', 'data'].includes(window.location.hash.slice(1)) ? window.location.hash.slice(1) : 'sources'; this.sourcesPage = 0; this.groupsPage = 0; this.membersPage = 0; this.selectedGroup = null;
    this.sourceFilter = {type: '', name: ''}; this.operationsOpen = true;
    this.identities = { after: null, data: null, error: null, received: null };
    this.nativePages = new Map(); this.editor = null; this.notice = null; this.desired = null;
    this.sourceUsageId = null; this.usage = null; this.usageDue = Infinity; this.readNotBefore = 0;
    this.usageFilters = {mode: '24h', offset: 480, day: '', filter: allFilters()}; this.usagePages = {models: 0, tokens: 0, buckets: 0}; this.usageGeneration = 0; this.usageMessage = null;
    this.renderer = createRenderer(this.document, {
      action: (action, id) => this.action(action, id), sourcePage: page => { this.sourcesPage = page; if (this.sourceUsageId !== null) this.selectSourceUsage(null); else this.paint(); },
      groupPage: page => { this.groupsPage = page; this.paint(); }, memberPage: page => { this.membersPage = page; this.paint(); },
      selectGroup: id => { this.selectedGroup = id; this.membersPage = 0; this.lanes.invalidate('work'); this.desired = null; this.editor = null; this.renderer.clearForm(); this.secret?.clear(); this.paint(); },
      identities: after => this.loadIdentities(after), native: (scope, page) => this.loadNative(scope, page),
      operation: (action, id) => this.operation(action, id), restoreOperation: () => this.restoreOperation(),
      sourceUsage: id => this.selectSourceUsage(id), identityUsage: identity => this.selectIdentityUsage(identity),
      copyAddress: address => this.copyAddress(address), closeEditor: () => { this.editor = null; },
    });
    this.secret = new SecretSlot({now, visible: () => this.lifecycle?.active === true && this.page === 'groups' && this.document.visibilityState !== 'hidden',
      show: (...args) => this.renderer.manualSecret(...args), clearView: () => this.renderer.clearSecret(),
      writeClipboard: key => { const clipboard = this.window.navigator.clipboard; if (typeof clipboard?.writeText !== 'function') return Promise.reject(new Error('clipboard unavailable')); return clipboard.writeText(key); },
      copied: () => { this.notice = '访问 Key 已复制。页面中的临时引用已清除；系统剪贴板历史不受本面板控制。'; this.paint(); },
    });
    this.lifecycle = new Lifecycle({ document: this.document, window, onTick: () => this.tick(), onSuspend: () => this.suspend(), onResume: () => this.resume() });
    this.nav = [];
  }
  get readOnly() { return !this.config || this.config.dry_run || !this.age.fresh(this.now()) || this.lanes.owners.work !== null; }
  start() {
    if (this.started) return; this.started = true;
    for (const button of this.document.querySelectorAll('[data-page]')) { const fn = () => this.switchPage(button.dataset.page); button.addEventListener('click', fn); this.nav.push([button, fn]); }
    this.refreshHandler = () => { if (this.lifecycle.active && !this.lanes.owners.state) { this.stateDue = 0; void this.readState(); } };
    this.toggleHandler = () => { this.operationsOpen = !this.operationsOpen; this.paint(); };
    this.filterHandler = event => {
      event.preventDefault(); const type = this.document.getElementById('source-type-filter').value, name = this.document.getElementById('source-name-filter').value;
      if (type !== '' && !ENUMS.sourceKind.includes(type)) return;
      this.sourceFilter = {type, name}; this.sourcesPage = 0;
      if (this.sourceUsageId !== null) this.selectSourceUsage(null); else this.paint();
    };
    this.hashHandler = () => { const page = this.window.location.hash.slice(1); if (['sources', 'groups', 'data'].includes(page)) this.switchPage(page); else this.window.history.replaceState(null, '', `#${this.page}`); };
    this.document.getElementById('refresh-state').addEventListener('click', this.refreshHandler);
    this.document.getElementById('toggle-operations').addEventListener('click', this.toggleHandler);
    this.document.getElementById('source-filters').addEventListener('submit', this.filterHandler);
    this.window.addEventListener('hashchange', this.hashHandler); this.window.history.replaceState(null, '', `#${this.page}`);
    this.lifecycle.start();
  }
  suspend() {
    this.lanes.invalidate('state'); this.lanes.invalidate('work'); this.age.fail();
    this.renderer.clearSecret(); this.secret?.clear();
    if (this.editor?.sensitive) { this.renderer.clearForm(); this.editor = null; }
    this.desired = null;
  }
  resume() {
    this.age.fail(); this.config = null; this.configDue = 0; this.stateDue = 0;
    this.usageDue = 0;
    if (this.page === 'data' || this.page === 'sources' && this.sourceUsageId !== null) this.desired = {run: () => this.queryUsage(), requiresFresh: true};
    if (this.page === 'groups') this.loadIdentities(this.identities.after);
    this.paint();
  }
  destroy() {
    this.lifecycle.destroy(); this.lanes.destroy(); this.secret?.clear(); this.renderer.clearSecret(); this.renderer.clearForm();
    for (const [button, fn] of this.nav) button.removeEventListener('click', fn);
    for (const [id, event, listener] of [['refresh-state', 'click', this.refreshHandler], ['toggle-operations', 'click', this.toggleHandler], ['source-filters', 'submit', this.filterHandler]]) if (listener) this.document.getElementById(id)?.removeEventListener(event, listener);
    if (this.hashHandler) this.window.removeEventListener('hashchange', this.hashHandler);
    this.nav = []; this.desired = null; this.editor = null;
    for (const item of this.pending.items.values()) { item.payload = null; item.descriptor = null; }
  }
  message(error) {
    if (error instanceof ActionError || error instanceof ProtocolError || error instanceof ClientError) return error.message;
    return '操作未完成，请检查输入或连接；不会自动重发写入。';
  }
  paint() {
    if (!this.lifecycle?.active) return;
    const observation = observations(this.age, this.now());
    this.renderer.frame({ page: this.page, snapshot: this.age.snapshot, observation, config: this.config, readOnly: this.readOnly,
      workBusy: this.lanes.owners.work !== null, notice: this.notice, sourcesPage: this.sourcesPage, groupsPage: this.groupsPage, membersPage: this.membersPage,
      stateBusy: this.lanes.owners.state !== null, operationsOpen: this.operationsOpen, sourceFilter: this.sourceFilter,
      selectedGroup: this.selectedGroup, identities: this.identities, nativePages: this.nativePages, sourceUsageId: this.sourceUsageId,
      storageError: this.pending.storageError, operations: [...this.pending.items.values()].map(i => ({id: i.id, receipt: i.receipt, error: i.error, tracking: i.tracking, canResubmit: i.payload !== null})),
    });
    if (this.editor) {
      const original = this.editor.draft?.value;
      const currentIdentity = this.editor.draft?.entity === 'identity' && this.identities.data?.items.find(i => i.token_id === original.token_id);
      const conflict = this.editor.draft?.entity === 'identity' ? currentIdentity && (currentIdentity.revision !== original.revision || currentIdentity.deleted_at !== original.deleted_at) : this.editor.draft && this.age.snapshot && draftConflict(this.editor.draft, this.age.snapshot, this.editor.action);
      this.renderer.conflict(conflict ? '对象已有新版本。请关闭后重新打开，不会覆盖当前草稿或自动重发。' : null, this.editor.readOnlyAction ? this.lanes.owners.work !== null : this.readOnly || !!conflict);
    }
    this.renderUsage?.();
  }
  tick() {
    if (!this.lifecycle.active) return;
    this.secret?.expire(); this.paint(); const now = this.now();
    if (!this.lanes.owners.state && now >= this.stateDue) this.readState();
    if (this.lanes.owners.work) return;
    if (now < this.readNotBefore) return;
    if (!this.config && now >= this.configDue) { this.readConfig(); return; }
    if (!this.config) return;
    if (this.desired && (!this.desired.requiresFresh || this.age.fresh(now))) { const desired = this.desired; this.desired = null; desired.run(); return; }
    const op = this.pending.due();
    if (op) { this.pollOperation(op.id); return; }
    if (now >= this.usageDue) this.queryUsage?.();
  }
  async readState() {
    if (!this.lifecycle.active) return;
    const ticket = this.lanes.acquire('state', 'state'); if (!ticket) return;
    try {
      const result = await requestJson(route('state'), null, ticket, {fetchImpl: this.fetchImpl, now: this.now, valid: t => this.lanes.valid(t)});
      this.age.ingest(result.value, result.age, ticket.started, result.received); this.stateFailures = 0; this.stateDue = this.now() + 5000;
      this.checkUsagePeriod?.();
    } catch {
      if (this.lanes.owns(ticket) && ticket.generation === this.lanes.generations.state) { this.age.fail(); this.stateDue = this.now() + backoff(++this.stateFailures); }
    } finally { this.lanes.finish(ticket); this.paint(); }
  }
  async work(descriptor, body, success, failure = null, key = descriptor.path) {
    if (!this.lifecycle.active) return false;
    if (!descriptor.write && this.now() < this.readNotBefore) {
      if (descriptor.action === 'reveal') { this.notice = '读取暂时冷却，请稍后再次点击复制。'; this.paint(); }
      else this.desired = {run: () => this.work(descriptor, body, success, failure, key), requiresFresh: false};
      return false;
    }
    const ticket = this.lanes.acquire('work', key, descriptor.write); if (!ticket) return false;
    try {
      this.paint();
      const result = await requestJson(descriptor, body, ticket, {fetchImpl: this.fetchImpl, now: this.now,
        valid: t => descriptor.write ? this.lanes.receiptValid(t) : this.lanes.valid(t)});
      if (!this.lanes.owns(ticket) || !descriptor.write && ticket.generation !== this.lanes.generations.work) return false;
      if (!(descriptor.write ? this.lanes.receiptValid(ticket) : this.lanes.valid(ticket))) throw new ClientError('deadline');
      success(result.value, ticket);
    } catch (error) {
      if (this.lanes.owns(ticket) && (descriptor.write || ticket.generation === this.lanes.generations.work)) {
        if (!descriptor.write && ['queue_full', 'query_cancelled'].includes(error.code)) this.readNotBefore = this.now() + 5000;
        if (failure) failure(error, ticket); else if (this.lifecycle.active) this.notice = this.message(error);
      }
    }
    finally { this.lanes.finish(ticket); this.paint(); }
    return true;
  }
  readConfig() {
    this.work(route('config'), null, value => { this.config = value; this.configFailures = 0; this.configDue = Infinity; }, (error, ticket) => {
      if (ticket.generation !== this.lanes.generations.work) return;
      this.config = null; this.configDue = this.now() + backoff(++this.configFailures); this.notice = this.message(error);
    });
  }
  desire(callback, requiresFresh = false) { this.lanes.invalidate('work'); this.desired = {run: callback, requiresFresh}; this.paint(); }
  switchPage(page) {
    if (!['sources', 'groups', 'data'].includes(page) || page === this.page) return;
    this.page = page; this.lanes.invalidate('work'); this.desired = null; this.editor = null; this.renderer.clearForm(); this.secret?.clear(); this.renderer.clearSecret();
    this.window.history.replaceState(null, '', `#${page}`);
    this.usage = null; this.usageDue = 0;
    if (page === 'groups') this.loadIdentities(this.identities.after);
    else if (page === 'data' || page === 'sources' && this.sourceUsageId !== null) this.desired = {run: () => this.queryUsage(), requiresFresh: true};
    this.paint();
  }
  loadIdentities(after = null) {
    // Step U7: 本地身份和各 owner 的原生页始终分别维护。
    this.secret.clear();
    this.identities.after = after;
    this.desire(() => this.work(route('identities', {after}), null, data => { this.identities = {after, data, received: this.now(), error: null}; }, error => { this.identities.error = `${this.message(error)} 原页结果仅供旧观察参考。`; }));
  }
  loadNative(scope, page) {
    this.secret.clear();
    const previous = this.nativePages.get(scope);
    this.nativePages.set(scope, {...previous, page, error: previous?.data && previous.data.page !== page ? '请求新页；下面仍是旧页观察。' : null});
    this.desire(() => this.work(route('nativePage', {owner_scope: scope, page}), null, data => {
      this.nativePages.set(scope, {page, data, received: this.now(), receivedLabel: new Date().toISOString(), error: null});
    }, error => { this.nativePages.set(scope, {...this.nativePages.get(scope), error: `${this.message(error)} 未据此删除任何本地身份。`}); }));
  }
  pollOperation(id) {
    this.work(route('operation', {id}), null, value => { if (this.pending.items.has(id)) this.pending.accept(id, value); }, error => this.pending.failed(id, this.message(error)), `operation:${id}`);
  }
  restoreOperation() {
    this.editor = {readOnlyAction: true};
    this.renderer.form('恢复操作查询', [field('operation_id', '操作 ID')], '查询原 ID', values => {
      try { this.pending.restore(operationId(values.operation_id)); this.renderer.clearForm(); this.editor = null; this.paint(); }
      catch (error) { this.notice = this.message(error); this.paint(); }
    }); this.paint();
  }
  operation(action, id) {
    const item = this.pending.items.get(id); if (!item) return;
    if (action === 'dismiss') { try { this.pending.dismiss(id); } catch (e) { this.notice = this.message(e); } this.paint(); return; }
    if (action === 'continue') { this.pending.continue(id); this.paint(); return; }
    if (this.readOnly || !this.lifecycle.active) return;
    const descriptor = action === 'retry' ? route('retry', {id}) : item.descriptor;
    const body = action === 'retry' ? {schema_version: 1} : item.payload;
    if (!descriptor || !body) { this.notice = '本标签页没有原载荷，只能查询原 ID。'; this.paint(); return; }
    this.pending.continue(id);
    this.work(descriptor, body, value => { if (this.pending.items.has(id)) this.pending.accept(id, value); }, error => this.pending.failed(id, `结果待核对。${this.message(error)}`), `operation:${id}`);
  }
  async copyAddress(address) {
    try { await this.window.navigator.clipboard.writeText(address); if (this.lifecycle.active) this.notice = '调用地址已复制。'; }
    catch { if (this.lifecycle.active) this.notice = '剪贴板未确认写入，请手动复制页面上的公开调用地址。'; }
    this.paint();
  }
  confirm(action, title, draft, fields = {}, explanation = '该操作会写入管理配置，请确认。', inputs = []) {
    this.editor = {action, draft, sensitive: false};
    this.renderer.form(title, [explain(explanation), ...inputs], '确认提交', values => this.submit(action, draft, {...fields, ...values}));
    this.paint();
  }
  action(action, id) {
    if (!this.lifecycle.active || !this.age.snapshot) return;
    if (['createToken', 'registerToken', 'reveal', 'enableToken', 'disableToken', 'deleteToken'].includes(action)) this.secret.clear();
    if (action === 'createSource') { if (!this.readOnly) this.openSource('glm'); return; }
    if (action === 'createGroup') { if (!this.readOnly) this.openGroup(); return; }
    if (action === 'registerToken') { if (!this.readOnly) this.openRegisterToken(); return; }
    const state = this.age.snapshot;
    const source = state.catalog.sources.find(p => p.source.source_id === id), group = state.catalog.groups.find(p => p.group.group_id === id);
    const member = state.catalog.memberships.find(p => p.membership.membership_id === id), identity = this.identities.data?.items.find(i => i.token_id === id);
    if (action === 'groupReferences') {
      if (this.lanes.owners.work) return;
      this.work(route('groupReferences', {id}), null, value => { if (this.lifecycle.active) { this.editor = {readOnlyAction: true}; this.renderer.form('仅已知本地引用', [explain(JSON.stringify(value))], '关闭', () => { this.renderer.clearForm(); this.editor = null; }); } }); return;
    }
    if (action === 'models' && source && !this.lanes.owners.work && this.age.fresh(this.now())) {
      const draft = freezeDraft('source', source);
      this.work(route('sourceModels', {id}), null, value => {
        if (value.source_revision !== draft.value.source.config_revision) throw new ActionError('来源版本已变化，请重新打开模型。');
        if (source.source.retired_at !== null || source.retire_requested) {
          this.editor = {readOnlyAction: true};
          this.renderer.form('归档来源模型（只读）', [{...field('models', '历史完整模型 JSON 数组', JSON.stringify(value.models, null, 2), 'textarea'), readOnly: true}], '关闭', () => {this.renderer.clearForm(); this.editor = null;}); return;
        }
        this.confirm('setModels', '来源模型', draft, {}, '使用 JSON 字符串数组；模型内换行和原标签不会被拆分。', [field('models', '模型 JSON 数组', JSON.stringify(value.models, null, 2), 'textarea')]);
      }); return;
    }
    if (this.readOnly) return;
    if (['setSourcePaused', 'retireSource', 'discoverModels', 'clearError', 'setVolcanoPolicy'].includes(action) && source) {
      const draft = freezeDraft('source', source);
      if (action === 'setVolcanoPolicy') { this.openPolicy(draft); return; }
      const affected = state.catalog.memberships.filter(m => m.membership.source_id === id).map(m => state.catalog.groups.find(g => g.group.group_id === m.membership.group_id)?.group.name ?? m.membership.group_id);
      const explanation = action === 'retireSource' ? `请求全局停用并归档共享来源，保留历史，不撤销供应商凭据。受影响组：${[...new Set(affected)].join('、') || '无'}。这不是只从某一个组移除。` : action === 'clearError' ? '只撤销错误耗尽判定；不会清除 Token 上界、暂停状态或提前重置周期。' : '修改共享来源配置，所有使用它的组都会受到影响。';
      this.confirm(action, `${source.source.name} · ${action}`, draft, action === 'setSourcePaused' ? {paused: !source.source.manual_paused} : {}, explanation); return;
    }
    if (['ensureMember', 'retireMember'].includes(action) && member) { this.confirm(action, `成员 #${id}`, freezeDraft('membership', member), {}, action === 'ensureMember' ? '显式提交启用意图；保存或同步成功仍不保证当前可路由。' : '只移除此组的成员，不删除共享来源。'); return; }
    if (action === 'setGroupPin' && member) {
      const g = state.catalog.groups.find(p => p.group.group_id === member.membership.group_id);
      if (g) this.confirm(action, '固定 GLM 组成员', freezeDraft('group', g), {membership_id: id}); return;
    }
    if (action === 'unpin' && group) { this.confirm('setGroupPin', '取消固定', freezeDraft('group', group), {membership_id: null}); return; }
    if (['checkGroup', 'registerGroup'].includes(action) && group) { this.confirm('checkGroup', '核验组配置', freezeDraft('group', group), {register: action === 'registerGroup'}); return; }
    if (action === 'createToken' && group) {
      this.confirm(action, `生成访问 Key · ${group.group.name}`, freezeDraft('group', group), {}, '固定使用发行人账号；此 Key 只属于所选组，同时支持两种调用协议。', [field('display_name', 'Key 名称'), field('expires_at', '到期时间（Unix 秒；空表示不设置）')]); return;
    }
    if (action === 'planMember' && group) {
      this.confirm(action, `加入来源 · ${group.group.name}`, null, {group_id: id}, '这一步只创建计划。保存后等待新目录版本，再显式 Ensure。', [field('source_id', '同类型来源', '', 'select', state.catalog.sources.filter(p => p.source.type === group.group.type && p.source.retired_at === null && !p.retire_requested).map(p => [p.source.source_id, p.source.name]))]); return;
    }
    if (identity) {
      this.secret.clear();
      const draft = freezeDraft('identity', identity);
      if (action === 'reveal') { this.reveal?.(identity); return; }
      if (['enableToken', 'disableToken'].includes(action)) this.confirm('setTokenEnabled', '设置访问 Key 启停', draft, {enabled: action === 'enableToken'});
      if (action === 'deleteToken') this.confirm(action, '删除访问 Key', draft, {}, '原生访问 Key 将被删除；历史统计身份仍保留。');
    }
  }
  submit(action, draft, fields) {
    if (this.readOnly || !this.lifecycle.active) return;
    try {
      if (draft?.entity === 'identity') {
        const current = this.identities.data?.items.find(i => i.token_id === draft.value.token_id);
        if (current && (current.revision !== draft.value.revision || current.deleted_at !== draft.value.deleted_at)) throw new ActionError('身份已有新版本，请重新打开并确认。分页未包含此身份不代表它已删除。');
      }
      const result = buildAction(action, draft, fields, this.age.snapshot, previewOperationId);
      const routeId = result.targetId ?? (action === 'createToken' ? null : draft?.value?.source?.source_id);
      const descriptor = route(action, routeId ? {id: routeId} : {});
      if (result.kind) {
        const item = this.pending.begin(descriptor, {kind: result.kind, targetId: result.targetId}, id => ({...result.body, operation_id: id}), this.window.crypto);
        this.renderer.clearForm(); this.editor = null;
        this.work(descriptor, item.payload, value => { if (this.pending.items.has(item.id)) this.pending.accept(item.id, value); this.notice = value.target_type === 'token' ? '已收到 Key 操作回执。请显式刷新本地身份页；原生启停状态需另行刷新对应账号页，统一目录快照不包含这些身份。' : '已收到操作回执，等待统一快照反映状态；没有自动续发其他写入。'; }, error => { this.pending.failed(item.id, `结果待核对。${this.message(error)}`); }, `operation:${item.id}`);
      } else {
        this.renderer.clearForm(); this.editor = null;
        this.work(descriptor, result.body, value => {
          this.notice = action === 'createSource' ? `来源 #${value.source.source_id} 已保存，模型来源 ${value.model_origin}。等待新目录快照。` : action === 'checkGroup' ? `核验结果：${value.ready ? '就绪' : `未就绪 · ${value.error_code}`}。` : action === 'planMember' ? '成员计划已保存。请等待新目录显示，再显式 Ensure。' : '已保存，等待目录或手动刷新本地身份。';
        }, error => { this.notice = error.existingSourceId ? `该凭据已对应来源 #${error.existingSourceId}，请核对原来源；没有覆盖配置。` : `${this.message(error)} 回执丢失不证明未写入：来源需重新填写凭据后显式提交核对；组/成员按唯一关系核对，不按名称认领。`; });
      }
    } catch (error) { this.notice = this.message(error); this.paint(); }
  }
  openGroup() {
    this.editor = {action: 'createGroup'};
    this.renderer.form('创建组', [field('name', '显示名称'), field('newapi_group', 'NewAPI 分组标识'), field('type', '来源类型', 'glm', 'select', [['glm', 'GLM'], ['volcano', '火山']])], '保存组', values => this.submit('createGroup', null, values)); this.paint();
  }
  openRegisterToken() {
    this.editor = {action: 'registerToken'};
    this.renderer.form('登记已有访问 Key', [field('owner_scope', '原生所属账号范围', 'issuer', 'select', [['issuer', '发行人'], ['legacy_admin', '历史管理员']]), field('native_token_id', '原生 Token ID（不是本地身份 ID）'), field('display_name', '显示名称')], '登记身份', values => this.submit('registerToken', null, values), () => this.lanes.invalidate('work')); this.paint();
  }
  policyFields(policy = {daily_token_limit: null, failure_threshold: 3, reset_minute: 60, utc_offset_minutes: 480}) {
    return [field('daily_token_limit', '每日 Token 上界（空表示不设置）', policy.daily_token_limit === null ? '' : String(policy.daily_token_limit)), field('failure_threshold', '连续失败阈值', String(policy.failure_threshold)), field('reset_minute', '每日重置分钟（01:00 = 60）', String(policy.reset_minute)), field('utc_offset_minutes', '重置时区偏移分钟（UTC+8 = 480）', String(policy.utc_offset_minutes))];
  }
  openPolicy(draft) { this.confirm('setVolcanoPolicy', '火山来源每日策略', draft, {}, '仅改变火山来源策略，不改 GLM。未初始化时使用运行版本 0。', this.policyFields(draft.value.volcano.policy)); }
  openSource(type) {
    this.secret?.clear(); this.lanes.invalidate('work'); this.editor = {action: 'createSource', sensitive: true};
    const fields = [explain('整个来源配置仅在本次提交中使用，不持久化到浏览器；隐藏页面会清除表单。'),
      field('type', '来源类型', type, 'select', [['glm', 'GLM'], ['volcano', '火山']]), field('name', '显示名称'), field('note', '备注'), field('credential', '上游凭据', '', 'password'),
      field('channel_type', 'NewAPI 渠道类型', type === 'glm' ? '8' : '1'), field('base_url', '上游渠道地址', type === 'glm' ? 'https://open.bigmodel.cn/api/coding/paas/v4/chat/completions' : ''),
      field('models_url', '模型发现地址', type === 'glm' ? 'https://open.bigmodel.cn/api/coding/paas/v4/models' : ''), field('discovery_auth', '模型发现鉴权', 'bearer', 'select', [['bearer', 'Bearer'], ['authorization_raw', 'Authorization 原值'], ['x_api_key', 'x-api-key']]),
      field('model_selection', '模型加入方式', 'discover', 'select', [['discover', '自动发现'], ['manual', '手动 JSON 数组'], ['discover_or_manual', '发现失败后使用手动数组']]), field('models', '手动或兜底模型 JSON 数组', '[]', 'textarea')];
    if (type === 'glm') fields.push(field('quota_url', '额度查询地址', 'https://open.bigmodel.cn/api/monitor/usage/quota/limit?type=2'), field('global_headers', '全局额外请求头 JSON 数组', '[]', 'textarea'), field('key_headers', '此 Key 请求头 JSON 数组（组织与项目 selector）', '[]', 'textarea'));
    else fields.push(...this.policyFields());
    this.renderer.form('添加共享来源', fields, '创建来源', values => this.submit('createSource', null, values), (name, value) => { if (name === 'type') this.openSource(value); }); this.paint();
  }
  selectSourceUsage(id) { this.sourceUsageId = id; this.changeUsage(); }
  selectIdentityUsage(identity) { this.usageFilters.filter.native_token_id = identityUsageFilter(identity); this.switchPage('data'); this.syncUsageControls(); this.changeUsage(); }
  changeUsage() { this.usageGeneration++; this.lanes.invalidate('work'); this.desired = this.page === 'data' || this.page === 'sources' && this.sourceUsageId !== null ? {run: () => this.queryUsage(), requiresFresh: true} : null; this.usage = null; this.usageMessage = null; this.usageDue = 0; this.usagePages = {models: 0, tokens: 0, buckets: 0}; this.paint(); }
  currentPeriodKey() {
    const s = this.age.snapshot?.catalog.sources.find(s => s.source.source_id === this.sourceUsageId);
    return s?.volcano?.runtime ? `${s.source.source_id}:${s.source.config_revision}:${s.volcano.runtime.period_id}` : null;
  }
  periodResetPending() {
    return this.page === 'sources' && this.sourceUsageId !== null && observations(this.age, this.now()).sourceViews.get(this.sourceUsageId)?.resetPending === true;
  }
  checkUsagePeriod() {
    if (this.page !== 'sources' || !this.sourceUsageId) return;
    const key = this.currentPeriodKey();
    if (this.usagePeriod !== undefined && this.usagePeriod !== key) { this.usageGeneration++; this.lanes.invalidate('work'); this.usage = null; this.usageDue = 0; this.usageMessage = '来源周期已变化，等待新周期查询。'; }
    this.usagePeriod = key;
  }
  queryUsage() {
    if (this.page !== 'data' && !(this.page === 'sources' && this.sourceUsageId)) { this.usageDue = Infinity; return; }
    if (!this.age.fresh(this.now()) || this.lanes.owners.work) return;
    let query, periodKey = null;
    try {
      if (this.page === 'sources') {
        if (this.periodResetPending()) throw new ActionError('已到重置边界，旧周期结果仅供参考；等待服务端进入新周期。');
        const source = this.age.snapshot.catalog.sources.find(s => s.source.source_id === this.sourceUsageId);
        if (!source) throw new ActionError('来源已不在目录。');
        ({query, periodKey} = volcanoUsageQuery(source, this.age.snapshot.captured_at_ms, this.usageFilters.offset));
        this.usagePeriod = periodKey;
      } else query = makeUsageQuery(this.usageFilters.mode, this.age.snapshot.captured_at_ms, this.usageFilters.offset, this.usageFilters.filter, this.usageFilters.day);
    } catch (error) { this.usageMessage = this.message(error); this.usageDue = this.now() + 5000; this.paint(); return; }
    const key = `${periodKey ?? ''}:${JSON.stringify(query)}`, generation = this.usageGeneration;
    if (this.usage?.key !== key) this.usage = null;
    this.usageMessage = '正在查询；截止时间固定为本次状态采样时间。'; this.usageDue = Infinity;
    this.work(route('usage'), query, value => {
      if (generation !== this.usageGeneration || periodKey !== null && periodKey !== this.currentPeriodKey()) return;
      if (periodKey !== null && this.periodResetPending()) { this.usageMessage = '查询期间已到重置边界，迟到旧周期结果不发布；等待新周期。'; this.usageDue = this.now() + 5000; return; }
      this.usage = {key, chart: prepareChart(value), received: new Date().toISOString(), error: null, periodKey};
      this.usageMessage = null; this.usageDue = this.now() + 60000;
    }, error => {
      if (generation !== this.usageGeneration) return;
      this.usageMessage = `${this.message(error)}${this.usage?.key === key ? ' 下面是同一查询的旧结果。' : ''}`;
      if (this.usage) this.usage.error = this.usageMessage;
      this.usageDue = this.now() + 60000;
    }, `usage:${key}`);
  }
  syncUsageControls() {
    const form = this.document.getElementById('usage-controls'); if (!form) return;
    const f = this.usageFilters, token = f.filter.native_token_id, model = f.filter.model;
    for (const [name, value] of Object.entries({mode: f.mode, offset: f.offset, day: f.day, tokenMode: token.mode, token: token.value ?? '', modelMode: model.mode, model: model.value ?? ''})) form.elements.namedItem(name).value = String(value);
  }
  ensureUsageControls(root) {
    if (this.document.getElementById('usage-controls')) return;
    const r = this.renderer, form = r.text('form'); form.id = 'usage-controls';
    const fields = [field('mode', '区间', '24h', 'select', [['24h', '近 24 小时'], ['30d', '近 30 天'], ['day', '单日钻取']]), field('day', '单日日期', '', 'date'), field('offset', '图表 UTC 偏移分钟', '480'), field('tokenMode', '访问 Key 筛选', 'any', 'select', [['any', '全部'], ['unknown', '未归属原生 Key'], ['exact', '指定原生 ID']]), field('token', '原生 Token ID（不是本地身份 ID）'), field('modelMode', '模型筛选', 'any', 'select', [['any', '全部模型'], ['unknown', '未归属模型'], ['exact', '指定模型原标签']]), field('model', '模型原标签')];
    for (const f of fields) {
      const label = r.text('label', f.label), input = r.text(f.type === 'select' ? 'select' : 'input'); input.name = f.name;
      if (f.type === 'select') for (const [value, labelText] of f.options) { const option = r.text('option', labelText); option.value = value; input.append(option); }
      else input.type = f.type ?? 'text'; input.value = f.value; label.append(input); form.append(label);
    }
    const submit = r.text('button', '应用筛选并查询', 'primary'); submit.type = 'submit'; form.append(submit, r.button('刷新用量', () => this.changeUsage()));
    form.addEventListener('submit', event => {
      event.preventDefault();
      try {
        const value = name => form.elements.namedItem(name).value;
        const filter = allFilters();
        filter.native_token_id = value('tokenMode') === 'exact' ? {mode: 'exact', value: id(value('token'))} : {mode: value('tokenMode')};
        filter.model = value('modelMode') === 'exact' ? {mode: 'exact', value: modelLabel(value('model'))} : {mode: value('modelMode')};
        this.usageFilters = {mode: value('mode'), day: value('day'), offset: decimal(value('offset'), -720, 840), filter}; this.changeUsage();
      } catch (error) { this.notice = this.message(error); this.paint(); }
    }); root.append(form); this.syncUsageControls();
    const report = r.text('div'); report.id = 'usage-report'; root.append(report);
  }
  renderUsage() {
    const r = this.renderer;
    let root;
    if (this.page === 'data') { this.ensureUsageControls(this.document.getElementById('data-content')); root = this.document.getElementById('usage-report'); }
    else if (this.page === 'sources' && this.sourceUsageId) root = this.document.getElementById('source-usage-content');
    if (!root) return;
    const periodExpired = this.periodResetPending();
    const stamp = `${this.usage?.received}:${this.usageMessage}:${this.usage?.error}:${JSON.stringify(this.usagePages)}:${this.age.expired}:${periodExpired}:${root.clientWidth}`;
    if (root.dataset.paint === stamp) return; root.dataset.paint = stamp; root.replaceChildren();
    if (this.usageMessage) root.append(r.text('p', this.usageMessage, 'warning'));
    if (!this.usage) { if (!this.usageMessage) root.append(r.text('p', '等待可用状态采样后查询。', 'muted')); return; }
    const chart = this.usage.chart, snapshot = chart.snapshot, query = snapshot.query;
    root.className = this.usage.error || !this.age.fresh(this.now()) || periodExpired ? 'stale' : '';
    if (periodExpired) root.append(r.text('p', '以下为旧周期结果。已到重置边界，等待服务端发布新周期。', 'warning'));
    root.append(r.text('h2', '分模型 Token 用量'), r.text('p', `查询区间 [${formatTime(query.from, 's', query.utc_offset_minutes)}, ${formatTime(query.to, 's', query.utc_offset_minutes)})；截至状态采样时间。读取于 ${this.usage.received}。`));
    if (this.usage.periodKey) root.append(r.text('p', `来源周期标识 ${this.usage.periodKey}；这是来源周期查询，不是自然日累计。`));
    root.append(r.text('p', `账本版本 ${snapshot.ledger.revision} · 初始补采起点 ${snapshot.ledger.initial_requested_from ?? '未初始化'}；扫描结束不保证供应商完整性。`, 'muted'));
    for (const scan of snapshot.ledger.scans) root.append(r.text('p', `${scan.lane} · ${scan.phase} · 下一页 ${scan.next_page} · 窗口 ${scan.window ? `[${scan.window.from}, ${scan.window.to})` : '无'} · 异常 ${scan.fault ?? '无'}`));
    if (chart.kind === 'uninitialized') { root.append(r.text('p', '账本尚未初始化；不是零统计。', 'warning')); return; }
    const totals = snapshot.usage.total;
    root.append(r.text('p', `输入 ${totals.input} · 输出 ${totals.output} · 总 Token ${totals.total} · 消费记录 ${totals.consumption_rows}`));
    if (chart.kind === 'empty') root.append(r.text('p', '此区间暂无已观测消费。'));
    if (chart.kind === 'zero') root.append(r.text('p', `已观测 ${totals.consumption_rows} 条消费记录，Token 总量为 0。`));
    const canvas = r.text('canvas', null, 'chart'); canvas.setAttribute('aria-label', '按模型堆叠的 Token 柱形图，精确数值见下方表格'); root.append(canvas);
    const geometry = drawChart(canvas, chart, this.window.devicePixelRatio);
    const tooltip = r.text('p', '移动到柱形查看该桶的精确数值；下表可用键盘读取。'); root.append(tooltip);
    canvas.addEventListener('pointermove', event => {
      if (!geometry || !geometry.count) return;
      const index = Math.floor((event.clientX - canvas.getBoundingClientRect().left - geometry.left) / geometry.step), bucket = chart.bars[index];
      if (bucket) tooltip.textContent = `${formatTime(bucket.from, 's', query.utc_offset_minutes)}：输入 ${bucket.total.input} / 输出 ${bucket.total.output} / 总 Token ${bucket.total.total} / 消费记录 ${bucket.total.consumption_rows}`;
    });
    const section = (name, title, rows, header, toRow) => {
      root.append(r.text('h3', title)); const total = rows.length, page = Math.min(this.usagePages[name], Math.max(0, Math.ceil(total / 50) - 1));
      root.append(r.pager(page, total, 50, next => { this.usagePages[name] = next; root.dataset.paint = ''; this.paint(); }, `usage:${name}`));
      root.append(r.table(header, rows.slice(page * 50, page * 50 + 50).map(toRow)));
    };
    const legend = model => {
      const label = r.text('span', null, 'legend-label'), swatch = r.text('canvas', null, 'legend-swatch'); swatch.width = 18; swatch.height = 18; swatch.setAttribute('aria-hidden', 'true');
      const context = swatch.getContext('2d'); if (context) { context.fillStyle = model.color; context.fillRect(0, 0, 18, 18); }
      label.append(swatch, r.text('span', model.model === null ? '未归属模型（null）' : model.model)); return label;
    };
    section('models', '模型图例与明细（图表保留全部模型）', chart.models, ['颜色 / 模型', '输入 / 输出 / 总 Token / 记录', '筛选'], m => [legend(m), `${m.usage.input} / ${m.usage.output} / ${m.usage.total} / ${m.usage.consumption_rows}`, r.button('只看此模型', () => { this.usageFilters.filter.model = modelFilter(m.model); this.switchPage('data'); this.syncUsageControls(); this.changeUsage(); })]);
    section('tokens', '原生 Key 明细', snapshot.usage.tokens, ['原生 ID', '输入 / 输出 / 总 Token / 记录'], t => [t.native_token_id ?? '未归属（null）', `${t.usage.input} / ${t.usage.output} / ${t.usage.total} / ${t.usage.consumption_rows}`]);
    const rows = chart.bars.flatMap(b => b.models.length ? b.models.map(m => ({bucket: b, model: m.model, usage: m.usage})) : [{bucket: b, model: undefined, usage: b.total}]);
    section('buckets', '按桶和模型读取精确数值', rows, ['桶区间', '模型', '输入 / 输出 / 总 Token / 记录'], row => [`[${formatTime(row.bucket.from, 's', query.utc_offset_minutes)}, ${formatTime(row.bucket.to, 's', query.utc_offset_minutes)})`, row.model === undefined ? '空桶' : row.model === null ? '未归属模型（null）' : row.model, `${row.usage.input} / ${row.usage.output} / ${row.usage.total} / ${row.usage.consumption_rows}`]);
  }
  reveal(identity) {
    if (this.readOnly || !this.lifecycle.active || this.page !== 'groups' || this.now() < this.readNotBefore) return;
    const binding = this.secret.begin(identity.token_id, identity.revision);
    this.work(route('reveal', {id: identity.token_id}), {schema_version: 1, expected_revision: identity.revision}, (value, ticket) => {
      this.secret.receive(binding, value, ticket.deadline);
    }, error => { this.secret.clear(); this.notice = this.message(error); }, `reveal:${identity.token_id}:${identity.revision}`);
  }
}

let instance = null;
export function bootstrap(window) {
  // Step U10: 能力失败不回退旧接口，也不引入隐藏后台写者。
  if (instance) return instance;
  try {
    if (['fetch', 'AbortController', 'structuredClone', 'TextEncoder', 'TextDecoder'].some(name => typeof window[name] !== 'function') || typeof String.prototype.isWellFormed !== 'function') throw new Error('required browser capability unavailable');
    instance = new PanelApp(window); instance.start(); return instance;
  } catch {
    try { instance?.destroy(); } catch { /* Continue to the static fail-closed screen. */ }
    instance = null;
    for (const button of window.document.querySelectorAll('button')) button.disabled = true;
    const message = window.document.getElementById('compatibility');
    message.hidden = false; message.textContent = '面板初始化失败或浏览器缺少必要能力。页面保持只读，没有切换到旧接口。请升级浏览器或重新加载。';
    const retry = window.document.createElement('button'); retry.type = 'button'; retry.textContent = '重新加载页面'; retry.addEventListener('click', () => window.location.reload()); message.append(retry);
    return null;
  }
}
if (typeof window !== 'undefined' && window.document?.getElementById('page-sources')) bootstrap(window);
