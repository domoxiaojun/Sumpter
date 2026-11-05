import { useEffect, useRef, useState } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { usePendingActions } from '../hooks/usePendingActions.js';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { api } from '../services/api.js';
import { copyTextToClipboard } from '../utils/clipboard.js';
import { buildInboundTokenWrite, inboundTokenDraft, randomToken } from '../utils/inboundTokens.js';
import { StatusBadge } from './StatusBadge.jsx';
import { Icon } from '../utils/icons.jsx';

export function InboundTokensPanel() {
  const { configDoc, saveConfig, addToast } = useAppControls();
  const source = useRef(configDoc);
  const revision = useRef(0);
  const hideTimers = useRef(new Map());
  const [entries, setEntries] = useState(() => inboundTokenDraft(configDoc));
  const [baseline, setBaseline] = useState(() => JSON.stringify(inboundTokenDraft(configDoc)));
  const [revealed, setRevealed] = useState({});
  const [visible, setVisible] = useState({});
  const [editing, setEditing] = useState({});
  const [error, setError] = useState('');
  const { pending, run } = usePendingActions();
  const busy = Object.values(pending).some(Boolean);
  const dirty = JSON.stringify(entries) !== baseline;
  useUnsavedChanges(dirty, Boolean(pending.save));

  function conceal() {
    revision.current += 1;
    for (const timer of hideTimers.current.values()) window.clearTimeout(timer);
    hideTimers.current.clear();
    setRevealed({}); setVisible({}); setEditing({});
  }
  function load(document) {
    conceal();
    source.current = document;
    const next = inboundTokenDraft(document);
    setEntries(next); setBaseline(JSON.stringify(next)); setError('');
  }
  useEffect(() => {
    if (!dirty) load(configDoc);
    else conceal();
  }, [configDoc]);
  useEffect(() => () => { revision.current += 1; for (const timer of hideTimers.current.values()) window.clearTimeout(timer); }, []);
  useEffect(() => {
    const hide = () => { if (document.hidden) conceal(); };
    document.addEventListener('visibilitychange', hide);
    return () => document.removeEventListener('visibilitychange', hide);
  }, []);
  function hideCredential(id) {
    window.clearTimeout(hideTimers.current.get(id)); hideTimers.current.delete(id);
    setVisible(previous => ({ ...previous, [id]: false }));
    setRevealed(previous => { const next = { ...previous }; delete next[id]; return next; });
  }

  const change = (id, patch) => {
    setError('');
    setEntries(previous => previous.map(entry => entry.id === id ? { ...entry, ...patch } : entry));
  };
  const credential = async entry => {
    if (entry.token !== null) return entry.token;
    if (revealed[entry.id] != null) return revealed[entry.id];
    const result = await api.getInboundTokenSecret(entry.id, source.current.generation);
    return result.token;
  };
  const useCredential = (entry, action) => run(`secret-${entry.id}`, async () => {
    const currentRevision = revision.current;
    setError('');
    try {
      const token = await credential(entry);
      if (currentRevision !== revision.current) return;
      if (action === 'copy') {
        await copyTextToClipboard(token);
        if (currentRevision === revision.current) addToast('Token 已复制到剪贴板', 'success');
      } else {
        setRevealed(previous => ({ ...previous, [entry.id]: token }));
        if (action === 'edit') {
          window.clearTimeout(hideTimers.current.get(entry.id)); hideTimers.current.delete(entry.id);
          setVisible(previous => ({ ...previous, [entry.id]: false }));
          setEditing(previous => ({ ...previous, [entry.id]: true }));
        }
        else {
          setVisible(previous => ({ ...previous, [entry.id]: true }));
          window.clearTimeout(hideTimers.current.get(entry.id));
          hideTimers.current.set(entry.id, window.setTimeout(() => hideCredential(entry.id), 60_000));
        }
      }
    } catch (failure) { if (currentRevision === revision.current) setError(failure.message); }
  });
  const save = () => run('save', async () => {
    setError('');
    try {
      const write = buildInboundTokenWrite(source.current, entries);
      if (write.opensAccess && !confirm('删除所有 Token 后将关闭入站认证。确认保存？')) return;
      const saved = await saveConfig(write.config, write.secrets, source.current);
      load(saved);
    } catch (failure) { setError(failure.message); }
  });
  const add = () => {
    const entry = { id: randomToken('token-'), name: '', enabled: true, token: randomToken() };
    setEntries(previous => [...previous, entry]);
    setEditing(previous => ({ ...previous, [entry.id]: true }));
  };

  return <section className="glass-panel panel-padded-stack inbound-tokens-panel" aria-labelledby="inbound-tokens-title">
    <div className="panel-toolbar">
      <h2 className="panel-title" id="inbound-tokens-title"><Icon name="key" size={18} />入站 API Token</h2>
      <button type="button" className="btn btn-primary" disabled={busy || entries.filter(entry => entry.id !== 'legacy').length >= 128} onClick={add}>新增 Token</button>
    </div>
    <p className="panel-description">默认 Token 和新增 Token 在这里统一管理。给不同客户端命名后，可在统计页按 Token 筛选用量。</p>
    <div className="inbound-token-list">
      {entries.map((entry, index) => {
        const status = entry.id === 'legacy' ? source.current?.secretStatus?.inboundAuthToken : source.current?.secretStatus?.inboundTokens?.[entry.id];
        const label = entry.name || `Token ${index + 1}`;
        const token = entry.token ?? revealed[entry.id] ?? '';
        const isEditing = Boolean(editing[entry.id]) || entry.token !== null;
        return <article className="inbound-token-row" key={entry.id} aria-label={label}>
          <div className="inbound-token-row-header">
            <div className="inbound-token-badges">
              {entry.id === 'legacy' && <span className="form-hint">默认 Token</span>}
              <StatusBadge kind={entry.enabled ? 'good' : 'muted'} text={entry.enabled ? '已启用' : '已停用'} />
              {!status?.configured && <span className="form-hint">待保存</span>}
            </div>
            <label className="inbound-token-toggle"><input type="checkbox" checked={entry.enabled} disabled={busy} onChange={event => change(entry.id, { enabled: event.target.checked })} />启用</label>
          </div>
          <div className="inbound-token-fields">
            <label className="form-group"><span className="form-label">名称</span><input className="form-input" aria-label={`${label} 名称`} value={entry.name} placeholder="例如：工作电脑" disabled={busy} onChange={event => change(entry.id, { name: event.target.value })} /></label>
            <label className="form-group"><span className="form-label">Token</span><input className="form-input inbound-token-value" aria-label={`${label} 凭据`} type={visible[entry.id] ? 'text' : 'password'} autoComplete="off" spellCheck={false} value={token} readOnly={!isEditing} disabled={busy} placeholder={status?.configured ? `已保存 · 尾号 ${status.last4 || '****'}` : '输入 Token'} onChange={event => change(entry.id, { token: event.target.value })} /></label>
          </div>
          <div className="inbound-token-row-footer">
            <span className="form-hint">{entry.token !== null ? '新凭据将在保存后生效' : '改名不会更换凭据或拆分历史统计'}</span>
            <div className="inbound-token-actions">
              <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => visible[entry.id] ? hideCredential(entry.id) : useCredential(entry, 'show')}>{visible[entry.id] ? '隐藏' : '显示'}</button>
              <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => useCredential(entry, 'copy')}>复制</button>
              <button type="button" className="btn btn-secondary" disabled={busy || isEditing} onClick={() => useCredential(entry, 'edit')}>修改 Token</button>
              <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => change(entry.id, { token: randomToken() })}>生成新凭据</button>
              <button type="button" className="btn btn-danger" disabled={busy} aria-label={`删除 ${label}`} onClick={() => { conceal(); setEntries(previous => previous.filter(item => item.id !== entry.id)); }}>删除</button>
            </div>
          </div>
        </article>;
      })}
    </div>
    {!entries.length && <p className="form-hint">暂无入站 Token，点击“新增 Token”开始配置。</p>}
    <p className="form-hint">明文默认隐藏，点击显示后 60 秒自动隐藏。全部停用会拒绝请求；删除保留历史统计。</p>
    {error && <p className="inbound-token-error" role="alert">{error}</p>}
    <div className="panel-actions">
      <button type="button" className="btn btn-primary" disabled={busy || !dirty} aria-busy={Boolean(pending.save)} onClick={save}>{pending.save ? '保存中…' : '保存修改'}</button>
      <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => { if (!dirty || confirm('放弃未保存的修改并重新载入？')) load(configDoc); }}>重新载入</button>
      {dirty && <span className="form-hint">有未保存的修改</span>}
    </div>
  </section>;
}
