import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {verifyBrowserResults} from '../../scripts/tests/ui-browser-reporter.mjs';
test('u10 browser gate rejects empty, skipped, missing-project and viewport-drift runs', () => {
  const manifest = JSON.parse(readFileSync(new URL('../../scripts/contract_audit/sgm6_b_browser_tests.json', import.meta.url), 'utf8'));
  const registered = manifest.projects.flatMap(project => manifest.normal.map(title => ({project, title})));
  const results = registered.map(t => ({...t, status: 'passed', expectedStatus: 'passed', retry: 0}));
  assert.equal(verifyBrowserResults(registered, results).passed, 88);
  assert.throws(() => verifyBrowserResults([], []));
  assert.throws(() => verifyBrowserResults(registered.slice(1), results.slice(1)));
  assert.throws(() => verifyBrowserResults(registered, results.map((r, i) => i === 0 ? {...r, status: 'skipped'} : r)));
  assert.throws(() => verifyBrowserResults(registered, results.map((r, i) => i === 0 ? {...r, expectedStatus: 'failed'} : r)));
  const wrong = structuredClone(registered); wrong[0].project.viewport.width = 999;
  assert.throws(() => verifyBrowserResults(wrong, results));
  assert.equal(verifyBrowserResults(registered.slice(0, 1), results.slice(0, 1), {partial: true}).mode, 'partial');
  const perf = manifest.projects.map(project => ({project, title: manifest.performance[0]}));
  assert.equal(verifyBrowserResults(perf, perf.map(t => ({...t, status: 'passed', expectedStatus: 'passed', retry: 0})), {performance: true}).passed, 4);
});
