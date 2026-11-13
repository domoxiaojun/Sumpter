import { useEffect, useState } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { api } from '../services/api.js';
import { Icon } from '../utils/icons.jsx';
import { CLIENT_KIND_LABELS, formatTimestamp } from '../utils/helpers.js';

const clients = Object.entries(CLIENT_KIND_LABELS);

export function ClientConnectionDiagnostics() {
  const { addToast } = useAppControls();
  const [refresh, setRefresh] = useState(0);
  const [observation, setObservation] = useState({ state: 'loading', events: [] });
  useEffect(() => {
    const controller = new AbortController();
    setObservation({ state: 'loading', events: [] });
    api.getRuntimeEventPage({ kind: 'client', pageSize: 100, signal: controller.signal }).then(page => {
      if (controller.signal.aborted) return;
      if (!Array.isArray(page?.events)) throw new Error('客户端事件列表不可用');
      setObservation({ state: 'loaded', events: page.events });
    }).catch(() => {
      if (!controller.signal.aborted) setObservation({ state: 'error', events: [] });
    });
    return () => controller.abort();
  }, [refresh]);
  return (
    <section className="glass-panel panel-padded-stack" aria-label="客户端连接诊断">
      <div className="panel-header diagnostics-client-header">
        <div className="panel-title-block">
          <div className="panel-title">
            <Icon name="activity" size={18} style={{ color: 'var(--primary)' }} />
            <span>客户端连接诊断</span>
          </div>
          <p className="panel-description">
            下表依据最近 100 条已完成客户端请求。未见请求表示未知，不能据此判断客户端绕过代理；客户端身份绑定在安全页配置。
          </p>
        </div>
        <div className="panel-actions">
          <button
            type="button"
            className="btn btn-secondary btn-compact"
            disabled={observation.state === 'loading'}
            onClick={() => setRefresh((value) => value + 1)}
          >
            <Icon name="refresh" size={14} />
            <span>{observation.state === 'loading' ? '刷新中…' : '刷新请求观察'}</span>
          </button>
        </div>
      </div>

      <dl className="diagnostics-client-grid">
        {clients.map(([kind, label]) => {
          const event = observation.events.find((e) => e.kind === 'client' && e.clientKind === kind);
          const missing = observation.state === 'loading'
            ? '正在读取请求记录…'
            : observation.state === 'error'
              ? '读取失败，暂无法判断'
              : '最近窗口未见请求';
          return (
            <div key={kind} className={`diagnostics-client-item${event ? ' is-observed' : ''}`}>
              <dt>
                <span className={`diagnostics-client-dot${event ? ' is-observed' : ''}`} aria-hidden="true" />
                <span>{label}</span>
              </dt>
              <dd>{event ? `已观察到请求 · ${formatTimestamp(event.timestamp)}` : missing}</dd>
            </div>
          );
        })}
      </dl>
    </section>
  );
}
