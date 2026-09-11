import React, { useEffect, useRef, useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';
import { ModelGroupBindingEditor } from '../components/ModelGroupBindingEditor.jsx';
import { clone } from '../utils/helpers.js';
import { endpointGroupModels, groupModels, modelCatalogCategories, modelMatches, newModelGroup, pruneGroupReferences } from '../utils/modelGroups.js';

function CategoryPicker({ categories, selected, onChange }) {
  const [query, setQuery] = useState('');
  const [activeID, setActiveID] = useState(null);
  const search = query.trim().toLowerCase();
  const active = categories.find((category) => category.id === activeID) || categories[0];
  const displayed = search
    ? categories.map((category) => ({ ...category, models: category.models.filter((model) => model.toLowerCase().includes(search)) })).filter((category) => category.models.length)
    : active ? [active] : [];
  const update = (models) => onChange([...new Set(models)]);
  return <div className="model-category-picker">
    <div className="model-search"><Icon name="search" size={18} /><input className="form-input" aria-label="搜索模型" placeholder="搜索模型" value={query} onChange={(e) => setQuery(e.target.value)} />{query && <button type="button" className="btn btn-ghost" aria-label="清除搜索" onClick={() => setQuery('')}>清除</button>}</div>
    {!search && <nav className="model-family-grid" aria-label="模型系列">
      {categories.map((category) => <button type="button" key={category.id} className={`model-family-card${active?.id === category.id ? ' is-active' : ''}`}
        aria-pressed={active?.id === category.id} onClick={() => setActiveID(category.id)}>
        <span>{category.title}</span><small>{category.models.filter((model) => selected.includes(model)).length}/{category.models.length}</small>
      </button>)}
    </nav>}
    {displayed.map((category) => <section className="model-category" key={category.id}>
      <div className="model-category-heading"><strong>{category.title} 系列</strong><small>{category.models.length} 个模型</small><div className="model-category-actions"><button type="button" className="btn btn-ghost" onClick={() => update([...selected, ...category.models])}>全选</button><button type="button" className="btn btn-ghost" onClick={() => update(selected.filter((model) => !category.models.includes(model)))}>清空</button></div></div>
      <div className="model-group-models">
        {category.models.map((model) => <label key={model} className={`model-option${selected.includes(model) ? ' is-selected' : ''}`}>
          <input type="checkbox" checked={selected.includes(model)} onChange={(e) => update(e.target.checked ? [...selected, model] : selected.filter((item) => item !== model))} /><span>{model}</span>
        </label>)}
      </div>
    </section>)}
    {!displayed.length && <p className="form-hint model-picker-empty">{search ? '没有匹配的模型，试试其他关键词。' : '暂无可选模型，请先在入口库添加模型，并确认当前组的模型范围。'}</p>}
  </div>;
}

export function ModelGroupsPage() {
  const { config, saveConfig, addToast, openModal } = useApp();
  const [groups, setGroups] = useState(() => groupModels(config));
  const [selected, setSelected] = useState(groups[0]?.id);
  const [editorTab, setEditorTab] = useState('models');
  const [saving, setSaving] = useState(false);
  const dirty = useRef(false);
  const [isDirty, setDirty] = useState(false);
  const drag = useRef(null);
  const endpoints = config?.endpoints || [];
  useEffect(() => {
    if (!dirty.current) setGroups(groupModels(config));
  }, [config]);
  const change = (fn) => {
    dirty.current = true; setDirty(true);
    setGroups((old) => { const next = clone(old); fn(next); return next; });
  };
  const updateGroup = (id, fn) => change((list) => { const group = list.find((g) => g.id === id); if (group) fn(group); });
  const move = (items, from, to) => {
    if (to < 0 || to >= items.length || from === to) return;
    items.splice(to, 0, items.splice(from, 1)[0]);
  };
  const current = groups.find((g) => g.id === selected) || groups[0];
  const removeGroup = (group) => openModal({
    title: `删除模型组「${group.name}」？`,
    content: <p>入口库中的连接配置会保留。保存更改后生效。</p>,
    actions: [
      { label: '取消', kind: 'ghost' },
      { label: '删除模型组', kind: 'danger', onClick: () => {
        change((list) => {
          const index = list.findIndex((g) => g.id === group.id);
          if (index >= 0) list.splice(index, 1);
        });
      } },
    ],
  });
  const save = async () => {
    setSaving(true);
    try {
      const next = pruneGroupReferences({ ...clone(config), modelGroups: clone(groups) });
      await saveConfig(next);
      setGroups(next.modelGroups); dirty.current = false; setDirty(false);
      addToast('模型组已保存，客户端地址保持不变', 'success');
    } catch (error) { addToast(`保存失败：${error.message}`, 'error'); }
    finally { setSaving(false); }
  };
  const categories = modelCatalogCategories(endpoints);
  const catalog = categories.flatMap((category) => category.models);
  const unlistedModels = (current?.models || []).filter((model) => !catalog.includes(model));
  return <div className="page-stack model-groups-page">
    <div className="page-header">
      <div className="page-title-group">
        <h1 className="page-title"><Icon name="route" size={24} style={{ color: 'var(--primary)' }} /><span>模型组</span></h1>
        <p className="page-subtitle">按组优先级 → 入口优先级调度；数字越小越优先，同级按排列顺序。</p>
      </div>
      <div className="page-actions">
        <button type="button" className="btn btn-secondary" disabled={saving} onClick={() => {
          try {
            const group = newModelGroup();
            change((list) => list.push(group));
            setSelected(group.id);
            setEditorTab('models');
            addToast('已新建模型组，保存后才会写入配置', 'success');
          } catch (error) {
            addToast(`新建失败：${error.message || '未知错误'}`, 'error');
          }
        }}>新建模型组</button>
        <button className="btn btn-primary" disabled={saving || (!isDirty && config?.modelGroups != null)} onClick={save}>{saving ? '保存中…' : '保存更改'}</button>
        {isDirty && <button className="btn btn-ghost" disabled={saving} onClick={() => { dirty.current = false; setDirty(false); setGroups(groupModels(config)); }}>撤销更改</button>}
      </div>
    </div>
    {groups.length === 0 && <div className="glass-panel panel-padded">尚无模型组。新建组后选择模型，再从入口库添加入口。</div>}
    <div className="model-group-layout">
      {current && <div className="model-group-overview">
      <nav className="model-group-list" aria-label="模型组列表">
        {groups.map((g, index) => <div key={g.id} className={`model-group-nav ${current?.id === g.id ? 'is-selected' : ''}`}
          draggable={!saving} onDragStart={(event) => {
            if (event.target.closest('input, select, label, button')) { event.preventDefault(); return; }
            drag.current = { kind: 'group', index };
          }} onDragEnd={() => { drag.current = null; }} onDragOver={(e) => e.preventDefault()}
          onDrop={(e) => { e.preventDefault(); if (drag.current?.kind === 'group') change((list) => move(list, drag.current.index, index)); drag.current = null; }}>
          <button className="model-group-select" disabled={saving} onClick={() => setSelected(g.id)} aria-pressed={current?.id === g.id}>
            <span><strong>{g.name || '未命名模型组'}</strong><small>{g.models.length} 模型 · {g.bindings.length} 入口{g.enabled ? '' : ' · 停用'}</small></span>{current?.id === g.id && <Icon name="check" size={16} />}
          </button>
        </div>)}
      </nav>
        <fieldset disabled={saving} className="model-group-card-settings" aria-label="当前模型组设置">
          <label className="group-name-field">组名称<input className="form-input" value={current.name} onChange={(e) => updateGroup(current.id, (draft) => { draft.name = e.target.value; })} /></label>
          <label className="group-priority-field">优先级<input type="number" min="0" step="1" aria-label="组优先级" title="数字越小越优先；同级按顶部排列顺序。" className="form-input" value={current.priority} onChange={(e) => updateGroup(current.id, (draft) => { draft.priority = Number(e.target.value); })} /></label>
          <label className="model-group-card-enabled"><input type="checkbox" checked={current.enabled} onChange={(e) => updateGroup(current.id, (draft) => { draft.enabled = e.target.checked; })} /> 启用该组</label>
          <details className="group-actions-menu" key={current.id} onKeyDown={(event) => { if (event.key === 'Escape') { event.currentTarget.open = false; event.currentTarget.querySelector('summary').focus(); } }}>
            <summary>组操作 <Icon name="chevron" size={14} /></summary>
            <div className="group-actions-popup">
              <button type="button" className="btn btn-ghost" disabled={saving || groups.indexOf(current) === 0} onClick={(event) => { event.currentTarget.closest('details').open = false; change((list) => move(list, groups.indexOf(current), groups.indexOf(current) - 1)); }}>前移</button>
              <button type="button" className="btn btn-ghost" disabled={saving || groups.indexOf(current) === groups.length - 1} onClick={(event) => { event.currentTarget.closest('details').open = false; change((list) => move(list, groups.indexOf(current), groups.indexOf(current) + 1)); }}>后移</button>
              <button type="button" className="btn btn-ghost model-group-card-delete" disabled={saving} onClick={(event) => { event.currentTarget.closest('details').open = false; removeGroup(current); }}>删除模型组</button>
            </div>
          </details>
        </fieldset>
      </div>}
      {current && <div className="model-group-workspace" key={current.id}>
        <fieldset disabled={saving} className="model-group-fieldset panel-stack">
          <nav className="model-group-tabbar" aria-label="模型组编辑层级">
            <button type="button" aria-pressed={editorTab === 'models'} className={`model-group-tab${editorTab === 'models' ? ' is-active' : ''}`} onClick={() => setEditorTab('models')}>模型选择 <small>{current.models.length}</small></button>
            <button type="button" aria-pressed={editorTab === 'bindings'} className={`model-group-tab${editorTab === 'bindings' ? ' is-active' : ''}`} onClick={() => setEditorTab('bindings')}>入口绑定 <small>{current.bindings.length}</small></button>
          </nav>
          {editorTab === 'models' && <>
          <div className="model-group-selection-toolbar"><strong>已选 {current.models.length - unlistedModels.length} 个模型</strong><span /><button className="btn btn-ghost" onClick={() => updateGroup(current.id, (g) => { g.models = [...new Set([...g.models, ...catalog])]; })}>全选已添加</button><button className="btn btn-ghost" onClick={() => updateGroup(current.id, (g) => { g.models = []; pruneGroupReferences({ endpoints, modelGroups: [g] }); })}>清空</button></div>
          <p className="form-hint">仅汇总入口库已添加的模型；点击模型整行即可勾选。</p>
          <CategoryPicker categories={categories} selected={current.models} onChange={(models) => updateGroup(current.id, (g) => { g.models = models; pruneGroupReferences({ endpoints, modelGroups: [g] }); })} />
          {!!unlistedModels.length && <div className="model-unavailable"><p className="form-hint">{unlistedModels.length} 项历史选择未在入口库添加，已从列表隐藏。</p><button type="button" className="btn btn-ghost" onClick={() => updateGroup(current.id, (g) => { g.models = g.models.filter((model) => catalog.includes(model)); pruneGroupReferences({ endpoints, modelGroups: [g] }); })}>清理历史选择</button></div>}
          {current.models.some((m) => !current.bindings.some((b) => b.enabled && endpoints.some((e) => e.id === b.endpointID && e.enabled !== false && (e.modelMappings || e.mappings || []).some((mapping) => modelMatches(mapping.from ?? mapping.clientPattern, m))) && (b.models == null || b.models.some((p) => modelMatches(p, m))))) && <p role="status" className="form-hint">部分模型尚未配置启用的承接入口；请求会继续检查其他组。</p>}
          </>}
          {editorTab === 'bindings' && <section className="model-group-bindings-panel">
            {current.id === 'default' && <p className="form-hint" role="note">默认组已绑定入口的顺序与优先级跟随入口库，请到入口库调整。新入口不会自动加入任何模型组，请在此手动添加并选择承接范围。</p>}
            <div className="model-group-binding-toolbar">
              <label>从入口库添加<select className="form-select" value="" onChange={(e) => { const id = e.target.value; if (id) updateGroup(current.id, (g) => g.bindings.push({ endpointID: id, enabled: true, priority: 0, models: [], overrides: [] })); }}><option value="">选择入口…</option>{endpoints.filter((e) => !current.bindings.some((b) => b.endpointID === e.id)).map((e) => <option key={e.id} value={e.id}>{e.name || e.id}{e.enabled === false ? '（入口已停用）' : ''}</option>)}</select></label>
              <span className="form-hint">点击入口整行展开编辑</span>
            </div>
            {!current.bindings.length && <p className="form-hint">添加入口后，可从该入口支持的模型中选择承接范围。</p>}
            {current.bindings.map((b, index) => {
            const endpoint = endpoints.find((e) => e.id === b.endpointID);
            const available = endpointGroupModels(endpoint, current.models);
            const edit = (fn) => updateGroup(current.id, (g) => fn(g.bindings[index]));
              return <details className="model-group-binding" key={b.endpointID} draggable={!saving && current.id !== 'default'} onDragStart={(event) => {
              if (event.target.closest('input, textarea, select, button, summary')) { event.preventDefault(); return; }
              drag.current = { kind: current.id, index };
              event.dataTransfer.setData('text/plain', b.endpointID);
            }} onDragEnd={() => { drag.current = null; }} onDragOver={(e) => e.preventDefault()} onDrop={(e) => { e.preventDefault(); if (drag.current?.kind === current.id) updateGroup(current.id, (g) => move(g.bindings, drag.current.index, index)); drag.current = null; }}>
              <summary className="model-group-binding-summary"><Icon name="server" size={20} /><span className="binding-header-text"><strong>{endpoint?.name || b.endpointID}</strong><small>{b.models == null ? `全部可用模型 · 可选 ${available.length}` : `已选 ${available.filter((name) => b.models.some((pattern) => modelMatches(pattern, name))).length} · 可选 ${available.length}`} · {current.id === 'default' ? '优先级跟随入口库' : `优先级 ${b.priority}`}</small></span><span className={`binding-state${endpoint?.enabled === false ? ' is-warning' : ''}`}>{endpoint?.enabled === false ? '入口库已停用' : b.enabled ? '组内启用' : '组内停用'}</span><span className="model-binding-edit"><Icon name="chevron" size={16} /></span>{current.id !== 'default' && <div className="model-group-actions"><button type="button" className="btn btn-ghost" aria-label="入口上移" disabled={!index} onClick={(event) => { event.preventDefault(); updateGroup(current.id, (g) => move(g.bindings, index, index - 1)); }}>↑</button><button type="button" className="btn btn-ghost" aria-label="入口下移" disabled={index === current.bindings.length - 1} onClick={(event) => { event.preventDefault(); updateGroup(current.id, (g) => move(g.bindings, index, index + 1)); }}>↓</button></div>}</summary>
              <ModelGroupBindingEditor binding={b} models={current.models} endpoint={endpoint} edit={edit} followsLibrary={current.id === 'default'} remove={() => updateGroup(current.id, (g) => g.bindings.splice(index, 1))} />
              </details>;
            })}
          </section>}
        </fieldset>
      </div>}
    </div>
  </div>;
}
