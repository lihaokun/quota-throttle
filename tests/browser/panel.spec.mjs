import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { richState, identity, operation, usageSnapshot, volcanoState } from '../ui/fixtures.mjs';

test.beforeEach(async ({page}) => {
  page.on('pageerror', error => { throw error; });
});

async function mockState(page, mutate = () => {}) {
  await page.route('**/api/admin/v1/state', async route => { const state = richState(Date.now()); mutate(state); await route.fulfill({json: state, headers: {'X-QT-Snapshot-Age-Ms': '0'}}); });
  await page.route('**/api/admin/v1/tokens', route => route.request().method() === 'GET' ? route.fulfill({json: {schema_version: 1, after: null, next_after: null, items: [identity()]}}) : route.continue());
}
// 卡片上的次要操作收在 ⋯ 分类菜单里：先展开该项所在的菜单，再点菜单项。
async function act(page, name) {
  const item = page.getByRole('button', {name, exact: true, includeHidden: true}).first();
  await item.waitFor({state: 'attached'});  // 访问 Key 等异步数据到达前菜单项还不存在
  const menu = item.locator('xpath=ancestor::div[contains(concat(" ", normalize-space(@class), " "), " menu ")][1]');
  if (await menu.count()) await menu.locator('.menu-btn').click();
  await item.click();
}
async function openNative(page) { await page.getByText('原生 Key 页（高级）', {exact: true}).click(); }
test('real Rust listener serves HTML modules CSP and fresh empty catalog without API mocks', async ({page, request}) => {
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  const response = await page.goto('/');
  expect(response.headers()['content-security-policy']).toBe("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'none'");
  await expect(page.getByText('还没有组，先在「来源」页添加来源，再创建组。')).toBeVisible();
  await page.getByRole('button', {name: '来源', exact: true}).click();
  await expect(page.getByText('还没有来源，请添加。')).toBeVisible();
  await expect(page.getByRole('button', {name: '添加来源', exact: true})).toBeEnabled();
  expect((await request.get('/api/admin/v1/ui-config')).status()).toBe(403);
  // Playwright normalizes an empty query away; the raw TCP suite covers that exact wire case.
  expect((await request.get('/assets/app.mjs?x=1')).status()).toBe(400);
  expect((await request.get('/assets/app.mjs', {headers: {Origin: 'https://evil.invalid'}})).status()).toBe(403);
  expect(errors).toEqual([]);
});
test('API fixture navigation, safe text and shared protocol key controls', async ({page}) => {
  await mockState(page, state => { state.catalog.sources[0].source.name = '<img src=x onerror=alert(1)> __proto__'; });
  await page.goto('/');
  await expect(page.getByRole('heading', {name: /<img src=x/})).toBeVisible();
  expect(await page.locator('img').count()).toBe(0);
  await page.getByRole('button', {name: '来源', exact: true}).click();
  await expect(page.getByRole('heading', {name: /<img src=x/})).toBeVisible();
  expect(await page.locator('img').count()).toBe(0);
  await page.getByRole('button', {name: '组', exact: true}).click();
  await expect(page.getByRole('cell', {name: '测试 Key', exact: true})).toBeVisible();
  await expect(page.getByText('local #12 / native 34', {exact: false})).toBeVisible();
  await expect(page.getByText('OpenAI 兼容（https://public.example.test/proxy/v1）与 Claude（https://public.example.test/proxy）', {exact: false})).toBeVisible();
  await expect(page.getByRole('button', {name: 'https://public.example.test/proxy/v1', exact: true})).toBeVisible();
  await expect(page.getByText('需核验', {exact: true})).toBeVisible();
});
test('API fixture source pause is single explicit write with frozen revision and stable operation ID', async ({page}) => {
  await mockState(page); const writes = [];
  await page.route('**/api/admin/v1/sources/1/paused', async route => {
    writes.push(route.request().postDataJSON());
    await route.fulfill({status: 202, json: operation({operation_id: writes.at(-1).operation_id, phase: 'waiting'})});
  });
  await page.goto('/#sources'); await act(page, '暂停来源');
  await page.getByRole('button', {name: '确认提交', exact: true}).click();
  await expect(page.getByText('waiting · 确认 0/1')).toBeVisible();
  expect(writes).toHaveLength(1); expect(writes[0]).toMatchObject({schema_version: 1, expected_revision: 1, paused: true});
  expect(writes[0].operation_id).toMatch(/^qt_[0-9a-f-]{36}$/);
  const persisted = await page.evaluate(() => JSON.parse(sessionStorage.getItem('qt.pending.v1')));
  expect(persisted).toEqual({schema_version: 1, operation_ids: [writes[0].operation_id]});
});
test('API fixture source form is cleared on pagehide and does not enter storage', async ({page}) => {
  await mockState(page); await page.goto('/#sources'); await page.getByRole('button', {name: '添加来源', exact: true}).click();
  await page.getByLabel('上游凭据', {exact: true}).fill('SYNTHETIC_NOT_A_REAL_CREDENTIAL');
  await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pagehide', {persisted: true})));
  await expect(page.locator('#editor')).toBeHidden();
  expect(await page.locator('input[type=password]').count()).toBe(0);
  expect(await page.evaluate(() => JSON.stringify({...sessionStorage, ...localStorage}))).not.toContain('SYNTHETIC_NOT_A_REAL_CREDENTIAL');
  await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent('pageshow', {persisted: true})));
  await expect(page.getByRole('button', {name: '添加来源', exact: true})).toBeEnabled();
});
test('API fixture refresh marks frozen source draft conflict without replacing user input', async ({page}) => {
  let revision = 1; await mockState(page, s => { s.catalog.sources[0].source.config_revision = revision; s.glm.sources[0].source_revision = revision; });
  await page.route('**/api/admin/v1/sources/1/models', route => route.fulfill({json: {schema_version: 1, source_id: '1', source_revision: 1, models: ['glm-test']}}));
  await page.goto('/#sources'); await act(page, '查看或编辑模型');
  const input = page.getByLabel('模型 JSON 数组', {exact: true}); await input.fill('["edited-model"]'); revision = 2;
  await page.evaluate(async () => { const {bootstrap} = await import('/assets/app.mjs'); await bootstrap(window).readState(); });
  await expect(page.getByText('对象已有新版本。请关闭后重新打开，不会覆盖当前草稿或自动重发。')).toBeVisible();
  await expect(input).toHaveValue('["edited-model"]'); await expect(page.getByRole('button', {name: '确认提交', exact: true})).toBeDisabled();
});
test('API fixture native page failure retains local identity and operation reload queries same ID', async ({page}) => {
  await mockState(page);
  await page.route('**/api/admin/v1/native-tokens?*', route => route.fulfill({status: 503, json: {schema_version: 1, error: 'overloaded'}}));
  const id = operation().operation_id; let polls = 0;
  await page.addInitScript(id => { sessionStorage.setItem('qt.pending.v1', JSON.stringify({schema_version: 1, operation_ids: [id]})); }, id);
  await page.route('**/api/admin/v1/operations/*', route => { polls++; expect(route.request().method()).toBe('GET'); expect(route.request().url()).toContain(id); return route.fulfill({json: operation({phase: 'succeeded'})}); });
  await page.goto('/'); await expect(page.getByText('succeeded · 确认 0/1')).toBeVisible();
  await expect(page.getByRole('cell', {name: '测试 Key', exact: true})).toBeVisible();
  await openNative(page); await page.getByRole('button', {name: '读取本页', exact: true}).first().click();
  await expect(page.getByText(/未据此删除任何本地身份/)).toBeVisible();
  await expect(page.getByRole('cell', {name: '测试 Key', exact: true})).toBeVisible();
  await page.reload(); await expect(page.getByText('succeeded · 确认 0/1')).toBeVisible(); expect(polls).toBe(2);
});
test('API fixture usage renders distinct unknown/string models, exact totals and model share rows', async ({page}) => {
  await mockState(page);
  const queries = [];
  await page.route('**/api/admin/v1/usage/query', route => {
    const query = route.request().postDataJSON(); queries.push(query);
    const facts = [{at: query.from, model: null, token: null, input: 2, output: 3}, {at: query.from, model: 'unknown', token: '34', input: 4, output: 5}].filter(row => query.filter.model.mode === 'any' || query.filter.model.mode === 'unknown' && row.model === null || query.filter.model.mode === 'exact' && row.model === query.filter.model.value);
    return route.fulfill({json: usageSnapshot(query, facts)});
  });
  await page.goto('/'); await page.getByRole('button', {name: '数据', exact: true}).click();
  await expect(page.locator('#usage-report .cnote')).toContainText('14 tokens');
  await expect(page.locator('#usage-report .cnote')).toContainText('输入 6 · 输出 8 · 2 次请求');
  const rows = page.locator('#usage-report table').first().locator('tbody tr');
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText('未归属模型（null）'); await expect(rows.nth(0)).toContainText('2 / 3');
  await expect(rows.nth(1)).toContainText('unknown'); await expect(rows.nth(1)).toContainText('4 / 5');
  await page.getByRole('button', {name: '只看此模型', exact: true}).first().click();
  await expect(page.locator('#usage-report .cnote')).toContainText('输入 2 · 输出 3 · 1 次请求');
  expect(queries.at(-1).filter.model).toEqual({mode: 'unknown'});
});
test('API fixture slow usage survives a new five-second state publication', async ({page}) => {
  await page.clock.install(); let states = 0; await mockState(page, () => states++); let release; const held = new Promise(resolve => {release = resolve;}); let queries = 0;
  await page.route('**/api/admin/v1/usage/query', async route => { queries++; const query = route.request().postDataJSON(); await held; await route.fulfill({json: usageSnapshot(query, [{at: query.from, model: 'slow', token: '34', input: 1, output: 1}])}); });
  await page.goto('/'); await page.getByRole('button', {name: '数据', exact: true}).click();
  await expect.poll(() => queries).toBe(1);
  await page.clock.fastForward(6100); await expect.poll(() => states).toBeGreaterThanOrEqual(2);
  await page.evaluate(async () => { const {bootstrap} = await import('/assets/app.mjs'); const app = bootstrap(window); const ticket = app.lanes.owners.work; await app.readState(); if (app.lanes.owners.work !== ticket || ticket.controller.signal.aborted) throw new Error('state refresh invalidated usage'); });
  release(); await expect(page.locator('#usage-report .cnote')).toContainText('输入 1 · 输出 1 · 1 次请求'); expect(queries).toBe(1);
});
test('API fixture clipboard denial reuses one reveal and changing identity clears the manual key', async ({page}) => {
  await mockState(page); let reveals = 0;
  await page.addInitScript(() => { Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {writeText: async () => {throw new Error('denied');}}}); });
  await page.route('**/api/admin/v1/tokens/12/reveal', route => { reveals++; return route.fulfill({json: {schema_version: 1, token_id: '12', key: 'sk-SYNTHETIC0123456789abcdef'}}); });
  await page.goto('/'); await act(page, '复制访问 Key');
  await expect(page.getByLabel('临时访问 Key')).toHaveValue('sk-SYNTHETIC0123456789abcdef');
  await page.getByRole('button', {name: '复制已取得的 Key', exact: true}).click(); expect(reveals).toBe(1);
  await act(page, '启用'); await expect(page.locator('#secret')).toBeHidden();
  expect(await page.evaluate(() => JSON.stringify({...sessionStorage, ...localStorage}))).not.toContain('SYNTHETIC');
});
test('API fixture late clipboard completion cannot recreate key after hiding', async ({page}) => {
  await mockState(page);
  await page.addInitScript(() => { Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {writeText: () => new Promise((_, reject) => {window.rejectClipboard = reject;})}}); });
  await page.route('**/api/admin/v1/tokens/12/reveal', route => route.fulfill({json: {schema_version: 1, token_id: '12', key: 'sk-SYNTHETIC0123456789abcdef'}}));
  await page.goto('/'); await act(page, '复制访问 Key');
  await expect.poll(() => page.evaluate(() => typeof window.rejectClipboard)).toBe('function');
  await page.evaluate(() => { window.dispatchEvent(new PageTransitionEvent('pagehide', {persisted: true})); window.rejectClipboard(new Error('late')); });
  await expect(page.locator('#secret')).toBeHidden(); expect(await page.locator('#secret textarea').count()).toBe(0);
});
test('missing browser capability shows safe reload screen without calling old or new API', async ({page}) => {
  let calls = 0; page.on('request', request => {if (request.url().includes('/api/')) calls++;});
  await page.addInitScript(() => { window.structuredClone = undefined; });
  await page.goto('/'); await expect(page.getByText(/面板初始化失败或浏览器缺少必要能力/)).toBeVisible();
  await expect(page.getByRole('button', {name: '重新加载页面', exact: true})).toBeVisible(); expect(calls).toBe(0);
});
test('API fixture rapid model filter changes never display the late old report under the new title', async ({page}) => {
  await mockState(page); let release; const held = new Promise(resolve => {release = resolve;}); let calls = 0;
  await page.route('**/api/admin/v1/usage/query', async route => {
    const query = route.request().postDataJSON(); calls++;
    const old = query.filter.model.mode === 'any'; if (old) await held;
    try { await route.fulfill({json: usageSnapshot(query, [{at: query.from, model: old ? 'old' : 'new', token: '34', input: old ? 999 : 10, output: 10}])}); } catch { /* Explicitly aborted old request. */ }
  });
  await page.goto('/'); await page.getByRole('button', {name: '数据', exact: true}).click(); await expect.poll(() => calls).toBe(1);
  await page.getByRole('combobox', {name: '模型筛选', exact: true}).selectOption('exact'); await page.getByLabel('模型原标签', {exact: true}).fill('new'); await page.getByRole('button', {name: '应用筛选并查询', exact: true}).click();
  await expect(page.locator('#usage-report .cnote')).toContainText('输入 10 · 输出 10 · 1 次请求'); release();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await expect(page.locator('#usage-report .cnote')).toContainText('输入 10 · 输出 10 · 1 次请求'); expect(await page.getByText('输入 999', {exact: false}).count()).toBe(0);
});
test('API fixture unchanged volcano period crossing display age labels the old chart and blocks new query', async ({page}) => {
  await page.clock.install(); const state = volcanoState(100000); state.catalog.sources[0].volcano.runtime.next_reset_at = 103; let queries = 0;
  await page.route('**/api/admin/v1/state', route => route.fulfill({json: state, headers: {'X-QT-Snapshot-Age-Ms': '0'}}));
  await page.route('**/api/admin/v1/usage/query', route => { queries++; const query = route.request().postDataJSON(); return route.fulfill({json: usageSnapshot(query, [{at: query.from, model: 'volcano-test', token: '34', input: 1, output: 1}])}); });
  await page.goto('/#sources'); await act(page, '展开分模型用量');
  await expect(page.locator('#source-usage-content .cnote')).toContainText('输入 1 · 输出 1 · 1 次请求');
  await page.clock.fastForward(4000); await expect(page.getByText('以下为旧周期结果。已到重置边界，等待服务端发布新周期。', {exact: true})).toBeVisible();
  await page.evaluate(async () => { const {bootstrap} = await import('/assets/app.mjs'); bootstrap(window).queryUsage(); }); expect(queries).toBe(1);
});
test('API fixture fulfilled clipboard promise clears the slot only after confirmed success', async ({page}) => {
  await mockState(page); let reveals = 0;
  await page.addInitScript(() => { window.copies = []; Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {writeText: async key => {window.copies.push(key);}}}); });
  await page.route('**/api/admin/v1/tokens/12/reveal', route => { reveals++; return route.fulfill({json: {schema_version: 1, token_id: '12', key: 'sk-SYNTHETIC0123456789abcdef'}}); });
  await page.goto('/'); await act(page, '复制访问 Key');
  await expect(page.getByText(/访问 Key 已复制/)).toBeVisible(); await expect(page.locator('#secret')).toBeHidden(); expect(reveals).toBe(1);
  expect(await page.evaluate(async () => { const {bootstrap} = await import('/assets/app.mjs'); return {count: window.copies.length, cleared: bootstrap(window).secret.slot === null}; })).toEqual({count: 1, cleared: true});
});
test('initialization exception cleans partial listeners and leaves explicit readonly reload', async ({page}) => {
  let calls = 0; page.on('request', request => {if (request.url().includes('/api/')) calls++;});
  await page.addInitScript(() => {
    const original = Document.prototype.querySelectorAll; let failed = false;
    Document.prototype.querySelectorAll = function(selector) { if (selector === '[data-page]' && !failed) {failed = true; throw new Error('synthetic init failure');} return original.call(this, selector); };
  });
  await page.goto('/'); await expect(page.getByText(/面板初始化失败或浏览器缺少必要能力/)).toBeVisible();
  await expect(page.getByRole('button', {name: '组', exact: true})).toBeDisabled(); expect(calls).toBe(0);
});
test('API fixture source filters header refresh and collapsible operation progress remain usable', async ({page}) => {
  let reads = 0; await mockState(page, () => reads++); await page.goto('/#sources');
  await expect(page.getByRole('button', {name: '添加来源', exact: true})).toBeEnabled();
  await page.getByRole('combobox', {name: '来源类型筛选', exact: true}).selectOption('volcano'); await page.getByRole('button', {name: '筛选来源', exact: true}).click();
  await expect(page.getByText('没有匹配此类型/名称的来源。', {exact: true})).toBeVisible();
  await page.getByRole('combobox', {name: '来源类型筛选', exact: true}).selectOption('glm'); await page.getByLabel('来源名称筛选', {exact: true}).fill('智谱'); await page.getByRole('button', {name: '筛选来源', exact: true}).click();
  await expect(page.getByRole('heading', {name: '智谱来源', exact: true})).toBeVisible(); await expect(page.getByText('GLM · #1', {exact: true})).toBeVisible(); await expect(page.getByText(/^采样 \d+ 秒前$/)).toBeVisible();
  const before = reads; await page.getByRole('button', {name: '刷新状态', exact: true}).click(); await expect.poll(() => reads).toBeGreaterThan(before);
  await expect(page.locator('#operations')).toBeHidden();
  await page.getByRole('button', {name: '操作进度（0）', exact: true}).click(); await expect(page.locator('#operations')).toBeVisible();
  await page.getByRole('button', {name: '操作进度（0）', exact: true}).click(); await expect(page.locator('#operations')).toBeHidden();
  await page.getByRole('button', {name: '组', exact: true}).click(); await expect(page).toHaveURL(/#groups$/);
});
test('API fixture retired source exposes complete readonly models without a write action', async ({page}) => {
  await mockState(page, state => {state.catalog.sources[0].source.retired_at = 100; state.catalog.sources[0].retire_requested = true;});
  let writes = 0;
  await page.route('**/api/admin/v1/sources/1/models', route => {if (route.request().method() !== 'GET') writes++; return route.fulfill({json: {schema_version: 1, source_id: '1', source_revision: 1, models: ['historic', 'a\nb']}});});
  await page.goto('/#sources'); await act(page, '只读查看历史模型');
  const models = page.getByLabel('历史完整模型 JSON 数组', {exact: true}); await expect(models).toHaveAttribute('readonly', '');
  expect(JSON.parse(await models.inputValue())).toEqual(['historic', 'a\nb']); expect(writes).toBe(0);
  expect(await page.getByRole('button', {name: '确认提交', exact: true}).count()).toBe(0);
});
test('API fixture group page queries group-filtered usage and an open menu survives state refreshes', async ({page}) => {
  await page.clock.install(); let states = 0; await mockState(page, () => states++); const queries = [];
  await page.route('**/api/admin/v1/usage/query', route => { const query = route.request().postDataJSON(); queries.push(query); return route.fulfill({json: usageSnapshot(query, [{at: query.from, model: 'grouped', token: '34', input: 3, output: 4}])}); });
  await page.goto('/');
  await expect(page.locator('#group-usage-content .cnote')).toContainText('输入 3 · 输出 4 · 1 次请求');
  const grouped = () => queries.filter(q => q.filter.group_id.mode === 'exact');
  expect(grouped().at(-1).filter.group_id).toEqual({mode: 'exact', value: '2'}); expect(grouped().at(-1).grain).toBe('hour');
  await page.locator('#groups-content article .menu-btn').first().click();
  const before = states; await page.clock.runFor(11000); await expect.poll(() => states).toBeGreaterThanOrEqual(before + 2);
  await expect(page.locator('#groups-content .menu.open')).toHaveCount(1);
  await page.keyboard.press('Escape'); await page.getByRole('button', {name: '近 30 天', exact: true}).click();
  await expect.poll(() => grouped().at(-1).grain).toBe('day'); expect(grouped().at(-1).filter.group_id).toEqual({mode: 'exact', value: '2'});
});
const extraGroups = [['3', '火山主力', 'volcano', 9e7], ['4', '火山备用', 'volcano', 2e6], ['5', 'GLM 团队B', 'glm', 3e7], ['6', '实验组', 'glm', 0], ['7', '夜间批处理', 'volcano', 7e5], ['8', 'Claude 专用', 'glm', 1.2e7], ['9', '测试', 'volcano', 4e4]];
test('API fixture group tiles order pinned then current then last-hour usage with five tiles and searchable more menu', async ({page}) => {
  await mockState(page, state => {
    for (const [id, name, type] of extraGroups) state.catalog.groups.push({group: {group_id: id, newapi_group: `g${id}`, name, type, revision: 1, ready: true}, policy_revision: 1, selected_member_id: null, pinned_member_id: null, regime: type === 'glm' ? 'normal' : null});
    for (const [id, , type] of extraGroups) if (type === 'glm') state.glm.groups.push({group_id: id, cycle_id: state.glm.cycle_id, evaluated_at_ms: state.captured_at_ms - 1000, current: true, imminent_members: [], pin_release: null});
  });
  const queries = [], usage = new Map([['2', 5e6], ...extraGroups.map(([id, , , n]) => [id, n])]);
  await page.route('**/api/admin/v1/usage/query', route => {
    const query = route.request().postDataJSON(); queries.push(query);
    const rows = [...usage].filter(([g, n]) => n > 0 && (query.filter.group_id.mode !== 'exact' || query.filter.group_id.value === g)).map(([group, n]) => ({at: query.from, model: 'm', token: '34', input: n, output: 0, group}));
    return route.fulfill({json: usageSnapshot(query, rows)});
  });
  await page.addInitScript(() => { if (!sessionStorage.getItem('seeded')) { sessionStorage.setItem('seeded', '1'); localStorage.setItem('qt.panel.groups.v1', JSON.stringify({pins: ['9'], last: '2'})); } });
  await page.goto('/');
  const tiles = () => page.locator('.gtile-main').evaluateAll(list => list.map(e => e.getAttribute('aria-label')));
  // 钉住（测试）→ 当前（上次看的 GLM 组）→ 其余按上小时用量降序；最多 5 张，其余进「更多」
  await expect.poll(tiles).toEqual(['测试 · 0 个成员', 'GLM 组 · 1 个成员', '火山主力 · 0 个成员', 'GLM 团队B · 0 个成员', 'Claude 专用 · 0 个成员']);
  await expect(page.getByRole('heading', {name: 'GLM 组', exact: true, level: 2})).toBeVisible();
  const rank = queries.filter(q => q.filter.group_id.mode === 'any' && q.to - q.from === 3600);
  expect(rank).toHaveLength(1); expect(rank[0].to % 3600).toBe(0);
  await page.getByRole('button', {name: '更多（3）'}).click();
  await expect(page.getByRole('button', {name: '＋ 新建组', exact: true})).toBeVisible();
  await page.getByLabel('搜索组', {exact: true}).fill('火山');
  await expect(page.locator('.glist .grow')).toHaveCount(1);
  await page.getByRole('button', {name: '火山备用 · 0 个成员', exact: true}).click();
  await expect(page.getByRole('heading', {name: '火山备用', exact: true, level: 2})).toBeVisible();
  await expect.poll(tiles).toEqual(['测试 · 0 个成员', '火山备用 · 0 个成员', '火山主力 · 0 个成员', 'GLM 团队B · 0 个成员', 'Claude 专用 · 0 个成员']);
  // 浏览器记住上次看的组：刷新后仍是火山备用
  await page.reload();
  await expect(page.getByRole('heading', {name: '火山备用', exact: true, level: 2})).toBeVisible();
  // 钉住最多 4 个：钉满后其余卡片的 📌 置灰
  for (const name of ['火山备用', '火山主力', 'GLM 团队B']) await page.getByRole('button', {name: `钉住 ${name}`, exact: true}).click();
  await expect(page.getByRole('button', {name: '钉住 Claude 专用', exact: true})).toBeDisabled();
  expect(JSON.parse(await page.evaluate(() => localStorage.getItem('qt.panel.groups.v1'))).pins).toEqual(['9', '4', '3', '5']);
});
test('API fixture unknown usage never renders as zero and zero groups never re-query usage', async ({page}) => {
  await page.clock.install(); let queries = 0, states = 0;
  await mockState(page, state => { states++; state.catalog.groups = []; state.catalog.memberships = []; state.glm.groups = []; Object.assign(state.glm.sources[0], {state: 'unknown', observed_at_ms: null, five_hour: null, weekly: null, level: null}); });
  await page.route('**/api/admin/v1/usage/query', route => { queries++; const query = route.request().postDataJSON(); return route.fulfill({json: usageSnapshot(query, [{at: query.from, model: 'm', token: '34', input: 1, output: 1}])}); });
  await page.goto('/');
  await expect(page.getByText('还没有组，先在「来源」页添加来源，再创建组。')).toBeVisible();
  await expect(page.getByRole('cell', {name: '测试 Key', exact: true})).toBeVisible();
  await page.getByRole('button', {name: '来源', exact: true}).click();
  const card = page.locator('#sources-content article').first();
  await expect(card.locator('.tier')).toHaveText('未知'); await expect(card.getByText('尚未观测', {exact: true})).toHaveCount(2);
  expect(await card.locator('.wlab .p').count()).toBe(0); expect(await card.locator('.fill').count()).toBe(0);
  await page.getByRole('button', {name: '数据', exact: true}).click(); await expect.poll(() => queries).toBe(1);
  const reads = states; await page.clock.runFor(16000); await expect.poll(() => states).toBeGreaterThanOrEqual(reads + 2); expect(queries).toBe(1);
});
test('API fixture more menu pages many groups without closing and keeps the search across state refreshes', async ({page}) => {
  await page.clock.install(); let states = 0;
  await mockState(page, state => { states++; for (let n = 3; n <= 30; n++) state.catalog.groups.push({group: {group_id: String(n), newapi_group: `g${n}`, name: `批量组 ${n}`, type: 'volcano', revision: 1, ready: true}, policy_revision: 1, selected_member_id: null, pinned_member_id: null, regime: null}); });
  await page.route('**/api/admin/v1/usage/query', route => { const query = route.request().postDataJSON(); return route.fulfill({json: usageSnapshot(query, [])}); });
  await page.goto('/'); await page.getByRole('button', {name: '更多（24）'}).click();
  const pager = page.locator('.gpager span'), open = page.locator('.gmore-wrap.open');
  await expect(pager).toHaveText(/^第 1 \/ \d+ 页$/);
  const rows = await page.locator('.glist .grow').count(); expect(rows).toBeGreaterThanOrEqual(5);
  await page.locator('.gpager').getByRole('button', {name: '下一页', exact: true}).click();
  await expect(open).toHaveCount(1); await expect(pager).toHaveText(/^第 2 \/ \d+ 页$/);
  await page.getByLabel('搜索组', {exact: true}).fill('批量组 2');
  await expect(pager).toHaveText(/^第 1 \/ \d+ 页$/);
  for (const label of await page.locator('.glist .grow').evaluateAll(list => list.map(e => e.getAttribute('aria-label')))) expect(label).toMatch(/^批量组 2\d? · /);
  const before = states; await page.clock.runFor(11000); await expect.poll(() => states).toBeGreaterThanOrEqual(before + 2);
  await expect(open).toHaveCount(1); await expect(page.getByLabel('搜索组', {exact: true})).toHaveValue('批量组 2');
});
test('real browser cross-origin preflight and simple POST cannot mutate the isolated Rust catalog', async ({page, request, baseURL}) => {
  const server = createServer((_request, response) => {response.writeHead(200, {'Content-Type': 'text/html', Connection: 'close'}); response.end('<!doctype html><html><body>Isolated untrusted origin</body></html>');});
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve)); const port = server.address().port; expect([3000, 3001]).not.toContain(port);
  try {
    const before = await (await request.get('/api/admin/v1/state', {headers: {'X-QT-Panel': '1'}})).json(), attempts = [];
    page.on('request', request => { if (request.url().startsWith(baseURL) && request.method() === 'POST') attempts.push(request.url()); });
    await page.goto(`http://127.0.0.1:${port}/`);
    const result = await page.evaluate(async target => {
      const body = JSON.stringify({schema_version: 1, name: 'synthetic attack', newapi_group: 'synthetic-attack', type: 'glm'});
      let corsRejected = false;
      try { await fetch(`${target}/api/admin/v1/groups`, {method: 'POST', headers: {'Content-Type': 'application/json', 'X-QT-Panel': '1'}, body}); } catch { corsRejected = true; }
      let simpleRejected = false;
      try { await fetch(`${target}/api/admin/v1/groups`, {method: 'POST', mode: 'no-cors', headers: {'Content-Type': 'text/plain'}, body}); } catch { simpleRejected = true; }
      return {corsRejected, simpleRejected};
    }, baseURL);
    // CORP same-origin also rejects the no-cors response; do not require an opaque success.
    expect(result).toEqual({corsRejected: true, simpleRejected: true}); expect(attempts).toHaveLength(2);
    let after;
    await expect.poll(async () => { const response = await request.get('/api/admin/v1/state', {headers: {'X-QT-Panel': '1'}}); if (response.status() !== 200) return before.captured_at_ms; after = await response.json(); return after.captured_at_ms; }, {timeout: 10000}).not.toBe(before.captured_at_ms);
    expect(after.catalog).toEqual(before.catalog);
  } finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
});
