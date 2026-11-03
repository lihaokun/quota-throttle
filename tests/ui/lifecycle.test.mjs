import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { Lifecycle } from '../../src/panel_http/ui/app.mjs';
function fixture() {
  const document = new EventTarget(), window = new EventTarget();
  document.visibilityState = 'visible'; window.navigator = { onLine: true };
  let serial = 0, ticks = 0, suspend = 0, resume = 0;
  const pending = new Map();
  const lifecycle = new Lifecycle({ document, window, onTick: () => ticks++, onSuspend: () => suspend++, onResume: () => resume++,
    setTimer: fn => { const id = ++serial; pending.set(id, fn); return id; }, clearTimer: id => pending.delete(id) });
  return { document, window, pending, lifecycle, counts: () => ({ticks, suspend, resume}), tick: () => { const first = pending.entries().next().value; if (first) { pending.delete(first[0]); first[1](); } } };
}
test('u3 single pump start/hide/show/offline/destroy are idempotent', () => {
  const f = fixture(); f.lifecycle.start(); f.lifecycle.start(); assert.equal(f.pending.size, 1);
  f.window.dispatchEvent(new Event('pagehide')); assert.equal(f.pending.size, 0);
  f.window.dispatchEvent(new Event('pagehide')); assert.equal(f.counts().suspend, 1);
  f.window.dispatchEvent(new Event('pageshow')); f.window.dispatchEvent(new Event('pageshow')); assert.equal(f.pending.size, 1);
  assert.equal(f.counts().resume, 2); f.window.dispatchEvent(new Event('offline')); assert.equal(f.pending.size, 0);
  f.document.dispatchEvent(new Event('visibilitychange')); assert.equal(f.pending.size, 0);
  f.window.dispatchEvent(new Event('online')); assert.equal(f.pending.size, 1);
  f.lifecycle.destroy(); f.lifecycle.destroy(); f.lifecycle.start(); f.window.dispatchEvent(new Event('pageshow')); assert.equal(f.pending.size, 0);
});
test('pbt3 lifecycle traces never own more than one timer and never catch up hidden work', () => {
  fc.assert(fc.property(fc.array(fc.integer({ min: 0, max: 6 }), { minLength: 1, maxLength: 200 }), events => {
    const f = fixture(); let active = true, destroyed = false; f.lifecycle.start();
    for (const event of events) {
      const before = f.counts().ticks;
      if (event === 0) f.lifecycle.start();
      if (event === 1) { f.window.dispatchEvent(new Event('pagehide')); active = false; }
      if (event === 2) { f.window.dispatchEvent(new Event('pageshow')); active = !destroyed; }
      if (event === 3) f.tick();
      if (event === 4) { f.lifecycle.destroy(); destroyed = true; active = false; }
      if (event === 5) { f.document.visibilityState = 'hidden'; f.document.dispatchEvent(new Event('visibilitychange')); active = false; }
      if (event === 6) { f.document.visibilityState = 'visible'; f.document.dispatchEvent(new Event('visibilitychange')); active = !destroyed; }
      if (f.document.visibilityState === 'hidden') active = false;
      assert.equal(f.pending.size, active ? 1 : 0);
      if (!active) assert.equal(f.counts().ticks, before);
    }
  }), { numRuns: 128, verbose: true });
});
