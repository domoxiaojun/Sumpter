export const DEFAULT_LIMITS = { maxConcurrent: 0, maxQueued: 128, queueTimeoutSeconds: 30, requestsPerMinute: 0 };
export const DEFAULT_REQUEST_POLICY = { maxTotalAttempts: 0, responseWaitTimeoutSeconds: 0, streamProtection: 'complete', streamBufferMib: 16, streamBufferChunks: 8192, streamBufferSeconds: 0, maxBufferedMib: 128, global: DEFAULT_LIMITS, endpoints: {} };

export function requestPolicy(value = {}) {
  const result = structuredClone({ ...DEFAULT_REQUEST_POLICY, ...value, global: { ...DEFAULT_LIMITS, ...value.global } });
  for (const [id, endpoint] of Object.entries(result.endpoints)) result.endpoints[id] = { ...endpoint, limits: { ...DEFAULT_LIMITS, ...endpoint.limits }, models: { ...endpoint.models } };
  return result;
}

export function validateRequestPolicy(value) {
  const check = (v, min, max, label) => { if (!Number.isInteger(v) || v < min || v > max) throw new Error(`${label}必须是 ${min}–${max} 的整数`); };
  check(value.maxTotalAttempts, 0, 10000, '总尝试上限');
  check(value.responseWaitTimeoutSeconds, 0, 86400, '代理响应等待上限');
  check(value.streamBufferMib, 1, 256, '单流缓冲'); check(value.maxBufferedMib, 1, 4096, '总缓冲');
  check(value.streamBufferChunks, 16, 65536, '响应块上限'); check(value.streamBufferSeconds, 0, 3600, '缓冲时限');
  if (!['complete', 'prelude'].includes(value.streamProtection)) throw new Error('请选择有效的流式策略');
  for (const limits of [value.global, ...Object.values(value.endpoints).map(p => p.limits)]) {
    check(limits.maxConcurrent, 0, 10000, '并发上限'); check(limits.maxQueued, 0, 10000, '队列上限');
    check(limits.queueTimeoutSeconds, 1, 300, '排队时限'); check(limits.requestsPerMinute, 0, 1000000, '每分钟请求上限');
  }
  return value;
}

export const traceActions = { plan: '规划', candidate: '候选', affinity: '会话归属', rule: '分流规则', queue: '排队', attempt: '发送上游', headers: '响应头', buffer: '缓冲', failure: '失败分类', skip: '跳过', admission_rejected: '本地限流', retry_scheduled: '计划重试', retry_wait: '重试等待', stop: '停止', first_chunk: '首个响应块', complete: '完成' };
export const traceReasons = { configured_candidates: '按配置选择候选', native: '原生转发', translated: '协议转换', admitted: '获得发送名额', send: '开始发送', received: '已收到响应头', first_chunk_received: '已收到响应块', transport_error: '连接失败', terminal: '已收到终态', first_output: '开始实时转发', time_budget: '达到缓冲时限', global_buffer_budget: '达到总缓冲额度', chunk_budget: '达到响应块数上限', byte_budget: '达到单流缓冲额度', quota_exhausted: '额度耗尽', credential: '鉴权错误', model_unsupported: '模型不支持', rate_limit: '暂时限流', upstream_error: '上游错误', queue_full: '队列已满', queue_timeout: '排队超时', attempt_budget: '达到总尝试上限', response_wait_timeout: '代理响应等待达到上限', backoff: '退避', wait_completed: '等待结束', same_group_capacity: '优先选择同组空闲入口', session_group: '从当前会话调度组开始', idle_timeout: '缓冲期间流空闲超时', missing_terminal: '上游未发送终态', retryable_terminal: '可重试的协议错误', failed: '失败', finished: '结束', cancelled: '已取消' };
