import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api } from '../services/api.js';
import {
  clampRuntimeEventPage,
  isRuntimeSnapshotError,
  normalizeRuntimeEventPage,
  persistRuntimeEventPageSize,
  readRuntimeEventPageSize,
  runtimeSnapshotRecoveryMessage,
} from '../utils/runtimeAnalyticsV2.js';
import { eventIsInFlight } from '../utils/helpers.js';

function eventMatchesKind(event, kind) {
  return !kind || event?.kind === kind;
}

function completedNewEventCount(events, snapshotSeq, kind) {
  if (snapshotSeq == null) return 0;
  const ids = new Set();
  for (const event of events || []) {
    if (!eventMatchesKind(event, kind) || eventIsInFlight(event)) continue;
    if (Number(event?.seq || 0) > Number(snapshotSeq)) ids.add(event.id);
  }
  return ids.size;
}

export function useRuntimeEventPage({
  recentEvents = [], eventKind = '', resetGeneration = 0, onRecovered,
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
  const queryKey = `${eventKind || 'all'}:${pageSize}:${Number(resetGeneration || 0)}`;

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
    const controller = new AbortController();
    const requestedPage = Math.max(1, Number(page) || 1);
    const snapshot = snapshotRef.current;
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
        snapshotSeq: useSnapshot ? snapshot?.snapshotSeq : undefined,
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
        snapshotRef.current = {
          snapshotSeq: normalized.snapshotSeq,
          historyGeneration: normalized.historyGeneration,
        };
        setResult(normalized);
        setMode('page');
        if (normalized.page !== requestedPage) setPageState(normalized.page);
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

    return () => controller.abort();
  }, [eventKind, onRecovered, page, pageSize, reloadGeneration, resetGeneration]);

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
    if (mode === 'page' && page !== 1) return [];
    return recentEvents.filter((event) => eventMatchesKind(event, eventKind) && eventIsInFlight(event));
  }, [eventKind, mode, page, recentEvents]);

  const persistedEvents = useMemo(() => {
    const source = mode === 'page' ? (result?.events || []) : recentEvents;
    const liveIDs = new Set(liveEvents.map((event) => event.id));
    return source.filter((event) => !liveIDs.has(event.id) && !eventIsInFlight(event));
  }, [eventKind, liveEvents, mode, recentEvents, result?.events]);

  const events = useMemo(() => [...liveEvents, ...persistedEvents], [liveEvents, persistedEvents]);

  // A page-1 snapshot follows the live event stream automatically. The old
  // pending-new-events prompt made a real-time run page look stale even though
  // SSE had already delivered the event. Debounce the reset so a burst of
  // completed events results in one stable-snapshot request.
  useEffect(() => {
    if (mode !== 'page' || page !== 1 || refreshing || snapshotRef.current == null) return undefined;
    const pending = completedNewEventCount(recentEvents, snapshotRef.current.snapshotSeq, eventKind);
    if (pending <= 0 || autoRefreshTimerRef.current != null) return undefined;
    const newest = recentEvents.filter((event) => eventMatchesKind(event, eventKind) && !eventIsInFlight(event))
      .reduce((value, event) => Math.max(value, Number(event.seq || 0)), 0);
    const signature = `${eventKind}:${snapshotRef.current.snapshotSeq}:${newest}`;
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
  }, [eventKind, refreshing, mode, page, recentEvents]);

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
    historyGeneration: result?.historyGeneration ?? null,
    setPage,
    setPageSize,
    retry: () => { backgroundRefreshRef.current = false; setReloadGeneration((value) => value + 1); },
  };
}
