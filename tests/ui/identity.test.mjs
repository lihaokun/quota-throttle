import test from 'node:test';
import assert from 'node:assert/strict';
import fc from 'fast-check';
import { matchNative, identityUsageFilter, validateIdentities, validateNativePage } from '../../src/panel_http/ui/core.mjs';
import { identity, native } from './fixtures.mjs';
test('u7 missing native pages never remove local historical identities or turn pending into Any', () => {
  const historical = {...identity(), deleted_at: 500}, items = [historical];
  assert.equal(matchNative(historical, []), null); assert.equal(items.length, 1);
  assert.deepEqual(identityUsageFilter(historical), {mode: 'exact', value: '34'});
  assert.throws(() => identityUsageFilter(identity('12', null)));
  assert.deepEqual(identityUsageFilter(null), {mode: 'any'});
  assert.throws(() => validateIdentities({schema_version: 1, after: null, next_after: '12', items: [identity()]}));
  assert.throws(() => validateNativePage({schema_version: 1, owner_scope: 'issuer', page: 1, page_size: 100, total: 2, items: [native(), native()]}, 'issuer', 1));
  const hundred = Array.from({length: 100}, (_, n) => identity(String(n + 1), String(n + 101)));
  assert.throws(() => validateIdentities({schema_version: 1, after: null, next_after: null, items: hundred}));
  assert.equal(validateIdentities({schema_version: 1, after: null, next_after: '100', items: hundred}).next_after, '100');
  assert.equal(validateIdentities({schema_version: 1, after: null, next_after: null, items: hundred.slice(0, 99)}).items.length, 99);
});
test('pbt6 owner-page omissions preserve identity and only exact native tuple matches', () => {
  fc.assert(fc.property(fc.bigInt({min: 1n, max: 9223372036854775806n}), fc.bigInt({min: 1n, max: 9223372036854775806n}), fc.boolean(), (owner, token, include) => {
    const i = identity('9223372036854775807', token.toString(), 'issuer', owner.toString());
    const correct = native(token.toString(), 'issuer', owner.toString());
    const wrongOwner = native(token.toString(), 'issuer', (owner + 1n).toString());
    const wrongToken = native((token + 1n).toString(), 'issuer', owner.toString());
    const wrongScope = native(token.toString(), 'legacy_admin', owner.toString());
    const pages = [{items: [wrongOwner, wrongToken, wrongScope, ...(include ? [correct] : [])]}];
    assert.equal(matchNative(i, pages), include ? correct : null);
    assert.deepEqual(identityUsageFilter(i), {mode: 'exact', value: token.toString()});
    assert.equal(i.token_id, '9223372036854775807'); assert.equal(i.deleted_at, null);
  }), { numRuns: 128, verbose: true });
});
