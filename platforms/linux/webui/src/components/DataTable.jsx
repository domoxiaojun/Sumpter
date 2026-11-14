import { cn } from '../lib/utils.js';
import { HorizontalScroll } from './HorizontalScroll.jsx';

const COLUMN_TYPE_DEFAULTS = Object.freeze({
  status: { width: '152px', minWidth: '132px' },
  number: { width: '104px', minWidth: '88px', align: 'right' },
  time: { width: '184px', minWidth: '168px' },
  action: { width: '176px', minWidth: '144px', align: 'right' },
  id: { minWidth: '190px' },
  text: { minWidth: '180px' },
});

function tableMinimumWidth(columns) {
  const pixels = columns.reduce((total, column) => {
    const presentation = columnPresentation(column);
    const value = String(presentation.minWidth || presentation.width || '0');
    const parsed = Number.parseFloat(value);
    return total + (Number.isFinite(parsed) ? parsed : 0);
  }, 0);
  return pixels > 0 ? `${Math.ceil(pixels)}px` : undefined;
}

function columnPresentation(column, adaptive = false) {
  const defaults = COLUMN_TYPE_DEFAULTS[column.type] || {};
  return {
    width: adaptive ? undefined : column.width ?? defaults.width,
    minWidth: column.minWidth ?? (adaptive ? undefined : defaults.minWidth),
    align: column.align ?? defaults.align ?? 'left',
  };
}

export function DataTable({
  columns = [],
  data = [],
  keyField = 'id',
  onRowClick,
  getRowProps,
  activeRowKey,
  emptyText = '暂无数据',
  className = '',
  containerStyle,
  containerRef,
  onContainerScroll,
  ariaLabel,
  tableMinWidth,
  adaptive = false,
  sortKey,
  sortDirection = 'asc',
  onSortChange,
  loading = false,
  scrollHeight,
  beforeTable = null,
}) {
  const hasData = Boolean(data && data.length > 0);

  const emptyState = (
    <div className="flex min-h-28 items-center justify-center px-4 py-8 text-center text-xs text-muted-foreground">
      {emptyText}
    </div>
  );

  if (!hasData && !beforeTable) {
    return emptyState;
  }

  return (
    <HorizontalScroll
      dragToScroll={adaptive}
      controlsAtTop={adaptive}
      shellClassName="w-full"
      containerRef={containerRef}
      className={cn('w-full', scrollHeight && 'overflow-y-auto', className)}
      style={{ ...(containerStyle || {}), ...(scrollHeight ? { maxHeight: scrollHeight, overflowY: 'auto' } : {}) }}
      onScroll={onContainerScroll}
      tabIndex={0}
      role="region"
      ariaLabel={ariaLabel || '数据表'}
    >
      {beforeTable}
      {hasData ? (
        <table
          className={cn(
            'w-full caption-bottom border-collapse text-sm',
            adaptive && 'table-auto',
          )}
          aria-label={ariaLabel}
          aria-busy={loading || undefined}
          style={{ minWidth: tableMinWidth || (adaptive ? '100%' : tableMinimumWidth(columns)) }}
        >
          <colgroup>
            {columns.map((col, idx) => {
              const presentation = columnPresentation(col, adaptive);
              return (
                <col
                  key={col.key || idx}
                  style={{
                    width: presentation.width || undefined,
                    minWidth: presentation.minWidth || undefined,
                  }}
                />
              );
            })}
          </colgroup>
          <thead className="sticky top-0 z-10 border-b bg-muted/75 backdrop-blur-sm">
            <tr>
              {columns.map((col, idx) => {
                const presentation = columnPresentation(col, adaptive);
                const sortable = Boolean(col.sortable && col.key && onSortChange);
                const activeSort = sortable && sortKey === col.key;
                const ariaSort = sortable
                  ? (activeSort ? (sortDirection === 'desc' ? 'descending' : 'ascending') : 'none')
                  : undefined;
                return (
                  <th
                    key={col.key || idx}
                    className={cn(
                      'h-10 px-3.5 align-middle text-[11px] font-medium tracking-wider text-muted-foreground whitespace-nowrap',
                      col.type ? `data-table-column-${col.type}` : '',
                    )}
                    aria-sort={ariaSort}
                    style={{
                      textAlign: presentation.align,
                      width: presentation.width || 'auto',
                      minWidth: presentation.minWidth || undefined,
                    }}
                  >
                    {sortable ? (
                      <button
                        type="button"
                        className={cn(
                          'data-table-sort-button inline-flex min-h-6 cursor-pointer items-center gap-1 rounded px-1 py-0.5 text-[11px] font-medium transition-colors hover:bg-accent hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50',
                          presentation.align === 'right' && 'ml-auto flex-row-reverse',
                          activeSort && 'text-foreground font-semibold',
                        )}
                        disabled={loading}
                        aria-label={`${col.title}，点击按${activeSort && sortDirection === 'asc' ? '降序' : '升序'}排列`}
                        onClick={() => onSortChange(
                          col.key,
                          activeSort && sortDirection === 'asc' ? 'desc' : 'asc',
                        )}
                      >
                        <span>{col.title}</span>
                        <span aria-hidden="true" className="text-[10px] opacity-75">
                          {activeSort ? (sortDirection === 'desc' ? '↓' : '↑') : '↕'}
                        </span>
                      </button>
                    ) : col.title}
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody className="divide-y divide-border/70">
            {data.map((row, rowIdx) => {
              const key = row[keyField] || rowIdx;
              const isActive = activeRowKey && key === activeRowKey;
              const customRowProps = getRowProps?.(row, rowIdx) || {};
              const { className: customRowClassName, ...rowInteractionProps } = customRowProps;
              return (
                <tr
                  key={key}
                  {...rowInteractionProps}
                  className={cn(
                    'transition-colors hover:bg-muted/45 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
                    isActive && 'bg-primary/10 hover:bg-primary/15',
                    customRowClassName,
                  )}
                  onClick={onRowClick ? () => onRowClick(row) : undefined}
                  onKeyDown={onRowClick ? (event) => {
                    // Interactive controls inside an actionable row own their
                    // keyboard events; do not turn Enter/Space on an export or
                    // delete button into an unrelated row drill-down.
                    if (event.target !== event.currentTarget) return;
                    if (event.key === 'Enter' || event.key === ' ') {
                      event.preventDefault();
                      onRowClick(row);
                    }
                  } : undefined}
                  tabIndex={onRowClick ? 0 : undefined}
                  aria-selected={onRowClick ? isActive : undefined}
                  style={{ cursor: onRowClick ? 'pointer' : 'default' }}
                >
                  {columns.map((col, colIdx) => {
                    const val = col.render ? col.render(row, rowIdx) : row[col.key];
                    const presentation = columnPresentation(col, adaptive);
                    return (
                      <td
                        key={col.key || colIdx}
                        data-label={col.title || col.key || undefined}
                        className={cn(
                          'px-3.5 py-2.5 align-middle text-xs',
                          presentation.align === 'right' && 'text-right font-mono tabular-nums',
                          col.type ? `data-table-cell-${col.type}` : '',
                        )}
                        style={{
                          textAlign: presentation.align,
                          minWidth: presentation.minWidth || undefined,
                        }}
                      >
                        {val !== undefined && val !== null && val !== '' ? val : '-'}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
          </tbody>
        </table>
      ) : emptyState}
    </HorizontalScroll>
  );
}
