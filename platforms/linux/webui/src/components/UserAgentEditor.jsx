import { useState } from 'react';
import { USER_AGENT_PROTOCOLS } from '../utils/userAgent.js';
import { Input } from './ui/input.jsx';
import { Label } from './ui/label.jsx';

const SELECT_CLS = 'h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs text-foreground shadow-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50';

export function UserAgentEditor({ initialValue, onChange }) {
  const [draft, setDraft] = useState(initialValue);
  const update = (key, field, value) => {
    const next = { ...draft, [key]: { ...draft[key], [field]: value } };
    setDraft(next);
    onChange(next);
  };
  return (
    <fieldset className="space-y-2.5 rounded-xl border bg-muted/20 p-3.5">
      <legend className="px-1.5 text-xs font-semibold text-foreground">
        上游 User-Agent（按协议）
      </legend>
      <div className="space-y-2">
        {USER_AGENT_PROTOCOLS.map(({ key, label }) => (
          <div className="grid gap-2 sm:grid-cols-[120px_110px_minmax(0,1fr)] sm:items-center" key={key}>
            <Label htmlFor={`ua-${key}`} className="text-xs font-medium text-foreground">
              {label}
            </Label>
            <select
              className={SELECT_CLS}
              aria-label={`${label} UA 模式`}
              value={draft[key]?.mode || 'auto'}
              onChange={(event) => update(key, 'mode', event.target.value)}
            >
              <option value="auto">自动</option>
              <option value="override">强覆盖</option>
            </select>
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
      <p className="text-[11px] leading-relaxed text-muted-foreground">
        自动：保留客户端非空 UA，缺失时补填写值。强覆盖：始终替换。最多 512 字节，不能包含换行或控制字符。
      </p>
      <p className="text-[11px] leading-relaxed text-muted-foreground">
        获取模型：固定协议使用对应 UA；自动协议依次尝试各协议的不同 UA，总超时 12 秒。
      </p>
    </fieldset>
  );
}
