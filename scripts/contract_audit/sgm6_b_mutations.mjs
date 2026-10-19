import { readFileSync, writeFileSync, mkdtempSync, mkdirSync, cpSync, symlinkSync, readdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
const sourceRoot = fileURLToPath(new URL('../../', import.meta.url));
const evidence = mkdtempSync('/tmp/quota-throttle-sgm6-b-mutations-');
const root = `${evidence}/workspace/`; mkdirSync(root);
// 仅复制测试所需源码和公开示例；不复制 .git、真实配置、密钥、数据库或运行目录。
for (const path of ['src', 'scripts', 'tests', 'Cargo.toml', 'Cargo.lock', 'config.example.toml', 'package.json', 'package-lock.json']) cpSync(sourceRoot + path, root + path, {recursive: true});
mkdirSync(root + 'docs/design/source-groups-token-management', {recursive: true});
cpSync(sourceRoot + 'docs/design/source-groups-token-management/schemas', root + 'docs/design/source-groups-token-management/schemas', {recursive: true});
symlinkSync(sourceRoot + 'node_modules', root + 'node_modules', 'dir');
const hash = value => createHash('sha256').update(value).digest('hex');
const inputs = [];
function recordInputs(directory) { for (const entry of readdirSync(root + directory, {withFileTypes: true})) { const path = `${directory}/${entry.name}`; if (entry.isDirectory()) recordInputs(path); else if (entry.isFile()) inputs.push(`${hash(readFileSync(root + path))}  ${path}`); } }
for (const directory of ['src', 'tests', 'scripts', 'docs']) recordInputs(directory);
for (const path of ['Cargo.toml', 'Cargo.lock', 'config.example.toml', 'package.json', 'package-lock.json']) inputs.push(`${hash(readFileSync(root + path))}  ${path}`);
writeFileSync(`${evidence}/snapshot.sha256`, inputs.sort().join('\n') + '\n');
const file = name => `src/panel_http/ui/${name}.mjs`;
const js = (name, module, before, after, testFile, testName) => ({name, path: file(module), before, after, testFile: `tests/ui/${testFile}.test.mjs`, testName});
const mutations = [
  js('refresh-failure-revives', 'core', 'fail() { this.expired = true; }', 'fail() { this.expired = false; }', 'core', 'u4 refresh failure at age 4s expires old publication and same publication cannot revive'),
  js('drop-generation', 'core', 'ticket.generation === this.generations[ticket.lane]', 'true', 'core', 'u3 invalidation retains owner until settle; old finally never releases new owner; hidden writes keep receipt'),
  js('old-finally-clears-owner', 'core', 'finish(ticket) { if (this.owns(ticket)) this.owners[ticket.lane] = null; }', 'finish(ticket) { this.owners[ticket.lane] = null; }', 'core', 'u3 invalidation retains owner until settle; old finally never releases new owner; hidden writes keep receipt'),
  js('duplicate-pump', 'app', 'this.timer = this.setTimer(() => this.tick(generation), 1000);', 'this.timer = (this.setTimer(() => this.tick(generation), 1000), this.setTimer(() => this.tick(generation), 1000));', 'lifecycle', 'u3 single pump start/hide/show/offline/destroy are idempotent'),
  js('wrong-member-revision', 'core', 'body = operationBase(p.control_revision);', 'body = operationBase(p.membership.applied_revision);', 'actions', 'u6 source/member/pin bodies use separate frozen revision categories'),
  js('retry-wrong-status', 'client', " && action !== 'retry'", '', 'client', 'u2 every route status matches the original HTTP response matrix including retry 200'),
  js('retry-new-operation-id', 'app', "route('retry', {id})", "route('retry', {id: 'qt_ffffffff-ffff-4fff-8fff-ffffffffffff'})", 'actions', 'u5 application retry and resubmit send the original ID and unchanged body'),
  js('local-id-for-usage', 'core', 'value: id(identity.native_token_id)', 'value: id(identity.token_id)', 'identity', 'u7 missing native pages never remove local historical identities or turn pending into Any'),
  js('drop-owner-dimension', 'core', '`${scope}:${id(owner)}:${id(token)}`', '`${scope}:${id(token)}`', 'identity', 'pbt6 owner-page omissions preserve identity and only exact native tuple matches'),
  js('chart-top-one', 'charts', 'const models = report.models.map(', 'const models = report.models.slice(0, 1).map(', 'usage', 'pbt7 model/bucket/native-token conservation uses independent integer totals and stable colors'),
  js('renew-sample-age', 'core', 'capture - time + elapsed <= 30000', 'capture - time + elapsed <= 60000', 'core', 'u4 group assessment downgrades when relevant 29.5s sample ages past 30s'),
  js('hidden-secret-reappears', 'app', 'slot.generation === this.generation && this.visible()', 'slot.generation === this.generation && true', 'secret', 'pbt8 malicious text and secret lifecycle never publish after clearing or expiry'),
  js('secret-generation-not-cleared', 'app', '  clear() {\n    this.generation++;', '  clear() {\n    this.generation += 0;', 'secret', 'u9 hidden, expired, changed identity and late clipboard completion cannot restore secret'),
  js('drop-send-deadline', 'client', '  try {\n    check();\n    response = await fetchImpl', '  try {\n    response = await fetchImpl', 'client', 'u2 deadline after serialization forbids sending and rejected headers cancel the unused body'),
  js('persist-extra-payload', 'core', 'JSON.stringify({ schema_version: 1, operation_ids: ids })', 'JSON.stringify({ schema_version: 1, operation_ids: ids, payload: "forbidden" })', 'actions', 'u5 persistent actions save IDs only and freeze document-local payload before sending'),
  {name: 'rust-api-header-bypass', path: 'src/panel_http.rs', before: 'match request::authorize(&head, &origin) {', after: 'match if head.path == "/api/admin/v1/ui-config" { Ok(()) } else { request::authorize(&head, &origin) } {', testName: 'panel_http::ui::tests::u1_static_does_not_weaken_api_or_origin_authorization'},
  {name: 'rust-json-as-html', path: 'src/panel_http.rs', before: 'Self::Json => "application/json; charset=utf-8",', after: 'Self::Json => "text/html; charset=utf-8",', testName: 'panel_http::ui::tests::u1_ui_config_same_worker_default_explicit_and_no_head_business'},
];
function run(command, args, label) {
  const result = spawnSync(command, args, {cwd: root, env: {...process.env, CARGO_TARGET_DIR: root + 'target'}, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024});
  const output = `${result.stdout ?? ''}\n${result.stderr ?? ''}`; writeFileSync(`${evidence}/${label}.log`, output);
  return {status: result.status, output};
}
function execute(m, label) {
  return m.testFile ? run(process.execPath, ['--test', '--experimental-test-isolation=none', '--test-reporter=tap', `--test-name-pattern=^${m.testName.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`, m.testFile], label)
    : run('cargo', ['test', '--locked', '--offline', m.testName, '--', '--exact', '--nocapture'], label);
}
const originalSourceHashes = new Map([...new Set(mutations.map(m => m.path))].map(path => [path, hash(readFileSync(sourceRoot + path))]));
for (const m of mutations) {
  const original = readFileSync(root + m.path, 'utf8'); assert.equal(original.split(m.before).length, 2, `${m.name}: unique mutation site`);
  const originalHash = hash(original);
  const baseline = execute(m, `${m.name}-baseline`); assert.equal(baseline.status, 0, `${m.name}: baseline failed`);
  assert.ok(m.testFile ? baseline.output.includes(`# pass 1\n`) : baseline.output.includes('1 passed; 0 failed;'), `${m.name}: nonempty baseline`);
  try {
    writeFileSync(root + m.path, original.replace(m.before, m.after));
    if (m.testFile) {
      const syntax = run(process.execPath, ['--check', m.path], `${m.name}-syntax`); assert.equal(syntax.status, 0);
      const load = run(process.execPath, ['--input-type=module', '-e', `await import(${JSON.stringify(`./${m.path}`)}); console.log("MUTANT_MODULE_LOADED")`], `${m.name}-load`);
      assert.equal(load.status, 0); assert.ok(load.output.includes('MUTANT_MODULE_LOADED'));
    }
    const mutated = execute(m, `${m.name}-mutated`);
    assert.equal(mutated.status, m.testFile ? 1 : 101, `${m.name}: mutation survived or environment failed`);
    assert.ok(m.testFile ? mutated.output.includes('# fail 1\n') && mutated.output.includes(`- ${m.testName}\n`) && /AssertionError|ERR_ASSERTION|Property failed/.test(mutated.output) : mutated.output.includes('0 passed; 1 failed;') && /assertion.*failed/.test(mutated.output), `${m.name}: must be one semantic target failure`);
    if (!m.testFile) assert.ok(mutated.output.includes('Compiling quota-throttle') && /Finished `test` profile/.test(mutated.output) && mutated.output.includes('Running unittests'), `${m.name}: mutation must actually compile`);
  } finally {
    writeFileSync(root + m.path, original);
    assert.equal(hash(readFileSync(root + m.path)), originalHash);
  }
  const restored = execute(m, `${m.name}-restored`); assert.equal(restored.status, 0, `${m.name}: restoration did not pass`);
  assert.ok(m.testFile ? restored.output.includes('# pass 1\n') : restored.output.includes('1 passed; 0 failed;'));
  if (!m.testFile) assert.ok(restored.output.includes('Compiling quota-throttle') && /Finished `test` profile/.test(restored.output) && restored.output.includes('Running unittests'), `${m.name}: restoration must actually recompile`);
  console.log(`${m.name}: compiled/loaded, one semantic failure, restored and re-executed`);
}
for (const [path, expected] of originalSourceHashes) assert.equal(hash(readFileSync(sourceRoot + path)), expected, `source workspace changed: ${path}`);
console.log(`Actual mutations captured: ${mutations.length}; logs ${evidence}`);
