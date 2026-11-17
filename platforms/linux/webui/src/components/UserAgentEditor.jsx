import { useState } from 'react';
import { USER_AGENT_PROTOCOLS } from '../utils/userAgent.js';
import { Input } from './ui/input.jsx';
import { Label } from './ui/label.jsx';
import { NativeSelect } from './ui/native-select.jsx';

export function UserAgentEditor({ initialValue, onChange }) {
  const [draft, setDraft] = useState(initialValue);
  const update = (key, field, value) => {
    const next = { ...draft, [key]: { ...draft[key], [field]: value } };
    setDraft(next);
    onChange(next);
  };
  return (
    <fieldset className="space-y-3 rounded-2xl border border-border/80 bg-muted/25 p-4" aria-label="上游 User-Agent（按协议）">
      <div className="text-xs font-semibold text-foreground">
        上游 User-Agent（按协议）
      </div>
      <div className="space-y-2.5">
        {USER_AGENT_PROTOCOLS.map(({ key, label }) => (
          <div className="grid gap-2 sm:grid-cols-[120px_116px_minmax(0,1fr)] sm:items-center" key={key}>
            <Label htmlFor={`ua-${key}`} className="text-xs font-medium text-foreground">
              {label}
            </Label>
            <NativeSelect
              aria-label={`${label} UA 模式`}
              value={draft[key]?.mode || 'auto'}
              onChange={(event) => update(key, 'mode', event.target.value)}
            >
              <option value="auto">自动</option>
              <option value="override">强覆盖</option>
            </NativeSelect>
            <Input
              id={`ua-${key}`}
              className="h-9 font-mono text-xs"
              value={draft[key]?.value || ''}
              placeholder="自动留空：沿用现有行为；强覆盖留空：默认 UA"
              onChange={(event) => update(key, 'value', event.target.value)}
            />
          </div>
        ))}
      </div>
      <div className="space-y-1 text-[11px] leading-relaxed text-muted-foreground">
        <p>自动：保留客户端非空 UA，缺失时补填写值。强覆盖：始终替换。最多 512 字节，不能包含换行或控制字符。</p>
        <p>获取模型：固定协议使用对应 UA；自动协议依次尝试各协议的不同 UA，总超时 12 秒。</p>
      </div>
    </fieldset>
  );
}
