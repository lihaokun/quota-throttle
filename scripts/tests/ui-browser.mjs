import { mkdtemp, readFile, writeFile, chmod } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../', import.meta.url));
const directory = await mkdtemp('/tmp/quota-throttle-sgm6-b-browser-');
await chmod(directory, 0o700);
const args = process.argv.slice(2), performance = args.includes('--config=playwright.performance.config.mjs');
const partial = args.some(arg => arg !== '--config=playwright.performance.config.mjs');
const fixture = spawn('cargo', ['test', '--locked', 'panel_http::ui::tests::u10_browser_fixture_server', '--', '--exact', '--ignored', '--nocapture'], {
  cwd: root, env: {...process.env, QT_BROWSER_FIXTURE_DIR: directory}, stdio: ['ignore', 'inherit', 'inherit'],
});
let fixtureDone = false;
const completion = new Promise(resolve => { fixture.once('error', () => { fixtureDone = true; resolve(1); }); fixture.once('exit', code => { fixtureDone = true; resolve(code ?? 1); }); });
let code = 1, interrupted = false, runner = null;
const interrupt = () => { interrupted = true; runner?.kill('SIGTERM'); void writeFile(`${directory}/stop`, 'stop'); };
process.once('SIGINT', interrupt); process.once('SIGTERM', interrupt);
try {
  let address;
  const deadline = Date.now() + 15 * 60 * 1000;
  while (!address) {
    if (interrupted || fixtureDone || Date.now() >= deadline) throw new Error('isolated Rust fixture did not become ready');
    try { address = await readFile(`${directory}/address`, 'utf8'); } catch (error) { if (error.code !== 'ENOENT') throw error; }
    if (!address) await delay(100);
  }
  if (!/^http:\/\/127\.0\.0\.1:[0-9]+$/.test(address) || [3000, 3001].includes(Number(new URL(address).port))) throw new Error('invalid fixture address');
  console.log(`Isolated browser listener: ${address}; synthetic fixtures only`);
  runner = spawn(process.execPath, ['node_modules/@playwright/test/cli.js', 'test', ...args], {
    cwd: root, env: {...process.env, QT_BROWSER_BASE_URL: address, QT_BROWSER_REPORT: `${directory}/report.json`, QT_UI_PARTIAL: partial ? '1' : '0', QT_UI_SUITE: performance ? 'performance' : 'normal', PLAYWRIGHT_BROWSERS_PATH: process.env.PLAYWRIGHT_BROWSERS_PATH ?? '/tmp/quota-throttle-sgm6-b-browsers'}, stdio: 'inherit',
  });
  code = await new Promise(resolve => { runner.once('error', () => resolve(1)); runner.once('exit', value => resolve(value ?? 1)); });
  if (code === 0) {
    const report = JSON.parse(await readFile(`${directory}/report.json`, 'utf8'));
    if (report.mode !== (partial ? 'partial' : 'full') || report.passed <= 0 || !partial && report.passed !== report.expected) throw new Error('missing or incomplete browser gate report');
  }
} finally {
  await writeFile(`${directory}/stop`, 'stop');
  const fixtureCode = await completion;
  if (fixtureCode !== 0 || interrupted) code = 1;
  process.removeListener('SIGINT', interrupt); process.removeListener('SIGTERM', interrupt);
  console.log(`Browser fixture exited; evidence directory ${directory}`);
}
process.exitCode = code;
