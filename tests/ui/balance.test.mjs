import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { ActionError, ProtocolError, validateState, validateReply, quotaFromDollars, formatQuota, balanceObservations, observations, balanceMaximum, balanceDollarsFloor, balanceRefillPrefill, freezeDraft, buildAction, draftConflict, SnapshotAge, Lanes, PendingOperations } from '../../src/panel_http/ui/core.mjs';
import { PanelApp, bootstrap } from '../../src/panel_http/ui/app.mjs';
import { route, requestJson, ClientError } from '../../src/panel_http/ui/client.mjs';
import { LIMITS } from '../../src/panel_http/ui/contract.mjs';
import { createRenderer } from '../../src/panel_http/ui/render.mjs';
import { balanceState } from './fixtures.mjs';
const usd = (quota_per_unit = 500000) => ({quota_per_unit, display_type: 'USD'});
const context = (state, age = 0, fresh = true) => ({unit: structuredClone(state.balance.unit), owner_scope: 'issuer', views: balanceObservations(state, {fresh, age})});
const refill = (amount = 1005000) => ({schema_version: 1, owner_scope: 'issuer', user_id: '9007199254740993', before: -1, amount, after: null});
const refillRequest = amount => ({schema_version: 1, owner_scope: 'issuer', amount});
const policyFields = {auto_refill: false, warn_below: '2.01', refill_to: '4.02'};
function balanceRenderer() {
  class Element {
    constructor(tag = 'div') { this.tagName = tag.toUpperCase(); this.dataset = {}; this.children = []; this.ownText = ''; this.classList = {toggle() {}, add() {}, remove() {}}; }
    set textContent(value) { this.ownText = String(value); this.children = []; }
    get textContent() { return this.ownText + this.children.map(child => child.textContent ?? String(child)).join(' '); }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this.ownText = ''; this.children = [...children]; }
    addEventListener() {} removeEventListener() {} setAttribute() {} removeAttribute() {}
    querySelector() { return null; } querySelectorAll() { return []; }
    contains(child) { return this === child || this.children.some(node => node.contains?.(child)); }
  }
  const roots = new Map(), document = {
    activeElement: null, defaultView: new Element(), createElement: tag => new Element(tag),
    getElementById(id) { if (!roots.has(id)) roots.set(id, new Element()); return roots.get(id); },
    addEventListener() {}, removeEventListener() {}, querySelector() { return new Element('button'); }, querySelectorAll() { return []; },
  };
  const renderer = createRenderer(document, {});
  return {renderer, text: () => document.getElementById('balance-strip').textContent, paint(snapshot, age = 0) {
    renderer.frame({snapshot: validateState(snapshot), page: 'data', readOnly: false, operations: [], operationsOpen: false,
      config: {dry_run: false, public_api_base_url: null}, observation: {fresh: true, age, sourceViews: new Map(), balanceViews: balanceObservations(snapshot, {fresh: true, age})}});
  }};
}

test('balance state and replies validate', () => {
  const state = balanceState();
  assert.deepEqual(validateState(state).balance, state.balance);
  for (const mutate of [
    s => { delete s.balance; }, s => { delete s.balance.accounts[0].last_refill; }, s => { s.balance.unit.quota_per_unit = 0; },
    s => { s.balance.unit.quota_per_unit = Infinity; }, s => { s.balance.unit.display_type = '美元'; }, s => { s.balance.policy.warn_below = s.balance.policy.refill_to; },
    s => { s.balance.accounts[0].user_id = 2; }, s => { s.balance.accounts[0].quota = null; }, s => { s.balance.accounts[0].observed_at_ms = null; },
    s => { s.balance.accounts[0].state = 'low'; }, s => { s.balance.accounts[0].unknown_reason = 'auth_failed'; },
    s => { s.balance.accounts[0].observed_at_ms = s.captured_at_ms - 180001; }, s => { s.balance.accounts[0].observed_at_ms++; },
    s => { s.balance.accounts[0].locked_until_ms = s.captured_at_ms; }, s => { s.balance.accounts[0].backoff_until_ms = s.captured_at_ms - 1; },
    s => { s.balance.accounts.push({...s.balance.accounts[0], user_id: '3'}); },
    s => { s.balance.accounts.push({...s.balance.accounts[0], owner_scope: 'legacy_admin'}); },
    s => { s.balance.accounts.unshift({...s.balance.accounts[0], owner_scope: 'legacy_admin', user_id: '3'}); },
    s => { s.balance.accounts[0].secret = 'not a wire field'; },
  ]) { const bad = structuredClone(state); mutate(bad); assert.throws(() => validateState(bad), ProtocolError); }
  for (const [outcome, amount, error_code] of [['refilled', 1, null], ['not_sent', null, 'not_ready'], ['not_sent', 1, 'queue_full'], ['rejected', 1, 'remote_rejected'], ['uncertain', 1, 'transport_unknown']]) {
    const sample = structuredClone(state); sample.balance.accounts[0].last_refill = {at_ms: 0, source: 'auto', outcome, amount, error_code};
    assert.doesNotThrow(() => validateState(sample));
    sample.balance.accounts[0].last_refill.error_code = outcome === 'refilled' ? 'remote_rejected' : null;
    assert.throws(() => validateState(sample), ProtocolError);
  }
  for (const [outcome, amount, error_code] of [['refilled', null, null], ['rejected', null, 'remote_rejected'], ['uncertain', 1, 'not_ready'], ['uncertain', null, 'transport_unknown']]) {
    const bad = structuredClone(state); bad.balance.accounts[0].last_refill = {at_ms: 0, source: 'manual', outcome, amount, error_code}; assert.throws(() => validateState(bad), ProtocolError);
  }
  const request = refillRequest(1005000), response = refill();
  assert.deepEqual(validateReply('balanceRefilled', response, request), response);
  for (const bad of [{...response, owner_scope: 'legacy_admin'}, {...response, amount: 2}, {...response, user_id: 2}, {...response, user_id: '0'}, {...response, after: undefined}]) assert.throws(() => validateReply('balanceRefilled', bad, request), ProtocolError);
  const body = {schema_version: 1, expected_revision: 9, auto_refill: false, warn_below: 1, refill_to: 2};
  const saved = {schema_version: 1, revision: 10, auto_refill: false, warn_below: 1, refill_to: 2};
  for (const revision of [9, 10]) assert.equal(validateReply('balancePolicySaved', {...saved, revision}, body).revision, revision);
  for (const bad of [{...saved, revision: 11}, {...saved, auto_refill: true}, {...saved, warn_below: 2}, {...saved, refill_to: 3}]) assert.throws(() => validateReply('balancePolicySaved', bad, body), ProtocolError);
  assert.throws(() => validateReply('balancePolicySaved', {...saved, revision: Number.MAX_SAFE_INTEGER + 1}, {...body, expected_revision: Number.MAX_SAFE_INTEGER}), ProtocolError);
  assert.doesNotThrow(() => validateReply('balancePolicySaved', {...saved, revision: Number.MAX_SAFE_INTEGER}, {...body, expected_revision: Number.MAX_SAFE_INTEGER}));
  const old = balanceState(); old.balance.accounts[0].observed_at_ms -= 179999;
  const age = new SnapshotAge(); age.ingest(validateState(old), '0', 0, 0);
  for (const [elapsed, fresh] of [[0, true], [1, true], [2, false]]) {
    const projected = observations(age, elapsed); assert.equal(projected.fresh, true);
    const a = projected.balanceViews.get('issuer'); assert.equal(a.fresh, fresh); assert.equal(a.effective_state, fresh ? 'sufficient' : 'unknown');
    assert.equal(balanceMaximum(a) === null, !fresh); assert.equal(balanceRefillPrefill(old.balance.policy, a, old.balance.unit) === '', !fresh);
  }
  fc.assert(fc.property(fc.integer({min: 0, max: 190000}), fc.array(fc.integer({min: 0, max: 3000}), {maxLength: 30}), fc.boolean(), (baseAge, increments, failed) => {
    const s = balanceState(); s.balance.accounts[0].observed_at_ms -= baseAge;
    if (failed || baseAge > 180000) s.balance.accounts[0].state = 'unknown';
    if (failed) s.balance.accounts[0].unknown_reason = 'auth_failed';
    validateState(s); let elapsed = 0, unknown = s.balance.accounts[0].state === 'unknown';
    for (const step of increments) { elapsed += step; unknown ||= baseAge + elapsed > 180000; assert.equal(context(s, elapsed).views.get('issuer').effective_state === 'unknown', unknown); }
  }), {numRuns: 128});
  for (const reason of ['auth_failed', null]) { const s = balanceState(); s.balance.accounts[0].state = 'unknown'; s.balance.accounts[0].unknown_reason = reason; assert.equal(context(validateState(s)).views.get('issuer').fresh, false); }
  assert.equal(context(state, 0, false).views.get('issuer').reason, 'snapshot');
  const missing = balanceState(); Object.assign(missing.balance.accounts[0], {state: 'unknown', quota: null, observed_at_ms: null}); assert.equal(context(validateState(missing)).views.get('issuer').reason, 'missing');
  const future = balanceState(); Object.assign(future.balance.accounts[0], {state: 'unknown', observed_at_ms: future.captured_at_ms + 1}); assert.equal(context(validateState(future)).views.get('issuer').reason, 'clock');
  // 真实renderer须随新capture更新静态剩余；同capture的浏览器tick不做倒计时。
  const rendering = balanceRenderer(), first = balanceState(1000000);
  Object.assign(first.balance.accounts[0], {locked_until_ms: 1600000, backoff_until_ms: 1700000});
  rendering.paint(first); assert.match(rendering.text(), /封锁：采样时约剩 600 秒/); assert.match(rendering.text(), /自动退避：采样时约剩 700 秒/);
  const next = structuredClone(first); next.captured_at_ms += 5000;
  assert.equal(JSON.stringify(next.balance), JSON.stringify(first.balance));
  rendering.paint(next); assert.match(rendering.text(), /封锁：采样时约剩 595 秒/); assert.match(rendering.text(), /自动退避：采样时约剩 695 秒/);
  rendering.paint(next, 5000); assert.match(rendering.text(), /封锁：采样时约剩 595 秒/); assert.match(rendering.text(), /自动退避：采样时约剩 695 秒/);
  rendering.renderer.destroy();
});

test('quotaFromDollars rounds and bounds', () => {
  for (const [text, ratio, expected] of [['0.29', 50, 15], ['0.57', 50, 29], ['2.01', 500000, 1005000], ['8000000000.00', 500000, LIMITS.balanceCap], ['20000000000000.00', 50, 1e15], ['0.01', 1e5, 1000], ['10000000.00', 1e-7, 1]]) assert.equal(quotaFromDollars(text, usd(ratio)), expected);
  for (const [text, unit] of [['8000000000.01', usd()], ['0.01', usd(7.3)], ['1', usd(1e308)], ['1', usd(5e-324)], ['1', null], ['1', {...usd(), display_type: 'CNY'}], ['0', usd()], ['1e2', usd()], ['01', usd()], ['-1', usd()], [' 1', usd()], ['1.001', usd()]]) assert.throws(() => quotaFromDollars(text, unit), ActionError);
  const minimumCap = `8${'0'.repeat(338)}.00`;
  assert.equal(quotaFromDollars(minimumCap, usd(5e-324)), LIMITS.balanceCap);
  assert.throws(() => quotaFromDollars(`1${'0'.repeat(349)}.00`, usd(5e-324)), /上限/);
  assert.throws(() => quotaFromDollars(`1${'0'.repeat(350)}`, usd(5e-324)), /350/);
  fc.assert(fc.property(fc.integer({min: 1, max: 100000000}), fc.integer({min: 1, max: 1000000}), (cents, tenths) => {
    const n = BigInt(cents) * BigInt(tenths), d = 1000n, quotient = n / d, remainder = n % d;
    const expected = quotient + (remainder >= d - remainder ? 1n : 0n), input = `${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, '0')}`;
    if (expected === 0n) assert.throws(() => quotaFromDollars(input, usd(tenths / 10)), ActionError);
    else assert.equal(quotaFromDollars(input, usd(tenths / 10)), Number(expected));
  }), {numRuns: 128});
});

test('formatQuota preserves negatives units and grouping', () => {
  assert.equal(formatQuota(-10955000, usd()), '-$21.91'); assert.equal(formatQuota(617283945, usd()), '$1,234.57');
  assert.equal(formatQuota(null, usd()), '未知'); assert.equal(formatQuota(0, usd()), '$0.00');
  assert.equal(formatQuota(-12, null), '-12 原始额度'); assert.equal(formatQuota(10, {...usd(), display_type: 'CNY'}), '10 原始额度');
  assert.equal(formatQuota(1, usd(7.3)), '$0.14'); assert.equal(balanceDollarsFloor(1, usd(7.3)), '0.13');
  const tiny = formatQuota(LIMITS.balanceCap, usd(5e-324)); assert.match(tiny, /^\$8(?:00[0-9],|[0-9,])*\.00$/); assert.doesNotMatch(tiny, /Infinity|e\+/);
  assert.equal(balanceDollarsFloor(LIMITS.balanceCap, usd(5e-324)), `8${'0'.repeat(338)}.00`);
  assert.throws(() => quotaFromDollars(formatQuota(500000, usd()), usd()), ActionError);
});

test('balance actions build exact bodies', () => {
  const s = balanceState(), ctx = context(s), draft = freezeDraft('balance', s.balance.policy);
  assert.deepEqual(buildAction('refillBalance', null, {owner_scope: 'issuer', amount: '2.01'}, s, null, ctx), {body: refillRequest(1005000), targetId: null, kind: null});
  const saved = buildAction('setBalancePolicy', draft, policyFields, s, null, ctx);
  assert.deepEqual(saved, {body: {schema_version: 1, expected_revision: 1, auto_refill: false, warn_below: 1005000, refill_to: 2010000}, targetId: null, kind: null});
  assert.equal(Object.isFrozen(saved.body), true); assert.equal(saved.body.balanceContext, undefined);
  for (const unit of [null, {...usd(), display_type: 'CNY'}]) { s.balance.unit = unit; assert.deepEqual(buildAction('setBalancePolicy', draft, {auto_refill: false, warn_below: 'bad', refill_to: 'bad'}, s, null, context(s)).body, {schema_version: 1, expected_revision: 1, auto_refill: false, warn_below: 1e14, refill_to: 1e15}); }
  s.balance.unit = usd();
  assert.throws(() => buildAction('refillBalance', null, {owner_scope: 'issuer', amount: '1'}, s), ActionError);
  assert.throws(() => buildAction('refillBalance', null, {owner_scope: 'legacy_admin', amount: '1'}, s, null, ctx), ActionError);
  assert.throws(() => buildAction('setBalancePolicy', freezeDraft('source', s.balance.policy), policyFields, s, null, ctx), ActionError);
  assert.throws(() => buildAction('setBalancePolicy', draft, {...policyFields, warn_below: '9'}, s, null, ctx), ActionError);
  s.balance.unit.quota_per_unit = 50; assert.throws(() => buildAction('setBalancePolicy', draft, policyFields, s, null, ctx), ActionError);
  s.balance.unit = usd(); s.balance.accounts[0].quota = LIMITS.balanceCap;
  assert.throws(() => buildAction('refillBalance', null, {owner_scope: 'issuer', amount: '1'}, s, null, context(s)), ActionError);
  assert.equal(buildAction('refillBalance', null, {owner_scope: 'issuer', amount: '1'}, s, null, context(s, 180001)).body.amount, 500000);
});

test('balance policy draft conflicts on revision', () => {
  const s = balanceState(), draft = freezeDraft('balance', s.balance.policy); assert.equal(draftConflict(draft, s, 'setBalancePolicy'), false);
  s.balance.policy.revision++; assert.equal(draftConflict(draft, s, 'setBalancePolicy'), true);
  assert.throws(() => buildAction('setBalancePolicy', draft, policyFields, s, null, context(s)), ActionError);
  const bad = {...s.balance.policy}; delete bad.revision;
  assert.throws(() => buildAction('setBalancePolicy', freezeDraft('balance', bad), policyFields, s, null, context(s)), ActionError);
});

function submitApp(reply = body => refill(body.amount)) {
  let time = 0; const state = balanceState(), age = new SnapshotAge(); age.ingest(state, '0', 0, 0);
  const saved = new Map(), storage = {getItem: k => saved.get(k) ?? null, setItem: (k, v) => saved.set(k, v)};
  const app = Object.create(PanelApp.prototype), forms = [], sent = [], cleared = [], editors = [];
  Object.assign(app, {age, now: () => time, lanes: new Lanes(() => time), pending: new PendingOperations(storage, () => time), config: {dry_run: false}, lifecycle: {active: true}, identities: {data: null}, readNotBefore: 0, notice: null, operationsOpen: false,
    renderer: {form(title, fields, label, submit) { forms.push({title, fields, label, submit}); }, clearForm() { cleared.push(true); }}, paint() {},
    fetchImpl: async (path, init) => { const body = JSON.parse(init.body); sent.push({path, method: init.method, body, deadline: app.lanes.owners.work.deadline}); const result = await reply(body, app); return new Response(JSON.stringify(result.value ?? result), {status: result.status ?? 200, headers: {'Content-Type': 'application/json'}}); },
    work(...args) { this.workPromise = PanelApp.prototype.work.apply(this, args); return this.workPromise; },
  });
  return {app, state, forms, sent, cleared, editors, time(value) { time = value; }, async send(action, values) { if (action === 'refillBalance') app.openRefill('issuer'); else app.openBalancePolicy(); editors.push(app.editor); forms.at(-1).submit(values); await app.workPromise; }};
}
function openedPolicy(warn_below, refill_to) {
  const original = {revision: 7, auto_refill: true, warn_below, refill_to};
  const t = submitApp(body => ({schema_version: 1, revision: body.expected_revision + Number(['auto_refill', 'warn_below', 'refill_to'].some(key => body[key] !== original[key])), auto_refill: body.auto_refill, warn_below: body.warn_below, refill_to: body.refill_to}));
  t.state.balance.policy = {...original}; t.app.openBalancePolicy();
  const editor = t.app.editor, form = t.forms.at(-1), values = Object.fromEntries(form.fields.filter(field => field.name).map(field => [field.name, field.value]));
  return {...t, editor, values, async save(edits = {}) { form.submit({...values, ...edits}); await t.app.workPromise; }};
}

test('balance submit success binds body and shows result', async () => {
  const r = submitApp(body => refill(body.amount)); await r.send('refillBalance', {amount: '2.01'});
  assert.deepEqual(r.sent, [{path: '/api/admin/v1/balance/refill', method: 'POST', body: refillRequest(1005000), deadline: 70000}]);
  assert.equal(r.app.notice, '已补足 $2.01'); assert.equal(r.app.editor, null); assert.equal(r.app.pending.items.size, 0); assert.equal(r.cleared.length, 1);
  const p = submitApp(body => ({schema_version: 1, revision: body.expected_revision + 1, auto_refill: body.auto_refill, warn_below: body.warn_below, refill_to: body.refill_to}));
  await p.send('setBalancePolicy', policyFields); assert.equal(p.app.notice, '余额设置已保存'); assert.equal(p.app.editor, null); assert.equal(p.sent[0].deadline, 40000); assert.equal(p.app.pending.items.size, 0);
  for (const mutate of [value => { value.owner_scope = 'legacy_admin'; }, value => { value.amount++; }, value => { value.user_id = 9007199254740992; }]) {
    const wrong = submitApp(body => { const value = refill(body.amount); mutate(value); return value; }); await wrong.send('refillBalance', {amount: '2.01'}); assert.equal(wrong.app.notice, '结果待核对：请看该账号的上次补足结果及其时间'); assert.equal(wrong.sent.length, 1);
  }
  for (const key of ['auto_refill', 'warn_below', 'refill_to', 'revision']) {
    const wrong = submitApp(body => { const value = {schema_version: 1, revision: body.expected_revision, auto_refill: body.auto_refill, warn_below: body.warn_below, refill_to: body.refill_to}; value[key] = key === 'auto_refill' ? !value[key] : value[key] + 2; return value; });
    await wrong.send('setBalancePolicy', policyFields); assert.equal(wrong.app.notice, '结果待核对：请看当前余额设置及修订号'); assert.equal(wrong.app.pending.items.size, 0);
  }
  const slow = submitApp(body => { slow.time(50000); return refill(body.amount); }); await slow.send('refillBalance', {amount: '2.01'}); assert.equal(slow.app.notice, '已补足 $2.01');
  const late = submitApp(body => { late.time(70000); return refill(body.amount); }); await late.send('refillBalance', {amount: '2.01'}); assert.match(late.app.notice, /^结果待核对/);
  const original = refillRequest(1005000), lanes = new Lanes(() => 0), ticket = lanes.acquire('work', 'binding', true);
  const received = await requestJson(route('refillBalance'), original, ticket, {now: () => 0, valid: t => lanes.valid(t), setTimer: () => 1, clearTimer() {}, fetchImpl: async (_path, init) => { original.amount = 1; assert.equal(JSON.parse(init.body).amount, 1005000); return new Response(JSON.stringify(refill()), {headers: {'Content-Type': 'application/json'}}); }});
  assert.equal(received.value.amount, 1005000);
  for (const [warn, target, warnText, targetText] of [[100001, 200003, '0.20', '0.40'], [1, 2, '', '']]) {
    for (const auto_refill of [true, false]) {
      const t = openedPolicy(warn, target);
      assert.deepEqual(t.values, {auto_refill: true, warn_below: warnText, refill_to: targetText});
      await t.save({auto_refill});
      assert.equal(t.sent.length, 1, 'opening a valid policy and changing only its toggle must submit even below one cent');
      assert.deepEqual(t.sent[0].body, {schema_version: 1, expected_revision: 7, auto_refill, warn_below: warn, refill_to: target}, 'unchanged displayed amounts preserve their exact frozen raw values');
      assert.equal(t.app.notice, '余额设置已保存'); assert.equal(t.app.editor, null); assert.equal(t.app.pending.items.size, 0);
    }
  }
  for (const [edits, warn, target] of [[{warn_below: '0.30'}, 150000, 200003], [{refill_to: '0.60'}, 100001, 300000]]) {
    const t = openedPolicy(100001, 200003); await t.save(edits);
    assert.deepEqual(t.sent[0].body, {schema_version: 1, expected_revision: 7, auto_refill: true, warn_below: warn, refill_to: target}, 'editing one amount must not round the other amount');
  }
  const tinyEdited = openedPolicy(1, 2); await tinyEdited.save({refill_to: '0.01'});
  assert.deepEqual(tinyEdited.sent[0].body, {schema_version: 1, expected_revision: 7, auto_refill: true, warn_below: 1, refill_to: 5000});
  const cleared = openedPolicy(100001, 200003); await cleared.save({warn_below: ''});
  assert.equal(cleared.sent.length, 0); assert.equal(cleared.app.editor, cleared.editor); assert.match(cleared.editor.rejection, /金额须为/);
  const changedUnit = openedPolicy(1, 2); changedUnit.state.balance.unit.quota_per_unit = 50; await changedUnit.save({auto_refill: false});
  assert.equal(changedUnit.sent.length, 0); assert.match(changedUnit.editor.rejection, /换算已变化/);
  const changedRevision = openedPolicy(1, 2); changedRevision.state.balance.policy.revision++; await changedRevision.save({auto_refill: false});
  assert.equal(changedRevision.sent.length, 0); assert.match(changedRevision.editor.rejection, /已有新版本/);
});

test('balance submit refusals stay inline and unknown stays pending check', async () => {
  const refused = code => ({status: code === 'revision_conflict' ? 409 : code === 'invalid_input' ? 422 : 503, value: {schema_version: 1, error: code, existing_source_id: null}});
  const r = submitApp(() => refused('not_ready')); r.state.balance.accounts[0].locked_until_ms = r.state.captured_at_ms + 600000;
  await r.send('refillBalance', {amount: '1'}); assert.equal(r.app.editor, r.editors[0]); assert.match(r.app.editor.rejection, /本次补足未发送.*约需等待 600 秒/); assert.equal(r.cleared.length, 0); assert.equal(r.app.pending.items.size, 0);
  const max = submitApp(() => refused('invalid_input')); await max.send('refillBalance', {amount: '1'}); assert.match(max.app.editor.rejection, /按上次观察，最多还能补约.*以服务端新鲜读数为准/);
  for (const code of ['invalid_input', 'not_ready']) { const unknown = submitApp(() => refused(code)); unknown.state.balance.accounts[0].state = 'unknown'; await unknown.send('refillBalance', {amount: '1'}); assert.doesNotMatch(unknown.app.editor.rejection, /最多还能补约|约需等待/); if (code === 'not_ready') assert.match(unknown.app.editor.rejection, /等待信息未知/); }
  const conflict = submitApp(() => refused('revision_conflict')); await conflict.send('setBalancePolicy', policyFields); assert.match(conflict.app.editor.rejection, /revision_conflict/); assert.equal(conflict.app.pending.items.size, 0);
  const invalid = submitApp(); await invalid.send('refillBalance', {amount: '1.001'}); assert.match(invalid.app.editor.rejection, /最多两位小数/); assert.equal(invalid.sent.length, 0);
  for (const action of ['refillBalance', 'setBalancePolicy']) for (const outcome of ['504', 'transport', 'protocol']) {
    const unknown = submitApp(() => { if (outcome === 'transport') throw new Error('connection lost'); return outcome === '504' ? {status: 504, value: {schema_version: 1, error: 'result_unconfirmed', existing_source_id: null}} : {}; });
    await unknown.send(action, action === 'refillBalance' ? {amount: '1'} : policyFields);
    assert.equal(unknown.app.notice, action === 'refillBalance' ? '结果待核对：请看该账号的上次补足结果及其时间' : '结果待核对：请看当前余额设置及修订号'); assert.equal(unknown.app.editor, null); assert.equal(unknown.app.pending.items.size, 0); assert.equal(unknown.sent.length, 1);
  }
  const next = {action: 'unrelated'};
  const switched = submitApp((_body, app) => { app.editor = next; return refused('not_ready'); }); await switched.send('refillBalance', {amount: '1'}); assert.equal(switched.app.editor, next); assert.equal(next.rejection, undefined); assert.match(switched.app.notice, /本次补足未发送/);
  const localSwitched = submitApp(); localSwitched.app.openRefill('issuer');
  localSwitched.app.submit('refillBalance', null, {owner_scope: 'issuer', get amount() { localSwitched.app.editor = next; return 'invalid'; }});
  assert.equal(localSwitched.app.editor, next); assert.equal(next.rejection, undefined); assert.match(localSwitched.app.notice, /最多两位小数/); assert.equal(localSwitched.sent.length, 0);
  const stale = submitApp(() => refused('invalid_input')); stale.state.balance.accounts[0].observed_at_ms -= 179999; stale.app.openRefill('issuer'); stale.time(2); stale.forms.at(-1).submit({amount: '1'}); await stale.app.workPromise; assert.doesNotMatch(stale.app.editor.rejection, /最多还能补约/);
});

test('balance forms disable unavailable currency and bound prefills', () => {
  for (const unit of [null, {...usd(), display_type: 'CNY'}]) {
    const t = submitApp(); t.state.balance.unit = unit; t.app.openRefill('issuer'); assert.equal(t.forms.at(-1).fields.find(f => f.name === 'amount').disabled, true); assert.equal(t.app.editor.amountUnavailable, true);
    t.app.openBalancePolicy(); const fields = t.forms.at(-1).fields; assert.equal(fields.find(f => f.name === 'auto_refill').disabled, undefined); for (const key of ['warn_below', 'refill_to']) assert.equal(fields.find(f => f.name === key).disabled, true);
  }
  const t = submitApp(); t.state.balance.policy.refill_to = LIMITS.balanceCap; Object.assign(t.state.balance.accounts[0], {quota: -10, state: 'exhausted'}); t.app.openRefill('issuer'); assert.equal(t.forms.at(-1).fields.find(f => f.name === 'amount').value, '8000000000.00');
  t.state.balance.unit = usd(7.3); t.state.balance.policy.refill_to = 2; t.state.balance.policy.warn_below = 1; Object.assign(t.state.balance.accounts[0], {quota: 1, state: 'sufficient'}); t.app.openRefill('issuer'); assert.equal(t.forms.at(-1).fields.find(f => f.name === 'amount').value, '0.13');
  Object.assign(t.state.balance.accounts[0], {quota: null, observed_at_ms: null, state: 'unknown'}); t.app.openRefill('issuer'); assert.equal(t.forms.at(-1).fields.find(f => f.name === 'amount').value, '');
  assert.throws(() => t.app.openRefill('absent'), ActionError);
  const disabledButton = {}, message = {append() {}};
  const fakeWindow = {fetch() {}, AbortController() {}, structuredClone() {}, TextEncoder() {}, TextDecoder() {}, BigInt: undefined, document: {querySelectorAll: () => [disabledButton], getElementById: () => message, createElement: () => ({addEventListener() {}})}};
  assert.equal(bootstrap(fakeWindow), null); assert.equal(disabledButton.disabled, true); assert.match(message.textContent, /只读/);
});
