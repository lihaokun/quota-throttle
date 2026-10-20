import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { id, decimal, safeAdd, validateState, validateConfig, validateOperation, validateIdentities, validateNativePage, catalogModels, consumedFields, SnapshotAge, observations, Lanes, formatTime } from '../../src/panel_http/ui/core.mjs';
import { WIRE_FIELDS } from '../../src/panel_http/ui/contract.mjs';
import { emptyState, richState, identity, native, operation } from './fixtures.mjs';
const pbt = { numRuns: 128, verbose: true };

test('u2 field consumption declarations match original schema fields', () => {
  for (const [name, fields] of consumedFields) assert.deepEqual([...fields].sort(), [...WIRE_FIELDS[name]].sort(), name);
  assert.ok(consumedFields.size >= 31);
});
test('u2 canonical ids, distinct model domains and safe arithmetic', () => {
  for (const value of [0, 1, '', '0', '01', '+1', '1.0', '1e3', '9223372036854775808', '__proto__']) assert.throws(() => id(value));
  assert.equal(id('9223372036854775807'), '9223372036854775807');
  for (const value of ['', ' ', '00', '-0', '1.0', '1e2']) assert.throws(() => decimal(value));
  assert.throws(() => safeAdd(Number.MAX_SAFE_INTEGER, 1));
  assert.equal(formatTime(Number.MAX_SAFE_INTEGER), '9007199254740991 s');
  const models = ['a\nb', 'x'.repeat(1000), '__proto__'];
  assert.deepEqual(catalogModels(models), models);
  assert.throws(() => catalogModels(['a,b']));
});
test('u2 projection strips unknown fields and rejects missing or ill-typed observations', () => {
  const s = richState(); s.private_config = 'never project'; s.catalog.sources[0].source.secret = 'never project';
  const p = validateState(s);
  assert.equal(p.private_config, undefined); assert.equal(p.catalog.sources[0].source.secret, undefined);
  for (const mutate of [
    v => { delete v.backend_ready; }, v => { v.schema_version = 2; }, v => { v.catalog.sources[0].source.config_revision = 0; },
    v => { v.glm.sources[0].five_hour.percentage = -1; }, v => { v.glm.sources[0].state = 'observed-new'; },
    v => { v.catalog.sources.push(v.catalog.sources[0]); }, v => { v.catalog.memberships[0].membership.source_id = '999'; },
    v => { v.catalog.sources[0].source.type = 'volcano'; }, v => { v.glm.cycle_id = '18446744073709551616'; },
  ]) { const bad = richState(); mutate(bad); assert.throws(() => validateState(bad)); }
  assert.equal(validateOperation(operation()).phase, 'queued');
  assert.throws(() => validateOperation(operation({ kind: 'create_token' })));
  assert.throws(() => validateConfig({ schema_version: 1, dry_run: false, public_api_base_url: 'https://example.test/%76%31' }));
});
test('u4 refresh failure at age 4s expires old publication and same publication cannot revive', () => {
  const age = new SnapshotAge(); age.ingest(emptyState(), '0', 0, 0);
  assert.equal(age.fresh(4000), true); age.fail(); assert.equal(age.fresh(4000), false);
  age.ingest(emptyState(), '1', 4000, 4010); assert.equal(age.fresh(4010), false); assert.equal(age.age(4010), 4010);
  age.ingest(emptyState(105000), '0', 5000, 5001); assert.equal(age.fresh(5001), true);
  assert.throws(() => age.ingest(emptyState(110000), null, 5001, 5002)); assert.equal(age.fresh(5002), false);
});
test('u4 group assessment downgrades when relevant 29.5s sample ages past 30s', () => {
  const s = richState(); s.glm.sources[0].observed_at_ms = s.captured_at_ms - 29500;
  const age = new SnapshotAge(); age.ingest(validateState(s), '0', 0, 0);
  assert.equal(observations(age, 500).groupViews.get('2').current, true);
  const old = observations(age, 1000).groupViews.get('2');
  assert.equal(old.current, false); assert.equal(old.imminent_members, null); assert.equal(old.pin_release, null);
  for (const irrelevant of [v => { v.catalog.sources[0].source.manual_paused = true; }, v => { v.catalog.memberships[0].membership.channel_id = null; v.catalog.memberships[0].membership.lifecycle = 'planned'; }]) {
    const v = richState(); irrelevant(v); v.glm.sources = [];
    const a = new SnapshotAge(); a.ingest(validateState(v), '0', 0, 0);
    assert.equal(observations(a, 1000).groupViews.get('2').current, true);
  }
  const v = richState(); v.glm.sources[0] = { ...v.glm.sources[0], state: 'unavailable', five_hour: null, weekly: null, level: null };
  const a = new SnapshotAge(); a.ingest(validateState(v), '0', 0, 0);
  assert.equal(observations(a, 1000).groupViews.get('2').current, true);
});
test('u4 reset passage only expires observations, never resets usage or synthesizes group policy', () => {
  const s = richState(); s.glm.sources[0].weekly.reset_at_ms = s.captured_at_ms + 1000;
  const age = new SnapshotAge(); age.ingest(validateState(s), '0', 0, 0);
  assert.equal(observations(age, 999).groupViews.get('2').current, true);
  const p = observations(age, 1000); assert.equal(p.sourceViews.get('1').weekly.current, false); assert.equal(p.groupViews.get('2').current, false);
  assert.equal(s.glm.sources[0].weekly.percentage, 20);
});
test('u4 a causally new publication after clock rollback is accepted and server stale samples stay stale', () => {
  const age = new SnapshotAge(); age.ingest(emptyState(200000), '0', 0, 0); age.fail();
  age.ingest(emptyState(100000), '0', 5000, 5001); assert.equal(age.fresh(5001), true); assert.equal(age.snapshot.captured_at_ms, 100000);
  const s = richState(); s.glm.sources[0] = { ...s.glm.sources[0], state: 'stale', five_hour: null, weekly: null, level: null };
  age.ingest(validateState(s), '0', 6000, 6001);
  assert.equal(observations(age, 6001).sourceViews.get('1').current, false);
  assert.equal(observations(age, 6001).groupViews.get('2').current, false);
});
test('u4 never-observed and absent source samples remain unknown, not invented stale observations', () => {
  for (const absent of [false, true]) {
    const s = richState(); s.glm.sources = absent ? [] : [{ ...s.glm.sources[0], state: 'unknown', observed_at_ms: null, five_hour: null, weekly: null, level: null }];
    const age = new SnapshotAge(); age.ingest(validateState(s), '0', 0, 0);
    assert.equal(observations(age, 0).sourceViews.get('1').state, 'unknown');
    assert.equal(observations(age, 0).sourceViews.get('1').current, false);
  }
});
test('u3 invalidation retains owner until settle; old finally never releases new owner; hidden writes keep receipt', () => {
  let now = 0; const lanes = new Lanes(() => now);
  const a = lanes.acquire('work', 'usage'); lanes.invalidate('work');
  assert.equal(lanes.valid(a), false); assert.equal(lanes.acquire('work', 'new'), null);
  lanes.finish(a); const b = lanes.acquire('work', 'latest'); lanes.finish(a); assert.equal(lanes.owns(b), true);
  lanes.finish(b); const w = lanes.acquire('work', 'write', true); lanes.invalidate('work');
  assert.equal(w.controller.signal.aborted, false); assert.equal(lanes.valid(w), false); assert.equal(lanes.receiptValid(w), true);
  now = 40000; assert.equal(lanes.receiptValid(w), false); assert.equal(lanes.acquire('work', 'new'), null);
  lanes.finish(w); assert.notEqual(lanes.acquire('work', 'new'), null);
});
test('u7 native and local pages retain exact owner, cursor and large IDs', () => {
  const i = identity('9007199254740993');
  assert.equal(validateIdentities({ schema_version: 1, items: [i], after: '9007199254740992', next_after: null }, '9007199254740992').items[0].token_id, i.token_id);
  assert.throws(() => validateIdentities({ schema_version: 1, items: [i], after: null, next_after: null }, '3'));
  const page = { schema_version: 1, owner_scope: 'issuer', page: 1, page_size: 100, total: 1, items: [native()] };
  assert.equal(validateNativePage(page, 'issuer', 1).items[0].remaining_quota, -10);
  assert.throws(() => validateNativePage(page, 'legacy_admin', 1));
});
test('pbt1 age bound is header + full RTT + elapsed and only degrades', () => {
  fc.assert(fc.property(fc.integer({ min: 0, max: 20000 }), fc.integer({ min: 0, max: 2000 }), fc.array(fc.integer({ min: 0, max: 1000 }), { maxLength: 50 }), (header, rtt, steps) => {
    const age = new SnapshotAge(); age.ingest(emptyState(), String(header), 100, 100 + rtt);
    let elapsed = 0, stale = header + rtt >= 10000;
    for (const step of steps) { elapsed += step; stale ||= header + rtt + elapsed >= 10000; assert.equal(age.age(100 + rtt + elapsed), header + rtt + elapsed); assert.equal(age.fresh(100 + rtt + elapsed), !stale); }
  }), pbt);
});
test('pbt2 lane generation traces conserve actual ownership and reject stale receipts', () => {
  fc.assert(fc.property(fc.array(fc.integer({ min: 0, max: 4 }), { minLength: 1, maxLength: 200 }), actions => {
    const lanes = new Lanes(() => 0), tickets = []; let owner = null, generation = 0;
    for (const action of actions) {
      if (action <= 1) { const t = lanes.acquire('work', 'read', action === 1); assert.equal(t !== null, owner === null); if (t) { owner = t; tickets.push({ t, generation }); } }
      if (action === 2) { lanes.invalidate('work'); generation++; }
      if (action === 3 && tickets.length) { const old = tickets[0].t; lanes.finish(old); if (owner === old) owner = null; }
      if (action === 4 && owner) { lanes.finish(owner); owner = null; }
      assert.equal(lanes.owners.work, owner);
      for (const {t, generation: g} of tickets) assert.equal(lanes.valid(t), owner === t && g === generation && !t.controller.signal.aborted);
    }
  }), pbt);
});
test('pbt4 catalog identity and typed revisions do not coerce strings or overflow IDs', () => {
  fc.assert(fc.property(fc.bigInt({ min: 1n, max: 9223372036854775807n }), fc.integer({ min: 1, max: 100000 }), (sourceId, rev) => {
    const s = richState(), text = sourceId.toString(); s.catalog.sources[0].source.source_id = text;
    s.catalog.memberships[0].membership.source_id = text; s.glm.sources[0].source_id = text;
    s.catalog.sources[0].source.config_revision = rev; s.glm.sources[0].source_revision = rev;
    const v = validateState(s); assert.equal(v.catalog.sources[0].source.source_id, text); assert.equal(v.catalog.sources[0].source.config_revision, rev);
    s.catalog.sources[0].source.config_revision = String(rev); assert.throws(() => validateState(s));
  }), pbt);
});
