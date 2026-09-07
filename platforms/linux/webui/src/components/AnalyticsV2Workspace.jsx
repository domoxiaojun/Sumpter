import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { DataTable } from './DataTable.jsx';
import { PaginationBar } from './PaginationBar.jsx';
import { StatusBadge } from './StatusBadge.jsx';
import { Icon } from '../utils/icons.jsx';
import { api } from '../services/api.js';
import { piAttributionState, PI_ATTRIBUTION_COPY } from '../utils/piAttribution.js';
import {
  CC_ATTRIBUTION_HINT,
  formatDuration,
  formatNumber,
  formatTimestamp,
  formatTokenCount,
  normalizeTimestampMS,
  clientKindLabel,
  failureKindLabel,
  failurePhaseLabel,
  shouldPromptCCAttribution,
  projectLabelText,
  projectClientKindsLabel,
  purposeLabel,
} from '../utils/helpers.js';
import { copyWithToast } from '../utils/clipboard.js';
import {
  isRuntimeSnapshotError,
  normalizeRuntimePagedResult,
  persistRuntimeAnalyticsPageSize,
  readRuntimeAnalyticsPageSize,
  runtimeAppleTimestamp,
  runtimeCacheRate,
  runtimeCostAmount,
  runtimeDateTimeLocal,
  runtimePagedResultRange,
  runtimeTodayBounds,
  runtimePriceInput,
  runtimePriceMicros,
  runtimeRatePercent,
  runtimeV2ErrorMessage,
} from '../utils/runtimeAnalyticsV2.js';

const SECTIONS = [
  { id: 'overview', label: '概览' },
  { id: 'trends', label: '趋势' },
  { id: 'tokens', label: '成本' },
  { id: 'errors', label: '错误' },
];

const TREND_METRICS = [
  ['clientRequests', '请求数', 'count'],
  ['successRate', '成功率', 'percent'],
  ['failureRate', '失败率', 'percent'],
  ['ttfbAverage', '平均首字节', 'duration'],
  ['durationAverage', '平均完成耗时', 'duration'],
  ['outputTokens', '输出 Token', 'count'],
  ['cacheTokenRate', '缓存读取命中率', 'percent'],
  ['failovers', '故障转移请求', 'count'],
  ['estimatedCost', '估算成本', 'money'],
];

function thresholdExceeded(metric, threshold) {
  if (!metric || threshold == null) return null;
  const bucket = (metric.thresholdBuckets || metric.threshold_buckets || []).find((item) => (
    Number(item?.thresholdMS ?? item?.threshold_ms) === Number(threshold)
  ));
  return bucket ? safeNumber(bucket.exceededRequests ?? bucket.exceeded_requests) : null;
}

const DIMENSION_OPTIONS = [
  ['endpoint', '入口'],
  ['model', '模型'],
  ['clientKind', '客户端'],
  ['purpose', '用途'],
  ['failureKind', '失败类型'],
  ['failurePhase', '失败阶段'],
  ['protocol', '协议路径'],
  ['streamTerminal', '流终止'],
  ['project', '项目'],
  ['session', '会话'],
];

function dimensionLabel(kind) {
  return DIMENSION_OPTIONS.find(([value]) => value === kind)?.[1] || kind;
}

function dimensionValueLabel(value, kind) {
  switch (kind) {
    case 'project': return projectLabelText(value);
    case 'internalFeature': {
      const labels = {
        ambient: 'ambient 后台功能',
        system: '系统线程',
        title: '标题生成',
        automation: '自动化',
        automated_review: '自动审查',
        guardian_review: 'Guardian 审查',
        memory_consolidation: '记忆整理',
        subagent: '子代理',
      };
      return labels[String(value || '')] || String(value || '线程类型未记录');
    }
    case 'clientKind': return clientKindLabel(value);
    case 'purpose': return purposeLabel(value);
    case 'failureKind': return failureKindLabel(value);
    case 'failurePhase': return failurePhaseLabel(value);
    case 'streamTerminal': {
      const labels = {
        completed: '正常结束',
        failed: '失败结束',
        incomplete: '未完整结束',
        interrupted: '中途断开',
        pending: '进行中',
      };
      return labels[String(value || '')] || String(value || '未记录');
    }
    case 'protocol': {
      const labels = {
        anthropic_messages: 'Anthropic Messages',
        openai_chat: 'OpenAI Chat Completions',
        openai_responses: 'OpenAI Responses',
      };
      return labels[String(value || '')] || String(value || '未记录');
    }
    default: return String(value || '未记录');
  }
}

const RANGE_OPTIONS = [
  ['today', '今天'],
  ['7d', '7 天'],
  ['30d', '30 天'],
  ['all', '全部保留历史'],
];

function safeNumber(value, fallback = 0) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

function isAbortError(error) {
  return error?.name === 'AbortError';
}

function sameJSON(left, right) {
  if (left === right) return true;
  try { return JSON.stringify(left) === JSON.stringify(right); } catch { return false; }
}

function beginLatestRequest(reference) {
  reference.current?.controller?.abort();
  const request = {
    id: safeNumber(reference.current?.id) + 1,
    controller: new AbortController(),
  };
  reference.current = request;
  return {
    ...request,
    isCurrent: () => reference.current?.id === request.id,
  };
}

function numberWithComma(value) {
  return formatTokenCount(value);
}

function percent(value, digits = 1) {
  const number = Number(value);
  return Number.isFinite(number) ? `${(number * 100).toFixed(digits)}%` : '—';
}

// Daemons before the v3 trend endpoint exposed the cache fields under
// tokenUsage (and a few development builds used snake_case).  Keep the
// display tolerant of those wire shapes while still preferring the server's
// protocol-normalized rate.  This also makes a migrated row with the raw
// token fields visible instead of silently rendering an empty card.
function mergeDefined(...values) {
  return Object.assign({}, ...values.map((value) => {
    if (!value || typeof value !== 'object') return {};
    return Object.fromEntries(Object.entries(value).filter(([, item]) => item !== null && item !== undefined));
  }));
}

function percentFromCount(numerator, denominator, digits = 1) {
  const top = safeNumber(numerator);
  const bottom = safeNumber(denominator);
  return bottom > 0 ? `${((top / bottom) * 100).toFixed(digits)}%` : '—';
}

function optionalNumberWithComma(value) {
  return value === null || value === undefined ? '—' : numberWithComma(value);
}

function tokenValue(tokens, camelCase, snakeCase = null) {
  if (!tokens || typeof tokens !== 'object') return null;
  const snake = snakeCase || camelCase.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
  return tokens[camelCase] ?? tokens[snake] ?? null;
}

function TokenCoreGrid({ tokens, compact = false }) {
  const cacheRead = tokenValue(tokens, 'cacheReadInputTokens');
  const cacheWrite = tokenValue(tokens, 'cacheCreationInputTokens');
  const cacheReadRate = runtimeCacheRate(tokens, 'token');
  const cards = [
    {
      id: 'input', label: '输入 Token', value: tokenValue(tokens, 'inputTokens'),
      detail: '输入用量',
      accent: 'var(--primary)',
    },
    {
      id: 'output', label: '输出 Token', value: tokenValue(tokens, 'outputTokens'),
      detail: '输出用量',
      accent: 'var(--accent-indigo)',
    },
    {
      id: 'cache-read', label: '缓存读取', value: cacheRead,
      detail: '缓存读取用量', tone: 'runtime-v2-token-card-cache',
      accent: 'var(--accent-cyan)',
    },
    {
      id: 'cache-write', label: '缓存写入', value: cacheWrite,
      detail: '缓存写入 Token', tone: 'runtime-v2-token-card-cache',
      accent: 'var(--accent-purple)',
    },
  ];
  return (
    <div className={`runtime-v2-token-core-grid${compact ? ' compact' : ''}`}>
      {cards.map((card) => (
        <div key={card.id} className={`runtime-v2-token-card ${card.tone || ''}`} data-token-role={card.id} style={{ '--card-accent': card.accent || 'var(--primary)' }}>
          <div className="runtime-v2-token-card-heading">
            <span>{card.label}</span>
          </div>
          <strong className="mono-cell">{optionalNumberWithComma(card.value)}</strong>
          <div className="runtime-v2-token-card-footer">
            <small>{card.detail}</small>
            {card.id === 'cache-read' && (
              <span className="runtime-v2-token-card-hit-rate">命中率 {percent(cacheReadRate)}</span>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

function percentFromOptionalCount(numerator, denominator, digits = 1) {
  if (numerator === null || numerator === undefined
      || denominator === null || denominator === undefined) return '—';
  return percentFromCount(numerator, denominator, digits);
}

function analyticsTokenUsage(analytics) {
  if (!analytics || typeof analytics !== 'object') return {};
  const value = analytics.tokenUsage ?? analytics.token_usage;
  return value && typeof value === 'object' ? value : {};
}

function legacyTrendTotals(analytics) {
  if (!analytics || typeof analytics !== 'object') return null;
  // A facet-only snapshot has no aggregate counters.  Do not turn it into a
  // fake all-zero legacy trend, otherwise the real v3 trend loading state is
  // hidden behind misleading zeros while the board request is in flight.
  const aggregateKeys = [
    'clientRequests', 'client_requests', 'clientSuccesses', 'client_successes',
    'clientFailures', 'client_failures', 'tokenUsage', 'token_usage',
  ];
  if (!aggregateKeys.some((key) => Object.prototype.hasOwnProperty.call(analytics, key))) return null;
  return {
    clientRequests: analytics.clientRequests ?? analytics.client_requests,
    clientSuccesses: analytics.clientSuccesses ?? analytics.client_successes,
    clientFailures: analytics.clientFailures ?? analytics.client_failures,
    clientCancelled: analytics.clientCancelled ?? analytics.client_cancelled,
    clientUnknownResults: analytics.clientUnknownResults
      ?? analytics.client_unknown_results
      ?? analytics.clientPending
      ?? analytics.client_pending,
    failovers: analytics.failovers ?? analytics.failover_requests ?? analytics.failovers_count,
    failoverTerminalRequests: null,
    failoverRecoveredRequests: null,
    failoverRecoveryRate: null,
    upstreamAttempts: analytics.upstreamAttempts ?? analytics.upstream_attempts,
    upstreamSuccesses: analytics.upstreamSuccesses ?? analytics.upstream_successes,
    upstreamFailures: analytics.upstreamFailures ?? analytics.upstream_failures,
    durationMS: {
      averageMS: analytics.averageDurationMS ?? analytics.average_duration_ms,
      observedRequests: 0,
      thresholdBuckets: [],
    },
    ttfbMS: {
      averageMS: analytics.averageTTFBMS ?? analytics.average_ttfb_ms,
      observedRequests: 0,
      thresholdBuckets: [],
    },
    tokens: analyticsTokenUsage(analytics),
    cost: {},
  };
}

function latencyObserved(metric) {
  return safeNumber(metric?.observedRequests ?? metric?.observed_requests);
}

function latencyAverage(metric) {
  const explicit = Number(metric?.averageMS ?? metric?.average_ms);
  if (Number.isFinite(explicit)) return explicit;
  const count = latencyObserved(metric);
  const sum = Number(metric?.sumMS ?? metric?.sum_ms);
  return count > 0 && Number.isFinite(sum) ? sum / count : null;
}

function formatBytes(value) {
  const bytes = safeNumber(value);
  if (bytes < 1024) return `${numberWithComma(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

function formatMoney(micros, currency = 'USD') {
  const amount = runtimeCostAmount(micros);
  if (amount == null) return '—';
  return new Intl.NumberFormat(undefined, {
    style: 'currency', currency, maximumFractionDigits: 6,
  }).format(amount);
}

function dateLabel(value) {
  return value == null ? '—' : formatTimestamp(value, { date: true });
}

function dimensionTokenValue(row, field) {
  const explicitPresence = row?.usageFieldPresence?.[field] ?? row?.usage_field_presence?.[field];
  if (explicitPresence != null) return safeNumber(explicitPresence) > 0 ? numberWithComma(row?.[field]) : '—';
  const tokenFields = ['inputTokens', 'outputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens', 'processedTotalTokens'];
  const hasAnyKnownUsage = tokenFields.some((key) => safeNumber(row?.[key]) > 0);
  return hasAnyKnownUsage ? numberWithComma(row?.[field]) : '—';
}

function PanelMessage({ error, onRetry, unsupported = false }) {
  if (!error) return null;
  return (
    <div className="runtime-v2-message" role="alert">
      <span>{unsupported ? runtimeV2ErrorMessage(error) : runtimeV2ErrorMessage(error)}</span>
      {onRetry && <button type="button" className="btn btn-secondary btn-sm" onClick={onRetry}>重试</button>}
    </div>
  );
}

function LoadingLine({ text = '读取中…' }) {
  return <div className="runtime-v2-loading" role="status" aria-live="polite"><span className="loading-dot" />{text}</div>;
}

function Metric({ label, value, detail, tone = '', accent = 'var(--primary)' }) {
  return (
    <div className="runtime-v2-metric" style={{ '--card-accent': accent }}>
      <span>{label}</span>
      <strong className={`mono-cell ${tone}`}>{value}</strong>
      {detail && <small>{detail}</small>}
    </div>
  );
}

function chartPoints(points, valueForPoint, width = 760, height = 220, padding = 26) {
  if (!points?.length) return '';
  const values = points.map((point) => Math.max(0, safeNumber(valueForPoint(point))));
  const max = Math.max(1, ...values);
  const innerWidth = width - padding * 2;
  const innerHeight = height - padding * 2;
  return values.map((value, index) => {
    const x = padding + (points.length === 1 ? innerWidth / 2 : (index / (points.length - 1)) * innerWidth);
    const y = height - padding - (value / max) * innerHeight;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(' ');
}

function trendMetricValue(point, metric) {
  const completed = safeNumber(point?.clientSuccesses) + safeNumber(point?.clientFailures) + safeNumber(point?.clientCancelled);
  switch (metric) {
    case 'successRate': return completed > 0 ? (safeNumber(point?.clientSuccesses) / completed) * 100 : null;
    case 'failureRate': return completed > 0 ? (safeNumber(point?.clientFailures) / completed) * 100 : null;
    case 'ttfbAverage': return latencyAverage(point?.ttfbMS);
    case 'durationAverage': return latencyAverage(point?.durationMS);
    case 'outputTokens': return safeNumber(point?.tokens?.outputTokens ?? point?.tokenUsage?.outputTokens);
    case 'cacheTokenRate': {
      const tokens = point?.tokens || point?.tokenUsage || {};
      const rate = runtimeCacheRate(tokens, 'token');
      return rate == null ? null : rate * 100;
    }
    case 'failovers': return safeNumber(point?.failovers);
    case 'estimatedCost': return runtimeCostAmount(point?.cost?.estimatedCostMicros ?? point?.estimatedCostMicros) ?? null;
    case 'clientRequests':
    default: return safeNumber(point?.clientRequests);
  }
}

function trendMetricLabel(metric) {
  return TREND_METRICS.find(([key]) => key === metric)?.[1] || metric;
}

function trendMetricUnit(metric) {
  return TREND_METRICS.find(([key]) => key === metric)?.[2] || 'count';
}

function formatTrendValue(value, metric) {
  if (value == null || !Number.isFinite(Number(value))) return '—';
  const unit = trendMetricUnit(metric);
  if (unit === 'percent') return `${Number(value).toFixed(1)}%`;
  if (unit === 'duration') return formatDuration(value);
  if (unit === 'money') return formatMoney(Number(value) * 1_000_000);
  return numberWithComma(value);
}

function TrendChart({ points, metric = 'clientRequests' }) {
  const width = 760;
  const height = 220;
  const values = (points || []).map((point) => trendMetricValue(point, metric));
  const line = chartPoints(points, (point) => trendMetricValue(point, metric), width, height);
  const maxValue = Math.max(1, ...values.filter((value) => Number.isFinite(Number(value))).map(Number));
  const latest = points?.at(-1);
  return (
    <div className="runtime-v2-chart-wrap">
      <div className="runtime-v2-chart-legend" aria-label="趋势图图例">
        <span><i className="runtime-v2-legend-line runtime-v2-legend-request" />{trendMetricLabel(metric)}</span>
        <span className="runtime-v2-chart-scale">峰值 {formatTrendValue(maxValue, metric)} / 桶</span>
      </div>
      <svg
        className="runtime-v2-chart"
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={`趋势图：${trendMetricLabel(metric)}，共 ${numberWithComma(points?.length || 0)} 个时间桶，最近桶 ${formatTrendValue(trendMetricValue(latest, metric), metric)}`}
      >
        {[0, 1, 2, 3].map((line) => {
          const y = 26 + line * ((height - 52) / 3);
          return <line key={line} x1="26" x2="734" y1={y} y2={y} className="runtime-v2-chart-grid" />;
        })}
        <polyline points={line} className="runtime-v2-chart-request" />
      </svg>
      <details className="runtime-v2-chart-table">
        <summary>显示趋势数据表（键盘和读屏备用）</summary>
        <div className="table-container">
          <table className="data-table" aria-label={`${trendMetricLabel(metric)}趋势数据表`}>
            <thead><tr><th>时间</th><th className="data-table-column-number">{trendMetricLabel(metric)}</th><th className="data-table-column-number">终态请求</th></tr></thead>
            <tbody>
              {(points || []).map((point) => (
                <tr key={`${point.bucketStart}-${point.bucketEnd}`}>
                  <td className="mono-cell">{dateLabel(point.bucketStart)}</td>
                  <td className="data-table-cell-number mono-cell">{formatTrendValue(trendMetricValue(point, metric), metric)}</td>
                  <td className="data-table-cell-number mono-cell">{numberWithComma(safeNumber(point.clientSuccesses) + safeNumber(point.clientFailures) + safeNumber(point.clientCancelled))}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </details>
    </div>
  );
}

function ErrorTable({ page, loading, onPageChange, onPageSizeChange, onSelectEvent }) {
  if (!page) return null;
  const groups = (page.groups || []).map((row, index) => ({
    ...row,
    __key: [row.failureKind, row.failurePhase, row.endpointID, row.model, row.upstreamStatusCode, index].join('|'),
  }));
  return (
    <>
      <DataTable
        className="runtime-v2-table"
        ariaLabel="错误聚合"
        tableMinWidth="1050px"
        data={groups}
        keyField="__key"
        emptyText="当前范围没有结构化错误"
        columns={[
          { title: '错误类型', type: 'text', minWidth: '190px', render: (row) => <div className="runtime-v2-cell-stack"><strong title={`${row.failureKind || '未分类'} / ${row.failurePhase || '未记录'}`}>{row.failureKind || '未分类'}</strong><small>{row.failurePhase || '阶段未记录'}</small></div> },
          { title: '入口 / 模型', type: 'text', minWidth: '220px', render: (row) => <div className="runtime-v2-cell-stack"><span title={row.endpointName || row.endpointID || ''}>{row.endpointName || row.endpointID || '—'}</span><small className="mono-cell" title={row.model || ''}>{row.model || '模型未记录'}</small></div> },
          { title: '状态', type: 'number', width: '90px', render: (row) => row.upstreamStatusCode == null ? '—' : <span className="mono-cell">{row.upstreamStatusCode}</span> },
          { title: '次数', type: 'number', width: '92px', render: (row) => <strong className="mono-cell">{numberWithComma(row.occurrences)}</strong> },
          { title: '影响请求', type: 'number', width: '110px', render: (row) => <span className="mono-cell">{numberWithComma(row.affectedRequests)}</span> },
          { title: '最近活动', type: 'time', width: '190px', render: (row) => <span className="mono-cell">{dateLabel(row.lastSeen)}</span> },
          { title: '样本', type: 'action', width: '110px', render: (row) => <div className="runtime-v2-sample-actions">{(row.sampleEventIDs || []).slice(0, 2).map((id) => <button key={id} type="button" className="btn btn-ghost btn-sm" title={id} onClick={() => onSelectEvent?.(id)}>查看</button>)}</div> },
        ]}
      />
      <PaginationBar
        itemNoun="条错误分组"
        emptySummary="当前范围没有错误分组"
        page={page.page}
        pageSize={page.pageSize}
        totalCount={page.totalCount}
        totalPages={page.totalPages}
        itemCount={groups.length}
        loading={loading}
        onPageChange={onPageChange}
        onPageSizeChange={onPageSizeChange}
        ariaLabel="错误分组分页"
      />
    </>
  );
}

function diagnosticRows(analytics, key) {
  const rows = analytics?.[key] || analytics?.[key.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`)] || [];
  return Array.isArray(rows) ? rows : [];
}

function AdvancedDiagnosticsPanel({ analytics, loading, error, onRetry }) {
  const dimensions = [
    ['models', '模型'],
    ['requestPurposes', '用途'],
    ['protocolRoutes', '协议路径'],
    ['failureKinds', '失败类型'],
    ['failurePhases', '失败阶段'],
    ['streamTerminals', '流终止'],
    ['internalFeatures', '后台功能线程'],
  ];
  return (
    <section className="runtime-v2-advanced-diagnostics" aria-labelledby="runtime-v2-advanced-diagnostics-heading">
      <div className="runtime-v2-overview-block-heading">
        <div><h3 id="runtime-v2-advanced-diagnostics-heading">结构化诊断摘要</h3><span>来自已保存事件的模型、用途、协议、失败和流终止事实</span></div>
        {onRetry && <button type="button" className="btn btn-ghost btn-sm" onClick={onRetry} disabled={loading}>刷新诊断</button>}
      </div>
      {error && <PanelMessage error={error} onRetry={onRetry} />}
      {loading && !analytics ? <LoadingLine text="正在读取结构化诊断…" /> : analytics ? (
        <>
          <div className="runtime-v2-diagnostic-summary-grid">
            {dimensions.map(([key, label]) => {
              const rows = diagnosticRows(analytics, key);
              const total = rows.reduce((sum, row) => sum + safeNumber(row.attempts ?? row.count), 0);
              const preview = rows.slice(0, 2).map((row) => `${dimensionValueLabel(row.name || row.value, key === 'requestPurposes' ? 'purpose' : key === 'failureKinds' ? 'failureKind' : key === 'failurePhases' ? 'failurePhase' : key === 'streamTerminals' ? 'streamTerminal' : key === 'protocolRoutes' ? 'protocol' : key === 'internalFeatures' ? 'internalFeature' : '')} ${numberWithComma(row.attempts ?? row.count)}`).join(' · ');
              return <div className="runtime-v2-diagnostic-summary" key={key}><strong>{label}</strong><b className="mono-cell">{numberWithComma(total)}</b><small title={preview}>{preview || '暂无记录'}</small></div>;
            })}
            <div className="runtime-v2-diagnostic-summary"><strong>工具调用</strong><b className="mono-cell">{numberWithComma(diagnosticRows(analytics, 'toolCalls').reduce((sum, row) => sum + safeNumber(row.count), 0))}</b><small>按工具名称汇总</small></div>
            <div className="runtime-v2-diagnostic-summary"><strong>Codex 元数据</strong><b className="mono-cell">{numberWithComma(analytics.codexMetadataPresent)}</b><small>已记录的客户端事件</small></div>
            <div className="runtime-v2-diagnostic-summary"><strong>后台功能请求</strong><b className="mono-cell">{numberWithComma(analytics.internalFeatureRequests)}</b><small>不计入普通项目维度</small></div>
          </div>
          <details className="runtime-v2-diagnostic-details">
            <summary>查看完整结构化分组</summary>
            <div className="runtime-v2-diagnostic-detail-grid">
              {dimensions.map(([key, label]) => {
                const rows = diagnosticRows(analytics, key);
                return <div key={key}><strong>{label}</strong><ul>{rows.slice(0, 8).map((row) => <li key={`${key}-${row.name || row.value}`}><span>{dimensionValueLabel(row.name || row.value, key === 'requestPurposes' ? 'purpose' : key === 'failureKinds' ? 'failureKind' : key === 'failurePhases' ? 'failurePhase' : key === 'streamTerminals' ? 'streamTerminal' : key === 'protocolRoutes' ? 'protocol' : key === 'internalFeatures' ? 'internalFeature' : '')}</span><b className="mono-cell">{numberWithComma(row.attempts ?? row.count)}</b></li>)}</ul></div>;
              })}
            </div>
          </details>
        </>
      ) : <div className="runtime-v2-empty">暂无结构化诊断数据</div>}
    </section>
  );
}

function CCAttributionHint({ addToast }) {
  return (
    <div className="runtime-v2-inline-hint" role="note">
      <strong>{CC_ATTRIBUTION_HINT.title}</strong>
      <p>{CC_ATTRIBUTION_HINT.message}</p>
      <div className="runtime-v2-hint-command">
        <code>{CC_ATTRIBUTION_HINT.command}</code>
        <button
          type="button"
          className="btn btn-secondary btn-sm"
          onClick={() => copyWithToast(CC_ATTRIBUTION_HINT.command, '配置命令', addToast)}
        >复制命令</button>
      </div>
      <small>{CC_ATTRIBUTION_HINT.hint}</small>
    </div>
  );
}

function dimensionSuccessRate(row) {
  const completed = safeNumber(row?.successes) + safeNumber(row?.failures) + safeNumber(row?.cancelled);
  return percentFromCount(row?.successes, completed);
}

function dimensionCostValue(row) {
  const cost = row?.cost;
  return safeNumber(cost?.pricedRequests) > 0
    ? formatMoney(cost?.estimatedCostMicros, cost?.currency || 'USD')
    : '—';
}

function unavailableCostRequests(cost) {
  return safeNumber(cost?.unpricedRequests) + safeNumber(cost?.unknownAccountingRequests);
}

function ProjectOverviewTable({ page, loading, onPageChange, onPageSizeChange, onSearchSubmit, onSortChange }) {
  const [searchDraft, setSearchDraft] = useState(page?.search || '');
  useEffect(() => setSearchDraft(page?.search || ''), [page?.search]);
  if (!page) return null;
  const rows = page.rows || [];
  const columns = [
    {
      title: '项目', key: 'name', type: 'text', minWidth: '240px', sortable: true,
      render: (row) => <div className="runtime-v2-cell-stack"><strong className="runtime-v2-ellipsis" title={row.name}>{dimensionValueLabel(row.name, 'project')}</strong><small>{projectClientKindsLabel(row.clientKinds)}</small></div>,
    },
    { title: '请求', key: 'requests', type: 'number', width: '100px', sortable: true, render: (row) => <span className="mono-cell">{numberWithComma(row.requests)}</span> },
    { title: '成功率', key: 'success_rate', type: 'number', width: '110px', sortable: true, render: (row) => <span className="mono-cell">{dimensionSuccessRate(row)}</span> },
    { title: '失败', key: 'failures', type: 'number', width: '95px', sortable: true, render: (row) => <span className="mono-cell">{numberWithComma(row.failures)}</span> },
    { title: '输入 Token', key: 'input_tokens', type: 'number', width: '132px', sortable: true, render: (row) => <span className="mono-cell">{dimensionTokenValue(row, 'inputTokens')}</span> },
    { title: '输出 Token', key: 'output_tokens', type: 'number', width: '132px', sortable: true, render: (row) => <span className="mono-cell">{dimensionTokenValue(row, 'outputTokens')}</span> },
    {
      title: '缓存读取', key: 'cache_read', type: 'number', width: '150px', sortable: true,
      render: (row) => <div className="runtime-v2-usage-cell"><span className="mono-cell">{dimensionTokenValue(row, 'cacheReadInputTokens')}</span><small>命中率 {percent(runtimeCacheRate(row, 'token'))}</small></div>,
    },
    { title: '缓存写入', key: 'cache_write', type: 'number', width: '132px', sortable: true, render: (row) => <span className="mono-cell">{dimensionTokenValue(row, 'cacheCreationInputTokens')}</span> },
    { title: '平均耗时', key: 'average_duration', type: 'time', width: '128px', sortable: true, render: (row) => <span className="mono-cell">{formatDuration(row.averageDurationMS)}</span> },
    { title: '最近活动', key: 'last_seen', type: 'time', width: '180px', sortable: true, render: (row) => <span className="mono-cell">{dateLabel(row.lastSeen)}</span> },
  ];
  return (
    <div className="runtime-v2-project-overview">
      <form className="runtime-v2-search-label" onSubmit={(event) => { event.preventDefault(); onSearchSubmit(searchDraft.trim()); }}>
        <label htmlFor="runtime-v2-project-overview-search">查找项目</label>
        <div className="runtime-v2-search-controls">
          <input id="runtime-v2-project-overview-search" className="form-input" type="search" value={searchDraft} onChange={(event) => setSearchDraft(event.target.value)} placeholder="按项目名或 ID 查找" />
          <button type="submit" className="btn btn-secondary" disabled={loading}>查找</button>
        </div>
      </form>
      <DataTable
        className="runtime-v2-table runtime-v2-project-table"
        ariaLabel="项目使用情况"
        tableMinWidth="1260px"
        data={rows}
        keyField="key"
        emptyText="当前范围没有项目数据"
        sortKey={page.sort}
        sortDirection={page.order}
        onSortChange={onSortChange}
        columns={columns}
      />
      <PaginationBar
        itemNoun="个项目"
        emptySummary="当前搜索没有项目"
        page={page.page}
        pageSize={page.pageSize}
        totalCount={page.totalCount}
        totalPages={page.totalPages}
        itemCount={rows.length}
        loading={loading}
        onPageChange={onPageChange}
        onPageSizeChange={onPageSizeChange}
        ariaLabel="项目分页"
      />
    </div>
  );
}

function DimensionTable({ page, loading, onPageChange, onPageSizeChange, onSearchSubmit, onSortChange, kind, clientKindFacets, addToast, onSessionExport, onSessionDelete, sessionActionID, onProjectStickyClear, stickyActionKey, mode = 'usage', activeRowKey, onRowClick }) {
  const [searchDraft, setSearchDraft] = useState(page?.search || '');
  useEffect(() => setSearchDraft(page?.search || ''), [page?.search, kind]);
  if (!page) return null;
  const rows = page.rows || [];
  // 提示只在项目维度出现:症状("未识别项目"行)就在这张表里。
  const showAttributionHint = kind === 'project'
    && shouldPromptCCAttribution(rows, clientKindFacets);
  const identityColumn = {
    title: dimensionLabel(kind), key: 'name', type: 'text', minWidth: '230px', sortable: true,
    render: (row) => <div className="runtime-v2-cell-stack"><strong className="runtime-v2-ellipsis" title={row.name}>{dimensionValueLabel(row.name, kind)}</strong><small>{kind === 'project' ? projectClientKindsLabel(row.clientKinds) : (row.source || '—')}</small></div>,
  };
  const usageColumns = [
    identityColumn,
    { title: '请求', key: 'requests', type: 'number', width: '100px', sortable: true, render: (row) => <span className="mono-cell">{numberWithComma(row.requests)}</span> },
    { title: '成功率', key: 'success_rate', type: 'number', width: '110px', sortable: true, render: (row) => <span className="mono-cell">{dimensionSuccessRate(row)}</span> },
    { title: '失败', key: 'failures', type: 'number', width: '100px', sortable: true, render: (row) => <span className="mono-cell">{numberWithComma(row.failures)}</span> },
    { title: '输入 Token', key: 'input_tokens', type: 'number', width: '132px', sortable: true, render: (row) => <span className="mono-cell">{dimensionTokenValue(row, 'inputTokens')}</span> },
    { title: '输出 Token', key: 'output_tokens', type: 'number', width: '132px', sortable: true, render: (row) => <span className="mono-cell">{dimensionTokenValue(row, 'outputTokens')}</span> },
    { title: '缓存读取', key: 'cache_read', type: 'number', width: '150px', sortable: true, render: (row) => <div className="runtime-v2-usage-cell"><span className="mono-cell">{dimensionTokenValue(row, 'cacheReadInputTokens')}</span><small>命中率 {percent(runtimeCacheRate(row, 'token'))}</small></div> },
    { title: '缓存写入', key: 'cache_write', type: 'number', width: '132px', sortable: true, render: (row) => <span className="mono-cell">{dimensionTokenValue(row, 'cacheCreationInputTokens')}</span> },
    { title: '平均耗时', key: 'average_duration', type: 'time', width: '128px', sortable: true, render: (row) => <span className="mono-cell">{formatDuration(row.averageDurationMS)}</span> },
    { title: '最近活动', key: 'last_seen', type: 'time', width: '190px', sortable: true, render: (row) => <span className="mono-cell">{dateLabel(row.lastSeen)}</span> },
    { title: '关联', type: 'action', width: '100px', render: (row) => <span className="runtime-v2-muted-action" title={`${numberWithComma(row.relatedCount)} 个关联维度`}>{numberWithComma(row.relatedCount)}</span> },
  ];
  const costColumns = [
    identityColumn,
    {
      // Cost is calculated from request-level price matches after the grouped
      // page is built. The server does not expose a cost ORDER BY key, so do
      // not advertise a client-only sort that would make pagination wrong.
      title: '估算成本', type: 'number', width: '150px',
      render: (row) => <div className="runtime-v2-usage-cell"><strong className="mono-cell">{dimensionCostValue(row)}</strong><small>{safeNumber(row?.cost?.pricedRequests) > 0 ? `已计价 ${numberWithComma(row.cost.pricedRequests)} 个请求` : '暂无可计价请求'}</small></div>,
    },
    { title: '请求', key: 'requests', type: 'number', width: '100px', sortable: true, render: (row) => <span className="mono-cell">{numberWithComma(row.requests)}</span> },
    { title: '暂无法计价', type: 'number', width: '132px', render: (row) => <span className="mono-cell">{numberWithComma(unavailableCostRequests(row.cost))}</span> },
    { title: '最近活动', key: 'last_seen', type: 'time', width: '180px', sortable: true, render: (row) => <span className="mono-cell">{dateLabel(row.lastSeen)}</span> },
  ];
  const columns = mode === 'cost' ? costColumns : usageColumns;
  if (mode === 'usage' && kind === 'project' && onProjectStickyClear) {
    columns.push({
      title: '操作',
      type: 'action',
      width: '130px',
      render: (row) => {
        const busy = stickyActionKey === row.key;
        return <div className="runtime-v2-sample-actions">
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            disabled={busy}
            onClick={(event) => { event.stopPropagation(); onProjectStickyClear(row); }}
            aria-label={`清除项目 ${row.name} 的粘性归属`}
            title="清除后新请求按入口库顺序重新选择入口"
          >{busy ? '清除中…' : '清除粘性'}</button>
        </div>;
      },
    });
  }
  if (mode === 'usage' && kind === 'session' && (onSessionExport || onSessionDelete)) {
    columns.push({
      title: '操作',
      type: 'action',
      width: '150px',
      render: (row) => {
        const busy = sessionActionID === row.name;
        return <div className="runtime-v2-sample-actions">
          <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={(event) => { event.stopPropagation(); onSessionExport?.(row.name); }} aria-label={`导出会话 ${row.name}`}>导出</button>
          <button type="button" className="btn btn-danger btn-sm" disabled={busy} onClick={(event) => { event.stopPropagation(); onSessionDelete?.(row.name); }} aria-label={`删除会话 ${row.name}`}>{busy ? '处理中…' : '删除'}</button>
        </div>;
      },
    });
  }
  return (
    <>
      {showAttributionHint && <CCAttributionHint addToast={addToast} />}
      {kind === 'project' && <div className="help-warning" data-pi-attribution={piAttributionState(rows)}>
        <span>pi 项目归因：{PI_ATTRIBUTION_COPY[piAttributionState(rows)]} </span>
        <a href="https://github.com/domoxiaojun/sumpter/blob/main/USAGE.md#pi-客户端" target="_blank" rel="noreferrer">接入说明</a>
      </div>}
      <form className="runtime-v2-search-label" onSubmit={(event) => { event.preventDefault(); onSearchSubmit(searchDraft.trim()); }}>
        <label htmlFor={`runtime-v2-${kind}-search`}>搜索{dimensionLabel(kind)}</label>
        <div className="runtime-v2-search-controls">
          <input id={`runtime-v2-${kind}-search`} className="form-input" type="search" value={searchDraft} onChange={(event) => setSearchDraft(event.target.value)} placeholder="输入名称后按回车" />
          <button type="submit" className="btn btn-secondary" disabled={loading}>搜索</button>
        </div>
      </form>
      <DataTable
        className={`runtime-v2-table runtime-v2-${mode}-table`}
        ariaLabel={`${dimensionLabel(kind)}${mode === 'cost' ? '成本' : '使用情况'}`}
        tableMinWidth={mode === 'cost' ? '820px' : '1500px'}
        data={rows}
        keyField="key"
        activeRowKey={activeRowKey}
        onRowClick={onRowClick}
        emptyText={`没有匹配${dimensionLabel(kind)}`}
        sortKey={page.sort}
        sortDirection={page.order}
        onSortChange={onSortChange}
        columns={columns}
      />
      <PaginationBar
        itemNoun={`个${dimensionLabel(kind)}`}
        emptySummary={`当前搜索没有${dimensionLabel(kind)}`}
        page={page.page}
        pageSize={page.pageSize}
        totalCount={page.totalCount}
        totalPages={page.totalPages}
        itemCount={rows.length}
        loading={loading}
        onPageChange={onPageChange}
        onPageSizeChange={onPageSizeChange}
        ariaLabel={`${dimensionLabel(kind)}${mode === 'cost' ? '成本' : '使用'}分页`}
      />
    </>
  );
}

const USAGE_DIMENSION_META = [
  ['endpoint', '入口使用情况'],
  ['project', '项目使用情况'],
  ['session', '会话使用情况'],
  ['model', '模型使用情况'],
];

function DimensionBlock({
  kind,
  title,
  page,
  loading,
  error,
  onRetry,
  onPageChange,
  onPageSizeChange,
  onSearchSubmit,
  onSortChange,
  clientKindFacets,
  addToast,
  onSessionExport,
  onSessionDelete,
  sessionActionID,
  onProjectStickyClear,
  stickyActionKey,
  activeRowKey,
  onRowClick,
  selectedProjectLabel,
  onClearProject,
  selectedSessionLabel,
  onClearSession,
  mode = 'usage',
}) {
  const displayTitle = kind === 'session' && selectedProjectLabel
    ? `${title} · 项目：${selectedProjectLabel}`
    : kind === 'model' && selectedSessionLabel
      ? `${title} · 会话：${selectedSessionLabel}`
      : kind === 'model' && selectedProjectLabel
        ? `${title} · 项目：${selectedProjectLabel}`
    : title;
  return (
    <section className={`runtime-v2-dimension-block runtime-v2-dimension-block-${mode}`} aria-labelledby={`runtime-v2-${mode}-${kind}-heading`}>
      <div className="runtime-v2-overview-block-heading">
        <h3 id={`runtime-v2-${mode}-${kind}-heading`}>{displayTitle}</h3>
        <span className="runtime-v2-dimension-heading-meta">
          {page ? `共 ${numberWithComma(page.totalCount)} 个${dimensionLabel(kind)}` : '读取中…'}
          {kind === 'session' && selectedProjectLabel && <button type="button" className="btn btn-ghost btn-sm" onClick={onClearProject}>清除项目选择</button>}
          {kind === 'model' && selectedSessionLabel && <button type="button" className="btn btn-ghost btn-sm" onClick={onClearSession}>清除会话选择</button>}
          {kind === 'model' && !selectedSessionLabel && selectedProjectLabel && <button type="button" className="btn btn-ghost btn-sm" onClick={onClearProject}>清除项目选择</button>}
        </span>
      </div>
      <PanelMessage error={error} onRetry={onRetry} />
      {loading && !page ? <LoadingLine text={`正在读取${displayTitle}…`} /> : page ? (
        <DimensionTable
          page={page}
          loading={loading}
          kind={kind}
          mode={mode}
          clientKindFacets={clientKindFacets}
          addToast={addToast}
          sessionActionID={sessionActionID}
          onSessionExport={onSessionExport}
          onSessionDelete={onSessionDelete}
          onProjectStickyClear={onProjectStickyClear}
          stickyActionKey={stickyActionKey}
          activeRowKey={activeRowKey}
          onRowClick={onRowClick}
          onPageChange={onPageChange}
          onPageSizeChange={onPageSizeChange}
          onSearchSubmit={onSearchSubmit}
          onSortChange={onSortChange}
        />
      ) : <div className="runtime-v2-empty">当前范围没有{dimensionLabel(kind)}数据</div>}
    </section>
  );
}

function storageStatePresentation(storage) {
  const state = String(storage?.state || '').trim().toLowerCase();
  const missingIndexes = Array.isArray(storage?.missingIndexes) ? storage.missingIndexes : [];
  if (state === 'error' || state === 'failed' || state === 'unavailable') {
    return { label: '需要关注', kind: 'critical', detail: '数据存储暂时不可用' };
  }
  if (state === 'backpressure') {
    return { label: '需要关注', kind: 'warning', detail: '有记录正在等待写入' };
  }
  if (state === 'degraded' || storage?.hourlyRollupFailed === true || missingIndexes.length > 0) {
    return { label: '需要关注', kind: 'warning', detail: '统计存储需要检查' };
  }
  if (storage && (
    storage.projectionIndexesReady === false
    || storage.projectionBackfillComplete === false
    || storage.hourlyRollupComplete === false
  )) {
    return { label: '正在整理', kind: 'warning', detail: '统计索引或聚合正在整理' };
  }
  if (state === 'ready' || storage?.backend === 'sqlite') {
    return { label: '统计已就绪', kind: 'good', detail: storage?.lastError ? `最近一次写入异常：${storage.lastError}` : '统计记录正在本机 SQLite 保存' };
  }
  return { label: '读取中', kind: 'muted', detail: '正在读取存储状态' };
}

function StorageTechnicalDetails({ storage }) {
  if (!storage) return null;
  const databaseBytes = storage.databaseBytes ?? storage.dbBytes;
  const effectiveBytes = storage.liveBytes ?? databaseBytes;
  const walBytes = storage.walBytes;
  const missingIndexes = Array.isArray(storage.missingIndexes) ? storage.missingIndexes : null;
  const count = (value) => value == null ? '—' : numberWithComma(value);
  const hourlyRollup = storage.hourlyRollupComplete == null
    ? '—'
    : storage.hourlyRollupComplete === false
      ? `待处理 ${count(storage.hourlyRollupDirtyBuckets)} 个桶`
      : storage.hourlyRollupMaxSeq == null ? '已完成' : `完成至 #${count(storage.hourlyRollupMaxSeq)}`;
  return (
    <details className="runtime-v2-storage-technical-details">
      <summary>
        <span>技术详情</span>
        <span className="runtime-v2-storage-technical-summary-meta">版本、文件与聚合状态</span>
      </summary>
      <dl className="runtime-v2-storage-technical-grid">
        <div><dt>Schema / 投影</dt><dd>v{storage.schemaVersion ?? '—'} / v{storage.projectionVersion ?? '—'}</dd></div>
        <div><dt>数据库文件 / 有效占用</dt><dd>{databaseBytes == null ? '—' : formatBytes(databaseBytes)} / {effectiveBytes == null ? '—' : formatBytes(effectiveBytes)}</dd></div>
        <div><dt>WAL / 空闲页</dt><dd>{walBytes == null ? '—' : formatBytes(walBytes)} / {storage.freelistBytes == null ? '—' : formatBytes(storage.freelistBytes)}</dd></div>
        <div><dt>用户删除</dt><dd>{count(storage.userDeletedEvents)} 条事件 / {count(storage.userDeletedRequests)} 个请求</dd></div>
        <div><dt>小时聚合</dt><dd>{hourlyRollup}</dd></div>
        <div><dt>缺失索引</dt><dd>{missingIndexes == null ? '—' : missingIndexes.length ? missingIndexes.join(', ') : '无'}</dd></div>
      </dl>
    </details>
  );
}

function StorageManagementCard({ storage, retention, loading, error, onOpen, onRetry }) {
  const pending = storage?.pendingEvents == null ? null : safeNumber(storage.pendingEvents);
  const pendingBytes = storage?.pendingBytes == null ? null : safeNumber(storage.pendingBytes);
  const databaseBytes = storage?.databaseBytes ?? storage?.dbBytes;
  const retainedEvents = storage?.retainedEvents ?? storage?.eventCount;
  const maxAgeDays = retention?.maxAgeDays ?? storage?.retention?.maxAgeDays;
  const storageLimitBytes = retention?.storageLimitBytes ?? storage?.retention?.storageLimitBytes;
  // Retention is enforced against liveBytes. Older daemons may not expose it,
  // so keep the previous database-size field as a display fallback.
  const effectiveBytes = storage?.liveBytes ?? databaseBytes;
  const capacityExceeded = storageLimitBytes != null
    && effectiveBytes != null
    && effectiveBytes >= Number(storageLimitBytes);
  const ageExceeded = maxAgeDays != null
    && storage?.earliestTimestamp != null
    && ((Date.now() / 1000) - 978307200 - Number(storage.earliestTimestamp)) >= Number(maxAgeDays) * 86400;
  const automaticRetention = maxAgeDays != null || storageLimitBytes != null;
  const retentionLabel = automaticRetention ? '自动轮换' : '仅手动清理';
  const retentionDetail = maxAgeDays != null && storageLimitBytes != null
    ? `按 ${maxAgeDays} 天 + 容量上限`
    : maxAgeDays != null
      ? `最长保存 ${maxAgeDays} 天`
      : storageLimitBytes != null
        ? '按容量上限'
        : '未设置上限';
  const hasPending = (pending != null && pending > 0) || (pendingBytes != null && pendingBytes > 0);
  const presentation = error
    ? { label: '读取失败', kind: 'warning', detail: '存储状态读取失败，保留上一份数据' }
    : capacityExceeded || ageExceeded
      ? { label: '需要关注', kind: 'warning', detail: capacityExceeded && ageExceeded ? '已达到时间和容量条件，旧数据会自动轮换' : capacityExceeded ? '已达到存储上限，旧数据会自动轮换' : '已达到保存时长，旧数据会自动轮换' }
      : storageStatePresentation(storage);
  return (
    <section className="runtime-v2-storage-management" aria-labelledby="runtime-v2-storage-management-heading" data-storage-management>
      <div className="runtime-v2-storage-management-icon" aria-hidden="true"><Icon name="database" size={22} /></div>
      <div className="runtime-v2-storage-management-copy">
        <div className="runtime-v2-storage-management-title">
          <h3 id="runtime-v2-storage-management-heading">运行统计存储</h3>
          <StatusBadge text={presentation.label} kind={presentation.kind} />
        </div>
        <p>{presentation.detail} · 查看当前占用与事件保留情况；需要调整保留策略或执行清理时打开存储设置。</p>
        {storage?.legacyRetentionDetected && (
          <div className="runtime-v2-retention-warning" role="alert">
            检测到旧版本的自动清理设置。它们已不再生效，也不会自动迁移；手动清理只删除事件，需使用“重置并新建数据库”移除旧字段。
          </div>
        )}
        <div className="runtime-v2-storage-management-metrics" aria-label="数据存储摘要">
          <span className={capacityExceeded ? 'is-warning' : ''}><strong>{effectiveBytes == null ? '—' : formatBytes(effectiveBytes)}</strong><small>有效占用</small></span>
          <span><strong>{retainedEvents == null ? '—' : numberWithComma(retainedEvents)}</strong><small>保留事件</small></span>
          <span><strong>{retentionLabel}</strong><small>{retentionDetail} · 在设置中调整</small></span>
          {hasPending && <span className="is-warning"><strong>{pending == null ? '—' : numberWithComma(pending)}</strong><small>待写入{pendingBytes == null ? '' : ` · ${formatBytes(pendingBytes)}`}</small></span>}
        </div>
        <StorageTechnicalDetails storage={storage} />
      </div>
      <div className="runtime-v2-storage-management-actions">
        {!storage && loading && <span className="runtime-v2-storage-management-loading" role="status">读取中…</span>}
        {onRetry && <button type="button" className="btn btn-ghost btn-sm" onClick={onRetry} disabled={loading}>{loading ? '读取中…' : '刷新'}</button>}
        <button
          type="button"
          className="btn btn-primary"
          onClick={onOpen}
        >
          <Icon name="edit" size={15} />
          存储设置…
        </button>
      </div>
      {error && <div className="runtime-v2-storage-management-error" role="alert">{runtimeV2ErrorMessage(error)}</div>}
    </section>
  );
}

function TrendPanel({ trend, legacyAnalytics, loading, error, onRetry }) {
  const [metric, setMetric] = useState('clientRequests');
  const totals = trend?.totals || legacyTrendTotals(legacyAnalytics);
  const legacyTokens = analyticsTokenUsage(legacyAnalytics);
  const tokens = mergeDefined(legacyTokens, totals?.tokens);
  const cost = totals?.cost || {};
  const completed = totals
    ? safeNumber(totals.clientSuccesses) + safeNumber(totals.clientFailures) + safeNumber(totals.clientCancelled)
    : 0;
  const duration = totals?.durationMS || {};
  const ttfb = totals?.ttfbMS || {};
  const thresholds = trend?.thresholds || {};
  const failoverDetail = totals && totals.failoverRecoveredRequests != null
    ? `恢复 ${numberWithComma(totals.failoverRecoveredRequests)} · 最终失败 ${numberWithComma(totals.failoverTerminalRequests)}`
    : '发生入口切换的请求';
  return (
    <section className="runtime-v2-panel" aria-labelledby="runtime-v2-trends-heading">
      <div className="runtime-v2-panel-header">
        <div><h2 id="runtime-v2-trends-heading">趋势</h2><p>使用上方范围筛选查看请求、响应速度、Token、缓存、故障转移与成本。</p></div>
      </div>
      <PanelMessage error={error} onRetry={onRetry} />
      {loading && !totals ? <LoadingLine text="正在读取趋势快照…" /> : totals ? (
        <>
          <div className="runtime-v2-metrics-grid">
            <Metric label="客户端请求" value={numberWithComma(totals.clientRequests)} detail={`完成 ${numberWithComma(completed)} · 成功率 ${percentFromCount(totals.clientSuccesses, completed)}`} accent="var(--primary)" />
            <Metric label="平均完成耗时" value={formatDuration(latencyAverage(duration))} detail="请求完成平均耗时" accent="var(--accent-indigo)" />
            <Metric label="平均首字节" value={formatDuration(latencyAverage(ttfb))} detail="首次响应平均耗时" accent="var(--accent-cyan)" />
            <Metric label="故障转移" value={numberWithComma(totals.failovers)} detail={failoverDetail} tone="runtime-v2-warning" accent="var(--status-warning)" />
            <Metric label="上游尝试" value={numberWithComma(totals.upstreamAttempts)} detail={`成功 ${numberWithComma(totals.upstreamSuccesses)} · 失败 ${numberWithComma(totals.upstreamFailures)}`} accent="var(--accent-indigo)" />
            <Metric label="慢请求" value={optionalNumberWithComma(thresholdExceeded(duration, thresholds.durationMS?.[0]))} detail={thresholds.durationMS?.[0] ? `完成耗时 > ${formatDuration(thresholds.durationMS[0])}` : '未提供阈值'} accent="var(--status-warning)" />
            <Metric label="严重慢请求" value={optionalNumberWithComma(thresholdExceeded(duration, thresholds.durationMS?.[1]))} detail={thresholds.durationMS?.[1] ? `完成耗时 > ${formatDuration(thresholds.durationMS[1])}` : '未提供阈值'} tone="runtime-v2-warning" accent="var(--status-danger)" />
            <Metric label="输入 Token" value={optionalNumberWithComma(tokenValue(tokens, 'inputTokens'))} detail="输入用量" accent="var(--primary)" />
            <Metric label="缓存读取" value={optionalNumberWithComma(tokenValue(tokens, 'cacheReadInputTokens'))} detail={`命中率 ${percent(runtimeCacheRate(tokens, 'token'))}`} accent="var(--accent-cyan)" />
            <Metric label="估算成本" value={formatMoney(cost.estimatedCostMicros, cost.currency || 'USD')} detail="按已配置价格估算" accent="var(--status-good)" />
          </div>
          {trend && <label className="runtime-v2-trend-metric"><span>展示指标</span><select className="form-select" value={metric} onChange={(event) => setMetric(event.target.value)}>{TREND_METRICS.map(([value, label, unit]) => <option key={value} value={value}>{label}{unit === 'percent' ? '（比例）' : ''}</option>)}</select></label>}
          {trend ? <TrendChart points={trend.points || []} metric={metric} /> : <div className="runtime-v2-empty">当前 daemon 未提供 v3 时间桶，以上为兼容统计摘要；升级后可查看趋势。</div>}
        </>
      ) : <div className="runtime-v2-empty">暂无趋势数据</div>}
    </section>
  );
}

// The SSE/status layer can update the parent workspace several times while a
// user is looking at another board.  TrendPanel owns only trend-derived data;
// memoizing it keeps those unrelated updates from rebuilding the SVG/table.
const MemoizedTrendPanel = React.memo(TrendPanel);

function OverviewPanel({
  trend,
  legacyAnalytics,
  loading,
  error,
  onRetry,
  storage,
  summaryStorage,
  storageRetention,
  storageLoading,
  storageError,
  storageSettingsOpen,
  onOpenStorage,
  onCloseStorage,
  onRetryStorage,
  onUpdateRetention,
  onManualCleanup,
  onRecreateDatabase,
  usagePages,
  usageLoading,
  usageErrors,
  usageCallbacks,
  clientKindFacets,
  addToast,
  onSessionExport,
  onSessionDelete,
  sessionActionID,
  onProjectStickyClear,
  stickyActionKey,
  selectedProject,
  selectedSession,
  onProjectSelect,
  onSessionSelect,
  onClearProject,
  onClearSession,
}) {
  const effectiveStorage = storage
    ? {
        ...storage,
        ...(summaryStorage || {}),
        // The probe owns durable byte/event counts; the live summary owns the
        // pending queue and writer health. Keep both instead of letting the
        // probe's nullable compatibility fields turn into fake zeros.
        pendingEvents: summaryStorage?.pendingEvents ?? storage.pendingEvents,
        pendingBytes: summaryStorage?.pendingBytes ?? storage.pendingBytes,
        state: summaryStorage?.state ?? storage.state,
        lastError: summaryStorage?.lastError ?? storage.lastError,
      }
    : summaryStorage;
  const totals = trend?.totals || legacyTrendTotals(legacyAnalytics);
  const completed = totals ? safeNumber(totals.clientSuccesses) + safeNumber(totals.clientFailures) + safeNumber(totals.clientCancelled) : 0;
  const failoverRequests = totals?.failovers ?? null;
  const duration = totals?.durationMS || {};
  const ttfb = totals?.ttfbMS || {};
  const tokens = mergeDefined(analyticsTokenUsage(legacyAnalytics), totals?.tokens);
  return (
    <section className="runtime-v2-panel runtime-v2-overview-panel" aria-labelledby="runtime-v3-overview-heading">
      <div className="runtime-v2-panel-header">
        <div>
          <h2 id="runtime-v3-overview-heading">使用概览</h2>
          <p>当前范围的入口、项目、会话与模型用量集中展示；点击项目或会话行，可查看对应模型明细。</p>
        </div>
        <span className="runtime-v2-panel-kicker">入口 · 项目 · 会话</span>
      </div>
      <PanelMessage error={error} onRetry={onRetry} />
      <StorageManagementCard
        storage={effectiveStorage}
        retention={storageRetention}
        loading={storageLoading}
        error={storageError}
        onOpen={onOpenStorage}
        onRetry={onRetryStorage}
      />
      {storageSettingsOpen && <StorageSettingsDialog
        storage={effectiveStorage}
        retention={storageRetention}
        loading={storageLoading}
        error={storageError}
        onClose={onCloseStorage}
        onRetry={onRetryStorage}
        onUpdateRetention={onUpdateRetention}
        onManualCleanup={onManualCleanup}
        onRecreateDatabase={onRecreateDatabase}
      />}
      {loading && !totals && <LoadingLine text="正在读取概览快照…" />}
      {totals && (
        <>
          <div className="runtime-v2-overview-block runtime-v2-overview-request-block">
            <div className="runtime-v2-overview-block-heading">
              <h3>核心指标</h3>
              <span>当前范围</span>
            </div>
            <div className="runtime-v2-overview-health-grid">
              <Metric label="请求" value={numberWithComma(totals.clientRequests)} detail={`已完成 ${numberWithComma(completed)} · 待定 ${numberWithComma(totals.clientUnknownResults)}`} accent="var(--primary)" />
              <Metric label="成功率" value={percentFromCount(totals.clientSuccesses, completed)} detail={`成功 ${numberWithComma(totals.clientSuccesses)} · 失败 ${numberWithComma(totals.clientFailures)} · 取消 ${numberWithComma(totals.clientCancelled)}`} accent="var(--status-good)" />
              <Metric label="平均首字节" value={formatDuration(latencyAverage(ttfb))} detail="首次响应平均耗时" accent="var(--accent-cyan)" />
              <Metric label="平均完成耗时" value={formatDuration(latencyAverage(duration))} detail="请求完成平均耗时" accent="var(--accent-indigo)" />
              <Metric label="故障转移" value={optionalNumberWithComma(failoverRequests)} detail="发生入口切换的请求" tone="runtime-v2-warning" accent="var(--status-warning)" />
            </div>
          </div>
          <div className="runtime-v2-overview-block runtime-v2-overview-token-section">
            <div className="runtime-v2-overview-block-heading">
              <h3>Token 与缓存</h3>
              <span>缓存读取下方显示缓存读取命中率</span>
            </div>
            <TokenCoreGrid tokens={tokens} compact />
          </div>
        </>
      )}
      {!totals && !loading && <div className="runtime-v2-empty">暂无概览数据</div>}
      <div className="runtime-v2-overview-block runtime-v2-overview-usage-block">
        <div className="runtime-v2-overview-block-heading">
          <h3>使用明细</h3>
          <span>项目 / 会话行可钻取模型用量</span>
        </div>
        <div className="runtime-v2-dimension-stack">
          {USAGE_DIMENSION_META.map(([kind, title]) => (
            <DimensionBlock
              key={kind}
              kind={kind}
              title={title}
              page={usagePages?.[kind]}
              loading={usageLoading?.[kind]}
              error={usageErrors?.[kind]}
              onRetry={() => usageCallbacks?.retry?.(kind)}
              onPageChange={(page) => usageCallbacks?.page?.(kind, page)}
              onPageSizeChange={(size) => usageCallbacks?.pageSize?.(kind, size)}
              onSearchSubmit={(value) => usageCallbacks?.search?.(kind, value)}
              onSortChange={(key, order) => usageCallbacks?.sort?.(kind, key, order)}
              clientKindFacets={clientKindFacets}
              addToast={addToast}
              onSessionExport={onSessionExport}
              onSessionDelete={onSessionDelete}
              sessionActionID={sessionActionID}
              onProjectStickyClear={onProjectStickyClear}
              stickyActionKey={stickyActionKey}
              activeRowKey={kind === 'project' ? selectedProject?.key : kind === 'session' ? selectedSession?.key : undefined}
              onRowClick={kind === 'project' ? onProjectSelect : kind === 'session' ? onSessionSelect : undefined}
              selectedProjectLabel={(kind === 'session' || kind === 'model') && !selectedSession ? selectedProject?.name : undefined}
              selectedSessionLabel={kind === 'model' ? selectedSession?.name : undefined}
              onClearProject={kind === 'session' || (kind === 'model' && !selectedSession) ? onClearProject : undefined}
              onClearSession={kind === 'model' && selectedSession ? onClearSession : undefined}
            />
          ))}
        </div>
      </div>
    </section>
  );
}

function StorageSettingsDialog({ storage, retention, loading, error, onClose, onRetry, onUpdateRetention, onManualCleanup, onRecreateDatabase }) {
  const dialogRef = useRef(null);

  useEffect(() => {
    if (typeof document === 'undefined') return undefined;
    const previousFocus = document.activeElement;
    const handleKeyDown = (event) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose?.();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = [...(dialogRef.current?.querySelectorAll(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
      ) || [])].filter((element) => element.offsetParent !== null);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    const focus = () => dialogRef.current?.querySelector('input, button')?.focus();
    const frame = typeof window !== 'undefined' && typeof window.requestAnimationFrame === 'function'
      ? window.requestAnimationFrame(focus)
      : globalThis.setTimeout(focus, 0);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      if (typeof window !== 'undefined' && typeof window.cancelAnimationFrame === 'function') window.cancelAnimationFrame(frame);
      else globalThis.clearTimeout(frame);
      if (previousFocus && typeof previousFocus.focus === 'function') previousFocus.focus({ preventScroll: true });
    };
  }, [onClose]);

  return (
    <div
      className="modal-overlay runtime-v2-storage-modal"
      onClick={(event) => { if (event.target === event.currentTarget) onClose?.(); }}
    >
      <section
        ref={dialogRef}
        className="modal-dialog runtime-v2-storage-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="runtime-v2-storage-dialog-heading"
        aria-describedby="runtime-v2-storage-dialog-description"
      >
        <header className="modal-header runtime-v2-storage-dialog-header">
          <div>
            <h2 id="runtime-v2-storage-dialog-heading">运行统计存储设置</h2>
            <p id="runtime-v2-storage-dialog-description">调整存储上限，或执行需要明确确认的统计维护操作。</p>
          </div>
          <button type="button" className="btn-icon" onClick={onClose} aria-label="关闭存储设置">
            <Icon name="close" size={16} />
          </button>
        </header>
        <div className="modal-body runtime-v2-storage-dialog-body">
          <StoragePanel
            storage={storage}
            retention={retention}
            loading={loading}
            error={error}
            onRetry={onRetry}
            onUpdateRetention={onUpdateRetention}
            onManualCleanup={onManualCleanup}
            onRecreateDatabase={onRecreateDatabase}
          />
        </div>
        <footer className="modal-footer runtime-v2-storage-dialog-footer">
          <button type="button" className="btn btn-secondary" onClick={onClose}>完成</button>
        </footer>
      </section>
    </div>
  );
}

function StoragePanel({ storage, retention, loading, error, onRetry, onUpdateRetention, onManualCleanup, onRecreateDatabase }) {
  const activeRetention = retention || storage?.retention;
  const maxAgeDays = activeRetention?.maxAgeDays;
  const storageLimitBytes = activeRetention?.storageLimitBytes;
  const databaseBytes = storage?.databaseBytes ?? storage?.dbBytes;
  // Keep the UI's quota indicator aligned with the daemon's live-byte limit.
  const effectiveBytes = storage?.liveBytes ?? databaseBytes;
  const retainedEvents = storage?.retainedEvents ?? storage?.eventCount;
  const pendingEvents = storage?.pendingEvents == null ? null : safeNumber(storage.pendingEvents);
  const pendingBytes = storage?.pendingBytes == null ? null : safeNumber(storage.pendingBytes);
  const usedPercent = storageLimitBytes != null && effectiveBytes != null
    ? Math.min(100, Math.round((Number(effectiveBytes) / Math.max(1, Number(storageLimitBytes))) * 100))
    : null;
  const [storageLimitMB, setStorageLimitMB] = useState('');
  const [maxAgeDaysInput, setMaxAgeDaysInput] = useState('');
  const [retentionDirty, setRetentionDirty] = useState(false);
  const [savingRetention, setSavingRetention] = useState(false);
  const [retentionError, setRetentionError] = useState('');
  useEffect(() => {
    if (!retentionDirty) {
      const bytes = activeRetention?.storageLimitBytes;
      setStorageLimitMB(bytes == null ? '' : String(Math.max(1, Math.round(Number(bytes) / 1048576))));
      setMaxAgeDaysInput(maxAgeDays == null ? '' : String(Math.max(1, Number(maxAgeDays))));
    }
  }, [activeRetention?.storageLimitBytes, maxAgeDays, retentionDirty]);
  const persistRetention = async (nextMaxAgeDays, nextStorageLimitBytes) => {
    if (!activeRetention || !onUpdateRetention) return;
    setRetentionError('');
    setSavingRetention(true);
    try {
      await onUpdateRetention({
        expectedRevision: activeRetention.revision,
        maxAgeDays: nextMaxAgeDays,
        storageLimitBytes: nextStorageLimitBytes,
      });
      setRetentionDirty(false);
    } catch (nextError) {
      setRetentionError(nextError?.message || '保留策略保存失败');
    } finally {
      setSavingRetention(false);
    }
  };
  const saveRetention = async (event) => {
    event?.preventDefault();
    if (!activeRetention || !onUpdateRetention) return;
    const ageValue = maxAgeDaysInput.trim();
    const nextMaxAgeDays = ageValue === '' ? null : Number(ageValue);
    if (nextMaxAgeDays != null && (!Number.isSafeInteger(nextMaxAgeDays) || nextMaxAgeDays < 1)) {
      setRetentionError('最大保存天数必须是至少 1 天的整数');
      return;
    }
    const sizeValue = storageLimitMB.trim();
    const megabytes = sizeValue === '' ? null : Number(sizeValue);
    if (megabytes != null && (!Number.isSafeInteger(megabytes) || megabytes < 1 || megabytes > Number.MAX_SAFE_INTEGER / 1048576)) {
      setRetentionError('存储上限必须是至少 1 MB 的整数');
      return;
    }
    await persistRetention(nextMaxAgeDays, megabytes == null ? null : megabytes * 1048576);
  };
  const disableStorageLimit = () => {
    setStorageLimitMB('');
    setRetentionDirty(false);
    persistRetention(maxAgeDays == null ? null : Number(maxAgeDays), null);
  };
  const disableMaxAge = () => {
    setMaxAgeDaysInput('');
    setRetentionDirty(false);
    persistRetention(null, storageLimitBytes == null ? null : Number(storageLimitBytes));
  };
  return (
    <div className="runtime-v2-storage-panel-content">
      <p className="runtime-v2-storage-dialog-copy">系统按滚动 24 小时的保存天数和 SQLite 有效占用上限自动轮换；任一条件先达到就触发。按请求组删除，进行中的请求组会完整保留。留空可分别关闭对应条件；轮换不会立即缩小数据库文件。</p>
      <PanelMessage error={error} onRetry={onRetry} />
      {loading && !storage ? <LoadingLine text="正在读取存储状态…" /> : storage ? (
        <>
          <div className="runtime-v2-storage-limit-editor">
            <div className="runtime-v2-storage-limit-heading">
              <strong>自动保留策略</strong>
              <span>{maxAgeDays == null && storageLimitBytes == null ? '仅手动清理' : `${maxAgeDays == null ? '' : `最长 ${maxAgeDays} 天`}${maxAgeDays != null && storageLimitBytes != null ? ' · ' : ''}${storageLimitBytes == null ? '' : `容量 ${formatBytes(storageLimitBytes)}`}`}</span>
            </div>
            <form className="runtime-v2-storage-limit-controls" onSubmit={saveRetention}>
              <label htmlFor="runtime-v2-storage-max-age-days">最大保存天数</label>
              <input id="runtime-v2-storage-max-age-days" className="form-input" inputMode="numeric" pattern="[0-9]*" value={maxAgeDaysInput} onChange={(event) => { setMaxAgeDaysInput(event.target.value.replace(/[^0-9]/g, '')); setRetentionDirty(true); }} placeholder="例如 30" />
              <span aria-hidden="true">天</span>
              <label htmlFor="runtime-v2-storage-limit-mb">容量上限</label>
              <input id="runtime-v2-storage-limit-mb" className="form-input" inputMode="numeric" pattern="[0-9]*" value={storageLimitMB} onChange={(event) => { setStorageLimitMB(event.target.value.replace(/[^0-9]/g, '')); setRetentionDirty(true); }} placeholder="例如 1024" />
              <span aria-hidden="true">MB</span>
              <button type="submit" className="btn btn-secondary" disabled={savingRetention || !activeRetention}>{savingRetention ? '保存中…' : '保存保留策略'}</button>
              <button type="button" className="btn btn-ghost" onClick={disableMaxAge} disabled={savingRetention || activeRetention?.maxAgeDays == null}>关闭时间上限</button>
              <button type="button" className="btn btn-ghost" onClick={disableStorageLimit} disabled={savingRetention || activeRetention?.storageLimitBytes == null}>关闭容量上限</button>
            </form>
            {retentionError && <div className="runtime-v2-storage-limit-error" role="alert">{retentionError}</div>}
          </div>
          <div className="runtime-v2-storage-dialog-context" aria-label="当前存储上下文">
            <span>保留事件 <strong>{retainedEvents == null ? '—' : numberWithComma(retainedEvents)}</strong></span>
            {pendingEvents != null && pendingEvents > 0 && <span className="is-warning">待写入 <strong>{numberWithComma(pendingEvents)}{pendingBytes == null ? '' : ` · ${formatBytes(pendingBytes)}`}</strong></span>}
          </div>
          <div className="runtime-v2-storage-grid">
            <div className="runtime-v2-subpanel runtime-v2-manual-cleanup-panel">
              <div className="runtime-v2-subpanel-heading"><strong>清理方式</strong><StatusBadge text={maxAgeDays == null && storageLimitBytes == null ? '仅手动清理' : '自动轮换'} kind="good" /></div>
              <p className="runtime-v2-storage-action-copy">{maxAgeDays == null && storageLimitBytes == null ? '未设置自动条件，记录会持续保留，直到你主动清理统计。' : `按${maxAgeDays == null ? '' : `时间（${maxAgeDays} 天）`}${maxAgeDays != null && storageLimitBytes != null ? '或' : ''}${storageLimitBytes == null ? '' : '容量'}自动轮换最旧的已完成请求；进行中的请求组会继续保留。`}</p>
              <div className="runtime-v2-form-actions">
                <span>按时间清理已完成统计，进行中的请求组会保留；诊断捕获不受影响。</span>
                <button type="button" className="btn btn-danger" onClick={onManualCleanup}>按时间清理统计</button>
              </div>
              <div className="runtime-v2-form-actions runtime-v2-recreate-database-actions">
                <span>旧版自动清理字段不会通过手动清理移除；需要彻底切换到当前数据库结构时使用此操作。</span>
                <button type="button" className="btn btn-danger" onClick={onRecreateDatabase}>重置并新建数据库</button>
              </div>
            </div>
            <div className="runtime-v2-subpanel runtime-v2-storage-note"><strong>数据说明</strong><p>运行记录会保留源事件和会话归属；自动轮换按请求组删除，任何含进行中事件的请求组都会跳过，避免请求链残缺。</p><p>完整诊断捕获独立保存，源请求正文、Headers 和流式 Chunk 不会重复计入统计。</p></div>
          </div>
        </>
      ) : <div className="runtime-v2-empty">暂无存储状态</div>}
    </div>
  );
}

function configuredEndpointModels(config) {
  const rows = [];
  (config?.endpoints || []).forEach((endpoint) => {
    const endpointID = String(endpoint?.id || '').trim();
    if (!endpointID) return;
    const models = new Set();
    const catalog = Array.isArray(endpoint?.catalog?.models) ? endpoint.catalog.models : [];
    catalog.forEach((model) => {
      const value = String(typeof model === 'object' ? (model.id || model.name || '') : model).trim();
      if (value) models.add(value);
    });
    (endpoint?.modelMappings || endpoint?.mappings || []).forEach((mapping) => {
      [mapping?.to, mapping?.upstreamModel, mapping?.from, mapping?.clientPattern]
        .map((value) => String(value || '').trim())
        .filter(Boolean)
        .forEach((value) => models.add(value));
    });
    models.forEach((modelKey) => rows.push({ endpointID, endpointName: endpoint.name || endpointID, modelKey }));
  });
  return rows.sort((left, right) => (left.endpointName + '/' + left.modelKey).localeCompare(right.endpointName + '/' + right.modelKey, 'zh-CN'));
}

export function PricingPanel({ pricing, config, endpointID = '', loading, error, onRetry, onSave }) {
  const [currency, setCurrency] = useState('USD');
  const [prices, setPrices] = useState([]);
  const [validationError, setValidationError] = useState(null);
  const configuredModels = useMemo(() => configuredEndpointModels(config)
    .filter((model) => !endpointID || model.endpointID === endpointID), [config, endpointID]);
  const visiblePrices = useMemo(() => prices
    .map((row, index) => ({ row, index }))
    .filter(({ row }) => !endpointID || !row.endpointID || row.endpointID === endpointID), [endpointID, prices]);
  const endpointName = endpointID
    ? ((config?.endpoints || []).find((endpoint) => endpoint.id === endpointID)?.name || endpointID)
    : '';
  useEffect(() => {
    if (!pricing) return;
    setCurrency(pricing.currency || 'USD');
    setPrices((pricing.prices || []).map((price) => ({
      ...price,
      endpointID: price.endpointID || '',
      modelKey: price.modelKey || '',
      effectiveFrom: runtimePriceDateValue(price.effectiveFrom),
      effectiveTo: runtimePriceDateValue(price.effectiveTo),
      input: runtimePriceInput(price.inputPerMillionMicros),
      output: runtimePriceInput(price.outputPerMillionMicros),
      cacheRead: runtimePriceInput(price.cacheReadPerMillionMicros),
      cacheCreation: runtimePriceInput(price.cacheCreationPerMillionMicros),
    })));
  }, [pricing]);
  const update = (index, key, value) => {
    setValidationError(null);
    setPrices((current) => current.map((row, rowIndex) => rowIndex === index ? { ...row, [key]: value } : row));
  };
  const add = () => {
    setValidationError(null);
    setPrices((current) => [...current, { id: 'new-' + Date.now(), endpointID: endpointID || '', modelKey: '', effectiveFrom: '', effectiveTo: '', input: '', output: '', cacheRead: '', cacheCreation: '' }]);
  };
  const addConfiguredModel = (model) => {
    setValidationError(null);
    setPrices((current) => [...current, {
      id: 'new-' + Date.now() + '-' + model.endpointID + '-' + model.modelKey,
      endpointID: model.endpointID,
      modelKey: model.modelKey,
      effectiveFrom: new Date().toISOString().slice(0, 16),
      effectiveTo: '',
      input: '',
      output: '',
      cacheRead: '',
      cacheCreation: '',
    }]);
  };
  const remove = (index) => {
    setValidationError(null);
    setPrices((current) => current.filter((_, rowIndex) => rowIndex !== index));
  };
  const submit = () => {
    if (!pricing) return;
    const nextCurrency = currency.trim().toUpperCase();
    if (!/^[A-Z]{3}$/.test(nextCurrency)) {
      setValidationError(new Error('货币必须是 3 位 ISO 代码，例如 USD'));
      return;
    }
    const payload = prices.map((row) => ({
      modelKey: row.modelKey.trim(),
      endpointID: row.endpointID?.trim() || null,
      effectiveFrom: runtimeAppleTimestamp(row.effectiveFrom),
      effectiveTo: row.effectiveTo.trim() ? runtimeAppleTimestamp(row.effectiveTo) : null,
      inputPerMillionMicros: runtimePriceMicros(row.input),
      outputPerMillionMicros: runtimePriceMicros(row.output),
      cacheReadPerMillionMicros: runtimePriceMicros(row.cacheRead),
      cacheCreationPerMillionMicros: runtimePriceMicros(row.cacheCreation),
    }));
    const invalidRow = payload.findIndex((row, index) => {
      const source = prices[index];
      // Input/output rates are required for a usable price row. Cache rates
      // remain optional, but malformed values (including negative numbers)
      // must never be sent to the API as NaN.
      const requiredAmountMissing = row.inputPerMillionMicros == null
        || row.outputPerMillionMicros == null;
      const amountInvalid = requiredAmountMissing || [
        row.inputPerMillionMicros,
        row.outputPerMillionMicros,
        row.cacheReadPerMillionMicros,
        row.cacheCreationPerMillionMicros,
      ].some((value) => Number.isNaN(value));
      const dateInvalid = !source.effectiveFrom || !Number.isFinite(row.effectiveFrom)
        || (row.effectiveTo != null && (!Number.isFinite(row.effectiveTo) || row.effectiveTo < row.effectiveFrom));
      return !row.modelKey || amountInvalid || dateInvalid;
    });
    if (invalidRow >= 0) {
      setValidationError(new Error(`第 ${invalidRow + 1} 行的模型、日期或金额无效；终止日期不能早于起始日期`));
      return;
    }
    setValidationError(null);
    onSave({ expectedRevision: pricing.revision, currency: nextCurrency, prices: payload });
  };
  return (
    <div className="runtime-v2-subpanel runtime-v2-pricing-panel">
      <div className="runtime-v2-subpanel-heading"><div><strong>{endpointID ? `入口模型价格 · ${endpointName}` : '入口模型价格'}</strong><small>{endpointID ? '当前入口优先使用专属价格；没有专属价格时回退到全局价格。' : '优先按“入口 + 生效模型”匹配；没有入口专属价格时回退到全局模型价格。'}</small></div></div>
      <PanelMessage error={validationError} />
      <PanelMessage error={error} onRetry={onRetry} />
      {loading && !pricing ? <LoadingLine text="正在读取价格表…" /> : pricing ? (
        <>
          <label className="runtime-v2-currency-field"><span>货币</span><input className="form-input" value={currency} maxLength={3} onChange={(event) => setCurrency(event.target.value)} /></label>
          {configuredModels.length > 0 && <div className="runtime-v2-pricing-catalog" aria-label="已配置入口模型"><div><strong>当前配置中的模型</strong><span>点击模型生成入口专属价格行</span></div><div className="runtime-v2-pricing-catalog-list">{configuredModels.map((model) => {
            const exists = prices.some((row) => row.endpointID === model.endpointID && row.modelKey === model.modelKey);
            return <button key={model.endpointID + '/' + model.modelKey} type="button" className="runtime-v2-pricing-model-chip" disabled={exists} onClick={() => addConfiguredModel(model)}><span>{model.endpointName}</span><strong>{model.modelKey}</strong><em>{exists ? '已添加' : '添加价格'}</em></button>;
          })}</div></div>}
          <div className="table-container runtime-v2-pricing-table-wrap"><table className="data-table runtime-v2-pricing-table"><thead><tr><th>入口</th><th>模型键</th><th>生效起点</th><th>生效终点</th><th>输入 / 1M</th><th>输出 / 1M</th><th>缓存读 / 1M</th><th>缓存写 / 1M</th><th>操作</th></tr></thead><tbody>{visiblePrices.map(({ row, index }) => <tr key={row.id || `price-${index}`}>
            <td>{endpointID && row.endpointID === endpointID ? <span className="runtime-v2-pricing-endpoint-label" title={endpointName}>{endpointName}</span> : <select className="form-select" value={row.endpointID || ''} onChange={(event) => update(index, 'endpointID', event.target.value)} aria-label={`第 ${index + 1} 行入口`}><option value="">全局回退</option>{(config?.endpoints || []).map((endpoint) => <option key={endpoint.id} value={endpoint.id}>{endpoint.name || endpoint.id}</option>)}</select>}</td>
            <td><input className="form-input" list="runtime-pricing-models" value={row.modelKey} onChange={(event) => update(index, 'modelKey', event.target.value)} placeholder="exact-model" /></td>
            <td><input className="form-input" type="datetime-local" value={row.effectiveFrom} onChange={(event) => update(index, 'effectiveFrom', event.target.value)} /></td>
            <td><input className="form-input" type="datetime-local" value={row.effectiveTo} onChange={(event) => update(index, 'effectiveTo', event.target.value)} /></td>
            <td><input className="form-input" inputMode="decimal" value={row.input} onChange={(event) => update(index, 'input', event.target.value)} placeholder="USD" required aria-label={`第 ${index + 1} 行输入价格`} /></td>
            <td><input className="form-input" inputMode="decimal" value={row.output} onChange={(event) => update(index, 'output', event.target.value)} placeholder="USD" required aria-label={`第 ${index + 1} 行输出价格`} /></td>
            <td><input className="form-input" inputMode="decimal" value={row.cacheRead} onChange={(event) => update(index, 'cacheRead', event.target.value)} placeholder="可空" /></td>
            <td><input className="form-input" inputMode="decimal" value={row.cacheCreation} onChange={(event) => update(index, 'cacheCreation', event.target.value)} placeholder="可空" /></td>
            <td><button type="button" className="btn btn-danger btn-sm" onClick={() => remove(index)}>移除</button></td>
          </tr>)}</tbody></table></div>
          <datalist id="runtime-pricing-models">{[...new Set(configuredModels.map((model) => model.modelKey))].map((modelKey) => <option key={modelKey} value={modelKey} />)}</datalist>
          <div className="runtime-v2-form-actions"><span>金额按每百万 Token 填写。</span><div className="page-actions"><button type="button" className="btn btn-secondary" onClick={add}>添加价格</button><button type="button" className="btn btn-primary" onClick={submit}>替换价格表</button></div></div>
        </>
      ) : <div className="runtime-v2-empty">暂无价格表</div>}
    </div>
  );
}

function TokenPanel({ trend, legacyAnalytics, loading, error, onRetry }) {
  const totals = trend?.totals || legacyTrendTotals(legacyAnalytics);
  const tokens = mergeDefined(analyticsTokenUsage(legacyAnalytics), totals?.tokens);
  return (
    <section className="runtime-v2-panel" aria-labelledby="runtime-v3-tokens-heading">
      <div className="runtime-v2-panel-header"><div><h2 id="runtime-v3-tokens-heading">成本</h2><p>按入口、项目和会话查看成本；缓存读取命中率只在缓存读取旁显示。</p></div></div>
      <PanelMessage error={error} onRetry={onRetry} />
      {loading && !totals ? <LoadingLine text="正在读取 Token 汇总…" /> : totals ? (
        <>
          <TokenCoreGrid tokens={tokens} />
        </>
      ) : <div className="runtime-v2-empty">暂无 Token 数据</div>}
    </section>
  );
}

function CostPanel({ pricing, trend, legacyAnalytics, costPages, costLoading, costErrors, costCallbacks, clientKindFacets, addToast, selectedProject, selectedSession, onProjectSelect, onSessionSelect, onClearProject, onClearSession }) {
  const totals = trend?.totals || legacyTrendTotals(legacyAnalytics);
  const cost = totals?.cost || {};
  return (
    <section className="runtime-v2-panel runtime-v2-cost-section" aria-labelledby="runtime-v3-cost-heading">
      <div className="runtime-v2-panel-header"><div><h2 id="runtime-v3-cost-heading">成本</h2><p>按入口、项目、会话和模型查看估算成本。点击项目或会话行，可继续查看对应模型成本。</p></div></div>
      <div className="runtime-v2-metrics-grid runtime-v2-cost-summary">
        <Metric label="估算成本" value={safeNumber(cost?.pricedRequests) > 0 ? formatMoney(cost.estimatedCostMicros, cost.currency || pricing?.currency || 'USD') : '—'} detail={safeNumber(cost?.pricedRequests) > 0 ? `基于 ${numberWithComma(cost.pricedRequests)} 个已计价请求` : '暂无可计价请求'} accent="var(--status-good)" />
        <Metric label="请求" value={optionalNumberWithComma(totals?.clientRequests)} detail="当前筛选范围" accent="var(--primary)" />
        <Metric label="暂无法计价" value={optionalNumberWithComma(safeNumber(cost?.unpricedRequests) + safeNumber(cost?.unknownAccountingRequests))} detail="缺少价格或完整用量" accent="var(--status-warning)" />
      </div>
      <div className="runtime-v2-cost-dimension-stack">
        {USAGE_DIMENSION_META.map(([kind, title]) => (
          <DimensionBlock
            key={kind}
            kind={kind}
            title={title.replace('使用情况', '成本')}
            page={costPages?.[kind]}
            loading={costLoading?.[kind]}
            error={costErrors?.[kind]}
            onRetry={() => costCallbacks?.retry?.(kind)}
            onPageChange={(page) => costCallbacks?.page?.(kind, page)}
            onPageSizeChange={(size) => costCallbacks?.pageSize?.(kind, size)}
            onSearchSubmit={(value) => costCallbacks?.search?.(kind, value)}
            onSortChange={(key, order) => costCallbacks?.sort?.(kind, key, order)}
            clientKindFacets={clientKindFacets}
            addToast={addToast}
            mode="cost"
            activeRowKey={kind === 'project' ? selectedProject?.key : kind === 'session' ? selectedSession?.key : undefined}
            onRowClick={kind === 'project' ? onProjectSelect : kind === 'session' ? onSessionSelect : undefined}
            selectedProjectLabel={(kind === 'session' || kind === 'model') && !selectedSession ? selectedProject?.name : undefined}
            selectedSessionLabel={kind === 'model' ? selectedSession?.name : undefined}
            onClearProject={kind === 'session' || (kind === 'model' && !selectedSession) ? onClearProject : undefined}
            onClearSession={kind === 'model' && selectedSession ? onClearSession : undefined}
          />
        ))}
      </div>
    </section>
  );
}

function runtimePriceDateValue(value) {
  if (value == null || value === '') return '';
  return runtimeDateTimeLocal(value, normalizeTimestampMS);
}

function ExportPanel({ addToast, filters = {} }) {
  const [scope, setScope] = useState('events');
  const [format, setFormat] = useState('jsonl');
  const [privacy, setPrivacy] = useState('stored');
  const [confirmStored, setConfirmStored] = useState(false);
  const [estimate, setEstimate] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);
  const filterKey = `${filters.clientKind || ''}\u0000${filters.endpointID || ''}\u0000${filters.projectID || ''}\u0000${filters.project || ''}\u0000${filters.sessionID || ''}`;
  useEffect(() => {
    setEstimate(null);
  }, [filterKey]);
  const loadEstimate = async () => {
    if (privacy === 'stored' && !confirmStored) {
      setError(new Error('选择 stored 前必须明确确认 SQLite 已存字段范围'));
      return;
    }
    setLoading(true); setError(null);
    try {
      const value = await api.estimateRuntimeExport({ scope, format, privacy, confirmStored, filters });
      setEstimate(value);
    } catch (nextError) { setError(nextError); }
    finally { setLoading(false); }
  };
  const download = async () => {
    if (!estimate) return loadEstimate();
    if (privacy === 'stored' && !confirmStored) return;
    setLoading(true); setError(null);
    try {
      await api.downloadRuntimeExport({ scope, format, privacy, confirmStored, snapshotSeq: estimate.snapshotSeq, historyGeneration: estimate.historyGeneration, filters });
      addToast?.('已按快照开始流式下载', 'success');
    } catch (nextError) { setError(nextError); }
    finally { setLoading(false); }
  };
  return (
    <div className="runtime-v2-export-content">
      <PanelMessage error={error} onRetry={loadEstimate} />
      <div className="runtime-v2-export-form">
        <label><span>范围</span><select className="form-select" value={scope} onChange={(event) => { setScope(event.target.value); setEstimate(null); }}><option value="events">事件</option><option value="projects">项目</option><option value="sessions">会话</option></select></label>
        <label><span>格式</span><select className="form-select" value={format} onChange={(event) => { setFormat(event.target.value); setEstimate(null); }}><option value="jsonl">JSONL（逐行）</option><option value="csv">CSV</option></select></label>
        <label><span>数据范围</span><select className="form-select" value={privacy} onChange={(event) => { setPrivacy(event.target.value); setConfirmStored(false); setEstimate(null); }}><option value="stored">源数据（SQLite 已存字段，默认）</option><option value="redacted">脱敏副本</option></select></label>
        <div className="runtime-v2-export-actions"><button type="button" className="btn btn-secondary" onClick={loadEstimate} disabled={loading}>{loading ? '估算中…' : '估算导出'}</button><button type="button" className="btn btn-primary" onClick={download} disabled={loading || !estimate}>{loading ? '处理中…' : '开始下载'}</button></div>
      </div>
      {privacy === 'stored' && <label className="runtime-v2-confirm-row"><input type="checkbox" checked={confirmStored} onChange={(event) => { setConfirmStored(event.target.checked); setEstimate(null); }} /> <span>我确认源数据导出包含 SQLite 已保存的会话/归属字段；诊断捕获正文、Headers、Cookie、Authorization 和 API Key 仍只在诊断捕获导出中提供。</span></label>}
      {estimate && <div className="runtime-v2-export-estimate" role="status"><strong>{numberWithComma(estimate.rowCount)} 行 · 约 {formatBytes(estimate.estimatedBytes)}</strong></div>}
    </div>
  );
}

export function AnalyticsWorkspace({ onSelectEvent, addToast, onManualCleanup, onRecreateDatabase, openExportSignal = 0, analyticsFilters = {}, analyticsRange = 'today', onAnalyticsRangeChange, facets = null, analyticsRefreshSignal = 0, onProjectSelectionChange, onSessionSelectionChange, projectSelectionResetSignal = 0, legacyAnalytics = null, summaryStorage = null, config = null }) {
  const [section, setSection] = useState('overview');
  const sectionTabRefs = useRef([]);
  const [range, setRange] = useState(analyticsRange || 'today');
  const [trend, setTrend] = useState(null);
  const [trendLoading, setTrendLoading] = useState(false);
  const [trendError, setTrendError] = useState(null);
  const [errors, setErrors] = useState(null);
  const [errorsLoading, setErrorsLoading] = useState(false);
  const [errorsError, setErrorsError] = useState(null);
  const [diagnostics, setDiagnostics] = useState(null);
  const [diagnosticsLoading, setDiagnosticsLoading] = useState(false);
  const [diagnosticsError, setDiagnosticsError] = useState(null);
  // Keep the user-facing comparison lists independent. A single shared
  // dimension slot made it impossible to see入口、项目、会话和模型 together and
  // caused one table's pagination/search to overwrite another's state.
  const [dimensions, setDimensions] = useState({ endpoint: null, project: null, session: null, model: null });
  const [dimensionLoading, setDimensionLoading] = useState({ endpoint: false, project: false, session: false, model: false });
  const [dimensionError, setDimensionError] = useState({ endpoint: null, project: null, session: null, model: null });
  const [dimensionSearch, setDimensionSearch] = useState({ endpoint: '', project: '', session: '', model: '' });
  // A project row is a local drill-down, not a replacement for the global
  // project filter. It only narrows the session projection below the tables.
  const [selectedProject, setSelectedProject] = useState(null);
  const [selectedSession, setSelectedSession] = useState(null);
  const previousSelectedProjectRef = useRef(null);
  const previousSelectedSessionRef = useRef(null);
  const skipNextClearedProjectReloadRef = useRef(false);
  const [storage, setStorage] = useState(null);
  const [retention, setRetention] = useState(null);
  const [pricing, setPricing] = useState(null);
  const [storageLoading, setStorageLoading] = useState(false);
  const [storageError, setStorageError] = useState(null);
  const [showExport, setShowExport] = useState(false);
  const [showStorageSettings, setShowStorageSettings] = useState(false);
  const [sessionActionID, setSessionActionID] = useState('');
  const [stickyActionKey, setStickyActionKey] = useState('');
  const trendRequestRef = useRef({ id: 0, controller: null });
  const errorsRequestRef = useRef({ id: 0, controller: null });
  const diagnosticsRequestRef = useRef({ id: 0, controller: null });
  const dimensionRequestRefs = useRef({
    endpoint: { current: { id: 0, controller: null } },
    project: { current: { id: 0, controller: null } },
    session: { current: { id: 0, controller: null } },
    model: { current: { id: 0, controller: null } },
  });
  const storageRequestRef = useRef({ id: 0, controller: null });
  // Keep the selected analytics page size in React state as well as
  // localStorage.  Reading it only once with useMemo made a size selected on
  // one board invisible to a board entered later in the same session.
  const [analyticsPageSize, setAnalyticsPageSize] = useState(() => readRuntimeAnalyticsPageSize());
  const pageSize = analyticsPageSize;
  const localTodayFrom = useMemo(() => {
    if (range !== 'today') return null;
    return runtimeTodayBounds().from;
  }, [range]);
  const activeFilters = useMemo(() => ({
    clientKind: String(analyticsFilters?.clientKind || '').trim(),
    endpointID: String(analyticsFilters?.endpointID || '').trim(),
    project: String(analyticsFilters?.project || '').trim(),
    projectID: String(analyticsFilters?.projectID || '').trim(),
    sessionID: String(analyticsFilters?.sessionID || '').trim(),
    model: String(analyticsFilters?.model || '').trim(),
    requestPurpose: String(analyticsFilters?.requestPurpose || '').trim(),
    outcome: String(analyticsFilters?.outcome || '').trim(),
    failureKind: String(analyticsFilters?.failureKind || '').trim(),
    failurePhase: String(analyticsFilters?.failurePhase || '').trim(),
    ...(localTodayFrom == null ? {} : { from: localTodayFrom }),
  }), [analyticsFilters?.clientKind, analyticsFilters?.endpointID, analyticsFilters?.project, analyticsFilters?.projectID, analyticsFilters?.sessionID, analyticsFilters?.model, analyticsFilters?.requestPurpose, analyticsFilters?.outcome, analyticsFilters?.failureKind, analyticsFilters?.failurePhase, localTodayFrom]);
  const activeQueryKey = useMemo(() => JSON.stringify({ range, filters: activeFilters }), [range, activeFilters]);
  const lastAnalyticsRefreshSignal = useRef(0);
  const previousQueryKeyRef = useRef(activeQueryKey);

  useEffect(() => {
    const nextRange = String(analyticsRange || 'today');
    if (nextRange === range) return;
    setRange(nextRange);
  }, [analyticsRange]); // eslint-disable-line react-hooks/exhaustive-deps

  const loadTrends = useCallback(async (nextRange = range) => {
    const request = beginLatestRequest(trendRequestRef);
    setTrendLoading(true); setTrendError(null);
    try {
      const value = await api.getRuntimeTrends({ range: nextRange, filters: activeFilters }, { signal: request.controller.signal });
      if (request.isCurrent()) setTrend((previous) => (sameJSON(previous, value) ? previous : value));
    } catch (error) {
      if (!isAbortError(error) && request.isCurrent()) setTrendError(error);
    } finally {
      if (request.isCurrent()) setTrendLoading(false);
    }
  }, [activeFilters, range]);

  const loadErrors = useCallback(async (page = 1, nextPageSize = errors?.pageSize || pageSize, snapshot = errors) => {
    const request = beginLatestRequest(errorsRequestRef);
    setErrorsLoading(true); setErrorsError(null);
    try {
      const value = await api.getRuntimeErrors({ page, pageSize: nextPageSize, snapshotSeq: snapshot?.snapshotSeq, historyGeneration: snapshot?.historyGeneration, filters: activeFilters }, { signal: request.controller.signal });
      const normalized = normalizeRuntimePagedResult(value, { itemsKey: 'groups', page, pageSize: nextPageSize });
      if (!normalized) throw new Error('错误聚合响应不是 v2 分页形状');
      if (request.isCurrent()) setErrors((previous) => (sameJSON(previous, normalized) ? previous : normalized));
    } catch (error) {
      if (isAbortError(error) || !request.isCurrent()) return;
      setErrorsError(error);
      if (isRuntimeSnapshotError(error) && snapshot) {
        try {
          const value = await api.getRuntimeErrors({ page: 1, pageSize: nextPageSize, filters: activeFilters }, { signal: request.controller.signal });
          const normalized = normalizeRuntimePagedResult(value, { itemsKey: 'groups', page: 1, pageSize: nextPageSize });
          if (request.isCurrent()) setErrors((previous) => (sameJSON(previous, normalized) ? previous : normalized));
        } catch (retryError) {
          if (!isAbortError(retryError) && request.isCurrent()) setErrorsError(retryError);
        }
      }
    } finally {
      if (request.isCurrent()) setErrorsLoading(false);
    }
  }, [activeFilters, errors, pageSize]);

  const loadDiagnostics = useCallback(async () => {
    const request = beginLatestRequest(diagnosticsRequestRef);
    setDiagnosticsLoading(true); setDiagnosticsError(null);
    try {
      const value = await api.getRuntimeAnalytics(range, activeFilters, { signal: request.controller.signal });
      if (request.isCurrent()) setDiagnostics((previous) => (sameJSON(previous, value) ? previous : value));
    } catch (error) {
      if (!isAbortError(error) && request.isCurrent()) setDiagnosticsError(error);
    } finally {
      if (request.isCurrent()) setDiagnosticsLoading(false);
    }
  }, [activeFilters, range]);

  const loadDimension = useCallback(async (kind, page = 1, nextPageSize = dimensions[kind]?.pageSize || pageSize, search = dimensionSearch[kind] || '', snapshot = dimensions[kind], localSelection = undefined) => {
    if (!['endpoint', 'project', 'session', 'model'].includes(kind)) return;
    const request = beginLatestRequest(dimensionRequestRefs.current[kind]);
    setDimensionLoading((previous) => ({ ...previous, [kind]: true }));
    setDimensionError((previous) => ({ ...previous, [kind]: null }));
    const localProject = localSelection === undefined ? selectedProject : localSelection?.project || null;
    const localSession = localSelection === undefined ? selectedSession : localSelection?.session || null;
    try {
      const sort = snapshot?.sort || 'last_seen';
      const order = snapshot?.order || 'desc';
      const queryFilters = (kind === 'session' || kind === 'model') && localSession
        ? {
          ...activeFilters,
          sessionID: localSession.key,
        }
        : (kind === 'session' || kind === 'model') && localProject
        ? {
          ...activeFilters,
          // The row key is the stable project_id for identified projects.
          // Do not also send the display name: the API treats both fields as
          // independent predicates, which needlessly slows the query and can
          // exclude rows whose name changed. Synthetic unidentified rows have
          // no project_id, so they must use the display-name predicate.
          projectID: localProject.key === 'unidentified_project' ? '' : localProject.key,
          project: localProject.key === 'unidentified_project' ? localProject.name : '',
        }
        : activeFilters;
      const value = await api.getRuntimeDimensions(kind, {
        page, pageSize: nextPageSize, search, sort, order,
        snapshotSeq: snapshot?.snapshotSeq, historyGeneration: snapshot?.historyGeneration,
        filters: queryFilters,
      }, { signal: request.controller.signal });
      const normalized = normalizeRuntimePagedResult(value, { page, pageSize: nextPageSize });
      if (!normalized) throw new Error(`${dimensionLabel(kind)}响应不是分页形状`);
      if (request.isCurrent()) {
        setDimensions((previous) => ({ ...previous, [kind]: sameJSON(previous[kind], normalized) ? previous[kind] : normalized }));
      }
    } catch (error) {
      if (isAbortError(error) || !request.isCurrent()) return;
      setDimensionError((previous) => ({ ...previous, [kind]: error }));
      if (isRuntimeSnapshotError(error) && snapshot) {
        try {
          const queryFilters = (kind === 'session' || kind === 'model') && localSession
            ? { ...activeFilters, sessionID: localSession.key }
            : (kind === 'session' || kind === 'model') && localProject
            ? {
              ...activeFilters,
              projectID: localProject.key === 'unidentified_project' ? '' : localProject.key,
              project: localProject.key === 'unidentified_project' ? localProject.name : '',
            }
            : activeFilters;
          const value = await api.getRuntimeDimensions(kind, { page: 1, pageSize: nextPageSize, search, sort: 'last_seen', order: 'desc', filters: queryFilters }, { signal: request.controller.signal });
          const normalized = normalizeRuntimePagedResult(value, { page: 1, pageSize: nextPageSize });
          if (request.isCurrent()) setDimensions((previous) => ({ ...previous, [kind]: normalized }));
        } catch (retryError) {
          if (!isAbortError(retryError) && request.isCurrent()) setDimensionError((previous) => ({ ...previous, [kind]: retryError }));
        }
      }
    } finally {
      if (request.isCurrent()) setDimensionLoading((previous) => ({ ...previous, [kind]: false }));
    }
  }, [activeFilters, dimensionSearch, dimensions, pageSize, selectedProject, selectedSession]);

  const loadDimensions = useCallback((kinds = ['endpoint', 'project', 'session', 'model']) => {
    return Promise.allSettled(kinds.map((kind) => loadDimension(kind)));
  }, [loadDimension]);

  const loadStorage = useCallback(async ({ includeStorage = true, includeRetention = false, includePricing = false } = {}) => {
    const request = beginLatestRequest(storageRequestRef);
    setStorageLoading(true); setStorageError(null);
    try {
      // Storage, retention and pricing are independent resources. Do not
      // make every board pay for all three SQLite reads: the overview needs
      // storage, the token/cost board needs pricing, and the capacity setting
      // is only needed when storage management is opened.
      const requests = [
        ...(includeStorage
          ? [['storage', api.getRuntimeStorage({ signal: request.controller.signal })]]
          : []),
        ...(includeRetention
          ? [['retention', api.getRuntimeRetention({ signal: request.controller.signal })]]
          : []),
        ...(includePricing
          ? [['pricing', api.getRuntimePricing({ signal: request.controller.signal })]]
          : []),
      ];
      const results = await Promise.allSettled(requests.map(([, promise]) => promise));
      if (!request.isCurrent()) return;
      const errors = results.filter((result) => result.status === 'rejected' && !isAbortError(result.reason));
      results.forEach((result, index) => {
        if (result.status !== 'fulfilled') return;
        const [kind] = requests[index];
        if (kind === 'storage') setStorage((previous) => (sameJSON(previous, result.value) ? previous : result.value));
        if (kind === 'retention') setRetention((previous) => (sameJSON(previous, result.value) ? previous : result.value));
        if (kind === 'pricing') setPricing((previous) => (sameJSON(previous, result.value) ? previous : result.value));
      });
      if (errors.length) setStorageError(errors[0].reason);
    } catch (error) {
      if (!isAbortError(error) && request.isCurrent()) setStorageError(error);
    } finally {
      if (request.isCurrent()) setStorageLoading(false);
    }
  }, []);

  useEffect(() => {
    // Every filter defines a new SQLite snapshot. Abort old reads and clear
    // their page/snapshot state before loading the fresh trend projection;
    // request generations also prevent a late response from repainting the
    // previous filter view.
    // The initial render is handled by the section-loading effect below. Do
    // not issue the same trend request twice just because the serialized
    // filter key was initialized during the first render.
    if (previousQueryKeyRef.current === activeQueryKey) return;
    previousQueryKeyRef.current = activeQueryKey;
    trendRequestRef.current.controller?.abort();
    errorsRequestRef.current.controller?.abort();
    diagnosticsRequestRef.current.controller?.abort();
    Object.values(dimensionRequestRefs.current).forEach((reference) => reference.current?.controller?.abort());
    trendRequestRef.current = { id: safeNumber(trendRequestRef.current.id) + 1, controller: null };
    errorsRequestRef.current = { id: safeNumber(errorsRequestRef.current.id) + 1, controller: null };
    diagnosticsRequestRef.current = { id: safeNumber(diagnosticsRequestRef.current.id) + 1, controller: null };
    Object.values(dimensionRequestRefs.current).forEach((reference) => {
      reference.current = { id: safeNumber(reference.current?.id) + 1, controller: null };
    });
    setTrend(null);
    setErrors(null);
    setDiagnostics(null);
    setDimensions({ endpoint: null, project: null, session: null, model: null });
    if (selectedProject) {
      // The filter-change path below reloads all dimensions, including the
      // unfiltered session table. Avoid scheduling a second session request
      // when the local drill-down is cleared as part of that same change.
      skipNextClearedProjectReloadRef.current = true;
      onProjectSelectionChange?.(null);
    }
    if (selectedSession) onSessionSelectionChange?.(null);
    setSelectedProject(null);
    setSelectedSession(null);
    setTrendError(null);
    setErrorsError(null);
    setDiagnosticsError(null);
    setDimensionError({ endpoint: null, project: null, session: null, model: null });
    if (section === 'overview' || section === 'trends' || section === 'tokens') loadTrends(range);
    if (section === 'overview') loadDimensions(['endpoint', 'project', 'session', 'model']);
    if (section === 'errors') { loadErrors(1, errors?.pageSize || pageSize, null); loadDiagnostics(); }
    if (section === 'tokens') loadDimensions(['endpoint', 'project', 'session', 'model']);
  }, [activeQueryKey]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const previousSelectedProject = previousSelectedProjectRef.current;
    const previousSelectedSession = previousSelectedSessionRef.current;
    previousSelectedProjectRef.current = selectedProject;
    previousSelectedSessionRef.current = selectedSession;
    const projectChanged = selectedProject?.key !== previousSelectedProject?.key;
    const sessionChanged = selectedSession?.key !== previousSelectedSession?.key;
    if (!['overview', 'tokens'].includes(section) || (!projectChanged && !sessionChanged)) return;
    const skipProjectReload = projectChanged && !selectedProject && skipNextClearedProjectReloadRef.current;
    if (skipProjectReload) skipNextClearedProjectReloadRef.current = false;
    // Let the selected row/title paint before starting the potentially costly
    // session aggregation. This keeps the click responsive on large SQLite
    // histories while retaining the latest-wins abort behavior in
    // `loadDimension`.
    const schedule = typeof window !== 'undefined' && typeof window.requestAnimationFrame === 'function'
      ? (callback) => window.requestAnimationFrame(callback)
      : (callback) => globalThis.setTimeout(callback, 0);
    const cancel = typeof window !== 'undefined' && typeof window.cancelAnimationFrame === 'function'
      ? (handle) => window.cancelAnimationFrame(handle)
      : (handle) => globalThis.clearTimeout(handle);
    const handle = schedule(() => {
      if (projectChanged && !skipProjectReload) {
        loadDimension('session', 1, dimensions.session?.pageSize || pageSize, dimensionSearch.session || '', dimensions.session);
      }
      if (projectChanged || sessionChanged) {
        loadDimension('model', 1, dimensions.model?.pageSize || pageSize, dimensionSearch.model || '', dimensions.model);
      }
    });
    return () => cancel(handle);
  }, [selectedProject, selectedSession]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (projectSelectionResetSignal <= 0 || (!selectedProject && !selectedSession)) return;
    setSelectedProject(null);
    setSelectedSession(null);
  }, [projectSelectionResetSignal]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    // AppContext refreshes only the cheap summary/facet layer on its timer.
    // Revalidate the currently visible board here so automatic refreshes do
    // not leave a stale trend/error/dimension table behind the fresh picker
    // counts.  A generation/AbortController in each loader keeps this safe
    // when a filter change and timer tick arrive together.
    if (!analyticsRefreshSignal || analyticsRefreshSignal === lastAnalyticsRefreshSignal.current) return;
    // The section-loading effect below owns the very first board request.
    // AppContext emits its first signal only after the lightweight facets
    // request settles; reloading here would duplicate the in-flight/just
    // completed trend, error, or dimension request and make the initial page
    // feel slow on large SQLite histories. Subsequent signals are genuine
    // automatic-refresh ticks and must still revalidate the visible board.
    const firstSignal = lastAnalyticsRefreshSignal.current === 0;
    lastAnalyticsRefreshSignal.current = analyticsRefreshSignal;
    if (firstSignal) return;
    if (section === 'overview' || section === 'trends' || section === 'tokens') loadTrends(range);
    if (section === 'overview') loadDimensions(['endpoint', 'project', 'session', 'model']);
    if (section === 'errors') { loadErrors(1, errors?.pageSize || pageSize, null); loadDiagnostics(); }
    if (section === 'tokens') loadDimensions(['endpoint', 'project', 'session', 'model']);
    if (section === 'overview' && showStorageSettings) loadStorage({ includeRetention: true });
  }, [analyticsRefreshSignal]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (openExportSignal > 0) setShowExport(true);
  }, [openExportSignal]);
  useEffect(() => {
    if ((section === 'overview' || section === 'trends' || section === 'tokens') && !trend) loadTrends();
    if (section === 'overview' && !dimensions.endpoint) loadDimensions(['endpoint', 'project', 'session', 'model']);
    if (section === 'errors' && !errors) loadErrors();
    if (section === 'errors' && !diagnostics) loadDiagnostics();
    if (section === 'tokens' && !dimensions.endpoint) loadDimensions(['endpoint', 'project', 'session', 'model']);
    if (section === 'overview' && !storage) loadStorage();
    if (section === 'tokens' && !pricing) loadStorage({ includeStorage: false, includePricing: true });
    // Load once when entering a section. Individual panels expose Retry, so a
    // failed request must not create an automatic retry loop.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [section]);

  useEffect(() => {
    // Load the independent capacity-reminder setting only when storage
    // management is visible.
    if (section === 'overview' && showStorageSettings && !retention) {
      loadStorage({ includeRetention: true });
    }
  }, [loadStorage, retention, section, showStorageSettings]);

  useEffect(() => () => {
    trendRequestRef.current.controller?.abort();
    errorsRequestRef.current.controller?.abort();
    diagnosticsRequestRef.current.controller?.abort();
    Object.values(dimensionRequestRefs.current).forEach((reference) => reference.current?.controller?.abort());
    storageRequestRef.current.controller?.abort();
  }, []);

  const changePageSize = (kind, value) => {
    const next = persistRuntimeAnalyticsPageSize(value);
    setAnalyticsPageSize(next);
    if (kind === 'errors') loadErrors(1, next, null);
    else if (['endpoint', 'project', 'session', 'model'].includes(kind)) loadDimension(kind, 1, next, dimensionSearch[kind] || '', null);
  };
  const retryTrends = useCallback(() => loadTrends(range), [loadTrends, range]);
  const closeStorageSettings = useCallback(() => setShowStorageSettings(false), []);
  const moveSection = (current, direction) => {
    const index = SECTIONS.findIndex((item) => item.id === current);
    if (index < 0) return;
    const nextIndex = direction === 'home'
      ? 0
      : direction === 'end'
        ? SECTIONS.length - 1
        : (index + direction + SECTIONS.length) % SECTIONS.length;
    const next = SECTIONS[nextIndex].id;
    setSection(next);
    window.requestAnimationFrame(() => sectionTabRefs.current[nextIndex]?.focus());
  };
  const updateStorageRetention = async (payload) => {
    setStorageError(null);
    try {
      const value = await api.updateRuntimeRetention(payload);
      setRetention((current) => ({ ...(current || {}), ...value }));
      setStorage((current) => (current ? { ...current, retention: { ...(current.retention || {}), ...value } } : current));
      const hasTime = payload.maxAgeDays != null;
      const hasCapacity = payload.storageLimitBytes != null;
      addToast?.(
        !hasTime && !hasCapacity
          ? '已关闭自动保留条件，之后请手动清理统计'
          : hasTime && hasCapacity
            ? '时间与容量保留策略已保存，旧数据会自动轮换'
            : hasTime
              ? '最大保存天数已保存，旧数据会自动轮换'
              : '存储容量上限已保存，旧数据会自动轮换',
        'success',
      );
    } catch (error) {
      setStorageError(error);
      addToast?.(`存储设置更新失败：${error.message}`, 'error');
      throw error;
    }
  };
  const exportSession = async (sessionID) => {
    const value = String(sessionID || '').trim();
    if (!value) return;
    setSessionActionID(value);
    try {
      const payload = await api.exportRuntimeSession(value);
      // A single session export is already bounded by the server-side session
      // projection. Use a data URL so the browser does not allocate a second
      // Blob copy or retain an object URL after the download.
      const url = `data:application/json;charset=utf-8,${encodeURIComponent(JSON.stringify(payload, null, 2))}`;
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = `sumpter-session-${value.replace(/[^a-zA-Z0-9._-]+/g, '_').slice(0, 96) || 'export'}.json`;
      anchor.click();
      addToast?.('会话已导出', 'success');
    } catch (error) {
      addToast?.(`导出会话失败：${error.message}`, 'error');
    } finally {
      setSessionActionID('');
    }
  };

  const deleteSession = async (sessionID) => {
    const value = String(sessionID || '').trim();
    if (!value) return;
    if (value === 'unidentified_session') {
      const phrase = window.prompt('未识别会话可能包含多个来源。请输入 DELETE 以确认删除：');
      if (phrase !== 'DELETE') return;
    } else if (!window.confirm(`确认删除会话“${value}”？这会移除其请求、重试和 Token 统计，诊断捕获不会删除。`)) {
      return;
    }
    setSessionActionID(value);
    try {
      await api.deleteRuntimeSession(value, { confirmUnidentified: value === 'unidentified_session' });
      const deletedSelectedSession = selectedSession?.key === value || selectedSession?.name === value;
      if (deletedSelectedSession) {
        setSelectedSession(null);
        onSessionSelectionChange?.(null);
      }
      setDimensions((previous) => ({ ...previous, session: null }));
      await Promise.allSettled([
        loadDimension(
          'session',
          1,
          pageSize,
          dimensionSearch.session || '',
          null,
          deletedSelectedSession ? { project: selectedProject, session: null } : undefined,
        ),
        loadTrends(range),
      ]);
      addToast?.('会话及其统计已删除', 'success');
    } catch (error) {
      addToast?.(`删除会话失败：${error.message}`, 'error');
    } finally {
      setSessionActionID('');
    }
  };

  const clearProjectSticky = async (row) => {
    const projectID = String(row?.key || '').trim();
    const label = row?.name || projectID;
    if (!projectID) return;
    if (!window.confirm(`清除“${label}”的会话粘性归属？清除后该项目的新请求会按入口库顺序重新选择入口。`)) return;
    setStickyActionKey(projectID);
    try {
      const result = await api.clearProjectSticky(projectID);
      const cleared = Number(result?.cleared ?? 0);
      addToast?.(cleared > 0 ? `已清除 ${cleared} 条粘性归属` : '该项目当前没有粘性归属', cleared > 0 ? 'success' : 'info');
    } catch (error) {
      addToast?.(`清除粘性归属失败：${error.message}`, 'error');
    } finally {
      setStickyActionKey('');
    }
  };

  const dimensionCallbacks = {
    retry: (kind) => loadDimension(kind, 1, dimensions[kind]?.pageSize || pageSize, dimensionSearch[kind] || '', null),
    page: (kind, page) => loadDimension(kind, page),
    pageSize: (kind, size) => changePageSize(kind, size),
    search: (kind, value) => {
      setDimensionSearch((previous) => ({ ...previous, [kind]: value }));
      loadDimension(kind, 1, dimensions[kind]?.pageSize || pageSize, value, null);
    },
    sort: (kind, key, order) => loadDimension(
      kind,
      1,
      dimensions[kind]?.pageSize || pageSize,
      dimensionSearch[kind] || '',
      { ...(dimensions[kind] || {}), sort: key, order },
    ),
  };

  const selectProject = useCallback((row) => {
    const key = String(row?.key || '').trim();
    if (!key) return;
    const next = { key, name: String(row?.name || key) };
    skipNextClearedProjectReloadRef.current = false;
    setSelectedSession(null);
    onSessionSelectionChange?.(null);
    setSelectedProject(next);
    onProjectSelectionChange?.(next);
  }, [onProjectSelectionChange, onSessionSelectionChange]);
  const selectSession = useCallback((row) => {
    const key = String(row?.key || row?.name || '').trim();
    if (!key) return;
    const next = { key, name: String(row?.name || key) };
    setSelectedSession(next);
    onSessionSelectionChange?.(next);
  }, [onSessionSelectionChange]);
  const clearSelectedProject = useCallback(() => {
    skipNextClearedProjectReloadRef.current = false;
    setSelectedSession(null);
    onSessionSelectionChange?.(null);
    setSelectedProject(null);
    onProjectSelectionChange?.(null);
  }, [onProjectSelectionChange, onSessionSelectionChange]);
  const clearSelectedSession = useCallback(() => {
    setSelectedSession(null);
    onSessionSelectionChange?.(null);
  }, [onSessionSelectionChange]);

  return (
    <section className="runtime-v2-workspace" aria-label="统计详情">
      <nav className="runtime-v2-board-picker" aria-label="统计看板">
        <div className="runtime-v2-section-tabs" role="tablist" aria-label="数据库分析分面">
          {SECTIONS.map((item, index) => <button
            key={item.id}
            ref={(node) => { sectionTabRefs.current[index] = node; }}
            id={`analytics-tab-${item.id}`}
            type="button"
            role="tab"
            aria-selected={section === item.id}
            aria-controls={`analytics-panel-${item.id}`}
            tabIndex={section === item.id ? 0 : -1}
            className={section === item.id ? 'active' : ''}
            onClick={() => setSection(item.id)}
            onKeyDown={(event) => {
              if (event.key === 'ArrowRight' || event.key === 'ArrowDown') { event.preventDefault(); moveSection(item.id, 1); }
              if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') { event.preventDefault(); moveSection(item.id, -1); }
              if (event.key === 'Home') { event.preventDefault(); moveSection(item.id, 'home'); }
              if (event.key === 'End') { event.preventDefault(); moveSection(item.id, 'end'); }
            }}
          >{item.label}</button>)}
        </div>
      </nav>
      {section === 'overview' && <section id="analytics-panel-overview" className="runtime-v2-tab-panel" role="tabpanel" aria-labelledby="analytics-tab-overview">
        <OverviewPanel
          trend={trend}
          legacyAnalytics={legacyAnalytics}
          loading={trendLoading}
          error={trendError}
          storage={storage}
          summaryStorage={summaryStorage}
          storageRetention={retention}
          storageLoading={storageLoading}
          storageError={storageError}
          storageSettingsOpen={showStorageSettings}
          onRetry={() => loadTrends(range)}
          onOpenStorage={() => setShowStorageSettings(true)}
          onCloseStorage={closeStorageSettings}
          onRetryStorage={() => loadStorage({ includeRetention: showStorageSettings })}
          onUpdateRetention={updateStorageRetention}
          onManualCleanup={onManualCleanup}
          onRecreateDatabase={onRecreateDatabase}
          usagePages={dimensions}
          usageLoading={dimensionLoading}
          usageErrors={dimensionError}
          usageCallbacks={dimensionCallbacks}
          clientKindFacets={facets?.clientKinds}
          addToast={addToast}
          onSessionExport={exportSession}
          onSessionDelete={deleteSession}
          sessionActionID={sessionActionID}
          selectedProject={selectedProject}
          selectedSession={selectedSession}
          onProjectSelect={selectProject}
          onSessionSelect={selectSession}
          onClearProject={clearSelectedProject}
          onClearSession={clearSelectedSession}
        />
      </section>}
      {section === 'trends' && <section id="analytics-panel-trends" className="runtime-v2-tab-panel" role="tabpanel" aria-labelledby="analytics-tab-trends"><MemoizedTrendPanel trend={trend} legacyAnalytics={legacyAnalytics} loading={trendLoading} error={trendError} onRetry={retryTrends} /></section>}
      {section === 'errors' && <section id="analytics-panel-errors" role="tabpanel" aria-labelledby="analytics-tab-errors" className="runtime-v2-panel"><div className="runtime-v2-panel-header"><div><h2>错误与结构化诊断</h2><p>先看错误分组与样本，再按需展开模型、用途、协议、失败和流终止分组。</p></div></div><PanelMessage error={errorsError} onRetry={() => loadErrors(1)} /><div className="runtime-v2-table-status">{errorsLoading && <LoadingLine text="正在读取错误分组…" />}</div>{errors && <ErrorTable page={errors} loading={errorsLoading} onPageChange={(page) => loadErrors(page)} onPageSizeChange={(size) => changePageSize('errors', size)} onSelectEvent={onSelectEvent} />}<AdvancedDiagnosticsPanel analytics={diagnostics} loading={diagnosticsLoading} error={diagnosticsError} onRetry={loadDiagnostics} /></section>}
      {section === 'tokens' && <section id="analytics-panel-tokens" role="tabpanel" aria-labelledby="analytics-tab-tokens">
        <CostPanel pricing={pricing} trend={trend} legacyAnalytics={legacyAnalytics} costPages={dimensions} costLoading={dimensionLoading} costErrors={dimensionError} costCallbacks={dimensionCallbacks} clientKindFacets={facets?.clientKinds} addToast={addToast} selectedProject={selectedProject} selectedSession={selectedSession} onProjectSelect={selectProject} onSessionSelect={selectSession} onClearProject={clearSelectedProject} onClearSession={clearSelectedSession} />
      </section>}
      {showExport && <section id="analytics-panel-export" className="runtime-v2-panel" aria-label="运行统计导出"><div className="runtime-v2-panel-header"><div><h2>运行统计导出</h2><p>导出当前筛选下的 SQLite 运行统计字段，不包含诊断正文。</p></div><button type="button" className="btn btn-ghost btn-sm" onClick={() => setShowExport(false)}>关闭</button></div><ExportPanel addToast={addToast} filters={activeFilters} /></section>}
    </section>
  );
}

// Compatibility export for extensions that imported the pre-v3 filename. The
// page and new code use AnalyticsWorkspace; keeping this alias avoids a hard
// failure for an out-of-tree plugin while the old component file is retired.
export const AnalyticsV2Workspace = AnalyticsWorkspace;
