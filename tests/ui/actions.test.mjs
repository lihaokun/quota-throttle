import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { PendingOperations, freezeDraft, buildAction, draftConflict, validateOperation, validateIdentity } from '../../src/panel_http/ui/core.mjs';
import { richState, operation, identity } from './fixtures.mjs';
import { PanelApp } from '../../src/panel_http/ui/app.mjs';
import { route } from '../../src/panel_http/ui/client.mjs';
const opId = operation().operation_id;
function storage() { const values = new Map(); return { getItem: k => values.get(k) ?? null, setItem: (k, v) => values.set(k, v), values }; }
function cryptoIds() { let counter = 0; return { randomUUID: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, '0')}` }; }
function begin(pending, crypto = cryptoIds(), binding = { kind: 'set_source_paused', targetId: '1' }) {
  return pending.begin({ action: 'setSourcePaused' }, binding, id => ({ schema_version: 1, operation_id: id, expected_revision: 3, paused: true }), crypto);
}
test('u5 persistent actions save IDs only and freeze document-local payload before sending', () => {
  const s = storage(), p = new PendingOperations(s, () => 0), item = begin(p);
  assert.deepEqual(JSON.parse(s.values.get('qt.pending.v1')), { schema_version: 1, operation_ids: [item.id] });
  assert.equal(Object.isFrozen(item.payload), true); assert.throws(() => { item.payload.expected_revision = 4; });
  const restored = new PendingOperations(s, () => 1000).items.get(item.id);
  assert.equal(restored.payload, null); assert.equal(restored.descriptor, null); assert.equal(restored.binding, null);
});
test('u5 corrupted/unavailable/full storage and missing UUID forbid new persistent actions', () => {
  for (const s of [{ getItem() { throw new Error(); }, setItem() {} }, { getItem() { return null; }, setItem() { throw new Error(); } }]) {
    const p = new PendingOperations(s); assert.throws(() => begin(p)); assert.equal(p.items.size, 0);
  }
  const corrupt = storage(); corrupt.setItem('qt.pending.v1', '{broken'); const bad = new PendingOperations(corrupt);
  assert.throws(() => begin(bad)); bad.restore(opId); assert.equal(bad.items.size, 1); assert.equal(corrupt.getItem('qt.pending.v1'), '{broken');
  const p = new PendingOperations(storage()), crypto = cryptoIds();
  assert.throws(() => begin(p, {}));
  for (let i = 0; i < 16; i++) begin(p, crypto);
  assert.throws(() => begin(p, crypto)); assert.equal(p.items.size, 16);
});
test('u5 first receipt binds restored ID and create-token learns token target not input group', () => {
  const p = new PendingOperations(storage(), () => 0); p.restore(opId);
  p.accept(opId, operation({ target_id: '901' }));
  assert.throws(() => p.accept(opId, operation({ target_id: '902' })));
  const created = begin(p, cryptoIds(), {kind: 'create_token', targetId: null});
  p.accept(created.id, operation({ operation_id: created.id, kind: 'create_token', target_type: 'token', target_id: '456', target_revision: 1 }));
  assert.equal(created.binding.target_id, '456');
  assert.throws(() => p.accept(created.id, operation({ operation_id: created.id, kind: 'create_token', target_type: 'token', target_id: '457' })));
});
test('u5 same-second phases and retry execution are accepted without invented phase ordering', () => {
  const p = new PendingOperations(storage(), () => 0); p.restore(opId);
  for (const phase of ['queued', 'applying', 'unknown', 'waiting', 'failed', 'applying', 'succeeded']) {
    p.accept(opId, operation({ phase, error_code: phase === 'failed' ? 'remote_rejected' : null, updated_at: 100 }));
    assert.equal(p.items.get(opId).receipt.phase, phase);
  }
  assert.throws(() => validateOperation(operation({ phase: 'succeeded', error_code: 'storage' })));
  assert.throws(() => validateOperation(operation({ phase: 'failed', error_code: null })));
  assert.throws(() => validateOperation(operation({kind: 'clear_volcano_error', phase: 'queued'})));
  assert.equal(validateOperation(operation({kind: 'clear_volcano_error', phase: 'succeeded', total_targets: 0, confirmed_targets: 0})).total_targets, 0);
  assert.throws(() => validateIdentity({ ...identity('1', null), origin: 'imported' }));
});
test('u5 polling is bounded, round-robin, pauses at 60 seconds and dismiss never cancels backend', () => {
  let now = 0; const s = storage(), p = new PendingOperations(s, () => now), crypto = cryptoIds();
  const a = begin(p, crypto), b = begin(p, crypto);
  assert.equal(p.due().id, a.id); assert.equal(p.due().id, b.id); assert.equal(p.due(), null);
  now = 4999; assert.equal(p.due(), null); now = 5000; assert.equal(p.due().id, a.id);
  p.failed(a.id, 'not_found'); assert.equal(p.items.has(a.id), true);
  now = 60000; assert.equal(p.due(), null); assert.equal(p.items.size, 2);
  p.continue(a.id); assert.equal(p.due().id, a.id); p.dismiss(a.id);
  assert.deepEqual(JSON.parse(s.getItem('qt.pending.v1')).operation_ids, [b.id]);
});
test('u5 retry with a lost response polls the original ID despite the old failed receipt', () => {
  let now = 0; const p = new PendingOperations(storage(), () => now); p.restore(opId);
  p.accept(opId, operation({phase: 'failed', error_code: 'remote_rejected'})); assert.equal(p.due(), null);
  p.continue(opId); p.failed(opId, 'result_unconfirmed'); now = 5000;
  assert.equal(p.due().id, opId);
  p.accept(opId, operation({phase: 'applying'})); assert.equal(p.items.get(opId).receipt.phase, 'applying');
});
test('u5 application retry and resubmit send the original ID and unchanged body', () => {
  const p = new PendingOperations(storage(), () => 0), item = begin(p), calls = [], app = Object.create(PanelApp.prototype);
  item.descriptor = route('setSourcePaused', {id: '1'});
  Object.assign(app, {pending: p, lifecycle: {active: true}, paint() {}, work(descriptor, body) {calls.push({descriptor, body});}});
  Object.defineProperty(app, 'readOnly', {value: false});
  app.operation('resubmit', item.id); assert.equal(calls[0].body, item.payload); assert.equal(calls[0].body.expected_revision, 3); assert.equal(calls[0].body.operation_id, item.id);
  p.accept(item.id, operation({operation_id: item.id, phase: 'failed', error_code: 'remote_rejected'}));
  app.operation('retry', item.id); assert.equal(calls[1].descriptor.path, `/api/admin/v1/operations/${item.id}/retry`); assert.deepEqual(calls[1].body, {schema_version: 1});
  assert.equal(p.items.size, 1); assert.equal(item.payload.expected_revision, 3); assert.equal(item.payload.operation_id, item.id);
});
test('u6 source/member/pin bodies use separate frozen revision categories', () => {
  const s = richState(); s.catalog.sources[0].source.config_revision = 11; s.catalog.memberships[0].control_revision = 22;
  s.catalog.groups[0].group.revision = 33; s.catalog.groups[0].policy_revision = 44;
  const source = freezeDraft('source', s.catalog.sources[0]), member = freezeDraft('membership', s.catalog.memberships[0]), group = freezeDraft('group', s.catalog.groups[0]);
  assert.deepEqual(buildAction('setSourcePaused', source, {paused: true, priority: 1000}, s, opId).body, {schema_version: 1, operation_id: opId, expected_revision: 11, paused: true});
  assert.equal(buildAction('ensureMember', member, {}, s, opId).body.expected_revision, 22);
  assert.deepEqual(buildAction('setGroupPin', group, {membership_id: '3'}, s, opId).body, {schema_version: 1, operation_id: opId, expected_revision: 33, expected_policy_revision: 44, membership_id: '3'});
  s.catalog.sources[0].source.config_revision++;
  assert.equal(draftConflict(source, s, 'setSourcePaused'), true); assert.throws(() => buildAction('setSourcePaused', source, {paused: true}, s, opId));
  assert.equal(source.value.source.config_revision, 11);
});
test('u6 source creation uses explicit discovery settings and excludes unrelated privileged fields', () => {
  const fields = {type: 'glm', name: 'Synthetic', note: '', credential: 'FAKE_NOT_A_KEY', channel_type: '8', base_url: 'https://example.test/chat/completions', models_url: 'https://example.test/models', discovery_auth: 'bearer', model_selection: 'discover', models: '[]', quota_url: 'https://example.test/quota?type=2', global_headers: '[]', key_headers: '[{"key":"Test-Project","value":"synthetic"}]', priority: 999, owner_user_id: '123'};
  const result = buildAction('createSource', null, fields, richState());
  assert.equal(result.kind, null); assert.deepEqual(result.body.model_selection, {mode: 'discover'});
  assert.deepEqual(result.body.settings.upstream.model_discovery, {url: 'https://example.test/models', auth: 'bearer'});
  assert.equal(result.body.owner_user_id, undefined); assert.equal(result.body.priority, undefined);
  assert.deepEqual(result.body.settings.quota.key_headers, [{key: 'Test-Project', value: 'synthetic'}]);
  assert.throws(() => buildAction('createSource', null, {...fields, model_selection: 'manual'}, richState()));
});
test('u6 model JSON arrays preserve long/newline labels and membership planning does not Ensure', () => {
  const s = richState(), draft = freezeDraft('source', s.catalog.sources[0]), models = ['a\nb', 'x'.repeat(1000)];
  const result = buildAction('setModels', draft, {models: JSON.stringify(models)}, s, opId);
  assert.deepEqual(result.body.models, models);
  assert.deepEqual(buildAction('planMember', null, {source_id: '1', group_id: '2'}, s), {body: {schema_version: 1, source_id: '1', group_id: '2'}, targetId: null, kind: null});
  const other = structuredClone(s); other.catalog.groups[0].group.type = 'volcano';
  assert.throws(() => buildAction('planMember', null, {source_id: '1', group_id: '2'}, other));
});
test('u6 volcano policy is runtime-version guarded without blocking config-only pause on usage refresh', () => {
  const s = richState(); s.catalog.sources[0].source.type = 'volcano';
  s.catalog.sources[0].volcano = { policy: {daily_token_limit: null, failure_threshold: 3, reset_minute: 60, utc_offset_minutes: 480}, runtime: null };
  const draft = freezeDraft('source', s.catalog.sources[0]);
  const fields = {daily_token_limit: '', failure_threshold: '3', reset_minute: '60', utc_offset_minutes: '480'};
  assert.equal(buildAction('setVolcanoPolicy', draft, fields, s, opId).body.expected_runtime_revision, 0);
  assert.throws(() => buildAction('clearError', draft, {}, s, opId));
  s.catalog.sources[0].volcano.runtime = {revision: 7};
  assert.equal(buildAction('setSourcePaused', draft, {paused: true}, s, opId).body.expected_revision, 1);
  assert.throws(() => buildAction('setVolcanoPolicy', draft, fields, s, opId));
});
test('u7 token creation uses group revision without owner override; reveal uses local ID revision', () => {
  const s = richState(), g = freezeDraft('group', s.catalog.groups[0]);
  const created = buildAction('createToken', g, {display_name: 'client', expires_at: '', owner_scope: 'legacy_admin'}, s, opId);
  assert.deepEqual(created.body, { schema_version: 1, operation_id: opId, group_id: '2', expected_group_revision: 1, display_name: 'client', expires_at: null });
  assert.equal(created.targetId, null);
  const i = freezeDraft('identity', identity('100', '200'));
  assert.deepEqual(buildAction('reveal', i, {}, s), { body: {schema_version: 1, expected_revision: 1}, targetId: '100', kind: null });
});
test('pbt5 operation storage/reload/phase sequences keep IDs and immutable payload bindings', () => {
  fc.assert(fc.property(fc.array(fc.constantFrom('queued', 'applying', 'waiting', 'unknown', 'failed', 'succeeded'), {minLength: 1, maxLength: 100}), fc.bigInt({min: 1n, max: 9223372036854775807n}), (phases, target) => {
    const s = storage(), p = new PendingOperations(s, () => 0), item = begin(p, cryptoIds(), {kind: 'set_source_paused', targetId: target.toString()});
    const original = JSON.stringify(item.payload), q = new PendingOperations(s, () => 0);
    for (const phase of phases) {
      const receipt = operation({operation_id: item.id, target_id: target.toString(), phase, error_code: phase === 'failed' ? 'storage' : null});
      p.accept(item.id, receipt); q.accept(item.id, receipt);
      assert.equal(p.items.get(item.id).receipt.phase, phase); assert.equal(q.items.get(item.id).receipt.phase, phase);
      assert.equal(JSON.stringify(item.payload), original); assert.equal(q.items.get(item.id).payload, null);
      assert.deepEqual(JSON.parse(s.getItem('qt.pending.v1')).operation_ids, [item.id]);
    }
  }), { numRuns: 128, verbose: true });
});
