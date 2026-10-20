import { ENUMS as E, LIMITS as L, VERSIONS as V } from './contract.mjs';

export class ProtocolError extends Error {
  constructor() { super('响应不兼容，请刷新界面或检查服务版本。'); this.name = 'ProtocolError'; }
}
export function requireThat(condition) { if (!condition) throw new ProtocolError(); }
export function integer(value, minimum = 0, maximum = L.jsonSafe) {
  requireThat(Number.isSafeInteger(value) && value >= minimum && value <= maximum);
  return value;
}
export function decimal(value, minimum = 0, maximum = L.jsonSafe) {
  requireThat(typeof value === 'string' && /^(0|[1-9][0-9]*|-[1-9][0-9]*)$/.test(value));
  return integer(Number(value), minimum, maximum);
}
export function id(value) {
  requireThat(typeof value === 'string' && /^[1-9][0-9]{0,18}$/.test(value));
  requireThat(value.length < 19 || value <= '9223372036854775807');
  return value;
}
export function compareId(a, b) { id(a); id(b); return a.length - b.length || (a < b ? -1 : a > b ? 1 : 0); }
export function operationId(value) { requireThat(typeof value === 'string' && /^[A-Za-z0-9][A-Za-z0-9_-]{15,63}$/.test(value)); return value; }
const cycle = value => { requireThat(typeof value === 'string' && /^[1-9][0-9]{0,19}$/.test(value) && (value.length < 20 || value <= '18446744073709551615')); return value; };
const string = value => { requireThat(typeof value === 'string'); return value; };
const boolean = value => { requireThat(typeof value === 'boolean'); return value; };
const revision = value => integer(value, 1);
const signed = value => integer(value, -L.jsonSafe);
const percentage = value => { requireThat(typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= L.jsonSafe); return value; };
const nullable = fn => value => value === null ? null : fn(value);
const enumeration = name => value => { requireThat(E[name].includes(value)); return value; };
const version = name => value => { requireThat(value === V[name]); return value; };
const array = (fn, max, min = 0) => value => { requireThat(Array.isArray(value) && value.length >= min && value.length <= max); return value.map(fn); };
const boundedText = (max, min = 0) => value => { string(value); requireThat([...value].length >= min && [...value].length <= max && !value.includes('\0')); return value; };
export const consumedFields = new Map();
function record(name, fields) {
  consumedFields.set(name, Object.keys(fields));
  const entries = Object.entries(fields);
  return value => {
    requireThat(value !== null && typeof value === 'object' && !Array.isArray(value));
    const out = {};
    for (const [key, validate] of entries) {
      requireThat(Object.hasOwn(value, key));
      out[key] = validate(value[key]);
    }
    return out;
  };
}
function unique(values, key) { const found = new Set(); for (const row of values) { const k = key(row); requireThat(!found.has(k)); found.add(k); } return values; }
const name = boundedText(128, 1), note = boundedText(2048);
const nativeGroup = value => { string(value); requireThat(/^[A-Za-z0-9_.-]{1,64}$/.test(value) && value !== 'auto'); return value; };
const source = record('source', { source_id: id, type: enumeration('sourceKind'), name, note, manual_paused: boolean, retired_at: nullable(integer), config_revision: revision });
const group = record('group', { group_id: id, newapi_group: nativeGroup, name, type: enumeration('sourceKind'), revision, ready: boolean });
const membershipFields = record('membership', {
  membership_id: id, source_id: id, group_id: id, channel_id: nullable(id), order: integer,
  lifecycle: enumeration('lifecycle'), desired_priority: signed, observed_status: enumeration('observedStatus'),
  observed_priority: nullable(signed), applied_revision: nullable(revision), retired_at: nullable(integer),
});
function membership(value) {
  const m = membershipFields(value);
  requireThat(m.lifecycle !== 'planned' || m.channel_id === null);
  requireThat(!['imported', 'created_disabled', 'ready'].includes(m.lifecycle) || m.channel_id !== null);
  requireThat((m.lifecycle === 'retired') === (m.retired_at !== null));
  return m;
}
export const policy = record('policy', {
  daily_token_limit: nullable(revision), failure_threshold: v => integer(v, 1, L.failureThreshold),
  reset_minute: v => integer(v, 0, L.resetMinute), utc_offset_minutes: v => integer(v, L.offsetMin, L.offsetMax),
});
const runtime = record('runtime', {
  revision, period_id: revision, period_start: integer, next_reset_at: integer,
  used_input: integer, used_output: integer, consecutive_failures: integer,
  error_exhausted: boolean, limit_exhausted: boolean, coverage_end: nullable(integer),
});
const volcano = record('volcano', { policy, runtime: nullable(runtime) });
const panelSourceFields = record('panelSource', { source, retire_requested: boolean, model_count: v => integer(v, 1, L.models), volcano: nullable(volcano) });
function panelSource(value) { const p = panelSourceFields(value); requireThat((p.source.type === 'glm') === (p.volcano === null)); return p; }
const panelGroupFields = record('panelGroup', { group, policy_revision: revision, selected_member_id: nullable(id), pinned_member_id: nullable(id), regime: enumeration('regime') });
function panelGroup(value) { const p = panelGroupFields(value); requireThat(p.group.type === 'glm' ? p.regime !== null : p.regime === null && p.pinned_member_id === null); return p; }
const panelMembership = record('panelMembership', {
  membership, control_revision: revision, status_directive: enumeration('statusDirective'), retire_requested: boolean,
  create_state: enumeration('createState'), dirty: boolean, next_attempt_at: integer, error_code: enumeration('controlError'),
});
const catalog = record('catalog', {
  sources: array(panelSource, L.sources), groups: array(panelGroup, L.groups), memberships: array(panelMembership, L.memberships),
});
const windowView = record('window', { percentage, reset_at_ms: nullable(revision) });
const quotaFields = record('quota', {
  source_id: id, source_revision: revision, observed_at_ms: nullable(integer), state: enumeration('quotaState'),
  five_hour: nullable(windowView), weekly: nullable(windowView), level: nullable(boundedText(128, 1)),
});
function quota(value) {
  const q = quotaFields(value);
  requireThat((q.state === 'unknown') === (q.observed_at_ms === null));
  requireThat(q.state === 'observed' ? q.five_hour !== null || q.weekly !== null : q.five_hour === null && q.weekly === null && q.level === null);
  return q;
}
const pinRelease = record('pinRelease', { membership_id: id, pct: percentage, limit: percentage });
const assessmentFields = record('assessment', {
  group_id: id, cycle_id: nullable(cycle), evaluated_at_ms: nullable(integer), current: boolean,
  imminent_members: nullable(array(id, L.memberships)), pin_release: nullable(pinRelease),
});
function assessment(value) {
  const a = assessmentFields(value);
  requireThat((a.cycle_id === null) === (a.evaluated_at_ms === null));
  requireThat(a.current ? a.cycle_id !== null && a.imminent_members !== null : a.imminent_members === null && a.pin_release === null);
  return a;
}
const glm = record('glm', { cycle_id: nullable(cycle), completed_at_ms: nullable(integer), sources: array(quota, L.sources), groups: array(assessment, L.groups) });
const stateFields = record('state', { schema_version: version('state'), captured_at_ms: integer, backend_ready: boolean, catalog, glm });

// Step U2: 只投影白名单字段；未知或私有字段不进入界面状态，不引入通用 schema 引擎。
export function validateState(value) {
  const state = stateFields(value), c = state.catalog;
  unique(c.sources, p => p.source.source_id); unique(c.groups, p => p.group.group_id);
  unique(c.memberships, p => p.membership.membership_id);
  unique(state.glm.sources, q => q.source_id); unique(state.glm.groups, g => g.group_id);
  requireThat((state.glm.cycle_id === null) === (state.glm.completed_at_ms === null));
  const sources = new Map(c.sources.map(p => [p.source.source_id, p.source]));
  const groups = new Map(c.groups.map(p => [p.group.group_id, p.group]));
  for (const p of c.memberships) {
    const m = p.membership, s = sources.get(m.source_id), g = groups.get(m.group_id);
    requireThat(s !== undefined && g !== undefined && s.type === g.type);
  }
  for (const q of state.glm.sources) requireThat(sources.get(q.source_id)?.type === 'glm');
  for (const a of state.glm.groups) requireThat(groups.get(a.group_id)?.type === 'glm');
  return state;
}
const configFields = record('config', { schema_version: version('config'), dry_run: boolean, public_api_base_url: nullable(string) });
export function validateConfig(value) {
  const c = configFields(value);
  if (c.public_api_base_url !== null) {
    const raw = c.public_api_base_url;
    requireThat(new TextEncoder().encode(raw).length <= 8192 && raw.trim() === raw);
    let u; try { u = new URL(raw); } catch { throw new ProtocolError(); }
    requireThat(['http:', 'https:'].includes(u.protocol) && !u.username && !u.password && !u.search && !u.hash && !raw.includes('?') && !raw.includes('#') && !raw.endsWith('/'));
    requireThat(!/%(?![A-Fa-f0-9]{2})/.test(u.pathname) && !/[^A-Za-z0-9\-._~!$&'()*+,;=:@/%]/.test(u.pathname));
    const last = (u.pathname.split('/').filter(Boolean).at(-1) ?? '').replace(/%([A-Fa-f0-9]{2})/g, (_, byte) => String.fromCharCode(Number.parseInt(byte, 16)));
    requireThat(last.toLowerCase() !== 'v1' && raw === u.href.replace(/\/+$/, ''));
  }
  return c;
}
const operationFields = record('operation', {
  schema_version: version('operation'), operation_id: operationId, kind: enumeration('operationKind'),
  target_type: enumeration('targetType'), target_id: id, phase: enumeration('operationPhase'), target_revision: revision,
  total_targets: v => integer(v, 0, L.operationTargets), confirmed_targets: v => integer(v, 0, L.operationTargets),
  error_code: enumeration('controlError'), updated_at: integer,
});
export function targetType(kind) {
  enumeration('operationKind')(kind);
  if (['create_token', 'set_token_enabled', 'delete_token'].includes(kind)) return 'token';
  if (['ensure_member', 'retire_member'].includes(kind)) return 'membership';
  if (['set_group_pin', 'set_group_priorities'].includes(kind)) return 'group';
  return 'source';
}
export function validateOperation(value) {
  const op = operationFields(value);
  requireThat(op.target_type === targetType(op.kind) && op.confirmed_targets <= op.total_targets);
  if (op.target_type === 'token') requireThat(op.total_targets === 1 && !['partial', 'superseded'].includes(op.phase));
  if (E.localSettingKind.includes(op.kind)) requireThat(op.phase === 'succeeded' && op.total_targets === 0 && op.confirmed_targets === 0);
  if (op.phase === 'succeeded') requireThat(op.error_code === null);
  if (op.phase === 'failed') requireThat(op.error_code !== null);
  return op;
}
const identityFields = record('identity', {
  schema_version: version('identity'), token_id: id, native_token_id: nullable(id), owner_user_id: id,
  owner_scope: enumeration('ownerScope'), origin: enumeration('identityOrigin'), group_id: nullable(id),
  explicit_group: boundedText(256), display_name: boundedText(256, 1), revision,
  create_state: enumeration('createState'), created_at: integer, deleted_at: nullable(integer),
});
export function validateIdentity(value) {
  const i = identityFields(value);
  requireThat((i.create_state === 'resolved') === (i.native_token_id !== null));
  requireThat(i.create_state === 'resolved' || i.origin === 'created');
  requireThat(i.origin !== 'created' || (i.owner_scope === 'issuer' && i.group_id !== null && nativeGroup(i.explicit_group)));
  requireThat(i.deleted_at === null || i.native_token_id !== null);
  return i;
}
const native = record('native', {
  schema_version: version('native'), token_id: id, owner_user_id: id, owner_scope: enumeration('ownerScope'),
  name: string, masked_key: value => { string(value); requireThat(/^.{0,4}\*+.{0,4}$/.test(value)); return value; },
  explicit_group: string, effective_group: boundedText(L.jsonSafe, 1), status: enumeration('nativeStatus'),
  created_at: integer, expires_at: nullable(integer), remaining_quota: signed, used_quota: integer,
  unlimited_quota: boolean, model_limits_enabled: boolean, model_limits: string, allow_ips: nullable(string), cross_group_retry: boolean,
});
const identities = record('identities', { schema_version: version('identities'), items: array(validateIdentity, L.identities), after: nullable(id), next_after: nullable(id) });
export function validateIdentities(value, after = null) {
  const p = identities(value); requireThat(p.after === after);
  unique(p.items, i => i.token_id);
  let last = after;
  for (const i of p.items) { requireThat(last === null || compareId(i.token_id, last) > 0); last = i.token_id; }
  requireThat(p.items.length === L.identities ? p.next_after === last : p.next_after === null);
  return p;
}
const nativePage = record('nativePage', {
  schema_version: version('nativePage'), owner_scope: enumeration('ownerScope'), page: v => integer(v, 1, L.nativePages),
  page_size: v => { requireThat(v === L.nativePage); return v; }, total: v => integer(v, 0, L.nativeTotal), items: array(native, L.nativePage),
});
export function validateNativePage(value, scope, page) {
  const p = nativePage(value); requireThat(p.owner_scope === scope && p.page === page);
  unique(p.items, i => `${i.owner_user_id}:${i.token_id}`);
  for (const i of p.items) requireThat(i.owner_scope === scope);
  return p;
}
export function nativeIdentityKey(scope, owner, token) { enumeration('ownerScope')(scope); return `${scope}:${id(owner)}:${id(token)}`; }
export function matchNative(identity, pages) {
  if (identity.native_token_id === null) return null;
  const key = nativeIdentityKey(identity.owner_scope, identity.owner_user_id, identity.native_token_id);
  for (const p of pages) for (const i of p?.items ?? []) {
    if (nativeIdentityKey(i.owner_scope, i.owner_user_id, i.token_id) === key) return i;
  }
  return null;
}
export function identityUsageFilter(identity) {
  if (identity === null) return { mode: 'any' };
  requireThat(identity.native_token_id !== null);
  return { mode: 'exact', value: id(identity.native_token_id) };
}
export function catalogModels(value) {
  return array(v => { string(v); requireThat(v.length > 0 && !/[,\0]/.test(v)); return v; }, L.models, 1)(value);
}
const sourceModels = record('sourceModels', { schema_version: version('sourceModels'), source_id: id, source_revision: revision, models: catalogModels });
const sourceCreated = record('sourceCreated', { schema_version: version('sourceCreated'), source, model_origin: enumeration('modelOrigin') });
const groupCreated = record('groupCreated', { schema_version: version('groupCreated'), group });
const memberPlanned = record('memberPlanned', { schema_version: version('memberPlanned'), membership });
const groupCheck = record('groupCheck', { schema_version: version('groupCheck'), group_id: id, revision, ready: boolean, error_code: enumeration('controlError') });
const count = v => integer(v, 0, L.memberships);
const groupReferences = record('groupReferences', {
  schema_version: version('groupReferences'), group_id: id, active_memberships: count, historical_memberships: count,
  live_tokens: count, pending_creations: count, historical_tokens: count, unknown_group_tokens: count,
  known_local_only: v => { requireThat(v === true); return v; }, deletion_authorized: v => { requireThat(v === false); return v; },
});
const reveal = record('reveal', { schema_version: version('reveal'), token_id: id, key: v => { string(v); requireThat(/^sk-[A-Za-z0-9]{16,128}$/.test(v)); return v; } });
const httpError = record('httpError', { schema_version: version('httpError'), error: enumeration('httpError') });
const adminError = record('adminError', { schema_version: version('adminError'), error: enumeration('adminError'), existing_source_id: nullable(id) });
export function validateError(value) {
  const e = Object.hasOwn(value ?? {}, 'existing_source_id') ? adminError(value) : httpError(value);
  if (Object.hasOwn(e, 'existing_source_id')) requireThat((e.error === 'duplicate_source') === (e.existing_source_id !== null));
  return e;
}
export function validateReply(family, value, query = {}) {
  switch (family) {
    case 'config': return validateConfig(value);
    case 'state': return validateState(value);
    case 'operation': return validateOperation(value);
    case 'identity': { const v = validateIdentity(value); if (query.id !== undefined) requireThat(v.token_id === query.id); return v; }
    case 'identities': return validateIdentities(value, query.after ?? null);
    case 'nativePage': return validateNativePage(value, query.owner_scope, query.page);
    case 'sourceCreated': return sourceCreated(value);
    case 'groupCreated': return groupCreated(value);
    case 'memberPlanned': return memberPlanned(value);
    case 'sourceModels': { const v = sourceModels(value); requireThat(v.source_id === query.id); return v; }
    case 'groupCheck': { const v = groupCheck(value); requireThat(v.group_id === query.id && v.ready === (v.error_code === null)); return v; }
    case 'groupReferences': { const v = groupReferences(value); requireThat(v.group_id === query.id); return v; }
    case 'reveal': { const v = reveal(value); requireThat(v.token_id === query.id); return v; }
    case 'usageSnapshot': return validateUsage(value, query);
    default: throw new ProtocolError();
  }
}
export function safeAdd(a, b) { integer(a); integer(b); return integer(a + b); }
export function formatTime(value, unit = 's', offset = 480) {
  integer(value); integer(offset, L.offsetMin, L.offsetMax);
  const ms = unit === 'ms' ? value : value * 1000;
  const local = ms + offset * 60000;
  if (!Number.isSafeInteger(ms) || !Number.isSafeInteger(local) || local < 0 || local > 8640000000000000) return `${value} ${unit}`;
  return `${new Date(local).toISOString().replace('T', ' ').replace('Z', '')} UTC${offset >= 0 ? '+' : '-'}${Math.floor(Math.abs(offset) / 60).toString().padStart(2, '0')}:${(Math.abs(offset) % 60).toString().padStart(2, '0')}`;
}

// Step U3: 结果失效不释放 owner；原 Promise 链结束才归槽。
export class Lanes {
  constructor(now = () => performance.now()) { this.now = now; this.serial = 0; this.owners = { state: null, work: null }; this.generations = { state: 0, work: 0 }; }
  acquire(lane, key, write = false) {
    requireThat(lane === 'state' || lane === 'work');
    if (this.owners[lane]) return null;
    const started = this.now();
    const ticket = { id: ++this.serial, lane, key, generation: this.generations[lane], started, deadline: started + (lane === 'state' ? 12000 : 40000), write, controller: new AbortController() };
    this.owners[lane] = ticket; return ticket;
  }
  owns(ticket) { return this.owners[ticket.lane] === ticket; }
  valid(ticket) { const now = this.now(); return this.owns(ticket) && ticket.generation === this.generations[ticket.lane] && !ticket.controller.signal.aborted && now >= ticket.started && now < ticket.deadline; }
  receiptValid(ticket) { const now = this.now(); return this.owns(ticket) && ticket.write && !ticket.controller.signal.aborted && now >= ticket.started && now < ticket.deadline; }
  invalidate(lane) { this.generations[lane]++; const ticket = this.owners[lane]; if (ticket && !ticket.write) ticket.controller.abort(); }
  finish(ticket) { if (this.owns(ticket)) this.owners[ticket.lane] = null; }
  destroy() { for (const lane of ['state', 'work']) { this.invalidate(lane); this.owners[lane]?.controller.abort(); } }
}
export const backoff = failures => [5000, 10000, 20000, 30000][Math.min(Math.max(failures - 1, 0), 3)];

// Step U4: 刷新失败立即使旧快照过期；同一 publication 不能使它复活。
export class SnapshotAge {
  constructor() { this.snapshot = null; this.received = 0; this.baseAge = Infinity; this.expired = true; }
  fail() { this.expired = true; }
  ingest(snapshot, headerAge, started, ended) {
    const age = typeof headerAge === 'string' && /^(0|[1-9][0-9]*)$/.test(headerAge) ? Number(headerAge) : NaN;
    if (!Number.isSafeInteger(age) || age < 0 || !Number.isFinite(ended - started) || ended < started) { this.fail(); throw new ProtocolError(); }
    const incoming = age + ended - started;
    if (!Number.isFinite(incoming)) { this.fail(); throw new ProtocolError(); }
    const same = this.snapshot?.captured_at_ms === snapshot.captured_at_ms;
    this.baseAge = same ? Math.max(this.age(ended), incoming) : incoming;
    this.received = ended; this.snapshot = snapshot;
    this.expired = same ? this.expired || this.baseAge >= 10000 : this.baseAge >= 10000;
    return !this.expired;
  }
  age(now) { const elapsed = now - this.received; return Number.isFinite(elapsed) && elapsed >= 0 ? this.baseAge + elapsed : Infinity; }
  fresh(now) { if (this.age(now) >= 10000) this.expired = true; return this.snapshot !== null && !this.expired; }
}
export function observations(age, now) {
  const state = age.snapshot, fresh = age.fresh(now), elapsed = age.age(now);
  const sourceViews = new Map(), groupViews = new Map();
  if (!state) return { fresh: false, age: elapsed, sourceViews, groupViews };
  const capture = state.captured_at_ms;
  const timeCurrent = time => fresh && time !== null && time <= capture && capture - time + elapsed <= 30000;
  const quotas = new Map(state.glm.sources.map(q => [q.source_id, q]));
  const sources = new Map(state.catalog.sources.map(p => [p.source.source_id, p]));
  const members = new Map(state.catalog.memberships.map(p => [p.membership.membership_id, p]));
  for (const p of state.catalog.sources) {
    const s = p.source, q = quotas.get(s.source_id);
    const current = !!q && ['observed', 'unavailable'].includes(q.state) && q.source_revision === s.config_revision && timeCurrent(q.observed_at_ms);
    const displayWindow = w => w !== null ? { ...w, current: current && (w.reset_at_ms === null || w.reset_at_ms > capture + elapsed) } : null;
    sourceViews.set(s.source_id, {
      current, state: !q || q.state === 'unknown' ? 'unknown' : current ? q.state : 'stale', five_hour: q ? displayWindow(q.five_hour) : null,
      weekly: q ? displayWindow(q.weekly) : null, level: current ? q.level : null,
      age_ms: q?.observed_at_ms !== null && q?.observed_at_ms <= capture ? capture - q.observed_at_ms + elapsed : null,
      resetPending: !!p.volcano?.runtime && p.volcano.runtime.next_reset_at * 1000 <= capture + elapsed,
    });
  }
  const related = new Map();
  for (const p of state.catalog.memberships) {
    const m = p.membership, s = sources.get(m.source_id);
    if (m.channel_id === null || m.lifecycle === 'retired' || m.retired_at !== null || p.retire_requested || !s || s.source.manual_paused || s.source.retired_at !== null || s.retire_requested) continue;
    if (!related.has(m.group_id)) related.set(m.group_id, new Set());
    related.get(m.group_id).add(m.source_id);
  }
  for (const g of state.glm.groups) {
    let current = g.current && g.cycle_id === state.glm.cycle_id && timeCurrent(g.evaluated_at_ms);
    for (const sourceId of related.get(g.group_id) ?? []) if (!sourceViews.get(sourceId)?.current) current = false;
    for (const memberId of g.imminent_members ?? []) {
      const member = members.get(memberId), weekly = member && quotas.get(member.membership.source_id)?.weekly;
      if (weekly?.reset_at_ms !== null && weekly?.reset_at_ms <= capture + elapsed) current = false;
    }
    groupViews.set(g.group_id, { current, imminent_members: current ? g.imminent_members : null, pin_release: current ? g.pin_release : null });
  }
  return { fresh, age: elapsed, sourceViews, groupViews };
}

export function modelLabel(value) {
  string(value); requireThat(value.length > 0 && value.isWellFormed() && new TextEncoder().encode(value).length <= 256 && !/\p{Cc}/u.test(value)); return value;
}
// 模型域由消费入口和筛选入口校验；Map 身份构造不重复编码已验证的标签。
export function modelKey(value) { requireThat(value === null || typeof value === 'string'); return value === null ? 'null:' : `string:${value}`; }
export function modelFilter(value) { return value === null ? {mode: 'unknown'} : {mode: 'exact', value: modelLabel(value)}; }
function filter(value, validate) {
  requireThat(value !== null && typeof value === 'object' && !Array.isArray(value));
  const mode = enumeration('filterMode')(value.mode);
  return mode === 'exact' ? {mode, value: validate(value.value)} : {mode};
}
const resolvedFilter = record('resolvedFilter', { source_id: v => filter(v, id), group_id: v => filter(v, id), native_token_id: v => filter(v, id), model: v => filter(v, modelLabel) });
const queryFields = record('resolvedQuery', { from: integer, to: integer, utc_offset_minutes: v => integer(v, L.offsetMin, L.offsetMax), grain: enumeration('grain'), filter: resolvedFilter });
export function validateQuery(value) { const q = queryFields(value); requireThat(q.from < q.to && q.to - q.from <= 31 * 86400); return q; }
export function allFilters() { return {source_id: {mode: 'any'}, group_id: {mode: 'any'}, native_token_id: {mode: 'any'}, model: {mode: 'any'}}; }
export function makeUsageQuery(mode, anchorMs, offset = 480, filters = allFilters(), day = null) {
  // Step U8: 真正取得工作槽时才冻结服务端采样锚点。
  integer(anchorMs); integer(offset, L.offsetMin, L.offsetMax); const to = Math.floor(anchorMs / 1000), shift = offset * 60;
  let from, end = to, grain = 'hour';
  if (mode === '24h') from = Math.max(0, to - 86400);
  else if (mode === '30d') { from = Math.max(0, Math.floor((to + shift) / 86400) * 86400 - shift - 29 * 86400); grain = 'day'; }
  else if (mode === 'day') {
    ensureAction(typeof day === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(day), '请选择日期。');
    const startMs = Date.parse(`${day}T00:00:00Z`);
    ensureAction(Number.isFinite(startMs) && new Date(startMs).toISOString().slice(0, 10) === day, '日期无效。');
    const start = startMs / 1000 - shift; from = Math.max(0, start); end = Math.min(to, start + 86400);
  } else throw new ActionError('未知统计区间。');
  ensureAction(from < end, '该日期尚未开始或没有可查询的区间。');
  return {schema_version: 1, ...validateQuery({from, to: end, utc_offset_minutes: offset, grain, filter: filters})};
}
export function volcanoUsageQuery(source, anchorMs, offset = 480) {
  ensureAction(source.source.type === 'volcano', '请选择火山来源。');
  const runtime = source.volcano.runtime, to = Math.floor(integer(anchorMs) / 1000);
  ensureAction(runtime !== null, '周期尚未初始化。');
  ensureAction(to < runtime.next_reset_at, '等待服务端进入新周期。');
  ensureAction(to > runtime.period_start, '本周期尚无可查询的时间区间。');
  const filter = allFilters(); filter.source_id = {mode: 'exact', value: source.source.source_id};
  const query = {schema_version: 1, ...validateQuery({from: runtime.period_start, to: Math.min(to, runtime.next_reset_at), utc_offset_minutes: offset, grain: 'hour', filter})};
  return {query, periodKey: `${source.source.source_id}:${source.source.config_revision}:${runtime.period_id}`};
}
const totalsFields = record('totals', { input: integer, output: integer, total: integer, consumption_rows: integer });
export function validateTotals(value) { const t = totalsFields(value); requireThat(safeAdd(t.input, t.output) === t.total && (t.consumption_rows > 0 || t.total === 0)); return t; }
const modelUsage = record('modelUsage', { model: nullable(modelLabel), usage: validateTotals });
const tokenUsage = record('tokenUsage', { native_token_id: nullable(id), usage: validateTotals });
const groupUsage = record('groupUsage', { group_id: nullable(id), usage: validateTotals });
const bucketFields = record('bucket', { from: integer, to: integer, total: validateTotals, models: array(modelUsage, L.bucketModels) });
const usageReport = record('usageReport', { total: validateTotals, models: array(modelUsage, L.usageModels), tokens: array(tokenUsage, L.usageTokens), groups: array(groupUsage, L.usageGroups), buckets: array(bucketFields, L.usageBuckets) });
const scanWindowFields = record('scanWindow', { from: integer, to: revision });
function scanWindow(value) { const w = scanWindowFields(value); requireThat(w.from < w.to); return w; }
const scanFields = record('scan', { lane: enumeration('scanLane'), generation: integer, window: nullable(scanWindow), next_page: revision, phase: enumeration('scanPhase'), last_observed_at: nullable(integer), fault: enumeration('scanFault') });
function scan(value) {
  const s = scanFields(value);
  if (s.phase === 'idle') requireThat(s.generation === 0 && s.window === null && s.next_page === 1 && s.last_observed_at === null && s.fault === null);
  else {
    requireThat(s.generation > 0 && s.window !== null);
    if (s.phase === 'stalled') requireThat(s.fault !== null && s.last_observed_at !== null);
    else requireThat(s.fault === null);
    if (s.phase === 'scanned') requireThat(s.last_observed_at !== null);
  }
  return s;
}
const ledgerFields = record('ledger', { revision, initial_requested_from: nullable(integer), scans: array(scan, 2, 2) });
function ledger(value) {
  const l = ledgerFields(value); requireThat(l.scans[0].lane === 'recent' && l.scans[1].lane === 'history');
  if (l.initial_requested_from === null) requireThat(l.scans.every(s => s.phase === 'idle'));
  return l;
}
const usageSnapshot = record('usageSnapshot', { schema_version: version('usageSnapshot'), query: validateQuery, ledger, usage: nullable(usageReport) });
const zeroTotals = () => ({input: 0, output: 0, total: 0, consumption_rows: 0});
function addTotals(a, b) { return {input: safeAdd(a.input, b.input), output: safeAdd(a.output, b.output), total: safeAdd(a.total, b.total), consumption_rows: safeAdd(a.consumption_rows, b.consumption_rows)}; }
function equalTotals(a, b) { requireThat(a.input === b.input && a.output === b.output && a.total === b.total && a.consumption_rows === b.consumption_rows); }
function sumTotals(parts, total) { let sum = zeroTotals(); for (const p of parts) sum = addTotals(sum, p); equalTotals(sum, total); }
export function validateUsage(value, expectedQuery) {
  const snapshot = usageSnapshot(value), expected = validateQuery(expectedQuery);
  requireThat(JSON.stringify(snapshot.query) === JSON.stringify(expected));
  requireThat((snapshot.ledger.initial_requested_from === null) === (snapshot.usage === null));
  if (snapshot.usage === null) return snapshot;
  const report = snapshot.usage;
  unique(report.models, m => modelKey(m.model)); unique(report.tokens, t => t.native_token_id); unique(report.groups, g => g.group_id);
  sumTotals(report.models.map(m => m.usage), report.total); sumTotals(report.tokens.map(t => t.usage), report.total); sumTotals(report.groups.map(g => g.usage), report.total);
  sumTotals(report.buckets.map(b => b.total), report.total);
  const joint = new Map(); let items = 0, cursor = snapshot.query.from;
  for (const bucket of report.buckets) {
    requireThat(bucket.from === cursor && bucket.from < bucket.to && bucket.to <= snapshot.query.to);
    cursor = bucket.to; items += bucket.models.length; requireThat(items <= L.bucketModelItems);
    unique(bucket.models, m => modelKey(m.model)); sumTotals(bucket.models.map(m => m.usage), bucket.total);
    for (const m of bucket.models) { const key = modelKey(m.model); joint.set(key, addTotals(joint.get(key) ?? zeroTotals(), m.usage)); }
  }
  requireThat(cursor === snapshot.query.to);
  requireThat(joint.size === report.models.length);
  for (const m of report.models) { const value = joint.get(modelKey(m.model)); requireThat(value !== undefined); equalTotals(value, m.usage); }
  return snapshot;
}

export class ActionError extends Error { constructor(message) { super(message); this.name = 'ActionError'; } }
function ensureAction(condition, message) { if (!condition) throw new ActionError(message); }
function freeze(value) { if (value && typeof value === 'object') { for (const v of Object.values(value)) freeze(v); Object.freeze(value); } return value; }
const PENDING_KEY = 'qt.pending.v1';
export const MAX_PENDING = 16;
export class PendingOperations {
  constructor(storage, now = () => performance.now()) {
    this.storage = storage; this.now = now; this.items = new Map(); this.storageError = null; this.cursor = 0;
    try {
      const text = storage.getItem(PENDING_KEY);
      if (text === null) return;
      requireThat(text.length <= 4096);
      const saved = JSON.parse(text);
      requireThat(saved && saved.schema_version === 1 && Object.keys(saved).sort().join(',') === 'operation_ids,schema_version');
      array(operationId, MAX_PENDING)(saved.operation_ids); unique(saved.operation_ids, v => v);
      for (const id of saved.operation_ids) this.items.set(id, this.entry(id));
    } catch { this.storageError = '操作恢复存储不可用或已损坏；不能发起新的持久操作。可手动输入 ID 查询。'; }
  }
  entry(id) { return { id, binding: null, payload: null, descriptor: null, receipt: null, error: null, started: this.now(), nextPoll: 0, tracking: true }; }
  persist(ids) {
    ensureAction(this.storageError === null, this.storageError);
    try { this.storage.setItem(PENDING_KEY, JSON.stringify({ schema_version: 1, operation_ids: ids })); }
    catch { this.storageError = '操作恢复存储写入失败，未发送新操作。'; throw new ActionError(this.storageError); }
  }
  begin(descriptor, binding, buildBody, crypto) {
    // Step U5: fetch 前仅保存新操作 ID；冻结载荷只存在于当前 document。
    ensureAction(this.items.size < MAX_PENDING, '最多追踪 16 个操作，请先移除已核对的记录。');
    ensureAction(this.storageError === null, this.storageError);
    ensureAction(typeof crypto?.randomUUID === 'function', '浏览器没有安全随机 ID 能力，不能发起持久操作。');
    let id;
    try { id = `qt_${crypto.randomUUID()}`; } catch { throw new ActionError('无法生成安全操作 ID。'); }
    operationId(id); ensureAction(id.length === 39 && !this.items.has(id), '操作 ID 无效或重复，未发送。');
    const body = buildBody(id), serialized = JSON.stringify(body);
    requireThat(new TextEncoder().encode(serialized).length <= 1024 * 1024 && body.operation_id === id);
    enumeration('operationKind')(binding.kind); if (binding.targetId !== null) idValue(binding.targetId);
    this.persist([...this.items.keys(), id]);
    const item = this.entry(id);
    item.binding = { kind: binding.kind, target_type: targetType(binding.kind), target_id: binding.targetId };
    item.payload = freeze(JSON.parse(serialized)); item.descriptor = descriptor;
    this.items.set(id, item); return item;
  }
  restore(id) {
    operationId(id);
    if (this.items.has(id)) return this.items.get(id);
    ensureAction(this.items.size < MAX_PENDING, '追踪列表已满。');
    if (this.storageError === null) { try { this.persist([...this.items.keys(), id]); } catch { /* Manual read recovery remains available. */ } }
    const item = this.entry(id); this.items.set(id, item); return item;
  }
  accept(id, receipt) {
    operationId(id); const item = this.items.get(id); requireThat(item !== undefined);
    const value = validateOperation(receipt); requireThat(value.operation_id === id);
    if (item.binding) {
      requireThat(value.kind === item.binding.kind && value.target_type === item.binding.target_type);
      requireThat(item.binding.target_id === null || value.target_id === item.binding.target_id);
    }
    item.binding = { kind: value.kind, target_type: value.target_type, target_id: value.target_id };
    // 调用者持有当前因果 ticket；不能按 phase、revision 或秒级时间排序回执。
    item.receipt = value; item.error = null; item.nextPoll = this.now() + 5000;
    if (['succeeded', 'superseded', 'failed'].includes(value.phase)) item.tracking = false;
    return item;
  }
  failed(id, error) { const item = this.items.get(id); if (item) { item.error = error; item.nextPoll = this.now() + 5000; } }
  continue(id) { const item = this.items.get(id); if (item) { item.started = this.now(); item.tracking = true; item.nextPoll = 0; } }
  due() {
    const items = [...this.items.values()], now = this.now();
    for (let n = 0; n < items.length; n++) {
      const index = (this.cursor + n) % items.length, item = items[index];
      if (now < item.started || now - item.started >= 60000) item.tracking = false;
      if (item.tracking && now >= item.nextPoll) {
        this.cursor = (index + 1) % items.length; item.nextPoll = now + 5000; return item;
      }
    }
    return null;
  }
  dismiss(id) {
    if (!this.items.has(id)) return;
    const ids = [...this.items.keys()].filter(v => v !== id);
    // 不覆盖损坏或不可用的存储；删除内存追踪不代表取消后端操作。
    if (this.storageError === null) this.persist(ids);
    this.items.delete(id);
  }
}
const idValue = id;

export function freezeDraft(entity, recordValue) { return freeze({ entity, value: structuredClone(recordValue) }); }
function currentEntity(draft, state, action) {
  const original = draft.value;
  let latest;
  if (draft.entity === 'source') latest = state.catalog.sources.find(p => p.source.source_id === original.source.source_id);
  if (draft.entity === 'group') latest = state.catalog.groups.find(p => p.group.group_id === original.group.group_id);
  if (draft.entity === 'membership') latest = state.catalog.memberships.find(p => p.membership.membership_id === original.membership.membership_id);
  ensureAction(latest !== undefined, '对象已不在当前目录，请重新载入。');
  const stamp = p => draft.entity === 'source' ? [p.source.config_revision, ...(['clearError', 'setVolcanoPolicy'].includes(action) ? [p.volcano?.runtime?.revision ?? 0] : [])] : draft.entity === 'group' ? [p.group.revision, ...(action === 'setGroupPin' ? [p.policy_revision] : [])] : [p.control_revision];
  ensureAction(JSON.stringify(stamp(latest)) === JSON.stringify(stamp(original)), '对象已有新版本，请重新载入并确认，不会自动重发。');
  return original;
}
export function draftConflict(draft, state, action) { try { currentEntity(draft, state, action); return false; } catch { return true; } }
function jsonArray(text) { try { const value = JSON.parse(text); requireThat(Array.isArray(value)); return value; } catch { throw new ActionError('请输入合法 JSON 数组。'); } }
function headers(text) { return array(record('requestHeader', { key: string, value: string }), 64)(jsonArray(text)); }
function parsePolicy(fields) {
  return policy({ daily_token_limit: fields.daily_token_limit === '' ? null : decimal(fields.daily_token_limit, 1),
    failure_threshold: decimal(fields.failure_threshold, 1, L.failureThreshold), reset_minute: decimal(fields.reset_minute, 0, L.resetMinute),
    utc_offset_minutes: decimal(fields.utc_offset_minutes, L.offsetMin, L.offsetMax) });
}
export function buildAction(action, draft, fields, state, op = null) {
  // Step U6: 固定动作白名单和分类版本；不提供任意 priority/status/owner-user 字段。
  const base = { schema_version: 1 }, operationBase = expected_revision => ({ ...base, operation_id: operationId(op), expected_revision: revision(expected_revision) });
  let body, targetId = null, kind = null;
  if (action === 'createSource') {
    const type = enumeration('sourceKind')(fields.type), selection = enumeration('modelSelection')(fields.model_selection);
    const upstream = { channel_type: decimal(fields.channel_type, 1), base_url: string(fields.base_url),
      model_discovery: fields.models_url === '' ? null : { url: string(fields.models_url), auth: enumeration('discoveryAuth')(fields.discovery_auth) } };
    const settings = type === 'glm' ? { type, upstream, quota: { quota_url: string(fields.quota_url), global_headers: headers(fields.global_headers), key_headers: headers(fields.key_headers) } } : { type, upstream, policy: parsePolicy(fields) };
    body = { ...base, name: string(fields.name), note: string(fields.note), credential: string(fields.credential), settings,
      model_selection: selection === 'discover' ? { mode: selection } : { mode: selection, manual_models: catalogModels(jsonArray(fields.models)) } };
  } else if (action === 'createGroup') {
    body = { ...base, name: string(fields.name), newapi_group: nativeGroup(fields.newapi_group), type: enumeration('sourceKind')(fields.type) };
  } else if (action === 'planMember') {
    const s = state.catalog.sources.find(p => p.source.source_id === fields.source_id), g = state.catalog.groups.find(p => p.group.group_id === fields.group_id);
    ensureAction(s && g && s.source.type === g.group.type && s.source.retired_at === null && !s.retire_requested, '来源和组必须存在、类型一致且来源未归档。');
    body = { ...base, source_id: id(fields.source_id), group_id: id(fields.group_id) };
  } else if (['setSourcePaused', 'retireSource', 'setModels', 'discoverModels', 'clearError', 'setVolcanoPolicy'].includes(action)) {
    const p = currentEntity(draft, state, action); ensureAction(draft.entity === 'source' && p.source.retired_at === null && !p.retire_requested, '来源已归档或正在归档。');
    targetId = p.source.source_id; body = operationBase(p.source.config_revision);
    if (action === 'setSourcePaused') { kind = 'set_source_paused'; body.paused = boolean(fields.paused); }
    if (action === 'retireSource') kind = 'retire_source';
    if (action === 'setModels') { kind = 'sync_source_models'; body.models = catalogModels(jsonArray(fields.models)); }
    if (action === 'discoverModels') kind = 'discover_source_models';
    if (action === 'clearError' || action === 'setVolcanoPolicy') {
      ensureAction(p.source.type === 'volcano', '此操作仅适用于火山来源。');
      body.expected_runtime_revision = p.volcano.runtime?.revision ?? 0;
      if (action === 'clearError') { revision(body.expected_runtime_revision); kind = 'clear_volcano_error'; }
      else { kind = 'update_volcano_policy'; body.policy = parsePolicy(fields); }
    }
  } else if (['ensureMember', 'retireMember'].includes(action)) {
    const p = currentEntity(draft, state, action); ensureAction(draft.entity === 'membership' && p.membership.retired_at === null && !p.retire_requested, '成员已移除或正在移除。');
    body = operationBase(p.control_revision); targetId = p.membership.membership_id; kind = action === 'ensureMember' ? 'ensure_member' : 'retire_member';
  } else if (['setGroupPin', 'checkGroup', 'createToken'].includes(action)) {
    const p = currentEntity(draft, state, action); ensureAction(draft.entity === 'group', '请选择组。'); targetId = p.group.group_id;
    if (action === 'checkGroup') body = { ...base, expected_revision: p.group.revision, register: boolean(fields.register) };
    if (action === 'setGroupPin') {
      ensureAction(p.group.type === 'glm', '固定来源仅适用于 GLM 组。'); const memberId = nullable(id)(fields.membership_id);
      ensureAction(memberId === null || state.catalog.memberships.some(m => m.membership.membership_id === memberId && m.membership.group_id === targetId && m.membership.retired_at === null), '成员不属于该组。');
      body = { ...operationBase(p.group.revision), expected_policy_revision: p.policy_revision, membership_id: memberId }; kind = 'set_group_pin';
    }
    if (action === 'createToken') {
      body = { ...base, operation_id: operationId(op), group_id: targetId, expected_group_revision: p.group.revision, display_name: string(fields.display_name), expires_at: fields.expires_at === '' ? null : decimal(fields.expires_at) };
      targetId = null; kind = 'create_token';
    }
  } else if (action === 'registerToken') {
    body = { ...base, owner_scope: enumeration('ownerScope')(fields.owner_scope), native_token_id: id(fields.native_token_id), display_name: string(fields.display_name) };
  } else if (['setTokenEnabled', 'deleteToken', 'reveal'].includes(action)) {
    const i = validateIdentity(draft.value); ensureAction(draft.entity === 'identity' && i.deleted_at === null, '此身份已删除。'); targetId = i.token_id;
    body = action === 'reveal' ? { ...base, expected_revision: i.revision } : operationBase(i.revision);
    if (action === 'setTokenEnabled') { body.enabled = boolean(fields.enabled); kind = 'set_token_enabled'; }
    if (action === 'deleteToken') kind = 'delete_token';
  } else throw new ActionError('未知操作。');
  return { body, targetId, kind };
}
