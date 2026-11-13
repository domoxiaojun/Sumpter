import { useEffect, useState, useRef } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { usePendingActions } from '../hooks/usePendingActions.js';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { CLIENT_CATALOG, CLIENT_VARIANTS } from '../utils/clientRegistry.js';
import { inboundTokenDraft } from '../utils/inboundTokens.js';
import {
  parseIdentityAliases, parseRelayEndpoints, identityAliasesText,
  patchIdentityBinding, validateIdentityBindings,
} from '../utils/clientIdentity.js';

export function ClientIdentityPanel() {
  const { configDoc, saveConfig, addToast } = useAppControls();
  const source = useRef(configDoc);
  const [bindings, setBindings] = useState({});
  const [aliases, setAliases] = useState('');
  const [uaAliases, setUaAliases] = useState('');
  const [relays, setRelays] = useState('');
  const [baseline, setBaseline] = useState('');
  const [error, setError] = useState('');
  const { pending, run } = usePendingActions();
  const busy = Boolean(pending.save);
  const snapshot = JSON.stringify([bindings, aliases, uaAliases, relays]);
  const dirty = baseline !== '' && snapshot !== baseline;
  useUnsavedChanges(dirty, busy);

  const load = (document) => {
    source.current = document;
    const value = document?.config?.listener?.clientIdentity || {};
    const nextBindings = value.tokenBindings || {};
    const nextAliases = identityAliasesText(value.reportingAliases);
    const nextUA = identityAliasesText(value.uaAliases);
    const nextRelays = identityAliasesText(value.relayEndpoints);
    setBindings(nextBindings);
    setAliases(nextAliases);
    setUaAliases(nextUA);
    setRelays(nextRelays);
    setBaseline(JSON.stringify([nextBindings, nextAliases, nextUA, nextRelays]));
    setError('');
  };
  useEffect(() => {
    if (!dirty && !busy) load(configDoc);
  }, [configDoc, dirty, busy]);

  const tokens = inboundTokenDraft(configDoc);
  const patch = (id, field, value) => setBindings(previous => patchIdentityBinding(previous, id, field, value));
  const save = () => run('save', async () => {
    setError('');
    try {
      validateIdentityBindings(bindings);
      const config = structuredClone(source.current.config);
      config.listener.clientIdentity = {
        tokenBindings: bindings,
        reportingAliases: parseIdentityAliases(aliases),
        uaAliases: parseIdentityAliases(uaAliases),
        relayEndpoints: parseRelayEndpoints(relays),
      };
      const saved = await saveConfig(config, {}, source.current);
      load(saved);
      addToast('客户端识别设置已保存', 'success');
    } catch (e) {
      setError(e.message);
    }
  });

  return (
    <section className="glass-panel panel-padded-stack client-identity-panel" aria-labelledby="client-identity-title">
      <h2 id="client-identity-title" className="panel-title">客户端识别</h2>
      <p className="panel-description">通常保持自动识别即可。仅当客户端缺少产品标记时，为它使用的独立 Token 填写兜底绑定；多个客户端共用 Token 时请留空。</p>
      <p id="client-identity-product-hint" className="form-hint">客户端标识可从建议中选择。自定义标识（如 my-app）仅补充产品标签，不会新增识别规则，也不会补出项目、会话或主代理身份。原生标记始终优先。</p>
      <datalist id="client-identity-products">
        {CLIENT_CATALOG.map(client => <option key={client.id} value={client.id}>{client.name}</option>)}
      </datalist>
      <div className="client-identity-token-list">
        {tokens.map(token => {
          const binding = bindings[token.id] || {};
          const bound = Boolean(binding.client);
          return (
            <fieldset key={token.id} className="client-identity-token" disabled={busy}>
              <legend>{token.name}{!token.enabled && '（已停用）'}</legend>
              <div className="client-identity-token-heading">
                <span className="form-hint">{bound ? '仅补足请求中缺少的身份' : '自动识别 · 无人工绑定'}</span>
                {bound && <button type="button" className="btn btn-ghost btn-sm" onClick={() => patch(token.id, 'client', '')}>恢复自动识别</button>}
              </div>
              <div className="client-identity-fields">
                <label className="form-group">
                  <span className="form-label">客户端标识（可选）</span>
                  <input className="form-input" list="client-identity-products" value={binding.client || ''} placeholder="自动识别（留空）" aria-describedby="client-identity-product-hint" autoCapitalize="none" spellCheck={false} onChange={e => patch(token.id, 'client', e.target.value)} />
                </label>
                <label className="form-group">
                  <span className="form-label">客户端形态（可选）</span>
                  <select className="form-select" value={binding.variant || ''} disabled={!bound} aria-describedby={`client-identity-variant-${token.id}`} onChange={e => patch(token.id, 'variant', e.target.value)}>
                    <option value="">自动识别</option>
                    {CLIENT_VARIANTS.map(variant => <option key={variant} value={variant}>{variant}</option>)}
                  </select>
                  <span id={`client-identity-variant-${token.id}`} className="form-hint">{bound ? '例如 cli / desktop；仅作身份记录，不选择上游入口。' : '填写客户端标识后可选；这不是代理上游入口。'}</span>
                </label>
              </div>
              <details className="client-identity-relay">
                <summary>受信中继设置{binding.relaySource ? '（已配置）' : '（普通客户端无需设置）'}</summary>
                <label className="form-group">
                  <span className="form-label">受信中继来源（可选）</span>
                  <input className="form-input" value={binding.relaySource || ''} placeholder="例如 office" disabled={!bound} aria-describedby={`client-identity-relay-${token.id}`} autoCapitalize="none" spellCheck={false} onChange={e => patch(token.id, 'relaySource', e.target.value)} />
                  <span id={`client-identity-relay-${token.id}`} className="form-hint">仅用于另一台 Sumpter 的专用 Token，先填写客户端标识（如 sumpter）。持有此 Token 的发送方可声明原始客户端，来源须与发送方一致；请勿与普通客户端共用。</span>
                </label>
              </details>
            </fieldset>
          );
        })}
        {!tokens.length && <p className="panel-description">尚未配置入站 Token。客户端仍按请求中的原生标记识别；需要兜底绑定时，先在上方添加独立 Token。</p>}
      </div>
      <details className="client-identity-advanced">
        <summary>高级设置：统计映射、UA 别名与中继目标</summary>
        <div className="client-identity-advanced-body">
          <label className="form-group">
            <span className="form-label">统计归属映射</span>
            <textarea className="form-textarea" rows={3} value={aliases} disabled={busy} onChange={e => setAliases(e.target.value)} placeholder="my-app=codex" aria-describedby="client-identity-reporting-hint" spellCheck={false} />
            <span id="client-identity-reporting-hint" className="form-hint">每行 产品标识=客户端ID。例如将 my-app 计入 Codex。仅影响查询时的统计分组与筛选；删除可恢复，历史原始身份、权限和路由保持不变。</span>
          </label>
          <label className="form-group">
            <span className="form-label">产品 UA 精确别名</span>
            <textarea className="form-textarea" rows={3} value={uaAliases} disabled={busy} onChange={e => setUaAliases(e.target.value)} placeholder="custom-cli=open_code" aria-describedby="client-identity-ua-hint" spellCheck={false} />
            <span id="client-identity-ua-hint" className="form-hint">每行 UA产品标识=客户端ID。仅匹配完整产品名，如 custom-cli/1.0 的 custom-cli；用于后续请求，不支持正则，也不覆盖已识别的原生客户端。</span>
          </label>
          <label className="form-group">
            <span className="form-label">中继目标</span>
            <textarea className="form-textarea" rows={2} value={relays} disabled={busy} onChange={e => setRelays(e.target.value)} placeholder="remote-endpoint=office" aria-describedby="client-identity-endpoints-hint" spellCheck={false} />
            <span id="client-identity-endpoints-hint" className="form-hint">每行 入口ID=来源标识。仅填写另一台 Sumpter 的上游入口，并与接收方“受信中继来源”一致；普通供应商入口留空。</span>
          </label>
        </div>
      </details>
      <details className="client-identity-advanced">
        <summary>原生识别能力目录</summary>
        <div className="client-identity-catalog table-container">
          <table className="data-table">
            <thead><tr><th>客户端 / ID</th><th>原生会话</th><th>原生角色</th><th>原生用途</th></tr></thead>
            <tbody>{CLIENT_CATALOG.map(client => <tr key={client.id}><td>{client.name} · {client.id}</td><td>{client.nativeSessionHeaders.length ? '支持' : '按请求证据'}</td><td>{client.nativeRoles ? '支持' : '无专用标记'}</td><td>{client.nativeTasks ? '支持' : '无专用标记'}</td></tr>)}</tbody>
          </table>
        </div>
      </details>
      {error && <p className="client-identity-error" role="alert">{error}</p>}
      <div className="panel-actions">
        <button type="button" className="btn btn-primary" disabled={!dirty || busy} onClick={save}>{busy ? '保存中…' : '保存客户端识别'}</button>
        {error && <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => load(configDoc)}>重新载入并放弃修改</button>}
        {dirty && <span className="form-hint">有未保存的修改</span>}
      </div>
    </section>
  );
}
