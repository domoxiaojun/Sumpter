export const RUNTIME_CACHE_RELEASE_MS = 60_000;
export const RUNTIME_DETAIL_CACHE_BYTES = 8 * 1024 * 1024;

// Response bytes are a budget proxy; they are not the JS heap footprint.
const responseCosts = new WeakMap();
export function rememberResponseCost(value, bytes) {
  if (value && typeof value === 'object') responseCosts.set(value, bytes);
  return value;
}
export function responseCost(value) {
  return responseCosts.get(value) ?? new TextEncoder().encode(JSON.stringify(value)).byteLength;
}

export class BoundedLRUCache {
  constructor(countLimit = 200, costLimit = RUNTIME_DETAIL_CACHE_BYTES) {
    this.countLimit = Math.max(0, countLimit);
    this.costLimit = Math.max(0, costLimit);
    this.items = new Map();
    this.cost = 0;
  }
  get size() { return this.items.size; }
  peek(key) { return this.items.get(key)?.value; }
  get(key) {
    const item = this.items.get(key);
    if (!item) return undefined;
    this.items.delete(key);
    this.items.set(key, item);
    return item.value;
  }
  set(key, value, cost = responseCost(value)) {
    this.delete(key);
    if (!Number.isFinite(cost) || cost < 0 || cost > this.costLimit || !this.countLimit) return this;
    while (this.items.size && (this.items.size >= this.countLimit || this.cost + cost > this.costLimit)) {
      this.delete(this.items.keys().next().value);
    }
    this.items.set(key, { value, cost });
    this.cost += cost;
    return this;
  }
  delete(key) {
    const item = this.items.get(key);
    if (item) this.cost -= item.cost;
    return this.items.delete(key);
  }
  clear() { this.items.clear(); this.cost = 0; }
}

export function runtimePageActive(route, visible = true) {
  return visible && (route === 'run' || route === 'statistics');
}

export function createPageCacheLifecycle({ onVisibility, onRetain, cancelRequests, release,
  schedule = setTimeout, cancel = clearTimeout, delay = RUNTIME_CACHE_RELEASE_MS }) {
  let visible = true;
  let timer;
  return {
    update(nextVisible) {
      visible = nextVisible;
      cancel(timer);
      timer = undefined;
      onVisibility(visible);
      if (visible) onRetain();
      else {
        cancelRequests();
        timer = schedule(() => { timer = undefined; if (!visible) release(); }, delay);
      }
    },
    dispose() { cancel(timer); timer = undefined; },
  };
}
