import React, { useState, useEffect, useCallback, useRef } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { StatusBadge } from '../components/StatusBadge.jsx';
import { Icon } from '../utils/icons.jsx';
import { copyWithToast } from '../utils/clipboard.js';
import { api } from '../services/api.js';
import { formatDuration, formatTimestamp, formatNumber, clientDeclaredProject, projectSourceLabel } from '../utils/helpers.js';

// 原始 Body/Chunks 可能接近整个捕获容量。详情页首屏不应该为每个折叠块
// 都创建一份字符串或 DOM；只有用户明确展开某一块时才计算和挂载内容。
// `valueFactory` 用来延迟 headersText/chunksText 等昂贵操作；完整 JSON 只由
// 明确的复制/下载动作编码，不会因为详情渲染而自动 stringify。
function RawBlock({ title, value, valueFactory }) {
  const [renderedValue, setRenderedValue] = useState(null);

  const loadValue = (event) => {
    if (!event.currentTarget.open) {
      // 关闭大块正文时释放格式化后的副本；原始详情仍由选择状态持有，
      // 下次展开会按需重新生成。
      if (renderedValue !== null) setRenderedValue(null);
      return;
    }
    if (renderedValue !== null) return;
    try {
      const next = typeof valueFactory === 'function' ? valueFactory() : value;
      setRenderedValue(next == null || next === '' ? '(空)' : String(next));
    } catch (error) {
      setRenderedValue(`读取失败：${error instanceof Error && error.message ? error.message : '原始内容无效'}`);
    }
  };

  return <details className="diagnostics-raw-block" onToggle={loadValue}><summary>{title}</summary>{renderedValue !== null && <pre className="mono-cell diagnostics-code">{renderedValue}</pre>}</details>;
}

function headersText(headers = []) {
  return headers.map(({ name, value }) => `${name}: ${value}`).join('\n');
}

function chunksText(chunks = []) {
  return chunks.map((chunk) => `[+${chunk.atMS}ms · ${chunk.bytes} bytes${chunk.truncated ? ' · 已截断' : ''}]\n${chunk.data}`).join('\n\n');
}

const SENSITIVE_NAME = /(authorization|proxy-authorization|cookie|set-cookie|api[-_]?key|token|secret|password|passwd|credential|signature|access[-_]?key)/i;

function redactURL(value) {
  if (typeof value !== 'string') return value;
  try {
    const url = new URL(value, window.location.origin);
    for (const key of [...url.searchParams.keys()]) {
      if (SENSITIVE_NAME.test(key)) url.searchParams.set(key, '[REDACTED]');
    }
    return url.toString();
  } catch {
    return value.replace(/([?&](?:token|key|secret|password|signature)=[^&]*)/gi, (match) => `${match.split('=')[0]}=[REDACTED]`);
  }
}

function redactCaptureValue(value, key = '') {
  if (SENSITIVE_NAME.test(key)) return '[REDACTED]';
  if (typeof value === 'string') return key.toLowerCase().includes('url') ? redactURL(value) : value;
  if (Array.isArray(value)) return value.map((item) => redactCaptureValue(item, key));
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value).map(([childKey, childValue]) => [
    childKey,
    redactCaptureValue(childValue, childKey),
  ]));
}

function redactedCapture(detail) {
  return redactCaptureValue(detail);
}

function formatBytes(bytes = 0) {
  const value = Number(bytes) || 0;
  if (value < 1048576) return `${(value / 1024).toFixed(value < 1024 ? 1 : 0)} KB`;
  if (value < 1073741824) return `${(value / 1048576).toFixed(1)} MB`;
  return `${(value / 1073741824).toFixed(2)} GB`;
}

export function normalizeDiagnosticCaptureIndex(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('诊断捕获响应不是有效对象');
  if (!Array.isArray(value.records)) throw new Error('诊断捕获响应缺少 records 列表');
  const maxBytes = Number(value.maxBytes);
  if (!Number.isFinite(maxBytes) || maxBytes <= 0) throw new Error('诊断捕获响应缺少有效容量上限');
  const capturedBytes = Number(value.capturedBytes ?? 0);
  const recordCount = Number(value.recordCount ?? value.records.length);
  return {
    ...value,
    enabled: Boolean(value.enabled),
    maxBytes,
    capturedBytes: Number.isFinite(capturedBytes) && capturedBytes >= 0 ? capturedBytes : 0,
    recordCount: Number.isFinite(recordCount) && recordCount >= 0 ? recordCount : value.records.length,
    indexTruncated: Boolean(value.indexTruncated),
    records: value.records.filter((record) => record && typeof record === 'object' && !Array.isArray(record)),
  };
}

export function normalizeDiagnosticCaptureDetail(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('诊断详情响应不是有效对象');
  if (!String(value.requestID || '').trim()) throw new Error('诊断详情缺少 requestID');
  return value;
}

export function formatListenerAddress(value, fallback) {
  if (typeof value === 'string') {
    const address = value.trim();
    return address || fallback;
  }
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    const host = value.host ?? value.hostname;
    const port = value.port;
    if (host != null && port != null && String(host).trim() && String(port).trim()) {
      return `${String(host).trim()}:${String(port).trim()}`;
    }
  }
  return fallback;
}

function errorMessage(error, fallback) {
  return error instanceof Error && error.message ? error.message : fallback;
}

export function DiagnosticsPage() {
  const { addToast } = useApp();
  const [diag, setDiag] = useState(null);
  const [loading, setLoading] = useState(true);
  const [capture, setCapture] = useState(null);
  const [captureLoading, setCaptureLoading] = useState(true);
  const [diagnosticsError, setDiagnosticsError] = useState(null);
  const [captureError, setCaptureError] = useState(null);
  const [captureDetail, setCaptureDetail] = useState(null);
  const [captureDetailLoading, setCaptureDetailLoading] = useState(false);
  const [captureDetailError, setCaptureDetailError] = useState(null);
  const [selectedCaptureID, setSelectedCaptureID] = useState('');
  const [captureBusy, setCaptureBusy] = useState(false);
  const [captureExportScope, setCaptureExportScope] = useState('selected');
  const [captureExportFormat, setCaptureExportFormat] = useState('jsonl');
  // 本地诊断的首要用途是复盘源请求链；索引/详情/导出默认使用源数据。
  // 脱敏仍可主动选择，raw 全量导出继续保留风险确认。
  const [captureExportPrivacy, setCaptureExportPrivacy] = useState('raw');
  const [captureExportConfirmRaw, setCaptureExportConfirmRaw] = useState(false);
  const [captureExportBusy, setCaptureExportBusy] = useState(false);
  const [captureCapacityMB, setCaptureCapacityMB] = useState('512');
  // 用户手动改过容量之后,自动刷新索引不能再用后端值覆盖输入框 —— 否则「敲 1024 → 等刷新
  // (或重进本页)→ 悄悄变回 512 → 点开始」,实际按 512 MB 采集,和界面上看到的不一致。
  const capacityDirtyRef = useRef(false);
  const requestGenerationRef = useRef(0);
  const requestAbortRef = useRef(null);
  const detailGenerationRef = useRef(0);
  const detailAbortRef = useRef(null);
  const captureIndexGenerationRef = useRef(0);
  const captureIndexAbortRef = useRef(null);
  const captureIndexSignatureRef = useRef('');
  const captureRef = useRef(null);
  const captureActionGenerationRef = useRef(0);
  const captureActionAbortRef = useRef(null);
  captureRef.current = capture;

  const fetchDiag = useCallback(async () => {
    const generation = ++requestGenerationRef.current;
    requestAbortRef.current?.abort();
    const controller = new AbortController();
    requestAbortRef.current = controller;
    const timeout = window.setTimeout(() => controller.abort(), 10_000);
    setLoading(true);
    setCaptureLoading(true);

    const diagnosticsTask = api.getDiagnostics({ signal: controller.signal })
      .then((value) => {
        if (requestGenerationRef.current !== generation) return;
        if (value && typeof value === 'object') {
          setDiag(value);
          setDiagnosticsError(null);
          return;
        }
        const message = '诊断信息响应不是有效对象';
        setDiag(null);
        setDiagnosticsError(message);
        addToast(`环境诊断：${message}`, 'error');
      })
      .catch((error) => {
        if (requestGenerationRef.current !== generation) return;
        const message = error?.name === 'AbortError'
          ? '诊断请求超时（10 秒），请检查 Admin 服务是否可用'
          : errorMessage(error, '诊断信息请求失败');
        setDiag(null);
        setDiagnosticsError(message);
        if (error?.name !== 'AbortError' || !controller.signal.aborted) addToast(`环境诊断：${message}`, 'error');
      })
      .finally(() => {
        if (requestGenerationRef.current === generation) setLoading(false);
      });

    const captureTask = api.getDiagnosticCapture({ signal: controller.signal })
      .then((value) => {
        if (requestGenerationRef.current !== generation) return;
        try {
          const index = normalizeDiagnosticCaptureIndex(value);
          captureIndexSignatureRef.current = JSON.stringify(index);
          setCapture(index);
          setCaptureError(null);
          // 已开始捕获时输入框不可编辑；若 App 之前已在 1024 MB 运行，也要把
          // 服务端确认值显示出来，不能因 enabled=true 而退回初始 512。
          if (!capacityDirtyRef.current) {
            setCaptureCapacityMB(String(Math.max(1, Math.round(index.maxBytes / 1048576))));
          }
          setSelectedCaptureID((current) => (
            index.records.some((record) => record.requestID === current) ? current : ''
          ));
        } catch (error) {
          const message = errorMessage(error, '诊断捕获索引响应格式无效');
          setCapture(null);
          setCaptureError(message);
          setSelectedCaptureID('');
          setCaptureDetail(null);
          addToast(`完整捕获：${message}`, 'error');
        }
      })
      .catch((error) => {
        if (requestGenerationRef.current !== generation) return;
        const timedOut = error?.name === 'AbortError';
        const message = timedOut
          ? '完整捕获索引请求超时（10 秒）'
          : errorMessage(error, '诊断捕获索引请求失败');
        setCapture(null);
        setCaptureError(message);
        setSelectedCaptureID('');
        setCaptureDetail(null);
        // The shared deadline is reported by the environment request when both
        // endpoints time out; keep the UI error here without a duplicate toast.
        if (!timedOut) addToast(`完整捕获：${message}`, 'error');
      })
      .finally(() => {
        if (requestGenerationRef.current === generation) setCaptureLoading(false);
      });

    // Do not use this join as a UI barrier: each panel above settles as soon
    // as its own response arrives. Waiting here only keeps cleanup deterministic.
    await Promise.allSettled([diagnosticsTask, captureTask]);
    window.clearTimeout(timeout);
    if (requestGenerationRef.current === generation) requestAbortRef.current = null;
  }, [addToast]);

  const refreshCaptureIndex = useCallback(async ({ silent = true } = {}) => {
    if (captureActionAbortRef.current) return;
    const generation = ++captureIndexGenerationRef.current;
    captureIndexAbortRef.current?.abort();
    const controller = new AbortController();
    captureIndexAbortRef.current = controller;
    const timeout = window.setTimeout(() => controller.abort(), 10_000);
    if (!silent && !captureRef.current) setCaptureLoading(true);
    try {
      const index = normalizeDiagnosticCaptureIndex(await api.getDiagnosticCapture({ signal: controller.signal }));
      if (captureIndexGenerationRef.current !== generation) return;
      const signature = JSON.stringify(index);
      if (signature !== captureIndexSignatureRef.current) {
        captureIndexSignatureRef.current = signature;
        setCapture(index);
      }
      setCaptureError(null);
      if (!capacityDirtyRef.current) {
        setCaptureCapacityMB(String(Math.max(1, Math.round(index.maxBytes / 1048576))));
      }
      setSelectedCaptureID((current) => (
        index.records.some((record) => record.requestID === current) ? current : ''
      ));
    } catch (error) {
      if (captureIndexGenerationRef.current !== generation || error?.name === 'AbortError' && controller.signal.aborted) return;
      setCaptureError(errorMessage(error, '诊断捕获索引自动刷新失败'));
    } finally {
      window.clearTimeout(timeout);
      if (captureIndexGenerationRef.current === generation) {
        captureIndexAbortRef.current = null;
        setCaptureLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    fetchDiag();
    return () => {
      requestGenerationRef.current += 1;
      requestAbortRef.current?.abort();
      requestAbortRef.current = null;
    };
  }, [fetchDiag]);

  useEffect(() => {
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') void refreshCaptureIndex({ silent: true });
    };
    const timer = window.setInterval(refreshWhenVisible, 5_000);
    document.addEventListener('visibilitychange', refreshWhenVisible);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', refreshWhenVisible);
      captureIndexGenerationRef.current += 1;
      captureIndexAbortRef.current?.abort();
      captureIndexAbortRef.current = null;
    };
  }, [refreshCaptureIndex]);

  useEffect(() => {
    detailGenerationRef.current += 1;
    detailAbortRef.current?.abort();
    detailAbortRef.current = null;
    setCaptureDetail(null);
    setCaptureDetailError(null);
    setCaptureDetailLoading(false);
  }, [selectedCaptureID]);

  const loadCaptureDetail = useCallback(async () => {
    const requestID = String(selectedCaptureID || '').trim();
    if (!requestID || !captureRef.current?.records?.some((item) => item.requestID === requestID)) return;
    const generation = ++detailGenerationRef.current;
    detailAbortRef.current?.abort();
    const controller = new AbortController();
    detailAbortRef.current = controller;
    // 明文详情可能接近 512 MiB/用户配置的 1024 MiB；索引仍用短超时，但详情
    // 要给本机编码、传输和 JSON 解码留出时间，避免把“大”误报成网络失败。
    const timeout = window.setTimeout(() => controller.abort(), 120_000);
    setCaptureDetail(null);
    setCaptureDetailError(null);
    setCaptureDetailLoading(true);
    try {
      const value = await api.getDiagnosticCaptureDetail(requestID, { signal: controller.signal });
      if (detailGenerationRef.current !== generation) return;
      setCaptureDetail(normalizeDiagnosticCaptureDetail(value));
    } catch (error) {
      if (detailGenerationRef.current !== generation) return;
      const message = error?.name === 'AbortError'
        ? '读取抓包详情超时（120 秒）'
        : errorMessage(error, '读取抓包详情失败');
      setCaptureDetailError(message);
      if (error?.name !== 'AbortError') addToast(message, 'error');
    } finally {
      window.clearTimeout(timeout);
      if (detailGenerationRef.current === generation) {
        detailAbortRef.current = null;
        setCaptureDetailLoading(false);
      }
    }
  }, [selectedCaptureID, addToast]);

  useEffect(() => () => {
    requestGenerationRef.current += 1;
    requestAbortRef.current?.abort();
    detailGenerationRef.current += 1;
    detailAbortRef.current?.abort();
    captureIndexGenerationRef.current += 1;
    captureIndexAbortRef.current?.abort();
    captureActionGenerationRef.current += 1;
    captureActionAbortRef.current?.abort();
  }, []);

  const copyCommand = (cmd) => copyWithToast(cmd, '日志命令', addToast);

  const journalCmd = diag?.journalctlCommand || 'journalctl --user -u sumpter -n 200 --no-pager';

  const selectedCaptureIndex = capture?.records?.find((record) => record.requestID === selectedCaptureID) || null;
  const captureStatus = capture?.enabled
    ? ['采集中', 'live']
    : capture?.limitReached
      ? ['已达容量', 'warning']
      : capture?.stopReason === 'manual' ? ['已停止', 'muted'] : ['未开始', 'muted'];
  const capturePercent = capture?.maxBytes
    ? Math.min(100, ((capture.capturedBytes || 0) / capture.maxBytes) * 100)
    : 0;
  const capacityNumber = Number(captureCapacityMB);
  const capacityValid = Number.isSafeInteger(capacityNumber)
    && capacityNumber > 0
    && capacityNumber <= Math.floor(Number.MAX_SAFE_INTEGER / 1048576);

  const beginCaptureAction = () => {
    captureActionAbortRef.current?.abort();
    const generation = ++captureActionGenerationRef.current;
    const controller = new AbortController();
    captureActionAbortRef.current = controller;
    const timeout = window.setTimeout(() => controller.abort(), 10_000);
    return { generation, controller, timeout };
  };

  const updateCapture = async (enabled) => {
    const action = beginCaptureAction();
    setCaptureBusy(true);
    try {
      const next = normalizeDiagnosticCaptureIndex(await api.setDiagnosticCapture(
        enabled,
        enabled ? capacityNumber * 1048576 : undefined,
        { signal: action.controller.signal },
      ));
      if (captureActionGenerationRef.current !== action.generation) return;
      setCapture(next);
      captureIndexSignatureRef.current = JSON.stringify(next);
      setCaptureError(null);
      if (!enabled) {
        capacityDirtyRef.current = false;
        setCaptureCapacityMB(String(Math.max(1, Math.round(next.maxBytes / 1048576))));
      }
      addToast(enabled ? '完整诊断捕获已开始' : '完整诊断捕获已停止', 'success');
    } catch (error) {
      if (captureActionGenerationRef.current !== action.generation) return;
      const message = errorMessage(error, '切换诊断捕获失败');
      setCaptureError(message);
      addToast(error?.name === 'AbortError' ? '切换诊断捕获超时（10 秒）' : message, 'error');
    } finally {
      window.clearTimeout(action.timeout);
      if (captureActionGenerationRef.current === action.generation) {
        captureActionAbortRef.current = null;
        setCaptureBusy(false);
      }
    }
  };

  const clearCapture = async () => {
    const action = beginCaptureAction();
    setCaptureBusy(true);
    try {
      await api.clearDiagnosticCapture({ signal: action.controller.signal });
      const next = normalizeDiagnosticCaptureIndex(await api.getDiagnosticCapture({ signal: action.controller.signal }));
      if (captureActionGenerationRef.current !== action.generation) return;
      setCapture(next);
      captureIndexSignatureRef.current = JSON.stringify(next);
      setSelectedCaptureID('');
      setCaptureDetail(null);
      setCaptureDetailError(null);
      setCaptureError(null);
      addToast('捕获记录已清空', 'success');
    } catch (error) {
      if (captureActionGenerationRef.current !== action.generation) return;
      const message = errorMessage(error, '清空诊断捕获失败');
      setCaptureError(message);
      addToast(error?.name === 'AbortError' ? '清空诊断捕获超时（10 秒）' : message, 'error');
    } finally {
      window.clearTimeout(action.timeout);
      if (captureActionGenerationRef.current === action.generation) {
        captureActionAbortRef.current = null;
        setCaptureBusy(false);
      }
    }
  };

  const captureDetailMatchesSelection = Boolean(
    captureDetail
      && String(captureDetail.requestID || '') === String(selectedCaptureID || ''),
  );

  const copyCapture = () => {
    // 只允许复制当前下拉框选中的一条记录；诊断详情默认就是源数据。
    if (!captureDetailMatchesSelection) return;
    copyWithToast(JSON.stringify(captureDetail, null, 2), '源诊断 JSON', addToast);
  };

  const startCaptureExport = async () => {
    if (!capture || Number(capture.recordCount || 0) <= 0) return;
    const requestID = captureExportScope === 'selected' ? selectedCaptureID : '';
    if (captureExportScope !== 'all' && !requestID) {
      addToast('请先选择要导出的捕获请求', 'warning');
      return;
    }
    if (captureExportPrivacy === 'raw' && !captureExportConfirmRaw) {
      addToast('导出未脱敏正文前必须勾选并确认风险', 'warning');
      return;
    }
    if (captureExportScope === 'all' && !window.confirm(`确认导出全部 ${formatNumber(capture.recordCount)} 条捕获？服务端将流式传输，浏览器不会聚合完整 JSON。`)) return;
    setCaptureExportBusy(true);
    try {
      // /admin/api/diagnostic-capture/export is an attachment stream; do not
      // replace it with fetch + Blob for a large capture.
      await api.downloadDiagnosticCapture({
        scope: captureExportScope,
        format: captureExportFormat,
        privacy: captureExportPrivacy,
        confirmRaw: captureExportConfirmRaw,
        requestID,
      });
      addToast(`已开始${captureExportPrivacy === 'raw' ? '未脱敏' : '脱敏'}诊断导出`, 'success');
    } catch (error) {
      addToast(`诊断导出失败：${errorMessage(error, '未知错误')}`, 'error');
    } finally {
      setCaptureExportBusy(false);
    }
  };

  // 捕获记录只带客户端声明的归因(X-Sumpter-*)。Codex 的结构化 workspace 不复制进捕获,
  // 它在入站 Body 的 client_metadata 里,所以这里不能笼统写成「未识别项目」。
  const captureProject = clientDeclaredProject(captureDetail);

  return (
    <div className="page-stack diagnostics-page">
      <div className="page-header">
        <div className="page-title-group">
          <h1 className="page-title">
            <Icon name="wrench" size={24} style={{ color: 'var(--primary)' }} />
            <span>诊断</span>
          </h1>
          <p className="page-subtitle">配置路径、最近请求和诊断捕获。</p>
        </div>
      </div>

      <div className="glass-panel panel-padded-stack diagnostics-capture-panel">
        <div className="panel-toolbar diagnostics-capture-header">
          <div className="diagnostics-capture-copy">
            <div className="panel-title"><Icon name="activity" size={18} style={{ color: 'var(--primary)' }} /><span>完整诊断捕获</span></div>
            <p className="panel-description diagnostics-capture-description">
              手动开始后保留实际收到的入站/出站 Headers、Body、上游响应和客户端输出。原始捕获文件不会被改写；页面默认按需读取源数据索引和单条正文，脱敏只作为主动导出选项。
            </p>
          </div>
          <div className="panel-actions diagnostics-capture-actions">
            {captureLoading ? <StatusBadge text="读取索引中" kind="muted" /> : capture ? (
              <>
                <StatusBadge text={captureStatus[0]} kind={captureStatus[1]} />
                <label htmlFor="diagnostic-capacity-mb" className="form-label" style={{ display: 'flex', alignItems: 'center', gap: '6px', margin: 0 }}>
                  <span>容量</span>
                  <input id="diagnostic-capacity-mb" type="number" min="1" step="1" inputMode="numeric" className="form-input" style={{ width: '112px' }} value={captureCapacityMB} disabled={captureBusy || capture.enabled} onChange={(event) => { capacityDirtyRef.current = true; setCaptureCapacityMB(event.target.value); }} aria-invalid={!capacityValid} />
                  <span>MB</span>
                </label>
                {capture.enabled
                  ? <button type="button" className="btn btn-secondary" disabled={captureBusy} onClick={() => updateCapture(false)}>停止捕获</button>
                  : <button type="button" className="btn btn-primary" disabled={captureBusy || !capacityValid} onClick={() => updateCapture(true)}>开始捕获</button>}
                <button type="button" className="btn btn-secondary" disabled={captureBusy || !capture.records.length} onClick={clearCapture}>清空</button>
              </>
            ) : <StatusBadge text="不可用" kind="warning" />}
          </div>
        </div>
        {captureError && !capture ? (
          <div role="alert" className="diagnostics-alert">
            <Icon name="warning" size={18} />
            <div className="diagnostics-alert-copy"><strong>完整诊断捕获暂不可用</strong><div>{captureError}</div></div>
            <button type="button" className="btn btn-secondary" disabled={loading} onClick={fetchDiag}>重试加载</button>
          </div>
        ) : capture ? (
          <>
            {captureError && <div role="alert" className="diagnostics-inline-warning"><Icon name="warning" size={15} /><span>{captureError}；保留上一次索引，页面会自动重试。</span></div>}
            <div className="diagnostics-capture-metrics">
              <div className="diagnostics-progress-meta">
                <span>{formatBytes(capture.capturedBytes)} / {formatBytes(capture.maxBytes)} · 显示最近 {capture.records.length} / 共 {capture.recordCount} 条索引{capture.indexTruncated ? '（仅显示最近 200 条）' : ''}</span><span>{capturePercent.toFixed(1)}%</span>
              </div>
              <div role="progressbar" aria-label="诊断捕获容量" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(capturePercent)} className="runtime-progress-track"><div className="runtime-progress-value" style={{ '--progress-scale': Math.max(0, Math.min(1, capturePercent / 100)), '--progress-color': capture.limitReached ? 'var(--status-warning)' : 'var(--primary)' }} /></div>
              {!capacityValid && <span style={{ color: 'var(--status-warning)', fontSize: '0.8rem' }}>容量必须是大于 0 的整数 MB。</span>}
              {capture.limitReached && <span style={{ color: 'var(--status-warning)', fontSize: '0.8rem' }}>已达到容量上限并自动停止；已捕获内容仍然保留。</span>}
            </div>
            <div className="diagnostics-capture-content">
              {capture.records.length ? (
                <>
                  <div className="diagnostics-capture-selector-row">
                    <select className="form-select diagnostics-capture-select" value={selectedCaptureIndex?.requestID || ''} onChange={(event) => setSelectedCaptureID(event.target.value)} aria-label="选择诊断捕获请求">
                      <option value="">选择请求以查看索引摘要…</option>
                      {capture.records.map((record) => <option key={record.requestID} value={record.requestID}>{formatTimestamp(record.timestamp, { date: true })} · {record.clientModel || '未知模型'} · {record.statusCode ?? '进行中'} · {record.requestID}</option>)}
                    </select>
                    <span className="diagnostics-helper">单条导出仅针对当前选中；索引每 5 秒自动刷新，切换记录不会读取大正文，完整 Headers、Body 与 Chunk 需显式加载</span>
                    <button type="button" className="btn btn-secondary" disabled={!captureDetailMatchesSelection} onClick={copyCapture} title="复制当前选中的源诊断 JSON">复制源数据 JSON</button>
                  </div>
                  <div className="diagnostics-export-wizard" aria-label="诊断导出向导">
                    <div className="diagnostics-export-heading"><strong>导出向导</strong><span>下载全部捕获快照也必须经过范围、格式和隐私确认；估算 {formatNumber(capture.recordCount)} 条 · 约 {formatBytes(capture.capturedBytes || 0)}</span></div>
                    <div className="diagnostics-export-fields">
                      <label><span>范围</span><select className="form-select" value={captureExportScope} onChange={(event) => setCaptureExportScope(event.target.value)}><option value="selected">当前选中请求</option><option value="all">全部捕获（服务端流式）</option></select></label>
                      <label><span>格式</span><select className="form-select" value={captureExportFormat} onChange={(event) => setCaptureExportFormat(event.target.value)}><option value="jsonl">JSONL（逐条）</option><option value="json">JSON</option></select></label>
                      <label><span>数据</span><select className="form-select" value={captureExportPrivacy} onChange={(event) => { setCaptureExportPrivacy(event.target.value); setCaptureExportConfirmRaw(false); }}><option value="raw">源数据（默认）</option><option value="redacted">脱敏副本</option></select></label>
                      <button type="button" className="btn btn-primary diagnostics-export-button" disabled={captureExportBusy || (captureExportScope !== 'all' && !selectedCaptureID)} onClick={startCaptureExport}>{captureExportBusy ? '准备导出…' : '开始流式导出'}</button>
                    </div>
                    {captureExportPrivacy === 'raw' && <label className="diagnostics-raw-confirm"><input type="checkbox" checked={captureExportConfirmRaw} onChange={(event) => setCaptureExportConfirmRaw(event.target.checked)} /> <span>我确认导出可能包含鉴权头、Cookie、URL 密钥、请求体和响应正文，并承担泄露风险。</span></label>}
                  </div>
                  {captureDetailLoading ? <p style={{ color: 'var(--text-secondary)' }}>正在读取选中请求详情…</p> : captureDetailError ? <div role="alert" className="diagnostics-index-preview"><span style={{ color: 'var(--status-warning)' }}>{captureDetailError}</span><button type="button" className="btn btn-secondary" onClick={loadCaptureDetail}>重试读取</button></div> : captureDetailMatchesSelection ? (
                    <div className="diagnostics-detail-stack">
                      <div className="grid-3col diagnostics-detail-grid">
                        <div>请求：<strong>{captureDetail.method} {captureDetail.path}</strong></div>
                        <div>模型：<strong>{captureDetail.clientModel || '-'} → {captureDetail.effectiveModel || '-'}</strong></div>
                        <div>结果：<strong>{captureDetail.statusCode ?? '进行中'} / {captureDetail.outcome || '-'}</strong></div>
                        <div>用途：<strong>{captureDetail.requestPurpose || '-'}</strong></div>
                        <div>耗时：<strong>{captureDetail.completedAtMS == null ? '进行中' : `${captureDetail.completedAtMS} ms`}</strong></div>
                        <div>项目：<strong>{captureProject ? `${captureProject.label} · ${projectSourceLabel('client_declared')}` : '未声明（Codex 工作区见入站 Body 的 client_metadata）'}</strong></div>
                      </div>
                      {(captureDetail.truncated || captureDetail.inboundBodyTruncated) && <StatusBadge text="达到内存上限，后续内容已截断" kind="warning" />}
                      {(captureDetail.failureKind || captureDetail.failureDetail) && <RawBlock title={`错误 · ${captureDetail.failureKind || 'unknown'}`} value={captureDetail.failureDetail} />}
                      <RawBlock title="入站请求 Headers" valueFactory={() => headersText(captureDetail.inboundHeaders)} />
                      <RawBlock title={`入站原始 Body · ${captureDetail.inboundBodyBytes || 0} bytes`} value={captureDetail.inboundBody} />
                      {(captureDetail.attempts || []).map((attempt, index) => <details key={attempt.id || index} className="diagnostics-attempt"><summary>上游尝试 #{index + 1} · {attempt.endpointName || attempt.endpointID || '-'} · {attempt.responseStatus ?? attempt.error ?? '等待响应'}</summary><div className="diagnostics-attempt-stack"><RawBlock title={`${attempt.outboundMethod || '请求'} ${attempt.outboundURL || ''} Headers`} valueFactory={() => headersText(attempt.outboundHeaders)} /><RawBlock title={`出站原始 Body · ${attempt.outboundBodyBytes || 0} bytes`} value={attempt.outboundBody} /><RawBlock title={`上游响应 Headers · HTTP ${attempt.responseStatus ?? '-'}`} valueFactory={() => headersText(attempt.responseHeaders)} />{attempt.error && <RawBlock title="尝试错误" value={attempt.error} />}<RawBlock title={`原始上游 Chunks · ${(attempt.upstreamChunks || []).length} 个`} valueFactory={() => chunksText(attempt.upstreamChunks)} /></div></details>)}
                      <RawBlock title={`桥接后客户端 Chunks · ${(captureDetail.clientChunks || []).length} 个`} valueFactory={() => chunksText(captureDetail.clientChunks)} />
                      <p style={{ margin: 0, color: 'var(--text-muted)', fontSize: '0.78rem' }}>
                        完整源 JSON 不在详情页自动编码；复制和导出默认保留源数据。需要生成脱敏副本时，请在上方导出向导主动选择“脱敏副本”；服务端会逐条流式生成并支持大文件背压。
                      </p>
                    </div>
                  ) : selectedCaptureIndex ? (
                    <div className="diagnostics-index-preview">
                      <div><strong>{selectedCaptureIndex.method || '请求'} {selectedCaptureIndex.path || ''}</strong><span className="mono-cell">{selectedCaptureIndex.requestID}</span></div>
                      <p>{selectedCaptureIndex.clientModel || '未知模型'} · {selectedCaptureIndex.statusCode ?? '进行中'} · {selectedCaptureIndex.attemptCount ?? 0} 次上游尝试 · {selectedCaptureIndex.clientChunkCount ?? 0} 个客户端 Chunk</p>
                      <p>正文、Headers 与流式 Chunk 尚未读取；这样选择和自动刷新索引不会卡住页面。</p>
                      <button type="button" className="btn btn-primary" onClick={loadCaptureDetail}><Icon name="search" size={14} />读取完整正文与流诊断</button>
                    </div>
                  ) : <p style={{ color: 'var(--text-secondary)' }}>选择一条请求查看索引摘要；需要时再读取完整源数据。</p>}
                </>
              ) : <p style={{ color: 'var(--text-muted)', margin: 0 }}>{capture.enabled ? '正在等待下一次代理请求…' : '开启捕获后，新请求会显示在这里。'}</p>}
            </div>
          </>
        ) : <p style={{ marginTop: '16px', color: 'var(--text-muted)' }}>正在读取诊断捕获索引…</p>}
      </div>

      {/* Diagnostics Grid */}
      <div className="glass-panel panel-padded-stack diagnostics-matrix-panel">
        <div className="panel-title">
          <span>运行环境状态矩阵</span>
        </div>

        {diagnosticsError && (
          <div role="alert" className="diagnostics-alert">
            <Icon name="warning" size={18} />
            <div className="diagnostics-alert-copy">
              <strong>环境诊断暂不可用</strong>
              <div>{diagnosticsError}</div>
            </div>
            <button type="button" className="btn btn-secondary" disabled={loading} onClick={fetchDiag}>重试</button>
          </div>
        )}

        <div className="grid-3col diagnostics-matrix-grid">
          <div>
            <span style={{ color: 'var(--text-muted)' }}>Daemon 版本：</span>
            <strong className="mono-cell" style={{ color: 'var(--text-primary)' }}>{diag?.version || '0.2.0'}</strong>
          </div>
          <div>
            <span style={{ color: 'var(--text-muted)' }}>配置代数 (Generation)：</span>
            <strong className="mono-cell" style={{ color: 'var(--primary)' }}>{diag?.generation || '-'}</strong>
          </div>
          <div>
            <span style={{ color: 'var(--text-muted)' }}>运行时长：</span>
            <strong className="mono-cell" style={{ color: 'var(--text-primary)' }}>{diag?.uptimeSeconds ? formatDuration(diag.uptimeSeconds * 1000) : '-'}</strong>
          </div>
          <div>
            <span style={{ color: 'var(--text-muted)' }}>Admin 管理监听：</span>
            <strong className="mono-cell" style={{ color: 'var(--text-primary)' }}>{formatListenerAddress(diag?.adminListener, '127.0.0.1:57879')}</strong>
          </div>
          <div>
            <span style={{ color: 'var(--text-muted)' }}>代理转发监听：</span>
            <strong className="mono-cell" style={{ color: 'var(--text-primary)' }}>{formatListenerAddress(diag?.proxyListener, '127.0.0.1:57878')}</strong>
          </div>
          <div>
            <span style={{ color: 'var(--text-muted)' }}>统计文件可写性：</span>
            <StatusBadge text={diag?.statsWritable ? '可正常写入' : '不可写'} kind={diag?.statsWritable ? 'good' : 'critical'} />
          </div>
        </div>

        <div className="diagnostics-paths">
          <div><span style={{ color: 'var(--text-muted)' }}>配置文件路径：</span><span className="mono-cell" style={{ color: 'var(--text-primary)' }}>{diag?.configPath || '~/.config/sumpter/config.json'}</span></div>
          <div><span style={{ color: 'var(--text-muted)' }}>WebUI 静态根目录：</span><span className="mono-cell" style={{ color: 'var(--text-primary)' }}>{diag?.webRoot || '~/.local/share/sumpter/web'}</span></div>
        </div>
      </div>

      {/* Warnings & Risks Panel */}
      <div className="glass-panel panel-padded-stack diagnostics-warnings-panel">
        <div className="panel-title">
          <span>系统健康与配置风险检查</span>
        </div>
        {diag?.warnings?.length > 0 ? (
          <div className="diagnostics-warning-list">
            {diag.warnings.map((w, idx) => (
              <div
                key={idx}
                className="diagnostics-warning-item"
              >
                <Icon name="warning" size={16} />
                <span>{typeof w === 'string' ? w : w.message}</span>
              </div>
            ))}
          </div>
        ) : (
          <div className="diagnostics-ok-state">
            <Icon name="check" size={18} />
            <span>服务端自检未发现任何配置风险或异常警告。</span>
          </div>
        )}
      </div>

      {/* systemd Journal Guide */}
      <div className="glass-panel panel-padded-stack diagnostics-journal-panel">
        <div className="panel-toolbar">
          <div className="panel-title">
            <Icon name="activity" size={18} style={{ color: 'var(--primary)' }} />
            <span>systemd 守护进程服务日志</span>
          </div>
          <button
            type="button"
            className="btn btn-secondary"
            onClick={() => copyCommand(journalCmd)}
          >
            <Icon name="copy" size={14} />
            <span>复制命令</span>
          </button>
        </div>
        <p className="panel-description">
          如需深入排查系统级启动崩溃或网络连接异常，可在 Linux 终端中运行以下只读命令查看实时日志：
        </p>
        <pre className="mono-cell diagnostics-journal-command">
          {journalCmd}
        </pre>
      </div>
    </div>
  );
}
