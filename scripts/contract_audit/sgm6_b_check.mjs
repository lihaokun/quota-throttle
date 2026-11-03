import { readFileSync, readdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../', import.meta.url));
const read = path => readFileSync(root + path, 'utf8');
const run = (cmd, args) => { const r = spawnSync(cmd, args, {cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024}); assert.equal(r.status, 0, `${cmd}: ${r.stderr}\n${r.stdout}`); return r.stdout; };
const base = 'f206984';
if (process.argv.includes('--freeze')) {
run('git', ['diff', '--check', base]);
run('git', ['diff', '--exit-code', base, '--', 'Cargo.toml', 'Cargo.lock', 'config.example.toml', 'docs/design/source-groups-token-management/schemas', 'src',
  ':(exclude)src/panel_http.rs', ':(exclude)src/panel_http/ui.rs', ':(exclude)src/panel_http/ui', ':(exclude)src/panel_http/tests.rs', ':(exclude)src/panel_http/admin.rs', ':(exclude)src/panel_http/admin/output.rs']);
for (const [path, extra] of [
  ['src/panel_http/tests.rs', '        Arc::new(ui::UiBundle::new(UiOptions::default(), false).unwrap()),\n'],
  ['src/panel_http/admin.rs', '                    content_type: super::ContentType::Json,\n'],
  ['src/panel_http/admin/output.rs', '        content_type: super::super::ContentType::Json,\n'],
]) {
  const current = read(path); assert.equal(current.split(extra).length, 2, path);
  assert.equal(current.replace(extra, ''), run('git', ['show', `${base}:${path}`]), `${path}: original assertions/behavior changed`);
}
const allowed = /^(?:src\/panel_http(?:\.rs|\/)|scripts\/(?:contract_audit\/sgm6_b|tests\/ui-)|tests\/(?:ui|browser)\/|docs\/audits\/sgm-6-three-page-panel\/b-|docs\/design\/source-groups-token-management\/subplans\/sgm-6-(?:browser-panel-b|three-page-panel)\.md$|\.github\/workflows\/ci\.yml$|\.gitignore$|package(?:-lock)?\.json$|playwright(?:\.performance)?\.config\.mjs$)/;
const changed = [...run('git', ['diff', '--name-only', base]).split('\n'), ...run('git', ['ls-files', '--others', '--exclude-standard']).split('\n')].filter(Boolean);
for (const path of changed) assert.match(path, allowed, `out-of-scope path: ${path}`);
}
run(process.execPath, ['scripts/contract_audit/sgm6_b_contract.mjs', '--check']);
const names = read('scripts/contract_audit/sgm6_b_node_tests.txt').trim().split('\n');
assert.equal(new Set(names).size, names.length); assert.ok(names.length >= 47);
const testFiles = readdirSync(root + 'tests/ui').filter(f => f.endsWith('.test.mjs')).sort().map(f => `tests/ui/${f}`);
const sourceNames = testFiles.flatMap(path => [...read(path).matchAll(/test\('([^']+)'/g)].map(m => m[1]));
assert.deepEqual(sourceNames.sort(), [...names].sort());
for (const path of testFiles) assert.doesNotMatch(read(path), /test\.(?:skip|todo|only)\s*\(/);
for (let n = 1; n <= 8; n++) assert.equal(names.filter(v => v.startsWith(`pbt${n} `)).length, 1);
const result = run(process.execPath, ['--test', '--experimental-test-isolation=none', ...testFiles]);
const actual = [...result.matchAll(/^ok [0-9]+ - (.+)$/gm)].map(m => m[1]);
assert.deepEqual(actual.sort(), [...names].sort()); assert.match(result, /^# fail 0$/m); assert.match(result, /^# skipped 0$/m);
const ui = ['src/panel_http.rs', 'src/panel_http/ui.rs', ...readdirSync(root + 'src/panel_http/ui').filter(f => /\.(mjs|html)$/.test(f)).map(f => `src/panel_http/ui/${f}`)];
for (let n = 1; n <= 10; n++) assert.ok(ui.some(path => read(path).includes(`Step U${n}:`)), `missing Step U${n}`);
const javascript = ui.filter(f => f.endsWith('.mjs'));
for (const path of javascript) {
  run(process.execPath, ['--check', path]);
  assert.doesNotMatch(read(path), /\.(?:innerHTML|outerHTML|insertAdjacentHTML)\b|document\.write\b|\beval\s*\(|\bsetInterval\s*\(/, path);
  if (!path.endsWith('core.mjs') && !path.endsWith('app.mjs')) assert.doesNotMatch(read(path), /(?:localStorage|sessionStorage)/, path);
}
assert.doesNotMatch(read('src/panel_http/ui/index.html'), /\s(?:style|on[a-z]+)\s*=/i);
if (process.argv.includes('--rust')) {
  const listing = run('cargo', ['test', '--locked', '--offline', '--', '--list']);
  const expectedOld = read('scripts/contract_audit/sgm6_a22_tests.txt').trim().split('\n'); assert.equal(expectedOld.length, 80);
  const actualHttp = [...listing.matchAll(/^(.+): test$/gm)].map(m => m[1]).filter(n => n.startsWith('panel_http::') || n.includes('::panel_admin::'));
  const newUi = ['u1_compiled_resource_budget_and_shared_bytes', 'u1_public_root_validation_and_canonical_byte_limit', 'u1_static_does_not_weaken_api_or_origin_authorization', 'u1_static_get_head_mime_csp_and_strict_whitelist', 'u1_ui_config_same_worker_default_explicit_and_no_head_business', 'u10_browser_fixture_server'].map(n => `panel_http::ui::tests::${n}`);
  assert.deepEqual(actualHttp.sort(), [...expectedOld, ...newUi].sort());
  const ignored = run('cargo', ['test', '--locked', '--offline', '--', '--ignored', '--list']);
  const actualIgnored = [...ignored.matchAll(/^(.+): test$/gm)].map(m => m[1]).filter(n => n.startsWith('panel_http::') || n.includes('::panel_admin::'));
  assert.deepEqual(actualIgnored.sort(), [...expectedOld.filter(n => n.includes('::performance_gate_')), 'panel_http::ui::tests::u10_browser_fixture_server'].sort());
}
console.log(`B checks: ${process.argv.includes('--freeze') ? 'core/schema/assertion freeze; ' : ''}Steps U1–U10; ${names.length} actual Node tests; 8 PBT groups; no unsafe DOM sinks${process.argv.includes('--rust') ? '; exact old 80 + UI 6 Rust manifest' : ''}.`);
