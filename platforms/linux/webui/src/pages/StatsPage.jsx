import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { AnalyticsWorkspace } from '../components/AnalyticsWorkspace.jsx';
import { StatusBadge } from '../components/StatusBadge.jsx';
import { useApp } from '../context/AppContext.jsx';
import { api } from '../services/api.js';
import {
  clientKindLabel,
  eventDurationText,
  eventEndpointName,
  eventFailureSummaryLabel,
  eventOutcomeLabel,
  eventRequestID,
  eventResultKind,
  failureKindLabel,
  failurePhaseLabel,
  formatNumber,
  formatTimestamp,
  purposeLabel,
  statusKind,
} from '../utils/helpers.js';
import { Icon } from '../utils/icons.jsx';

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

  if (!open) return null;
  const olderThan = cleanupCutoff(selection, customDate);
  const canPreview = olderThan != null && !loading;
  const runPreview = async () => {
    if (olderThan == null) return;
    setLoading(true); setError(''); setPreview(null);
    try {
      setPreview(await api.previewRuntimeCleanup({ olderThan }));
    } catch (nextError) {
      setError(nextError?.message || '无法预览清理范围');
    } finally { setLoading(false); }
  };
  const runCleanup = async () => {
    if (!preview || olderThan == null || loading) return;
    setLoading(true); setError('');
    try {
      const result = await api.cleanupRuntime({ olderThan });
      addToast(`已按时间清理 ${formatNumber(result.deletedEvents || 0)} 条统计事件`, 'success');
      onClose?.();
      await onCompleted?.();
    } catch (nextError) {
      setError(nextError?.message || '清理失败');
    } finally { setLoading(false); }
  };
  const nowDate = new Date();
  const today = [nowDate.getFullYear(), String(nowDate.getMonth() + 1).padStart(2, '0'), String(nowDate.getDate()).padStart(2, '0')].join('-');
  return (
    <div className="modal-overlay runtime-v2-storage-modal" onClick={(event) => { if (event.target === event.currentTarget && !loading) onClose?.(); }}>
      <section className="modal-dialog runtime-v2-storage-dialog runtime-cleanup-dialog" role="dialog" aria-modal="true" aria-labelledby="runtime-cleanup-heading">
        <header className="modal-header runtime-v2-storage-dialog-header">
          <div><h2 id="runtime-cleanup-heading">按时间清理运行统计</h2><p>只删除已完成且整组早于截止时间的请求；进行中的请求组会完整保留。</p></div>
          <button type="button" className="btn-icon" onClick={onClose} disabled={loading} aria-label="关闭清理窗口"><Icon name="close" size={16} /></button>
        </header>
        <div className="modal-body runtime-v2-storage-dialog-body runtime-cleanup-body">
          <label><span>清理范围</span><select className="form-select" value={selection} onChange={(event) => { setSelection(event.target.value); setPreview(null); setError(''); }} disabled={loading}>
            <option value="7">早于 7 天</option><option value="30">早于 30 天</option><option value="90">早于 90 天</option><option value="custom">自定义日期</option><option value="all">全部已完成记录</option>
          </select></label>
          {selection === 'custom' && <label><span>清理早于此日期的记录</span><input className="form-input" type="date" value={customDate} max={today} onChange={(event) => { setCustomDate(event.target.value); setPreview(null); setError(''); }} disabled={loading} /></label>}
          <p className="runtime-cleanup-help">将删除截止时间之前的统计事件。诊断捕获、自动保留策略和数据库结构不会受影响。</p>
          {selection === 'all' && <div className="runtime-v2-storage-limit-error" role="note">“全部”也不会删除进行中的请求；如需移除旧版字段或回收数据库文件，请单独使用“重置并新建数据库”。</div>}
          {preview && <div className="runtime-cleanup-preview" role="status"><strong>预计删除 {formatNumber(preview.deletableEvents)} 条事件</strong><span>涉及约 {formatNumber(preview.deletableRequests)} 个请求 · 清理后保留 {formatNumber(preview.remainingEvents)} 条事件</span></div>}
          {error && <div className="runtime-v2-storage-limit-error" role="alert">{error}</div>}
        </div>
        <footer className="modal-footer runtime-v2-storage-dialog-footer">
          <button type="button" className="btn btn-secondary" onClick={onClose} disabled={loading}>取消</button>
          {!preview ? <button type="button" className="btn btn-primary" onClick={runPreview} disabled={!canPreview}>{loading ? '计算中…' : '预览清理范围'}</button> : <button type="button" className="btn btn-danger" onClick={runCleanup} disabled={loading || preview.deletableEvents <= 0}>{loading ? '清理中…' : `确认清理 ${formatNumber(preview.deletableEvents)} 条`}</button>}
        </footer>
      </section>
    </div>
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
    config,
    runtimeAnalytics,
    runtimeFacets,
    analyticsRefreshSignal,
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
  const [selectedEventID, setSelectedEventID] = useState(null);
  const [eventDetailError, setEventDetailError] = useState('');
  const [openExportSignal, setOpenExportSignal] = useState(0);
  const [cleanupOpen, setCleanupOpen] = useState(false);
  const [selectedProject, setSelectedProject] = useState(null);
  const [selectedSession, setSelectedSession] = useState(null);
  const [projectSelectionResetSignal, setProjectSelectionResetSignal] = useState(0);

  useEffect(() => {
    if (!selectedEventID) {
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
  }, [loadRuntimeEvent, selectedEventID]);

  const filters = analyticsFilters || { clientKind: '', endpointID: '', project: '', projectID: '', sessionID: '', model: '', requestPurpose: '', outcome: '', failureKind: '', failurePhase: '' };
  const activeFilterCount = [filters.clientKind, filters.endpointID, filters.projectID, filters.project, filters.sessionID, filters.model, filters.requestPurpose, filters.outcome, filters.failureKind, filters.failurePhase, selectedProject?.key, selectedSession?.key]
    .filter((value) => String(value || '').trim()).length;
  const facets = useMemo(() => ({
    clientKinds: facetOptions(runtimeFacets, 'clientKinds', filters.clientKind),
    endpoints: endpointOptions(runtime, config, runtimeFacets, filters.endpointID),
    projects: facetOptions(runtimeFacets, 'projects', filters.project),
    sessions: facetOptions(runtimeFacets, 'sessions', filters.sessionID),
    models: facetOptions(runtimeFacets, 'models', filters.model),
    requestPurposes: facetOptions(runtimeFacets, 'requestPurposes', filters.requestPurpose),
    failureKinds: facetOptions(runtimeFacets, 'failureKinds', filters.failureKind),
    failurePhases: facetOptions(runtimeFacets, 'failurePhases', filters.failurePhase),
  }), [config, runtime, runtimeFacets, filters.clientKind, filters.endpointID, filters.project, filters.sessionID, filters.model, filters.requestPurpose, filters.failureKind, filters.failurePhase]);

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
    loadRuntimeAnalytics(analyticsRange, { clientKind: '', endpointID: '', project: '', projectID: '', sessionID: '', model: '', requestPurpose: '', outcome: '', failureKind: '', failurePhase: '' }).catch(() => null);
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

  const selectedEvent = runtimeEventDetail?.id === selectedEventID ? runtimeEventDetail : null;
  return (
    <div className="analytics-page analytics-v3-page">
      <div className="page-header analytics-v3-page-header">
        <div className="page-title-group">
          <h1 className="page-title">
            <Icon name="chart" size={24} style={{ color: 'var(--primary)' }} />
            <span>统计</span>
          </h1>
          <p className="page-subtitle">请求统计与运行健康</p>
        </div>
        <div className="page-actions">
          <button type="button" className="btn btn-secondary" onClick={() => setOpenExportSignal((value) => value + 1)}>
            <Icon name="download" size={15} />导出
          </button>
        </div>
      </div>

      <section className="glass-panel analytics-v3-filter-panel" aria-labelledby="analytics-v3-filter-heading">
        <div className="analytics-v3-filter-status">
          <div>
            <h2 id="analytics-v3-filter-heading">范围与筛选</h2>
            <p>所有条件均可选；留空时显示全部，选择后下面的指标和列表会一起更新。</p>
          </div>
        </div>

        <div id="analytics-v3-filter-grid" className="analytics-v3-filter-grid">
          <fieldset className="analytics-v3-range" aria-label="统计时间范围">
            <legend>时间</legend>
            <div className="analytics-v3-segmented">
              {RANGE_OPTIONS.map(([value, label]) => (
                <button
                  key={value}
                  type="button"
                  aria-pressed={analyticsRange === value}
                  className={`btn btn-sm ${analyticsRange === value ? 'btn-primary' : 'btn-secondary'}`}
                  onClick={() => loadRuntimeAnalytics(value, filters).catch(() => null)}
                >
                  {label}
                </button>
              ))}
            </div>
          </fieldset>
          <label>
            <span>入口</span>
            <select className="form-select" value={filters.endpointID || ''} onChange={(event) => updateFilter('endpointID', event.target.value)}>
              <option value="">不限入口</option>
              {facets.endpoints.map((item) => <option key={item.value} value={item.value}>{item.label} · {formatNumber(item.count)}</option>)}
            </select>
          </label>
          <label>
            <span>项目</span>
            <select className="form-select" value={filters.project || ''} onChange={(event) => updateFilter('project', event.target.value)}>
              <option value="">不限项目</option>
              {facets.projects.map((item) => <option key={item.value} value={item.value}>{projectLabel(item.value)} · {formatNumber(item.count)}</option>)}
            </select>
          </label>
          <label>
            <span>会话</span>
            <select className="form-select" value={filters.sessionID || ''} onChange={(event) => updateFilter('sessionID', event.target.value)}>
              <option value="">不限会话</option>
              {facets.sessions.map((item) => <option key={item.value} value={item.value}>{sessionLabel(item.value)} · {formatNumber(item.count)}</option>)}
            </select>
          </label>
          <label>
            <span>客户端</span>
            <select className="form-select" value={filters.clientKind || ''} onChange={(event) => updateFilter('clientKind', event.target.value)}>
              <option value="">不限客户端</option>
              {facets.clientKinds.map((item) => <option key={item.value} value={item.value}>{clientKindLabel(item.value)} · {formatNumber(item.count)}</option>)}
            </select>
          </label>
          <label>
            <span>模型</span>
            <select className="form-select" value={filters.model || ''} onChange={(event) => updateFilter('model', event.target.value)}>
              <option value="">不限模型</option>
              {facets.models.map((item) => <option key={item.value} value={item.value}>{facetOptionLabel(item, 'model')}</option>)}
            </select>
          </label>
          <label>
            <span>最终结果</span>
            <select className="form-select" value={filters.outcome || ''} onChange={(event) => updateFilter('outcome', event.target.value)}>
              <option value="">不限结果</option>
              <option value="succeeded">成功</option>
              <option value="failed">失败</option>
              <option value="cancelled">已取消</option>
            </select>
          </label>
          <label>
            <span>用途</span>
            <select className="form-select" value={filters.requestPurpose || ''} onChange={(event) => updateFilter('requestPurpose', event.target.value)}>
              <option value="">不限用途</option>
              {facets.requestPurposes.map((item) => <option key={item.value} value={item.value}>{facetOptionLabel(item, 'purpose')}</option>)}
            </select>
          </label>
          <label>
            <span>失败类型</span>
            <select className="form-select" value={filters.failureKind || ''} onChange={(event) => updateFilter('failureKind', event.target.value)}>
              <option value="">不限失败类型</option>
              {facets.failureKinds.map((item) => <option key={item.value} value={item.value}>{facetOptionLabel(item, 'failureKind')}</option>)}
            </select>
          </label>
          <label>
            <span>失败阶段</span>
            <select className="form-select" value={filters.failurePhase || ''} onChange={(event) => updateFilter('failurePhase', event.target.value)}>
              <option value="">不限失败阶段</option>
              {facets.failurePhases.map((item) => <option key={item.value} value={item.value}>{facetOptionLabel(item, 'failurePhase')}</option>)}
            </select>
          </label>
          <button
            type="button"
            className="btn btn-secondary analytics-v3-clear-filter"
            disabled={!activeFilterCount}
            onClick={clearAllFilters}
          >
            清除筛选
          </button>
        </div>
        <div className="analytics-v3-filter-feedback" role={analyticsError ? 'alert' : 'status'} aria-live="polite">
          {analyticsError
            ? `统计刷新失败：${analyticsError}${analyticsStale ? '；当前保留上一份结果。' : ''}`
            : analyticsLoading ? '正在更新统计…' : activeFilterCount ? `已应用 ${activeFilterCount} 项筛选` : '显示全部项目'}
        </div>
      </section>

      <AnalyticsWorkspace
        onSelectEvent={setSelectedEventID}
        addToast={addToast}
        onManualCleanup={() => setCleanupOpen(true)}
        onRecreateDatabase={recreateDatabase}
        openExportSignal={openExportSignal}
        analyticsFilters={filters}
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
        onCompleted={async () => { clearLocalDrilldowns(); await refreshCore(); }}
        addToast={addToast}
      />

      {selectedEventID && (
        <section id="analytics-request-drilldown" className="glass-panel analytics-v3-event-detail" aria-labelledby="analytics-v3-event-heading">
          <div className="panel-header">
            <div className="panel-title-group">
              <h2 className="panel-title" id="analytics-v3-event-heading">错误样本详情</h2>
              <span className="panel-hint mono-cell" title={selectedEventID}>{selectedEventID}</span>
            </div>
            <div className="page-actions">
              <button type="button" className="btn btn-ghost" onClick={() => navigator.clipboard?.writeText(JSON.stringify(selectedEvent || {}, null, 2))} disabled={!selectedEvent}><Icon name="copy" size={14} />复制源事件 JSON</button>
              <button type="button" className="btn btn-ghost" onClick={() => setSelectedEventID(null)}><Icon name="close" size={14} />关闭</button>
            </div>
          </div>
          <div className="panel-body">
            {eventDetailError ? <div className="runtime-v2-message" role="alert">{eventDetailError}</div> : selectedEvent ? (
              <>
              <div className="analytics-v3-event-grid" data-event-detail-tier="primary">
                <div><span>时间</span><strong className="mono-cell">{formatTimestamp(selectedEvent.timestamp, { date: true })}</strong></div>
                <div><span>最终结果</span><strong><StatusBadge text={eventOutcomeLabel(selectedEvent)} kind={statusKind(selectedEvent)} /></strong></div>
                <div><span>入口</span><strong title={eventEndpointName(selectedEvent)}>{eventEndpointName(selectedEvent)}</strong></div>
                <div><span>耗时</span><strong className="mono-cell">{eventDurationText(selectedEvent)}</strong></div>
                <div><span>故障转移</span><strong>{selectedEvent.failover ? '已发生' : '未发生'}</strong></div>
                <div><span>生命周期</span><strong>{selectedEvent.phase || '终态'}</strong></div>
              </div>
              <details className="event-secondary-details" data-event-detail-tier="secondary">
                <summary>路由与协议详情</summary>
                <div className="analytics-v3-event-grid"><div><span>入口</span><strong>{eventEndpointName(selectedEvent)}</strong></div><div><span>客户端模型</span><strong>{selectedEvent.clientModel || '—'}</strong></div></div>
              </details>
              <details className="event-diagnostics-details" data-event-detail-tier="diagnostics">
                <summary>失败、工具与流诊断</summary>
                <div className="analytics-v3-event-grid">
                  <div><span>Request ID</span><strong className="mono-cell">{eventRequestID(selectedEvent)}</strong></div>
                  <div><span>事件 ID</span><strong className="mono-cell">{selectedEvent.id || '—'}</strong></div>
                  <div><span>入口 ID</span><strong className="mono-cell">{selectedEvent.endpointID || '—'}</strong></div>
                  <div><span>入口名称</span><strong>{eventEndpointName(selectedEvent)}</strong></div>
                  <div><span>上游 Host</span><strong className="mono-cell">{selectedEvent.upstreamHost || '—'}</strong></div>
                  <div><span>上游请求 ID</span><strong className="mono-cell">{selectedEvent.upstreamRequestID || '—'}</strong></div>
                  <div><span>实际超时阈值</span><strong className="mono-cell">{selectedEvent.timeoutMS == null ? '—' : `${selectedEvent.timeoutMS} ms`}</strong></div>
                  <div><span>原始引擎消息</span><strong>{selectedEvent.message || '—'}</strong></div>
                  <div><span>诊断</span><strong className={eventResultKind(selectedEvent) === 'failed' ? 'runtime-v2-critical' : ''}>{eventFailureSummaryLabel(selectedEvent)}</strong></div>
                </div>
              </details>
              <details className="analytics-selected-codex"><summary>Codex 元数据（如有）</summary><p>完整 Codex 元数据仅在明确展开后读取。</p></details>
              </>
            ) : <div className="runtime-v2-loading" role="status">正在按需读取单条详情…</div>}
          </div>
        </section>
      )}
    </div>
  );
}
