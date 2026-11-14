import { useEffect, useState } from 'react';
import { Activity, RefreshCw } from 'lucide-react';
import { Button } from './ui/button.jsx';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from './ui/card.jsx';
import { cn } from '../lib/utils.js';
import { api } from '../services/api.js';
import { CLIENT_KIND_LABELS, formatTimestamp } from '../utils/helpers.js';

const clients = Object.entries(CLIENT_KIND_LABELS);

export function ClientConnectionDiagnostics() {
  const [refresh, setRefresh] = useState(0);
  const [observation, setObservation] = useState({ state: 'loading', events: [] });

  useEffect(() => {
    const controller = new AbortController();
    setObservation({ state: 'loading', events: [] });
    api.getRuntimeEventPage({ kind: 'client', pageSize: 100, signal: controller.signal }).then((page) => {
      if (controller.signal.aborted) return;
      if (!Array.isArray(page?.events)) throw new Error('客户端事件列表不可用');
      setObservation({ state: 'loaded', events: page.events });
    }).catch(() => {
      if (!controller.signal.aborted) setObservation({ state: 'error', events: [] });
    });
    return () => controller.abort();
  }, [refresh]);

  return (
    <Card aria-label="客户端连接诊断">
      <CardHeader className="border-b border-border/60 pb-4">
        <div className="flex items-center gap-2">
          <Activity className="size-4 text-primary" />
          <CardTitle>客户端连接诊断</CardTitle>
        </div>
        <CardDescription>
          下表依据最近 100 条已完成客户端请求。未见请求表示未知，不能据此判断客户端绕过代理；客户端身份绑定在安全页配置。
        </CardDescription>
        <CardAction>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={observation.state === 'loading'}
            onClick={() => setRefresh((value) => value + 1)}
          >
            <RefreshCw className={cn('size-3.5', observation.state === 'loading' && 'animate-spin')} />
            <span>{observation.state === 'loading' ? '刷新中…' : '刷新请求观察'}</span>
          </Button>
        </CardAction>
      </CardHeader>

      <CardContent>
        <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {clients.map(([kind, label]) => {
            const event = observation.events.find((e) => e.kind === 'client' && e.clientKind === kind);
            const missing = observation.state === 'loading'
              ? '正在读取请求记录…'
              : observation.state === 'error'
                ? '读取失败，暂无法判断'
                : '最近窗口未见请求';
            return (
              <div
                key={kind}
                className={cn(
                  'rounded-lg border px-3.5 py-3 transition-colors',
                  event ? 'border-status-good/35 bg-status-good/5' : 'border-border/75 bg-muted/20',
                )}
              >
                <dt className="flex items-center gap-2 text-xs font-semibold text-foreground">
                  <span
                    className={cn('size-2 rounded-full', event ? 'bg-status-good' : 'bg-muted-foreground/45')}
                    aria-hidden="true"
                  />
                  <span>{label}</span>
                </dt>
                <dd className="mt-1.5 font-mono text-[11px] text-muted-foreground">
                  {event ? `已观察到请求 · ${formatTimestamp(event.timestamp)}` : missing}
                </dd>
              </div>
            );
          })}
        </dl>
      </CardContent>
    </Card>
  );
}
