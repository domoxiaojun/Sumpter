import React from 'react';
import { Icon } from '../utils/icons.jsx';

export function MetricCard({ label, value, detail, icon, accent = 'var(--primary)' }) {
  return (
    <div className="metric-card" style={{ '--card-accent': accent }}>
      <div className="metric-head">
        <span className="metric-label">{label}</span>
        {icon && (
          <div className="metric-icon-box">
            <Icon name={icon} size={18} />
          </div>
        )}
      </div>
      <div className="metric-value">{value}</div>
      {detail && <div className="metric-detail">{detail}</div>}
    </div>
  );
}
