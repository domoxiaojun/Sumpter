import { useEffect, useState, useRef } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { CLIENT_CATALOG, CLIENT_VARIANTS } from '../utils/clientRegistry.js';
import { parseIdentityAliases, parseRelayEndpoints, identityAliasesText, validateIdentityBindings } from '../utils/clientIdentity.js';

export function ClientIdentityPanel() {
  const { configDoc, saveConfig, addToast }=useAppControls();
  const source=useRef(configDoc);
  const [bindings,setBindings]=useState({});
  const [aliases,setAliases]=useState('');
  const [uaAliases,setUaAliases]=useState('');
  const [relays,setRelays]=useState('');
  const [baseline,setBaseline]=useState('');
  const [busy,setBusy]=useState(false),[error,setError]=useState('');
  const snapshot=JSON.stringify([bindings,aliases,uaAliases,relays]);
  const dirty=baseline!==''&&snapshot!==baseline;
  useUnsavedChanges(dirty,busy);
  useEffect(()=>{if(dirty||busy)return;source.current=configDoc;const value=configDoc?.config?.listener?.clientIdentity||{};
    const b=value.tokenBindings||{},a=identityAliasesText(value.reportingAliases),u=identityAliasesText(value.uaAliases);
    const r=identityAliasesText(value.relayEndpoints);setBindings(b);setAliases(a);setUaAliases(u);setRelays(r);setBaseline(JSON.stringify([b,a,u,r]));},[configDoc,dirty,busy]);
  const tokens=[...(configDoc?.secretStatus?.inboundAuthToken?.configured?[{id:'legacy',name:configDoc.config.listener.authTokenName||'默认 Token'}]:[]),...(configDoc?.config?.listener?.authTokens||[])];
  const patch=(id,key,value)=>setBindings(previous=>{const next={...previous};const b={...(next[id]||{}),[key]:value};if(!value)delete b[key];if(!b.client)delete next[id];else Object.defineProperty(next,id,{value:b,enumerable:true,writable:true,configurable:true});return next;});
  const save=async()=>{setBusy(true);setError('');try{validateIdentityBindings(bindings);const config=structuredClone(source.current.config);
    config.listener.clientIdentity={tokenBindings:bindings,reportingAliases:parseIdentityAliases(aliases),uaAliases:parseIdentityAliases(uaAliases),relayEndpoints:parseRelayEndpoints(relays)};
    const saved=await saveConfig(config,{},source.current);source.current=saved;setBaseline(snapshot);addToast('客户端识别设置已保存','success');
  }catch(e){setError(e.message);}finally{setBusy(false);}};
  return <section className="glass-panel panel-padded-stack"><h2 className="panel-title">客户端识别</h2>
    <p className="panel-description">原生标记优先。为缺少产品标记的客户端绑定独立 Token；同一 Token 被多个客户端共用时请留空。绑定不会补出项目或会话。</p>
    <datalist id="client-identity-products">{CLIENT_CATALOG.map(c=><option key={c.id} value={c.id}>{c.name}</option>)}</datalist>
    {tokens.map(t=><fieldset key={t.id} disabled={busy}><legend>{t.name}</legend><div className="form-grid">
      <label className="form-group">客户端标识<input className="form-input" list="client-identity-products" value={bindings[t.id]?.client||''} placeholder="自动识别" onChange={e=>patch(t.id,'client',e.target.value)}/></label>
      <label className="form-group">入口<select className="form-input" value={bindings[t.id]?.variant||''} onChange={e=>patch(t.id,'variant',e.target.value)}>{['',...CLIENT_VARIANTS].map(v=><option key={v} value={v}>{v||'自动识别'}</option>)}</select></label>
      <label className="form-group">受信中继来源<input className="form-input" value={bindings[t.id]?.relaySource||''} placeholder="普通客户端留空" onChange={e=>patch(t.id,'relaySource',e.target.value)}/></label>
    </div></fieldset>)}
    <p className="form-hint">中继来源仅用于专用中继 Token；持有该 Token 的发送方可声明原始客户端。声明不会改变权限或路由。</p>
    <label className="form-group">统计归属映射<textarea className="form-input" rows={3} value={aliases} disabled={busy} onChange={e=>setAliases(e.target.value)} placeholder="my-bridge=codex"/><span className="form-hint">每行 产品标识=客户端ID。只改变统计分组，可随时删除；历史原始身份保持不变。</span></label>
    <label className="form-group">产品 UA 精确别名<textarea className="form-input" rows={3} value={uaAliases} disabled={busy} onChange={e=>setUaAliases(e.target.value)} placeholder="custom-cli=open_code"/></label>
    <label className="form-group">中继目标<textarea className="form-input" rows={2} value={relays} disabled={busy} onChange={e=>setRelays(e.target.value)} placeholder="remote-endpoint=home"/><span className="form-hint">每行 入口ID=来源标识。仅填写另一台 Sumpter 的入口。</span></label>
    <details><summary>原生识别能力目录</summary><table><thead><tr><th>客户端</th><th>原生会话</th><th>原生角色</th><th>原生用途</th></tr></thead><tbody>{CLIENT_CATALOG.map(c=><tr key={c.id}><td>{c.name} · {c.id}</td><td>{c.nativeSessionHeaders.length?'支持':'按请求证据'}</td><td>{c.nativeRoles?'支持':'无专用标记'}</td><td>{c.nativeTasks?'支持':'无专用标记'}</td></tr>)}</tbody></table></details>
    {error&&<p role="alert">{error}</p>}<button className="btn btn-primary" disabled={!dirty||busy} onClick={save}>{busy?'保存中…':'保存客户端识别'}</button>
  </section>;
}
