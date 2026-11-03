import { readFileSync, writeFileSync } from 'node:fs';
import assert from 'node:assert/strict';
const manifest = JSON.parse(readFileSync(new URL('../contract_audit/sgm6_b_browser_tests.json', import.meta.url), 'utf8'));
const key = test => `${test.project.name}\0${test.title}`;
export function verifyBrowserResults(registered, results, {partial = false, performance = false} = {}) {
  const titles = performance ? manifest.performance : manifest.normal;
  const expected = manifest.projects.flatMap(project => titles.map(title => ({project, title})));
  assert.ok(registered.length > 0 && results.length > 0, 'empty browser run');
  for (const test of registered) {
    assert.ok(titles.includes(test.title), 'unregistered browser case');
    assert.deepEqual(test.project, manifest.projects.find(p => p.name === test.project.name), 'browser/viewport drift');
  }
  assert.equal(new Set(registered.map(key)).size, registered.length, 'duplicate registered test');
  if (!partial) assert.deepEqual(registered.map(key).sort(), expected.map(key).sort(), 'incomplete full browser gate');
  assert.deepEqual(results.map(key).sort(), registered.map(key).sort(), 'missing, repeated or extra execution');
  for (const result of results) assert.ok(result.status === 'passed' && result.expectedStatus === 'passed' && result.retry === 0, 'skipped/failed/xfail/retried browser case');
  return {mode: partial ? 'partial' : 'full', suite: performance ? 'performance' : 'normal', passed: results.length, expected: expected.length};
}
function entry(test) {
  const project = test.parent.project();
  return {title: test.title, project: {name: project.name, browserName: project.use.browserName, viewport: project.use.viewport}};
}
export default class ExactBrowserReporter {
  constructor() { this.registered = []; this.results = []; }
  onBegin(_config, suite) { this.registered = suite.allTests().map(entry); }
  onTestEnd(test, result) { this.results.push({...entry(test), status: result.status, expectedStatus: test.expectedStatus, retry: result.retry}); }
  async onEnd(result) {
    try {
      assert.equal(result.status, 'passed');
      const summary = verifyBrowserResults(this.registered, this.results, {partial: process.env.QT_UI_PARTIAL === '1', performance: process.env.QT_UI_SUITE === 'performance'});
      const output = process.env.QT_BROWSER_REPORT;
      assert.ok(output?.startsWith('/tmp/quota-throttle-sgm6-b-browser-') && output.endsWith('/report.json'));
      writeFileSync(output, JSON.stringify({...summary, results: this.results}, null, 2));
      console.log(`${summary.mode === 'full' ? 'FULL BROWSER GATE' : 'PARTIAL DEBUG RUN (not a full gate)'}: ${summary.passed}/${summary.expected}; ${output}`);
    } catch (error) {
      console.error(`Browser manifest gate failed: ${error.message}`);
      return {status: 'failed'};
    }
  }
}
