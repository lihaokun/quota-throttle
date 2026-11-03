import { readFileSync, writeFileSync, readdirSync, realpathSync } from 'node:fs';
import assert from 'node:assert/strict';
import { LIMITS } from '../../src/panel_http/ui/contract.mjs';
import { buildAction, freezeDraft, makeUsageQuery, validateReply, validateError, validateState, validateConfig, balanceObservations } from '../../src/panel_http/ui/core.mjs';
import { richState, volcanoState, identity, operation, usageSnapshot, balanceState } from '../../tests/ui/fixtures.mjs';
const destination = realpathSync(process.argv[2]); assert.ok(destination.startsWith('/tmp/quota-throttle-sgm6-b-'));
const wires = [], glm = richState(), volcanic = volcanoState(), op = operation().operation_id;
const push = (family, definition, value) => wires.push({family, definition, value});
const sourceFields = {type: 'glm', name: 'Synthetic', note: '', credential: 'FAKE_NOT_A_KEY', channel_type: '8', base_url: 'https://example.test/chat/completions', models_url: 'https://example.test/models', discovery_auth: 'bearer', model_selection: 'discover', models: '["model-a"]', quota_url: 'https://example.test/quota?type=2', global_headers: '[]', key_headers: '[]', daily_token_limit: '', failure_threshold: '3', reset_minute: '60', utc_offset_minutes: '480'};
for (const type of ['glm', 'volcano']) for (const model_selection of ['discover', 'manual', 'discover_or_manual']) push('request', 'CreateSource', buildAction('createSource', null, {...sourceFields, type, model_selection}, glm).body);
const policy = {daily_token_limit: '', failure_threshold: '3', reset_minute: '60', utc_offset_minutes: '480'};
const cases = [
  ['CreateGroup', 'createGroup', null, {name: 'Synthetic group', newapi_group: 'test-only', type: 'glm'}, glm],
  ['PlanMembership', 'planMember', null, {source_id: '1', group_id: '2'}, glm],
  ['SetSourcePaused', 'setSourcePaused', freezeDraft('source', glm.catalog.sources[0]), {paused: true}, glm],
  ['OperationRevision', 'retireSource', freezeDraft('source', glm.catalog.sources[0]), {}, glm],
  ['OperationRevision', 'discoverModels', freezeDraft('source', glm.catalog.sources[0]), {}, glm],
  ['OperationRevision', 'ensureMember', freezeDraft('membership', glm.catalog.memberships[0]), {}, glm],
  ['OperationRevision', 'retireMember', freezeDraft('membership', glm.catalog.memberships[0]), {}, glm],
  ['SetModels', 'setModels', freezeDraft('source', glm.catalog.sources[0]), {models: '["model-a","a\\nb"]'}, glm],
  ['ClearVolcanoError', 'clearError', freezeDraft('source', volcanic.catalog.sources[0]), {}, volcanic],
  ['UpdateVolcanoPolicy', 'setVolcanoPolicy', freezeDraft('source', volcanic.catalog.sources[0]), policy, volcanic],
  ['CheckGroup', 'checkGroup', freezeDraft('group', glm.catalog.groups[0]), {register: false}, glm],
  ['SetGroupPin', 'setGroupPin', freezeDraft('group', glm.catalog.groups[0]), {membership_id: null}, glm],
  ['CreateToken', 'createToken', freezeDraft('group', glm.catalog.groups[0]), {display_name: 'Synthetic client', expires_at: ''}, glm],
  ['SetTokenEnabled', 'setTokenEnabled', freezeDraft('identity', identity()), {enabled: false}, glm],
  ['OperationRevision', 'deleteToken', freezeDraft('identity', identity()), {}, glm],
  ['RevealToken', 'reveal', freezeDraft('identity', identity()), {}, glm],
  ['RegisterToken', 'registerToken', null, {owner_scope: 'issuer', native_token_id: '34', display_name: 'Synthetic client'}, glm],
];
for (const [definition, action, draft, fields, state] of cases) push('request', definition, buildAction(action, draft, fields, state, op).body);
const bal = balanceState(), balanceContext = {unit: structuredClone(bal.balance.unit), views: balanceObservations(bal, {fresh: true, age: 0}), owner_scope: 'issuer'};
const refillBody = buildAction('refillBalance', null, {owner_scope: 'issuer', amount: '2.01'}, bal, null, balanceContext).body;
const policyBody = buildAction('setBalancePolicy', freezeDraft('balance', bal.balance.policy), {auto_refill: false, warn_below: '2.01', refill_to: '4.02'}, bal, null, balanceContext).body;
push('request', 'RefillBalance', refillBody); push('request', 'SetBalancePolicy', policyBody);
const refillReply = {schema_version: 1, owner_scope: refillBody.owner_scope, user_id: '9007199254740993', before: -10, amount: refillBody.amount, after: null};
const policyReply = {schema_version: 1, revision: policyBody.expected_revision + 1, auto_refill: policyBody.auto_refill, warn_below: policyBody.warn_below, refill_to: policyBody.refill_to};
push('response', 'BalanceRefilled', validateReply('balanceRefilled', refillReply, refillBody));
push('response', 'BalanceRefilled', validateReply('balanceRefilled', {...refillReply, after: 0}, refillBody));
push('response', 'BalancePolicySaved', validateReply('balancePolicySaved', policyReply, policyBody));
push('response', 'BalancePolicySaved', validateReply('balancePolicySaved', {...policyReply, revision: policyBody.expected_revision}, policyBody));
for (const [outcome, amount, error_code] of [['refilled', 1, null], ['not_sent', null, 'invalid_input'], ['not_sent', 1, 'not_ready'], ['rejected', 1, 'remote_rejected'], ['uncertain', 1, 'transport_unknown']]) {
  const state = structuredClone(bal);
  state.balance.accounts[0].last_refill = {at_ms: state.captured_at_ms, source: 'manual', outcome, amount, error_code};
  push('state', '', validateState(state));
}
for (const wrong of [{...refillBody, owner_scope: 'legacy_admin'}, {...refillBody, amount: refillBody.amount + 1}]) assert.throws(() => validateReply('balanceRefilled', refillReply, wrong));
for (const wrong of [{...policyBody, auto_refill: !policyBody.auto_refill}, {...policyBody, warn_below: policyBody.warn_below + 1}, {...policyBody, refill_to: policyBody.refill_to + 1}, {...policyBody, expected_revision: policyBody.expected_revision + 2}]) assert.throws(() => validateReply('balancePolicySaved', policyReply, wrong));
push('request', 'VersionOnly', {schema_version: 1}); push('request', 'IdentityQuery', {after: '9007199254740993'}); push('request', 'NativeTokenQuery', {owner_scope: 'legacy_admin', page: 2});
const query = makeUsageQuery('24h', 200000000); push('request', 'UsageQuery', query);
const usage = usageSnapshot(query, [{at: query.from, model: null, token: null, input: 0, output: 0}, {at: query.from + 1, model: 'unknown', token: '34', input: 2, output: 3}]);
push('response', 'UsageSnapshot', validateReply('usageSnapshot', usage, query)); push('state', '', validateState(glm)); push('state', '', validateState(volcanic));
for (const url of [null, 'https://public.example.test/proxy']) push('config', '', validateConfig({schema_version: 1, dry_run: false, public_api_base_url: url}));
writeFileSync(`${destination}/ui-wires.json`, JSON.stringify(wires));
console.log(`UI-generated wires: ${wires.length}, all synthetic; ${destination}/ui-wires.json`);
if (process.argv[3]) {
  const directory = realpathSync(process.argv[3]); assert.ok(directory.startsWith('/tmp/'));
  const families = {SourceCreated: 'sourceCreated', GroupCreated: 'groupCreated', MemberPlanned: 'memberPlanned', SourceModels: 'sourceModels', IdentityPage: 'identities', NativePage: 'nativePage', TokenReveal: 'reveal', UsageSnapshot: 'usageSnapshot', original_control_operation: 'operation', original_token_identity: 'identity', original_group_check: 'groupCheck', original_group_references: 'groupReferences', BalanceRefilled: 'balanceRefilled', BalancePolicySaved: 'balancePolicySaved'};
  let count = 0; const covered = new Set();
  for (const file of readdirSync(directory).filter(f => f.startsWith('response_'))) {
    const match = /^response_(.+)-([0-9]+)-([0-9]+)\.json$/.exec(file); assert.ok(match);
    const [, definition, pid, pair] = match;
    const value = JSON.parse(readFileSync(`${directory}/${file}`, 'utf8'));
    if (definition === 'AdminError') validateError(value);
    else if (['BalanceRefilled', 'BalancePolicySaved'].includes(definition)) {
      const context = JSON.parse(readFileSync(`${directory}/balance-context/${file}`, 'utf8'));
      assert.deepEqual(Object.keys(context), ['request_file']);
      const requestFamily = definition === 'BalanceRefilled' ? 'RefillBalance' : 'SetBalancePolicy';
      assert.equal(context.request_file, `request_${requestFamily}-${pid}-${pair}.json`);
      const originalBody = JSON.parse(readFileSync(`${directory}/${context.request_file}`, 'utf8'));
      validateReply(families[definition], value, originalBody);
      const mismatches = definition === 'BalanceRefilled'
        ? [{...originalBody, owner_scope: originalBody.owner_scope === 'issuer' ? 'legacy_admin' : 'issuer'}, {...originalBody, amount: originalBody.amount === 1 ? 2 : 1}]
        : [{...originalBody, auto_refill: !originalBody.auto_refill}, {...originalBody, warn_below: originalBody.warn_below === 1 ? 2 : 1}, {...originalBody, refill_to: originalBody.refill_to === 2 ? 3 : 2}, {...originalBody, expected_revision: originalBody.expected_revision <= LIMITS.jsonSafe - 2 ? originalBody.expected_revision + 2 : originalBody.expected_revision - 2}];
      for (const wrong of mismatches) assert.throws(() => validateReply(families[definition], value, wrong));
    } else validateReply(families[definition], value, definition === 'UsageSnapshot' ? value.query : {id: value.source_id ?? value.group_id ?? value.token_id, after: value.after, owner_scope: value.owner_scope, page: value.page});
    count++; covered.add(definition);
  }
  assert.equal(covered.size, 15); assert.ok(count > 0); console.log(`Actual Rust response consumer compatibility: ${count} replies, ${covered.size} families.`);
}
