import React, { useRef } from 'react';
import { connectionsPresentation, visibleConnectionsObservation } from '../utils/responsesConnections.js';

export function ResponsesConnectionsPanel({ observation, autoRefresh }) {
  const displayed = useRef(null);
  displayed.current = visibleConnectionsObservation(displayed.current, observation, autoRefresh);
  const presentation = connectionsPresentation(displayed.current);
  return (
    <section className="responses-connections-panel" aria-label="当前 Responses WebSocket">
      <div className="responses-connections-heading">
        <strong>当前 Responses WebSocket</strong>
        {!autoRefresh && <span>已暂停刷新</span>}
        {presentation.error && <span className="responses-connections-error" role="status">{presentation.error}</span>}
      </div>
      {presentation.message ? <p>{presentation.message}</p> : (
        <dl className="responses-connections-values">
          {presentation.rows.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
        </dl>
      )}
      <p>当前进程内的连接快照；已进入转发不代表模型正在生成。</p>
    </section>
  );
}
