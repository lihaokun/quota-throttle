import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { PendingOperations, freezeDraft, buildAction, draftConflict, validateOperation, validateIdentity } from '../../src/panel_http/ui/core.mjs';
import { richState, operation, identity } from './fixtures.mjs';
import { PanelApp, REJECTION_TEXT } from '../../src/panel_http/ui/app.mjs';
import { route, ClientError } from '../../src/panel_http/ui/client.mjs';
import { ENUMS } from '../../src/panel_http/ui/contract.mjs';
const opId = operation().operation_id;
test('u6 both provider forms select the supported Custom channel type', () => {
  const app = Object.create(PanelApp.prototype); let fields;
  Object.assign(app, {lanes: {invalidate() {}}, paint() {}, renderer: {form(_title, value) { fields = value; }}});
  for (const kind of ['volcano', 'glm', 'volcano']) {
    app.openSource(kind);
    const channel = fields.find(field => field.name === 'channel_type');
    assert.equal(channel.value, '8');
    assert.equal(channel.type, 'select');
    assert.deepEqual(channel.options, [['8', 'Custom (8)']]);
  }
});
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
// 提交被拒就地反馈（SGM-8 第 1 步）：假 renderer + 假 work，回调方式与真实 work 相同（success 抛错转 failure）。
function submitApp(outcome) {
  const s = storage(), p = new PendingOperations(s, () => 0), app = Object.create(PanelApp.prototype), cleared = [], state = richState();
  Object.assign(app, {pending: p, lifecycle: {active: true}, paint() {}, notice: null, operationsOpen: false, window: {crypto: cryptoIds()},
    age: {snapshot: state}, identities: {data: null}, renderer: {clearForm() { cleared.push(true); }},
    work(descriptor, body, success, failure) { try { outcome(app, body, success, failure); } catch (error) { failure(error); } }});
  Object.defineProperty(app, 'readOnly', {value: false});
  const draft = freezeDraft('source', state.catalog.sources[0]), editor = {action: 'setSourcePaused', draft, sensitive: false};
  app.editor = editor;
  return {app, s, p, cleared, draft, editor, pause: () => app.submit('setSourcePaused', draft, {paused: true}), group: () => {
    const groupEditor = {action: 'createGroup'}; app.editor = groupEditor; app.submit('createGroup', null, {name: 'dup', newapi_group: 'managed', type: 'glm'}); return groupEditor;
  }};
}
const pausedReceipt = body => operation({operation_id: body.operation_id, kind: 'set_source_paused', target_type: 'source', target_id: '1'});
test('submit rejected keeps the confirm open with the reason and tracks nothing', () => {
  const t = submitApp((_app, _body, _success, failure) => failure(new ClientError('not_ready', 503)));
  t.pause();
  assert.equal(t.p.items.size, 0); assert.deepEqual(JSON.parse(t.s.getItem('qt.pending.v1')).operation_ids, []);
  assert.equal(t.app.editor, t.editor); assert.equal(t.editor.rejection, `提交未完成：${REJECTION_TEXT.not_ready}（not_ready）`);
  assert.equal(t.cleared.length, 0); assert.equal(t.app.operationsOpen, false); assert.equal(t.app.notice, null);
  const g = submitApp((_app, _body, _success, failure) => failure(new ClientError('duplicate_group', 409)));
  const groupEditor = g.group();
  assert.equal(g.app.editor, groupEditor); assert.match(groupEditor.rejection, /相同 NewAPI 分组标识.*（duplicate_group）$/); assert.equal(g.cleared.length, 0);
  assert.equal(t.app.rejectionText(new ClientError('duplicate_source', 409, '7')), '提交未完成：该凭据已对应来源 #7，没有覆盖配置（duplicate_source）');
  assert.equal(t.app.rejectionText(new ClientError('limit_exceeded', 422)), '提交未完成：后端拒绝了请求（limit_exceeded）');
  // 再次提交前清空上一次的原因。
  const again = submitApp((app, body, success) => success(pausedReceipt(body)));
  again.editor.rejection = 'old'; again.pause(); assert.equal(again.editor.rejection, null); assert.equal(again.app.editor, null); assert.equal(again.cleared.length, 1);
});
test('submit with unknown outcome still records pending check and closes the confirm', () => {
  for (const outcome of [
    (_app, _body, _success, failure) => failure(new ClientError('result_unconfirmed', 504)),
    (_app, _body, _success, failure) => failure(new ClientError('transport')),
    (_app, body, success) => success(operation({operation_id: body.operation_id, kind: 'retire_source'})),
  ]) {
    const t = submitApp(outcome);
    t.pause();
    const [item] = t.p.items.values();
    assert.equal(t.p.items.size, 1); assert.match(item.error, /^结果待核对。/);
    assert.equal(t.cleared.length, 1); assert.equal(t.app.editor, null); assert.equal(t.app.operationsOpen, true);
  }
  const g = submitApp((_app, _body, _success, failure) => failure(new ClientError('storage', 500)));
  g.group(); assert.equal(g.app.editor, null); assert.equal(g.cleared.length, 1); assert.match(g.app.notice, /回执丢失不证明未写入/);
});
test('submit outcome never touches a form opened during the request', () => {
  const other = {action: 'restore', readOnlyAction: true};
  const rejected = submitApp((app, _body, _success, failure) => { app.editor = other; failure(new ClientError('not_ready', 503)); });
  rejected.pause();
  assert.equal(rejected.app.editor, other); assert.equal(other.rejection, undefined); assert.equal(rejected.editor.rejection, null);
  assert.match(rejected.app.notice, /^提交未完成：/); assert.equal(rejected.cleared.length, 0);
  const succeeded = submitApp((app, body, success) => { app.editor = other; success(pausedReceipt(body)); });
  succeeded.pause();
  assert.equal(succeeded.app.editor, other); assert.equal(succeeded.cleared.length, 0); assert.equal(succeeded.p.items.size, 1);
});
test('operation poll 404 explains and keeps polling within the window', () => {
  let now = 0, reply = new ClientError('not_found', 404);
  const p = new PendingOperations(storage(), () => now), app = Object.create(PanelApp.prototype);
  Object.assign(app, {pending: p, work(_descriptor, _body, _success, failure) { failure(reply); }});
  p.restore(opId); app.pollOperation(opId);
  const item = p.items.get(opId);
  assert.match(item.error, /^后端暂未找到这条操作记录/); assert.equal(item.tracking, true);
  now = 5000; assert.equal(p.due()?.id, opId);
  reply = new ClientError('queue_full', 503); app.pollOperation(opId); assert.equal(item.error, '请求失败：queue_full');
  now = 60000; assert.equal(p.due(), null);
  p.continue(opId); assert.equal(p.due()?.id, opId);
});
test('rejection texts only name contract error codes', () => {
  const codes = new Set([...ENUMS.adminError, ...ENUMS.httpError]);
  for (const key of Object.keys(REJECTION_TEXT)) assert.ok(codes.has(key), key);
});
test('pending discard removes the entry even when the storage write fails', () => {
  let fail = false; const values = new Map();
  const s = {getItem: k => values.get(k) ?? null, setItem: (k, v) => { if (fail) throw new Error('quota'); values.set(k, v); }};
  const p = new PendingOperations(s, () => 0), item = begin(p);
  fail = true;
  // 与 dismiss 不同：后端本来就没有这条记录，存储写失败也必须移除内存条目，且不抛错。
  assert.doesNotThrow(() => p.discard(item.id));
  assert.equal(p.items.size, 0); assert.match(p.storageError, /写入失败/);
  p.discard(item.id); assert.equal(p.items.size, 0);
});
test('rejected sensitive source form stays only until the page is hidden', () => {
  const t = submitApp((_app, _body, _success, failure) => failure(new ClientError('duplicate_source', 409, '7')));
  const editor = {action: 'createSource', sensitive: true}; t.app.editor = editor;
  t.app.submit('createSource', null, {type: 'glm', name: 'Synthetic', note: '', credential: 'FAKE_NOT_A_KEY', channel_type: '8', base_url: 'https://example.test/chat/completions', models_url: '', discovery_auth: 'bearer', model_selection: 'manual', models: '["m"]', quota_url: 'https://example.test/quota?type=2', global_headers: '[]', key_headers: '[]'});
  assert.equal(t.app.editor, editor); assert.equal(editor.rejection, '提交未完成：该凭据已对应来源 #7，没有覆盖配置（duplicate_source）'); assert.equal(t.cleared.length, 0);
  let interactionCleared = 0;
  Object.assign(t.app, {lanes: {invalidate() {}}, age: {fail() {}}, secret: null, desired: null}); t.app.renderer.clearSecret = () => {}; t.app.renderer.clearInteraction = () => { interactionCleared++; };
  t.app.suspend();
  assert.equal(t.app.editor, null); assert.equal(t.cleared.length, 1);
  assert.equal(interactionCleared, 1);
});
