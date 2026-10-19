import { readFileSync, writeFileSync, readdirSync, realpathSync } from 'node:fs';
import assert from 'node:assert/strict';
import { buildAction, freezeDraft, makeUsageQuery, validateReply, validateError, validateState, validateConfig } from '../../src/panel_http/ui/core.mjs';
import { richState, volcanoState, identity, operation, usageSnapshot } from '../../tests/ui/fixtures.mjs';
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
push('request', 'VersionOnly', {schema_version: 1}); push('request', 'IdentityQuery', {after: '9007199254740993'}); push('request', 'NativeTokenQuery', {owner_scope: 'legacy_admin', page: 2});
const query = makeUsageQuery('24h', 200000000); push('request', 'UsageQuery', query);
const usage = usageSnapshot(query, [{at: query.from, model: null, token: null, input: 0, output: 0}, {at: query.from + 1, model: 'unknown', token: '34', input: 2, output: 3}]);
push('response', 'UsageSnapshot', validateReply('usageSnapshot', usage, query)); push('state', '', validateState(glm)); push('state', '', validateState(volcanic));
for (const url of [null, 'https://public.example.test/proxy']) push('config', '', validateConfig({schema_version: 1, dry_run: false, public_api_base_url: url}));
writeFileSync(`${destination}/ui-wires.json`, JSON.stringify(wires));
console.log(`UI-generated wires: ${wires.length}, all synthetic; ${destination}/ui-wires.json`);
if (process.argv[3]) {
  const directory = realpathSync(process.argv[3]); assert.ok(directory.startsWith('/tmp/'));
  const families = {SourceCreated: 'sourceCreated', GroupCreated: 'groupCreated', MemberPlanned: 'memberPlanned', SourceModels: 'sourceModels', IdentityPage: 'identities', NativePage: 'nativePage', TokenReveal: 'reveal', UsageSnapshot: 'usageSnapshot', original_control_operation: 'operation', original_token_identity: 'identity', original_group_check: 'groupCheck', original_group_references: 'groupReferences'};
  let count = 0; const covered = new Set();
  for (const file of readdirSync(directory).filter(f => f.startsWith('response_'))) {
    const definition = /^response_(.+)-[0-9]+-[0-9]+\.json$/.exec(file)?.[1]; assert.ok(definition);
    const value = JSON.parse(readFileSync(`${directory}/${file}`, 'utf8'));
    if (definition === 'AdminError') validateError(value);
    else validateReply(families[definition], value, definition === 'UsageSnapshot' ? value.query : {id: value.source_id ?? value.group_id ?? value.token_id, after: value.after, owner_scope: value.owner_scope, page: value.page});
    count++; covered.add(definition);
  }
  assert.equal(covered.size, 13); assert.ok(count > 0); console.log(`Actual Rust response consumer compatibility: ${count} replies, ${covered.size} families.`);
}
