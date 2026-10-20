// Real browser -> real serve -> real worker; no API interception or injected replies.
import assert from 'node:assert/strict';
import {writeFileSync, realpathSync} from 'node:fs';
import {chromium, firefox, expect} from '@playwright/test';

const [base, upstream, rawDirectory] = process.argv.slice(2);
for (const value of [base, upstream]) {
  const url = new URL(value);
  assert.equal(url.protocol, 'http:'); assert.equal(url.hostname, '127.0.0.1');
  assert.ok(url.port && !['3000', '3001'].includes(url.port));
}
const directory = realpathSync(rawDirectory);
assert.ok(directory.startsWith('/tmp/quota-throttle-sgm7-combination-'));
const completed = [];
const projects = [
  ['chromium', chromium, 'desktop', {width: 1280, height: 900}],
  ['chromium', chromium, 'mobile', {width: 390, height: 844}],
  ['firefox', firefox, 'desktop', {width: 1280, height: 900}],
  ['firefox', firefox, 'mobile', {width: 390, height: 844}],
];
for (const [name, engine, viewportName, viewport] of projects) {
  const browser = await engine.launch();
  const context = await browser.newContext({viewport});
  const page = await context.newPage(), errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const label = `browser-${name}-${viewportName}`;
  try {
    const response = await page.goto(`${base}/#sources`);
    assert.equal(response.status(), 200);
    assert.ok(response.headers()['content-security-policy'].includes("script-src 'self'"));
    await expect(page.getByRole('button', {name: '添加来源', exact: true})).toBeEnabled({timeout: 20000});
    await page.getByRole('button', {name: '添加来源', exact: true}).click();
    await page.getByRole('combobox', {name: '来源类型', exact: true}).selectOption('volcano');
    await expect(page.getByRole('combobox', {name: 'NewAPI 渠道类型', exact: true})).toHaveValue('8');
    await page.getByLabel('显示名称', {exact: true}).fill(label);
    await page.getByLabel('上游凭据', {exact: true}).fill(`SYNTHETIC_${label}`);
    await page.getByLabel('上游渠道地址', {exact: true}).fill(`${upstream}/${label}/chat/completions`);
    await page.getByLabel('模型发现地址', {exact: true}).fill(`${upstream}/${label}/models`);
    const sourceResponse = page.waitForResponse(r => r.url() === `${base}/api/admin/v1/sources` && r.request().method() === 'POST');
    await page.getByRole('button', {name: '创建来源', exact: true}).click();
    const created = await sourceResponse;
    assert.equal(created.status(), 201);
    const source = await created.json();
    assert.equal(source.model_origin, 'discovered');
    await expect(page.getByRole('heading', {name: label, exact: true})).toBeVisible({timeout: 20000});
    await expect(page.getByText(`火山 · #${source.source.source_id}`, {exact: true})).toBeVisible();
    assert.equal(await page.locator('input[type=password]').count(), 0);
    assert.ok(!(await page.evaluate(() => JSON.stringify({...sessionStorage, ...localStorage}))).includes(`SYNTHETIC_${label}`));
    await page.getByRole('button', {name: '组', exact: true}).click();
    // 「＋ 新建组」在组不足 6 个时是卡片排里的一张卡（或空目录的按钮），组更多时收在「更多」菜单第一行。
    const create = page.getByRole('button', {name: '＋ 新建组', exact: true, includeHidden: true}).first();
    await create.waitFor({state: 'attached', timeout: 20000});
    if (!(await create.isVisible())) await page.locator('.gmore').click();
    await create.click();
    await page.getByLabel('显示名称', {exact: true}).fill(label);
    await page.getByLabel('NewAPI 分组标识', {exact: true}).fill(label);
    await page.getByRole('combobox', {name: '来源类型', exact: true}).selectOption('volcano');
    const groupResponse = page.waitForResponse(r => r.url() === `${base}/api/admin/v1/groups` && r.request().method() === 'POST');
    await page.getByRole('button', {name: '保存组', exact: true}).click();
    const groupReply = await groupResponse;
    assert.equal(groupReply.status(), 201);
    const group = (await groupReply.json()).group;
    // 组页的组卡片按钮名为「组名 · N 个成员」；新组尚无成员。它可能在卡片排，也可能（组多于 5 个时）在「更多」菜单里：两处都找。
    const groupButton = page.getByRole('button', {name: `${label} · 0 个成员`, exact: true});
    await expect.poll(async () => {
      if (await groupButton.isVisible()) return true;
      if (!(await page.locator('.gmore').count())) return false;
      if (!(await page.locator('.gmore-wrap.open').count())) await page.locator('.gmore').click();
      await page.getByLabel('搜索组', {exact: true}).fill(label);
      return groupButton.isVisible();
    }, {timeout: 20000}).toBe(true);
    await page.keyboard.press('Escape');
    await page.getByRole('button', {name: '数据', exact: true}).click();
    await expect(page.locator('#data-content .chart')).toBeVisible({timeout: 20000});
    // 组页也有「按模型」表（页面隐藏时仍在 DOM 里）：只在数据页内找。
    for (const model of ['gpt-4o-mini', 'gpt-4o']) await expect(page.locator('#data-content').getByText(model, {exact: true}).first()).toBeVisible();
    await page.screenshot({path: `${directory}/${label}.png`, fullPage: true});
    assert.deepEqual(errors, []);
    completed.push({browser: name, viewport, sourceCreated: true, groupCreated: true, realUsageChart: true});
    console.log(`REAL INTEGRATION BROWSER ${name}-${viewportName}: passed`);
  } finally {
    await context.close();
    await browser.close();
  }
}
assert.equal(completed.length, 4);
writeFileSync(`${directory}/browser-report.json`, JSON.stringify({expected: 4, passed: 4, apiMocks: false, completed}, null, 2));
