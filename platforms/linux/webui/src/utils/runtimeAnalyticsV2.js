export const RUNTIME_DEFAULT_PAGE_SIZE = 10;
export const RUNTIME_EVENT_PAGE_SIZES = Object.freeze([RUNTIME_DEFAULT_PAGE_SIZE, 25, 50, 100, 200]);
export const RUNTIME_EVENT_PAGE_SIZE_STORAGE_KEY = 'sumpter-runtime-event-page-size';
export const RUNTIME_ANALYTICS_PAGE_SIZE_STORAGE_KEY = 'sumpter-runtime-analytics-page-size';

// Runtime timestamps use Apple's epoch (seconds since 2001-01-01), while the
// browser's Date uses the Unix epoch.  Keep the conversion here so the
// statistics page can express “today” in the user's local timezone without
// depending on the daemon's timezone.
export const APPLE_EPOCH_OFFSET_SECONDS = 978307200;

export function runtimeTodayBounds(now = new Date()) {
  const current = now instanceof Date ? now : new Date(now);
  if (Number.isNaN(current.getTime())) return { from: null, to: null };
  const start = new Date(current.getFullYear(), current.getMonth(), current.getDate());
  return {
    from: start.getTime() / 1000 - APPLE_EPOCH_OFFSET_SECONDS,
    to: current.getTime() / 1000 - APPLE_EPOCH_OFFSET_SECONDS,
  };
}

/**
 * Return only the lower bound for a local-day query.  The server clamps the
 * upper bound to its current instant, which keeps an auto-refreshing page
 * current and avoids a stale `to` value captured at first render.
 */
export function runtimeRangeFilter(range, now = new Date()) {
  if (String(range || '').toLowerCase() !== 'today') return {};
  const { from } = runtimeTodayBounds(now);
  return from == null ? {} : { from };
}

function finiteInteger(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? Math.trunc(number) : fallback;
}

export function normalizeRuntimeEventPageSize(value, fallback = RUNTIME_DEFAULT_PAGE_SIZE) {
  const normalizedFallback = RUNTIME_EVENT_PAGE_SIZES.includes(Number(fallback))
    ? Number(fallback)
    : RUNTIME_DEFAULT_PAGE_SIZE;
  const number = finiteInteger(value, normalizedFallback);
  return RUNTIME_EVENT_PAGE_SIZES.includes(number) ? number : normalizedFallback;
}

export function readRuntimeEventPageSize(storage) {
  try {
    const target = storage ?? globalThis.localStorage;
    return normalizeRuntimeEventPageSize(target?.getItem(RUNTIME_EVENT_PAGE_SIZE_STORAGE_KEY));
  } catch {
    return RUNTIME_DEFAULT_PAGE_SIZE;
  }
}

export function persistRuntimeEventPageSize(value, storage) {
  const pageSize = normalizeRuntimeEventPageSize(value);
  try {
    const target = storage ?? globalThis.localStorage;
    target?.setItem(RUNTIME_EVENT_PAGE_SIZE_STORAGE_KEY, String(pageSize));
  } catch {
    // The selected size still applies to the current session when storage is unavailable.
  }
  return pageSize;
}

export function readRuntimeAnalyticsPageSize(storage) {
  try {
    const target = storage ?? globalThis.localStorage;
    return normalizeRuntimeEventPageSize(target?.getItem(RUNTIME_ANALYTICS_PAGE_SIZE_STORAGE_KEY));
  } catch {
    return RUNTIME_DEFAULT_PAGE_SIZE;
  }
}

export function persistRuntimeAnalyticsPageSize(value, storage) {
  const pageSize = normalizeRuntimeEventPageSize(value);
  try {
    const target = storage ?? globalThis.localStorage;
    target?.setItem(RUNTIME_ANALYTICS_PAGE_SIZE_STORAGE_KEY, String(pageSize));
  } catch {
    // Keep the in-memory selection when localStorage is unavailable.
  }
  return pageSize;
}

export function normalizeRuntimeEventPage(value, request = {}) {
  const source = value && typeof value === 'object' ? value : {};
  const events = Array.isArray(source.events) ? source.events : null;
  const hasV2Shape = events
    && source.totalCount != null
    && source.snapshotSeq != null
    && source.historyGeneration != null;
  if (!hasV2Shape) return null;

  const pageSize = normalizeRuntimeEventPageSize(source.pageSize, request.pageSize);
  const totalCount = Math.max(0, finiteInteger(source.totalCount, 0));
  const calculatedPages = totalCount ? Math.ceil(totalCount / pageSize) : 0;
  const totalPages = Math.max(0, finiteInteger(source.totalPages, calculatedPages));
  const requestedPage = Math.max(1, finiteInteger(source.page, finiteInteger(request.page, 1)));
  const page = totalPages ? Math.min(requestedPage, totalPages) : 1;

  return {
    ...source,
    apiVersion: finiteInteger(source.apiVersion, 3),
    events,
    page,
    pageSize,
    totalCount,
    totalPages,
    snapshotSeq: Math.max(0, finiteInteger(source.snapshotSeq, 0)),
    historyGeneration: Math.max(0, finiteInteger(source.historyGeneration, 0)),
    resetGeneration: Math.max(0, finiteInteger(source.resetGeneration, 0)),
    hasNext: source.hasNext == null ? page < totalPages : Boolean(source.hasNext),
    hasPrevious: source.hasPrevious == null ? page > 1 : Boolean(source.hasPrevious),
    nextCursor: source.nextCursor ?? null,
    previousCursor: source.previousCursor ?? null,
    filters: source.filters && typeof source.filters === 'object' ? source.filters : {},
  };
}

export function isRuntimeSnapshotError(error) {
  return Number(error?.status) === 409
    && ['runtime_snapshot_expired', 'runtime_snapshot_trimmed'].includes(String(error?.code || ''));
}

export function runtimeSnapshotRecoveryMessage(error) {
  if (error?.code === 'runtime_snapshot_trimmed') return '这页历史已被手动删除或重置，已回到最新快照';
  return '事件历史已重置或删除，已回到最新快照';
}

export function clampRuntimeEventPage(page, totalPages) {
  const maximum = Math.max(1, finiteInteger(totalPages, 1));
  return Math.min(maximum, Math.max(1, finiteInteger(page, 1)));
}

export function runtimeEventPageWindow(page, totalPages, radius = 2) {
  const maximum = Math.max(0, finiteInteger(totalPages, 0));
  if (!maximum) return [];
  const current = clampRuntimeEventPage(page, maximum);
  const size = Math.max(0, finiteInteger(radius, 2));
  const values = new Set([1, maximum]);
  for (let candidate = current - size; candidate <= current + size; candidate += 1) {
    if (candidate >= 1 && candidate <= maximum) values.add(candidate);
  }
  return [...values].sort((left, right) => left - right);
}

export function runtimeEventPageRange(page) {
  const totalCount = Math.max(0, finiteInteger(page?.totalCount, 0));
  const pageSize = normalizeRuntimeEventPageSize(page?.pageSize);
  const current = Math.max(1, finiteInteger(page?.page, 1));
  const itemCount = page?.itemCount == null
    ? (Array.isArray(page?.events) ? page.events.length : 0)
    : Math.max(0, finiteInteger(page.itemCount, 0));
  if (!totalCount || !itemCount) return { from: 0, to: 0 };
  const from = (current - 1) * pageSize + 1;
  return { from, to: Math.min(totalCount, from + itemCount - 1) };
}

export function normalizeRuntimePagedResult(value, {
  itemsKey = 'rows',
  page = 1,
  pageSize = RUNTIME_DEFAULT_PAGE_SIZE,
} = {}) {
  const source = value && typeof value === 'object' ? value : {};
  const items = Array.isArray(source[itemsKey]) ? source[itemsKey] : null;
  if (!items || source.totalCount == null || source.snapshotSeq == null
      || source.historyGeneration == null) return null;

  const normalizedPageSize = normalizeRuntimeEventPageSize(source.pageSize, pageSize);
  const totalCount = Math.max(0, finiteInteger(source.totalCount, 0));
  const calculatedPages = totalCount ? Math.ceil(totalCount / normalizedPageSize) : 0;
  const totalPages = Math.max(0, finiteInteger(source.totalPages, calculatedPages));
  const requestedPage = Math.max(1, finiteInteger(source.page, page));
  const currentPage = totalPages ? Math.min(requestedPage, totalPages) : 1;
  return {
    ...source,
    [itemsKey]: items,
    page: currentPage,
    pageSize: normalizedPageSize,
    totalCount,
    totalPages,
    snapshotSeq: Math.max(0, finiteInteger(source.snapshotSeq, 0)),
    historyGeneration: Math.max(0, finiteInteger(source.historyGeneration, 0)),
    retainedFromSeq: Math.max(0, finiteInteger(source.retainedFromSeq, 0)),
    hasNext: source.hasNext == null ? currentPage < totalPages : Boolean(source.hasNext),
    hasPrevious: source.hasPrevious == null ? currentPage > 1 : Boolean(source.hasPrevious),
    itemCount: items.length,
  };
}

export function runtimePagedResultRange(value, itemsKey = 'rows') {
  return runtimeEventPageRange({
    ...value,
    itemCount: Array.isArray(value?.[itemsKey]) ? value[itemsKey].length : value?.itemCount,
  });
}

export function isRuntimeV2Unsupported(error) {
  return [404, 405].includes(Number(error?.status));
}

export function runtimeV2ErrorMessage(error) {
  if (isRuntimeV2Unsupported(error)) return '当前 daemon 版本不支持此数据库分析能力';
  if (error?.code === 'runtime_projection_not_ready') return '历史索引正在后台补齐，请稍后重试';
  if (error?.code === 'runtime_snapshot_trimmed') return '当前快照涉及已手动删除的旧数据，请刷新';
  if (error?.code === 'runtime_snapshot_expired') return '当前快照已因重置或删除失效，请刷新';
  return error?.message || '数据库分析读取失败';
}

export function runtimeRatePercent(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number * 100 : null;
}

function definedValue(source, keys) {
  if (!source || typeof source !== 'object') return null;
  for (const key of keys) {
    if (!Object.prototype.hasOwnProperty.call(source, key)) continue;
    const value = source[key];
    if (value !== null && value !== undefined && value !== '') return value;
  }
  return null;
}

function numericRate(value) {
  if (typeof value === 'string' && value.trim().endsWith('%')) {
    const percentage = Number.parseFloat(value.trim().slice(0, -1));
    return Number.isFinite(percentage) ? percentage / 100 : Number.NaN;
  }
  return Number(value);
}

/**
 * Return a cache hit ratio in the wire-contract 0...1 form.
 *
 * v3 sends the two rates explicitly. Older Linux daemons only expose the
 * aggregate token fields, so token hit rate may be reconstructed only when
 * the protocol accounting semantics (or an explicit eligible-request count)
 * prove that processedInputTokens is a valid denominator. Missing fields and
 * a zero denominator intentionally stay null so the UI renders an em dash,
 * never a plausible-looking 0%.
 */
export function runtimeCacheRate(tokens, kind = 'token') {
  const source = tokens && typeof tokens === 'object' ? tokens : {};
  const explicitKeys = kind === 'request'
    ? ['cacheReadRequestRate', 'cache_read_request_rate']
    : ['cacheReadTokenRate', 'cache_read_token_rate', 'cacheHitRate', 'cache_hit_rate'];
  const explicitRaw = definedValue(source, explicitKeys);
  if (explicitRaw != null) {
    let explicit = numericRate(explicitRaw);
    // A few pre-v3 WebUI fixtures called the percentage-shaped field
    // cacheHitRate. Canonical cacheRead*Rate fields are always 0...1.
    if (explicitKeys.slice(2).some((key) => Object.prototype.hasOwnProperty.call(source, key))
        && explicit > 1 && explicit <= 100) explicit /= 100;
    if (Number.isFinite(explicit)) return Math.max(0, Math.min(1, explicit));
  }

  if (kind === 'request') {
    const hitsRaw = definedValue(source, ['cacheReadHitRequests', 'cache_read_hit_requests']);
    const reportedRaw = definedValue(source, ['cacheReadReportedRequests', 'cache_read_reported_requests']);
    const hits = Number(hitsRaw);
    const reported = Number(reportedRaw);
    if (hitsRaw != null && reportedRaw != null
        && Number.isFinite(hits) && Number.isFinite(reported) && reported > 0) {
      return Math.max(0, Math.min(1, hits / reported));
    }
    return null;
  }

  const semantics = String(definedValue(source, [
    'tokenAccountingSemantics', 'token_accounting_semantics',
  ]) || '').toLowerCase();
  const eligibleRaw = definedValue(source, [
    'cacheReadTokenEligibleRequests', 'cache_read_token_eligible_requests',
  ]);
  const eligible = Number(eligibleRaw);
  // A mixed aggregate can contain both subset and independent protocol rows.
  // Unless the daemon sends an explicit aggregate rate, its processed-input
  // denominator is not safe to use for a reconstructed percentage.
  const denominatorCertified = ['subset', 'independent'].includes(semantics)
    || (eligibleRaw != null && Number.isFinite(eligible) && eligible > 0);
  if (!denominatorCertified) return null;

  const numeratorRaw = definedValue(source, [
    'cacheReadInputTokens', 'cache_read_input_tokens',
  ]);
  const denominatorRaw = definedValue(source, [
    'processedInputTokens', 'processed_input_tokens',
  ]);
  const numerator = Number(numeratorRaw);
  const denominator = Number(denominatorRaw);
  if (numeratorRaw != null && denominatorRaw != null
      && Number.isFinite(numerator) && Number.isFinite(denominator)
      && numerator >= 0 && denominator > 0) {
    return Math.max(0, Math.min(1, numerator / denominator));
  }
  return null;
}

/**
 * Return the cache-write share in the wire-contract 0...1 form.
 *
 * Cache creation is an amount written to the provider's prompt cache; it is
 * not a cache hit. The daemon must provide an explicit, proven denominator for
 * this value. Older
 * responses without that field stay null; the UI must never reuse
 * processedInputTokens as a guessed denominator.
 */
export function runtimeCacheWriteRate(tokens) {
  const source = tokens && typeof tokens === 'object' ? tokens : {};
  const explicitRaw = definedValue(source, [
    'cacheCreationTokenRate', 'cache_creation_token_rate',
    'cacheWriteRate', 'cache_write_rate',
  ]);
  if (explicitRaw != null) {
    const explicit = numericRate(explicitRaw);
    if (Number.isFinite(explicit)) return Math.max(0, Math.min(1, explicit));
  }
  return null;
}

export function runtimeCacheRequestUnknownRequests(tokens) {
  const source = tokens && typeof tokens === 'object' ? tokens : {};
  const observedRaw = definedValue(source, ['observedRequests', 'observed_requests']);
  const reportedRaw = definedValue(source, [
    'cacheReadReportedRequests', 'cache_read_reported_requests',
  ]);
  const observed = Number(observedRaw);
  const reported = Number(reportedRaw);
  if (observedRaw == null || reportedRaw == null
      || !Number.isFinite(observed) || !Number.isFinite(reported)
      || observed < 0 || reported < 0) return null;
  return Math.max(0, observed - reported);
}

export function runtimeCostAmount(value) {
  const micros = Number(value);
  return Number.isFinite(micros) ? micros / 1_000_000 : null;
}

export function runtimePriceInput(value) {
  if (value == null || value === '') return '';
  const number = Number(value);
  if (!Number.isFinite(number)) return '';
  return (number / 1_000_000).toFixed(6).replace(/\.?0+$/, '');
}

export function runtimePriceMicros(value) {
  if (value == null || String(value).trim() === '') return null;
  const number = Number(value);
  if (!Number.isFinite(number) || number < 0) return Number.NaN;
  return Math.round(number * 1_000_000);
}

export function runtimeDateTimeLocal(timestamp, normalizeTimestampMS) {
  if (timestamp == null || timestamp === '') return '';
  const milliseconds = normalizeTimestampMS(timestamp);
  if (!Number.isFinite(milliseconds)) return '';
  const date = new Date(milliseconds);
  const pad = (number) => String(number).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
    + `T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function runtimeAppleTimestamp(value) {
  if (value == null || String(value).trim() === '') return null;
  const milliseconds = new Date(value).getTime();
  if (!Number.isFinite(milliseconds)) return Number.NaN;
  return (milliseconds / 1000) - 978_307_200;
}
