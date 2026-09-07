// Pure analytics helpers for the runtime-event dashboard.  Keep the wire
// parser tolerant here, while leaving the page component responsible only for
// hierarchy and presentation.
import {
  cleanText,
  eventClientKindLabel,
  eventCodexMetadata,
  eventFailureKind,
  eventFailurePhase,
  eventField,
  eventIsInFlight,
  eventProtocolRouteLabel,
  eventPurposeLabel,
  eventResultKind,
  eventStreamTrace,
  eventToolCalls,
  failureKindLabel,
  failurePhaseLabel,
} from './helpers.js';

function number(value) {
  const result = Number(value);
  return Number.isFinite(result) ? result : 0;
}

function traceValue(trace, camel, snake) {
  return trace?.[camel] ?? trace?.[snake];
}

function mapCount(map, key, amount = 1) {
  const normalized = cleanText(key) || '未记录';
  map.set(normalized, (map.get(normalized) || 0) + amount);
}

function mapList(map) {
  return Array.from(map.entries())
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
}

export function codexDimensionValues(metadata, dimension) {
  if (!metadata || typeof metadata !== 'object') return [];
  const value = (camel, snake = camel) => metadata[camel] ?? metadata[snake];
  switch (dimension) {
    case 'requestKind': return [value('requestKind', 'request_kind') || '未记录请求类型'];
    case 'subagentKind': {
      const kind = value('subagentKind', 'subagent_kind') || value('subagentHeader', 'subagent_header');
      const isSubagent = Boolean(value('isSubagent', 'is_subagent'));
      if (kind === 'guardian' || value('threadSource', 'thread_source') === 'guardian_review') return ['Guardian 安全审查'];
      return [kind || (isSubagent ? '子代理（类型未记录）' : '未发现子代理证据')];
    }
    case 'threadSource': return [value('threadSource', 'thread_source') || '未记录线程来源'];
    case 'agentName': return [value('agentName', 'agent_name') || '未记录代理路径'];
    case 'workspace': {
      const workspaces = value('workspaces');
      const paths = workspaces && typeof workspaces === 'object' ? Object.keys(workspaces) : [];
      return paths.length ? paths : ['未记录工作区'];
    }
    case 'toolNamespace': {
      const namespaces = value('toolNamespacesInfo', 'tool_namespaces_info');
      const names = namespaces && typeof namespaces === 'object' ? Object.keys(namespaces) : [];
      return names.length ? names : ['未记录工具命名空间'];
    }
    case 'compaction': {
      const compaction = value('compaction');
      if (!compaction || typeof compaction !== 'object') return ['无压缩元数据'];
      const trigger = cleanText(compaction.trigger ?? compaction.kind ?? compaction.reason);
      const phase = cleanText(compaction.phase ?? compaction.status);
      return [trigger && phase ? `${trigger} · ${phase}` : trigger || phase || '已记录压缩'];
    }
    default: return [];
  }
}

export function aggregateEvents(events = [], extractor = (event) => event?.id || '未记录', kindFilter = null) {
  const rows = new Map();
  events.forEach((event) => {
    // notify 是控制面通知，不是客户端请求或上游尝试；即使旧记录带
    // statusCode=200，也不能进入成功率、延迟或路由维度统计。
    if (cleanText(event?.kind).toLowerCase() === 'notify') return;
    if (kindFilter && event?.kind !== kindFilter) return;
    const rawKey = extractor(event);
    const keys = Array.isArray(rawKey) ? rawKey : [rawKey];
    keys.filter((key) => key !== undefined && key !== null).forEach((key) => {
      const name = cleanText(key) || '未记录';
      let row = rows.get(name);
      if (!row) {
        row = {
          name,
          attempts: 0,
          successes: 0,
          failures: 0,
          cancelled: 0,
          pending: 0,
          failovers: 0,
          totalTTFB: 0,
          countTTFB: 0,
          totalDuration: 0,
          countDuration: 0,
          totalBytes: 0,
          totalChunks: 0,
          countChunks: 0,
          maxChunkGapMS: 0,
          terminalEvents: new Map(),
          failureKinds: new Map(),
          failurePhases: new Map(),
          tools: new Map(),
          upstreamStatuses: new Map(),
          effectiveModels: new Map(),
          featureRules: new Map(),
          eventIDs: [],
        };
        rows.set(name, row);
      }
      row.attempts += 1;
      if (event?.id && !row.eventIDs.includes(event.id)) row.eventIDs.push(event.id);

      const result = eventResultKind(event);
      if (result === 'succeeded') row.successes += 1;
      else if (result === 'failed') row.failures += 1;
      else if (result === 'cancelled') row.cancelled += 1;
      else row.pending += 1;

      if (eventFailover(event)) row.failovers += 1;
      const inFlight = eventIsInFlight(event);
      const ttfb = number(eventField(event, 'ttfbMS', 'ttfb_ms'));
      if (!inFlight && eventField(event, 'ttfbMS', 'ttfb_ms') != null) {
        row.totalTTFB += ttfb;
        row.countTTFB += 1;
      }
      const duration = eventField(event, 'durationMS', 'duration_ms');
      if (!inFlight && duration != null && number(duration) >= 0) {
        row.totalDuration += number(duration);
        row.countDuration += 1;
      }

      const trace = eventStreamTrace(event);
      if (trace) {
        const bytes = traceValue(trace, 'bytesReceived', 'bytes_received');
        const chunks = traceValue(trace, 'chunkCount', 'chunk_count');
        const gap = traceValue(trace, 'maxChunkGapMS', 'max_chunk_gap_ms');
        if (bytes != null) row.totalBytes += number(bytes);
        if (chunks != null) {
          row.totalChunks += number(chunks);
          row.countChunks += 1;
        }
        if (gap != null) row.maxChunkGapMS = Math.max(row.maxChunkGapMS, number(gap));
        const terminal = traceValue(trace, 'terminalEvent', 'terminal_event');
        if (terminal) mapCount(row.terminalEvents, terminal);
      }

      const failure = eventFailureKind(event);
      if (failure) mapCount(row.failureKinds, failure);
      const failurePhase = eventFailurePhase(event);
      if (failurePhase) mapCount(row.failurePhases, failurePhase);
      eventToolCalls(event).forEach((tool) => mapCount(row.tools, tool));
      const upstreamStatus = eventField(event, 'upstreamStatusCode', 'upstream_status_code');
      if (upstreamStatus != null) mapCount(row.upstreamStatuses, String(upstreamStatus));
      const effectiveModel = eventField(event, 'effectiveModel', 'effective_model');
      if (effectiveModel) mapCount(row.effectiveModels, effectiveModel);
      const featureRule = eventField(event, 'featureRuleID', 'feature_rule_id');
      if (featureRule) mapCount(row.featureRules, featureRule);
    });
  });

  return Array.from(rows.values()).map((row) => {
    const {
      failureKinds, failurePhases, tools, terminalEvents, upstreamStatuses,
      effectiveModels, featureRules, ...plainRow
    } = row;
    return {
      ...plainRow,
      successRate: (row.successes + row.failures + row.cancelled)
        ? (row.successes / (row.successes + row.failures + row.cancelled)) * 100
        : 0,
      avgTTFB: row.countTTFB ? Math.round(row.totalTTFB / row.countTTFB) : 0,
      avgDuration: row.countDuration ? Math.round(row.totalDuration / row.countDuration) : 0,
      avgChunks: row.countChunks ? Math.round(row.totalChunks / row.countChunks) : 0,
      failureKindsList: mapList(failureKinds),
      failurePhasesList: mapList(failurePhases),
      toolsList: mapList(tools),
      terminalEventsList: mapList(terminalEvents),
      upstreamStatusesList: mapList(upstreamStatuses),
      effectiveModelsList: mapList(effectiveModels),
      featureRulesList: mapList(featureRules),
    };
  }).sort((a, b) => b.attempts - a.attempts || a.name.localeCompare(b.name));
}

export function dimensionRows(events, extractor, kindFilter = null) {
  return aggregateEvents(events, extractor, kindFilter);
}

export function failureRows(events = []) {
  return aggregateEvents(events.filter((event) => eventResultKind(event) === 'failed'), (event) => {
    const kind = eventFailureKind(event);
    return kind ? `${failureKindLabel(kind)} · ${failurePhaseLabel(eventFailurePhase(event))}` : '无结构化失败原因';
  }, null);
}

export function toolRows(events = []) {
  return aggregateEvents(events, (event) => eventToolCalls(event), null)
    .filter((row) => row.name !== '未记录');
}

export function streamRows(events = []) {
  return aggregateEvents(events, (event) => {
    const trace = eventStreamTrace(event);
    const terminal = traceValue(trace, 'terminalEvent', 'terminal_event');
    return terminal || (trace ? '未观察到协议终止' : '无流诊断');
  }, null);
}

export function codexRows(events = [], dimension) {
  return aggregateEvents(
    events.filter((event) => Boolean(eventCodexMetadata(event))),
    (event) => codexDimensionValues(eventCodexMetadata(event), dimension),
    null,
  );
}

export function clientDimensionRows(events = []) {
  return dimensionRows(events, (event) => eventClientKindLabel(event), 'client');
}

export function purposeDimensionRows(events = []) {
  return dimensionRows(events, eventPurposeLabel, 'client');
}

export function protocolRouteRows(events = []) {
  return aggregateEvents(events, eventProtocolRouteLabel, 'client');
}

function eventFailover(event) {
  const value = eventField(event, 'failover', 'failover');
  return typeof value === 'string' ? ['true', '1'].includes(value.toLowerCase()) : Boolean(value);
}
