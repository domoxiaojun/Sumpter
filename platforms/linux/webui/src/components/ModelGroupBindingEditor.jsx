import React, { useState } from 'react';
import { endpointGroupModels, modelMatches } from '../utils/modelGroups.js';
import { Icon } from '../utils/icons.jsx';

// Each model owns its selection and optional overrides in one row.
export function ModelGroupBindingEditor({ binding, models, endpoint, edit, followsLibrary = false, remove }) {
  const [query, setQuery] = useState('');
  const available = endpointGroupModels(endpoint, models);
  const all = binding.models == null;
  const selected = (name) => all || binding.models.some((pattern) => modelMatches(pattern, name));
  const inherited = (name) => !all && selected(name) && !binding.models.includes(name);
  const unavailable = (binding.models || []).filter((name) => !available.includes(name));
  const rows = available;
  const visible = rows.filter((name) => name.toLowerCase().includes(query.trim().toLowerCase()));
  const prune = (value) => {
    if (value.models != null) value.overrides = (value.overrides || []).filter((item) => value.models.some((pattern) => modelMatches(pattern, item.model)));
  };
  const override = (model, field, value) => edit((draft) => {
    draft.overrides ||= [];
    let item = draft.overrides.find((o) => o.model === model);
    if (!item) { item = { model }; draft.overrides.push(item); }
    if (value === '') delete item[field]; else item[field] = field === 'priority' ? Number(value) : value;
    draft.overrides = draft.overrides.filter((o) => o.priority != null || o.upstreamModel != null);
  });
  const summary = all ? `全部可用模型 · 可选 ${available.length}` : `已选 ${available.filter(selected).length} · 可选 ${available.length}`;

  return <div className="binding-editor">
    <div className="binding-settings">
      <label className="binding-setting"><span>组内状态</span><span className="binding-enabled"><input type="checkbox" checked={binding.enabled} onChange={(e) => edit((value) => { value.enabled = e.target.checked; })} /> 在此组中启用</span></label>
      <label className="binding-setting"><span>默认优先级</span>{followsLibrary
        ? <span className="form-hint" title="默认组的顺序与优先级保存时自动跟随入口库">跟随入口库</span>
        : <input className="form-input binding-priority" type="number" min="0" step="1" aria-label="入口优先级" title="数字越小越优先" value={binding.priority} onChange={(e) => edit((value) => { value.priority = Number(e.target.value); })} />}</label>
      <label className="binding-setting"><span>承接范围</span><select className="form-select" value={all ? 'all' : 'selected'} onChange={(e) => edit((value) => { value.models = e.target.value === 'all' ? null : available; prune(value); })}><option value="selected">指定模型</option><option value="all">全部可用模型</option></select></label>
    </div>
    {endpoint?.enabled === false && <p className="binding-notice is-warning">入口库已停用，此处设置保留，启用入口后才会参与调度。</p>}
    {all && <p className="binding-notice">仅承接本入口已添加的组内模型；新增可用模型会自动纳入，移除入口模型后立即停止承接新请求。</p>}
    <section className="binding-model-section" aria-label="模型与覆盖">
      <div className="binding-model-toolbar">
        <div><div className="binding-model-title"><strong>模型与覆盖</strong><span>{summary}</span></div><p className="form-hint">覆盖留空时继承入口设置。</p></div>
        <div className="model-group-actions"><button type="button" className="btn btn-ghost" onClick={() => edit((value) => { value.models = available; prune(value); })}>全选可用</button><button type="button" className="btn btn-ghost" onClick={() => edit((value) => { value.models = []; value.overrides = []; })}>清空</button></div>
      </div>
      {(rows.length > 8 || query) && <input className="form-input" aria-label="搜索入口模型" placeholder="搜索模型" value={query} onChange={(e) => setQuery(e.target.value)} />}
      <div className="binding-model-list">
        <div className="binding-model-columns" aria-hidden="true"><span>模型</span><div><span>上游模型名称</span><span>优先级</span><span /></div></div>
        {visible.map((name) => {
          const checked = selected(name);
          const covered = inherited(name);
          const item = binding.overrides?.find((o) => o.model === name);
          return <div className="binding-model-row" key={name}>
            <label className="binding-model-choice"><input type="checkbox" checked={checked} disabled={all || covered} onChange={(e) => edit((value) => {
              value.models = e.target.checked ? [...new Set([...value.models, name])] : value.models.filter((model) => model !== name); prune(value);
            })} /><span><strong>{name}</strong>{covered ? <small>由已选通配符承接</small> : !available.includes(name) ? <small className="is-warning">未在此入口添加</small> : item ? <small className="is-overridden">已设置覆盖</small> : null}</span></label>
            {name.includes('*') ? <p className="binding-row-hint">通配符沿用入口映射；具体模型可设置单独覆盖。</p> : !checked ? <p className="binding-row-hint">选中后可设置上游名称与优先级</p> : <div className="binding-row-fields">
              <label className="binding-setting"><span>上游模型名称</span><input className="form-input" aria-label={`${name} 上游模型名称`} placeholder="继承入口映射" value={item?.upstreamModel ?? ''} onChange={(e) => override(name, 'upstreamModel', e.target.value)} /></label>
              <label className="binding-setting"><span>优先级</span><input className="form-input" type="number" min="0" step="1" aria-label={`${name} 优先级`} placeholder={`继承 ${binding.priority}`} value={item?.priority ?? ''} onChange={(e) => override(name, 'priority', e.target.value)} /></label>
              <button type="button" className="btn btn-ghost binding-reset" aria-label={`${name} 恢复继承`} title="恢复继承" disabled={!item} onClick={() => edit((value) => { value.overrides = value.overrides.filter((o) => o.model !== name); })}><Icon name="refresh" size={16} /></button>
            </div>}
          </div>;
        })}
        {!visible.length && <p className="binding-empty">{rows.length ? '没有匹配的模型。' : '暂无可选模型，请先在入口库添加模型并纳入当前组。'}</p>}
      </div>
    </section>
    <div className="binding-footer">{!!unavailable.length && <><span className="binding-notice is-warning">{unavailable.length} 项历史选择不在当前可选范围，已从列表隐藏。</span><button type="button" className="btn btn-ghost" onClick={() => edit((value) => { value.models = available.filter(selected); prune(value); })}>清理历史选择</button></>}<button type="button" className="btn btn-ghost binding-remove" onClick={remove}>移出组</button></div>
  </div>;
}
