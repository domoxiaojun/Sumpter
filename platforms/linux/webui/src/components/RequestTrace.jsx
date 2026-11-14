import { traceActions, traceReasons } from '../utils/requestPolicy.js';

export function RequestTrace({ trace }) {
  if (!trace?.steps?.length) {
    return (
      <p className="rounded-lg border border-dashed px-3 py-2.5 text-xs text-muted-foreground">
        此记录没有路由过程；旧记录与未采集的传输不会补造阶段耗时。
      </p>
    );
  }
  return (
    <section aria-label="路由过程" className="mt-3 space-y-2">
      <h4 className="text-xs font-semibold text-foreground">路由过程</h4>
      <ol className="space-y-1.5 border-l border-border/80 pl-3">
        {trace.steps.map((step, index) => (
          <li key={index} className="flex flex-wrap items-baseline gap-2 text-xs">
            <small className="font-mono text-[11px] text-muted-foreground tabular-nums">
              +{step.atMs} ms
            </small>
            <strong className="font-medium text-foreground">
              {traceActions[step.action] || step.action}
            </strong>
            <span className="text-muted-foreground tabular-nums">
              {step.endpointId && <>{step.endpointId} · </>}
              {traceReasons[step.detail] || step.detail}
              {step.durationMs != null && <> · {step.durationMs} ms</>}
            </span>
          </li>
        ))}
      </ol>
      {trace.omitted > 0 && (
        <p className="text-[11px] text-muted-foreground">
          另外 {trace.omitted} 个步骤因记录上限省略。
        </p>
      )}
      <p className="text-[11px] text-muted-foreground">
        时间相对本次请求；计划重试不代表已经再次发送。缓冲与上游等待可能重叠，不直接相加。
      </p>
    </section>
  );
}
