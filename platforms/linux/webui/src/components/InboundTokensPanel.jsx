import { useEffect, useRef, useState } from 'react';
import { Copy, Eye, EyeOff, KeyRound, Pencil, Plus, RefreshCw, RotateCcw, Save, Trash2 } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import { usePendingActions } from '../hooks/usePendingActions.js';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { api } from '../services/api.js';
import { copyTextToClipboard } from '../utils/clipboard.js';
import { buildInboundTokenWrite, inboundTokenDraft, randomToken } from '../utils/inboundTokens.js';
import { StatusBadge } from './StatusBadge.jsx';
import { Badge } from './ui/badge.jsx';
import { Button } from './ui/button.jsx';
import { Card, CardAction, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from './ui/card.jsx';
import { Input } from './ui/input.jsx';
import { Label } from './ui/label.jsx';

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
    setRevealed({});
    setVisible({});
    setEditing({});
  }

  function load(document) {
    conceal();
    source.current = document;
    const next = inboundTokenDraft(document);
    setEntries(next);
    setBaseline(JSON.stringify(next));
    setError('');
  }

  useEffect(() => {
    if (!dirty) load(configDoc);
    else conceal();
  }, [configDoc]);

  useEffect(() => () => {
    revision.current += 1;
    for (const timer of hideTimers.current.values()) window.clearTimeout(timer);
  }, []);

  useEffect(() => {
    const hide = () => {
      if (document.hidden) conceal();
    };
    document.addEventListener('visibilitychange', hide);
    return () => document.removeEventListener('visibilitychange', hide);
  }, []);

  function hideCredential(id) {
    window.clearTimeout(hideTimers.current.get(id));
    hideTimers.current.delete(id);
    setVisible((previous) => ({ ...previous, [id]: false }));
    setRevealed((previous) => {
      const next = { ...previous };
      delete next[id];
      return next;
    });
  }

  const change = (id, patch) => {
    setError('');
    setEntries((previous) => previous.map((entry) => (entry.id === id ? { ...entry, ...patch } : entry)));
  };

  const credential = async (entry) => {
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
        setRevealed((previous) => ({ ...previous, [entry.id]: token }));
        if (action === 'edit') {
          window.clearTimeout(hideTimers.current.get(entry.id));
          hideTimers.current.delete(entry.id);
          setVisible((previous) => ({ ...previous, [entry.id]: false }));
          setEditing((previous) => ({ ...previous, [entry.id]: true }));
        } else {
          setVisible((previous) => ({ ...previous, [entry.id]: true }));
          window.clearTimeout(hideTimers.current.get(entry.id));
          hideTimers.current.set(entry.id, window.setTimeout(() => hideCredential(entry.id), 60_000));
        }
      }
    } catch (failure) {
      if (currentRevision === revision.current) setError(failure.message);
    }
  });

  const save = () => run('save', async () => {
    setError('');
    try {
      const write = buildInboundTokenWrite(source.current, entries);
      if (write.opensAccess && !confirm('删除所有 Token 后将关闭入站认证。确认保存？')) return;
      const saved = await saveConfig(write.config, write.secrets, source.current);
      load(saved);
    } catch (failure) {
      setError(failure.message);
    }
  });

  const add = () => {
    const entry = { id: randomToken('token-'), name: '', enabled: true, token: randomToken() };
    setEntries((previous) => [...previous, entry]);
    setEditing((previous) => ({ ...previous, [entry.id]: true }));
  };

  return (
    <Card aria-labelledby="inbound-tokens-title">
      <CardHeader className="border-b border-border/60 pb-4">
        <div className="flex items-center gap-2">
          <KeyRound className="size-4 text-primary" />
          <CardTitle id="inbound-tokens-title">入站 API Token</CardTitle>
        </div>
        <CardDescription>
          默认 Token 和新增 Token 在这里统一管理。给不同客户端命名后，可在统计页按 Token 筛选用量。
        </CardDescription>
        <CardAction>
          <Button
            type="button"
            size="sm"
            disabled={busy || entries.filter((entry) => entry.id !== 'legacy').length >= 128}
            onClick={add}
          >
            <Plus className="size-3.5" />
            <span>新增 Token</span>
          </Button>
        </CardAction>
      </CardHeader>

      <CardContent className="space-y-3">
        <div className="space-y-3">
          {entries.map((entry, index) => {
            const status = entry.id === 'legacy'
              ? source.current?.secretStatus?.inboundAuthToken
              : source.current?.secretStatus?.inboundTokens?.[entry.id];
            const label = entry.name || `Token ${index + 1}`;
            const token = entry.token ?? revealed[entry.id] ?? '';
            const isEditing = Boolean(editing[entry.id]) || entry.token !== null;

            return (
              <article
                className="space-y-3 rounded-lg border border-border/80 bg-muted/25 p-4 transition-colors hover:border-border"
                key={entry.id}
                aria-label={label}
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex flex-wrap items-center gap-2">
                    {entry.id === 'legacy' && <Badge variant="outline">默认 Token</Badge>}
                    <StatusBadge kind={entry.enabled ? 'good' : 'muted'} text={entry.enabled ? '已启用' : '已停用'} />
                    {!status?.configured && <Badge variant="warning">待保存</Badge>}
                  </div>
                  <label className="inline-flex cursor-pointer items-center gap-2 text-xs font-medium text-foreground select-none">
                    <input
                      type="checkbox"
                      className="size-4 cursor-pointer rounded border-input accent-primary"
                      checked={entry.enabled}
                      disabled={busy}
                      onChange={(event) => change(entry.id, { enabled: event.target.checked })}
                    />
                    <span>启用</span>
                  </label>
                </div>

                <div className="grid gap-3 md:grid-cols-2">
                  <div className="space-y-1.5">
                    <Label htmlFor={`inbound-token-name-${entry.id}`}>名称</Label>
                    <Input
                      id={`inbound-token-name-${entry.id}`}
                      aria-label={`${label} 名称`}
                      value={entry.name}
                      placeholder="例如：工作电脑"
                      disabled={busy}
                      onChange={(event) => change(entry.id, { name: event.target.value })}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor={`inbound-token-value-${entry.id}`}>Token</Label>
                    <Input
                      id={`inbound-token-value-${entry.id}`}
                      className="font-mono text-xs"
                      aria-label={`${label} 凭据`}
                      type={visible[entry.id] ? 'text' : 'password'}
                      autoComplete="off"
                      spellCheck={false}
                      value={token}
                      readOnly={!isEditing}
                      disabled={busy}
                      placeholder={status?.configured ? `已保存 · 尾号 ${status.last4 || '****'}` : '输入 Token'}
                      onChange={(event) => change(entry.id, { token: event.target.value })}
                    />
                  </div>
                </div>

                <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
                  <span className="text-xs text-muted-foreground">
                    {entry.token !== null ? '新凭据将在保存后生效' : '改名不会更换凭据或拆分历史统计'}
                  </span>
                  <div className="flex flex-wrap items-center gap-1.5">
                    <Button
                      type="button"
                      variant="secondary"
                      size="xs"
                      disabled={busy}
                      onClick={() => (visible[entry.id] ? hideCredential(entry.id) : useCredential(entry, 'show'))}
                    >
                      {visible[entry.id] ? <EyeOff className="size-3" /> : <Eye className="size-3" />}
                      <span>{visible[entry.id] ? '隐藏' : '显示'}</span>
                    </Button>
                    <Button
                      type="button"
                      variant="secondary"
                      size="xs"
                      disabled={busy}
                      onClick={() => useCredential(entry, 'copy')}
                    >
                      <Copy className="size-3" />
                      <span>复制</span>
                    </Button>
                    <Button
                      type="button"
                      variant="secondary"
                      size="xs"
                      disabled={busy || isEditing}
                      onClick={() => useCredential(entry, 'edit')}
                    >
                      <Pencil className="size-3" />
                      <span>修改 Token</span>
                    </Button>
                    <Button
                      type="button"
                      variant="secondary"
                      size="xs"
                      disabled={busy}
                      onClick={() => change(entry.id, { token: randomToken() })}
                    >
                      <RefreshCw className="size-3" />
                      <span>生成新凭据</span>
                    </Button>
                    <Button
                      type="button"
                      variant="destructive"
                      size="xs"
                      disabled={busy}
                      aria-label={`删除 ${label}`}
                      onClick={() => {
                        conceal();
                        setEntries((previous) => previous.filter((item) => item.id !== entry.id));
                      }}
                    >
                      <Trash2 className="size-3" />
                      <span>删除</span>
                    </Button>
                  </div>
                </div>
              </article>
            );
          })}
        </div>

        {!entries.length && (
          <p className="rounded-lg border border-dashed border-border/80 bg-muted/15 px-4 py-5 text-center text-xs text-muted-foreground">
            暂无入站 Token，点击“新增 Token”开始配置。
          </p>
        )}

        <p className="text-xs text-muted-foreground">
          明文默认隐藏，点击显示后 60 秒自动隐藏。全部停用会拒绝请求；删除保留历史统计。
        </p>

        {error && (
          <p className="rounded-lg border border-status-critical/35 bg-status-critical/10 px-3 py-2 text-xs font-medium text-status-critical" role="alert">
            {error}
          </p>
        )}
      </CardContent>

      <CardFooter className="flex flex-wrap items-center gap-2 border-t border-border/60 pt-4">
        <Button type="button" size="sm" disabled={busy || !dirty} aria-busy={Boolean(pending.save)} onClick={save}>
          <Save className="size-3.5" />
          <span>{pending.save ? '保存中…' : '保存修改'}</span>
        </Button>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          disabled={busy}
          onClick={() => {
            if (!dirty || confirm('放弃未保存的修改并重新载入？')) load(configDoc);
          }}
        >
          <RotateCcw className="size-3.5" />
          <span>重新载入</span>
        </Button>
        {dirty && <span className="text-xs font-medium text-status-warning">有未保存的修改</span>}
      </CardFooter>
    </Card>
  );
}
