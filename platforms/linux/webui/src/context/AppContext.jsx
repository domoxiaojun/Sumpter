import React, { createContext, useContext, useState, useEffect, useCallback, useRef } from 'react';
import { api, getAuthState, openEventStream, subscribeAuth } from '../services/api.js';
import { upsertRuntimeEvent } from '../utils/runtimeEvents.js';
import { fetchRuntimeChanges, mergeRuntimeListItems, RUNTIME_API_VERSION } from '../utils/runtimeSync.js';
import { clone } from '../utils/helpers.js';

const AppContext = createContext(null);

function normalizeAnalyticsFilters(filters = {}) {
  return {
    clientKind: String(filters.clientKind || ''),
    endpointID: String(filters.endpointID || ''),
    project: String(filters.project || ''),
    projectID: String(filters.projectID || ''),
    sessionID: String(filters.sessionID || ''),
    model: String(filters.model || ''),
    requestPurpose: String(filters.requestPurpose || ''),
    outcome: String(filters.outcome || ''),
    failureKind: String(filters.failureKind || ''),
    failurePhase: String(filters.failurePhase || ''),
  };
}

const refreshIntervalOptions = [1, 2, 5, 10, 15, 30];
const refreshIntervalStorageKeys = {
  run: 'kekulv-refresh-interval-run',
  statistics: 'kekulv-refresh-interval-statistics',
};

function readRefreshInterval(key, fallback) {
  try {
    const value = Number(localStorage.getItem(key));
    return refreshIntervalOptions.includes(value) ? value : fallback;
  } catch {
    return fallback;
  }
}

function persistRefreshInterval(key, value) {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    // Refresh settings still apply for this session when storage is unavailable.
  }
}

// Cheap change detector for the statistics workspace. Only fields that can
// change the visible result participate; object identity and SQLite internals
// must not turn a quiet polling tick into a React tree update.
function runtimeSummaryRevision(summary) {
  const storage = summary?.storage || {};
  const latest = summary?.latestEvent || {};
  return JSON.stringify({
    resetGeneration: summary?.resetGeneration ?? 0,
    historyGeneration: summary?.historyGeneration ?? storage?.historyGeneration ?? 0,
    counters: summary?.counters || {},
    latestEvent: {
      id: latest.id || null,
      seq: latest.seq ?? null,
      changeSeq: latest.changeSeq ?? null,
      phase: latest.phase || null,
      outcome: latest.outcome || null,
      statusCode: latest.statusCode ?? null,
      timestamp: latest.timestamp ?? null,
    },
    storage: {
      state: storage.state || null,
      eventCount: storage.eventCount ?? storage.event_count ?? null,
      completedEventCount: storage.completedEventCount ?? storage.completed_event_count ?? null,
      inFlightEventCount: storage.inFlightEventCount ?? storage.in_flight_event_count ?? null,
      pendingEvents: storage.pendingEvents ?? storage.pending_events ?? null,
      pendingBytes: storage.pendingBytes ?? storage.pending_bytes ?? null,
      lastError: storage.lastError ?? storage.last_error ?? null,
    },
  });
}

function sameJSON(left, right) {
  if (left === right) return true;
  try { return JSON.stringify(left) === JSON.stringify(right); } catch { return false; }
}

// Three-way merge for queued config saves. It preserves edits that landed
// while a caller was waiting (especially catalog writes) while still allowing
// the caller's intentional fields to win. Endpoint arrays are merged by id
// when their membership/order is unchanged; structural edits remain explicit.
function mergeConfigChanges(source, requested, latest) {
  const result = clone(latest || {});
  const sourceValue = source || {};
  const requestedValue = requested || {};
  for (const key of new Set([...Object.keys(sourceValue), ...Object.keys(requestedValue)])) {
    if (key === 'endpoints' && Array.isArray(requestedValue.endpoints) && Array.isArray(sourceValue.endpoints)) {
      const sourceIDs = sourceValue.endpoints.map((item) => item?.id);
      const requestedIDs = requestedValue.endpoints.map((item) => item?.id);
      if (!sameJSON(sourceIDs, requestedIDs)) {
        result.endpoints = clone(requestedValue.endpoints);
        continue;
      }
      const latestByID = new Map((result.endpoints || []).map((item) => [item?.id, item]));
      result.endpoints = requestedValue.endpoints.map((requestedEndpoint, index) => {
        const sourceEndpoint = sourceValue.endpoints[index] || {};
        const latestEndpoint = clone(latestByID.get(requestedEndpoint?.id) || requestedEndpoint);
        for (const field of new Set([...Object.keys(sourceEndpoint), ...Object.keys(requestedEndpoint || {})])) {
          if (!sameJSON(sourceEndpoint[field], requestedEndpoint?.[field])) {
            latestEndpoint[field] = clone(requestedEndpoint[field]);
          }
        }
        return latestEndpoint;
      });
      continue;
    }
    if (!sameJSON(sourceValue[key], requestedValue[key])) {
      result[key] = clone(requestedValue[key]);
    }
  }
  return result;
}

export function AppProvider({ children }) {
  // Navigation & Page State
  const [currentRoute, setCurrentRoute] = useState(() => {
    const hash = window.location.hash.replace(/^#/, '');
    const legacyProviderRoute = hash === 'providers-primary' || hash === 'providers-fallback';
    const validRoutes = ['run', 'providers', 'routing', 'security', 'statistics', 'diagnostics', 'help', 'about'];
    return legacyProviderRoute ? 'providers' : (validRoutes.includes(hash) ? hash : 'run');
  });

  // Theme State
  const [theme, setTheme] = useState(() => localStorage.getItem('kekulv-theme') || 'dark');

  // Core Data State
  const [configDoc, setConfigDoc] = useState(null);
  const [status, setStatus] = useState(null);
  const [runtime, setRuntime] = useState(null);
  const [runtimeAnalytics, setRuntimeAnalytics] = useState(null);
  const [runtimeFacets, setRuntimeFacets] = useState(null);
  // Incremented after a successful statistics snapshot refresh.  The
  // lightweight summary/facet request lives in AppContext, while trend and
  // board data live in AnalyticsWorkspace; this signal keeps those two layers
  // on the same automatic-refresh cadence without reintroducing a full
  // /runtime/analytics request into the critical path.
  const [analyticsRefreshSignal, setAnalyticsRefreshSignal] = useState(0);
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const [analyticsError, setAnalyticsError] = useState(null);
  const [analyticsStale, setAnalyticsStale] = useState(false);
  const [runtimeEventDetail, setRuntimeEventDetail] = useState(null);
  const [analyticsRange, setAnalyticsRange] = useState('today');
  const [analyticsFilters, setAnalyticsFilters] = useState({ clientKind: '', endpointID: '', project: '', projectID: '', sessionID: '', model: '', requestPurpose: '', outcome: '', failureKind: '', failurePhase: '' });
  const [diagnostics, setDiagnostics] = useState(null);
  const [autostart, setAutostart] = useState(null);
  const [isLoading, setIsLoading] = useState(true);
  const [lastError, setLastError] = useState(null);
  const [auth, setAuth] = useState(getAuthState());
  const [authLoading, setAuthLoading] = useState(true);
  // Keep the latest config document available to queued writes. React state
  // updates are asynchronous, so two fast actions (notably parallel model
  // catalog fetches) must not both capture the same generation.
  const configDocRef = useRef(null);
  const configSaveQueueRef = useRef(Promise.resolve());
  const streamRef = useRef(null);
  const lastChangeSeqRef = useRef(0);
  const resetGenerationRef = useRef(0);
  const runtimeRefreshTimerRef = useRef(null);
  const coreAbortRef = useRef(null);
  const analyticsFacetAbortRef = useRef(null);
  const migrationNoticesRef = useRef(new Set());
  // These refs keep data requests independent from React render identity. In
  // particular, changing analytics filters must not recreate the SSE effect.
  const analyticsRangeRef = useRef(analyticsRange);
  const analyticsFiltersRef = useRef(analyticsFilters);
  const refreshGenerationRef = useRef(0);
  const analyticsGenerationRef = useRef(0);
  const analyticsSummaryRevisionRef = useRef('');
  const analyticsFacetsRevisionRef = useRef('');
  const eventDetailGenerationRef = useRef(0);

  configDocRef.current = configDoc;

  // Facets are useful for the filter controls but are not part of the core
  // liveness snapshot. Keep them on a separate, latest-wins path so a slow
  // GROUP BY over a large SQLite history cannot block status/config/event
  // rendering or make the whole page appear stuck.
  const fetchAnalyticsFacets = useCallback((generation, parentController = null, options = {}) => {
    // A filter/range change should stop the old HTTP transfer and client-side
    // decode, not merely hide its eventual response. This does not claim to
    // interrupt a synchronous SQLite statement already running server-side;
    // the SQL range pushdown below is what bounds that work. Keep a private
    // controller so cancelling facets never aborts the caller's core
    // status/config request; relay the core controller only when that refresh
    // itself is superseded.
    analyticsFacetAbortRef.current?.abort();
    const controller = new AbortController();
    analyticsFacetAbortRef.current = controller;
    const relayAbort = () => controller.abort();
    parentController?.signal?.addEventListener('abort', relayAbort, { once: true });
    return api.getRuntimeFacets(analyticsRangeRef.current, analyticsFiltersRef.current, { signal: controller.signal })
      .then((value) => {
        if (analyticsGenerationRef.current !== generation) return null;
        const nextRevision = JSON.stringify(value);
        if (analyticsFacetsRevisionRef.current !== nextRevision) {
          analyticsFacetsRevisionRef.current = nextRevision;
          setRuntimeFacets(value);
        }
        setRuntimeAnalytics((previous) => (previous == null ? previous : null));
        setAnalyticsError((previous) => (previous == null ? previous : null));
        setAnalyticsStale((previous) => (previous ? false : previous));
        // Filter/range changes already trigger the workspace loader directly.
        // Only the periodic refresh path asks the workspace to revalidate its
        // active board; this avoids issuing the same trend request twice for a
        // single user selection.
        if (options.notifyWorkspace) setAnalyticsRefreshSignal((current) => current + 1);
        return value;
      })
      .catch((error) => {
        if (analyticsGenerationRef.current !== generation || error?.name === 'AbortError') return null;
        setAnalyticsError(error?.message || '加载统计筛选项失败');
        setAnalyticsStale(true);
        return null;
      })
      .finally(() => {
        parentController?.signal?.removeEventListener('abort', relayAbort);
        if (analyticsFacetAbortRef.current === controller) analyticsFacetAbortRef.current = null;
        if (analyticsGenerationRef.current === generation) setAnalyticsLoading(false);
      });
  }, []);

  const requireRuntimeApiV1 = useCallback((summary) => {
    if (Number(summary?.apiVersion) !== RUNTIME_API_VERSION) {
      throw new Error(`运行统计 API 版本不匹配：需要 v${RUNTIME_API_VERSION}，当前为 v${summary?.apiVersion ?? '未知'}，请同步升级 daemon 与 WebUI`);
    }
  }, []);

  const requireStatusRuntimeApiV1 = useCallback((statusValue) => {
    if (statusValue?.runtimeApiVersion != null
      && Number(statusValue.runtimeApiVersion) !== RUNTIME_API_VERSION) {
      throw new Error(`daemon 运行统计 API 版本为 v${statusValue.runtimeApiVersion}，WebUI 需要 v${RUNTIME_API_VERSION}，请同步升级`);
    }
  }, []);

  // Real-time Controls
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [runRefreshInterval, setRunRefreshIntervalState] = useState(() => (
    readRefreshInterval(refreshIntervalStorageKeys.run, 5)
  ));
  const [statisticsRefreshInterval, setStatisticsRefreshIntervalState] = useState(() => (
    readRefreshInterval(refreshIntervalStorageKeys.statistics, 15)
  ));
  const [streamConnected, setStreamConnected] = useState(false);

  const setRunRefreshInterval = useCallback((value) => {
    const next = refreshIntervalOptions.includes(Number(value)) ? Number(value) : 5;
    setRunRefreshIntervalState(next);
    persistRefreshInterval(refreshIntervalStorageKeys.run, next);
  }, []);

  const setStatisticsRefreshInterval = useCallback((value) => {
    const next = refreshIntervalOptions.includes(Number(value)) ? Number(value) : 15;
    setStatisticsRefreshIntervalState(next);
    persistRefreshInterval(refreshIntervalStorageKeys.statistics, next);
  }, []);

  // Modals, Palette & Toasts
  const [toasts, setToasts] = useState([]);
  const [isCommandOpen, setIsCommandOpen] = useState(false);
  const [activeModal, setActiveModal] = useState(null);

  // Sync Theme to DOM
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem('kekulv-theme', theme);
  }, [theme]);

  // Sync Hash to Route
  useEffect(() => {
    const handleHashChange = () => {
      const hash = window.location.hash.replace(/^#/, '');
      if (hash === 'providers-primary' || hash === 'providers-fallback') {
        setCurrentRoute('providers');
      } else if (['run', 'providers', 'routing', 'security', 'statistics', 'diagnostics', 'help', 'about'].includes(hash)) {
        setCurrentRoute(hash);
      }
    };
    window.addEventListener('hashchange', handleHashChange);
    return () => window.removeEventListener('hashchange', handleHashChange);
  }, []);

  // Set Route with Hash update
  const navigate = useCallback((route) => {
    window.location.hash = `#${route}`;
    setCurrentRoute(route);
  }, []);

  // Toast Management
  const addToast = useCallback((message, kind = 'info', timeout = 4000) => {
    const id = `toast-${Date.now()}-${Math.random().toString(36).substr(2, 6)}`;
    const newToast = { id, message, kind };
    setToasts((prev) => [...prev, newToast]);
    if (timeout > 0) {
      setTimeout(() => {
        setToasts((prev) => prev.filter((t) => t.id !== id));
      }, timeout);
    }
  }, []);

  const removeToast = useCallback((id) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const showMigrationNotice = useCallback((notice) => {
    const id = String(notice?.id || '').trim();
    if (!id || migrationNoticesRef.current.has(id)) return;
    const storageKey = `kekulv-migration-notice:${id}`;
    let alreadyShown = false;
    try { alreadyShown = Boolean(localStorage.getItem(storageKey)); } catch { /* notification stays non-blocking */ }
    if (alreadyShown) {
      migrationNoticesRef.current.add(id);
      return;
    }
    migrationNoticesRef.current.add(id);
    try { localStorage.setItem(storageKey, 'shown'); } catch { /* in-memory de-duplication still applies */ }
    const convertedIDs = Array.isArray(notice?.convertedToAutoEndpointIds)
      ? notice.convertedToAutoEndpointIds
      : [];
    const expanded = Number(notice?.expandedLegacyPassthroughEndpoints
      ?? notice?.autoEndpointCount
      ?? notice?.migratedEndpointCount
      ?? convertedIDs.length
      ?? 0);
    addToast(
      expanded > 0
        ? `配置已升级到 v${notice?.toSchema || 4}：${expanded} 个入口已转为“自动（三协议）”，请按上游能力复核`
        : `配置已安全升级到 v${notice?.toSchema || 4}`,
      'info',
      10000,
    );
  }, [addToast]);

  useEffect(() => subscribeAuth(setAuth), []);

  // Fetch Core Data
  const refreshCore = useCallback(async () => {
    if (!auth.authenticated) return;
    const refreshGeneration = ++refreshGenerationRef.current;
    const analyticsGeneration = ++analyticsGenerationRef.current;
    const shouldLoadAnalytics = currentRoute === 'statistics';
    const shouldLoadEvents = currentRoute === 'run';
    let analyticsSnapshotChanged = false;
    coreAbortRef.current?.abort();
    const controller = new AbortController();
    coreAbortRef.current = controller;
    const timeout = window.setTimeout(() => controller.abort(), 10_000);
    // A fallback refresh while the statistics page is already populated must
    // keep the current board visible. Only the first picker read needs a
    // blocking loading state; later reads are background reconciliation.
    const analyticsNeedsInitialLoad = shouldLoadAnalytics && analyticsFacetsRevisionRef.current === '';
    if (analyticsNeedsInitialLoad) {
      setAnalyticsLoading(true);
      setAnalyticsError(null);
      setAnalyticsStale(true);
    }
    try {
      setLastError(null);
      const [confRes, statusRes, summaryRes, eventsRes] = await Promise.all([
        api.getConfig({ signal: controller.signal }),
        api.getStatus({ signal: controller.signal }),
        api.getRuntimeSummary({ signal: controller.signal }),
        shouldLoadEvents
          ? api.getRuntimeEvents({ limit: 10, signal: controller.signal })
          : Promise.resolve({ events: [] }),
      ]);
      if (refreshGenerationRef.current !== refreshGeneration) return;
      requireStatusRuntimeApiV1(statusRes);
      requireRuntimeApiV1(summaryRes);
      const nextAnalyticsRevision = runtimeSummaryRevision(summaryRes);
      analyticsSnapshotChanged = analyticsSummaryRevisionRef.current !== nextAnalyticsRevision;
      analyticsSummaryRevisionRef.current = nextAnalyticsRevision;
      setConfigDoc((previous) => (sameJSON(previous, confRes) ? previous : confRes));
      showMigrationNotice(confRes?.migrationNotice);
      setStatus((previous) => (sameJSON(previous, statusRes) ? previous : statusRes));
      const nextResetGeneration = Number(summaryRes?.resetGeneration || 0);
      const resetGenerationChanged = nextResetGeneration !== resetGenerationRef.current;
      setRuntime((previous) => {
        const nextRuntime = {
          ...(summaryRes?.counters || {}),
          summary: summaryRes,
          storage: summaryRes?.storage,
          resetGeneration: summaryRes?.resetGeneration || 0,
          // Statistics refreshes do not request the run-page event list;
          // never replace the live overlay with an empty compatibility page.
          recentEvents: shouldLoadEvents ? (eventsRes?.events || []) : (previous?.recentEvents || []),
          eventPage: shouldLoadEvents ? eventsRes : previous?.eventPage,
        };
        return sameJSON(previous, nextRuntime) ? previous : nextRuntime;
      });
      if (resetGenerationChanged) {
        lastChangeSeqRef.current = 0;
        setRuntimeEventDetail(null);
      }
      resetGenerationRef.current = nextResetGeneration;
      lastChangeSeqRef.current = Math.max(
        resetGenerationChanged ? 0 : lastChangeSeqRef.current,
        ...(eventsRes?.events || []).map((item) => Number(item?.changeSeq || 0)),
      );
      const facetsNeedRefresh = analyticsSnapshotChanged || analyticsFacetsRevisionRef.current === '';
      if (shouldLoadAnalytics && facetsNeedRefresh && analyticsGenerationRef.current === analyticsGeneration) {
        // Start facets after the first paint; do not await them in the core
        // Promise.all above. The visible board is driven by the summary
        // revision immediately; a slow GROUP BY for picker options must not
        // hold back trend/table replacement.
        void fetchAnalyticsFacets(analyticsGeneration, controller, { notifyWorkspace: false });
        if (analyticsSnapshotChanged) setAnalyticsRefreshSignal((current) => current + 1);
      }
    } catch (err) {
      if (refreshGenerationRef.current !== refreshGeneration) return;
      const message = err?.name === 'AbortError'
        ? '核心数据读取超时（10 秒），请检查 Admin 服务是否可用'
        : (err.message || '加载核心数据失败');
      setLastError(message);
      addToast(message, 'error');
      if (shouldLoadAnalytics && analyticsGenerationRef.current === analyticsGeneration) {
        setAnalyticsLoading(false);
        setAnalyticsError(message);
        setAnalyticsStale(true);
      }
    } finally {
      window.clearTimeout(timeout);
      if (coreAbortRef.current === controller) coreAbortRef.current = null;
      if (refreshGenerationRef.current === refreshGeneration) setIsLoading(false);
    }
  }, [addToast, auth.authenticated, currentRoute, fetchAnalyticsFacets, requireRuntimeApiV1, requireStatusRuntimeApiV1, showMigrationNotice]);

  const refreshRuntimeAggregates = useCallback(async () => {
    if (!auth.authenticated) return false;
    const refreshGeneration = ++refreshGenerationRef.current;
    const analyticsGeneration = ++analyticsGenerationRef.current;
    const shouldLoadAnalytics = currentRoute === 'statistics';
    try {
      const summaryRes = await api.getRuntimeSummary();
      if (refreshGenerationRef.current !== refreshGeneration) return false;
      requireRuntimeApiV1(summaryRes);
      const nextResetGeneration = Number(summaryRes?.resetGeneration || 0);
      if (nextResetGeneration !== resetGenerationRef.current) {
        await refreshCore();
        return false;
      }
      const nextAnalyticsRevision = runtimeSummaryRevision(summaryRes);
      const analyticsSnapshotChanged = analyticsSummaryRevisionRef.current !== nextAnalyticsRevision;
      analyticsSummaryRevisionRef.current = nextAnalyticsRevision;
      if (analyticsSnapshotChanged) {
        setRuntime((previous) => {
          const next = {
            ...(previous || {}),
            ...(summaryRes?.counters || {}),
            summary: summaryRes,
            storage: summaryRes?.storage,
            resetGeneration: nextResetGeneration,
          };
          return sameJSON(previous, next) ? previous : next;
        });
      }
      if (shouldLoadAnalytics && analyticsGenerationRef.current === analyticsGeneration && analyticsSnapshotChanged) {
        void fetchAnalyticsFacets(analyticsGeneration, null, { notifyWorkspace: false });
        setAnalyticsRefreshSignal((current) => current + 1);
      }
      return Number(summaryRes?.storage?.pendingEvents || 0) > 0;
    } catch (error) {
      if (refreshGenerationRef.current !== refreshGeneration) return false;
      const message = error?.message || '刷新运行统计失败';
      if (shouldLoadAnalytics && analyticsGenerationRef.current === analyticsGeneration) {
        setAnalyticsError(message);
        setAnalyticsStale(true);
      }
      return false;
    } finally {
    }
  }, [auth.authenticated, currentRoute, fetchAnalyticsFacets, refreshCore, requireRuntimeApiV1]);

  const scheduleRuntimeAggregatesRefresh = useCallback((delay = 1200) => {
    if (runtimeRefreshTimerRef.current != null) return;
    runtimeRefreshTimerRef.current = window.setTimeout(async () => {
      runtimeRefreshTimerRef.current = null;
      try {
        const stillPending = await refreshRuntimeAggregates();
        if (stillPending) scheduleRuntimeAggregatesRefresh(1500);
      } catch { /* the normal reconnect/refresh path will retry */ }
    }, delay);
  }, [refreshRuntimeAggregates]);

  useEffect(() => () => {
    if (runtimeRefreshTimerRef.current != null) {
      window.clearTimeout(runtimeRefreshTimerRef.current);
      runtimeRefreshTimerRef.current = null;
    }
  }, []);

  const loadRuntimeAnalytics = useCallback(async (
    range = analyticsRangeRef.current,
    filters = analyticsFiltersRef.current,
  ) => {
    const nextRange = String(range || 'today');
    const nextFilters = normalizeAnalyticsFilters(filters);
    analyticsRangeRef.current = nextRange;
    analyticsFiltersRef.current = nextFilters;
    setAnalyticsRange(nextRange);
    setAnalyticsFilters(nextFilters);
    const generation = ++analyticsGenerationRef.current;
    setAnalyticsLoading(true);
    setAnalyticsError(null);
    setAnalyticsStale(true);
    return fetchAnalyticsFacets(generation);
  }, [fetchAnalyticsFacets]);

  const loadRuntimeEvent = useCallback(async (id) => {
    const generation = ++eventDetailGenerationRef.current;
    if (!id) {
      setRuntimeEventDetail(null);
      return null;
    }
    try {
      const value = await api.getRuntimeEvent(id);
      if (eventDetailGenerationRef.current !== generation) return null;
      setRuntimeEventDetail(value);
      return value;
    } catch (error) {
      if (eventDetailGenerationRef.current !== generation) return null;
      throw error;
    }
  }, []);

  const deleteRuntimeSession = useCallback(async (sessionID, options = {}) => {
    const value = await api.deleteRuntimeSession(sessionID, options);
    setRuntimeEventDetail(null);
    await refreshCore();
    return value;
  }, [refreshCore]);

  const exportRuntimeSession = useCallback((sessionID) => api.exportRuntimeSession(sessionID), []);

  const reconcileRuntimeChanges = useCallback(async () => {
    // A zero cursor is not proof that there is nothing to reconcile: it can
    // mean a fresh daemon, an old event page, or a reset. Fetch the newest
    // page so the periodic fallback can recover even while SSE is open.
    if (lastChangeSeqRef.current <= 0) {
      await refreshCore();
      return 'core';
    }
    const result = await fetchRuntimeChanges({
      fetchPage: (params) => api.getRuntimeEvents(params),
      afterChangeSeq: lastChangeSeqRef.current,
      resetGeneration: resetGenerationRef.current,
    });
    if (!result.valid) {
      await refreshCore();
      return 'core';
    }
    // A heartbeat with no changes is not a state transition. Avoid replacing
    // the runtime object merely because a new empty array was returned.
    if (!result.changes?.length) return 'none';
    setRuntime((previous) => {
      const merged = mergeRuntimeListItems(previous?.recentEvents || [], result.changes);
      return {
        ...(previous || {}),
        recentEvents: merged,
        resetGeneration: result.resetGeneration ?? previous?.resetGeneration ?? 0,
        eventPage: previous?.eventPage
          ? {
            ...previous.eventPage,
            events: mergeRuntimeListItems(previous.eventPage.events || [], result.changes),
            resetGeneration: result.resetGeneration ?? previous.eventPage.resetGeneration,
          }
          : previous?.eventPage,
      };
    });
    if (result.resetGeneration != null) resetGenerationRef.current = Number(result.resetGeneration);
    lastChangeSeqRef.current = Math.max(lastChangeSeqRef.current, Number(result.lastChangeSeq || 0));
    return 'changes';
  }, [refreshCore]);

  const loadMoreRuntimeEvents = useCallback(async ({ pages = 5 } = {}) => {
    const initialEvents = runtime?.eventPage?.events || [];
    let beforeSeq = initialEvents.length ? initialEvents[initialEvents.length - 1]?.seq : undefined;
    let lastPage = null;
    const loadedEvents = [];
    const pageLimit = 200;
    const pageCount = Math.max(1, Math.min(10, Number(pages) || 5));

    for (let index = 0; index < pageCount; index += 1) {
      const page = await api.getRuntimeEvents({ beforeSeq, limit: pageLimit });
      lastPage = page;
      if (page?.resetGeneration != null
        && page.resetGeneration !== resetGenerationRef.current) {
        await refreshCore();
        return page;
      }
      loadedEvents.push(...(page.events || []));
      if (!page.hasMore || !page.events?.length) break;
      beforeSeq = page.events[page.events.length - 1]?.seq;
      if (beforeSeq == null) break;
    }

    const loadedByID = new Map();
    for (const item of loadedEvents) {
      const previous = loadedByID.get(item.id);
      if (!previous || Number(item.changeSeq || 0) >= Number(previous.changeSeq || 0)) {
        loadedByID.set(item.id, item);
      }
    }
    setRuntime((previous) => {
      const mergedByID = new Map((previous?.eventPage?.events || initialEvents).map((item) => [item.id, item]));
      for (const item of loadedByID.values()) {
        const existing = mergedByID.get(item.id);
        if (!existing || Number(item.changeSeq || 0) >= Number(existing.changeSeq || 0)) {
          mergedByID.set(item.id, item);
        }
      }
      const merged = [...mergedByID.values()].sort((left, right) => Number(right.seq || 0) - Number(left.seq || 0));
      return {
        ...(previous || {}),
        recentEvents: merged,
        resetGeneration: lastPage?.resetGeneration ?? previous?.resetGeneration ?? 0,
        eventPage: { ...(lastPage || {}), events: merged },
      };
    });
    return lastPage || { events: [], hasMore: false };
  }, [refreshCore, runtime?.eventPage?.events, runtime?.resetGeneration]);

  // Initial session check. Protected APIs must never be called before this resolves.
  useEffect(() => {
    let cancelled = false;
    api.getAuthSession()
      .catch(() => ({ authenticated: false }))
      .finally(() => { if (!cancelled) setAuthLoading(false); });
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    if (authLoading || !auth.authenticated) {
      streamRef.current?.close();
      streamRef.current = null;
      setStreamConnected(false);
      setIsLoading(false);
      return undefined;
    }
    refreshCore();
    streamRef.current?.close();
    streamRef.current = openEventStream({
      onRuntimeChange: (change) => {
        if (!change?.seq || !change?.changeSeq) {
          refreshCore();
          return;
        }
        lastChangeSeqRef.current = Math.max(lastChangeSeqRef.current, Number(change?.changeSeq || 0));
        setRuntime((previous) => ({
          ...(previous || {}),
          lastChangeSeq: change?.changeSeq,
          recentEvents: upsertRuntimeEvent(previous?.recentEvents, {
            ...(change?.event || change),
            seq: change?.seq,
            changeSeq: change?.changeSeq,
          }),
        }));
        // 详情接口可能已经提供了完整 trace；轻量实时更新不能把它覆盖掉。
        setRuntimeEventDetail((previous) => previous?.event?.id === change?.event?.id
          ? { ...previous, seq: change.seq, changeSeq: change.changeSeq }
          : previous);
        scheduleRuntimeAggregatesRefresh();
      },
      onConfigReloaded: () => refreshCore(),
      onConfigMigrated: (notice) => { showMigrationNotice(notice); refreshCore(); },
      onStatsReset: () => {
        lastChangeSeqRef.current = 0;
        resetGenerationRef.current = 0;
        setRuntimeEventDetail(null);
        setRuntime((previous) => previous ? { ...previous, recentEvents: [], eventPage: null, resetGeneration: 0 } : previous);
        refreshCore();
      },
      onProxyState: (next) => setStatus((previous) => ({ ...(previous || {}), ...next, running: Boolean(next?.running) })),
      onStatus: async (state) => {
        setStreamConnected(state === 'open');
        if (state !== 'open' || lastChangeSeqRef.current <= 0) return;
        try {
          await reconcileRuntimeChanges();
        } catch { refreshCore(); }
      },
    });
    return () => {
      streamRef.current?.close();
      streamRef.current = null;
      coreAbortRef.current?.abort();
      coreAbortRef.current = null;
      analyticsFacetAbortRef.current?.abort();
      analyticsFacetAbortRef.current = null;
    };
  }, [auth.authenticated, authLoading, reconcileRuntimeChanges, refreshCore, scheduleRuntimeAggregatesRefresh, showMigrationNotice]);

  useEffect(() => {
    if (!autoRefresh || !auth.authenticated || authLoading) return;
    const statisticsPage = currentRoute === 'statistics';
    const interval = statisticsPage ? statisticsRefreshInterval : runRefreshInterval;
    const timer = setInterval(() => {
      if (document.hidden) return; // Background throttle
      api.getStatus().then((nextStatus) => {
        requireStatusRuntimeApiV1(nextStatus);
        setStatus((previous) => (sameJSON(previous, nextStatus) ? previous : nextStatus));
      }).catch(() => {});
      if (statisticsPage && streamConnected) {
        reconcileRuntimeChanges()
          // reconcileRuntimeChanges may already have performed a full core
          // refresh after a reset/cursor mismatch. Do not immediately issue a
          // second aggregate read for that same tick.
          .then((result) => result === 'core' ? null : refreshRuntimeAggregates())
          .catch(() => refreshCore());
      } else if (streamConnected) {
        reconcileRuntimeChanges().catch(() => refreshCore());
      } else {
        refreshCore();
      }
    }, interval * 1000);
    return () => clearInterval(timer);
  }, [
    autoRefresh,
    auth.authenticated,
    authLoading,
    currentRoute,
    refreshCore,
    reconcileRuntimeChanges,
    refreshRuntimeAggregates,
    requireStatusRuntimeApiV1,
    runRefreshInterval,
    statisticsRefreshInterval,
    streamConnected,
  ]);

  // Keyboard shortcut for Command Palette (⌘K / Ctrl+K)
  useEffect(() => {
    const handleKeyDown = (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        e.preventDefault();
        setIsCommandOpen((prev) => !prev);
      }
      if (e.key === 'Escape') {
        setIsCommandOpen(false);
        setActiveModal(null);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  // Actions
  const toggleProxy = useCallback(async () => {
    if (!status) return;
    const willStart = !status.running;
    try {
      await api.toggleProxy(willStart);
      setStatus((prev) => ({ ...prev, running: willStart }));
      addToast(willStart ? '代理已成功启动' : '代理已成功停止', 'success');
      refreshCore();
    } catch (err) {
      addToast(`操作失败: ${err.message}`, 'error');
    }
  }, [status, addToast, refreshCore]);

  const saveConfig = useCallback((newConfig, secretUpdates = {}) => {
    // Serialize config writes in the browser. A function updater is resolved
    // against a freshly-read document, which makes targeted updates (catalog
    // status/models) merge-safe even while another page is editing settings.
    const queuedSource = configDocRef.current;
    const queuedGeneration = queuedSource?.generation;
    const queuedConfig = typeof newConfig === 'function' ? null : clone(newConfig);
    const operation = async () => {
      try {
        let base = configDocRef.current;
        let nextConfig = queuedConfig;
        if (typeof newConfig === 'function') {
          base = await api.getConfig();
          configDocRef.current = base;
          setConfigDoc((previous) => (sameJSON(previous, base) ? previous : base));
          nextConfig = await newConfig(clone(base?.config || {}), base);
        } else if (base?.generation !== queuedGeneration) {
          nextConfig = mergeConfigChanges(queuedSource?.config, nextConfig, base?.config);
        }
        const res = await api.saveConfig(base?.generation, nextConfig, secretUpdates);
        configDocRef.current = res;
        setConfigDoc(res);
        addToast('配置已成功保存并热生效', 'success');
        return true;
      } catch (err) {
        addToast(`保存配置失败: ${err.message}`, 'error');
        throw err;
      }
    };
    const result = configSaveQueueRef.current.then(operation, operation);
    // Keep the queue alive after a failed write while still returning the
    // original rejection to the caller.
    configSaveQueueRef.current = result.catch(() => undefined);
    return result;
  }, [addToast]);

  const login = useCallback(async (username, password) => {
    await api.login(username, password);
    await refreshCore();
  }, [refreshCore]);

  const logout = useCallback(async () => {
    await api.logout();
    setConfigDoc(null); setStatus(null); setRuntime(null); setRuntimeEventDetail(null); setDiagnostics(null);
  }, []);

  const value = {
    currentRoute,
    navigate,
    theme,
    toggleTheme: () => setTheme((prev) => (prev === 'dark' ? 'light' : 'dark')),
    configDoc,
    config: configDoc?.config,
    secretStatus: configDoc?.secretStatus,
    status,
    runtime,
    runtimeAnalytics,
    runtimeFacets,
    analyticsRefreshSignal,
    analyticsLoading,
    analyticsError,
    analyticsStale,
    runtimeEventDetail,
    analyticsRange,
    analyticsFilters,
    diagnostics,
    autostart,
    isLoading,
    lastError,
    auth,
    authLoading,
    login,
    logout,
    autoRefresh,
    setAutoRefresh,
    refreshIntervalOptions,
    runRefreshInterval,
    setRunRefreshInterval,
    statisticsRefreshInterval,
    setStatisticsRefreshInterval,
    streamConnected,
    toasts,
    addToast,
    removeToast,
    isCommandOpen,
    setIsCommandOpen,
    activeModal,
    openModal: setActiveModal,
    closeModal: () => setActiveModal(null),
    refreshCore,
    loadRuntimeAnalytics,
    loadRuntimeEvent,
    deleteRuntimeSession,
    exportRuntimeSession,
    loadMoreRuntimeEvents,
    toggleProxy,
    saveConfig,
  };

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}

export function useApp() {
  const context = useContext(AppContext);
  if (!context) throw new Error('useApp must be used within an AppProvider');
  return context;
}
