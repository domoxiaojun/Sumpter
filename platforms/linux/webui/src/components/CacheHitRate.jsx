import React from 'react';
import { eventCacheHitRateLabel } from '../utils/eventPresentation.js';

export function CacheHitRate({ event }) {
  if (event.kind === 'notify') return null;
  return <small className="event-cache-hit-rate"
    title="缓存读取 Token / 总输入 Token；Anthropic 总输入包含缓存读取和缓存写入。数据未确认或分母未知时显示 —。">
    {eventCacheHitRateLabel(event)}
  </small>;
}
