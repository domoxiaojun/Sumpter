const APPLE_EPOCH_SECONDS = 978307200;

export function localDayString(date = new Date()) {
  return `${String(date.getFullYear()).padStart(4, '0')}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}

export function nextDown(value) {
  if (Number.isNaN(value) || value === -Infinity) return value;
  if (value === 0) return -Number.MIN_VALUE;
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, value);
  view.setBigUint64(0, view.getBigUint64(0) + (value > 0 ? -1n : 1n));
  return view.getFloat64(0);
}

function parseDay(value) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value || '')) return null;
  const [year, month, day] = value.split('-').map(Number);
  const result = new Date(0);
  result.setFullYear(year, month - 1, day);
  result.setHours(0, 0, 0, 0);
  return localDayString(result) === value ? result : null;
}

export function statisticsDateBounds(startDay, endDay, now = new Date()) {
  const start = parseDay(startDay);
  const end = parseDay(endDay);
  if (!start || !end) throw new Error('请选择完整有效的开始和结束日期');
  if (start > end) throw new Error('开始日期不能晚于结束日期');
  if (endDay > localDayString(now)) throw new Error('结束日期不能晚于今天');
  const next = new Date(end);
  next.setDate(next.getDate() + 1);
  return {
    from: start.getTime() / 1000 - APPLE_EPOCH_SECONDS,
    to: Math.min(now.getTime() / 1000 - APPLE_EPOCH_SECONDS, nextDown(next.getTime() / 1000 - APPLE_EPOCH_SECONDS)),
  };
}

export function statisticsRangeIsLive(range, filters = {}, now = new Date()) {
  return range !== 'custom' || filters.customEndDay === localDayString(now);
}

export function runtimeWireRange(range) { return range === 'custom' ? 'all' : range; }
