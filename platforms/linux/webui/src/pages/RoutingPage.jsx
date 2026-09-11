import React, { useRef } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { DataTable } from '../components/DataTable.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { Icon } from '../utils/icons.jsx';
import { clone } from '../utils/helpers.js';
import { sourceFormatLabel } from '../utils/protocols.js';
import { FeatureRuleEditor } from '../components/FeatureRuleEditor.jsx';
import { createRouteCatalogLoader, isBuiltinRule } from '../utils/featureRoutes.js';
import { api } from '../services/api.js';

const catalogLoaderFactory = (fetchModels) => createRouteCatalogLoader(fetchModels);

export function RoutingPage() {
  const { config, saveConfig, openModal, addToast } = useApp();
  const rules = config?.featureRules || [];
  const catalogLoaderRef = useRef(null);
  if (!catalogLoaderRef.current) catalogLoaderRef.current = catalogLoaderFactory((endpointID, options) => api.fetchProviderModels(endpointID, options));
  const catalogLoader = catalogLoaderRef.current;

  const handleToggleRule = async (ruleId, enabled) => {
    try {
      const nextConfig = clone(config);
      const targetRule = nextConfig.featureRules.find((r) => r.id === ruleId);
      if (targetRule) { targetRule.enabled = enabled; await saveConfig(nextConfig); addToast(`规则「${targetRule.name || targetRule.id}」已${enabled ? '启用' : '停用'}`, 'success'); }
    } catch (err) { addToast(`操作失败: ${err.message}`, 'error'); }
  };

  const handleDeleteRule = async (rule) => {
    if (isBuiltinRule(rule)) { addToast('内建分流规则只能调整启停和目标，不能删除', 'warning'); return; }
    if (!confirm(`确定删除分流规则「${rule.name || rule.id}」吗？`)) return;
    try { const nextConfig = clone(config); nextConfig.featureRules = nextConfig.featureRules.filter((r) => r.id !== rule.id); await saveConfig(nextConfig); addToast('分流规则已删除', 'success'); }
    catch (err) { addToast(`删除失败: ${err.message}`, 'error'); }
  };

  const openRuleEditor = (rule = null) => {
    openModal({ title: rule ? `编辑分流规则 · ${rule.name || rule.id}` : '新建智能分流规则', maxWidth: '680px', content: <FeatureRuleEditor rule={rule} catalogLoader={catalogLoader} /> });
  };

  const formatMatchSummary = (match = {}) => {
    const parts = [];
    if (match.requestKind) parts.push(`用途: ${match.requestKind}`);
    if (match.toolTypePrefix) parts.push(`工具: ${match.toolTypePrefix}`);
    if (match.modelEquals) parts.push(`模型: ${match.modelEquals}`);
    if (match.systemContains) parts.push(`Prompt包含: ${match.systemContains}`);
    return parts.join(' 且 ') || '全局默认匹配';
  };

  const formatTargetSummary = (target = {}) => {
    const endpoint = (config?.endpoints || []).find((e) => e.id === target.endpointID);
    const effort = target.effort || '跟随请求';
    const protocol = target.protocol ? sourceFormatLabel(target.protocol) : '自动协议';
    return `${endpoint ? endpoint.name : '候选序列自动选择'} ➔ ${target.model || '-'} · ${protocol} · effort: ${effort}`;
  };

  const columns = [
    {
      title: '状态',
      width: '80px',
      render: (row) => (
        <QuickToggle
          checked={row.enabled}
          onChange={(val) => handleToggleRule(row.id, val)}
          title={row.enabled ? '点击停用' : '点击启用'}
          ariaLabel={`${row.name || row.id}：${row.enabled ? '停用分流规则' : '启用分流规则'}`}
        />
      ),
    },
    {
      title: '规则名称',
      minWidth: '260px',
      render: (row) => (
        <div>
          <div style={{ display: 'flex', alignItems: 'center', gap: '7px', fontWeight: 600, color: 'var(--text-primary)' }}>
            <span>{row.name || row.id}</span>
            {isBuiltinRule(row) && <span className="route-builtin-badge">内建</span>}
          </div>
          <div style={{ fontSize: '0.75rem', color: 'var(--text-muted)' }}>{formatMatchSummary(row.match)}</div>
        </div>
      ),
    },
    {
      title: '分流 Provider 与模型',
      minWidth: '360px',
      render: (row) => (
        <span className="mono-cell" style={{ color: 'var(--primary)', fontWeight: 600 }}>
          {formatTargetSummary(row.target)}
        </span>
      ),
    },
    {
      title: '操作',
      type: 'action',
      align: 'right',
      width: '140px',
      render: (row) => (
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: '6px' }}>
          <button
            type="button"
            className="btn btn-secondary btn-compact"
            onClick={() => openRuleEditor(row)}
          >
            <Icon name="edit" size={13} />
            <span>{isBuiltinRule(row) ? '编辑目标' : '编辑'}</span>
          </button>
          {!isBuiltinRule(row) && (
            <button
              type="button"
              className="btn btn-danger btn-compact"
              onClick={() => handleDeleteRule(row)}
              aria-label={`删除分流规则「${row.name || row.id}」`}
              title={`删除分流规则「${row.name || row.id}」`}
            >
              <Icon name="trash" size={13} />
            </button>
          )}
        </div>
      ),
    },
  ];

  return (
    <div className="page-stack">
      <div className="page-header">
        <div className="page-title-group">
          <h1 className="page-title">
            <Icon name="route" size={24} style={{ color: 'var(--primary)' }} />
            <span>Claude Code 路由</span>
          </h1>
          <p className="page-subtitle">Claude Code 内部子请求分流与 effort 覆盖。</p>
        </div>
        <div className="page-actions">
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => openRuleEditor(null)}
          >
            <Icon name="plus" size={16} />
            <span>添加分流规则</span>
          </button>
        </div>
      </div>

      <div className="glass-panel">
        <div className="panel-header">
          <div className="panel-title-group">
            <div className="panel-title">Claude Code 分流规则 ({rules.length})</div>
            <span className="panel-hint">命中特征条件的请求会自动绕过主对话模型并路由至指定的目标通道。</span>
          </div>
        </div>
        <div className="panel-body panel-body-flush">
          <div className="routing-desktop-table">
            <DataTable
              columns={columns}
              data={rules}
              keyField="id"
              tableMinWidth="880px"
              emptyText="暂无配置分流规则"
              ariaLabel="Claude Code 分流规则"
            />
          </div>

          <div className="routing-mobile-list" role="list" aria-label="Claude Code 分流规则">
            {rules.length === 0 ? (
              <div className="responsive-card-empty">暂无配置分流规则</div>
            ) : rules.map((rule) => (
              <article key={rule.id} className="responsive-data-card routing-mobile-card" role="listitem">
                <div className="responsive-data-card-heading">
                  <div className="responsive-data-card-title">
                    <strong>{rule.name || rule.id}</strong>
                    {isBuiltinRule(rule) && <span className="route-builtin-badge">内建</span>}
                    <small className="mono-cell">{rule.id}</small>
                  </div>
                  <QuickToggle
                    checked={rule.enabled}
                    onChange={(value) => handleToggleRule(rule.id, value)}
                    title={rule.enabled ? '点击停用' : '点击启用'}
                    ariaLabel={`${rule.name || rule.id}：${rule.enabled ? '停用分流规则' : '启用分流规则'}`}
                  />
                </div>

                <dl className="responsive-data-card-fields">
                  <div>
                    <dt>匹配条件</dt>
                    <dd>{formatMatchSummary(rule.match)}</dd>
                  </div>
                  <div>
                    <dt>目标 Provider 与模型</dt>
                    <dd className="mono-cell responsive-data-card-primary-value">{formatTargetSummary(rule.target)}</dd>
                  </div>
                </dl>

                <div className="responsive-data-card-actions">
                  <button
                    type="button"
                    className="btn btn-secondary"
                    onClick={() => openRuleEditor(rule)}
                  >
                    <Icon name="edit" size={14} />
                    <span>{isBuiltinRule(rule) ? '编辑目标' : '编辑规则'}</span>
                  </button>
                  {!isBuiltinRule(rule) && (
                    <button
                      type="button"
                      className="btn btn-danger"
                      onClick={() => handleDeleteRule(rule)}
                      aria-label={`删除分流规则「${rule.name || rule.id}」`}
                      title={`删除分流规则「${rule.name || rule.id}」`}
                    >
                      <Icon name="trash" size={14} />
                      <span>删除</span>
                    </button>
                  )}
                </div>
              </article>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
