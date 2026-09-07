import React, { useEffect, useRef, useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { DataTable } from '../components/DataTable.jsx';
import { PricingPanel } from '../components/AnalyticsV2Workspace.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { StatusBadge } from '../components/StatusBadge.jsx';
import { Icon } from '../utils/icons.jsx';
import { api } from '../services/api.js';
import { clone, formatTimestamp } from '../utils/helpers.js';
import { ENDPOINT_PROTOCOL_MODES, endpointProtocolLabel, normalizeEndpointProtocol } from '../utils/protocols.js';

function endpointMappings(endpoint) {
  return endpoint?.modelMappings || endpoint?.mappings || [];
}


// Keep catalog IDs aligned with the Rust model-name parser before matching or
// persisting a mapping. Catalogs may expose reasoning/context suffixes; those
// are transport decorations, not distinct client IDs.
function cleanCatalogModel(model) {
  return String(model || '')
    .trim()
    .replace(/\[[^\]]*\]\s*$/, '')
    .replace(/\s*\((?:none|auto|minimal|low|medium|high|xhigh|max)\)\s*$/i, '')
    .trim();
}

function readableCatalogModel(model) {
  return cleanCatalogModel(model);
}

function uniqueCatalogModels(values) {
  if (!Array.isArray(values)) return [];
  const seen = new Set();
  const result = [];
  for (const rawValue of values) {
    const model = readableCatalogModel(rawValue);
    const key = modelKey(model);
    if (!key || seen.has(key)) continue;
    seen.add(key);
    result.push(model);
  }
  return result;
}

function catalogModels(endpoint) {
  return uniqueCatalogModels(endpoint?.catalog?.models);
}

function modelKey(model) {
  return readableCatalogModel(model).toLowerCase();
}

function defaultCatalogContext(model) {
  return /opus|fable/i.test(String(model || '')) ? 'oneMillion' : 'passThrough';
}

function catalogTimestamp(value) {
  if (!value) return '';
  const formatted = formatTimestamp(value, { date: true });
  return formatted === '-' ? String(value) : formatted;
}

function mappingClient(mapping) {
  return String(mapping?.from ?? mapping?.clientPattern ?? '').trim();
}

function mappingUpstream(mapping) {
  return String(mapping?.to ?? mapping?.upstreamModel ?? '').trim();
}

function LocalToggle({ initial, label, title, ariaLabel, onChange }) {
  const [checked, setChecked] = useState(Boolean(initial));
  return (
    <QuickToggle
      checked={checked}
      onChange={(value) => {
        setChecked(value);
        onChange?.(value);
      }}
      label={typeof label === 'function' ? label(checked) : (label || (checked ? '已启用' : '已停用'))}
      title={title}
      ariaLabel={ariaLabel}
    />
  );
}

// Keep the retry_delay input and its pass-through switch in one stateful
// surface.  The modal itself stores draft values in a closure for the save
// action, but the input disabled state must still react immediately when the
// user flips the switch.
// 映射编辑器里的 Thinking/effort 联动控件。modal 的 content 是打开时创建的静态
// JSX，闭包变量改写不会触发重渲染；切到「自适应」时必须靠组件内 state 才能让
// effort 覆盖选择器动态出现。
function MappingThinkingControls({ thinking: initialThinking, effort: initialEffort, onThinking, onEffort }) {
  const [thinking, setThinking] = useState(initialThinking || 'disable');
  const [effort, setEffort] = useState(initialEffort || 'auto');
  return (
    <div className="form-group">
      <label className="form-label">思考模式 (Thinking)</label>
      <select
        className="form-select"
        value={thinking}
        onChange={(e) => {
          setThinking(e.target.value);
          if (onThinking) onThinking(e.target.value);
        }}
      >
        <option value="adaptive">自适应 (Adaptive)</option>
        <option value="passThrough">透传 (PassThrough)</option>
        <option value="disable">禁用 (Disable)</option>
      </select>
      {thinking === 'adaptive' && (
        <>
          <label className="form-label" style={{ marginTop: 8 }}>思考级别覆盖</label>
          <select
            className="form-select"
            value={effort}
            onChange={(e) => {
              setEffort(e.target.value);
              if (onEffort) onEffort(e.target.value);
            }}
          >
            <option value="auto">自动（跟随客户端）</option>
            <option value="low">Low</option><option value="medium">Medium</option>
            <option value="high">High</option><option value="xhigh">Xhigh</option>
            <option value="max">Max</option><option value="ultra">Ultra</option>
          </select>
        </>
      )}
    </div>
  );
}

function RetryDelayControls({ initialSeconds, initialEnabled, onChange }) {
  const [seconds, setSeconds] = useState(initialSeconds ?? '');
  const [enabled, setEnabled] = useState(Boolean(initialEnabled));

  const update = (nextSeconds, nextEnabled) => {
    onChange?.({ seconds: nextSeconds, enabled: nextEnabled });
  };

  return (
    <>
      <div className="form-group">
        <label className="form-label">retry_delay 秒数</label>
        <input
          type="number"
          min="0.1"
          step="0.1"
          className="form-input"
          value={seconds}
          placeholder="例如 2.5"
          disabled={!enabled}
          onChange={(event) => {
            const nextSeconds = event.target.value ? Number(event.target.value) : '';
            setSeconds(nextSeconds);
            update(nextSeconds, enabled);
          }}
        />
        <span className="form-hint">填写秒数后，最终失败响应可返回 retry_delay 数字字段。</span>
      </div>

      <div className="form-group">
        <label className="form-label">透传 retry_delay</label>
        <LocalToggle
          initial={enabled}
          label={(checked) => checked ? '已开启' : '已关闭'}
          title="控制最终失败响应是否带回 retry_delay 与 Retry-After"
          ariaLabel="透传 retry_delay 与 Retry-After"
          onChange={(nextEnabled) => {
            setEnabled(nextEnabled);
            update(seconds, nextEnabled);
          }}
        />
        <span className="form-hint">关闭后即使填写秒数，也不会把 retry_delay 或 Retry-After 返回给客户端。</span>
      </div>
    </>
  );
}

const PROVIDER_DRAG_INTERACTIVE_SELECTOR = 'button,input,select,textarea,a,summary,[role="button"]';

function providerDropPosition(event) {
  const bounds = event.currentTarget.getBoundingClientRect();
  return event.clientY >= bounds.top + bounds.height / 2 ? 'after' : 'before';
}

/**
 * Build a native drag image from the complete table row.  A row clone keeps
 * the preview aligned with the destination row and makes it obvious that the
 * whole Provider record (not one cell) is being moved.
 */
function createProviderDragImage(sourceRow) {
  if (!sourceRow || typeof document === 'undefined') return null;
  const bounds = sourceRow.getBoundingClientRect();
  const clone = sourceRow.cloneNode(true);
  clone.removeAttribute('aria-selected');
  clone.classList.add('provider-drag-preview-row');
  const preview = sourceRow.tagName === 'TR' ? document.createElement('table') : document.createElement('div');
  preview.className = `provider-drag-preview${sourceRow.tagName === 'TR' ? ' data-table' : ''}`;
  preview.style.width = `${Math.max(280, Math.round(bounds.width))}px`;
  preview.style.maxWidth = `${Math.max(280, Math.round(bounds.width))}px`;
  if (sourceRow.tagName === 'TR') {
    const body = document.createElement('tbody');
    body.appendChild(clone);
    preview.appendChild(body);
  } else {
    preview.appendChild(clone);
  }
  preview.style.position = 'fixed';
  preview.style.left = '-10000px';
  preview.style.top = '-10000px';
  document.body.appendChild(preview);
  return preview;
}

function CatalogModelPicker({ models, onChange }) {
  const [selected, setSelected] = useState(() => new Set(models));

  const update = (next) => {
    setSelected(next);
    onChange?.([...next]);
  };

  const toggle = (model) => {
    const next = new Set(selected);
    if (next.has(model)) next.delete(model); else next.add(model);
    update(next);
  };

  const toggleAll = () => {
    update(selected.size === models.length ? new Set() : new Set(models));
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: '12px' }}>
        <span className="form-hint">选择目录中尚未映射的模型；默认全选。共 {models.length} 个。</span>
        <button type="button" className="btn btn-secondary btn-compact" onClick={toggleAll}>
          {selected.size === models.length ? '取消全选' : '全选'}
        </button>
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(230px, 1fr))',
          gap: '8px',
          maxHeight: '300px',
          overflowY: 'auto',
          padding: '4px',
        }}
      >
        {models.map((model) => (
          <label
            key={model}
            style={{ display: 'flex', alignItems: 'center', gap: '8px', padding: '8px 10px', border: '1px solid var(--border-subtle)', borderRadius: 'var(--radius-sm)', cursor: 'pointer' }}
          >
            <input type="checkbox" checked={selected.has(model)} onChange={() => toggle(model)} />
            <span className="mono-cell" style={{ overflowWrap: 'anywhere', fontSize: '0.8rem' }}>{model}</span>
          </label>
        ))}
      </div>
      <span className="form-hint">将按入口协议和模型映射生成客户端模型名与上游模型名。</span>
    </div>
  );
}

function EndpointPricingEditor({ endpoint, config, onSaved }) {
  const [pricing, setPricing] = useState(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  const loadPricing = async () => {
    setLoading(true);
    setError(null);
    try {
      setPricing(await api.getRuntimePricing());
    } catch (nextError) {
      setError(nextError);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadPricing();
    // The modal is opened for one stable endpoint; do not refetch on each
    // parent config refresh while the user is editing a price draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [endpoint?.id]);

  const savePricing = async (payload) => {
    try {
      const value = await api.updateRuntimePricing(payload);
      setPricing((current) => ({
        ...(current || {}),
        revision: value.revision,
        currency: value.currency,
      }));
      onSaved?.(value);
      await loadPricing();
    } catch (nextError) {
      setError(nextError);
    }
  };

  return (
    <PricingPanel
      pricing={pricing}
      config={config}
      endpointID={endpoint?.id || ''}
      loading={loading}
      error={error}
      onRetry={loadPricing}
      onSave={savePricing}
    />
  );
}

export function PrimaryProvidersPage() {
  const { config, secretStatus, saveConfig, openModal, addToast } = useApp();
  const [selectedEndpointID, setSelectedEndpointID] = useState(null);
  const [fetchingModelEndpointIDs, setFetchingModelEndpointIDs] = useState(() => new Set());
  const [reorderingEndpointID, setReorderingEndpointID] = useState(null);
  const [providerDragState, setProviderDragState] = useState({
    sourceID: null,
    targetID: null,
    position: null,
  });
  const reorderTimerRef = useRef(null);
  const providerDragPreviewRef = useRef(null);
  const modelFetchAbortControllersRef = useRef(new Map());

  const clearProviderDrag = () => {
    setProviderDragState({ sourceID: null, targetID: null, position: null });
    if (providerDragPreviewRef.current?.parentNode) {
      providerDragPreviewRef.current.parentNode.removeChild(providerDragPreviewRef.current);
    }
    providerDragPreviewRef.current = null;
  };

  useEffect(() => () => {
    if (reorderTimerRef.current) window.clearTimeout(reorderTimerRef.current);
    for (const controller of modelFetchAbortControllersRef.current.values()) controller.abort();
    modelFetchAbortControllersRef.current.clear();
    if (providerDragPreviewRef.current?.parentNode) {
      providerDragPreviewRef.current.parentNode.removeChild(providerDragPreviewRef.current);
    }
  }, []);

  const endpoints = config?.endpoints || [];
  const retry = config?.retry || {};
  const selectedEndpoint = endpoints.find((e) => e.id === selectedEndpointID) || endpoints[0];
  const enabledEndpointCount = endpoints.filter((endpoint) => endpoint.enabled !== false).length;
  const mappedModelCount = endpoints.reduce((total, endpoint) => total + endpointMappings(endpoint).length, 0);
  const catalogModelCount = new Set(
    endpoints.flatMap((endpoint) => catalogModels(endpoint).map((model) => modelKey(model))),
  ).size;

  const setModelFetchState = (endpointID, fetching) => {
    setFetchingModelEndpointIDs((previous) => {
      const next = new Set(previous);
      if (fetching) next.add(endpointID); else next.delete(endpointID);
      return next;
    });
  };

  const cancelModelFetch = (endpointID) => {
    modelFetchAbortControllersRef.current.get(endpointID)?.abort();
  };

  const handleFetchModels = async (endpoint) => {
    if (!endpoint?.id || fetchingModelEndpointIDs.has(endpoint.id)
      || modelFetchAbortControllersRef.current.has(endpoint.id)) return;
    const controller = new AbortController();
    modelFetchAbortControllersRef.current.set(endpoint.id, controller);
    setModelFetchState(endpoint.id, true);
    try {
      const result = await api.fetchProviderModels(endpoint.id, { signal: controller.signal });
      const models = uniqueCatalogModels(result?.models);
      await saveConfig((latestConfig) => {
        const target = latestConfig.endpoints?.find((item) => item.id === endpoint.id);
        if (!target) throw new Error('入口已不存在，请刷新页面');
        target.catalog = {
          ...(target.catalog || {}),
          models,
          source: result?.source || 'api',
          status: '已获取',
          error: '',
          updatedAt: result?.updatedAt || new Date().toISOString(),
        };
        return latestConfig;
      });
      addToast(`已获取 ${models.length} 个模型`, 'success');
    } catch (err) {
      if (err?.name === 'AbortError') return;
      const message = String(err?.message || err || '获取模型失败').slice(0, 300);
      try {
        await saveConfig((latestConfig) => {
          const target = latestConfig.endpoints?.find((item) => item.id === endpoint.id);
          if (!target) return latestConfig;
          target.catalog = {
            ...(target.catalog || {}),
            status: '获取失败',
            error: message,
            updatedAt: new Date().toISOString(),
          };
          return latestConfig;
        });
      } catch {
        // 保留原始抓取错误；目录状态保存失败不应覆盖它。
      }
      addToast(`获取模型失败: ${message}`, 'error');
    } finally {
      modelFetchAbortControllersRef.current.delete(endpoint.id);
      setModelFetchState(endpoint.id, false);
    }
  };

  const openCatalogMappingEditor = (endpoint = selectedEndpoint) => {
    if (!endpoint) return;
    const mapped = new Set(endpointMappings(endpoint).map((mapping) => modelKey(mappingClient(mapping))));
    const unmapped = catalogModels(endpoint).filter((model, index, all) => {
      const key = modelKey(model);
      return key && !mapped.has(key) && all.findIndex((candidate) => modelKey(candidate) === key) === index;
    });
    if (!unmapped.length) {
      addToast('没有尚未映射的目录模型，请先获取模型或检查现有映射', 'info');
      return;
    }
    let selectedModels = [...unmapped];
    openModal({
      title: `从已知模型添加映射 · ${endpoint.name}`,
      maxWidth: '720px',
      content: (
        <CatalogModelPicker models={unmapped} onChange={(models) => { selectedModels = models; }} />
      ),
      actions: [
        { label: '取消', kind: 'ghost' },
        {
          label: '添加选中映射',
          kind: 'primary',
          onClick: async () => {
            if (!selectedModels.length) {
              addToast('至少选择一个模型', 'warning');
              return true;
            }
            const nextConfig = clone(config);
            const target = nextConfig.endpoints?.find((item) => item.id === endpoint.id);
            if (!target) {
              addToast('入口已不存在，请刷新页面', 'error');
              return true;
            }
            const existingMappings = endpointMappings(target);
            const existing = new Set(existingMappings.map((mapping) => modelKey(mappingClient(mapping))));
            target.modelMappings = [...existingMappings];
            for (const raw of selectedModels) {
              const client = readableCatalogModel(raw);
              const upstream = readableCatalogModel(raw);
              const key = modelKey(client);
              if (!client || existing.has(key)) continue;
              target.modelMappings.push({
                from: client,
                to: upstream,
                thinking: 'passThrough',
                context: defaultCatalogContext(client),
              });
              existing.add(key);
            }
            await saveConfig(nextConfig);
            addToast(`已添加 ${target.modelMappings.length - existingMappings.length} 条模型映射`, 'success');
            return false;
          },
        },
      ],
    });
  };

  // Toggle Endpoint Enable/Disable
  const handleToggleEndpoint = async (endpointId, enabled) => {
    try {
      const nextConfig = clone(config);
      const targetEp = nextConfig.endpoints.find((e) => e.id === endpointId);
      if (targetEp) {
        targetEp.enabled = enabled;
        await saveConfig(nextConfig);
        addToast(`入口「${targetEp.name}」已${enabled ? '启用' : '停用'}`, 'success');
      }
    } catch (err) {
      addToast(`操作失败: ${err.message}`, 'error');
    }
  };

  // Move Endpoint Up / Down
  const handleMoveEndpoint = async (index, direction) => {
    const newIndex = index + direction;
    if (newIndex < 0 || newIndex >= endpoints.length) return;
    try {
      const nextConfig = clone(config);
      const [moved] = nextConfig.endpoints.splice(index, 1);
      nextConfig.endpoints.splice(newIndex, 0, moved);
      await saveConfig(nextConfig);
      addToast('同优先级入口顺序已调整', 'success');
    } catch (err) {
      addToast(`调整失败: ${err.message}`, 'error');
    }
  };

  const handleReorderEndpoint = async (draggedID, targetID, placeAfter) => {
    if (reorderingEndpointID || draggedID === targetID) return;
    const orderedIDs = endpoints.map((endpoint) => endpoint.id);
    const sourceIndex = orderedIDs.indexOf(draggedID);
    const targetIndex = orderedIDs.indexOf(targetID);
    if (sourceIndex < 0 || targetIndex < 0) return;

    let insertionIndex = targetIndex + (placeAfter ? 1 : 0);
    if (sourceIndex < insertionIndex) insertionIndex -= 1;
    if (insertionIndex === sourceIndex) return;

    // Defer the write until the browser drag session has fully closed. This
    // keeps the visible table stable and avoids racing a row-selection update.
    setReorderingEndpointID(draggedID);
    if (reorderTimerRef.current) window.clearTimeout(reorderTimerRef.current);
    reorderTimerRef.current = window.setTimeout(async () => {
      reorderTimerRef.current = null;
      try {
        const nextConfig = clone(config);
        const [moved] = nextConfig.endpoints.splice(sourceIndex, 1);
        nextConfig.endpoints.splice(insertionIndex, 0, moved);
        await saveConfig(nextConfig);
        addToast('入口顺序已保存', 'success');
      } catch (err) {
        addToast(`入口顺序保存失败: ${err.message}`, 'error');
      } finally {
        setReorderingEndpointID(null);
      }
    }, 120);
  };

  const providerRowProps = (row) => {
    const isDragSource = providerDragState.sourceID === row.id;
    const isDropTarget = providerDragState.targetID === row.id && !isDragSource;
    const disabled = Boolean(reorderingEndpointID);
    const dropClass = isDropTarget && providerDragState.position
      ? ` provider-drop-${providerDragState.position}`
      : '';

    return {
      draggable: !disabled,
      className: `provider-draggable-row${isDragSource ? ' provider-row-dragging' : ''}${dropClass}${disabled ? ' provider-row-reordering' : ''}`,
      'aria-grabbed': isDragSource || undefined,
      title: disabled ? '正在保存入口顺序…' : `拖动入口「${row.name || row.id}」调整顺序`,
      onDragStart: (event) => {
        if (disabled) {
          event.preventDefault();
          return;
        }
        // Keep switches, buttons, links and fields as native controls. The
        // row remains draggable from its blank/read-only areas and from the
        // visible grip in the name cell.
        if (event.target.closest?.(PROVIDER_DRAG_INTERACTIVE_SELECTOR)) {
          event.preventDefault();
          return;
        }
        event.dataTransfer.effectAllowed = 'move';
        event.dataTransfer.setData('text/plain', row.id);
        const preview = createProviderDragImage(event.currentTarget);
        if (preview) {
          const bounds = event.currentTarget.getBoundingClientRect();
          const offsetX = Math.max(8, Math.min(bounds.width - 8, event.clientX - bounds.left));
          const offsetY = Math.max(8, Math.min(bounds.height - 8, event.clientY - bounds.top));
          event.dataTransfer.setDragImage(preview, offsetX, offsetY);
          providerDragPreviewRef.current = preview;
        }
        setProviderDragState({ sourceID: row.id, targetID: null, position: null });
      },
      onDragOver: (event) => {
        if (disabled || !providerDragState.sourceID) return;
        event.preventDefault();
        event.stopPropagation();
        if (providerDragState.sourceID === row.id) {
          setProviderDragState((current) => ({ ...current, targetID: null, position: null }));
          return;
        }
        event.dataTransfer.dropEffect = 'move';
        const position = providerDropPosition(event);
        setProviderDragState((current) => (
          current.targetID === row.id && current.position === position
            ? current
            : { ...current, targetID: row.id, position }
        ));
      },
      onDragLeave: (event) => {
        if (providerDragState.targetID !== row.id) return;
        const bounds = event.currentTarget.getBoundingClientRect();
        const inside = event.clientX >= bounds.left && event.clientX <= bounds.right
          && event.clientY >= bounds.top && event.clientY <= bounds.bottom;
        if (!inside) {
          setProviderDragState((current) => (
            current.targetID === row.id ? { ...current, targetID: null, position: null } : current
          ));
        }
      },
      onDrop: (event) => {
        if (disabled) return;
        event.preventDefault();
        event.stopPropagation();
        const draggedID = providerDragState.sourceID || event.dataTransfer.getData('text/plain');
        const position = providerDropPosition(event);
        clearProviderDrag();
        if (draggedID && draggedID !== row.id) {
          handleReorderEndpoint(draggedID, row.id, position === 'after');
        }
      },
      onDragEnd: clearProviderDrag,
    };
  };

  // Delete Endpoint
  const handleDeleteEndpoint = async (endpoint) => {
    if (!confirm(`确定删除上游入口「${endpoint.name}」及其映射规则吗？`)) return;
    try {
      const nextConfig = clone(config);
      nextConfig.endpoints = nextConfig.endpoints.filter((e) => e.id !== endpoint.id);
      await saveConfig(nextConfig);
      addToast('入口已删除', 'success');
    } catch (err) {
      addToast(`删除失败: ${err.message}`, 'error');
    }
  };

  // Open Global Retry Policy Editor Modal
  const openRetryEditor = () => {
    let responseTimeout = retry.responseTimeoutSeconds ?? '';
    let streamIdleTimeout = retry.streamIdleTimeoutSeconds ?? '';
    let stickyRetries = retry.sessionStickyRetries ?? 2;
    let maxRounds = retry.maxDeferredRounds ?? retry.crossRoundRetries ?? 3;
    let maxDuration = retry.maxRetryDurationSeconds ?? 0;
    let max500Retries = retry.max500Retries ?? 0;
    let failoverOn500 = retry.failoverOn500 ?? true;
    let retryDelaySeconds = retry.retryDelaySeconds ?? '';
    let passThroughRetryDelay = retry.passThroughRetryDelay ?? true;

    openModal({
      title: '编辑全局转发与重试策略',
      content: (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '18px' }}>
          <div style={{ fontSize: '0.85rem', color: 'var(--text-muted)' }}>
            这些参数作用于所有 Provider 入口；可分别控制超时、HTTP 500 处理、错误重试轮数，以及是否向客户端透传 retry_delay。
          </div>

          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label">首响应截止（秒）</label>
              <input
                type="number"
                min="0.1"
                step="0.1"
                className="form-input"
                defaultValue={responseTimeout}
                placeholder="留空由客户端决定"
                onChange={(e) => { responseTimeout = e.target.value ? Number(e.target.value) : null; }}
              />
              <span className="form-hint">连接、TLS 握手与首包响应头总截止。</span>
            </div>

            <div className="form-group">
              <label className="form-label">流式空闲截止（秒）</label>
              <input
                type="number"
                min="0.1"
                step="0.1"
                className="form-input"
                defaultValue={streamIdleTimeout}
                placeholder="留空允许无限空闲"
                onChange={(e) => { streamIdleTimeout = e.target.value ? Number(e.target.value) : null; }}
              />
              <span className="form-hint">首响应后两次流式 chunk 间的最长间隔。</span>
            </div>
          </div>

          <div className="retry-policy-section-title">HTTP 500 处理</div>

          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label">500 失败后切换入口</label>
              <LocalToggle
                initial={failoverOn500}
                label={(checked) => checked ? '已开启' : '已关闭'}
                title="控制 HTTP 500 重试耗尽后是否切换到下一个入口"
                ariaLabel="HTTP 500 失败后切换入口"
                onChange={(value) => { failoverOn500 = value; }}
              />
              <span className="form-hint">开启：重试耗尽后继续下一个入口；关闭：在当前入口直接返回 500。</span>
            </div>

            <div className="form-group">
              <label className="form-label">入口内 500 重试</label>
              <input
                type="number"
                min="0"
                step="1"
                className="form-input"
                defaultValue={max500Retries}
                onChange={(e) => { max500Retries = Number(e.target.value); }}
              />
              <span className="form-hint">仅针对同一入口连续收到的 HTTP 500；0 表示不额外重试。</span>
            </div>

            <RetryDelayControls
              initialSeconds={retryDelaySeconds}
              initialEnabled={passThroughRetryDelay}
              onChange={({ seconds, enabled }) => {
                retryDelaySeconds = seconds;
                passThroughRetryDelay = enabled;
              }}
            />
          </div>

          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label">粘性入口额外重试</label>
              <input
                type="number"
                min="0"
                step="1"
                className="form-input"
                defaultValue={stickyRetries}
                onChange={(e) => { stickyRetries = Number(e.target.value); }}
              />
              <span className="form-hint">设置 2 = 非 500 可重试故障后额外重试 2 次；全部遇到可重试故障才切换其它组，成功后立即改绑。</span>
            </div>

            <div className="form-group">
              <label className="form-label">故障重试最大轮数</label>
              <input
                type="number"
                min="0"
                step="1"
                className="form-input"
                defaultValue={maxRounds}
                onChange={(e) => { maxRounds = Number(e.target.value); }}
              />
              <span className="form-hint">0 表示不限制轮数。</span>
            </div>
          </div>

          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label">跨轮最长时长（秒）</label>
              <input
                type="number"
                min="0"
                step="0.1"
                className="form-input"
                defaultValue={maxDuration}
                placeholder="0 表示不限总时长"
                onChange={(e) => { maxDuration = Number(e.target.value); }}
              />
              <span className="form-hint">超时后停止重试并返回最后错误。</span>
            </div>
          </div>
        </div>
      ),
      actions: [
        { label: '取消', kind: 'ghost' },
        {
          label: '保存重试策略',
          kind: 'primary',
          onClick: async () => {
            const responseTimeoutNumber = responseTimeout == null || responseTimeout === '' ? null : Number(responseTimeout);
            const streamIdleTimeoutNumber = streamIdleTimeout == null || streamIdleTimeout === '' ? null : Number(streamIdleTimeout);
            const stickyRetriesNumber = Number(stickyRetries);
            const maxRoundsNumber = Number(maxRounds);
            const maxDurationNumber = Number(maxDuration);
            const max500RetriesNumber = Number(max500Retries);
            const retryDelaySecondsNumber = retryDelaySeconds == null || retryDelaySeconds === '' ? null : Number(retryDelaySeconds);
            if ((responseTimeoutNumber != null && (!Number.isFinite(responseTimeoutNumber) || responseTimeoutNumber <= 0))
              || (streamIdleTimeoutNumber != null && (!Number.isFinite(streamIdleTimeoutNumber) || streamIdleTimeoutNumber <= 0))) {
              addToast('首响应截止和流式空闲截止必须留空或填写大于 0 的数字', 'warning');
              return true;
            }
            if (!Number.isInteger(max500RetriesNumber) || max500RetriesNumber < 0
              || (retryDelaySecondsNumber != null && (!Number.isFinite(retryDelaySecondsNumber) || retryDelaySecondsNumber <= 0))
              || !Number.isInteger(stickyRetriesNumber) || stickyRetriesNumber < 0
              || !Number.isInteger(maxRoundsNumber) || maxRoundsNumber < 0
              || !Number.isFinite(maxDurationNumber) || maxDurationNumber < 0) {
              addToast('入口内 500 重试、故障轮数和粘性重试必须是非负整数；retry_delay 秒数和超时必须大于 0', 'warning');
              return true;
            }
            const nextConfig = clone(config);
            nextConfig.retry = {
              responseTimeoutSeconds: responseTimeoutNumber,
              streamIdleTimeoutSeconds: streamIdleTimeoutNumber,
              max500Retries: max500RetriesNumber,
              failoverOn500,
              retryDelaySeconds: retryDelaySecondsNumber,
              passThroughRetryDelay,
              sessionStickyRetries: stickyRetriesNumber,
              maxDeferredRounds: maxRoundsNumber,
              maxRetryDurationSeconds: maxDurationNumber,
            };
            await saveConfig(nextConfig);
            return false;
          },
        },
      ],
    });
  };

  const openPricingEditor = (endpoint) => {
    if (!endpoint?.id) return;
    openModal({
      title: `成本价格 · ${endpoint.name || endpoint.id}`,
      maxWidth: '1120px',
      content: (
        <EndpointPricingEditor
          endpoint={endpoint}
          config={config}
          onSaved={(value) => addToast(`价格表已替换（${value?.priceCount ?? 0} 条）`, 'success')}
        />
      ),
      actions: [{ label: '关闭', kind: 'ghost' }],
    });
  };

  // Open Edit Endpoint Modal with Advanced Settings & Mappings
  const openEndpointEditor = async (endpoint = null) => {
    const isNew = !endpoint;
    let endpointID = endpoint?.id || '';
    let name = endpoint?.name || '';
    let baseURL = endpoint?.baseURL || '';
    let protocol = normalizeEndpointProtocol(endpoint?.protocol, 'auto');
    let apiKey = '';
    let priority = endpoint?.priority ?? endpoints.length;
    let enabled = endpoint?.enabled !== false;
    // 新入口默认开启；编辑已有入口时严格保留其显式配置（缺省旧配置仍为关闭）。
    let keepAlive = isNew ? true : endpoint?.keepAlive === true;
    let stickyGroup = endpoint?.stickyGroup || '';
    let modelMappings = clone(endpointMappings(endpoint));
    let apiKeyTouched = false;

    // Try fetching existing secret if editing
    let existingSecretStatus = secretStatus?.endpoints?.[endpoint?.id];
    let fetchedKey = '';
    if (!isNew && endpoint?.id) {
      try {
        const res = await api.getEndpointSecret(endpoint.id);
        if (res?.apiKey) fetchedKey = res.apiKey;
      } catch {
        // ignore
      }
    }

    openModal({
      title: isNew ? '添加 Provider 入口' : `编辑入口 · ${endpoint.name}`,
      maxWidth: '680px',
      content: (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label">入口 ID {isNew ? '' : '(只读)'}</label>
              {isNew ? (
                <input
                  type="text"
                  className="form-input"
                  defaultValue=""
                  placeholder="留空则自动生成"
                  onChange={(e) => { endpointID = e.target.value; }}
                />
              ) : (
                <input type="text" className="form-input" value={endpointID} readOnly aria-readonly="true" />
              )}
              <span className="form-hint">只允许字母、数字、点、下划线和连字符；保存后不可修改。</span>
            </div>

            <div className="form-group">
              <label className="form-label">入口状态</label>
              <LocalToggle initial={enabled} onChange={(value) => { enabled = value; }} label={(value) => (value ? '已启用' : '已停用')} title="切换入口是否参与调度" ariaLabel="切换入口是否参与调度" />
            </div>
          </div>

          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label">入口名称 *</label>
              <input
                type="text"
                className="form-input"
                defaultValue={name}
                placeholder="如：Anthropic 官方直连"
                onChange={(e) => { name = e.target.value; }}
              />
            </div>

            <div className="form-group">
              <label className="form-label">调度优先级 (Priority)</label>
              <input
                type="number"
                className="form-input"
                defaultValue={priority}
                onChange={(e) => { priority = Number(e.target.value); }}
              />
              <span className="form-hint">数值越小越优先调度。</span>
            </div>
          </div>

          <div className="form-group">
            <label className="form-label">API 地址 *</label>
            <input
              type="text"
              className="form-input"
              defaultValue={baseURL}
              placeholder="https://api.anthropic.com"
              onChange={(e) => { baseURL = e.target.value; }}
            />
          </div>

          <div className="form-group">
            <label className="form-label">API Key 密钥</label>
            <input
              type="password"
              className="form-input"
              defaultValue={fetchedKey}
              placeholder={existingSecretStatus?.configured ? `已保存 (尾号 ${existingSecretStatus.last4}) · 输入以替换` : '输入上游 API Key'}
              onChange={(e) => { apiKey = e.target.value; apiKeyTouched = true; }}
            />
            <span className="form-hint">编辑时清空再保存，会移除该入口已保存的 Key。</span>
          </div>

          <div className="form-group">
            <label className="form-label">入口协议</label>
            <select
              className="form-select"
              defaultValue={protocol}
              onChange={(e) => { protocol = normalizeEndpointProtocol(e.target.value, 'auto'); }}
            >
              {ENDPOINT_PROTOCOL_MODES.map((option) => (
                <option key={option.value} value={option.value}>{option.label}</option>
              ))}
            </select>
            <span className="form-hint">自动（三协议）会按入站路径选择对应的原生协议；固定协议用于强制指定上游格式。</span>
          </div>

          <div className="form-group">
            <label className="form-label">粘性分组</label>
            <input type="text" className="form-input" defaultValue={stickyGroup} placeholder="留空 = 入口独立分组" onChange={(e) => { stickyGroup = e.target.value; }} />
            <span className="form-hint">同一分组共享会话粘性；当前分组故障后再故障转移到其他分组。</span>
          </div>

          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label">连接复用</label>
              <LocalToggle initial={keepAlive} onChange={(value) => { keepAlive = value; }} label={(value) => (value ? '启用连接复用' : '每请求新建连接')} ariaLabel="切换连接复用（Keep-Alive）" />
              <span className="form-hint">新入口默认启用；可按入口关闭。启用后复用出站连接，可减少 TCP/TLS 握手开销。</span>
            </div>
          </div>

          {!isNew && endpoint && (
            <div className="form-group endpoint-pricing-entry">
              <label className="form-label">成本价格</label>
              <div className="endpoint-pricing-entry-actions">
                <button type="button" className="btn btn-secondary btn-compact" onClick={() => openPricingEditor(endpoint)}>
                  <Icon name="currency" size={13} />
                  <span>编辑此入口价格</span>
                </button>
                <span className="form-hint">按入口维护输入、输出和缓存价格；未设置专属价格时回退到全局价格。</span>
              </div>
            </div>
          )}

        </div>
      ),
      actions: [
        { label: '取消', kind: 'ghost' },
        {
          label: '保存入口配置',
          kind: 'primary',
          onClick: async () => {
            if (!name.trim() || !baseURL.trim()) {
              addToast('请填写完整的入口名称与 API 地址', 'warning');
              return true;
            }
            const normalizedID = endpointID.trim() || `endpoint-${Date.now().toString(36)}`;
            if (!/^[A-Za-z0-9._-]+$/.test(normalizedID)) {
              addToast('入口 ID 只能包含字母、数字、点、下划线和连字符', 'warning');
              return true;
            }
            if (!Number.isInteger(Number(priority)) || Number(priority) < 0) {
              addToast('调度优先级必须是非负整数', 'warning');
              return true;
            }
            try {
              const parsed = new URL(baseURL.trim());
              if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname
                || parsed.username || parsed.password || parsed.search || parsed.hash) {
                throw new Error('invalid base url');
              }
            } catch {
              addToast('API 地址必须是无凭据、query 和 fragment 的有效 http/https 地址', 'warning');
              return true;
            }

            const nextConfig = clone(config);
            const secretUpdates = {};

            if (isNew) {
              const newId = normalizedID;
              if (nextConfig.endpoints.some((item) => item.id === newId)) {
                addToast(`入口 ID 已存在: ${newId}`, 'warning');
                return true;
              }
              nextConfig.endpoints.push({
                id: newId,
                name: name.trim(),
                baseURL: baseURL.trim(),
                protocol: normalizeEndpointProtocol(protocol, 'auto'),
                priority: Number(priority),
                stickyGroup: stickyGroup.trim() || null,
                keepAlive,
                enabled,
                modelMappings,
              });
              if (apiKey.trim()) secretUpdates[newId] = apiKey.trim();
            } else {
              const target = nextConfig.endpoints.find((e) => e.id === endpoint.id);
              if (target) {
                target.name = name.trim();
                target.baseURL = baseURL.trim();
                target.protocol = normalizeEndpointProtocol(protocol, 'auto');
                target.priority = Number(priority);
                target.enabled = enabled;
                target.keepAlive = keepAlive;
                target.stickyGroup = stickyGroup.trim() || null;
                target.modelMappings = modelMappings;
                if (apiKeyTouched) secretUpdates[endpoint.id] = apiKey.trim();
              }
            }

            await saveConfig(nextConfig, secretUpdates);
            return false;
          },
        },
      ],
    });
  };

  // Open Model Mapping Editor Modal
  const openMappingEditor = (mapping = null, mappingIdx = -1) => {
    let fromModel = mappingClient(mapping);
    let toModel = mappingUpstream(mapping);
    let thinking = mapping?.thinking || 'disable';
    let effort = mapping?.effort || 'auto';
    let context = mapping?.context || 'passThrough';
    let failoverTimeout = mapping?.failoverTimeoutSeconds ?? '';

    openModal({
      title: mapping ? '编辑模型映射' : `为「${selectedEndpoint?.name}」添加模型映射`,
      content: (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
          <div className="form-group">
            <label className="form-label">客户端请求模型 (From) *</label>
            <input
              type="text"
              className="form-input"
              defaultValue={fromModel}
              placeholder="如：claude-3-5-sonnet-20241022"
              onChange={(e) => { fromModel = e.target.value; }}
            />
          </div>

          <div className="form-group">
            <label className="form-label">转发生效模型 (To)</label>
            <input
              type="text"
              className="form-input"
              defaultValue={toModel}
              placeholder="如：gpt-4o 或 claude-3-7-sonnet"
              onChange={(e) => { toModel = e.target.value; }}
            />
            <span className="form-hint">留空表示与客户端模型同名；需要改名时填写上游实际模型名。</span>
          </div>

          <div className="grid-2col">
            <MappingThinkingControls
              thinking={thinking}
              effort={effort}
              onThinking={(value) => { thinking = value; }}
              onEffort={(value) => { effort = value; }}
            />
            <div className="form-group">
              <label className="form-label">上下文 (Context)</label>
              <select className="form-select" defaultValue={context} onChange={(e) => { context = e.target.value; }}>
                <option value="oneMillion">1M 上下文</option>
                <option value="passThrough">标准/透传</option>
                <option value="strip">剥离 1M 标记</option>
              </select>
            </div>
          </div>

          <div className="form-group">
            <label className="form-label">映射级首响应超时（秒）</label>
            <input
              type="number"
              min="0.1"
              step="0.1"
              className="form-input"
              defaultValue={failoverTimeout}
              placeholder="留空只使用全局单次响应超时"
              onChange={(e) => { failoverTimeout = e.target.value; }}
            />
            <span className="form-hint">填写后与全局单次响应超时取较小值；到期且尚未收到响应时尝试下一个 Provider 入口。</span>
          </div>
        </div>
      ),
      actions: [
        { label: '取消', kind: 'ghost' },
        {
          label: '保存映射',
          kind: 'primary',
          onClick: async () => {
            if (!fromModel.trim()) {
              addToast('客户端来源模型不能为空', 'warning');
              return true;
            }
            const failoverTimeoutNumber = failoverTimeout === '' ? null : Number(failoverTimeout);
            if (failoverTimeoutNumber != null && (!Number.isFinite(failoverTimeoutNumber) || failoverTimeoutNumber <= 0)) {
              addToast('映射级首响应超时必须留空或填写大于 0 的数字', 'warning');
              return true;
            }
            const nextConfig = clone(config);
            const targetEp = nextConfig.endpoints.find((e) => e.id === selectedEndpoint.id);
            targetEp.modelMappings = clone(endpointMappings(targetEp));
            const duplicate = targetEp.modelMappings.some((candidate, index) => (
              index !== mappingIdx && modelKey(mappingClient(candidate)) === modelKey(fromModel)
            ));
            if (duplicate) {
              addToast(`该入口已存在客户端模型映射: ${fromModel.trim()}`, 'warning');
              return true;
            }

            const nextMapping = {
              from: fromModel.trim(),
              to: toModel.trim(),
              thinking,
              ...(thinking === 'adaptive' && effort !== 'auto' ? { effort } : {}),
              context,
              ...(failoverTimeoutNumber == null ? {} : { failoverTimeoutSeconds: failoverTimeoutNumber }),
            };
            if (mappingIdx >= 0) {
              // Preserve any future fields from the existing mapping while replacing editable values.
              targetEp.modelMappings[mappingIdx] = { ...targetEp.modelMappings[mappingIdx], ...nextMapping };
              if (failoverTimeoutNumber == null) delete targetEp.modelMappings[mappingIdx].failoverTimeoutSeconds;
            } else {
              targetEp.modelMappings.push(nextMapping);
            }

            await saveConfig(nextConfig);
            return false;
          },
        },
      ],
    });
  };

  // Delete Mapping
  const handleDeleteMapping = async (mappingIdx) => {
    try {
      const nextConfig = clone(config);
      const targetEp = nextConfig.endpoints.find((e) => e.id === selectedEndpoint.id);
      targetEp.modelMappings.splice(mappingIdx, 1);
      await saveConfig(nextConfig);
      addToast('模型映射已删除', 'success');
    } catch (err) {
      addToast(`删除失败: ${err.message}`, 'error');
    }
  };

  const endpointColumns = [
    {
      title: '状态',
      width: '80px',
      render: (row) => (
        <QuickToggle
          checked={row.enabled}
          onChange={(val) => handleToggleEndpoint(row.id, val)}
          title={row.enabled ? '点击停用' : '点击启用'}
          ariaLabel={`${row.name || row.id}：${row.enabled ? '停用入口' : '启用入口'}`}
        />
      ),
    },
    {
      title: '入口名称',
      width: '178px',
      minWidth: '160px',
      render: (row) => (
        <span className="provider-reorder-cell">
          <span className="provider-reorder-handle" aria-hidden="true"><Icon name="drag" size={15} /></span>
          <span className="provider-reorder-copy">
            <strong>{row.name}</strong>
            <small className="mono-cell">{row.id}</small>
          </span>
        </span>
      ),
    },
    {
      title: '优先级',
      width: '62px',
      render: (row) => (
        <span className="mono-cell" style={{ fontWeight: 700, color: 'var(--primary)' }}>
          {row.priority ?? 0}
        </span>
      ),
    },
    {
      title: '协议',
      width: '108px',
      render: (row) => (
        <span style={{ fontSize: '0.8rem', color: 'var(--text-secondary)' }}>
          {endpointProtocolLabel(row.protocol)}
        </span>
      ),
    },
    {
      title: '映射',
      width: '72px',
      render: (row) => (
        <span style={{ fontSize: '0.82rem', color: 'var(--primary)' }}>
          {endpointMappings(row).length} 条
        </span>
      ),
    },
    {
      title: '模型目录',
      width: '126px',
      render: (row) => {
        const models = catalogModels(row);
        const status = row.catalog?.status;
        return (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '3px' }}>
            <span style={{ fontSize: '0.8rem', color: status === '获取失败' ? 'var(--status-critical)' : 'var(--text-secondary)' }}>
              {status || '未获取'}{models.length ? ` · ${models.length} 个` : ''}
            </span>
            {row.catalog?.updatedAt && <span className="form-hint" style={{ fontSize: '0.68rem' }}>{catalogTimestamp(row.catalog.updatedAt)}</span>}
          </div>
        );
      },
    },
    {
      title: '操作',
      type: 'action',
      align: 'right',
      width: '292px',
      minWidth: '292px',
      render: (row, idx) => (
        <div className="provider-row-actions">
          <button
            type="button"
            className="btn-icon btn-compact-icon"
            disabled={idx === 0}
            onClick={(e) => { e.stopPropagation(); handleMoveEndpoint(idx, -1); }}
            title="上移同优先级入口顺序"
            aria-label={`上移入口「${row.name || row.id}」`}
          >
            <Icon name="chevron" size={14} style={{ transform: 'rotate(-90deg)' }} />
          </button>
          <button
            type="button"
            className="btn-icon btn-compact-icon"
            disabled={idx === endpoints.length - 1}
            onClick={(e) => { e.stopPropagation(); handleMoveEndpoint(idx, 1); }}
            title="下移同优先级入口顺序"
            aria-label={`下移入口「${row.name || row.id}」`}
          >
            <Icon name="chevron" size={14} style={{ transform: 'rotate(90deg)' }} />
          </button>
          <button
            type="button"
            className="btn btn-secondary btn-compact"
            onClick={(e) => { e.stopPropagation(); openEndpointEditor(row); }}
            aria-label={`编辑入口「${row.name || row.id}」`}
            title={`编辑入口「${row.name || row.id}」`}
          >
            <Icon name="edit" size={13} />
            <span>编辑</span>
          </button>
          <button
            type="button"
            className="btn btn-secondary btn-compact"
            onClick={(e) => {
              e.stopPropagation();
              if (fetchingModelEndpointIDs.has(row.id)) cancelModelFetch(row.id);
              else void handleFetchModels(row);
            }}
            aria-label={`${fetchingModelEndpointIDs.has(row.id) ? '取消获取' : '获取'}入口「${row.name || row.id}」的模型`}
            title={`${fetchingModelEndpointIDs.has(row.id) ? '取消获取' : '获取'}入口「${row.name || row.id}」的模型`}
          >
            <Icon name="refresh" size={13} />
            <span>{fetchingModelEndpointIDs.has(row.id) ? '取消获取（读取中…）' : '获取模型'}</span>
          </button>
          <button
            type="button"
            className="btn btn-danger btn-compact"
            onClick={(e) => { e.stopPropagation(); handleDeleteEndpoint(row); }}
            aria-label={`删除入口「${row.name || row.id}」`}
            title={`删除入口「${row.name || row.id}」`}
          >
            <Icon name="trash" size={13} />
          </button>
        </div>
      ),
    },
  ];

  return (
    <div className="page-stack">
      {/* Header */}
      <div className="page-header">
        <div className="page-title-group">
          <h1 className="page-title">
            <Icon name="server" size={24} style={{ color: 'var(--primary)' }} />
            <span>入口库</span>
          </h1>
          <p className="page-subtitle">统一维护上游地址、密钥和原始映射，供多个模型组复用。</p>
        </div>
        <div className="page-actions">
          <button
            type="button"
            className="btn btn-secondary"
            onClick={openRetryEditor}
          >
            <Icon name="wrench" size={15} />
            <span>编辑重试策略</span>
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => openEndpointEditor(null)}
          >
            <Icon name="plus" size={16} />
            <span>添加入口通道</span>
          </button>
        </div>
      </div>

      {/* Top Overview Cards */}
      <div className="grid-2col">
        {/* Explicit Mapping Card */}
        <div className="glass-panel panel-padded-stack">
          <div className="panel-toolbar">
            <span style={{ fontSize: '0.82rem', fontWeight: 700, textTransform: 'uppercase', color: 'var(--text-muted)' }}>
              入口模型映射与入口概览
            </span>
            <StatusBadge text="按入口生效" kind="good" />
          </div>
          <p className="panel-description">
            每个入口都必须在“模型映射”中声明客户端模型、上游模型、Thinking 和上下文策略。
            精确模型名优先于 <code>prefix-*</code> 通配映射；点击入口后即可编辑。
          </p>
          <span className="form-hint">模型组决定承接范围与组内入口优先级；这里的优先级用于旧配置和新增默认组。原始映射的模型参数继续继承。</span>
          <div className="grid-3col provider-overview-metrics" aria-label="Provider 入口概览">
            <div><span>入口总数</span><strong className="mono-cell">{endpoints.length}</strong></div>
            <div><span>已启用</span><strong className="mono-cell">{enabledEndpointCount}</strong></div>
            <div><span>未启用</span><strong className="mono-cell">{Math.max(0, endpoints.length - enabledEndpointCount)}</strong></div>
            <div><span>模型映射</span><strong className="mono-cell">{mappedModelCount} 条</strong></div>
            <div><span>目录模型（去重）</span><strong className="mono-cell">{catalogModelCount ? `${catalogModelCount} 个` : '尚未获取'}</strong></div>
          </div>
        </div>

        {/* Global Retry Policy Card */}
        <div className="glass-panel panel-padded-stack">
          <div className="panel-toolbar">
            <span style={{ fontSize: '0.82rem', fontWeight: 700, textTransform: 'uppercase', color: 'var(--text-muted)' }}>
              全局超时与重试参数摘要
            </span>
            <StatusBadge text="生效中" kind="good" />
          </div>
          <div className="grid-2col summary-grid">
            <div>首响应截止：<strong className="mono-cell">{retry.responseTimeoutSeconds ? `${retry.responseTimeoutSeconds}s` : '由客户端决定'}</strong></div>
            <div>流式空闲截止：<strong className="mono-cell">{retry.streamIdleTimeoutSeconds ? `${retry.streamIdleTimeoutSeconds}s` : '无限空闲'}</strong></div>
            <div>500 失败后切换入口：<strong className="mono-cell">{(retry.failoverOn500 ?? true) ? '开启' : '关闭'}</strong></div>
            <div>入口内 500 重试：<strong className="mono-cell">{retry.max500Retries ?? 0} 次</strong></div>
            <div>retry_delay 透传：<strong className="mono-cell">{(retry.passThroughRetryDelay ?? true) ? (retry.retryDelaySeconds ? `${retry.retryDelaySeconds}s` : '未配置') : '关闭'}</strong></div>
            <div>粘性入口重试：<strong className="mono-cell">{retry.sessionStickyRetries ?? 2} 次</strong></div>
            <div>故障重试最大轮数：<strong className="mono-cell">{retry.maxDeferredRounds ?? retry.crossRoundRetries ?? 3} 轮</strong></div>
            <div>跨轮最长时长：<strong className="mono-cell">{retry.maxRetryDurationSeconds ? `${retry.maxRetryDurationSeconds}s` : '不限时长'}</strong></div>
          </div>
        </div>
      </div>

      {/* Endpoints Table */}
      <div className="glass-panel">
        <div className="panel-header">
          <div className="panel-title-group">
            <div className="panel-title">上游通道入口列表 ({endpoints.length})</div>
            <span className="panel-hint">调度优先按 Priority 数值（小者优先），同级按列表顺序。点击行查看详细映射；拖动整行可调整入口顺序，目标行上/下半区会显示插入位置。</span>
          </div>
        </div>
        <div className="panel-body panel-body-flush">
          <div className="provider-desktop-table">
            <DataTable
              className="provider-table"
              columns={endpointColumns}
              data={endpoints}
              keyField="id"
              getRowProps={providerRowProps}
              tableMinWidth="1114px"
              onRowClick={(row) => setSelectedEndpointID(row.id)}
              activeRowKey={selectedEndpoint?.id}
              emptyText="当前暂无 Provider 入口"
              ariaLabel="上游 Provider 入口"
            />
          </div>

          <div className="provider-mobile-list" role="list" aria-label="上游 Provider 入口">
            {endpoints.length === 0 ? (
              <div className="responsive-card-empty">当前暂无 Provider 入口</div>
            ) : endpoints.map((endpoint, idx) => {
              const models = catalogModels(endpoint);
              const selected = selectedEndpoint?.id === endpoint.id;
              const dragProps = providerRowProps(endpoint);
              return (
                <article
                  {...dragProps}
                  key={endpoint.id}
                  className={`${dragProps.className} responsive-data-card provider-mobile-card${selected ? ' is-selected' : ''}`}
                  role="listitem"
                >
                  <div className="responsive-data-card-heading">
                    <span className="provider-mobile-drag-handle" aria-hidden="true">
                      <Icon name="drag" size={15} />
                    </span>
                    <div className="responsive-data-card-title">
                      <strong>{endpoint.name || endpoint.id}</strong>
                      <small className="mono-cell">{endpoint.id}</small>
                    </div>
                    <QuickToggle
                      checked={endpoint.enabled}
                      onChange={(value) => handleToggleEndpoint(endpoint.id, value)}
                      title={endpoint.enabled ? '点击停用' : '点击启用'}
                      ariaLabel={`${endpoint.name || endpoint.id}：${endpoint.enabled ? '停用入口' : '启用入口'}`}
                    />
                  </div>

                  <dl className="responsive-data-card-fields provider-mobile-summary">
                    <div>
                      <dt>优先级</dt>
                      <dd className="mono-cell responsive-data-card-primary-value">{endpoint.priority ?? 0}</dd>
                    </div>
                    <div>
                      <dt>协议</dt>
                      <dd>{endpointProtocolLabel(endpoint.protocol)}</dd>
                    </div>
                    <div>
                      <dt>模型映射</dt>
                      <dd>{endpointMappings(endpoint).length} 条</dd>
                    </div>
                    <div>
                      <dt>模型目录</dt>
                      <dd className={endpoint.catalog?.status === '获取失败' ? 'provider-secret-missing' : undefined}>
                        {endpoint.catalog?.status || '未获取'}{models.length ? ` · ${models.length} 个` : ''}
                      </dd>
                    </div>
                  </dl>

                  <div className="responsive-data-card-actions provider-mobile-primary-actions">
                    <button
                      type="button"
                      className={`btn ${selected ? 'btn-primary' : 'btn-secondary'}`}
                      onClick={() => {
                        setSelectedEndpointID(endpoint.id);
                        window.requestAnimationFrame(() => document.getElementById('provider-endpoint-inspector')?.scrollIntoView({ block: 'start' }));
                      }}
                    >
                      <Icon name="server" size={14} />
                      <span>{selected ? '正在查看' : '查看详情'}</span>
                    </button>
                    <button
                      type="button"
                      className="btn btn-secondary"
                      onClick={() => openEndpointEditor(endpoint)}
                    >
                      <Icon name="edit" size={14} />
                      <span>编辑</span>
                    </button>
                  </div>

                  <details className="responsive-data-card-more provider-mobile-more">
                    <summary>排序、模型与删除</summary>
                    <div className="responsive-data-card-actions">
                      <button
                        type="button"
                        className="btn btn-secondary"
                        disabled={idx === 0}
                        onClick={() => handleMoveEndpoint(idx, -1)}
                      >
                        <Icon name="chevron" size={14} style={{ transform: 'rotate(-90deg)' }} />
                        <span>上移</span>
                      </button>
                      <button
                        type="button"
                        className="btn btn-secondary"
                        disabled={idx === endpoints.length - 1}
                        onClick={() => handleMoveEndpoint(idx, 1)}
                      >
                        <Icon name="chevron" size={14} style={{ transform: 'rotate(90deg)' }} />
                        <span>下移</span>
                      </button>
                      <button
                        type="button"
                        className="btn btn-secondary"
                        onClick={() => {
                          if (fetchingModelEndpointIDs.has(endpoint.id)) cancelModelFetch(endpoint.id);
                          else void handleFetchModels(endpoint);
                        }}
                      >
                        <Icon name="refresh" size={14} />
                        <span>{fetchingModelEndpointIDs.has(endpoint.id) ? '取消获取（读取中…）' : '获取模型'}</span>
                      </button>
                      <button
                        type="button"
                        className="btn btn-danger"
                        onClick={() => handleDeleteEndpoint(endpoint)}
                        aria-label={`删除入口「${endpoint.name || endpoint.id}」`}
                      >
                        <Icon name="trash" size={14} />
                        <span>删除</span>
                      </button>
                    </div>
                  </details>
                </article>
              );
            })}
          </div>
        </div>
      </div>

      {/* Selected Endpoint Inspector & Full Model Mappings */}
      {selectedEndpoint && (
        <div id="provider-endpoint-inspector" className="glass-panel panel-padded-stack provider-endpoint-inspector">
          <div className="panel-toolbar">
            <div className="panel-actions">
              <h3 style={{ fontSize: '1.05rem', fontWeight: 700, color: 'var(--text-primary)', margin: 0 }}>
                入口详情 · {selectedEndpoint.name}
              </h3>
              <StatusBadge text={selectedEndpoint.enabled ? '已就绪' : '已停用'} kind={selectedEndpoint.enabled ? 'good' : 'muted'} />
            </div>

            <div className="panel-actions">
              <button
                type="button"
                className="btn btn-secondary btn-compact"
                onClick={() => {
                  if (fetchingModelEndpointIDs.has(selectedEndpoint.id)) cancelModelFetch(selectedEndpoint.id);
                  else void handleFetchModels(selectedEndpoint);
                }}
              >
                <Icon name="refresh" size={13} />
                <span>{fetchingModelEndpointIDs.has(selectedEndpoint.id) ? '取消获取（读取中…）' : '获取模型'}</span>
              </button>
              <button
                type="button"
                className="btn btn-secondary btn-compact"
                onClick={() => openMappingEditor(null)}
              >
                <Icon name="plus" size={13} />
                <span>添加模型映射</span>
              </button>
              <button
                type="button"
                className="btn btn-secondary btn-compact"
                onClick={() => openEndpointEditor(selectedEndpoint)}
              >
                <Icon name="edit" size={13} />
                <span>编辑通道设置</span>
              </button>
            </div>
          </div>

          <div className="grid-4col endpoint-detail-grid">
            <div><span style={{ color: 'var(--text-muted)' }}>入口 ID：</span><span className="mono-cell">{selectedEndpoint.id}</span></div>
            <div><span style={{ color: 'var(--text-muted)' }}>API 地址：</span><span className="mono-cell">{selectedEndpoint.baseURL}</span></div>
            <div><span style={{ color: 'var(--text-muted)' }}>API Key：</span><span className={secretStatus?.endpoints?.[selectedEndpoint.id]?.configured ? 'provider-secret-configured' : 'provider-secret-missing'}>{secretStatus?.endpoints?.[selectedEndpoint.id]?.configured ? `已配置 · 尾号 ${secretStatus.endpoints[selectedEndpoint.id].last4 || '****'}` : '未配置'}</span></div>
            <div><span style={{ color: 'var(--text-muted)' }}>入口协议：</span><span className="mono-cell">{endpointProtocolLabel(selectedEndpoint.protocol)}</span></div>
            <div><span style={{ color: 'var(--text-muted)' }}>粘性分组：</span><span className="mono-cell">{selectedEndpoint.stickyGroup || '独立分组'}</span></div>
            <div><span style={{ color: 'var(--text-muted)' }}>连接复用：</span><span className="mono-cell">{selectedEndpoint.keepAlive ? '开启' : '关闭'}</span></div>
          </div>

          <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'flex-start', justifyContent: 'space-between', gap: '12px', padding: '12px 14px', borderRadius: 'var(--radius-md)', background: 'var(--bg-surface-glass)', border: '1px solid var(--border-subtle)' }}>
            <div style={{ minWidth: 0, flex: '1 1 420px' }}>
              <div style={{ fontSize: '0.85rem', fontWeight: 700, color: 'var(--text-primary)' }}>
                模型目录 · {catalogModels(selectedEndpoint).length} 个
                {selectedEndpoint.catalog?.status ? <span style={{ marginLeft: '8px', color: selectedEndpoint.catalog.status === '获取失败' ? 'var(--status-critical)' : 'var(--status-good)' }}>{selectedEndpoint.catalog.status}</span> : null}
              </div>
              <div className="form-hint" style={{ marginTop: '4px' }}>
                {selectedEndpoint.catalog?.updatedAt ? `更新于 ${catalogTimestamp(selectedEndpoint.catalog.updatedAt)}` : '尚未获取该入口的模型目录'}
                {selectedEndpoint.catalog?.source ? <span> · 来源 <span className="mono-cell" style={{ overflowWrap: 'anywhere' }}>{selectedEndpoint.catalog.source}</span></span> : null}
              </div>
              {selectedEndpoint.catalog?.error && <div role="alert" style={{ marginTop: '6px', color: 'var(--status-critical)', fontSize: '0.78rem', overflowWrap: 'anywhere' }}>{selectedEndpoint.catalog.error}</div>}
              {catalogModels(selectedEndpoint).length > 0 && (
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: '6px', maxHeight: '132px', overflowY: 'auto', marginTop: '10px', paddingRight: '4px' }}>
                  {catalogModels(selectedEndpoint).map((model) => (
                    <span key={model} className="mono-cell" style={{ padding: '3px 7px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-subtle)', color: 'var(--text-secondary)', fontSize: '0.72rem', overflowWrap: 'anywhere' }}>
                      {readableCatalogModel(model)}
                    </span>
                  ))}
                </div>
              )}
            </div>
            <div style={{ display: 'flex', flexWrap: 'wrap', gap: '8px', flexShrink: 0 }}>
              <button type="button" className="btn btn-secondary" style={{ padding: '4px 10px', fontSize: '0.78rem' }} disabled={!catalogModels(selectedEndpoint).length} onClick={() => openCatalogMappingEditor(selectedEndpoint)}>
                <Icon name="plus" size={13} />
                <span>从已知模型添加</span>
              </button>
            </div>
          </div>

          {/* Model Mappings Table */}
          <div>
            <div style={{ fontSize: '0.85rem', fontWeight: 700, color: 'var(--text-primary)', marginBottom: '10px' }}>
              模型映射 · {selectedEndpoint.name} ({endpointMappings(selectedEndpoint).length})
            </div>

            {endpointMappings(selectedEndpoint).length > 0 ? (
              <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
                {endpointMappings(selectedEndpoint).map((map, idx) => (
                  <div
                    key={idx}
                    className="mono-cell"
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      padding: '10px 14px',
                      borderRadius: 'var(--radius-md)',
                      background: 'var(--bg-surface-glass)',
                      border: '1px solid var(--border-subtle)',
                      fontSize: '0.85rem',
                    }}
                  >
                    <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: '10px 14px', minWidth: 0 }}>
                      <span style={{ color: 'var(--text-primary)', fontWeight: 600 }}>{mappingClient(map)}</span>
                      <span style={{ color: 'var(--primary)' }}>➔</span>
                      <span style={{ color: 'var(--status-good)', fontWeight: 600 }}>{mappingUpstream(map) || '(同名)'}</span>
                      <span className="form-hint" style={{ fontSize: '0.68rem' }}>{map.thinking || 'disable'} · {map.context || 'passThrough'}{map.failoverTimeoutSeconds != null ? ` · ${map.failoverTimeoutSeconds}s` : ''}</span>
                    </div>

                    <div style={{ display: 'flex', gap: '6px' }}>
                      <button
                        type="button"
                        className="btn-icon btn-compact-icon"
                        onClick={() => openMappingEditor(map, idx)}
                        title="编辑映射"
                        aria-label={`编辑模型映射「${mappingClient(map)}」`}
                      >
                        <Icon name="edit" size={13} />
                      </button>
                      <button
                        type="button"
                        className="btn-icon btn-compact-icon"
                        style={{ color: 'var(--status-critical)' }}
                        onClick={() => handleDeleteMapping(idx)}
                        title="删除映射"
                        aria-label={`删除模型映射「${mappingClient(map)}」`}
                      >
                        <Icon name="trash" size={13} />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            ) : (
              <div style={{ padding: '16px', borderRadius: 'var(--radius-md)', background: 'var(--bg-surface-glass)', color: 'var(--text-muted)', fontSize: '0.85rem' }}>
                当前通道未配置任何模型重命名映射；所有请求原样使用客户端模型名称进行上游转发。
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
