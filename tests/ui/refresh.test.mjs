import test from 'node:test';
import assert from 'node:assert/strict';
import { PanelApp } from '../../src/panel_http/ui/app.mjs';
import { Lanes, SnapshotAge, validateState } from '../../src/panel_http/ui/core.mjs';
import { route } from '../../src/panel_http/ui/client.mjs';
import { volcanoState, usageSnapshot } from './fixtures.mjs';
test('u3 queue-full cooldown retains only latest desired read for at least five seconds', async () => {
  let now = 0, sent = [];
  const app = Object.create(PanelApp.prototype);
  Object.assign(app, {now: () => now, lanes: new Lanes(() => now), paint() {}, lifecycle: {active: true}, stateDue: Infinity,
    config: {dry_run: false}, pending: {due: () => null}, usageDue: Infinity, readNotBefore: 0, desired: null,
    fetchImpl: async path => { sent.push(path); return new Response(JSON.stringify({schema_version: 1, error: 'queue_full', existing_source_id: null}), {status: 503, headers: {'Content-Type': 'application/json'}}); },
  });
  await app.work(route('identities'), null, () => {}, () => {});
  assert.equal(app.readNotBefore, 5000);
  app.desire(() => sent.push('old')); app.desire(() => sent.push('latest'));
  now = 4999; app.tick(); assert.equal(sent.length, 1);
  now = 5000; app.tick(); assert.deepEqual(sent, ['/api/admin/v1/tokens', 'latest']); app.tick(); assert.equal(sent.length, 2);
});
test('u8 same volcano period ID crossing display-age boundary blocks start and late acceptance', () => {
  let now = 0, calls = 0, complete;
  const app = Object.create(PanelApp.prototype), age = new SnapshotAge(); age.ingest(validateState(volcanoState()), '0', 0, 0);
  Object.assign(app, {now: () => now, age, lanes: new Lanes(() => now), page: 'sources', sourceUsageId: '1', usageFilters: {offset: 480}, usageGeneration: 0,
    usage: null, paint() {}, work(_route, query, success) { calls++; complete = () => success(usageSnapshot(query)); }});
  app.queryUsage(); assert.equal(calls, 1); now = 2000; complete(); assert.equal(app.usage, null); assert.match(app.usageMessage, /重置边界/);
  app.queryUsage(); assert.equal(calls, 1); assert.match(app.usageMessage, /等待服务端/);
  assert.equal(age.snapshot.catalog.sources[0].volcano.runtime.period_id, 1); assert.equal(age.snapshot.catalog.sources[0].volcano.runtime.used_input, 0);
});
test('u3 explicit usage shares the latest desired slot and takes precedence over operation polling', () => {
  const events = [], app = Object.create(PanelApp.prototype); let fresh = true;
  Object.assign(app, {now: () => 0, lanes: new Lanes(() => 0), page: 'data', usageGeneration: 0, paint() {}, age: {fresh: () => fresh}, config: {dry_run: false}, lifecycle: {active: true}, stateDue: Infinity, readNotBefore: 0,
    pending: {due() {events.push('operation'); return null;}}, queryUsage() {events.push('usage');}});
  app.changeUsage(); app.tick(); assert.deepEqual(events, ['usage']);
  events.length = 0; fresh = false; app.changeUsage(); app.usageDue = Infinity; app.tick(); assert.deepEqual(events, ['operation']); assert.notEqual(app.desired, null);
  fresh = true; app.tick(); assert.deepEqual(events, ['operation', 'usage']); assert.equal(app.desired, null);
  app.changeUsage(); app.desire(() => events.push('latest-read')); app.tick(); assert.equal(events.at(-1), 'latest-read');
});
