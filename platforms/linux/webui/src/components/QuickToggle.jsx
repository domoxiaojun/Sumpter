import React from 'react';

export function QuickToggle({ checked, onChange, disabled = false, label = '', title = '', ariaLabel = '' }) {
  const accessibleLabel = ariaLabel || title || `${label || '状态'}${checked ? '（已启用）' : '（已停用）'}`;
  return (
    <label className={`switch-control${disabled ? ' is-disabled' : ''}`} title={title}>
      <input
        type="checkbox"
        className="switch-input"
        checked={checked}
        onChange={(e) => !disabled && onChange && onChange(e.target.checked)}
        disabled={disabled}
        aria-label={accessibleLabel}
      />
      <span className="switch-track">
        <span className="switch-thumb" />
      </span>
      {label && <span className="switch-label">{label}</span>}
    </label>
  );
}
