import { modelKey, validateUsage } from './core.mjs';

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
export function barModel(chart) {
  // main 风格 div 柱状图的几何：每桶一根柱，高度为占峰值的百分比（0–100，恒为有限数）；按模型的拆分见下方表格。
  const maximum = chart.maximum;
  return chart.bars.map(b => ({from: b.from, to: b.to, total: b.total.total, rows: b.total.consumption_rows,
    height: maximum > 0 ? Math.max(0, Math.min(100, b.total.total / maximum * 100)) : 0, zero: b.total.total === 0}));
}
