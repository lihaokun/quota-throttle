export function emptyState(at = 100000) {
  return { schema_version: 1, captured_at_ms: at, backend_ready: true,
    catalog: { sources: [], groups: [], memberships: [] },
    glm: { cycle_id: null, completed_at_ms: null, sources: [], groups: [] } };
}
export function richState(at = 100000) {
  const s = emptyState(at);
  s.catalog.sources = [{ source: { source_id: '1', type: 'glm', name: '智谱来源', note: '', manual_paused: false, retired_at: null, config_revision: 1 }, retire_requested: false, model_count: 2, volcano: null }];
  s.catalog.groups = [{ group: { group_id: '2', newapi_group: 'glm-main', name: 'GLM 组', type: 'glm', revision: 1, ready: true }, policy_revision: 1, selected_member_id: '3', pinned_member_id: null, regime: 'normal' }];
  s.catalog.memberships = [{ membership: { membership_id: '3', source_id: '1', group_id: '2', channel_id: '4', order: 0, lifecycle: 'ready', desired_priority: 100, observed_status: 1, observed_priority: 100, applied_revision: 1, retired_at: null }, control_revision: 1, status_directive: 'preserve', retire_requested: false, create_state: 'resolved', dirty: false, next_attempt_at: 0, error_code: null }];
  s.glm = { cycle_id: '18446744073709551615', completed_at_ms: at - 1000,
    sources: [{ source_id: '1', source_revision: 1, observed_at_ms: at - 1000, state: 'observed', five_hour: { percentage: 12.5, reset_at_ms: at + 100000 }, weekly: { percentage: 20, reset_at_ms: at + 50000 }, level: 'Max' }],
    groups: [{ group_id: '2', cycle_id: '18446744073709551615', evaluated_at_ms: at - 1000, current: true, imminent_members: ['3'], pin_release: null }] };
  return s;
}
export function identity(token = '12', native = '34', scope = 'issuer', owner = '56') {
  return { schema_version: 1, token_id: token, native_token_id: native, owner_user_id: owner, owner_scope: scope,
    origin: native === null ? 'created' : 'imported', group_id: '2', explicit_group: 'glm-main', display_name: '测试 Key', revision: 1,
    create_state: native === null ? 'never' : 'resolved', created_at: 100, deleted_at: null };
}
export function native(token = '34', scope = 'issuer', owner = '56') {
  return { schema_version: 1, token_id: token, owner_user_id: owner, owner_scope: scope, name: '测试 Key',
    masked_key: 'sk-a****xyz', explicit_group: 'glm-main', effective_group: 'glm-main', status: 'enabled',
    created_at: 100, expires_at: null, remaining_quota: -10, used_quota: 10, unlimited_quota: false,
    model_limits_enabled: false, model_limits: '', allow_ips: null, cross_group_retry: false };
}
export function operation(overrides = {}) {
  return { schema_version: 3, operation_id: 'qt_00000000-0000-4000-8000-000000000000', kind: 'set_source_paused',
    target_type: 'source', target_id: '1', phase: 'queued', target_revision: 1, total_targets: 1, confirmed_targets: 0,
    error_code: null, updated_at: 100, ...overrides };
}
export function usageSnapshot(query, rows = [], initialized = true) {
  const zero = () => ({input: 0, output: 0, total: 0, consumption_rows: 0});
  const sum = (a, b) => ({input: a.input + b.input, output: a.output + b.output, total: a.total + b.total, consumption_rows: a.consumption_rows + b.consumption_rows});
  const range = {from: query.from, to: query.to, utc_offset_minutes: query.utc_offset_minutes, grain: query.grain, filter: structuredClone(query.filter)};
  const snapshot = {schema_version: 1, query: range, ledger: {revision: 1, initial_requested_from: initialized ? 0 : null, scans: ['recent', 'history'].map(lane => ({lane, generation: 0, window: null, next_page: 1, phase: 'idle', last_observed_at: null, fault: null}))}, usage: null};
  if (!initialized) return snapshot;
  const grain = query.grain === 'hour' ? 3600 : 86400, shift = query.utc_offset_minutes * 60;
  const buckets = []; let cursor = query.from;
  while (cursor < query.to) { const to = Math.min(query.to, Math.floor((cursor + shift) / grain) * grain + grain - shift); buckets.push({from: cursor, to, total: zero(), models: new Map()}); cursor = to; }
  const models = new Map(), tokens = new Map(), groups = new Map(); let total = zero();
  for (const row of rows) {
    const amount = {input: row.input, output: row.output, total: row.input + row.output, consumption_rows: 1};
    const bucket = buckets.find(b => row.at >= b.from && row.at < b.to); if (!bucket) throw new Error('fixture fact outside query');
    total = sum(total, amount); bucket.total = sum(bucket.total, amount);
    models.set(row.model, sum(models.get(row.model) ?? zero(), amount)); tokens.set(row.token, sum(tokens.get(row.token) ?? zero(), amount));
    const group = row.group ?? null; groups.set(group, sum(groups.get(group) ?? zero(), amount));
    bucket.models.set(row.model, sum(bucket.models.get(row.model) ?? zero(), amount));
  }
  snapshot.usage = {total, models: [...models].map(([model, usage]) => ({model, usage})), tokens: [...tokens].map(([native_token_id, usage]) => ({native_token_id, usage})), groups: [...groups].map(([group_id, usage]) => ({group_id, usage})), buckets: buckets.map(b => ({...b, models: [...b.models].map(([model, usage]) => ({model, usage}))}))};
  return snapshot;
}
export function volcanoState(at = 100000) {
  const s = richState(at); s.catalog.sources[0].source.type = 'volcano';
  s.catalog.sources[0].volcano = {policy: {daily_token_limit: null, failure_threshold: 3, reset_minute: 60, utc_offset_minutes: 480}, runtime: {revision: 1, period_id: 1, period_start: Math.max(0, Math.floor(at / 1000) - 3600), next_reset_at: Math.floor(at / 1000) + 1, used_input: 0, used_output: 0, consecutive_failures: 0, error_exhausted: false, limit_exhausted: false, coverage_end: null}};
  s.catalog.groups[0].group.type = 'volcano'; s.catalog.groups[0].regime = null;
  s.glm = {cycle_id: null, completed_at_ms: null, sources: [], groups: []}; return s;
}
