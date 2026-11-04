import { runtimeRangeFilter } from './runtimeAnalyticsV2.js';

export const RUN_OVERVIEW_RANGE = 'today';

export function runOverviewQuery(now = new Date()) {
  return { range: RUN_OVERVIEW_RANGE, granularity: 'hour', filters: runtimeRangeFilter(RUN_OVERVIEW_RANGE, now) };
}

// Cards use one server aggregate for the same local calendar day. Missing data
// stays unknown; it must never fall back to the lifetime summary counters.
export function runOverviewTotals(series) {
  const totals = series?.totals;
  const tokens = totals?.tokens;
  if (!tokens || typeof tokens !== 'object') return null;
  const number = value => value != null && value !== '' && Number.isFinite(Number(value)) && Number(value) >= 0 ? Number(value) : null;
  const observed = number(tokens.observedRequests ?? tokens.observed_requests);
  if (observed == null) return null;
  return {
    input: number(tokens.inputTokens ?? tokens.input_tokens),
    output: number(tokens.outputTokens ?? tokens.output_tokens),
    observed,
    clientRequests: number(totals.clientRequests ?? totals.client_requests),
    clientSuccesses: number(totals.clientSuccesses ?? totals.client_successes),
    clientFailures: number(totals.clientFailures ?? totals.client_failures),
    upstreamAttempts: number(totals.upstreamAttempts ?? totals.upstream_attempts),
    failovers: number(totals.failovers),
  };
}
