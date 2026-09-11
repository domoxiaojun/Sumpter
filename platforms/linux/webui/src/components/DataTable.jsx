import React from 'react';
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

function columnPresentation(column) {
  const defaults = COLUMN_TYPE_DEFAULTS[column.type] || {};
  return {
    width: column.width ?? defaults.width,
    minWidth: column.minWidth ?? defaults.minWidth,
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
  sortKey,
  sortDirection = 'asc',
  onSortChange,
  scrollHeight,
  beforeTable = null,
}) {
  const hasData = Boolean(data && data.length > 0);

  const emptyState = (
    <div
      style={{
        padding: '40px 20px',
        textAlign: 'center',
        color: 'var(--text-muted)',
        fontSize: '0.9rem',
        background: 'var(--bg-surface-glass)',
        borderRadius: 'var(--radius-lg)',
        border: '1px dashed var(--border-medium)',
      }}
    >
      {emptyText}
    </div>
  );

  if (!hasData && !beforeTable) {
    return emptyState;
  }

  return (
    <HorizontalScroll
      shellClassName="data-table-shell"
      containerRef={containerRef}
      className={`table-container ${scrollHeight ? 'table-container-scroll' : ''} ${className}`}
      style={{ ...(containerStyle || {}), ...(scrollHeight ? { maxHeight: scrollHeight, overflowY: 'auto' } : {}) }}
      onScroll={onContainerScroll}
      tabIndex={0}
      role="region"
      ariaLabel={ariaLabel || '数据表'}
    >
      {beforeTable}
      {hasData ? (
        <table
          className="data-table"
          aria-label={ariaLabel}
          style={{ minWidth: tableMinWidth || tableMinimumWidth(columns) }}
        >
          <colgroup>
            {columns.map((col, idx) => {
              const presentation = columnPresentation(col);
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
          <thead>
            <tr>
              {columns.map((col, idx) => {
                const presentation = columnPresentation(col);
                const sortable = Boolean(col.sortable && col.key && onSortChange);
                const activeSort = sortable && sortKey === col.key;
                const ariaSort = sortable
                  ? (activeSort ? (sortDirection === 'desc' ? 'descending' : 'ascending') : 'none')
                  : undefined;
                return (
                  <th
                    key={col.key || idx}
                    className={col.type ? `data-table-column-${col.type}` : undefined}
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
                        className="data-table-sort-button"
                        onClick={() => onSortChange(
                          col.key,
                          activeSort && sortDirection === 'asc' ? 'desc' : 'asc',
                        )}
                      >
                        <span>{col.title}</span>
                        <span aria-hidden="true" className="data-table-sort-indicator">
                          {activeSort ? (sortDirection === 'desc' ? '↓' : '↑') : '↕'}
                        </span>
                      </button>
                    ) : col.title}
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {data.map((row, rowIdx) => {
              const key = row[keyField] || rowIdx;
              const isActive = activeRowKey && key === activeRowKey;
              const customRowProps = getRowProps?.(row, rowIdx) || {};
              const { className: customRowClassName, ...rowInteractionProps } = customRowProps;
              return (
                <tr
                  key={key}
                  {...rowInteractionProps}
                  className={[isActive ? 'active-row' : '', customRowClassName || ''].filter(Boolean).join(' ')}
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
                    const presentation = columnPresentation(col);
                    return (
                      <td
                        key={col.key || colIdx}
                        data-label={col.title || col.key || undefined}
                        className={col.type ? `data-table-cell-${col.type}` : undefined}
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
