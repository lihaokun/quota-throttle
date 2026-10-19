import { modelKey, validateUsage, formatTime } from './core.mjs';

export function modelColor(model) {
  const key = modelKey(model); let hash = 2166136261;
  for (const c of key) { hash ^= c.codePointAt(0); hash = Math.imul(hash, 16777619); }
  return `hsl(${(hash >>> 0) % 360} 58% 43%)`;
}
export function prepareChart(snapshot) {
  // 校验整份报表及桶/模型边际；明细分页不截断绘图数据。
  const value = validateUsage(snapshot, snapshot.query), report = value.usage;
  if (report === null) return {kind: 'uninitialized', snapshot: value, bars: [], models: [], maximum: 0};
  const models = report.models.map(m => ({...m, key: modelKey(m.model), color: modelColor(m.model)}));
  const colors = new Map(models.map(m => [m.key, m.color]));
  let maximum = 0;
  const bars = report.buckets.map(b => { maximum = Math.max(maximum, b.total.total); return {...b, parts: b.models.map(m => ({...m, key: modelKey(m.model), color: colors.get(modelKey(m.model))}))}; });
  return {kind: report.total.consumption_rows === 0 ? 'empty' : report.total.total === 0 ? 'zero' : 'observed', snapshot: value, models, bars, maximum};
}
export function drawChart(canvas, chart, devicePixelRatio = 1) {
  const width = Math.max(1, Math.min(4096, Math.floor(canvas.getBoundingClientRect().width))), height = 320;
  const dpr = Math.max(1, Math.min(2, Number.isFinite(devicePixelRatio) ? devicePixelRatio : 1));
  canvas.width = Math.ceil(width * dpr); canvas.height = height * dpr;
  const context = canvas.getContext('2d'); if (!context) return null;
  context.setTransform(dpr, 0, 0, dpr, 0, 0); context.clearRect(0, 0, width, height);
  const left = Math.min(70, width / 4), right = 8, top = 20, bottom = 36;
  const plotWidth = Math.max(1, width - left - right), plotHeight = height - top - bottom;
  context.fillStyle = '#8b94a3'; context.font = '12px sans-serif'; context.textAlign = 'left';
  context.fillText(String(chart.maximum), 2, top); context.fillText('0', 2, height - bottom);
  const step = plotWidth / Math.max(1, chart.bars.length), labelStep = Math.max(1, Math.ceil(chart.bars.length / Math.max(1, Math.floor(plotWidth / 90))));
  chart.bars.forEach((bar, index) => {
    let accumulated = 0;
    for (const part of bar.parts) {
      const h = chart.maximum === 0 ? 0 : part.usage.total / chart.maximum * plotHeight;
      const y = height - bottom - accumulated - h; context.fillStyle = part.color;
      context.fillRect(left + index * step, y, Math.max(0.1, step * 0.9), h); accumulated += h;
    }
    if (index % labelStep === 0) { context.fillStyle = '#8b94a3'; const label = formatTime(bar.from, 's', chart.snapshot.query.utc_offset_minutes); context.fillText(label.slice(5, 16), left + index * step, height - 12, 90); }
  });
  return {width, height, left, step, count: chart.bars.length};
}
