import { useEffect, useRef, useState } from 'react';
import { ChevronRight, Fingerprint, RotateCcw, Save } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import { usePendingActions } from '../hooks/usePendingActions.js';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { CLIENT_CATALOG, CLIENT_VARIANTS } from '../utils/clientRegistry.js';
import { inboundTokenDraft } from '../utils/inboundTokens.js';
import {
  parseIdentityAliases,
  parseRelayEndpoints,
  identityAliasesText,
  patchIdentityBinding,
  validateIdentityBindings,
} from '../utils/clientIdentity.js';
import { Badge } from './ui/badge.jsx';
import { Button } from './ui/button.jsx';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from './ui/card.jsx';
import { Input } from './ui/input.jsx';
import { Label } from './ui/label.jsx';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from './ui/table.jsx';

const SELECT_CLS = 'flex h-9 w-full rounded-md border border-input bg-background/60 px-2.5 text-sm text-foreground shadow-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 disabled:cursor-not-allowed disabled:opacity-50';
const TEXTAREA_CLS = 'flex min-h-[72px] w-full rounded-md border border-input bg-background/60 px-3 py-2 font-mono text-xs text-foreground shadow-xs placeholder:text-muted-foreground/70 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 disabled:cursor-not-allowed disabled:opacity-50';

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
  const patch = (id, field, value) => setBindings((previous) => patchIdentityBinding(previous, id, field, value));

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
    <Card aria-labelledby="client-identity-title">
      <CardHeader className="border-b border-border/60 pb-4">
        <div className="flex items-center gap-2">
          <Fingerprint className="size-4 text-primary" />
          <CardTitle id="client-identity-title">客户端识别</CardTitle>
        </div>
        <CardDescription>
          通常保持自动识别即可。仅当客户端缺少产品标记时，为它使用的独立 Token 填写兜底绑定；多个客户端共用 Token 时请留空。
        </CardDescription>
      </CardHeader>

      <CardContent className="space-y-4">
        <p id="client-identity-product-hint" className="text-xs leading-relaxed text-muted-foreground">
          客户端标识可从建议中选择。自定义标识（如 my-app）仅补充产品标签，不会新增识别规则，也不会补出项目、会话或主代理身份。原生标记始终优先。
        </p>

        <datalist id="client-identity-products">
          {CLIENT_CATALOG.map((client) => (
            <option key={client.id} value={client.id}>{client.name}</option>
          ))}
        </datalist>

        <div className="space-y-3">
          {tokens.map((token) => {
            const binding = bindings[token.id] || {};
            const bound = Boolean(binding.client);
            return (
              <fieldset
                key={token.id}
                className="space-y-3 rounded-lg border border-border/80 bg-muted/20 p-4 disabled:opacity-60"
                disabled={busy}
              >
                <legend className="px-1.5 text-xs font-semibold text-foreground">
                  {token.name}{!token.enabled && '（已停用）'}
                </legend>

                <div className="flex flex-wrap items-center justify-between gap-2">
                  <Badge variant={bound ? 'default' : 'secondary'}>
                    {bound ? '仅补足请求中缺少的身份' : '自动识别 · 无人工绑定'}
                  </Badge>
                  {bound && (
                    <Button type="button" variant="ghost" size="xs" onClick={() => patch(token.id, 'client', '')}>
                      <RotateCcw className="size-3" />
                      <span>恢复自动识别</span>
                    </Button>
                  )}
                </div>

                <div className="grid gap-3 md:grid-cols-2">
                  <div className="space-y-1.5">
                    <Label htmlFor={`client-identity-client-${token.id}`}>客户端标识（可选）</Label>
                    <Input
                      id={`client-identity-client-${token.id}`}
                      list="client-identity-products"
                      value={binding.client || ''}
                      placeholder="自动识别（留空）"
                      aria-describedby="client-identity-product-hint"
                      autoCapitalize="none"
                      spellCheck={false}
                      onChange={(e) => patch(token.id, 'client', e.target.value)}
                    />
                  </div>
                  <div className="space-y-1.5">
                    <Label htmlFor={`client-identity-variant-select-${token.id}`}>客户端形态（可选）</Label>
                    <select
                      id={`client-identity-variant-select-${token.id}`}
                      className={SELECT_CLS}
                      value={binding.variant || ''}
                      disabled={!bound}
                      aria-describedby={`client-identity-variant-${token.id}`}
                      onChange={(e) => patch(token.id, 'variant', e.target.value)}
                    >
                      <option value="">自动识别</option>
                      {CLIENT_VARIANTS.map((variant) => (
                        <option key={variant} value={variant}>{variant}</option>
                      ))}
                    </select>
                    <p id={`client-identity-variant-${token.id}`} className="text-[11px] leading-relaxed text-muted-foreground">
                      {bound ? '例如 cli / desktop；仅作身份记录，不选择上游入口。' : '填写客户端标识后可选；这不是代理上游入口。'}
                    </p>
                  </div>
                </div>

                <details className="group rounded-md border border-border/70 bg-background/40">
                  <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 text-xs font-medium text-foreground/90 select-none hover:bg-muted/40">
                    <ChevronRight className="size-3.5 text-muted-foreground transition-transform group-open:rotate-90" />
                    <span>受信中继设置{binding.relaySource ? '（已配置）' : '（普通客户端无需设置）'}</span>
                  </summary>
                  <div className="space-y-1.5 border-t border-border/60 p-3">
                    <Label htmlFor={`client-identity-relay-input-${token.id}`}>受信中继来源（可选）</Label>
                    <Input
                      id={`client-identity-relay-input-${token.id}`}
                      value={binding.relaySource || ''}
                      placeholder="例如 office"
                      disabled={!bound}
                      aria-describedby={`client-identity-relay-${token.id}`}
                      autoCapitalize="none"
                      spellCheck={false}
                      onChange={(e) => patch(token.id, 'relaySource', e.target.value)}
                    />
                    <p id={`client-identity-relay-${token.id}`} className="text-[11px] leading-relaxed text-muted-foreground">
                      仅用于另一台 Sumpter 的专用 Token，先填写客户端标识（如 sumpter）。持有此 Token 的发送方可声明原始客户端，来源须与发送方一致；请勿与普通客户端共用。
                    </p>
                  </div>
                </details>
              </fieldset>
            );
          })}

          {!tokens.length && (
            <p className="rounded-lg border border-dashed border-border/80 bg-muted/15 px-4 py-4 text-xs text-muted-foreground">
              尚未配置入站 Token。客户端仍按请求中的原生标记识别；需要兜底绑定时，先在上方添加独立 Token。
            </p>
          )}
        </div>

        <details className="group rounded-lg border border-border/80 bg-muted/15">
          <summary className="flex cursor-pointer list-none items-center gap-2 px-3.5 py-2.5 text-xs font-semibold text-foreground select-none hover:bg-muted/35">
            <ChevronRight className="size-3.5 text-muted-foreground transition-transform group-open:rotate-90" />
            <span>高级设置：统计映射、UA 别名与中继目标</span>
          </summary>
          <div className="space-y-3 border-t border-border/60 p-3.5">
            <div className="space-y-1.5">
              <Label htmlFor="client-identity-reporting-aliases">统计归属映射</Label>
              <textarea
                id="client-identity-reporting-aliases"
                className={TEXTAREA_CLS}
                rows={3}
                value={aliases}
                disabled={busy}
                onChange={(e) => setAliases(e.target.value)}
                placeholder="my-app=codex"
                aria-describedby="client-identity-reporting-hint"
                spellCheck={false}
              />
              <p id="client-identity-reporting-hint" className="text-[11px] leading-relaxed text-muted-foreground">
                每行 产品标识=客户端ID。例如将 my-app 计入 Codex。仅影响查询时的统计分组与筛选；删除可恢复，历史原始身份、权限和路由保持不变。
              </p>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="client-identity-ua-aliases">产品 UA 精确别名</Label>
              <textarea
                id="client-identity-ua-aliases"
                className={TEXTAREA_CLS}
                rows={3}
                value={uaAliases}
                disabled={busy}
                onChange={(e) => setUaAliases(e.target.value)}
                placeholder="custom-cli=open_code"
                aria-describedby="client-identity-ua-hint"
                spellCheck={false}
              />
              <p id="client-identity-ua-hint" className="text-[11px] leading-relaxed text-muted-foreground">
                每行 UA产品标识=客户端ID。仅匹配完整产品名，如 custom-cli/1.0 的 custom-cli；用于后续请求，不支持正则，也不覆盖已识别的原生客户端。
              </p>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="client-identity-relay-endpoints">中继目标</Label>
              <textarea
                id="client-identity-relay-endpoints"
                className={TEXTAREA_CLS}
                rows={2}
                value={relays}
                disabled={busy}
                onChange={(e) => setRelays(e.target.value)}
                placeholder="remote-endpoint=office"
                aria-describedby="client-identity-endpoints-hint"
                spellCheck={false}
              />
              <p id="client-identity-endpoints-hint" className="text-[11px] leading-relaxed text-muted-foreground">
                每行 入口ID=来源标识。仅填写另一台 Sumpter 的上游入口，并与接收方“受信中继来源”一致；普通供应商入口留空。
              </p>
            </div>
          </div>
        </details>

        <details className="group rounded-lg border border-border/80 bg-muted/15">
          <summary className="flex cursor-pointer list-none items-center gap-2 px-3.5 py-2.5 text-xs font-semibold text-foreground select-none hover:bg-muted/35">
            <ChevronRight className="size-3.5 text-muted-foreground transition-transform group-open:rotate-90" />
            <span>原生识别能力目录</span>
          </summary>
          <div className="border-t border-border/60">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>客户端 / ID</TableHead>
                  <TableHead>原生会话</TableHead>
                  <TableHead>原生角色</TableHead>
                  <TableHead>原生用途</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {CLIENT_CATALOG.map((client) => (
                  <TableRow key={client.id}>
                    <TableCell className="font-mono text-xs font-medium">{client.name} · {client.id}</TableCell>
                    <TableCell className="text-xs text-muted-foreground">{client.nativeSessionHeaders.length ? '支持' : '按请求证据'}</TableCell>
                    <TableCell className="text-xs text-muted-foreground">{client.nativeRoles ? '支持' : '无专用标记'}</TableCell>
                    <TableCell className="text-xs text-muted-foreground">{client.nativeTasks ? '支持' : '无专用标记'}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        </details>

        {error && (
          <p className="rounded-lg border border-status-critical/35 bg-status-critical/10 px-3 py-2 text-xs font-medium text-status-critical" role="alert">
            {error}
          </p>
        )}
      </CardContent>

      <CardFooter className="flex flex-wrap items-center gap-2 border-t border-border/60 pt-4">
        <Button type="button" size="sm" disabled={!dirty || busy} onClick={save}>
          <Save className="size-3.5" />
          <span>{busy ? '保存中…' : '保存客户端识别'}</span>
        </Button>
        {error && (
          <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={() => load(configDoc)}>
            <RotateCcw className="size-3.5" />
            <span>重新载入并放弃修改</span>
          </Button>
        )}
        {dirty && <span className="text-xs font-medium text-status-warning">有未保存的修改</span>}
      </CardFooter>
    </Card>
  );
}
