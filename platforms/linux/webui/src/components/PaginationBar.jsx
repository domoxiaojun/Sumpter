import React, { useEffect, useId, useState } from 'react';
import { formatNumber } from '../utils/helpers.js';
import {
  RUNTIME_EVENT_PAGE_SIZES,
  normalizeRuntimeEventPageSize,
  runtimeEventPageRange,
  runtimeEventPageWindow,
} from '../utils/runtimeAnalyticsV2.js';

export function PaginationBar({
  page = 1,
  pageSize = 10,
  totalCount = 0,
  totalPages = 0,
  itemCount = 0,
  loading = false,
  liveItemCount = 0,
  onPageChange,
  onPageSizeChange,
  ariaLabel = '分页导航',
  itemNoun = '条持久事件',
  emptySummary,
  liveItemNoun = '条进行中',
}) {
  const pageInputID = `pagination-page-${useId().replace(/:/g, '')}`;
  const normalizedPageSize = normalizeRuntimeEventPageSize(pageSize);
  const normalizedTotalCount = Math.max(0, Number.isFinite(Number(totalCount)) ? Math.trunc(Number(totalCount)) : 0);
  const normalizedItemCount = Math.max(0, Number.isFinite(Number(itemCount)) ? Math.trunc(Number(itemCount)) : 0);
  const requestedTotalPages = Math.max(0, Number.isFinite(Number(totalPages)) ? Math.trunc(Number(totalPages)) : 0);
  // A malformed/old response may omit totalPages while still returning a
  // count. Derive it here so the jump affordance never renders “/ 0” for a
  // non-empty result.
  const normalizedTotalPages = requestedTotalPages || (normalizedTotalCount
    ? Math.ceil(normalizedTotalCount / normalizedPageSize)
    : 0);
  const requestedPage = Math.max(1, Number.isFinite(Number(page)) ? Math.trunc(Number(page)) : 1);
  const currentPage = normalizedTotalPages
    ? Math.min(requestedPage, normalizedTotalPages)
    : 1;
  const pages = runtimeEventPageWindow(currentPage, normalizedTotalPages);
  const [pageDraft, setPageDraft] = useState(String(currentPage));
  const range = runtimeEventPageRange({
    page: currentPage,
    pageSize: normalizedPageSize,
    totalCount: normalizedTotalCount,
    itemCount: normalizedItemCount,
  });
  const pageCount = Math.max(1, normalizedTotalPages || 1);

  useEffect(() => {
    setPageDraft(String(currentPage));
  }, [currentPage]);

  const submitPage = (event) => {
    event?.preventDefault();
    if (normalizedTotalPages === 0) return;
    const parsed = Number.parseInt(String(pageDraft).trim(), 10);
    const target = Math.min(pageCount, Math.max(1, Number.isFinite(parsed) ? parsed : currentPage));
    setPageDraft(String(target));
    if (target !== currentPage) onPageChange?.(target);
  };

  return (
    <nav
      className={`pagination-bar${loading ? ' is-loading' : ''}`}
      aria-label={ariaLabel}
      aria-busy={loading}
      data-page={currentPage}
      data-loading={loading ? 'true' : 'false'}
    >
      <div className="pagination-summary" aria-live="polite">
        <strong>{formatNumber(normalizedTotalCount)} {itemNoun}</strong>
        <span>
          {normalizedTotalCount
            ? `第 ${formatNumber(range.from)}–${formatNumber(range.to)} 条 · 第 ${formatNumber(currentPage)} / ${formatNumber(normalizedTotalPages)} 页`
            : (emptySummary || `当前筛选没有记录`)}
        </span>
        {liveItemCount > 0 && <span className="pagination-live-count">另有 {formatNumber(liveItemCount)} {liveItemNoun}</span>}
        {loading && <span className="pagination-loading">正在读取第 {formatNumber(currentPage)} 页…</span>}
      </div>

      <div className="pagination-controls">
        <label className="pagination-page-size">
          <span>每页</span>
          {onPageSizeChange ? (
            <select
              className="form-select"
              value={normalizedPageSize}
              onChange={(event) => onPageSizeChange(Number(event.target.value))}
              aria-label="每页显示数量"
            >
              {RUNTIME_EVENT_PAGE_SIZES.map((value) => (
                <option key={value} value={value}>{value} 条</option>
              ))}
            </select>
          ) : <span className="pagination-page-size-value">{normalizedPageSize} 条</span>}
        </label>

        <form className="pagination-page-jump" onSubmit={submitPage}>
          <label htmlFor={pageInputID}>跳转到</label>
          <input
            id={pageInputID}
            className="form-input pagination-page-input"
            type="number"
            inputMode="numeric"
            min="1"
            max={pageCount}
            value={pageDraft}
            onChange={(event) => setPageDraft(event.target.value)}
            disabled={normalizedTotalPages === 0}
            aria-label={`跳转到页码，共 ${pageCount} 页`}
          />
          <span aria-hidden="true">/ {formatNumber(pageCount)}</span>
          <button type="submit" className="btn btn-secondary pagination-jump-button" disabled={normalizedTotalPages === 0}>跳转</button>
        </form>

        <div className="pagination-pages" role="group" aria-label="选择页码">
          <button
            type="button"
            className="btn btn-secondary pagination-edge"
            onClick={() => onPageChange?.(1)}
            disabled={currentPage <= 1}
            aria-label="第一页"
          >
            首页
          </button>
          <button
            type="button"
            className="btn btn-secondary pagination-edge"
            onClick={() => onPageChange?.(currentPage - 1)}
            disabled={currentPage <= 1}
            aria-label="上一页"
          >
            上一页
          </button>
          {pages.map((value, index) => (
            <React.Fragment key={value}>
              {index > 0 && value - pages[index - 1] > 1 && <span className="pagination-ellipsis" aria-hidden="true">…</span>}
              <button
                type="button"
                className={`btn ${value === currentPage ? 'btn-primary' : 'btn-secondary'} pagination-number`}
                onClick={() => onPageChange?.(value)}
                disabled={value === currentPage}
                aria-label={`第 ${value} 页`}
                aria-current={value === currentPage ? 'page' : undefined}
              >
                {value}
              </button>
            </React.Fragment>
          ))}
          <button
            type="button"
            className="btn btn-secondary pagination-edge"
            onClick={() => onPageChange?.(currentPage + 1)}
            disabled={normalizedTotalPages === 0 || currentPage >= normalizedTotalPages}
            aria-label="下一页"
          >
            下一页
          </button>
          <button
            type="button"
            className="btn btn-secondary pagination-edge"
            onClick={() => onPageChange?.(normalizedTotalPages)}
            disabled={normalizedTotalPages === 0 || currentPage >= normalizedTotalPages}
            aria-label="最后一页"
          >
            末页
          </button>
        </div>
      </div>
    </nav>
  );
}
