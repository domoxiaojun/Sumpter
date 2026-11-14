import { useContext, useEffect, useMemo, useRef, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { ModalBusyContext } from '../context/ModalContext.js';
import { useAppControls } from '../context/AppContext.jsx';
import { createLocalID } from '../utils/helpers.js';
import { isBuiltinRule, routeCatalogKey, routeModelChoices, saveFeatureRule } from '../utils/featureRoutes.js';
import { Button } from './ui/button.jsx';
import { Input } from './ui/input.jsx';
import { Label } from './ui/label.jsx';

const SELECT_CLS = 'h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs text-foreground shadow-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50';

const effortOptions = [
  ['', '跟随原请求'],
  ['none', '关闭推理（none）'],
  ['auto', '自动（auto）'],
  ['minimal', '最低（minimal）'],
  ['low', '低（low）'],
  ['medium', '中（medium）'],
  ['high', '高（high）'],
  ['xhigh', '很高（xhigh）'],
  ['max', '最高（max）'],
];

export function FeatureRuleEditor({ rule, catalogLoader }) {
  const { config, configDoc, saveConfig, closeModal } = useAppControls();
  const draftSource = useRef(configDoc);
  const [draft, setDraft] = useState(() => ({
    id: rule?.id || createLocalID('rule'),
    name: rule?.name || '',
    requestKind: rule?.match?.requestKind || 'websearch',
    toolTypePrefix: rule?.match?.toolTypePrefix || '',
    modelEquals: rule?.match?.modelEquals || '',
    systemContains: rule?.match?.systemContains || '',
    messagesContain: rule?.match?.messagesContain || '',
    endpointID: rule?.target?.endpointID || '',
    model: rule?.target?.model || '',
    protocol: rule?.target?.protocol || '',
    effort: rule?.target?.effort || '',
  }));
  const [modelState, setModelState] = useState(null);
  const [refresh, setRefresh] = useState(0);
  const [query, setQuery] = useState('');
  const [saving, setSaving] = useState(false);
  const setModalBusy = useContext(ModalBusyContext);
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
    if (!selected) {
      setModelState(null);
      return undefined;
    }
    const controller = new AbortController();
    const cached = catalogLoader.peek(selected);
    let timedOut = false;
    const timeout = window.setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, 15_000);
    setModelState({ key: endpointKey, loading: true, catalog: cached, error: '' });
    catalogLoader.load(selected, { signal: controller.signal }).then((catalog) => {
      if (!controller.signal.aborted) setModelState({ key: endpointKey, catalog, loading: false, error: '' });
    }).catch((error) => {
      if (controller.signal.aborted && !timedOut) return;
      setModelState({
        key: endpointKey,
        catalog: cached,
        loading: false,
        error: timedOut ? '获取模型超时，请重试。' : `获取模型失败：${String(error.message || error).slice(0, 300)}`,
      });
    }).finally(() => window.clearTimeout(timeout));
    return () => {
      window.clearTimeout(timeout);
      controller.abort();
    };
  }, [endpointKey, refresh, catalogLoader]);

  const activeModels = modelState?.key === endpointKey ? modelState : null;
  const models = useMemo(
    () => routeModelChoices(endpoints, draft.endpointID, activeModels?.catalog),
    [endpoints, draft.endpointID, activeModels?.catalog],
  );
  const selectedModel = models.includes(draft.model) ? draft.model : '';
  const loadingModels = Boolean(endpoint && (!activeModels || activeModels.loading));
  const matchingModels = useMemo(
    () => models.filter((model) => model.toLowerCase().includes(query.trim().toLowerCase())),
    [models, query],
  );
  const visibleModels = matchingModels.slice(0, 80);
  const selectionOutsideResults = selectedModel && !visibleModels.includes(selectedModel);

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
    setModalBusy(true);
    setSaveError('');
    try {
      await saveConfig(
        (latest) => saveFeatureRule(
          latest,
          rule,
          draft,
          activeModels?.catalog ? { key: endpointKey, catalog: activeModels.catalog } : null,
        ),
        {},
        draftSource.current,
      );
      if (mountedRef.current) closeModal();
    } catch (error) {
      if (mountedRef.current) setSaveError(error.message || '保存失败，请重试');
    } finally {
      setModalBusy(false);
      savingRef.current = false;
      if (mountedRef.current) setSaving(false);
    }
  };

  const textField = (field, label, placeholder = '') => (
    <label className="flex flex-col gap-1.5 text-xs">
      <span className="font-medium text-muted-foreground">{label}</span>
      <Input
        className="h-9 text-xs"
        value={draft[field]}
        placeholder={placeholder}
        onChange={(event) => edit(field, event.target.value)}
      />
    </label>
  );

  return (
    <form className="space-y-4" onSubmit={submit}>
      <fieldset disabled={saving} className="space-y-3.5">
        {builtin ? (
          <details className="group rounded-xl border bg-muted/20 p-3">
            <summary className="flex cursor-pointer list-none items-center gap-1.5 text-xs font-semibold text-foreground select-none">
              <ChevronRight className="size-3.5 text-muted-foreground transition-transform group-open:rotate-90" />
              <span>内建识别条件 · {rule.name || rule.id}</span>
            </summary>
            <p className="mt-2 text-[11px] text-muted-foreground">
              内建规则的名称和匹配条件由代理协议固定，只能调整启停与分流目标。
            </p>
            <dl className="mt-2 grid gap-2 sm:grid-cols-2">
              {Object.entries(rule.match || {})
                .filter(([, value]) => value)
                .map(([key, value]) => (
                  <div key={key} className="rounded-lg border bg-background px-2.5 py-1.5 text-xs">
                    <dt className="text-[10px] text-muted-foreground">{key}</dt>
                    <dd className="font-mono text-xs text-foreground">{String(value)}</dd>
                  </div>
                ))}
            </dl>
          </details>
        ) : (
          <>
            {textField('name', '规则名称 *', '如：WebSearch 搜索分流')}
            <details className="group rounded-xl border bg-muted/15 p-3" open={!rule}>
              <summary className="flex cursor-pointer list-none items-center gap-1.5 text-xs font-semibold text-foreground select-none">
                <ChevronRight className="size-3.5 text-muted-foreground transition-transform group-open:rotate-90" />
                <span>触发匹配条件</span>
              </summary>
              <div className="mt-3 space-y-3">
                <div className="grid gap-3 sm:grid-cols-2">
                  <label className="flex flex-col gap-1.5 text-xs">
                    <span className="font-medium text-muted-foreground">请求用途 (Request Kind)</span>
                    <select
                      className={SELECT_CLS}
                      value={draft.requestKind}
                      onChange={(event) => edit('requestKind', event.target.value)}
                    >
                      <option value="websearch">WebSearch (搜索)</option>
                      <option value="webfetch">WebFetch (网页抓取)</option>
                      <option value="classifier">安全分类器 (Classifier)</option>
                    </select>
                  </label>
                  {textField('toolTypePrefix', '工具调用前缀 (可选)', '如：web_search 或 web_fetch')}
                </div>
                {textField('modelEquals', '客户端模型精确匹配 (可选)')}
                {textField('systemContains', 'System Prompt 包含文本 (可选)')}
                {textField('messagesContain', 'Messages 包含文本 (可选)')}
              </div>
            </details>
          </>
        )}

        <label className="flex flex-col gap-1.5 text-xs">
          <span className="font-medium text-muted-foreground">固定入口通道（可选）</span>
          <select
            className={SELECT_CLS}
            value={draft.endpointID}
            onChange={(event) => {
              const endpointID = event.target.value;
              setDraft((previous) => ({ ...previous, endpointID, model: '' }));
              setQuery('');
              setSaveError('');
            }}
          >
            <option value="">按模型组与入口优先级自动选择</option>
            {draft.endpointID && !endpoint && <option value={draft.endpointID}>入口已不存在</option>}
            {endpoints.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name || item.id}{item.enabled === false ? '（已停用）' : ''}
              </option>
            ))}
          </select>
        </label>
        {endpoint?.enabled === false && (
          <p className="text-[11px] text-status-warning">此入口已停用，启用后才能承接请求。</p>
        )}

        <div className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Label htmlFor="route-target-model" className="text-xs font-medium text-muted-foreground">
              目标承接模型 *
            </Label>
            <div className="flex items-center gap-2">
              <span className="text-[11px] text-muted-foreground" role="status">
                {loadingModels ? '正在获取入口模型…' : `${models.length} 个模型候选`}
              </span>
              {endpoint && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={fetchAgain}
                  disabled={loadingModels}
                >
                  {activeModels?.error ? '重试获取模型' : '刷新模型'}
                </Button>
              )}
            </div>
          </div>

          {activeModels?.error && (
            <p className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">
              {activeModels.error} {models.length ? '仍可从已有模型中选择。' : '请重试或先在入口库配置模型。'}
            </p>
          )}

          {(models.length > 80 || query) && (
            <Input
              className="h-8 text-xs"
              aria-label="搜索入口模型"
              placeholder="输入关键词筛选模型"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          )}

          <select
            id="route-target-model"
            className={SELECT_CLS}
            value={selectedModel}
            required
            onChange={(event) => edit('model', event.target.value)}
            disabled={!models.length}
          >
            <option value="">
              {visibleModels.length
                ? `从${endpoint ? '此入口' : '已配置'}模型中选择`
                : loadingModels
                  ? '正在获取模型…'
                  : models.length
                    ? '没有匹配的模型'
                    : '暂无可选模型'}
            </option>
            {selectionOutsideResults && <option value={selectedModel}>{selectedModel}</option>}
            {visibleModels.map((model) => (
              <option key={model} value={model}>{model}</option>
            ))}
          </select>

          {!loadingModels && draft.model && !selectedModel && (
            <p className="text-xs text-destructive" role="alert">
              原目标模型 {draft.model} 不在当前列表，请重新选择。
            </p>
          )}
          {!loadingModels && !models.length && (
            <p className="text-[11px] text-muted-foreground">
              {endpoint ? '请刷新模型列表，或先在入口库为此入口配置模型。' : '请先在入口库配置模型，或指定入口获取模型列表。'}
            </p>
          )}
          {matchingModels.length > visibleModels.length && (
            <p className="text-[11px] text-muted-foreground">
              显示前 80 项，共 {matchingModels.length} 项；输入关键词可缩小范围。
            </p>
          )}
          <p className="text-[11px] text-muted-foreground">
            {endpoint
              ? '仅显示此入口的模型目录与已配置模型；切换入口后请重新选择。'
              : '显示已启用入口的已配置模型；指定入口后可获取该入口的模型列表。'}
          </p>
        </div>

        <div className="grid gap-3 sm:grid-cols-2">
          <label className="flex flex-col gap-1.5 text-xs">
            <span className="font-medium text-muted-foreground">目标协议（可选）</span>
            <select
              className={SELECT_CLS}
              value={draft.protocol}
              onChange={(event) => edit('protocol', event.target.value)}
            >
              <option value="">自动：优先原生，必要时桥接</option>
              <option value="anthropic">Anthropic Messages</option>
              <option value="openai">OpenAI Chat Completions</option>
              <option value="openai-responses">OpenAI Responses</option>
            </select>
          </label>
          <label className="flex flex-col gap-1.5 text-xs">
            <span className="font-medium text-muted-foreground">Effort 覆盖</span>
            <select
              className={SELECT_CLS}
              value={draft.effort}
              onChange={(event) => edit('effort', event.target.value)}
            >
              {effortOptions.map(([value, label]) => (
                <option key={value} value={value}>{label}</option>
              ))}
            </select>
          </label>
        </div>
        <p className="text-[11px] text-muted-foreground">
          仅在命中这条 Claude Code 路由时覆盖；跟随原请求不会改写客户端 effort。
        </p>
      </fieldset>

      {saveError && (
        <p className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">
          {saveError}
        </p>
      )}

      <div className="flex justify-end gap-2 pt-2">
        <Button type="button" variant="outline" disabled={saving} onClick={closeModal}>
          取消
        </Button>
        <Button
          type="submit"
          disabled={saving || !draft.name.trim() || !selectedModel}
          aria-busy={saving}
        >
          {saving ? '正在保存…' : '保存规则'}
        </Button>
      </div>
    </form>
  );
}
