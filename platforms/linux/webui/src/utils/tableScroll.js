export function tableScrollMetrics(node) {
  const viewport = Math.max(0, Number(node?.clientWidth) || 0);
  const total = Math.max(viewport, Number(node?.scrollWidth) || 0);
  const overflow = Math.max(0, total - viewport);
  const max = overflow > 1 && viewport > 0 ? overflow : 0;
  return { viewport, total, max, left: Math.max(0, Math.min(max, Number(node?.scrollLeft) || 0)) };
}

export function tableScrollHint({ left, max }) {
  if (max <= 0) return '';
  if (left <= 1) return '向右滚动查看更多列';
  if (left >= max - 1) return '向左滚动查看更多列';
  return '可左右滚动查看更多列';
}
