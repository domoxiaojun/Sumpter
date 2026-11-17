import { useCallback, useEffect, useMemo, useState } from 'react';
import { BarChart3, ChevronRight, Download, Filter, RefreshCw, RotateCcw, Trash2 } from 'lucide-react';
import { AnalyticsWorkspace } from '../components/AnalyticsWorkspace.jsx';
import { EventInspector } from '../components/EventInspector.jsx';
import { Badge } from '../components/ui/badge.jsx';
import { Button } from '../components/ui/button.jsx';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card.jsx';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../components/ui/dialog.jsx';
import { Input } from '../components/ui/input.jsx';
import { Label } from '../components/ui/label.jsx';
import { NativeSelect } from '../components/ui/native-select.jsx';
import { PageHeader } from '../components/ui/page-header.jsx';
import { useApp, useRuntimePagePreference } from '../context/AppContext.jsx';
import { cn } from '../lib/utils.js';
import { api } from '../services/api.js';
import { copyWithToast } from '../utils/clipboard.js';
import {
  agentRoleLabel,
  clientKindLabel,
  clientVariantLabel,
  failureKindLabel,
  failurePhaseLabel,
  formatNumber,
  purposeLabel,
  requestKindLabel,
} from '../utils/helpers.js';
import { inboundTokenOptions } from '../utils/inboundTokens.js';
import { localDayString, statisticsDateBounds } from '../utils/statisticsDateRange.js';

const RANGE_OPTIONS = [
  ['today', '今天'],
  ['7d', '7 天'],
  ['30d', '30 天'],
  ['all', '全部'],
];

function projectLabel(value) {
  if (value === 'unidentified_project') return '未识别项目';
  if (value === 'multiple_workspaces') return '多工作区（未拆分）';
  return value;
}

function sessionLabel(value) {
  if (value === 'unidentified_session') return '未识别会话';
  const text = String(value || '');
  return text.length <= 32 ? text : `${text.slice(0, 14)}…${text.slice(-12)}`;
}

const APPLE_EPOCH_OFFSET_SECONDS = 978307200;

function cleanupCutoff(selection, customDate) {
  const now = Date.now() / 1000 - APPLE_EPOCH_OFFSET_SECONDS;
  if (selection === 'all') return now + 1;
  if (selection === 'custom') {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(customDate || '')) return null;
    const [year, month, day] = customDate.split('-').map(Number);
    const local = new Date(year, month - 1, day);
    if (local.getFullYear() !== year || local.getMonth() !== month - 1 || local.getDate() !== day) return null;
    return local.getTime() / 1000 - APPLE_EPOCH_OFFSET_SECONDS;
  }
  const days = Number(selection);
  return Number.isInteger(days) && days > 0 ? now - days * 86400 : null;
}

function RuntimeCleanupDialog({ open, onClose, onCompleted, addToast }) {
  const [selection, setSelection] = useState('30');
  const [customDate, setCustomDate] = useState('');
  const [preview, setPreview] = useState(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (open) {
      setSelection('30');
      setCustomDate('');
      setPreview(null);
      setError('');
    }
  }, [open]);

  const olderThan = cleanupCutoff(selection, customDate);
  const canPreview = olderThan != null && !loading;
  const runPreview = async () => {
    if (olderThan == null) return;
    setLoading(true);
    setError('');
    setPreview(null);
    try {
      setPreview(await api.previewRuntimeCleanup({ olderThan }));
    } catch (nextError) {
      setError(nextError?.message || '无法预览清理范围');
    } finally {
      setLoading(false);
    }
  };
  const runCleanup = async () => {
    if (!preview || olderThan == null || loading) return;
    setLoading(true);
    setError('');
    try {
      const result = await api.cleanupRuntime({ olderThan });
      addToast(`已按时间清理 ${formatNumber(result.deletedEvents || 0)} 条统计事件`, 'success');
      onClose?.();
      await onCompleted?.();
    } catch (nextError) {
      setError(nextError?.message || '清理失败');
    } finally {
      setLoading(false);
    }
  };
  const nowDate = new Date();
  const today = [
    nowDate.getFullYear(),
    String(nowDate.getMonth() + 1).padStart(2, '0'),
    String(nowDate.getDate()).padStart(2, '0'),
  ].join('-');

  return (
    <Dialog
      open={Boolean(open)}
      onOpenChange={(nextOpen) => {
        if (!nextOpen && !loading) onClose?.();
      }}
    >
      <DialogContent
        showCloseButton={!loading}
        aria-busy={loading || undefined}
        className="sm:max-w-lg"
        onEscapeKeyDown={(event) => {
          if (loading) event.preventDefault();
        }}
        onInteractOutside={(event) => {
          if (loading) event.preventDefault();
        }}
      >
        <DialogHeader className="border-b border-border/70 pb-3.5">
          <DialogTitle id="runtime-cleanup-heading">按时间清理运行统计</DialogTitle>
          <DialogDescription>
            只删除已完成且整组早于截止时间的请求；进行中的请求组会完整保留。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 text-xs">
          <label className="block space-y-1.5">
            <span className="font-medium text-foreground">清理范围</span>
            <NativeSelect
              value={selection}
              onChange={(event) => {
                setSelection(event.target.value);
                setPreview(null);
                setError('');
              }}
              disabled={loading}
            >
              <option value="7">早于 7 天</option>
              <option value="30">早于 30 天</option>
              <option value="90">早于 90 天</option>
              <option value="custom">自定义日期</option>
              <option value="all">全部已完成记录</option>
            </NativeSelect>
          </label>

          {selection === 'custom' && (
            <label className="block space-y-1.5">
              <span className="font-medium text-foreground">清理早于此日期的记录</span>
              <Input
                type="date"
                value={customDate}
                max={today}
                onChange={(event) => {
                  setCustomDate(event.target.value);
                  setPreview(null);
                  setError('');
                }}
                disabled={loading}
              />
            </label>
          )}

          <p className="leading-relaxed text-muted-foreground">
            将删除截止时间之前的统计事件。诊断捕获、自动保留策略和数据库结构不会受影响。
          </p>

          {selection === 'all' && (
            <div className="rounded-xl border border-status-warning/30 bg-status-warning/10 p-3 text-status-warning" role="note">
              “全部”也不会删除进行中的请求；如需移除旧版字段或回收数据库文件，请单独使用“重置并新建数据库”。
            </div>
          )}

          {preview && (
            <div className="rounded-xl border border-primary/30 bg-primary/10 p-3.5" role="status">
              <strong className="block font-semibold text-foreground">
                预计删除 {formatNumber(preview.deletableEvents)} 条事件
              </strong>
              <span className="mt-0.5 block text-muted-foreground">
                涉及约 {formatNumber(preview.deletableRequests)} 个请求 · 清理后保留 {formatNumber(preview.remainingEvents)} 条事件
              </span>
            </div>
          )}

          {error && (
            <div className="rounded-xl border border-status-critical/30 bg-status-critical/10 p-3 text-status-critical" role="alert">
              {error}
            </div>
          )}
        </div>

        <DialogFooter className="border-t border-border/70 pt-3.5">
          <Button type="button" variant="outline" size="sm" onClick={onClose} disabled={loading}>
            取消
          </Button>
          {!preview ? (
            <Button type="button" size="sm" onClick={runPreview} disabled={!canPreview}>
              {loading ? '计算中…' : '预览清理范围'}
            </Button>
          ) : (
            <Button
              type="button"
              variant="destructive"
              size="sm"
              onClick={runCleanup}
              disabled={loading || preview.deletableEvents <= 0}
            >
              {loading ? '清理中…' : `确认清理 ${formatNumber(preview.deletableEvents)} 条`}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function facetOptions(analytics, key, selectedValue) {
  const values = analytics?.facets?.[key];
  const options = Array.isArray(values) ? [...values] : [];
  const selected = String(selectedValue || '').trim();
  if (selected && !options.some((item) => String(item?.value || '') === selected)) {
    // A facet is allowed to exclude its own active filter. Keep the selected
    // value visible so a user can move to another filter or clear it without
    // having to return to “全部” first.
    options.unshift({ value: selected, count: 0, selected: true });
  }
  return options;
}

function facetOptionLabel(item, kind) {
  const value = String(item?.value || '').trim();
  const label = kind === 'purpose'
    ? purposeLabel(value)
    : kind === 'failureKind'
      ? failureKindLabel(value)
      : kind === 'failurePhase'
        ? failurePhaseLabel(value)
        : value;
  const count = Number.isFinite(Number(item?.count)) ? ` · ${formatNumber(item.count)}` : '';
  return `${label || '未记录'}${count}`;
}

function endpointOptions(runtime, config, analytics, selectedValue) {
  const rows = new Map();
  (config?.endpoints || []).forEach((endpoint) => {
    if (endpoint?.id) rows.set(String(endpoint.id), { value: String(endpoint.id), label: endpoint.name || endpoint.id, count: 0 });
  });
  (runtime?.recentEvents || []).forEach((event) => {
    const value = String(event?.endpointID || '').trim();
    if (!value) return;
    const current = rows.get(value) || { value, label: event.endpointName || event.endpoint || value, count: 0 };
    current.count += 1;
    if (!current.label || current.label === value) current.label = event.endpointName || event.endpoint || value;
    rows.set(value, current);
  });
  // Compatibility analytics rows from newer daemons carry endpointID. The
  // lightweight v3 facets response nests them under `facets`; older daemons
  // exposed the same rows at the top level. Merge both shapes into the
  // configured options so a historical entry is never hidden just because it
  // is outside the short runtime event window.
  const endpointFacetRows = analytics?.facets?.endpoints || analytics?.endpoints || [];
  endpointFacetRows.forEach((row) => {
    const label = String(row?.name || row?.label || '').trim();
    const endpointID = String(row?.endpointID || row?.endpointId || row?.value || '').trim();
    if (endpointID) {
      const current = rows.get(endpointID) || { value: endpointID, label: label || endpointID, count: 0 };
      current.label = current.label === endpointID && label ? label : current.label;
      current.count = Math.max(current.count, Number(row?.attempts ?? row?.count ?? 0));
      rows.set(endpointID, current);
      return;
    }
    // Older daemons may only expose endpoint names. Keep a name-valued
    // fallback rather than hiding a dimension the server did return.
    if (label && ![...rows.values()].some((item) => item.label === label)) {
      rows.set(label, { value: label, label, count: Number(row?.attempts ?? row?.count ?? 0) });
    }
  });
  const selected = String(selectedValue || '').trim();
  if (selected && !rows.has(selected)) rows.set(selected, { value: selected, label: selected, count: 0 });
  return [...rows.values()].sort((left, right) => left.label.localeCompare(right.label, 'zh-CN'));
}

/**
 * Statistics is intentionally a single database-backed workspace. The old
 * recent-event summary duplicated the same counters from a bounded browser
 * sample and could disagree with the SQLite snapshot shown below it.
 */
export function StatsPage() {
  const {
    runtime,
    pageVisible,
    config,
    secretStatus,
    runtimeAnalytics,
    runtimeFacets,
    analyticsRefreshSignal,
    analyticsQueryRevision,
    runtimeEventDetail,
    analyticsRange,
    analyticsFilters,
    analyticsLoading,
    analyticsError,
    analyticsStale,
    loadRuntimeAnalytics,
    loadRuntimeEvent,
    refreshCore,
    addToast,
  } = useApp();

  const [customDateEditor, setCustomDateEditor] = useRuntimePagePreference('stats.customEditor', false);
  const [customStartDay, setCustomStartDay] = useRuntimePagePreference('stats.customStart', localDayString);
  const [customEndDay, setCustomEndDay] = useRuntimePagePreference('stats.customEnd', localDayString);
  const [customDateError, setCustomDateError] = useState('');
  const [selectedEventID, setSelectedEventID] = useState(null);
  const [expandedGroups, setExpandedGroups] = useState({});
  const [eventChain, setEventChain] = useState([]);
  const [eventDetailError, setEventDetailError] = useState('');
  const [openExportSignal, setOpenExportSignal] = useState(0);
  const [cleanupOpen, setCleanupOpen] = useState(false);
  const [selectedProject, setSelectedProject] = useState(null);
  const [selectedSession, setSelectedSession] = useState(null);
  const [projectSelectionResetSignal, setProjectSelectionResetSignal] = useState(0);

  useEffect(() => {
    if (!selectedEventID || !pageVisible) {
      loadRuntimeEvent(null);
      setEventDetailError('');
      return;
    }
    let active = true;
    setEventDetailError('');
    loadRuntimeEvent(selectedEventID).catch((error) => {
      if (active) setEventDetailError(error?.message || '无法读取事件详情');
    });
    return () => { active = false; };
  }, [loadRuntimeEvent, selectedEventID, pageVisible]);

  const filters = analyticsFilters || {
    inboundTokenID: '',
    clientKind: '',
    requestKind: '',
    clientVariant: '',
    agentRole: '',
    agentName: '',
    parentThreadID: '',
    parentTurnID: '',
    rootTurnID: '',
    endpointID: '',
    project: '',
    projectID: '',
    sessionID: '',
    model: '',
    requestPurpose: '',
    outcome: '',
    failureKind: '',
    failurePhase: '',
  };

  const advancedFilterLabels = [
    ['requestKind', '请求类型'],
    ['clientVariant', '客户端形态'],
    ['agentRole', '代理角色'],
    ['agentName', '代理名称'],
    ['parentThreadID', '父线程'],
    ['parentTurnID', '父回合'],
    ['rootTurnID', '根回合'],
    ['requestPurpose', '用途'],
    ['failureKind', '失败类型'],
    ['failurePhase', '失败阶段'],
  ].filter(([key]) => filters[key]).map(([key, label]) => `${label}=${filters[key]}`);

  const activeFilterCount = [
    filters.inboundTokenID,
    filters.clientKind,
    filters.requestKind,
    filters.clientVariant,
    filters.agentRole,
    filters.agentName,
    filters.parentThreadID,
    filters.parentTurnID,
    filters.rootTurnID,
    filters.endpointID,
    filters.projectID,
    filters.project,
    filters.sessionID,
    filters.model,
    filters.requestPurpose,
    filters.outcome,
    filters.failureKind,
    filters.failurePhase,
    selectedProject?.key,
    selectedSession?.key,
  ].filter((value) => String(value || '').trim()).length;

  const facets = useMemo(() => ({
    inboundTokens: inboundTokenOptions(config, runtimeFacets?.facets, filters.inboundTokenID, secretStatus?.inboundAuthToken?.configured),
    clientKinds: facetOptions(runtimeFacets, 'clientKinds', filters.clientKind),
    requestKinds: facetOptions(runtimeFacets, 'requestKinds', filters.requestKind),
    clientVariants: facetOptions(runtimeFacets, 'clientVariants', filters.clientVariant),
    agentRoles: facetOptions(runtimeFacets, 'agentRoles', filters.agentRole),
    agentNames: facetOptions(runtimeFacets, 'agentNames', filters.agentName),
    parentThreads: facetOptions(runtimeFacets, 'parentThreads', filters.parentThreadID),
    parentTurns: facetOptions(runtimeFacets, 'parentTurns', filters.parentTurnID),
    rootTurns: facetOptions(runtimeFacets, 'rootTurns', filters.rootTurnID),
    endpoints: endpointOptions(runtime, config, runtimeFacets, filters.endpointID),
    projects: facetOptions(runtimeFacets, 'projects', filters.project),
    sessions: facetOptions(runtimeFacets, 'sessions', filters.sessionID),
    models: facetOptions(runtimeFacets, 'models', filters.model),
    requestPurposes: facetOptions(runtimeFacets, 'requestPurposes', filters.requestPurpose),
    failureKinds: facetOptions(runtimeFacets, 'failureKinds', filters.failureKind),
    failurePhases: facetOptions(runtimeFacets, 'failurePhases', filters.failurePhase),
  }), [
    config,
    secretStatus,
    runtime,
    runtimeFacets,
    filters.inboundTokenID,
    filters.clientKind,
    filters.requestKind,
    filters.clientVariant,
    filters.agentRole,
    filters.agentName,
    filters.parentThreadID,
    filters.parentTurnID,
    filters.rootTurnID,
    filters.endpointID,
    filters.project,
    filters.sessionID,
    filters.model,
    filters.requestPurpose,
    filters.failureKind,
    filters.failurePhase,
  ]);

  const updateFilter = (key, value) => {
    const next = {
      ...filters,
      [key]: value,
      ...(key === 'project' ? { projectID: '', sessionID: '' } : {}),
    };
    loadRuntimeAnalytics(analyticsRange, next).catch(() => null);
  };

  const handleProjectSelectionChange = useCallback((project) => {
    setSelectedProject(project || null);
  }, []);

  const handleSessionSelectionChange = useCallback((session) => {
    setSelectedSession(session || null);
  }, []);

  const clearLocalDrilldowns = useCallback(() => {
    setProjectSelectionResetSignal((value) => value + 1);
    setSelectedProject(null);
    setSelectedSession(null);
  }, []);

  const clearAllFilters = () => {
    clearLocalDrilldowns();
    loadRuntimeAnalytics(analyticsRange, {
      inboundTokenID: '',
      clientKind: '',
      requestKind: '',
      clientVariant: '',
      agentRole: '',
      agentName: '',
      parentThreadID: '',
      parentTurnID: '',
      rootTurnID: '',
      endpointID: '',
      project: '',
      projectID: '',
      sessionID: '',
      model: '',
      requestPurpose: '',
      outcome: '',
      failureKind: '',
      failurePhase: '',
    }).catch(() => null);
  };

  const recreateDatabase = async () => {
    if (!window.confirm('将删除全部 SQLite 运行统计，并用当前版本重新创建 runtime.sqlite3。旧版自动清理字段将被移除，诊断捕获不受影响，此操作无法撤销。')) return;
    try {
      await api.recreateRuntime();
      clearLocalDrilldowns();
      await refreshCore();
      addToast('已重置并新建 SQLite 数据库', 'success');
    } catch (error) {
      addToast(`新建数据库失败：${error?.message || '未知错误'}`, 'error');
    }
  };

  const selectedEvent = runtimeEventDetail?.event?.id === selectedEventID ? runtimeEventDetail.event : null;
  const requestID = selectedEvent?.requestID;

  useEffect(() => {
    setEventChain([]);
    if (!requestID || !pageVisible) return undefined;
    const controller = new AbortController();
    api.getRuntimeRequestChain(requestID, { signal: controller.signal }).then((value) => {
      if (!controller.signal.aborted) setEventChain(value.events || []);
    }).catch(() => {});
    return () => controller.abort();
  }, [requestID, pageVisible]);

  return (
    <div className="space-y-6">
      <PageHeader
        icon={BarChart3}
        title={<span>统计</span>}
        description="按请求用途、入口、项目、会话与模型查看成功率、延迟与成本。"
        actions={(
          <>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => loadRuntimeAnalytics(analyticsRange, filters).catch((error) => addToast(error.message, 'error'))}
            >
              <RefreshCw className={cn('size-3.5', analyticsLoading && 'animate-spin')} />
              <span>刷新</span>
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setOpenExportSignal((value) => value + 1)}
            >
              <Download className="size-3.5" />
              <span>导出</span>
            </Button>
          </>
        )}
      />

      {runtime?.startupIssue && (
        <div
          className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-status-critical/35 bg-status-critical/10 p-4 text-xs text-status-critical"
          role="alert"
        >
          <div className="space-y-0.5">
            <strong className="block text-sm font-semibold">运行统计数据库需要重建</strong>
            <p>{runtime.startupIssue.message || '检测到旧版 runtime 数据库，代理暂未启动。'}</p>
          </div>
          <Button type="button" variant="destructive" size="sm" onClick={recreateDatabase}>
            <Trash2 className="size-3.5" />
            <span>清空并重建数据库</span>
          </Button>
        </div>
      )}

      <Card className="gap-4 py-4" aria-labelledby="analytics-v3-filter-heading">
        <CardHeader className="flex flex-col gap-3 border-b border-border/70 pb-3.5 lg:flex-row lg:items-center lg:justify-between">
          <div className="space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <Filter className="size-4 text-primary" />
              <CardTitle id="analytics-v3-filter-heading">范围与筛选</CardTitle>
              {activeFilterCount > 0 && <Badge variant="default">{activeFilterCount} 项筛选</Badge>}
            </div>
            <CardDescription>按所选时间范围展示数据；常用条件直接选择，其他条件在高级筛选中展开。</CardDescription>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <div
              className={cn(
                'inline-flex items-center gap-1.5 rounded-full border border-border/75 bg-muted/50 px-3 py-1 text-xs',
                analyticsError ? 'border-status-warning/35 bg-status-warning/10 font-medium text-status-warning' : 'text-muted-foreground',
              )}
              role={analyticsError ? 'alert' : 'status'}
              aria-live="polite"
            >
              {analyticsLoading && <span className="size-1.5 rounded-full bg-primary animate-pulse" aria-hidden="true" />}
              <span>
                {analyticsError
                  ? `统计刷新失败：${analyticsError}${analyticsStale ? '；当前保留上一份结果。' : ''}`
                  : analyticsLoading ? '正在更新统计…' : activeFilterCount ? `已应用 ${activeFilterCount} 项筛选` : '显示全部项目'}
              </span>
            </div>

            {activeFilterCount > 0 && (
              <Button
                type="button"
                variant="outline"
                size="xs"
                onClick={clearAllFilters}
              >
                <RotateCcw className="size-3" />
                <span>清除筛选</span>
              </Button>
            )}
          </div>
        </CardHeader>

        <CardContent className="space-y-3.5">
          <div
            id="analytics-v3-filter-grid"
            className="analytics-v3-filter-grid grid gap-3 sm:grid-cols-2 lg:grid-cols-4 xl:grid-cols-7"
          >
            <fieldset className="space-y-2 sm:col-span-2 lg:col-span-4 xl:col-span-7" aria-label="统计时间范围">
              <legend className="sr-only">时间</legend>
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-xs font-medium text-muted-foreground">时间范围</span>
                <div className="inline-flex flex-wrap items-center gap-0.5 rounded-xl border border-border/75 bg-muted/65 p-1">
                  {RANGE_OPTIONS.map(([value, label]) => {
                    const active = analyticsRange === value && !customDateEditor;
                    return (
                      <button
                        key={value}
                        type="button"
                        aria-pressed={analyticsRange === value}
                        onClick={() => {
                          setCustomDateEditor(false);
                          setCustomDateError('');
                          loadRuntimeAnalytics(value, filters).catch(() => null);
                        }}
                        className={cn(
                          'cursor-pointer rounded-lg px-3 py-1 text-xs font-medium transition-all duration-150 outline-none focus-visible:ring-2 focus-visible:ring-ring/40',
                          active
                            ? 'bg-card font-semibold text-foreground shadow-xs'
                            : 'text-muted-foreground hover:text-foreground',
                        )}
                      >
                        {label}
                      </button>
                    );
                  })}
                  <button
                    type="button"
                    aria-pressed={customDateEditor}
                    onClick={() => setCustomDateEditor(true)}
                    className={cn(
                      'cursor-pointer rounded-lg px-3 py-1 text-xs font-medium transition-all duration-150 outline-none focus-visible:ring-2 focus-visible:ring-ring/40',
                      customDateEditor
                        ? 'bg-card font-semibold text-foreground shadow-xs'
                        : 'text-muted-foreground hover:text-foreground',
                    )}
                  >
                    自定义
                  </button>
                </div>
              </div>
              {customDateEditor && (
                <div className="mt-2 flex flex-wrap items-end gap-3 rounded-2xl border border-border/80 bg-muted/25 p-3.5">
                  <Label className="space-y-1.5">
                    <span className="block">开始日期</span>
                    <Input
                      type="date"
                      className="h-8 w-auto text-xs"
                      max={localDayString()}
                      value={customStartDay}
                      onChange={(event) => {
                        setCustomStartDay(event.target.value);
                        setCustomDateError('');
                      }}
                    />
                  </Label>
                  <Label className="space-y-1.5">
                    <span className="block">结束日期</span>
                    <Input
                      type="date"
                      className="h-8 w-auto text-xs"
                      max={localDayString()}
                      value={customEndDay}
                      onChange={(event) => {
                        setCustomEndDay(event.target.value);
                        setCustomDateError('');
                      }}
                    />
                  </Label>
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => {
                      try {
                        statisticsDateBounds(customStartDay, customEndDay);
                        setCustomDateError('');
                        loadRuntimeAnalytics('custom', { ...filters, customStartDay, customEndDay }).catch((error) => setCustomDateError(error.message));
                      } catch (error) {
                        setCustomDateError(error.message);
                      }
                    }}
                  >
                    应用
                  </Button>
                  <small className="text-[11px] text-muted-foreground">
                    {analyticsRange === 'custom' ? `已应用 ${filters.customStartDay} 至 ${filters.customEndDay} · ` : '选择日期后点击应用 · '}
                    包含结束当天 · 本地时区 {Intl.DateTimeFormat().resolvedOptions().timeZone}
                  </small>
                  {customDateError && <span role="alert" className="text-xs font-medium text-status-critical">{customDateError}</span>}
                </div>
              )}
            </fieldset>

            <label className="space-y-1">
              <span className="block text-[11px] font-medium text-muted-foreground">入口</span>
              <NativeSelect size="sm" value={filters.endpointID || ''} onChange={(event) => updateFilter('endpointID', event.target.value)}>
                <option value="">全部</option>
                {facets.endpoints.map((item) => (
                  <option key={item.value} value={item.value}>{item.label} · {formatNumber(item.count)}</option>
                ))}
              </NativeSelect>
            </label>

            <label className="space-y-1">
              <span className="block text-[11px] font-medium text-muted-foreground">项目</span>
              <NativeSelect size="sm" value={filters.project || ''} onChange={(event) => updateFilter('project', event.target.value)}>
                <option value="">全部</option>
                {facets.projects.map((item) => (
                  <option key={item.value} value={item.value}>{projectLabel(item.value)} · {formatNumber(item.count)}</option>
                ))}
              </NativeSelect>
            </label>

            <label className="space-y-1">
              <span className="block text-[11px] font-medium text-muted-foreground">会话</span>
              <NativeSelect size="sm" value={filters.sessionID || ''} onChange={(event) => updateFilter('sessionID', event.target.value)}>
                <option value="">全部</option>
                {facets.sessions.map((item) => (
                  <option key={item.value} value={item.value}>{sessionLabel(item.value)} · {formatNumber(item.count)}</option>
                ))}
              </NativeSelect>
            </label>

            <label className="space-y-1">
              <span className="block text-[11px] font-medium text-muted-foreground">入站 Token</span>
              <NativeSelect
                size="sm"
                aria-label="入站 Token"
                value={filters.inboundTokenID || ''}
                onChange={(e) => updateFilter('inboundTokenID', e.target.value)}
              >
                <option value="">全部 Token</option>
                {facets.inboundTokens.map((item) => (
                  <option key={item.value} value={item.value}>{item.label} · {formatNumber(item.count)}</option>
                ))}
              </NativeSelect>
            </label>

            <label className="space-y-1">
              <span className="block text-[11px] font-medium text-muted-foreground">客户端</span>
              <NativeSelect size="sm" value={filters.clientKind || ''} onChange={(event) => updateFilter('clientKind', event.target.value)}>
                <option value="">全部</option>
                {facets.clientKinds.map((item) => (
                  <option key={item.value} value={item.value}>{clientKindLabel(item.value)} · {formatNumber(item.count)}</option>
                ))}
              </NativeSelect>
            </label>

            <label className="space-y-1">
              <span className="block text-[11px] font-medium text-muted-foreground">模型</span>
              <NativeSelect size="sm" value={filters.model || ''} onChange={(event) => updateFilter('model', event.target.value)}>
                <option value="">全部</option>
                {facets.models.map((item) => (
                  <option key={item.value} value={item.value}>{facetOptionLabel(item, 'model')}</option>
                ))}
              </NativeSelect>
            </label>

            <label className="space-y-1">
              <span className="block text-[11px] font-medium text-muted-foreground">最终结果</span>
              <NativeSelect size="sm" value={filters.outcome || ''} onChange={(event) => updateFilter('outcome', event.target.value)}>
                <option value="">全部</option>
                <option value="succeeded">成功</option>
                <option value="failed">失败</option>
                <option value="cancelled">已取消</option>
              </NativeSelect>
            </label>
          </div>

          <details className="analytics-v3-advanced-filters group overflow-hidden rounded-xl border border-border/80 bg-muted/20">
            <summary className="flex cursor-pointer list-none items-center gap-2 px-3.5 py-2.5 text-xs font-medium text-foreground select-none transition-colors hover:bg-muted/40">
              <ChevronRight className="size-3.5 text-muted-foreground transition-transform duration-150 group-open:rotate-90" />
              <span>高级筛选{advancedFilterLabels.length > 0 ? ` · 已选 ${advancedFilterLabels.length} 项` : ' · 代理、用途与失败诊断'}</span>
            </summary>
            <div className="grid gap-3 border-t border-border/65 p-3.5 sm:grid-cols-2 lg:grid-cols-4 xl:grid-cols-5">
              {[
                ['requestKind', '请求类型', 'requestKinds'],
                ['clientVariant', '客户端形态', 'clientVariants'],
                ['agentRole', '代理角色', 'agentRoles'],
                ['agentName', '代理名称', 'agentNames'],
                ['parentThreadID', '父线程', 'parentThreads'],
                ['parentTurnID', '父回合', 'parentTurns'],
                ['rootTurnID', '根回合', 'rootTurns'],
              ].map(([key, label, facetKey]) => (
                <label key={key} className="space-y-1">
                  <span className="block text-[11px] font-medium text-muted-foreground">{label}</span>
                  <NativeSelect size="sm" value={filters[key] || ''} onChange={(event) => updateFilter(key, event.target.value)}>
                    <option value="">全部</option>
                    {(facets[facetKey] || []).map((item) => (
                      <option key={item.value} value={item.value}>
                        {key === 'requestKind'
                          ? requestKindLabel(item.value)
                          : key === 'clientVariant'
                            ? clientVariantLabel(item.value)
                            : key === 'agentRole'
                              ? agentRoleLabel(item.value)
                              : item.value} · {formatNumber(item.count)}
                      </option>
                    ))}
                  </NativeSelect>
                </label>
              ))}

              <label className="space-y-1">
                <span className="block text-[11px] font-medium text-muted-foreground">用途</span>
                <NativeSelect size="sm" value={filters.requestPurpose || ''} onChange={(event) => updateFilter('requestPurpose', event.target.value)}>
                  <option value="">全部</option>
                  {facets.requestPurposes.map((item) => (
                    <option key={item.value} value={item.value}>{facetOptionLabel(item, 'purpose')}</option>
                  ))}
                </NativeSelect>
              </label>

              <label className="space-y-1">
                <span className="block text-[11px] font-medium text-muted-foreground">失败类型</span>
                <NativeSelect size="sm" value={filters.failureKind || ''} onChange={(event) => updateFilter('failureKind', event.target.value)}>
                  <option value="">全部</option>
                  {facets.failureKinds.map((item) => (
                    <option key={item.value} value={item.value}>{facetOptionLabel(item, 'failureKind')}</option>
                  ))}
                </NativeSelect>
              </label>

              <label className="space-y-1">
                <span className="block text-[11px] font-medium text-muted-foreground">失败阶段</span>
                <NativeSelect size="sm" value={filters.failurePhase || ''} onChange={(event) => updateFilter('failurePhase', event.target.value)}>
                  <option value="">全部</option>
                  {facets.failurePhases.map((item) => (
                    <option key={item.value} value={item.value}>{facetOptionLabel(item, 'failurePhase')}</option>
                  ))}
                </NativeSelect>
              </label>
            </div>
          </details>

          {advancedFilterLabels.length > 0 && (
            <p className="font-mono text-xs text-primary">{advancedFilterLabels.join(' · ')}</p>
          )}
        </CardContent>
      </Card>

      <AnalyticsWorkspace
        onSelectEvent={setSelectedEventID}
        addToast={addToast}
        onManualCleanup={() => setCleanupOpen(true)}
        onRecreateDatabase={recreateDatabase}
        openExportSignal={openExportSignal}
        analyticsFilters={filters}
        analyticsQueryRevision={analyticsQueryRevision}
        analyticsRange={analyticsRange}
        onAnalyticsRangeChange={(value) => loadRuntimeAnalytics(value, filters).catch(() => null)}
        facets={runtimeFacets?.facets || null}
        analyticsRefreshSignal={analyticsRefreshSignal}
        onProjectSelectionChange={handleProjectSelectionChange}
        onSessionSelectionChange={handleSessionSelectionChange}
        projectSelectionResetSignal={projectSelectionResetSignal}
        legacyAnalytics={runtimeAnalytics}
        summaryStorage={runtime?.summary?.storage || runtime?.storage || null}
        config={config}
      />

      <RuntimeCleanupDialog
        open={cleanupOpen}
        onClose={() => setCleanupOpen(false)}
        onCompleted={async () => {
          clearLocalDrilldowns();
          await refreshCore();
        }}
        addToast={addToast}
      />

      {selectedEventID && (
        <Card id="analytics-request-drilldown" className="gap-0 overflow-hidden py-0" aria-labelledby="analytics-v3-event-heading">
          <CardHeader className="flex flex-row items-center justify-between border-b border-border/75 py-4">
            <CardTitle id="analytics-v3-event-heading">请求详情</CardTitle>
            <Button type="button" variant="outline" size="xs" onClick={() => setSelectedEventID(null)}>
              关闭
            </Button>
          </CardHeader>
          <div>
            {eventDetailError ? (
              <div className="p-5">
                <div className="rounded-xl border border-status-critical/30 bg-status-critical/10 p-3.5 text-xs text-status-critical" role="alert">
                  {eventDetailError}
                </div>
              </div>
            ) : selectedEvent ? (
              <EventInspector
                event={selectedEvent}
                chain={eventChain}
                onSelect={setSelectedEventID}
                expanded={expandedGroups}
                onToggle={(key, open) => setExpandedGroups((previous) => (previous[key] === open ? previous : { ...previous, [key]: open }))}
                onCopy={(text, label) => copyWithToast(text, label, addToast)}
              />
            ) : (
              <div className="px-5 py-8 text-center text-xs text-muted-foreground" role="status">正在按需读取单条详情…</div>
            )}
          </div>
        </Card>
      )}
    </div>
  );
}
