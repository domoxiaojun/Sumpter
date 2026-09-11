import React, { useCallback, useId, useLayoutEffect, useRef, useState } from 'react';
import { tableScrollMetrics, tableScrollHint } from '../utils/tableScroll.js';

// Native overlay scrollbars can disappear even with overflow: scroll. Keep a
// keyboard-operable control outside the scroller for every overflowing list.
export function HorizontalScroll({ children, className = '', shellClassName = '', style,
  containerRef, onScroll, ariaLabel = '列表', role = 'region', tabIndex = 0 }) {
  const id = useId();
  const scroller = useRef(null);
  const [scroll, setScroll] = useState({ left: 0, max: 0, viewport: 0, total: 0 });
  const attach = useCallback((node) => {
    scroller.current = node;
    if (typeof containerRef === 'function') containerRef(node);
    else if (containerRef) containerRef.current = node;
  }, [containerRef]);
  const measure = useCallback(() => {
    const next = tableScrollMetrics(scroller.current);
    setScroll(previous => Object.keys(next).every(key => previous[key] === next[key]) ? previous : next);
  }, []);
  useLayoutEffect(() => {
    const node = scroller.current;
    if (!node) return undefined;
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    for (const child of node.children) observer.observe(child);
    return () => observer.disconnect();
  }, [children, measure]);
  const move = (left) => {
    if (!scroller.current) return;
    scroller.current.scrollLeft = Math.min(scroll.max, Math.max(0, left));
    measure();
  };
  return <div className={`horizontal-scroll-shell ${shellClassName}`}>
    <div id={id} ref={attach} className={`horizontal-scroll-viewport ${className}`} style={style}
      role={role} tabIndex={role === 'tablist' ? undefined : tabIndex}
      aria-label={`${ariaLabel}${scroll.max > 0 ? '（可横向滚动）' : ''}`}
      onScroll={event => { measure(); onScroll?.(event); }}>
      {children}
    </div>
    {scroll.max > 0 && <div className="table-scroll-controls" role="group" aria-label={`${ariaLabel}滚动控制`}>
      <button type="button" className="table-scroll-step" aria-label={`${ariaLabel}向左滚动`}
        aria-controls={id} disabled={scroll.left <= 1} onClick={() => move(scroll.left - scroll.viewport * 0.75)}>‹</button>
      <input type="range" className="table-scrollbar" aria-label={`${ariaLabel}横向滚动`} aria-controls={id}
        min="0" max={scroll.max} step="1" value={scroll.left}
        aria-valuetext={`${Math.round(scroll.left / scroll.max * 100)}%`}
        title={tableScrollHint(scroll)}
        style={{ '--table-scroll-thumb': `${Math.max(8, scroll.viewport / scroll.total * 100)}%` }}
        onChange={event => move(Number(event.target.value))} />
      <button type="button" className="table-scroll-step" aria-label={`${ariaLabel}向右滚动`}
        aria-controls={id} disabled={scroll.left >= scroll.max - 1} onClick={() => move(scroll.left + scroll.viewport * 0.75)}>›</button>
    </div>}
  </div>;
}
