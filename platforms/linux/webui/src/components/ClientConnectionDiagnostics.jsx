import { useEffect, useState } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { api } from '../services/api.js';
import { formatTimestamp } from '../utils/helpers.js';
import { copyWithToast } from '../utils/clipboard.js';

const clients = [['Claude Code', ['claude_code']], ['Codex', ['codex']], ['Gemini CLI', ['gemini_cli']], ['pi', ['pi']], ['Grok Build', ['grok_build']]];

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
  return <section className="glass-panel panel-padded-stack" aria-label="客户端连接诊断">
    <h2>客户端连接诊断</h2>
    <p>下表依据最近 100 条已完成客户端请求。未见请求表示未知，不能据此判断客户端绕过代理。</p>
    <dl className="event-inspector-fields">{clients.map(([label, kinds]) => {
      const event = observation.events.find(e => e.kind === 'client' && kinds.includes(e.clientKind));
      const missing = observation.state === 'loading' ? '正在读取请求记录…' : observation.state === 'error' ? '读取失败，暂无法判断' : '最近窗口未见请求';
      return <div key={label}><dt>{label}</dt><dd>{event ? `已观察到请求 · ${formatTimestamp(event.timestamp)}` : missing}</dd></div>;
    })}</dl>
    <button className="btn btn-secondary" disabled={observation.state === 'loading'} onClick={() => setRefresh(value => value + 1)}>刷新请求观察</button>
    <p>在运行客户端的机器上执行归因脚本的只读诊断，检查可见地址、环境变量、包装器状态与 pi 显式标记。浏览器无法读取另一台机器的客户端配置。</p>
    <code>node client-attribution.mjs doctor all</code>
    <p>远程代理可追加 <code>--proxy-url 实际代理地址</code>。命令不读取鉴权存储、不发起模型请求、不修改配置。</p>
    <button className="btn btn-secondary" onClick={() => copyWithToast('node client-attribution.mjs doctor all', addToast)}>复制诊断命令</button>
  </section>;
}
