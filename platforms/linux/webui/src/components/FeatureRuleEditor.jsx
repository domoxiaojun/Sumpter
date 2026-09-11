import React, { useEffect, useMemo, useRef, useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { createLocalID } from '../utils/helpers.js';
import { isBuiltinRule, routeCatalogKey, routeModelChoices, saveFeatureRule } from '../utils/featureRoutes.js';

const effortOptions = [
  ['', '跟随原请求'], ['none', '关闭推理（none）'], ['auto', '自动（auto）'],
  ['minimal', '最低（minimal）'], ['low', '低（low）'], ['medium', '中（medium）'],
  ['high', '高（high）'], ['xhigh', '很高（xhigh）'], ['max', '最高（max）'],
];

export function FeatureRuleEditor({ rule, catalogLoader }) {
  const { config, saveConfig, closeModal } = useApp();
  const [draft, setDraft] = useState(() => ({
    id: rule?.id || createLocalID('rule'), name: rule?.name || '',
    requestKind: rule?.match?.requestKind || 'websearch',
    toolTypePrefix: rule?.match?.toolTypePrefix || '', modelEquals: rule?.match?.modelEquals || '',
    systemContains: rule?.match?.systemContains || '', messagesContain: rule?.match?.messagesContain || '',
    endpointID: rule?.target?.endpointID || '', model: rule?.target?.model || '',
    protocol: rule?.target?.protocol || '', effort: rule?.target?.effort || '',
  }));
  const [modelState, setModelState] = useState(null);
  const [refresh, setRefresh] = useState(0);
  const [query, setQuery] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState('');
  const savingRef = useRef(false);
  const mountedRef = useRef(false);
  const endpoints = config?.endpoints || [];
  const endpoint = endpoints.find((item) => item.id === draft.endpointID);
  const endpointRef = useRef(endpoint);
  endpointRef.current = endpoint;
  const endpointKey = endpoint ? routeCatalogKey(endpoint) : '';
  const builtin = isBuiltinRule(rule);
  const edit = (field, value) => setDraft((previous) => ({ ...previous, [field]: value }));

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  useEffect(() => {
    const selected = endpointRef.current;
    if (!selected) { setModelState(null); return undefined; }
    const controller = new AbortController();
    const cached = catalogLoader.peek(selected);
    let timedOut = false;
    const timeout = window.setTimeout(() => { timedOut = true; controller.abort(); }, 15_000);
    setModelState({ key: endpointKey, loading: true, catalog: cached, error: '' });
    catalogLoader.load(selected, { signal: controller.signal }).then((catalog) => {
      if (!controller.signal.aborted) setModelState({ key: endpointKey, catalog, loading: false, error: '' });
    }).catch((error) => {
      if (controller.signal.aborted && !timedOut) return;
      setModelState({ key: endpointKey, catalog: cached, loading: false,
        error: timedOut ? '获取模型超时，请重试；也可以手动输入模型。' : `获取模型失败：${String(error.message || error).slice(0, 300)}` });
    }).finally(() => window.clearTimeout(timeout));
    return () => { window.clearTimeout(timeout); controller.abort(); };
  }, [endpointKey, refresh, catalogLoader]);

  const activeModels = modelState?.key === endpointKey ? modelState : null;
  const models = useMemo(() => routeModelChoices(endpoints, draft.endpointID, activeModels?.catalog), [endpoints, draft.endpointID, activeModels?.catalog]);
  const matchingModels = useMemo(() => models.filter((model) => model.toLowerCase().includes(query.trim().toLowerCase())), [models, query]);
  const visibleModels = matchingModels.slice(0, 80);

  const fetchAgain = () => {
    // Invalidate only this endpoint; changing selection keeps other caches useful.
    catalogLoader.invalidate(endpoint);
    setRefresh((value) => value + 1);
  };

  const submit = async (event) => {
    event.preventDefault();
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    setSaveError('');
    try {
      await saveConfig((latest) => saveFeatureRule(latest, rule, draft,
        activeModels?.catalog ? { key: endpointKey, catalog: activeModels.catalog } : null));
      if (mountedRef.current) closeModal();
    } catch (error) {
      if (mountedRef.current) setSaveError(error.message || '保存失败，请重试');
    } finally {
      savingRef.current = false;
      if (mountedRef.current) setSaving(false);
    }
  };

  const textField = (field, label, placeholder = '') => (
    <label className="form-group"><span className="form-label">{label}</span>
      <input className="form-input" value={draft[field]} placeholder={placeholder} onChange={(event) => edit(field, event.target.value)} />
    </label>
  );

  return <form className="panel-stack route-editor" onSubmit={submit}>
    <fieldset disabled={saving} className="route-editor-fields panel-stack">
      {builtin ? <details className="route-match-details">
        <summary>内建识别条件 · {rule.name || rule.id}</summary>
        <p className="form-hint">内建规则的名称和匹配条件由代理协议固定，只能调整启停与分流目标。</p>
        <dl className="responsive-data-card-fields">
          {Object.entries(rule.match || {}).filter(([, value]) => value).map(([key, value]) => <div key={key}><dt>{key}</dt><dd>{String(value)}</dd></div>)}
        </dl>
      </details> : <>
        {textField('name', '规则名称 *', '如：WebSearch 搜索分流')}
        <details className="route-match-details" open={!rule}>
          <summary>触发匹配条件</summary>
          <div className="panel-stack">
            <div className="grid-2col">
              <label className="form-group"><span className="form-label">请求用途 (Request Kind)</span>
                <select className="form-select" value={draft.requestKind} onChange={(event) => edit('requestKind', event.target.value)}>
                  <option value="websearch">WebSearch (搜索)</option><option value="webfetch">WebFetch (网页抓取)</option><option value="classifier">安全分类器 (Classifier)</option>
                </select>
              </label>
              {textField('toolTypePrefix', '工具调用前缀 (可选)', '如：web_search 或 web_fetch')}
            </div>
            {textField('modelEquals', '客户端模型精确匹配 (可选)')}
            {textField('systemContains', 'System Prompt 包含文本 (可选)')}
            {textField('messagesContain', 'Messages 包含文本 (可选)')}
          </div>
        </details>
      </>}

      <label className="form-group"><span className="form-label">固定入口通道（可选）</span>
        <select className="form-select" value={draft.endpointID} onChange={(event) => { edit('endpointID', event.target.value); setQuery(''); }}>
          <option value="">按模型组与入口优先级自动选择</option>
          {draft.endpointID && !endpoint && <option value={draft.endpointID}>入口已不存在</option>}
          {endpoints.map((item) => <option key={item.id} value={item.id}>{item.name || item.id}{item.enabled === false ? '（已停用）' : ''}</option>)}
        </select>
      </label>
      {endpoint?.enabled === false && <p className="form-hint">此入口已停用，启用后才能承接请求。</p>}

      <div className="form-group">
        <label className="form-group"><span className="form-label">目标承接模型 *</span>
          <input className="form-input" value={draft.model} placeholder="从下方选择，或手动输入模型名称" onChange={(event) => edit('model', event.target.value)} />
        </label>
        <div className="route-model-toolbar">
          <span className="form-hint" role="status">{activeModels?.loading ? '正在后台获取入口模型，可继续编辑…' : `${models.length} 个模型候选`}</span>
          {endpoint && <button type="button" className="btn btn-ghost btn-compact" onClick={fetchAgain} disabled={activeModels?.loading}>{activeModels?.error ? '重试获取模型' : '刷新模型'}</button>}
        </div>
        {activeModels?.error && <p className="form-hint route-editor-error" role="alert">{activeModels.error} 已有候选与手动输入仍可使用。</p>}
        {(models.length > 80 || query) && <input className="form-input" aria-label="搜索入口模型" placeholder="输入关键词筛选模型" value={query} onChange={(event) => setQuery(event.target.value)} />}
        <select className="form-select" aria-label="选择目标模型" value="" onChange={(event) => edit('model', event.target.value)} disabled={!visibleModels.length}>
          <option value="">{visibleModels.length ? `从${endpoint ? '此入口' : '已配置'}模型中选择` : '暂无匹配模型，可手动输入'}</option>
          {visibleModels.map((model) => <option key={model} value={model}>{model}</option>)}
        </select>
        {matchingModels.length > visibleModels.length && <span className="form-hint">显示前 80 项，共 {matchingModels.length} 项；输入关键词可缩小范围。</span>}
        <span className="form-hint">{endpoint ? '选择入口后自动获取模型；切换入口会保留当前目标模型，请按需重新选择。' : '自动选择使用已配置的模型映射；指定入口后可获取该入口的模型列表。'}</span>
      </div>
      <div className="grid-2col">
        <label className="form-group"><span className="form-label">目标协议（可选）</span>
          <select className="form-select" value={draft.protocol} onChange={(event) => edit('protocol', event.target.value)}>
            <option value="">自动：优先原生，必要时桥接</option><option value="anthropic">Anthropic Messages</option><option value="openai">OpenAI Chat Completions</option><option value="openai-responses">OpenAI Responses</option>
          </select>
        </label>
        <label className="form-group"><span className="form-label">Effort 覆盖</span>
          <select className="form-select" value={draft.effort} onChange={(event) => edit('effort', event.target.value)}>
            {effortOptions.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </label>
      </div>
      <p className="form-hint">仅在命中这条 Claude Code 路由时覆盖；跟随原请求不会改写客户端 effort。</p>
    </fieldset>
    {saveError && <p className="route-editor-error" role="alert">{saveError}</p>}
    <div className="route-editor-actions">
      <button type="button" className="btn btn-ghost" disabled={saving} onClick={closeModal}>取消</button>
      <button type="submit" className="btn btn-primary" disabled={saving} aria-busy={saving}>{saving ? '正在保存…' : '保存规则'}</button>
    </div>
  </form>;
}
