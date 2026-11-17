import { useEffect, useRef, useState } from 'react';
import { ArrowDown, ArrowUp, Check, ChevronRight, GripVertical, Layers, Plus, Search, Server, Trash2 } from 'lucide-react';
import { cn } from '../lib/utils.js';
import { useAppControls } from '../context/AppContext.jsx';
import { ModelGroupBindingEditor } from '../components/ModelGroupBindingEditor.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { clone } from '../utils/helpers.js';
import { endpointGroupModels, groupModels, modelCatalogCategories, modelMatches, newModelGroup, pruneGroupReferences } from '../utils/modelGroups.js';
import { Badge } from '../components/ui/badge.jsx';
import { Button } from '../components/ui/button.jsx';
import { Card, CardContent, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { Checkbox } from '../components/ui/checkbox.jsx';
import { Input } from '../components/ui/input.jsx';
import { InputGroup, InputGroupAddon, InputGroupInput } from '../components/ui/input-group.jsx';
import { NativeSelect } from '../components/ui/native-select.jsx';
import { PageHeader } from '../components/ui/page-header.jsx';
import { Tabs, TabsList, TabsTrigger } from '../components/ui/tabs.jsx';

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
    <div className="space-y-3.5">
      <div className="flex items-center gap-2">
        <InputGroup className="flex-1">
          <InputGroupAddon>
            <Search className="size-3.5 text-muted-foreground" />
          </InputGroupAddon>
          <InputGroupInput
            aria-label="搜索模型"
            placeholder="搜索模型名称…"
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
                  'flex cursor-pointer items-center justify-between gap-2 rounded-xl border px-3.5 py-2.5 text-left text-xs transition-all duration-150',
                  isActive
                    ? 'border-primary/45 bg-primary/[0.08] font-semibold text-foreground shadow-2xs'
                    : 'border-border/80 bg-card text-muted-foreground hover:border-border hover:bg-accent/50 hover:text-foreground',
                )}
              >
                <span className="truncate">{category.title}</span>
                <small
                  className={cn(
                    'rounded-full px-2 py-0.5 font-mono text-[10px] tabular-nums',
                    isActive ? 'bg-primary/15 text-primary' : 'bg-muted text-muted-foreground',
                  )}
                >
                  {selectedCount}/{category.models.length}
                </small>
              </button>
            );
          })}
        </nav>
      )}

      {displayed.map((category) => (
        <section className="space-y-3 rounded-2xl border border-border/80 bg-muted/20 p-4" key={category.id}>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-baseline gap-2 text-xs">
              <strong className="font-semibold text-foreground">{category.title} 系列</strong>
              <small className="text-muted-foreground">{category.models.length} 个模型</small>
            </div>
            <div className="flex items-center gap-1">
              <Button
                type="button"
                variant="ghost"
                size="xs"
                onClick={() => update([...selected, ...category.models])}
              >
                全选
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="xs"
                onClick={() => update(selected.filter((model) => !category.models.includes(model)))}
              >
                清空
              </Button>
            </div>
          </div>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {category.models.map((model) => {
              const checked = selected.includes(model);
              return (
                <label
                  key={model}
                  className={cn(
                    'flex cursor-pointer items-center gap-2.5 rounded-xl border px-3 py-2 text-xs transition-all duration-150',
                    checked
                      ? 'border-primary/40 bg-primary/[0.08] text-foreground shadow-2xs'
                      : 'border-border/75 bg-card/90 text-muted-foreground hover:border-border hover:bg-card hover:text-foreground',
                  )}
                >
                  <Checkbox
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
        <p className="rounded-2xl border border-dashed border-border/80 bg-muted/15 px-4 py-10 text-center text-xs text-muted-foreground">
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
      <PageHeader
        icon={Layers}
        title={<span>模型组</span>}
        badge={isDirty ? <Badge variant="warning">未保存更改</Badge> : null}
        description="按组优先级 → 入口优先级调度；同级入口可按顺序、随机方式选择并保持会话粘性。"
        actions={(
          <>
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
            <Button
              type="button"
              size="sm"
              disabled={saving || (!isDirty && config?.modelGroups != null)}
              onClick={save}
            >
              {saving ? '保存中…' : '保存更改'}
            </Button>
          </>
        )}
      />

      {groups.length === 0 && (
        <Card>
          <CardContent className="py-10 text-center text-xs text-muted-foreground">
            尚无模型组。新建组后选择模型，再从入口库添加入口。
          </CardContent>
        </Card>
      )}

      {current && (
        <div className="grid gap-5 lg:grid-cols-[310px_minmax(0,1fr)] lg:items-start">
          <div className="space-y-4">
            <nav className="space-y-2" aria-label="模型组列表">
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
                      'rounded-2xl border transition-all duration-150',
                      active
                        ? 'border-primary/45 bg-card shadow-[var(--shadow-card),inset_3px_0_0_var(--primary)]'
                        : 'border-border/80 bg-card/85 hover:border-border hover:bg-card',
                    )}
                  >
                    <button
                      type="button"
                      disabled={saving}
                      onClick={() => setSelected(g.id)}
                      aria-pressed={active}
                      className="flex w-full cursor-pointer items-center justify-between gap-2.5 px-4 py-3 text-left"
                    >
                      <span className="flex min-w-0 items-center gap-2.5">
                        <GripVertical className="size-3.5 shrink-0 text-muted-foreground/60" aria-hidden="true" />
                        <span className="min-w-0 space-y-0.5">
                          <strong className="block truncate text-xs font-semibold text-foreground">
                            {g.name || '未命名模型组'}
                          </strong>
                          <small className="block truncate text-[11px] text-muted-foreground tabular-nums">
                            {g.models.length} 模型 · {g.bindings.length} 入口{g.enabled ? '' : ' · 已停用'}
                          </small>
                        </span>
                      </span>
                      {active && <Check className="size-4 shrink-0 text-primary" />}
                    </button>
                  </div>
                );
              })}
            </nav>

            <Card>
              <CardHeader className="border-b border-border/70 pb-3.5">
                <CardTitle className="text-xs font-semibold">当前模型组设置</CardTitle>
              </CardHeader>
              <CardContent>
                <fieldset disabled={saving} className="space-y-3.5" aria-label="当前模型组设置">
                  <label className="flex flex-col gap-1.5 text-xs">
                    <span className="font-medium text-foreground/85">组名称</span>
                    <Input
                      className="h-9 text-xs"
                      value={current.name}
                      onChange={(e) => updateGroup(current.id, (draft) => { draft.name = e.target.value; })}
                    />
                  </label>
                  <label className="flex flex-col gap-1.5 text-xs">
                    <span className="font-medium text-foreground/85">优先级</span>
                    <Input
                      type="number"
                      min="0"
                      step="1"
                      aria-label="组优先级"
                      title="数字越小越优先；同级按顶部排列顺序。"
                      className="h-9 font-mono text-xs tabular-nums"
                      value={current.priority}
                      onChange={(e) => updateGroup(current.id, (draft) => { draft.priority = Number(e.target.value); })}
                    />
                  </label>
                  <label className="flex flex-col gap-1.5 text-xs">
                    <span className="font-medium text-foreground/85">调度策略</span>
                    <NativeSelect
                      aria-label="模型组调度策略"
                      title="只影响当前模型组内同优先级入口"
                      value={current.schedulingStrategy || 'priority'}
                      onChange={(e) => updateGroup(current.id, (draft) => { draft.schedulingStrategy = e.target.value; })}
                    >
                      <option value="priority">优先级顺序</option>
                      <option value="randomSticky">同优先级随机并保持会话粘性</option>
                      <option value="roundRobinSticky">同优先级按顺序轮询并保持会话粘性</option>
                    </NativeSelect>
                  </label>
                  <div className="flex items-center justify-between rounded-xl bg-muted/40 px-3 py-2.5">
                    <span className="text-xs font-medium text-foreground">启用该组</span>
                    <QuickToggle
                      checked={current.enabled}
                      onChange={(val) => updateGroup(current.id, (draft) => { draft.enabled = val; })}
                      ariaLabel="启用该模型组"
                    />
                  </div>

                  <details
                    className="group rounded-xl border border-border/80 bg-muted/20"
                    key={current.id}
                    onKeyDown={(event) => {
                      if (event.key === 'Escape') {
                        event.currentTarget.open = false;
                        event.currentTarget.querySelector('summary')?.focus();
                      }
                    }}
                  >
                    <summary className="flex cursor-pointer list-none items-center justify-between px-3.5 py-2.5 text-xs font-medium text-muted-foreground select-none hover:text-foreground">
                      <span>组排序与删除</span>
                      <ChevronRight className="size-3.5 transition-transform duration-150 group-open:rotate-90" />
                    </summary>
                    <div className="flex flex-wrap gap-1.5 border-t border-border/65 p-2.5">
                      <Button
                        type="button"
                        variant="outline"
                        size="xs"
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
                        variant="outline"
                        size="xs"
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
                        size="xs"
                        className="text-destructive hover:bg-destructive/10 hover:text-destructive"
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
            <CardContent className="px-5">
              <fieldset disabled={saving} className="space-y-4">
                <Tabs value={editorTab} onValueChange={setEditorTab}>
                  <TabsList aria-label="模型组编辑层级">
                    <TabsTrigger value="models" className="gap-1.5 text-xs">
                      <span>模型选择</span>
                      <Badge variant="secondary" className="px-1.5 py-0 font-mono text-[10px]">{current.models.length}</Badge>
                    </TabsTrigger>
                    <TabsTrigger value="bindings" className="gap-1.5 text-xs">
                      <span>入口绑定</span>
                      <Badge variant="secondary" className="px-1.5 py-0 font-mono text-[10px]">{current.bindings.length}</Badge>
                    </TabsTrigger>
                  </TabsList>
                </Tabs>

                {editorTab === 'models' && (
                  <div className="space-y-3.5">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <strong className="text-xs font-semibold text-foreground tabular-nums">
                        已选 {current.models.length - unlistedModels.length} 个模型
                      </strong>
                      <div className="flex items-center gap-1.5">
                        <Button
                          type="button"
                          variant="outline"
                          size="xs"
                          onClick={() => updateGroup(current.id, (g) => {
                            g.models = [...new Set([...g.models, ...catalog])];
                          })}
                        >
                          全选已添加
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="xs"
                          onClick={() => updateGroup(current.id, (g) => {
                            g.models = [];
                            pruneGroupReferences({ endpoints, modelGroups: [g] });
                          })}
                        >
                          清空
                        </Button>
                      </div>
                    </div>
                    <p className="text-[11px] leading-relaxed text-muted-foreground">
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
                      <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-status-warning/30 bg-status-warning/10 px-3.5 py-2.5 text-xs">
                        <p className="text-status-warning">
                          {unlistedModels.length} 项历史选择未在入口库添加，已从列表隐藏。
                        </p>
                        <Button
                          type="button"
                          variant="ghost"
                          size="xs"
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
                  <section className="space-y-3.5">
                    {current.id === 'default' && (
                      <p className="rounded-xl border border-border/75 bg-muted/35 px-3.5 py-2.5 text-[11px] leading-relaxed text-muted-foreground" role="note">
                        默认组已绑定入口的顺序与优先级跟随入口库，请到入口库调整。新入口不会自动加入任何模型组，请在此手动添加；新绑定默认承接全部可用模型。
                      </p>
                    )}

                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <label className="flex items-center gap-2 text-xs">
                        <span className="shrink-0 font-medium text-muted-foreground">从入口库添加</span>
                        <NativeSelect
                          size="sm"
                          wrapperClassName="w-auto min-w-[180px]"
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
                        </NativeSelect>
                      </label>
                      <span className="text-[11px] text-muted-foreground">点击入口整行展开编辑</span>
                    </div>

                    {!current.bindings.length && (
                      <p className="rounded-2xl border border-dashed border-border/80 bg-muted/15 px-4 py-10 text-center text-xs text-muted-foreground">
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
                            className="group overflow-hidden rounded-2xl border border-border/80 bg-card shadow-2xs"
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
                            <summary className="model-group-binding-summary flex cursor-pointer list-none flex-wrap items-center gap-3 px-4 py-3 text-xs select-none transition-colors hover:bg-muted/35">
                              <span className="flex size-8 shrink-0 items-center justify-center rounded-xl border border-border/75 bg-muted/40 text-primary" aria-hidden="true">
                                <Server className="size-4" />
                              </span>
                              <span className="min-w-0 flex-1 space-y-0.5">
                                <strong className="block truncate text-xs font-semibold text-foreground">
                                  {endpoint?.name || b.endpointID}
                                </strong>
                                <small className="block truncate text-[11px] text-muted-foreground tabular-nums">
                                  {summaryText} · {current.id === 'default' ? '优先级跟随入口库' : `优先级 ${b.priority}`}
                                </small>
                              </span>
                              <span className={cn(endpoint?.enabled === false && 'text-status-warning')}>
                                <label
                                  className="binding-state-toggle relative inline-flex cursor-pointer items-center gap-2 rounded-full border border-border/80 bg-background/85 px-2.5 py-1 text-[11px] font-medium shadow-2xs transition-colors hover:bg-accent/60"
                                  title={endpoint?.enabled === false ? '入口库已停用，启用入口后才会参与调度' : '在此组中启用或停用该入口'}
                                  onClick={(event) => event.stopPropagation()}
                                >
                                  <input
                                    type="checkbox"
                                    className="peer sr-only"
                                    aria-label={`在此组中启用 ${endpoint?.name || b.endpointID}`}
                                    checked={b.enabled}
                                    disabled={saving || endpoint?.enabled === false}
                                    onChange={(event) => edit((value) => { value.enabled = event.target.checked; })}
                                  />
                                  <span
                                    aria-hidden="true"
                                    className="relative inline-flex h-4 w-7 shrink-0 items-center rounded-full bg-input/90 transition-colors peer-checked:bg-primary peer-disabled:opacity-50"
                                  >
                                    <span
                                      className={cn(
                                        'size-3 rounded-full bg-white shadow-xs transition-transform duration-150',
                                        b.enabled ? 'translate-x-3.5' : 'translate-x-0.5',
                                      )}
                                    />
                                  </span>
                                  <span>{endpoint?.enabled === false ? '入口库已停用' : b.enabled ? '组内启用' : '组内停用'}</span>
                                </label>
                              </span>
                              {current.id !== 'default' && (
                                <div className="flex items-center gap-1" onClick={(event) => event.preventDefault()}>
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="icon-sm"
                                    className="size-7 text-xs"
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
                                    className="size-7 text-xs"
                                    aria-label="入口下移"
                                    disabled={index === current.bindings.length - 1}
                                    onClick={() => updateGroup(current.id, (g) => move(g.bindings, index, index + 1))}
                                  >
                                    ↓
                                  </Button>
                                </div>
                              )}
                              <ChevronRight className="size-3.5 shrink-0 text-muted-foreground transition-transform duration-150 group-open:rotate-90" />
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
