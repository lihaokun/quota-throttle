import { emptyState, usageSnapshot } from './fixtures.mjs';
import { allFilters } from '../../src/panel_http/ui/core.mjs';
// Mirrors the accepted A1 capacity fixture: 1000 sources, 100 groups, 10000 distinct memberships.
export function maximumPanel() {
  const safe = Number.MAX_SAFE_INTEGER, at = safe - 86400000, base = 9223372036854755807n;
  const value = emptyState(at); value.glm.cycle_id = '18446744073709551615'; value.glm.completed_at_ms = at;
  for (let i = 0; i < 1000; i++) {
    const sourceId = String(base + BigInt(i));
    value.catalog.sources.push({source: {source_id: sourceId, type: 'glm', name: '\u0001'.repeat(128), note: '\u0001'.repeat(2048), manual_paused: false, retired_at: null, config_revision: safe}, retire_requested: false, model_count: 1000, volcano: null});
    value.glm.sources.push({source_id: sourceId, source_revision: safe, observed_at_ms: at, state: 'observed', five_hour: {percentage: 2.2250738585072014e-308, reset_at_ms: at + 600000}, weekly: {percentage: 2.2250738585072014e-308, reset_at_ms: at + 600000}, level: '\u0001'.repeat(128)});
  }
  for (let g = 0; g < 100; g++) {
    const groupId = String(base + BigInt(g)), members = [];
    for (let k = 0; k < 100; k++) {
      const index = g * 100 + k, memberId = String(base + BigInt(index)), priority = k === 0 ? safe : -safe; members.push(memberId);
      value.catalog.memberships.push({membership: {membership_id: memberId, source_id: value.catalog.sources[index % 1000].source.source_id, group_id: groupId, channel_id: memberId, order: safe - 100 + k, lifecycle: 'ready', desired_priority: priority, observed_status: 1, observed_priority: priority, applied_revision: safe, retired_at: null}, control_revision: safe, status_directive: 'preserve', retire_requested: false, create_state: 'resolved', dirty: false, next_attempt_at: safe, error_code: 'transport_unknown'});
    }
    value.catalog.groups.push({group: {group_id: groupId, newapi_group: `g${String(g).padStart(3, '0')}${'a'.repeat(60)}`, name: '\u0001'.repeat(128), type: 'glm', revision: safe, ready: true}, policy_revision: safe, selected_member_id: members[0], pinned_member_id: members[0], regime: 'normal'});
    value.glm.groups.push({group_id: groupId, cycle_id: '18446744073709551615', evaluated_at_ms: at, current: true, imminent_members: members, pin_release: null});
  }
  return value;
}
export function maximumUsage() {
  const query = {schema_version: 1, from: 1, to: 31 * 86400 + 1, utc_offset_minutes: 480, grain: 'hour', filter: allFilters()};
  const snapshot = usageSnapshot(query);
  const labels = Array.from({length: 1024}, (_, n) => { const prefix = `model-${n}-`; return n === 0 ? null : prefix + 'x'.repeat(256 - prefix.length); });
  const total = count => ({input: count, output: count * 2, total: count * 3, consumption_rows: count});
  const buckets = snapshot.usage.buckets.map(b => ({...b, total: total(0), models: []}));
  for (let n = 0; n < 65536; n++) { const bucket = buckets[n % buckets.length]; bucket.models.push({model: labels[n % 1024], usage: total(1)}); bucket.total = total(bucket.models.length); }
  snapshot.usage = {total: total(65536), models: labels.map(model => ({model, usage: total(64)})), tokens: Array.from({length: 4096}, (_, n) => ({native_token_id: String(9223372036854775807n - BigInt(n)), usage: total(16)})), buckets};
  return snapshot;
}
