import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../services/api.js';
import {
  clampRuntimeEventPage,
  isRuntimeSnapshotError,
  normalizeRuntimeEventPage,
  persistRuntimeEventPageSize,
  readRuntimeEventPageSize,
  runtimeRangeFilter,
  runtimeCurrentRangeSnapshot,
  runtimeSnapshotFilters,
  runtimeSnapshotRecoveryMessage,
} from '../utils/runtimeAnalyticsV2.js';
import { eventIsInFlight, eventResultKind } from '../utils/helpers.js';
import { completedEventsAfterSnapshot, mergeCompletedPage } from '../utils/runtimeEventSnapshot.js';
import { runtimeEventInRange } from '../utils/runtimeEvents.js';

function eventMatchesKind(event, kind) {
  return !kind || event?.kind === kind;
}

function eventMatchesOutcome(event, outcome) {
  return !outcome || eventResultKind(event) === outcome;
}

export function useRuntimeEventPage({
  recentEvents = [], eventKind = '', outcome = '', resetGeneration = 0, onRecovered,
  autoRefresh = true, refreshInterval = 15, enabled = true,
  range = 'all',
  scopeKey = '',
} = {}) {
  const [page, setPageState] = useState(1);
  const [pageSize, setPageSizeState] = useState(readRuntimeEventPageSize);
  const [result, setResult] = useState(null);
  const [mode, setMode] = useState('loading');
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const backgroundRefreshRef = useRef(false);
  const lastAutoRefreshRef = useRef(null);
  const [error, setError] = useState(null);
  const [reloadGeneration, setReloadGeneration] = useState(0);
  const requestGenerationRef = useRef(0);
  const snapshotRef = useRef(null);
  const autoRefreshTimerRef = useRef(null);
  const queryKey = `${range}:${scopeKey}:${eventKind || 'all'}:${outcome || 'all'}:${pageSize}:${Number(resetGeneration || 0)}`;
  const currentSnapshot = runtimeCurrentRangeSnapshot(range, result);
  const from = currentSnapshot?.filters?.from ?? runtimeRangeFilter(range).from;
  const to = currentSnapshot?.filters?.to;
  const scopedRecentEvents = useMemo(() => recentEvents.filter(event => runtimeEventInRange(event, { from, to })), [recentEvents, from, to]);

  useEffect(() => {
    backgroundRefreshRef.current = false;
    lastAutoRefreshRef.current = null;
    snapshotRef.current = null;
    setMode('loading');
    setLoading(true);
    setResult(null);
    setError(null);
    setPageState(1);
  }, [queryKey]);

  useEffect(() => {
    const generation = ++requestGenerationRef.current;
    if (!enabled) { setRefreshing(false); return undefined; }
    const controller = new AbortController();
    const snapshot = runtimeCurrentRangeSnapshot(range, snapshotRef.current);
    const requestedPage = snapshotRef.current && !snapshot ? 1 : Math.max(1, Number(page) || 1);
    const background = backgroundRefreshRef.current;
    backgroundRefreshRef.current = false;
    setLoading(!background);
    setRefreshing(true);
    setError(null);

    const fetchPage = async (useSnapshot = true) => {
      const value = await api.getRuntimeEventPage({
        page: useSnapshot ? requestedPage : 1,
        pageSize,
        kind: eventKind || undefined,
        outcome: outcome || undefined,
        ...runtimeSnapshotFilters(runtimeRangeFilter(range), useSnapshot ? snapshot : null),
        snapshotSeq: useSnapshot ? snapshot?.snapshotSeq : undefined,
        snapshotChangeSeq: useSnapshot ? snapshot?.snapshotChangeSeq : undefined,
        historyGeneration: useSnapshot ? snapshot?.historyGeneration : undefined,
        signal: controller.signal,
      });
      return normalizeRuntimeEventPage(value, {
        page: useSnapshot ? requestedPage : 1,
        pageSize,
      });
    };

    (async () => {
      try {
        let normalized;
        try {
          normalized = await fetchPage(true);
        } catch (requestError) {
          if (!isRuntimeSnapshotError(requestError)) throw requestError;
          normalized = await fetchPage(false);
          if (generation === requestGenerationRef.current) {
            setPageState(1);
            onRecovered?.(runtimeSnapshotRecoveryMessage(requestError));
          }
        }
        if (generation !== requestGenerationRef.current) return;
        if (!normalized) {
          // Old daemons ignore `view=page` and return the v1 cursor shape.
          setMode('legacy');
          setResult(null);
          return;
        }
        snapshotRef.current = normalized;
        setResult(normalized);
        setMode('page');
        if (normalized.page !== page) setPageState(normalized.page);
      } catch (requestError) {
        if (generation !== requestGenerationRef.current || requestError?.name === 'AbortError') return;
        if ([400, 404, 405].includes(Number(requestError?.status))) {
          setMode('legacy');
          setResult(null);
          return;
        }
        setError(requestError?.message || '加载事件历史失败');
      } finally {
        if (generation === requestGenerationRef.current) { setLoading(false); setRefreshing(false); }
      }
    })();

    return () => { ++requestGenerationRef.current; controller.abort(); };
  }, [enabled, range, scopeKey, eventKind, outcome, onRecovered, page, pageSize, reloadGeneration, resetGeneration]);

  const setPage = useCallback((value) => {
    const next = clampRuntimeEventPage(value, result?.totalPages || 1);
    backgroundRefreshRef.current = false;
    setPageState(next);
  }, [result?.totalPages]);

  const setPageSize = useCallback((value) => {
    backgroundRefreshRef.current = false;
    const next = persistRuntimeEventPageSize(value);
    snapshotRef.current = null;
    setPageSizeState(next);
    setPageState(1);
  }, []);

  // In-flight requests are a live overlay. They must not consume a slot in
  // the persisted page-size contract, otherwise “每页 10 条” can render 11
  // visible table rows when a request is still streaming.
  const liveEvents = useMemo(() => {
    if ((mode === 'page' && page !== 1) || outcome) return [];
    return scopedRecentEvents.filter((event) => eventMatchesKind(event, eventKind) && eventIsInFlight(event));
  }, [eventKind, outcome, mode, page, scopedRecentEvents]);

  const persistedEvents = useMemo(() => {
    const additions = (mode === 'page' && page === 1
      ? completedEventsAfterSnapshot(scopedRecentEvents, result, eventKind) : [])
      .filter((event) => eventMatchesOutcome(event, outcome));
    const source = mode === 'page'
      ? mergeCompletedPage(result?.events || [], additions, pageSize) : scopedRecentEvents;
    const liveIDs = new Set(liveEvents.map((event) => event.id));
    return source.filter((event) => (
      eventMatchesKind(event, eventKind)
      && runtimeEventInRange(event, { from, to })
      && eventMatchesOutcome(event, outcome)
      && !liveIDs.has(event.id)
      && !eventIsInFlight(event)
    ));
  }, [eventKind, outcome, liveEvents, mode, page, pageSize, scopedRecentEvents, result, from, to]);

  const events = useMemo(() => [...liveEvents, ...persistedEvents], [liveEvents, persistedEvents]);

  // A page-1 snapshot follows the live event stream automatically. The old
  // pending-new-events prompt made a real-time run page look stale even though
  // SSE had already delivered the event. Debounce the reset so a burst of
  // completed events results in one stable-snapshot request.
  useEffect(() => {
    if (!enabled || !autoRefresh || mode !== 'page' || page !== 1 || refreshing || snapshotRef.current == null) return undefined;
    const changes = completedEventsAfterSnapshot(scopedRecentEvents, snapshotRef.current, eventKind);
    if (!changes.length || autoRefreshTimerRef.current != null) return undefined;
    const newest = Math.max(...changes.map((event) => Number(event.changeSeq || 0)));
    const signature = `${eventKind}:${snapshotRef.current.snapshotChangeSeq ?? 'legacy'}:${newest}`;
    // A persisted snapshot can lag its SSE notice. Do not restart the same
    // refresh on every loading transition; retry only with progress or a later poll.
    const last = lastAutoRefreshRef.current;
    if (last?.signature === signature && Date.now() - last.at < 5000) return undefined;
    autoRefreshTimerRef.current = window.setTimeout(() => {
      autoRefreshTimerRef.current = null;
      lastAutoRefreshRef.current = { signature, at: Date.now() };
      backgroundRefreshRef.current = true;
      snapshotRef.current = null;
      setReloadGeneration((value) => value + 1);
    }, 220);
    return () => {
      if (autoRefreshTimerRef.current != null) {
        window.clearTimeout(autoRefreshTimerRef.current);
        autoRefreshTimerRef.current = null;
      }
    };
  }, [enabled, autoRefresh, eventKind, refreshing, mode, page, scopedRecentEvents]);

  useEffect(() => {
    if (!enabled || !autoRefresh || page !== 1 || mode !== 'page') return undefined;
    const timer = window.setInterval(() => {
      if (document.hidden || refreshing) return;
      backgroundRefreshRef.current = true;
      snapshotRef.current = null;
      setReloadGeneration((value) => value + 1);
    }, Math.max(1, Number(refreshInterval) || 15) * 1000);
    return () => window.clearInterval(timer);
  }, [enabled, autoRefresh, page, mode, refreshInterval, refreshing]);

  return {
    mode,
    loading,
    error,
    events,
    liveEvents,
    persistedEvents,
    liveEventCount: liveEvents.length,
    // Expose the requested page immediately. This lets pagination remain
    // interruptible: a second click can abort the in-flight request and move
    // directly to the newest target while the previous table stays visible.
    page,
    pageSize,
    totalCount: result?.totalCount ?? events.length,
    totalPages: result?.totalPages ?? (events.length ? 1 : 0),
    hasNext: Boolean(result?.hasNext),
    hasPrevious: Boolean(result?.hasPrevious),
    snapshotSeq: result?.snapshotSeq ?? null,
    snapshotChangeSeq: result?.snapshotChangeSeq ?? null,
    historyGeneration: result?.historyGeneration ?? null,
    setPage,
    setPageSize,
    retry: () => { backgroundRefreshRef.current = false; setReloadGeneration((value) => value + 1); },
  };
}
