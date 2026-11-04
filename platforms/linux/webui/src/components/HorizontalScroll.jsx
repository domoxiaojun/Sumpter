import { useCallback, useId, useLayoutEffect, useRef, useState } from 'react';
import { tableScrollMetrics, tableScrollHint } from '../utils/tableScroll.js';

// Native overlay scrollbars can disappear even with overflow: scroll. Keep a
// keyboard-operable control outside the scroller for every overflowing list.
export function HorizontalScroll({ children, className = '', shellClassName = '', style,
  containerRef, onScroll, ariaLabel = '列表', role = 'region', tabIndex = 0,
  dragToScroll = false, controlsAtTop = false }) {
  const id = useId();
  const scroller = useRef(null);
  const drag = useRef(null);
  const suppressClick = useRef(false);
  const [dragging, setDragging] = useState(false);
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
  const finishDrag = (event) => {
    drag.current = null;
    setDragging(false);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };
  const controls = (position) => scroll.max > 0 && <div
    className={`table-scroll-controls table-scroll-controls-${position}`} role="group"
    aria-label={`${ariaLabel}滚动控制（${position === 'top' ? '顶部' : '底部'}）`}>
    {position === 'top' && <span className="table-scroll-hint" id={`${id}-hint`}>
      {dragToScroll ? '左右拖动表格，或使用滚动条' : tableScrollHint(scroll)}
    </span>}
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
  </div>;
  return <div className={`horizontal-scroll-shell ${shellClassName}`}>
    {controlsAtTop && controls('top')}
    <div id={id} ref={attach} className={`horizontal-scroll-viewport ${className}${dragToScroll && scroll.max > 0 ? ' is-draggable' : ''}${dragging ? ' is-dragging' : ''}`} style={style}
      role={role} tabIndex={role === 'tablist' ? undefined : tabIndex}
      aria-label={`${ariaLabel}${scroll.max > 0 ? '（可横向滚动）' : ''}`}
      aria-describedby={controlsAtTop && scroll.max > 0 ? `${id}-hint` : undefined}
      onPointerDown={event => {
        suppressClick.current = false;
        if (!dragToScroll || scroll.max <= 0 || event.pointerType !== 'mouse' || event.button !== 0
          || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey
          || event.target.closest('button, a, input, select, textarea, summary, [contenteditable="true"], [draggable="true"]')) return;
        drag.current = { id: event.pointerId, x: event.clientX, y: event.clientY, left: scroller.current.scrollLeft, active: false };
      }}
      onPointerMove={event => {
        const start = drag.current;
        if (!start || start.id !== event.pointerId) return;
        const dx = event.clientX - start.x;
        const dy = event.clientY - start.y;
        if (!start.active) {
          if (Math.abs(dy) > Math.abs(dx)) { drag.current = null; return; }
          if (Math.abs(dx) < 6) return;
          start.active = true;
          suppressClick.current = true;
          event.currentTarget.setPointerCapture(event.pointerId);
          setDragging(true);
        }
        event.preventDefault();
        window.getSelection()?.removeAllRanges();
        move(start.left - dx);
      }}
      onPointerUp={finishDrag}
      onPointerCancel={finishDrag}
      onLostPointerCapture={() => { drag.current = null; setDragging(false); }}
      onPointerLeave={() => { if (!drag.current?.active) drag.current = null; }}
      onClickCapture={event => {
        if (!suppressClick.current) return;
        suppressClick.current = false;
        event.preventDefault();
        event.stopPropagation();
      }}
      onKeyDown={event => {
        if (event.target !== event.currentTarget || scroll.max <= 0 || event.altKey || event.ctrlKey || event.metaKey) return;
        const positions = { ArrowLeft: scroll.left - 80, ArrowRight: scroll.left + 80, Home: 0, End: scroll.max };
        if (positions[event.key] === undefined) return;
        event.preventDefault();
        move(positions[event.key]);
      }}
      onScroll={event => { measure(); onScroll?.(event); }}>
      {children}
    </div>
    {controls('bottom')}
  </div>;
}
