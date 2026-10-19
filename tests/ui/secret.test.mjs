import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { SecretSlot } from '../../src/panel_http/ui/app.mjs';
import { node } from '../../src/panel_http/ui/render.mjs';
const key = 'sk-SYNTHETIC0123456789abcdef';
function fixture(writeClipboard = async () => { throw new Error(); }) {
  let now = 0, visible = true, shown = null, copied = 0, timer = null;
  const secret = new SecretSlot({now: () => now, visible: () => visible, show: value => { shown = value; }, clearView: () => { shown = null; }, writeClipboard,
    copied: () => copied++, setTimer: fn => {timer = fn; return 1;}, clearTimer: () => {timer = null;} });
  return {secret, state: () => ({shown, copied, timer}), time: value => {now = value;}, visibility: value => {visible = value;}};
}
test('u9 clipboard rejection uses the same short-lived key and no network retry', async () => {
  let calls = 0; const f = fixture(async text => { assert.equal(text, key); if (++calls === 1) throw new Error(); });
  const binding = f.secret.begin('1', 2); assert.equal(f.secret.receive(binding, {token_id: '1', key}, 40000), true);
  await Promise.resolve(); assert.equal(f.state().shown, key); assert.equal(f.state().copied, 0);
  await f.secret.copy(); assert.equal(calls, 2); assert.equal(f.state().copied, 1); assert.equal(f.state().shown, null); assert.equal(f.secret.slot, null);
});
test('u9 hidden, expired, changed identity and late clipboard completion cannot restore secret', async () => {
  let reject; const f = fixture(() => new Promise((_, no) => {reject = no;}));
  const binding = f.secret.begin('1', 1); f.secret.receive(binding, {token_id: '1', key}, 40000);
  f.visibility(false); f.secret.clear(); reject(new Error()); await Promise.resolve();
  assert.equal(f.state().shown, null); assert.equal(f.secret.slot, null); assert.equal(f.state().copied, 0);
  f.visibility(true); const changed = f.secret.begin('2', 2);
  assert.equal(f.secret.receive(binding, {token_id: '1', key}, 40000), false);
  f.time(40000); assert.equal(f.secret.receive(changed, {token_id: '2', key}, 40000), false);
  const g = fixture(); const b = g.secret.begin('1', 1); g.secret.receive(b, {token_id: '1', key}, 40000); await Promise.resolve(); g.time(30000); g.secret.expire(); assert.equal(g.state().shown, null); assert.equal(g.secret.slot, null);
});
test('pbt8 malicious text and secret lifecycle never publish after clearing or expiry', async () => {
  await fc.assert(fc.asyncProperty(fc.array(fc.constantFrom('clear', 'hide', 'expire', 'same'), {minLength: 1, maxLength: 50}), fc.constantFrom('<img onerror=alert(1)>', '__proto__', '模型\u2028标签', '" onclick="alert(1)'), async (events, text) => {
    let reject; const f = fixture(() => new Promise((_, no) => { reject = no; })); const binding = f.secret.begin('1', 1);
    f.secret.receive(binding, {token_id: '1', key}, 40000); let invalid = false;
    for (const event of events) {
      if (event === 'clear') { f.secret.clear(); invalid = true; }
      if (event === 'hide') { f.visibility(false); f.secret.expire(); invalid = true; }
      if (event === 'expire') { f.time(30000); f.secret.expire(); invalid = true; }
    }
    reject(new Error(text)); await Promise.resolve();
    assert.equal(f.state().shown, invalid ? null : key); assert.equal(f.state().copied, 0);
    const document = {createElement(tag) { assert.equal(tag, 'span'); return {set innerHTML(_) {throw new Error('HTML injection');}, setAttribute() {throw new Error('dynamic attribute');}}; }};
    assert.equal(node(document, 'span', text).textContent, text);
    assert.equal(JSON.stringify({ids: [binding.tokenId], name: text}).includes(key), false);
  }), {numRuns: 128, verbose: true});
});
