import { BoundedLRUCache, createPageCacheLifecycle, runtimePageActive } from '../utils/runtimeResources.js';
import { statisticsDateBounds, statisticsRangeIsLive } from '../utils/statisticsDateRange.js';
import { createContext, useMemo, useContext, useState, useEffect, useCallback, useRef } from 'react';
import { api, getAuthState, openEventStream, subscribeAuth } from '../services/api.js';
import { upsertRuntimeEvent, mergeRuntimeEvent } from '../utils/runtimeEvents.js';
import { fetchRuntimeChanges, mergeRuntimeListItems, reconcileRuntimeRefresh, runtimeSummaryRevision, RUNTIME_API_VERSION } from '../utils/runtimeSync.js';
import { clone } from '../utils/helpers.js';
import { configDraftWrite, requireDraftGeneration } from '../utils/configDraft.js';
import { createNavigationGuard } from '../utils/navigationGuard.js';

const AppContext = createContext(null);
const RuntimeContext = createContext(null);

function normalizeAnalyticsFilters(filters = {}) {
  return {
    customStartDay: String(filters.customStartDay || ''),
    customEndDay: String(filters.customEndDay || ''),
    inboundTokenID: String(filters.inboundTokenID || ''),
    clientKind: String(filters.clientKind || ''),
    requestKind: String(filters.requestKind || ''),
    clientVariant: String(filters.clientVariant || ''),
    agentRole: String(filters.agentRole || ''),
    agentName: String(filters.agentName || ''),
    parentThreadID: String(filters.parentThreadID || ''),
    parentTurnID: String(filters.parentTurnID || ''),
    rootTurnID: String(filters.rootTurnID || ''),
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
  run: 'sumpter-refresh-interval-run',
  statistics: 'sumpter-refresh-interval-statistics',
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

function sameJSON(left, right) {
  if (left === right) return true;
  try { return JSON.stringify(left) === JSON.stringify(right); } catch { return false; }
}

export function AppProvider({ children }) {
  // Navigation & Page State
  const [currentRoute, setCurrentRoute] = useState(() => {
    const hash = window.location.hash.replace(/^#/, '');
    const legacyProviderRoute = hash === 'providers-primary' || hash === 'providers-fallback';
    const validRoutes = ['run', 'providers', 'model-groups', 'routing', 'security', 'statistics', 'diagnostics', 'help', 'about'];
    return legacyProviderRoute ? 'providers' : (validRoutes.includes(hash) ? hash : 'run');
  });

  // Theme State
  const [theme, setTheme] = useState(() => {
    try {
      return localStorage.getItem('sumpter-theme') || 'dark';
    } catch {
      return 'dark';
    }
  });

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
  const [analyticsQueryRevision, setAnalyticsQueryRevision] = useState(0);
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const [analyticsError, setAnalyticsError] = useState(null);
  const [analyticsStale, setAnalyticsStale] = useState(false);
  const [runtimeEventDetail, setRuntimeEventDetail] = useState(null);
  const [pageVisible, setPageVisible] = useState(() => !document.hidden);
  const [runtimePagesRetained, setRuntimePagesRetained] = useState(true);
  const runtimePagePreferences = useRef(new Map());
  const runtimeDataActive = () => runtimePageActive(currentRouteRef.current, !document.hidden);
  const [analyticsRange, setAnalyticsRange] = useState('today');
  const [analyticsFilters, setAnalyticsFilters] = useState({ inboundTokenID: '', clientKind: '', requestKind: '', clientVariant: '', agentRole: '', agentName: '', parentThreadID: '', parentTurnID: '', rootTurnID: '', endpointID: '', project: '', projectID: '', sessionID: '', model: '', requestPurpose: '', outcome: '', failureKind: '', failurePhase: '' });
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
  const statusRequestRef = useRef(null);
  const aggregateAbortRef = useRef(null);
  const analyticsFacetAbortRef = useRef(null);
  const facetsPendingRef = useRef(false);
  const migrationNoticesRef = useRef(new Set());
  // These refs keep data requests independent from React render identity. In
  // particular, changing analytics filters must not recreate the SSE effect.
  const analyticsRangeRef = useRef(analyticsRange);
  const analyticsFiltersRef = useRef(analyticsFilters);
  // Complete core refreshes and lightweight aggregate refreshes have
  // independent lifecycles. A slow config/status read must not be discarded
  // just because an SSE heartbeat started a summary read in the meantime.
  const coreRefreshGenerationRef = useRef(0);
  const aggregateRefreshGenerationRef = useRef(0);
  const analyticsGenerationRef = useRef(0);
  const analyticsSummaryRevisionRef = useRef('');
  const analyticsFacetsRevisionRef = useRef('');
  const eventDetailGenerationRef = useRef(0);
  const eventDetailCacheRef = useRef(new BoundedLRUCache());
  const eventDetailAbortRef = useRef(null);
  const reconcileAbortRef = useRef(null);
  const aggregatePendingRef = useRef(false);
  const corePendingRef = useRef(false);
  const autoRefreshRef = useRef(true);
  const sseBatchRef = useRef(new Map());
  const sseBatchTimerRef = useRef(null);

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
    if (!runtimeDataActive() || currentRouteRef.current !== 'statistics') return Promise.resolve(null);
    if (options.coalesce && analyticsFacetAbortRef.current) {
      facetsPendingRef.current = true;
      return Promise.resolve(null);
    }
    facetsPendingRef.current = false;
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
        if (facetsPendingRef.current && runtimeDataActive() && currentRouteRef.current === 'statistics') {
          facetsPendingRef.current = false;
          void fetchAnalyticsFacets(++analyticsGenerationRef.current);
        }
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
  autoRefreshRef.current = autoRefresh;
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
  const currentRouteRef = useRef(currentRoute);
  const leaveGuardRef = useRef(null);
  if (!leaveGuardRef.current) leaveGuardRef.current = createNavigationGuard({
    confirmDiscard: message => window.confirm(message),
    notifyBusy: () => window.alert('正在保存，请等待完成后再离开。'),
  });
  const registerNavigationGuard = useCallback(read => leaveGuardRef.current.register(read), []);
  const canLeavePage = useCallback(() => leaveGuardRef.current.canLeave(), []);

  // Sync Theme to DOM
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
    try {
      localStorage.setItem('sumpter-theme', theme);
    } catch {
      // Theme still applies for this session when storage is unavailable.
    }
  }, [theme]);

  // Sync Hash to Route
  useEffect(() => {
    const handleHashChange = () => {
      const hash = window.location.hash.replace(/^#/, '');
      const route = hash === 'providers-primary' || hash === 'providers-fallback' ? 'providers' : hash;
      if (!['run', 'providers', 'model-groups', 'routing', 'security', 'statistics', 'diagnostics', 'help', 'about'].includes(route)
        || route === currentRouteRef.current) return;
      if (!leaveGuardRef.current.canLeave()) {
        window.history.replaceState(null, '', `#${currentRouteRef.current}`);
        return;
      }
      currentRouteRef.current = route;
      setCurrentRoute(route);
    };
    const handleBeforeUnload = event => {
      if (!leaveGuardRef.current.shouldBlockUnload()) return;
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('hashchange', handleHashChange);
    window.addEventListener('beforeunload', handleBeforeUnload);
    return () => {
      window.removeEventListener('hashchange', handleHashChange);
      window.removeEventListener('beforeunload', handleBeforeUnload);
    };
  }, []);

  // Set Route with Hash update
  const navigate = useCallback((route) => {
    if (route === currentRouteRef.current) return true;
    if (!leaveGuardRef.current.canLeave()) return false;
    currentRouteRef.current = route;
    window.location.hash = `#${route}`;
    setCurrentRoute(route);
    return true;
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
    const storageKey = `sumpter-migration-notice:${id}`;
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
        ? `配置已升级到 v${notice?.toSchema || 4}：${expanded} 个入口已转为“自动（四协议）”，请按上游能力复核`
        : `配置已安全升级到 v${notice?.toSchema || 4}`,
      'info',
      10000,
    );
  }, [addToast]);

  useEffect(() => subscribeAuth(setAuth), []);

  // Fetch Core Data
  // Polling and explicit refreshes share the same lightweight status read.
  const readStatus = useCallback(() => {
    if (statusRequestRef.current) return statusRequestRef.current.promise;
    const controller = new AbortController();
    const request = { controller, promise: null };
    const timeout = window.setTimeout(() => controller.abort(), 10_000);
    request.promise = api.getStatus({ signal: controller.signal }).finally(() => {
      window.clearTimeout(timeout);
      if (statusRequestRef.current === request) statusRequestRef.current = null;
    });
    statusRequestRef.current = request;
    return request.promise;
  }, []);

  const refreshCore = useCallback(async () => {
    if (!auth.authenticated) return;
    if (document.hidden && configDocRef.current) return;
    if (coreAbortRef.current) { corePendingRef.current = true; return; }
    const refreshGeneration = ++coreRefreshGenerationRef.current;
    const eventsCursorAtStart = lastChangeSeqRef.current;
    const shouldLoadAnalytics = !document.hidden && currentRouteRef.current === 'statistics';
    const shouldLoadEvents = !document.hidden && currentRouteRef.current === 'run';
    const shouldLoadRuntime = shouldLoadAnalytics || shouldLoadEvents;
    let analyticsSnapshotChanged = false;
    coreAbortRef.current?.abort();
    aggregateAbortRef.current?.abort();
    ++aggregateRefreshGenerationRef.current;
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
        readStatus(),
        shouldLoadRuntime ? api.getRuntimeSummary({ signal: controller.signal }) : Promise.resolve(null),
        shouldLoadEvents
          ? api.getRuntimeEvents({ limit: 10, signal: controller.signal })
          : Promise.resolve({ events: [] }),
      ]);
      if (coreRefreshGenerationRef.current !== refreshGeneration) return;
      requireStatusRuntimeApiV1(statusRes);
      setConfigDoc((previous) => (sameJSON(previous, confRes) ? previous : confRes));
      showMigrationNotice(confRes?.migrationNotice);
      setStatus((previous) => (sameJSON(previous, statusRes) ? previous : statusRes));
      if (!shouldLoadRuntime) return;
      requireRuntimeApiV1(summaryRes);
      const nextAnalyticsRevision = runtimeSummaryRevision(summaryRes);
      analyticsSnapshotChanged = analyticsSummaryRevisionRef.current !== nextAnalyticsRevision;
      analyticsSummaryRevisionRef.current = nextAnalyticsRevision;
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
          recentEvents: reconcileRuntimeRefresh(
            shouldLoadEvents ? (eventsRes?.events || []) : (previous?.recentEvents || []),
            summaryRes?.inFlightEvents,
            resetGenerationChanged ? [] : (previous?.recentEvents || []),
            eventsCursorAtStart,
          ),
          eventPage: shouldLoadEvents ? eventsRes : previous?.eventPage,
        };
        return sameJSON(previous, nextRuntime) ? previous : nextRuntime;
      });
      if (resetGenerationChanged) {
        lastChangeSeqRef.current = 0;
        eventDetailCacheRef.current.clear();
        setRuntimeEventDetail(null);
      }
      resetGenerationRef.current = nextResetGeneration;
      lastChangeSeqRef.current = Math.max(
        resetGenerationChanged ? 0 : lastChangeSeqRef.current,
        ...(eventsRes?.events || []).map((item) => Number(item?.changeSeq || 0)),
      );
      const facetsNeedRefresh = analyticsSnapshotChanged || analyticsFacetsRevisionRef.current === '';
      if (shouldLoadAnalytics && facetsNeedRefresh) {
        // Start facets after the first paint; do not await them in the core
        // Promise.all above. The visible board is driven by the summary
        // revision immediately; a slow GROUP BY for picker options must not
        // hold back trend/table replacement.
        const analyticsGeneration = ++analyticsGenerationRef.current;
        void fetchAnalyticsFacets(analyticsGeneration, controller, { notifyWorkspace: false });
        if (analyticsSnapshotChanged) setAnalyticsRefreshSignal((current) => current + 1);
      }
    } catch (err) {
      if (coreRefreshGenerationRef.current !== refreshGeneration) return;
      const message = err?.name === 'AbortError'
        ? '核心数据读取超时（10 秒），请检查 Admin 服务是否可用'
        : (err.message || '加载核心数据失败');
      setLastError(message);
      addToast(message, 'error');
      if (shouldLoadAnalytics) {
        setAnalyticsLoading(false);
        setAnalyticsError(message);
        setAnalyticsStale(true);
      }
    } finally {
      window.clearTimeout(timeout);
      if (coreAbortRef.current === controller) coreAbortRef.current = null;
      if (coreRefreshGenerationRef.current === refreshGeneration) setIsLoading(false);
      if (corePendingRef.current && !document.hidden) {
        corePendingRef.current = false;
        queueMicrotask(() => void refreshCore());
      }
    }
  }, [addToast, auth.authenticated, fetchAnalyticsFacets, readStatus, requireRuntimeApiV1, requireStatusRuntimeApiV1, showMigrationNotice]);

  const refreshRuntimeAggregates = useCallback(async () => {
    if (!auth.authenticated || !runtimeDataActive()) return false;
    if (currentRouteRef.current === 'statistics' && !statisticsRangeIsLive(analyticsRangeRef.current, analyticsFiltersRef.current)) return false;
    if (aggregateAbortRef.current) { aggregatePendingRef.current = true; return false; }
    const refreshGeneration = ++aggregateRefreshGenerationRef.current;
    aggregateAbortRef.current?.abort();
    const controller = new AbortController();
    aggregateAbortRef.current = controller;
    const timeout = window.setTimeout(() => controller.abort(), 10_000);
    const shouldLoadAnalytics = !document.hidden && currentRouteRef.current === 'statistics';
    try {
      const summaryRes = await api.getRuntimeSummary({ signal: controller.signal });
      if (aggregateRefreshGenerationRef.current !== refreshGeneration) return false;
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
      if (shouldLoadAnalytics && analyticsSnapshotChanged) {
        const analyticsGeneration = analyticsFacetAbortRef.current ? analyticsGenerationRef.current : ++analyticsGenerationRef.current;
        void fetchAnalyticsFacets(analyticsGeneration, controller, { notifyWorkspace: false, coalesce: true });
        setAnalyticsRefreshSignal((current) => current + 1);
      }
      return Number(summaryRes?.storage?.pendingEvents || 0) > 0;
    } catch (error) {
      if (aggregateRefreshGenerationRef.current !== refreshGeneration) return false;
      if (error?.name === 'AbortError') return false;
      const message = error?.message || '刷新运行统计失败';
      if (shouldLoadAnalytics) {
        setAnalyticsError(message);
        setAnalyticsStale(true);
      }
      return false;
    } finally {
      window.clearTimeout(timeout);
      if (aggregateAbortRef.current === controller) aggregateAbortRef.current = null;
      if (aggregatePendingRef.current && runtimeDataActive()) {
        aggregatePendingRef.current = false;
        queueMicrotask(() => void refreshRuntimeAggregates());
      }
    }
  }, [auth.authenticated, fetchAnalyticsFacets, refreshCore, requireRuntimeApiV1]);

  const scheduleRuntimeAggregatesRefresh = useCallback((delay = 1200) => {
    if (!runtimeDataActive() || !autoRefreshRef.current || runtimeRefreshTimerRef.current != null) return;
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
    if (nextRange === 'custom') statisticsDateBounds(nextFilters.customStartDay, nextFilters.customEndDay);
    analyticsRangeRef.current = nextRange;
    analyticsFiltersRef.current = nextFilters;
    setAnalyticsRange(nextRange);
    setAnalyticsFilters(nextFilters);
    setAnalyticsQueryRevision(value => value + 1);
    const generation = ++analyticsGenerationRef.current;
    setAnalyticsLoading(true);
    setAnalyticsError(null);
    setAnalyticsStale(true);
    return fetchAnalyticsFacets(generation);
  }, [fetchAnalyticsFacets]);

  const loadRuntimeEvent = useCallback(async (id, expectedChangeSeq = 0) => {
    const generation = ++eventDetailGenerationRef.current;
    eventDetailAbortRef.current?.abort();
    eventDetailAbortRef.current = null;
    if (!id || !runtimeDataActive()) {
      setRuntimeEventDetail(null);
      return null;
    }
    const cached = eventDetailCacheRef.current.get(id);
    if (cached && cached.event?.detailsOmitted !== true && Number(cached.changeSeq || 0) >= Number(expectedChangeSeq || 0)) {
      setRuntimeEventDetail(cached);
      return cached;
    }
    const controller = new AbortController();
    eventDetailAbortRef.current = controller;
    try {
      const fetched = await api.getRuntimeEvent(id, { signal: controller.signal });
      if (controller.signal.aborted || eventDetailGenerationRef.current !== generation) return null;
      const latest = eventDetailCacheRef.current.get(id);
      const value = latest && Number(latest.changeSeq || 0) > Number(fetched.changeSeq || 0) ? latest : fetched;
      eventDetailCacheRef.current.set(id, value);

      setRuntimeEventDetail(value);
      return value;
    } catch (error) {
      if (controller.signal.aborted || eventDetailGenerationRef.current !== generation) return null;
      throw error;
    } finally {
      if (eventDetailAbortRef.current === controller) eventDetailAbortRef.current = null;
    }
  }, []);

  const deleteRuntimeSession = useCallback(async (sessionID, options = {}) => {
    const value = await api.deleteRuntimeSession(sessionID, options);
    eventDetailCacheRef.current.clear();
    setRuntimeEventDetail(null);
    await refreshCore();
    return value;
  }, [refreshCore]);

  const exportRuntimeSession = useCallback((sessionID) => api.exportRuntimeSession(sessionID), []);

  const reconcileRuntimeChanges = useCallback(async () => {
    if (!runtimeDataActive() || !autoRefreshRef.current || currentRouteRef.current !== 'run') return 'none';
    if (reconcileAbortRef.current) return 'none';
    const controller = new AbortController();
    reconcileAbortRef.current = controller;
    try {
    // A zero cursor is not proof that there is nothing to reconcile: it can
    // mean a fresh daemon, an old event page, or a reset. Fetch the newest
    // page so the periodic fallback can recover even while SSE is open.
    if (lastChangeSeqRef.current <= 0) {
      await refreshCore();
      return 'core';
    }
    const result = await fetchRuntimeChanges({
      fetchPage: (params) => api.getRuntimeEvents({ ...params, signal: controller.signal }),
      afterChangeSeq: lastChangeSeqRef.current,
      resetGeneration: resetGenerationRef.current,
    });
    if (controller.signal.aborted || !runtimeDataActive()) return 'none';
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
    } finally {
      if (reconcileAbortRef.current === controller) reconcileAbortRef.current = null;
    }
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

  const cancelRuntimeDataRequests = useCallback(() => {
    statusRequestRef.current?.controller.abort();
    statusRequestRef.current = null;
    ++coreRefreshGenerationRef.current;
    ++aggregateRefreshGenerationRef.current;
    ++analyticsGenerationRef.current;
    ++eventDetailGenerationRef.current;
    for (const reference of [coreAbortRef, aggregateAbortRef, analyticsFacetAbortRef, eventDetailAbortRef, reconcileAbortRef]) {
      reference.current?.abort(); reference.current = null;
    }
    for (const reference of [runtimeRefreshTimerRef, sseBatchTimerRef]) {
      if (reference.current != null) window.clearTimeout(reference.current);
      reference.current = null;
    }
    corePendingRef.current = false;
    aggregatePendingRef.current = false;
    facetsPendingRef.current = false;
    sseBatchRef.current.clear();
    setAnalyticsLoading(false);
  }, []);

  useEffect(() => {
    const lifecycle = createPageCacheLifecycle({
      onVisibility: setPageVisible,
      onRetain: () => setRuntimePagesRetained(true),
      cancelRequests: cancelRuntimeDataRequests,
      release: () => {
        eventDetailCacheRef.current.clear();
        setRuntimeEventDetail(null);
        setRuntime(null);
        setRuntimeFacets(null);
        setRuntimeAnalytics(null);
        analyticsFacetsRevisionRef.current = '';
        analyticsSummaryRevisionRef.current = '';
        lastChangeSeqRef.current = 0;
        setRuntimePagesRetained(false);
      },
    });
    const update = () => lifecycle.update(!document.hidden);
    update();
    document.addEventListener('visibilitychange', update);
    return () => { lifecycle.dispose(); document.removeEventListener('visibilitychange', update); };
  }, [cancelRuntimeDataRequests]);

  const previousRuntimePage = useRef(`${currentRoute}:${pageVisible}`);
  useEffect(() => {
    const key = `${currentRoute}:${pageVisible}`;
    if (previousRuntimePage.current === key) return;
    previousRuntimePage.current = key;
    cancelRuntimeDataRequests();
    if (pageVisible && auth.authenticated && !authLoading) void refreshCore();
  }, [currentRoute, pageVisible, auth.authenticated, authLoading, refreshCore, cancelRuntimeDataRequests]);

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
      statusRequestRef.current?.controller.abort();
      statusRequestRef.current = null;
      setStreamConnected(false);
      setIsLoading(false);
      return undefined;
    }
    refreshCore();
    streamRef.current?.close();
    streamRef.current = openEventStream({
      onRuntimeChange: (change) => {
        if (!autoRefreshRef.current || !runtimeDataActive()) return;
        if (!change?.seq || !change?.changeSeq) { refreshCore(); return; }
        if (currentRouteRef.current !== 'run') { scheduleRuntimeAggregatesRefresh(); return; }
        lastChangeSeqRef.current = Math.max(lastChangeSeqRef.current, Number(change.changeSeq));
        const previous = sseBatchRef.current.get(change.event.id);
        if (!previous || previous.changeSeq < change.changeSeq) sseBatchRef.current.set(change.event.id, change);
        if (sseBatchTimerRef.current != null) return;
        sseBatchTimerRef.current = window.setTimeout(() => {
          sseBatchTimerRef.current = null;
          const batch = [...sseBatchRef.current.values()];
          sseBatchRef.current.clear();
          if (!runtimeDataActive() || !autoRefreshRef.current || currentRouteRef.current !== 'run') return;
          const changes = batch.map(change => ({ ...change.event, seq: change.seq, changeSeq: change.changeSeq }));
          setRuntime(previous => ({ ...(previous || {}), recentEvents: mergeRuntimeListItems(previous?.recentEvents || [], changes),
            lastChangeSeq: Math.max(...changes.map(change => change.changeSeq)) }));
          for (const change of batch) {
            const prior = eventDetailCacheRef.current.peek(change.event.id);
            const incoming = { ...change.event, seq: change.seq, changeSeq: change.changeSeq };
            if (prior && Number(prior.changeSeq || 0) > Number(change.changeSeq)) continue;
            const updated = { ...change, event: mergeRuntimeEvent(prior?.event, incoming) };
            if (prior) eventDetailCacheRef.current.set(change.event.id, updated);
            setRuntimeEventDetail(previous => previous?.event?.id === change.event.id ? updated : previous);
          }
          scheduleRuntimeAggregatesRefresh();
        }, 50);
      },
      onConfigReloaded: () => refreshCore(),
      onModelCatalogUpdated: () => refreshCore(),
      onModelMetadataUpdated: () => refreshCore(),
      onConfigMigrated: (notice) => { showMigrationNotice(notice); refreshCore(); },
      onStatsReset: () => {
        lastChangeSeqRef.current = 0;
        resetGenerationRef.current = 0;
        eventDetailCacheRef.current.clear();
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
      statusRequestRef.current?.controller.abort();
      statusRequestRef.current = null;
      coreAbortRef.current?.abort();
      coreAbortRef.current = null;
      aggregateAbortRef.current?.abort();
      aggregateAbortRef.current = null;
      analyticsFacetAbortRef.current?.abort();
      analyticsFacetAbortRef.current = null;
    };
  }, [auth.authenticated, authLoading, reconcileRuntimeChanges, refreshCore, scheduleRuntimeAggregatesRefresh, showMigrationNotice]);

  useEffect(() => {
    if (!autoRefresh || !auth.authenticated || authLoading) return;
    const statisticsPage = currentRoute === 'statistics';
    const interval = !pageVisible ? 15 : statisticsPage ? statisticsRefreshInterval : runRefreshInterval;
    const timer = setInterval(() => {
      readStatus().then((nextStatus) => {
        requireStatusRuntimeApiV1(nextStatus);
        setStatus((previous) => (sameJSON(previous, nextStatus) ? previous : nextStatus));
      }).catch(() => {});
      if (document.hidden || !runtimeDataActive()) return;
      if (statisticsPage && !statisticsRangeIsLive(analyticsRangeRef.current, analyticsFiltersRef.current)) return;
      if (statisticsPage && streamConnected) {
        reconcileRuntimeChanges()
          // reconcileRuntimeChanges may already have performed a full core
          // refresh after a reset/cursor mismatch. Do not immediately issue a
          // second aggregate read for that same tick.
          .then((result) => result === 'core' ? null : refreshRuntimeAggregates())
          .catch(() => refreshCore());
      } else if (streamConnected) {
        reconcileRuntimeChanges()
          .then((result) => result === 'core' || currentRoute !== 'run' ? null : refreshRuntimeAggregates())
          .catch(() => refreshCore());
      } else {
        refreshCore();
      }
    }, interval * 1000);
    return () => clearInterval(timer);
  }, [
    pageVisible,
    readStatus,
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
        if (document.querySelector('[role="dialog"][aria-modal="true"]:not(.command-palette-dialog)')) return;
        setIsCommandOpen((prev) => !prev);
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

  const saveConfig = useCallback((newConfig, secretUpdates = {}, sourceDocument = null) => {
    // 窄操作基于最新文档执行；完整草稿始终提交编辑时的 generation，冲突不覆盖草稿。
    const queuedDraft = typeof newConfig === 'function' ? null : configDraftWrite(newConfig, sourceDocument);
    const operation = async () => {
      try {
        let base = sourceDocument;
        let nextConfig = queuedDraft?.config;
        if (typeof newConfig === 'function') {
          base = await api.getConfig();
          requireDraftGeneration(sourceDocument, base);
          nextConfig = await newConfig(clone(base?.config || {}), base);
        }
        const res = await api.saveConfig(queuedDraft?.generation ?? base?.generation, nextConfig, secretUpdates);
        configDocRef.current = res;
        setConfigDoc(res);
        addToast('配置已成功保存并热生效', 'success');
        return res;
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
    if (!leaveGuardRef.current.canLeave()) return;
    await api.logout();
    eventDetailCacheRef.current.clear();
    setConfigDoc(null); setStatus(null); setRuntime(null); setRuntimeEventDetail(null); setDiagnostics(null);
  }, []);

  const value = useMemo(() => ({
    currentRoute,
    pageVisible,
    runtimePagesRetained,
    runtimePagePreferences,
    navigate,
    registerNavigationGuard,
    canLeavePage,
    theme,
    toggleTheme: () => setTheme((prev) => (prev === 'dark' ? 'light' : 'dark')),
    configDoc,
    config: configDoc?.config,
    secretStatus: configDoc?.secretStatus,
    status,
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
    toggleProxy,
    saveConfig,
  }), [currentRoute, pageVisible, runtimePagesRetained, navigate, theme, configDoc, status, diagnostics, autostart, isLoading, lastError, auth, authLoading, login, logout, autoRefresh, setAutoRefresh, refreshIntervalOptions, runRefreshInterval, setRunRefreshInterval, statisticsRefreshInterval, setStatisticsRefreshInterval, streamConnected, toasts, addToast, removeToast, isCommandOpen, setIsCommandOpen, activeModal, refreshCore, toggleProxy, saveConfig]);

  const runtimeValue = useMemo(() => ({
    runtime,
    runtimeAnalytics,
    runtimeFacets,
    analyticsRefreshSignal,
    analyticsQueryRevision,
    analyticsLoading,
    analyticsError,
    analyticsStale,
    runtimeEventDetail,
    analyticsRange,
    analyticsFilters,
    loadRuntimeAnalytics,
    loadRuntimeEvent,
    deleteRuntimeSession,
    exportRuntimeSession,
    loadMoreRuntimeEvents,
  }), [runtime, runtimeAnalytics, runtimeFacets, analyticsRefreshSignal, analyticsQueryRevision, analyticsLoading, analyticsError, analyticsStale, runtimeEventDetail, analyticsRange, analyticsFilters, loadRuntimeAnalytics, loadRuntimeEvent, deleteRuntimeSession, exportRuntimeSession, loadMoreRuntimeEvents]);

  return <AppContext.Provider value={value}><RuntimeContext.Provider value={runtimeValue}>{children}</RuntimeContext.Provider></AppContext.Provider>;
}

export function useAppControls() {
  const context = useContext(AppContext);
  if (!context) throw new Error('useApp must be used within an AppProvider');
  return context;
}

export function useApp() {
  const controls = useAppControls();
  const runtime = useContext(RuntimeContext);
  return { ...controls, ...runtime };
}

// Session-only UI preferences survive the release of bulky page results.
export function useRuntimePagePreference(key, initialValue) {
  const { runtimePagePreferences } = useAppControls();
  const [value, setValue] = useState(() => runtimePagePreferences.current.has(key)
    ? runtimePagePreferences.current.get(key) : (typeof initialValue === 'function' ? initialValue() : initialValue));
  useEffect(() => { runtimePagePreferences.current.set(key, value); }, [key, value, runtimePagePreferences]);
  return [value, setValue];
}
