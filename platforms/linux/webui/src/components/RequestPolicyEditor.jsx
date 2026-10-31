import { useState } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { DEFAULT_LIMITS, requestPolicy, validateRequestPolicy } from '../utils/requestPolicy.js';

export function RequestPolicyEditor() {
  const { config, configDoc, saveConfig, closeModal, addToast } = useAppControls();
  const [draft, setDraft] = useState(() => requestPolicy(config.requestPolicy));
  const [sourceDocument] = useState(() => configDoc);
  const [endpointID, setEndpointID] = useState('');
  const [model, setModel] = useState('*');
  const [protocol, setProtocol] = useState('all');
  const [saving, setSaving] = useState(false);
  const change = (edit) => setDraft(prior => { const next = structuredClone(prior); edit(next); return next; });
  const endpoint = draft.endpoints[endpointID] || { limits: DEFAULT_LIMITS, models: {} };
  const ensureEndpoint = (next) => next.endpoints[endpointID] ||= { limits: { ...DEFAULT_LIMITS }, models: {} };
  const profiles = protocol === 'all' ? endpoint.models : endpoint.protocolModels?.[protocol] || {};
  const ensureProfiles = next => {
    const p = ensureEndpoint(next);
    if (protocol === 'all') return p.models;
    p.protocolModels ||= {}; p.protocolModels[protocol] ||= {}; return p.protocolModels[protocol];
  };
  const number = (label, value, onChange, min, max) => <label className="request-policy-field">{label}<input className="form-input" type="number" min={min} max={max} step="1" value={value} onChange={event => onChange(event.target.value === '' ? '' : Number(event.target.value))} /></label>;
  const limits = (value, update) => <div className="request-policy-grid">
    {number('并发上限（0 = 不限）', value.maxConcurrent, v => update('maxConcurrent', v), 0, 10000)}
    {number('排队数量（0 = 不排队）', value.maxQueued, v => update('maxQueued', v), 0, 10000)}
    {number('排队时限（秒）', value.queueTimeoutSeconds, v => update('queueTimeoutSeconds', v), 1, 300)}
    {number('每分钟上游尝试（0 = 不限）', value.requestsPerMinute, v => update('requestsPerMinute', v), 0, 1000000)}
  </div>;
  const save = async event => {
    event.preventDefault();
    try { validateRequestPolicy(draft); setSaving(true); await saveConfig({ ...structuredClone(sourceDocument.config), requestPolicy: draft }, {}, sourceDocument); closeModal(); addToast('请求策略已保存', 'success'); }
    catch (error) { addToast(error.message, 'error'); }
    finally { setSaving(false); }
  };
  return <form onSubmit={save} className="request-policy-form">
    <p>适用于所有客户端的 HTTP/SSE 请求。并发名额保持到上游响应结束；重试也计入每分钟额度。WebSocket 不按连接数等同为请求数。</p>
    <h3>全局请求限制</h3>
    {limits(draft.global, (k, v) => change(next => { next.global[k] = v; }))}
    {number('一次请求最多尝试（0 = 沿用原有重试限制）', draft.maxTotalAttempts, v => change(next => { next.maxTotalAttempts = v; }), 0, 10000)}
    {number('代理响应等待上限（秒，0 = 不限）', draft.responseWaitTimeoutSeconds, v => change(next => { next.responseWaitTimeoutSeconds = v; }), 0, 86400)}
    <p>从代理开始转发到向客户端交付响应，包含排队、所有入口尝试、退避和缓冲；到时返回 504。0 保持原重试策略；已开始交付的长回答不受影响。</p>
    <button type="button" className="btn btn-secondary" onClick={() => change(next => { next.maxTotalAttempts = 6; next.global = { ...next.global, maxConcurrent: 8, maxQueued: 16, queueTimeoutSeconds: 15 }; })}>填入交互预设</button>
    <h3>Responses 流保护</h3>
    <label className="request-policy-field">流式策略<select className="form-select" value={draft.streamProtection} onChange={e => change(next => { next.streamProtection = e.target.value; })}>
      <option value="complete">完整缓冲：支持中途断流恢复</option><option value="prelude">前奏保护：更早开始输出</option>
    </select></label>
    <p>仅在原有流错误重试开启时生效。前奏模式提交业务输出后不再透明重放；达到任一缓冲额度时恢复透传，并在请求详情记录原因。</p>
    <div className="request-policy-grid">
      {number('单流缓冲（MiB）', draft.streamBufferMib, v => change(next => { next.streamBufferMib = v; }), 1, 256)}
      {number('所有流总缓冲（MiB）', draft.maxBufferedMib, v => change(next => { next.maxBufferedMib = v; }), 1, 4096)}
      {number('单流响应块上限', draft.streamBufferChunks, v => change(next => { next.streamBufferChunks = v; }), 16, 65536)}
      {number('缓冲时限（秒，0 = 不限）', draft.streamBufferSeconds, v => change(next => { next.streamBufferSeconds = v; }), 0, 3600)}
    </div>
    <h3>入口限制与兼容声明</h3>
    <label className="request-policy-field">入口<select className="form-select" value={endpointID} onChange={e => setEndpointID(e.target.value)}><option value="">选择入口</option>{config.endpoints.map(e => <option value={e.id} key={e.id}>{e.name || e.id}</option>)}</select></label>
    {endpointID && <>
      {limits(endpoint.limits, (k, v) => change(next => { ensureEndpoint(next).limits[k] = v; }))}
      <label className="request-policy-field">上游模型（精确 ID 或 *）<input className="form-input" value={model} onChange={e => setModel(e.target.value)} /></label>
      <label className="request-policy-field">兼容声明适用协议<select className="form-select" value={protocol} onChange={e => setProtocol(e.target.value)}><option value="all">所有协议的默认声明</option><option value="anthropic">Anthropic Messages</option><option value="openai">Chat Completions</option><option value="openai-responses">Responses</option><option value="gemini">Gemini</option></select></label>
      <p>未知保持原有转发；明确不支持时跳过该入口，全部候选都不支持则拒绝请求。不会静默删除工具或改写正文。</p>
      <div className="request-policy-grid">{[['strictTools', '严格工具'], ['grammarTools', '语法工具'], ['developerRole', 'Developer 角色'], ['structuredOutput', '结构化输出']].map(([key, label]) => <label className="request-policy-field" key={key}>{label}<select className="form-select" disabled={!model.trim()} value={profiles[model.trim()]?.[key] == null ? 'unknown' : String(profiles[model.trim()][key])} onChange={e => change(next => { const p = ensureProfiles(next); p[model.trim()] ||= {}; p[model.trim()][key] = e.target.value === 'unknown' ? null : e.target.value === 'true'; })}><option value="unknown">未知 / 保持原样</option><option value="true">支持</option><option value="false">不支持</option></select></label>)}</div>
    </>}
    <div className="modal-actions"><button type="button" className="btn btn-secondary" onClick={closeModal}>取消</button><button className="btn btn-primary" disabled={saving}>{saving ? '保存中…' : '保存请求策略'}</button></div>
  </form>;
}
