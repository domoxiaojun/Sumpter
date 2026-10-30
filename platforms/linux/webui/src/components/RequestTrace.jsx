import { traceActions, traceReasons } from '../utils/requestPolicy.js';

export function RequestTrace({ trace }) {
  if (!trace?.steps?.length) return <p className="event-chain-empty">此记录没有路由过程；旧记录与未采集的传输不会补造阶段耗时。</p>;
  return <section aria-label="路由过程">
    <h4>路由过程</h4>
    <ol className="request-trace-list">{trace.steps.map((step, index) => <li key={index}>
      <small>+{step.atMs} ms</small><strong>{traceActions[step.action] || step.action}</strong>
      <span>{step.endpointId && <>{step.endpointId} · </>}{traceReasons[step.detail] || step.detail}{step.durationMs != null && <> · {step.durationMs} ms</>}</span>
    </li>)}</ol>
    {trace.omitted > 0 && <p>另外 {trace.omitted} 个步骤因记录上限省略。</p>}
    <p className="event-chain-empty">时间相对本次请求；计划重试不代表已经再次发送。缓冲与上游等待可能重叠，不直接相加。</p>
  </section>;
}
