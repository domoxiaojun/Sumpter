import { useEffect, useRef, useState } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { usePendingActions } from '../hooks/usePendingActions.js';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { clone } from '../utils/helpers.js';
import { randomToken, validateInboundTokens } from '../utils/inboundTokens.js';

function draft(config) {
  return { legacyName: config?.listener?.authTokenName || '默认 Token', entries: (config?.listener?.authTokens || []).map(entry => ({ ...entry, token: '' })) };
}

export function InboundTokensPanel() {
  const { config, configDoc, secretStatus, saveConfig, addToast } = useAppControls();
  const source = useRef(configDoc);
  const [value, setValue] = useState(() => draft(config));
  const [baseline, setBaseline] = useState(() => JSON.stringify(draft(config)));
  const [visible, setVisible] = useState({});
  const { pending, run } = usePendingActions();
  const dirty = JSON.stringify(value) !== baseline;
  useUnsavedChanges(dirty, Boolean(pending.save));
  useEffect(() => {
    if (dirty) return;
    source.current = configDoc;
    const next = draft(config);
    setValue(next); setBaseline(JSON.stringify(next));
  }, [configDoc, dirty]);
  const change = (id, patch) => setValue(prev => ({ ...prev, entries: prev.entries.map(entry => entry.id === id ? { ...entry, ...patch } : entry) }));
  const add = () => setValue(prev => ({ ...prev, entries: [...prev.entries, { id: randomToken('token-'), name: '', enabled: true, token: randomToken() }] }));
  const save = () => run('save', async () => {
    const error = validateInboundTokens(value.entries);
    if (error) return addToast(error, 'warning');
    if (!value.legacyName.trim()) return addToast('默认 Token 名称不能为空', 'warning');
    if (!value.entries.length && !secretStatus?.inboundAuthToken?.configured && (source.current.config.listener.authTokens || []).length && !confirm('删除最后一个 Token 后将关闭入站认证。确认保存？')) return;
    try {
      const next = clone(source.current.config);
      next.listener.authTokenName = value.legacyName.trim();
      next.listener.authTokens = value.entries.map(entry => ({ id: entry.id, name: entry.name.trim(), enabled: entry.enabled, token: '' }));
      const secrets = Object.fromEntries(value.entries.filter(entry => entry.token).map(entry => [entry.id, entry.token]));
      source.current = await saveConfig(next, { inboundTokens: secrets }, source.current);
      const saved = draft(source.current.config);
      setValue(saved); setBaseline(JSON.stringify(saved)); setVisible({});
      addToast('Token 列表已保存，后续请求立即生效', 'success');
    } catch (error) { addToast(error.message, 'error'); }
  });
  return <section className="glass-panel panel-padded-stack">
    <h2 className="panel-title">多个入站客户端 API Token</h2>
    <p className="panel-description">给不同客户端分配独立 Token，可按名称筛选请求、Token 用量和费用。保存后只显示尾号，请先复制新生成的 Token。未配置默认 Token 时，全部停用会拒绝请求；删除不会清除历史统计。</p>
    {secretStatus?.inboundAuthToken?.configured && <label className="form-group"><span className="form-label">默认 Token 名称</span><input className="form-input" value={value.legacyName} disabled={pending.save} onChange={e => setValue(prev => ({ ...prev, legacyName: e.target.value }))} /></label>}
    {value.entries.map((entry, index) => <div className="glass-panel panel-padded-stack" key={entry.id}>
      <div className="grid-2col">
        <label className="form-group"><span className="form-label">Token 名称</span><input className="form-input" value={entry.name} placeholder="例如：工作电脑、个人项目" disabled={pending.save} onChange={e => change(entry.id, { name: e.target.value })} /></label>
        <label className="form-group"><span className="form-label">Token 凭据</span><input className="form-input" type={visible[entry.id] ? 'text' : 'password'} autoComplete="off" value={entry.token} disabled={pending.save} placeholder={secretStatus?.inboundTokens?.[entry.id]?.configured ? '留空保留现有 Token · 尾号 ' + secretStatus.inboundTokens[entry.id].last4 : '输入至少 16 位 Token'} onChange={e => change(entry.id, { token: e.target.value })} /></label>
      </div>
      <div className="panel-actions">
        <label><input type="checkbox" checked={entry.enabled} disabled={pending.save} onChange={e => change(entry.id, { enabled: e.target.checked })} /> 启用</label>
        <button type="button" className="btn btn-secondary" disabled={pending.save} onClick={() => setVisible(prev => ({ ...prev, [entry.id]: !prev[entry.id] }))}>{visible[entry.id] ? '隐藏' : '显示'}凭据</button>
        <button type="button" className="btn btn-secondary" disabled={pending.save} onClick={() => change(entry.id, { token: randomToken() })}>生成新凭据</button>
        <button type="button" className="btn btn-danger" disabled={pending.save} aria-label={'删除 Token ' + (entry.name || index + 1)} onClick={() => setValue(prev => ({ ...prev, entries: prev.entries.filter(item => item.id !== entry.id) }))}>删除</button>
      </div>
    </div>)}
    {!value.entries.length && <p className="form-hint">尚未添加命名 Token。</p>}
    <div className="panel-actions">
      <button type="button" className="btn btn-secondary" disabled={pending.save || value.entries.length >= 128} onClick={add}>新增 Token</button>
      <button type="button" className="btn btn-secondary" disabled={pending.save} onClick={() => {
        if (dirty && !confirm('放弃未保存的 Token 修改并重新载入？')) return;
        source.current = configDoc;
        const next = draft(config); setValue(next); setBaseline(JSON.stringify(next)); setVisible({});
      }}>重新载入</button>
      <button type="button" className="btn btn-primary" disabled={pending.save || !dirty} onClick={save}>{pending.save ? '保存中…' : '保存 Token 列表'}</button>
    </div>
  </section>;
}
