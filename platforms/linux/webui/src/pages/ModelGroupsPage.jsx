import { useEffect, useRef, useState } from 'react';
import { ArrowDown, ArrowUp, Check, ChevronRight, Layers, Plus, Search, Server, Trash2 } from 'lucide-react';
import { cn } from '../lib/utils.js';
import { useAppControls } from '../context/AppContext.jsx';
import { ModelGroupBindingEditor } from '../components/ModelGroupBindingEditor.jsx';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { clone } from '../utils/helpers.js';
import { endpointGroupModels, groupModels, modelCatalogCategories, modelMatches, newModelGroup, pruneGroupReferences } from '../utils/modelGroups.js';
import { Badge } from '../components/ui/badge.jsx';
import { Button } from '../components/ui/button.jsx';
import { Card, CardContent, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { Input } from '../components/ui/input.jsx';
import { InputGroup, InputGroupAddon, InputGroupInput } from '../components/ui/input-group.jsx';
import { Tabs, TabsList, TabsTrigger } from '../components/ui/tabs.jsx';

const SELECT_CLS = 'h-9 rounded-md border border-input bg-background px-2.5 text-xs text-foreground shadow-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50';

function CategoryPicker({ categories, selected, onChange }) {
  const [query, setQuery] = useState('');
  const [activeID, setActiveID] = useState(null);
  const search = query.trim().toLowerCase();
  const active = categories.find((category) => category.id === activeID) || categories[0];
  const displayed = search
    ? categories
      .map((category) => ({
        ...category,
        models: category.models.filter((model) => model.toLowerCase().includes(search)),
      }))
      .filter((category) => category.models.length)
    : (active ? [active] : []);

  const update = (models) => onChange([...new Set(models)]);

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <InputGroup className="flex-1">
          <InputGroupAddon>
            <Search className="size-3.5 text-muted-foreground" />
          </InputGroupAddon>
          <InputGroupInput
            aria-label="搜索模型"
            placeholder="搜索模型"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </InputGroup>
        {query && (
          <Button type="button" variant="ghost" size="sm" aria-label="清除搜索" onClick={() => setQuery('')}>
            清除
          </Button>
        )}
      </div>

      {!search && (
        <nav className="grid gap-2 sm:grid-cols-3 md:grid-cols-4" aria-label="模型系列">
          {categories.map((category) => {
            const isActive = active?.id === category.id;
            const selectedCount = category.models.filter((model) => selected.includes(model)).length;
            return (
              <button
                type="button"
                key={category.id}
                aria-pressed={isActive}
                onClick={() => setActiveID(category.id)}
                className={cn(
                  'flex cursor-pointer items-center justify-between gap-2 rounded-lg border px-3 py-2 text-left text-xs transition-colors',
                  isActive
                    ? 'border-primary/60 bg-primary/10 text-foreground font-semibold'
                    : 'bg-card hover:bg-muted/50 text-muted-foreground',
                )}
              >
                <span className="truncate">{category.title}</span>
                <small className="font-mono text-[11px] tabular-nums">
                  {selectedCount}/{category.models.length}
                </small>
              </button>
            );
          })}
        </nav>
      )}

      {displayed.map((category) => (
        <section className="space-y-2 rounded-xl border bg-muted/15 p-3.5" key={category.id}>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-baseline gap-2 text-xs">
              <strong className="font-semibold text-foreground">{category.title} 系列</strong>
              <small className="text-muted-foreground">{category.models.length} 个模型</small>
            </div>
            <div className="flex items-center gap-1">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 px-2 text-xs"
                onClick={() => update([...selected, ...category.models])}
              >
                全选
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 px-2 text-xs"
                onClick={() => update(selected.filter((model) => !category.models.includes(model)))}
              >
                清空
              </Button>
            </div>
          </div>
          <div className="grid gap-1.5 sm:grid-cols-2 lg:grid-cols-3">
            {category.models.map((model) => {
              const checked = selected.includes(model);
              return (
                <label
                  key={model}
                  className={cn(
                    'flex cursor-pointer items-center gap-2 rounded-lg border px-2.5 py-2 text-xs transition-colors',
                    checked
                      ? 'border-primary/50 bg-primary/10 text-foreground'
                      : 'border-border/70 bg-background hover:bg-muted/40 text-muted-foreground',
                  )}
                >
                  <input
                    type="checkbox"
                    className="size-3.5 shrink-0 accent-primary"
                    checked={checked}
                    onChange={(e) => update(e.target.checked ? [...selected, model] : selected.filter((item) => item !== model))}
                  />
                  <span className="truncate font-mono text-[11px]" title={model}>{model}</span>
                </label>
              );
            })}
          </div>
        </section>
      ))}

      {!displayed.length && (
        <p className="rounded-xl border border-dashed px-4 py-8 text-center text-xs text-muted-foreground">
          {search ? '没有匹配的模型，试试其他关键词。' : '暂无可选模型，请先在入口库添加模型，并确认当前组的模型范围。'}
        </p>
      )}
    </div>
  );
}

export function ModelGroupsPage() {
  const { config, configDoc, saveConfig, addToast, openModal } = useAppControls();
  const draftSource = useRef(configDoc);
  const [groups, setGroups] = useState(() => groupModels(config));
  const [selected, setSelected] = useState(groups[0]?.id);
  const [editorTab, setEditorTab] = useState('models');
  const [saving, setSaving] = useState(false);
  const dirty = useRef(false);
  const [isDirty, setDirty] = useState(false);
  useUnsavedChanges(isDirty, saving);
  const drag = useRef(null);
  const endpoints = config?.endpoints || [];

  useEffect(() => {
    if (!dirty.current) {
      draftSource.current = configDoc;
      setGroups(groupModels(config));
    }
  }, [config, configDoc]);

  const change = (fn) => {
    dirty.current = true;
    setDirty(true);
    setGroups((old) => {
      const next = clone(old);
      fn(next);
      return next;
    });
  };
  const updateGroup = (id, fn) => change((list) => {
    const group = list.find((g) => g.id === id);
    if (group) fn(group);
  });
  const move = (items, from, to) => {
    if (to < 0 || to >= items.length || from === to) return;
    items.splice(to, 0, items.splice(from, 1)[0]);
  };
  const current = groups.find((g) => g.id === selected) || groups[0];

  const removeGroup = (group) => openModal({
    title: `删除模型组「${group.name}」？`,
    content: <p className="text-sm text-muted-foreground">入口库中的连接配置会保留。保存更改后生效。</p>,
    actions: [
      { label: '取消', kind: 'ghost' },
      {
        label: '删除模型组',
        kind: 'danger',
        onClick: () => {
          change((list) => {
            const index = list.findIndex((g) => g.id === group.id);
            if (index >= 0) list.splice(index, 1);
          });
        },
      },
    ],
  });

  const save = async () => {
    setSaving(true);
    try {
      const next = pruneGroupReferences({ ...clone(draftSource.current.config), modelGroups: clone(groups) });
      const saved = await saveConfig(next, {}, draftSource.current);
      draftSource.current = saved;
      setGroups(groupModels(saved.config));
      dirty.current = false;
      setDirty(false);
      addToast('模型组已保存，客户端地址保持不变', 'success');
    } catch (error) {
      addToast(`保存失败：${error.message}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  const categories = modelCatalogCategories(endpoints);
  const catalog = categories.flatMap((category) => category.models);
  const unlistedModels = (current?.models || []).filter((model) => !catalog.includes(model));

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div className="space-y-1">
          <h1 className="flex items-center gap-2.5 text-2xl font-semibold tracking-tight text-foreground">
            <Layers className="size-5 text-primary" aria-hidden="true" />
            <span>模型组</span>
          </h1>
          <p className="text-xs text-muted-foreground">
            按组优先级 → 入口优先级调度；同级入口可按顺序、随机方式选择并保持会话粘性。
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={saving}
            onClick={() => {
              try {
                const group = newModelGroup(catalog);
                change((list) => list.push(group));
                setSelected(group.id);
                setEditorTab('models');
                addToast('已新建模型组，保存后才会写入配置', 'success');
              } catch (error) {
                addToast(`新建失败：${error.message || '未知错误'}`, 'error');
              }
            }}
          >
            <Plus className="size-3.5" />
            <span>新建模型组</span>
          </Button>
          <Button
            type="button"
            size="sm"
            disabled={saving || (!isDirty && config?.modelGroups != null)}
            onClick={save}
          >
            {saving ? '保存中…' : '保存更改'}
          </Button>
          {isDirty && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={saving}
              onClick={() => {
                dirty.current = false;
                setDirty(false);
                draftSource.current = configDoc;
                setGroups(groupModels(config));
              }}
            >
              撤销更改
            </Button>
          )}
        </div>
      </div>

      {groups.length === 0 && (
        <Card>
          <CardContent className="py-8 text-center text-xs text-muted-foreground">
            尚无模型组。新建组后选择模型，再从入口库添加入口。
          </CardContent>
        </Card>
      )}

      {current && (
        <div className="grid gap-5 lg:grid-cols-[300px_minmax(0,1fr)]">
          <div className="space-y-4">
            <nav className="space-y-1.5" aria-label="模型组列表">
              {groups.map((g, index) => {
                const active = current?.id === g.id;
                return (
                  <div
                    key={g.id}
                    draggable={!saving}
                    onDragStart={(event) => {
                      if (event.target.closest('input, select, label, button')) {
                        event.preventDefault();
                        return;
                      }
                      drag.current = { kind: 'group', index };
                    }}
                    onDragEnd={() => { drag.current = null; }}
                    onDragOver={(e) => e.preventDefault()}
                    onDrop={(e) => {
                      e.preventDefault();
                      if (drag.current?.kind === 'group') {
                        change((list) => move(list, drag.current.index, index));
                      }
                      drag.current = null;
                    }}
                    className={cn(
                      'rounded-xl border transition-colors',
                      active
                        ? 'border-primary/50 bg-primary/10 shadow-xs'
                        : 'bg-card hover:bg-muted/40',
                    )}
                  >
                    <button
                      type="button"
                      disabled={saving}
                      onClick={() => setSelected(g.id)}
                      aria-pressed={active}
                      className="flex w-full cursor-pointer items-center justify-between gap-2 px-3.5 py-2.5 text-left"
                    >
                      <span className="min-w-0">
                        <strong className="block truncate text-xs font-semibold text-foreground">
                          {g.name || '未命名模型组'}
                        </strong>
                        <small className="block truncate text-[11px] text-muted-foreground tabular-nums">
                          {g.models.length} 模型 · {g.bindings.length} 入口{g.enabled ? '' : ' · 停用'}
                        </small>
                      </span>
                      {active && <Check className="size-4 shrink-0 text-primary" />}
                    </button>
                  </div>
                );
              })}
            </nav>

            <Card>
              <CardHeader className="pb-2">
                <CardTitle className="text-xs font-semibold">当前模型组设置</CardTitle>
              </CardHeader>
              <CardContent>
                <fieldset disabled={saving} className="space-y-3" aria-label="当前模型组设置">
                  <label className="flex flex-col gap-1 text-xs">
                    <span className="font-medium text-muted-foreground">组名称</span>
                    <Input
                      className="h-8 text-xs"
                      value={current.name}
                      onChange={(e) => updateGroup(current.id, (draft) => { draft.name = e.target.value; })}
                    />
                  </label>
                  <label className="flex flex-col gap-1 text-xs">
                    <span className="font-medium text-muted-foreground">优先级</span>
                    <Input
                      type="number"
                      min="0"
                      step="1"
                      aria-label="组优先级"
                      title="数字越小越优先；同级按顶部排列顺序。"
                      className="h-8 font-mono text-xs tabular-nums"
                      value={current.priority}
                      onChange={(e) => updateGroup(current.id, (draft) => { draft.priority = Number(e.target.value); })}
                    />
                  </label>
                  <label className="flex flex-col gap-1 text-xs">
                    <span className="font-medium text-muted-foreground">调度策略</span>
                    <select
                      className={SELECT_CLS}
                      aria-label="模型组调度策略"
                      title="只影响当前模型组内同优先级入口"
                      value={current.schedulingStrategy || 'priority'}
                      onChange={(e) => updateGroup(current.id, (draft) => { draft.schedulingStrategy = e.target.value; })}
                    >
                      <option value="priority">优先级顺序</option>
                      <option value="randomSticky">同优先级随机并保持会话粘性</option>
                      <option value="roundRobinSticky">同优先级按顺序轮询并保持会话粘性</option>
                    </select>
                  </label>
                  <label className="flex cursor-pointer items-center gap-2 pt-1 text-xs font-medium text-foreground">
                    <input
                      type="checkbox"
                      className="size-3.5 accent-primary"
                      checked={current.enabled}
                      onChange={(e) => updateGroup(current.id, (draft) => { draft.enabled = e.target.checked; })}
                    />
                    启用该组
                  </label>

                  <details
                    className="rounded-lg border bg-muted/20"
                    key={current.id}
                    onKeyDown={(event) => {
                      if (event.key === 'Escape') {
                        event.currentTarget.open = false;
                        event.currentTarget.querySelector('summary')?.focus();
                      }
                    }}
                  >
                    <summary className="flex cursor-pointer list-none items-center justify-between px-3 py-2 text-xs font-medium text-muted-foreground select-none hover:text-foreground">
                      <span>组操作</span>
                      <ChevronRight className="size-3.5" />
                    </summary>
                    <div className="flex flex-wrap gap-1.5 border-t p-2">
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="h-7 gap-1 px-2 text-xs"
                        disabled={saving || groups.indexOf(current) === 0}
                        onClick={(event) => {
                          event.currentTarget.closest('details').open = false;
                          change((list) => move(list, groups.indexOf(current), groups.indexOf(current) - 1));
                        }}
                      >
                        <ArrowUp className="size-3" /> 前移
                      </Button>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="h-7 gap-1 px-2 text-xs"
                        disabled={saving || groups.indexOf(current) === groups.length - 1}
                        onClick={(event) => {
                          event.currentTarget.closest('details').open = false;
                          change((list) => move(list, groups.indexOf(current), groups.indexOf(current) + 1));
                        }}
                      >
                        <ArrowDown className="size-3" /> 后移
                      </Button>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="h-7 gap-1 px-2 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
                        disabled={saving}
                        onClick={(event) => {
                          event.currentTarget.closest('details').open = false;
                          removeGroup(current);
                        }}
                      >
                        <Trash2 className="size-3" /> 删除模型组
                      </Button>
                    </div>
                  </details>
                </fieldset>
              </CardContent>
            </Card>
          </div>

          <Card key={current.id} className="min-w-0">
            <CardContent className="p-4 md:p-5">
              <fieldset disabled={saving} className="space-y-4">
                <Tabs value={editorTab} onValueChange={setEditorTab}>
                  <TabsList aria-label="模型组编辑层级">
                    <TabsTrigger value="models" className="gap-1.5 text-xs">
                      <span>模型选择</span>
                      <Badge variant="outline" className="px-1.5 py-0 font-mono text-[10px]">{current.models.length}</Badge>
                    </TabsTrigger>
                    <TabsTrigger value="bindings" className="gap-1.5 text-xs">
                      <span>入口绑定</span>
                      <Badge variant="outline" className="px-1.5 py-0 font-mono text-[10px]">{current.bindings.length}</Badge>
                    </TabsTrigger>
                  </TabsList>
                </Tabs>

                {editorTab === 'models' && (
                  <div className="space-y-3">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <strong className="text-xs font-semibold text-foreground tabular-nums">
                        已选 {current.models.length - unlistedModels.length} 个模型
                      </strong>
                      <div className="flex items-center gap-1.5">
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="h-7 px-2 text-xs"
                          onClick={() => updateGroup(current.id, (g) => {
                            g.models = [...new Set([...g.models, ...catalog])];
                          })}
                        >
                          全选已添加
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="h-7 px-2 text-xs"
                          onClick={() => updateGroup(current.id, (g) => {
                            g.models = [];
                            pruneGroupReferences({ endpoints, modelGroups: [g] });
                          })}
                        >
                          清空
                        </Button>
                      </div>
                    </div>
                    <p className="text-[11px] text-muted-foreground">
                      新组默认勾选入口库已添加的模型；已绑定入口新增模型时自动勾选，仍可手动取消。
                    </p>
                    <CategoryPicker
                      categories={categories}
                      selected={current.models}
                      onChange={(models) => updateGroup(current.id, (g) => {
                        g.models = models;
                        pruneGroupReferences({ endpoints, modelGroups: [g] });
                      })}
                    />
                    {!!unlistedModels.length && (
                      <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-status-warning/35 bg-status-warning/10 px-3 py-2 text-xs">
                        <p className="text-status-warning">
                          {unlistedModels.length} 项历史选择未在入口库添加，已从列表隐藏。
                        </p>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="h-7 px-2 text-xs"
                          onClick={() => updateGroup(current.id, (g) => {
                            g.models = g.models.filter((model) => catalog.includes(model));
                            pruneGroupReferences({ endpoints, modelGroups: [g] });
                          })}
                        >
                          清理历史选择
                        </Button>
                      </div>
                    )}
                    {current.models.some((m) => !current.bindings.some((b) => b.enabled && endpoints.some((e) => e.id === b.endpointID && e.enabled !== false && (e.modelMappings || e.mappings || []).some((mapping) => modelMatches(mapping.from ?? mapping.clientPattern, m))) && (b.models == null || b.models.some((p) => modelMatches(p, m))))) && (
                      <p role="status" className="text-[11px] text-status-warning">
                        部分模型尚未配置启用的承接入口；请求会继续检查其他组。
                      </p>
                    )}
                  </div>
                )}

                {editorTab === 'bindings' && (
                  <section className="space-y-3">
                    {current.id === 'default' && (
                      <p className="rounded-lg border bg-muted/30 px-3 py-2 text-[11px] text-muted-foreground" role="note">
                        默认组已绑定入口的顺序与优先级跟随入口库，请到入口库调整。新入口不会自动加入任何模型组，请在此手动添加；新绑定默认承接全部可用模型。
                      </p>
                    )}

                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <label className="flex items-center gap-2 text-xs">
                        <span className="font-medium text-muted-foreground">从入口库添加</span>
                        <select
                          className={SELECT_CLS}
                          value=""
                          onChange={(e) => {
                            const id = e.target.value;
                            if (id) {
                              updateGroup(current.id, (g) => g.bindings.push({
                                endpointID: id,
                                enabled: true,
                                priority: 0,
                                models: null,
                                overrides: [],
                              }));
                            }
                          }}
                        >
                          <option value="">选择入口…</option>
                          {endpoints
                            .filter((e) => !current.bindings.some((b) => b.endpointID === e.id))
                            .map((e) => (
                              <option key={e.id} value={e.id}>
                                {e.name || e.id}{e.enabled === false ? '（入口已停用）' : ''}
                              </option>
                            ))}
                        </select>
                      </label>
                      <span className="text-[11px] text-muted-foreground">点击入口整行展开编辑</span>
                    </div>

                    {!current.bindings.length && (
                      <p className="rounded-xl border border-dashed px-4 py-8 text-center text-xs text-muted-foreground">
                        添加入口后，可从该入口支持的模型中选择承接范围。
                      </p>
                    )}

                    <div className="space-y-2.5">
                      {current.bindings.map((b, index) => {
                        const endpoint = endpoints.find((e) => e.id === b.endpointID);
                        const available = endpointGroupModels(endpoint, current.models);
                        const edit = (fn) => updateGroup(current.id, (g) => fn(g.bindings[index]));
                        const summaryText = b.models == null
                          ? `全部可用模型 · 可选 ${available.length}`
                          : `已选 ${available.filter((name) => b.models.some((pattern) => modelMatches(pattern, name))).length} · 可选 ${available.length}`;

                        return (
                          <details
                            className="group overflow-hidden rounded-xl border bg-card"
                            key={b.endpointID}
                            draggable={!saving && current.id !== 'default'}
                            onDragStart={(event) => {
                              if (event.target.closest('input, textarea, select, button, summary')) {
                                event.preventDefault();
                                return;
                              }
                              drag.current = { kind: current.id, index };
                              event.dataTransfer.setData('text/plain', b.endpointID);
                            }}
                            onDragEnd={() => { drag.current = null; }}
                            onDragOver={(e) => e.preventDefault()}
                            onDrop={(e) => {
                              e.preventDefault();
                              if (drag.current?.kind === current.id) {
                                updateGroup(current.id, (g) => move(g.bindings, drag.current.index, index));
                              }
                              drag.current = null;
                            }}
                          >
                            <summary className="model-group-binding-summary flex cursor-pointer list-none flex-wrap items-center gap-3 px-3.5 py-2.5 text-xs select-none hover:bg-muted/35">
                              <Server className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                              <span className="min-w-0 flex-1">
                                <strong className="block truncate text-xs font-semibold text-foreground">
                                  {endpoint?.name || b.endpointID}
                                </strong>
                                <small className="block truncate text-[11px] text-muted-foreground tabular-nums">
                                  {summaryText} · {current.id === 'default' ? '优先级跟随入口库' : `优先级 ${b.priority}`}
                                </small>
                              </span>
                              <span className={cn(endpoint?.enabled === false && 'text-status-warning')}>
                                <label
                                  className="binding-state-toggle inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] font-medium"
                                  title={endpoint?.enabled === false ? '入口库已停用，启用入口后才会参与调度' : '在此组中启用或停用该入口'}
                                  onClick={(event) => event.stopPropagation()}
                                >
                                  <input
                                    type="checkbox"
                                    className="size-3.5 accent-primary"
                                    aria-label={`在此组中启用 ${endpoint?.name || b.endpointID}`}
                                    checked={b.enabled}
                                    disabled={saving || endpoint?.enabled === false}
                                    onChange={(event) => edit((value) => { value.enabled = event.target.checked; })}
                                  />
                                  <span>{endpoint?.enabled === false ? '入口库已停用' : b.enabled ? '组内启用' : '组内停用'}</span>
                                </label>
                              </span>
                              {current.id !== 'default' && (
                                <div className="flex items-center gap-1" onClick={(event) => event.preventDefault()}>
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="icon-sm"
                                    className="size-6 text-xs"
                                    aria-label="入口上移"
                                    disabled={!index}
                                    onClick={() => updateGroup(current.id, (g) => move(g.bindings, index, index - 1))}
                                  >
                                    ↑
                                  </Button>
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="icon-sm"
                                    className="size-6 text-xs"
                                    aria-label="入口下移"
                                    disabled={index === current.bindings.length - 1}
                                    onClick={() => updateGroup(current.id, (g) => move(g.bindings, index, index + 1))}
                                  >
                                    ↓
                                  </Button>
                                </div>
                              )}
                              <ChevronRight className="size-3.5 shrink-0 text-muted-foreground transition-transform group-open:rotate-90" />
                            </summary>

                            <ModelGroupBindingEditor
                              binding={b}
                              models={current.models}
                              endpoint={endpoint}
                              edit={edit}
                              followsLibrary={current.id === 'default'}
                              remove={() => updateGroup(current.id, (g) => g.bindings.splice(index, 1))}
                            />
                          </details>
                        );
                      })}
                    </div>
                  </section>
                )}
              </fieldset>
            </CardContent>
          </Card>
        </div>
      )}
    </div>
  );
}
