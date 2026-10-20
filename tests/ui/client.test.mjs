import test from 'node:test';
import assert from 'node:assert/strict';
import { route, requestJson, serializeBody, ClientError } from '../../src/panel_http/ui/client.mjs';
import { Lanes, ProtocolError } from '../../src/panel_http/ui/core.mjs';
import { emptyState, operation } from './fixtures.mjs';
function setup() {
  let time = 0; const lanes = new Lanes(() => time);
  const ticket = lanes.acquire('state', 'state');
  const options = { now: () => time, valid: t => lanes.valid(t), setTimer: () => 1, clearTimer: () => {} };
  return { ticket, lanes, options, time: t => { time = t; } };
}
function json(value, status = 200, headers = {}) { return new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json', ...headers } }); }
test('u2 route builder permits only fixed same-origin routes and canonical path IDs', () => {
  assert.equal(route('identity', { id: '9007199254740993' }).path, '/api/admin/v1/tokens/9007199254740993');
  assert.equal(route('nativePage', { owner_scope: 'issuer', page: 1 }).path, '/api/admin/v1/native-tokens?owner_scope=issuer&page=1');
  for (const bad of ['/../', 'https://evil.invalid', '?x=y', '0']) assert.throws(() => route('identity', { id: bad }));
  assert.throws(() => route('https://evil.invalid'));
  assert.throws(() => route('nativePage', { owner_scope: 'anything', page: 1 }));
  assert.throws(() => serializeBody({ secret: 'x'.repeat(1024 * 1024) }));
});
test('u2 every route status matches the original HTTP response matrix including retry 200', () => {
  const expected = {
    config: 200, state: 200, createSource: 201, sourceModels: 200, setModels: 202, discoverModels: 202,
    setSourcePaused: 202, retireSource: 202, clearError: 202, setVolcanoPolicy: 202, createGroup: 201,
    checkGroup: 200, groupReferences: 200, setGroupPin: 202, planMember: 201, ensureMember: 202,
    retireMember: 202, operation: 200, retry: 200, identities: 200, identity: 200, createToken: 202,
    setTokenEnabled: 202, deleteToken: 202, registerToken: 200, reveal: 200, nativePage: 200, usage: 200,
  };
  for (const [action, status] of Object.entries(expected)) assert.equal(route(action, {id: ['operation', 'retry'].includes(action) ? operation().operation_id : '1', owner_scope: 'issuer', page: 1}).status, status, action);
});
test('u2 fetch policy is fixed and response fields are projected without raw private fields', async () => {
  const f = setup(); let observed;
  const result = await requestJson(route('state'), null, f.ticket, { ...f.options, fetchImpl: async (path, init) => { observed = {path, init}; return json({ ...emptyState(), extra_private: 'DO_NOT_PROJECT' }, 200, { 'X-QT-Snapshot-Age-Ms': '4' }); } });
  assert.equal(observed.path, '/api/admin/v1/state');
  assert.equal(observed.init.headers['X-QT-Panel'], '1'); assert.equal(observed.init.credentials, 'omit');
  assert.equal(observed.init.cache, 'no-store'); assert.equal(observed.init.redirect, 'error');
  assert.equal(result.value.extra_private, undefined); assert.equal(result.age, '4');
});
test('u2 fixed error families never echo arbitrary backend bodies', async () => {
  for (const response of [json({ schema_version: 1, error: 'DO_NOT_ECHO_PRIVATE' }, 500), new Response('DO_NOT_ECHO_PRIVATE', { status: 500 }), json({ private_key: 'DO_NOT_ECHO_PRIVATE' }), json({ schema_version: 1, error: 'bad_request', private_key: 'DO_NOT_ECHO_PRIVATE' }, 400)]) {
    const f = setup();
    await assert.rejects(requestJson(route('state'), null, f.ticket, { ...f.options, fetchImpl: async () => response }), e => { assert.equal(e.message.includes('DO_NOT_ECHO_PRIVATE'), false); return e instanceof ProtocolError || e instanceof ClientError; });
  }
});
test('u2 malformed UTF8 and response byte overflow reject whole response and cancel reader', async () => {
  const badUtf8 = new Response(new Uint8Array([0xff]), { headers: { 'Content-Type': 'application/json' } });
  const a = setup(); await assert.rejects(requestJson(route('state'), null, a.ticket, { ...a.options, fetchImpl: async () => badUtf8 }), ProtocolError);
  let chunks = 0, cancelled = false;
  const stream = new ReadableStream({ pull(c) { chunks++; c.enqueue(new Uint8Array(1024 * 1024)); }, cancel() { cancelled = true; } });
  const b = setup(); await assert.rejects(requestJson(route('state'), null, b.ticket, { ...b.options, fetchImpl: async () => new Response(stream, { headers: { 'Content-Type': 'application/json' } }) }), ProtocolError);
  assert.ok(chunks <= 35); assert.equal(cancelled, true);
});
test('u2 deadline and generation checked after fetch and body completion', async () => {
  const a = setup();
  await assert.rejects(requestJson(route('state'), null, a.ticket, { ...a.options, fetchImpl: async () => { a.time(12000); return json(emptyState()); } }), e => e.code === 'deadline');
  const b = setup(); let pulls = 0;
  const stream = new ReadableStream({ pull(c) { if (pulls++ === 0) c.enqueue(new TextEncoder().encode(JSON.stringify(emptyState()))); else { b.time(12000); c.close(); } } });
  await assert.rejects(requestJson(route('state'), null, b.ticket, { ...b.options, fetchImpl: async () => new Response(stream, { headers: { 'Content-Type': 'application/json' } }) }), e => e.code === 'deadline');
  const c = setup();
  await assert.rejects(requestJson(route('state'), null, c.ticket, { ...c.options, fetchImpl: async () => { c.lanes.invalidate('state'); return json(emptyState()); } }), e => e.code === 'cancelled');
});
test('u2 deadline after serialization forbids sending and rejected headers cancel the unused body', async () => {
  const f = setup(); f.lanes.finish(f.ticket); const ticket = f.lanes.acquire('work', 'pause', true); let calls = 0;
  const body = { toJSON() { f.time(40000); return { schema_version: 1 }; } };
  await assert.rejects(requestJson(route('setSourcePaused', {id: '1'}), body, ticket, { ...f.options, fetchImpl: async () => { calls++; return json(operation(), 202); } }), e => e.code === 'deadline');
  assert.equal(calls, 0);
  for (const expired of [false, true]) {
    const g = setup(); let cancelled = false;
    const stream = new ReadableStream({ cancel() { cancelled = true; } });
    await assert.rejects(requestJson(route('state'), null, g.ticket, { ...g.options, fetchImpl: async () => { if (expired) g.time(12000); return new Response(stream, { headers: { 'Content-Type': expired ? 'application/json' : 'text/plain' } }); } }));
    assert.equal(cancelled, true); assert.equal(g.ticket.controller.signal.aborted, true);
  }
});
test('u2 abort does not orphan fetch or let replacement own lane before settlement', async () => {
  const f = setup(); let settle;
  const promise = requestJson(route('state'), null, f.ticket, { ...f.options, fetchImpl: () => new Promise(resolve => { settle = resolve; }) }).finally(() => f.lanes.finish(f.ticket));
  f.lanes.invalidate('state'); assert.equal(f.lanes.acquire('state', 'new'), null);
  settle(json(emptyState())); await assert.rejects(promise, e => e.code === 'cancelled');
  assert.notEqual(f.lanes.acquire('state', 'new'), null);
});
test('u2 JSON write method/headers and accepted status retain original operation phase', async () => {
  const f = setup(); f.lanes.finish(f.ticket); const ticket = f.lanes.acquire('work', 'pause', true);
  const body = { schema_version: 1, operation_id: operation().operation_id, expected_revision: 1, paused: true };
  const response = await requestJson(route('setSourcePaused', {id: '1'}), body, ticket, { ...f.options, valid: t => f.lanes.receiptValid(t), fetchImpl: async (_, init) => {
    assert.equal(init.method, 'PUT'); assert.equal(init.headers['Content-Type'], 'application/json'); assert.deepEqual(JSON.parse(init.body), body);
    return json(operation({ phase: 'applying' }), 202);
  } });
  assert.equal(response.value.phase, 'applying');
});
