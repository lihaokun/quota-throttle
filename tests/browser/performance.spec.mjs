import { test, expect } from '@playwright/test';
import { maximumUsage } from '../ui/performance-fixtures.mjs';
import { volcanoState } from '../ui/fixtures.mjs';
test('serial full chart and 100-row DOM budgets including expanded source detail reuse', async ({page, browserName, browser}) => {
  await page.route('**/api/admin/v1/state', route => {
    const state = volcanoState(Date.now()); state.catalog.sources[0].volcano.runtime.next_reset_at += 86400;
    return route.fulfill({json: state, headers: {'X-QT-Snapshot-Age-Ms': '0'}});
  });
  await page.goto('/#sources'); await expect(page.getByRole('button', {name: '添加来源', exact: true})).toBeEnabled();
  const result = await page.evaluate(async snapshot => {
    const {prepareChart, barModel} = await import('/assets/charts.mjs'); const {bootstrap} = await import('/assets/app.mjs');
    const app = bootstrap(window), chart = prepareChart(snapshot); app.lifecycle.suspend();
    const flush = async () => { await new Promise(requestAnimationFrame); await new Promise(requestAnimationFrame); };
    // main 风格 div 柱状图：整份最大报表一次建好并完成布局
    const drawStart = performance.now(), plot = app.renderer.bars(chart, chart.snapshot.query); document.body.append(plot); plot.scrollIntoView({block: 'center'}); plot.getBoundingClientRect(); await flush(); const drawMs = performance.now() - drawStart;
    const domStart = performance.now(), rows = Array.from({length: 100}, (_, i) => [`原生 #${i}`, `模型 ${i}`, String(i * 10)]);
    const table = app.renderer.table(['ID', '模型', 'Token'], rows); document.body.append(table); table.getBoundingClientRect(); const domMs = performance.now() - domStart;
    app.page = 'sources'; app.sourceUsageId = '1'; app.usage = {key: 'performance', chart, periodKey: app.currentPeriodKey(), received: 'performance', error: null}; app.usageDue = Infinity;
    const render = () => { app.lifecycle.active = true; try { app.paint(); } finally { app.lifecycle.active = false; } };
    const detailStart = performance.now(); render();
    const before = document.querySelector('#source-usage-content .chart');
    if (!(before instanceof HTMLElement)) throw new Error('missing expanded-source chart');
    before.scrollIntoView({block: 'center'}); await flush(); const detailMs = performance.now() - detailStart;
    const rect = before.getBoundingClientRect(), visible = rect.width > 0 && rect.height === 110 && rect.bottom > 0 && rect.top < innerHeight;
    render(); const reused = before === document.querySelector('#source-usage-content .chart');
    return {drawMs, domMs, detailMs, reused, visible, geometry: barModel(chart).length === 745 && plot.querySelectorAll('.cb').length === 745, items: chart.bars.reduce((n, b) => n + b.parts.length, 0), rows: table.querySelectorAll('tbody tr').length};
  }, maximumUsage());
  console.log(JSON.stringify({browserName, version: browser.version(), viewport: page.viewportSize(), ...result}));
  expect(result.items).toBe(65536); expect(result.rows).toBe(100); expect(result.drawMs).toBeLessThanOrEqual(2000); expect(result.domMs).toBeLessThanOrEqual(500); expect(result.detailMs).toBeLessThanOrEqual(2000); expect(result.reused).toBe(true); expect(result.visible).toBe(true); expect(result.geometry).toBe(true);
});
