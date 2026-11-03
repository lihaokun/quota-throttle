import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { allFilters, makeUsageQuery, volcanoUsageQuery, validateUsage, modelFilter, modelKey, consumedFields } from '../../src/panel_http/ui/core.mjs';
import { prepareChart, modelColor, barModel } from '../../src/panel_http/ui/charts.mjs';
import { WIRE_FIELDS } from '../../src/panel_http/ui/contract.mjs';
import { usageSnapshot } from './fixtures.mjs';
const query = {schema_version: 1, from: 0, to: 7200, grain: 'hour', utc_offset_minutes: 480, filter: allFilters()};
test('u8 all consumed usage fields match schemas and nullable model identity stays distinct', () => {
  for (const name of ['resolvedFilter', 'resolvedQuery', 'totals', 'modelUsage', 'tokenUsage', 'bucket', 'usageReport', 'scanWindow', 'scan', 'ledger', 'usageSnapshot']) assert.deepEqual(consumedFields.get(name), WIRE_FIELDS[name]);
  assert.notEqual(modelKey(null), modelKey('unknown')); assert.notEqual(modelKey(null), modelKey('null'));
  assert.deepEqual(modelFilter(null), {mode: 'unknown'}); assert.deepEqual(modelFilter('unknown'), {mode: 'exact', value: 'unknown'});
  assert.throws(() => modelFilter('x'.repeat(257)));
});
test('u8 fixed offset ranges, day drilling, empty futures and volcano reset boundaries', () => {
  const anchor = Date.UTC(2026, 9, 19, 4), to = anchor / 1000;
  assert.equal(makeUsageQuery('24h', anchor).from, to - 86400);
  assert.equal(makeUsageQuery('30d', anchor).from, Date.UTC(2026, 8, 19, 16) / 1000);
  const day = makeUsageQuery('day', anchor, 480, allFilters(), '2026-10-18'); assert.equal(day.to - day.from, 86400);
  const today = makeUsageQuery('day', anchor, 480, allFilters(), '2026-10-19'); assert.equal(today.to - today.from, 12 * 3600);
  assert.throws(() => makeUsageQuery('day', anchor, 480, allFilters(), '2026-10-20'));
  assert.throws(() => makeUsageQuery('day', anchor, 480, allFilters(), '2026-02-30'));
  const source = {source: {source_id: '3', type: 'volcano', config_revision: 7}, volcano: {runtime: {period_id: 5, period_start: 100, next_reset_at: 200}}};
  assert.throws(() => volcanoUsageQuery(source, 100000)); assert.throws(() => volcanoUsageQuery(source, 200000));
  const result = volcanoUsageQuery(source, 150000); assert.equal(result.query.grain, 'hour'); assert.equal(result.periodKey, '3:7:5'); assert.deepEqual(result.query.filter.source_id, {mode: 'exact', value: '3'});
});
test('u8 uninitialized, empty and zero-token consumption retain distinct branches and zero buckets', () => {
  assert.equal(prepareChart(usageSnapshot(query, [], false)).kind, 'uninitialized');
  const empty = prepareChart(usageSnapshot(query)); assert.equal(empty.kind, 'empty'); assert.equal(empty.bars.length, 2);
  const zero = prepareChart(usageSnapshot(query, [{at: 1, model: null, token: null, input: 0, output: 0}]));
  assert.equal(zero.kind, 'zero'); assert.equal(zero.snapshot.usage.total.consumption_rows, 1); assert.equal(zero.maximum, 0);
  const bars = barModel(zero); assert.equal(bars.length, zero.bars.length);
  for (const bar of bars) { assert.ok(Number.isFinite(bar.height)); assert.equal(bar.height, 0); assert.equal(bar.zero, true); }
  const observed = barModel(prepareChart(usageSnapshot(query, [{at: 1, model: 'a', token: null, input: 2, output: 2}, {at: 3601, model: 'a', token: null, input: 1, output: 1}])));
  assert.deepEqual(observed.map(b => b.height), [100, 50]); assert.deepEqual(observed.map(b => b.rows), [1, 1]);
});
test('u8 query mismatch, overflow and model marginal drift reject the entire report', () => {
  const rows = [{at: 1, model: null, token: null, input: 2, output: 3}, {at: 3601, model: 'unknown', token: '9', input: 4, output: 5}];
  const snapshot = usageSnapshot(query, rows); assert.equal(validateUsage(snapshot, query).usage.total.total, 14);
  assert.throws(() => validateUsage(snapshot, {...query, to: 7201}));
  const bad = structuredClone(snapshot); bad.usage.models[0].usage.input++; bad.usage.models[0].usage.total++; bad.usage.models[1].usage.input--; bad.usage.models[1].usage.total--;
  assert.throws(() => validateUsage(bad, query));
  const grouped = usageSnapshot(query, rows.map((row, i) => ({...row, group: i ? '2' : null})));
  assert.deepEqual(validateUsage(grouped, query).usage.groups.map(g => [g.group_id, g.usage.total]), [[null, 5], ['2', 9]]);
  const groupDrift = structuredClone(grouped); groupDrift.usage.groups[1].usage.input++; groupDrift.usage.groups[1].usage.total++; assert.throws(() => validateUsage(groupDrift, query));
  const groupDuplicate = structuredClone(grouped); groupDuplicate.usage.groups[0].group_id = '2'; assert.throws(() => validateUsage(groupDuplicate, query));
  const groupMissing = structuredClone(grouped); delete groupMissing.usage.groups; assert.throws(() => validateUsage(groupMissing, query));
  const overflow = structuredClone(snapshot); overflow.usage.total.input = Number.MAX_SAFE_INTEGER; assert.throws(() => validateUsage(overflow, query));
  const malformed = structuredClone(snapshot); malformed.usage.buckets[1].from++; assert.throws(() => validateUsage(malformed, query));
  const impossible = structuredClone(snapshot);
  for (const t of [impossible.usage.total, ...impossible.usage.models.map(m => m.usage), ...impossible.usage.tokens.map(t => t.usage), ...impossible.usage.buckets.flatMap(b => [b.total, ...b.models.map(m => m.usage)])]) t.consumption_rows = 0;
  assert.throws(() => validateUsage(impossible, query));
});
test('pbt7 model/bucket/native-token conservation uses independent integer totals and stable colors', () => {
  fc.assert(fc.property(fc.array(fc.record({at: fc.integer({min: 0, max: 7199}), model: fc.constantFrom(null, 'unknown', 'null', '__proto__', '模型 A'), token: fc.constantFrom(null, '1', '9007199254740993'), input: fc.integer({min: 0, max: 100000}), output: fc.integer({min: 0, max: 100000})}), {maxLength: 100}), rows => {
    const chart = prepareChart(usageSnapshot(query, rows));
    const expectedInput = rows.reduce((a, row) => a + BigInt(row.input), 0n), expectedOutput = rows.reduce((a, row) => a + BigInt(row.output), 0n);
    assert.equal(BigInt(chart.snapshot.usage.total.total), expectedInput + expectedOutput);
    assert.equal(chart.snapshot.usage.total.consumption_rows, rows.length); assert.equal(chart.bars.length, 2);
    const names = new Set(rows.map(row => row.model)); assert.equal(chart.models.length, names.size);
    for (const m of chart.models) { const selected = prepareChart(usageSnapshot(query, rows.filter(row => row.model === m.model))); assert.equal(selected.models[0].color, m.color); assert.equal(m.color, modelColor(m.model)); }
  }), {numRuns: 128, verbose: true});
});
