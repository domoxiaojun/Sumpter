import { useState } from 'react';
import { USER_AGENT_PROTOCOLS } from '../utils/userAgent.js';
import { QuickToggle } from './QuickToggle.jsx';

export function UserAgentEditor({ initialValue, onChange }) {
  const [draft, setDraft] = useState(initialValue);
  const update = (key, field, value) => {
    const next = { ...draft, [key]: { ...draft[key], [field]: value } };
    setDraft(next);
    onChange(next);
  };
  return (
    <fieldset className="ua-editor">
      <legend className="form-label">上游 User-Agent（按协议）</legend>
      {USER_AGENT_PROTOCOLS.map(({ key, label, forceClient }) => {
        const forced = forceClient && draft[key]?.forceClient === true;
        return (
          <div className="ua-rule" key={key}>
            <label className="form-label" htmlFor={`ua-${key}`}>{label}</label>
            <select className="form-select" aria-label={`${label} UA 模式`} value={draft[key]?.mode || 'auto'} disabled={forced} onChange={(event) => update(key, 'mode', event.target.value)}>
              <option value="auto">自动</option>
              <option value="override">强覆盖</option>
            </select>
            <input id={`ua-${key}`} className="form-input ua-value" value={draft[key]?.value || ''} disabled={forced} placeholder={forced ? '已由官方客户端身份决定' : '自动留空：沿用现有行为；强覆盖留空：默认 UA'} onChange={(event) => update(key, 'value', event.target.value)} />
            {forceClient && (
              <div className="ua-force">
                <QuickToggle checked={forced} onChange={(value) => update(key, 'forceClient', value)} label={forceClient} ariaLabel={`切换${forceClient}`} />
              </div>
            )}
          </div>
        );
      })}
      <p className="form-hint">自动：保留客户端非空 UA，缺失时补填写值。强覆盖：始终替换。最多 512 字节，不能包含换行或控制字符。</p>
      <p className="form-hint">强制 Claude Code：Anthropic 出站对齐 Claude Code 的 UA、鉴权、会话/设备身份、system 前缀、消息块与缓存标记。强制 Codex：仅 Responses 出站对齐 Codex 的 UA、originator、会话头与 store:false。均不伪造工具桩。</p>
      <p className="form-hint">获取模型：固定协议使用对应 UA；自动协议依次尝试各协议的不同 UA，总超时 12 秒。</p>
    </fieldset>
  );
}
