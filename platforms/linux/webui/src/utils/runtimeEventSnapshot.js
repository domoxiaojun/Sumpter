import { eventIsInFlight } from './helpers.js';

// seq 是事件入队身份；长请求晚完成时只能比较完成变更水位。
export function completedEventsAfterSnapshot(events, snapshot, kind = '') {
  if (!snapshot) return [];
  const existing = new Map((snapshot.events || []).map((event) => [event.id, event]));
  const oldestVisibleSeq = Math.min(...(snapshot.events || []).map((event) => Number(event.seq || 0)));
  return (events || []).filter((event) => {
    if ((kind && event.kind !== kind) || eventIsInFlight(event)) return false;
    if (snapshot.snapshotChangeSeq != null) {
      return Number(event.changeSeq || 0) > Number(snapshot.snapshotChangeSeq);
    }
    const previous = existing.get(event.id);
    // 旧 daemon 没有完成水位；仅比对可见页范围内的身份/版本，避免旧历史触发永久刷新。
    if (previous) return Number(event.changeSeq || 0) > Number(previous.changeSeq || 0);
    return !Number.isFinite(oldestVisibleSeq) || Number(event.seq || 0) >= oldestVisibleSeq;
  });
}

export function mergeCompletedPage(events, additions, pageSize) {
  const byID = new Map((events || []).filter((event) => !eventIsInFlight(event)).map((event) => [event.id, event]));
  for (const event of additions || []) {
    const previous = byID.get(event.id);
    if (!previous || Number(event.changeSeq || 0) >= Number(previous.changeSeq || 0)) byID.set(event.id, event);
  }
  return [...byID.values()].sort((left, right) => Number(right.seq || 0) - Number(left.seq || 0)).slice(0, pageSize);
}
