import React, { useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { DataTable } from '../components/DataTable.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { Icon } from '../utils/icons.jsx';
import { clone } from '../utils/helpers.js';
import { sourceFormatLabel } from '../utils/protocols.js';

const effortOptions = [
  { value: '', label: '跟随原请求' },
  { value: 'none', label: '关闭推理（none）' },
  { value: 'auto', label: '自动（auto）' },
  { value: 'minimal', label: '最低（minimal）' },
  { value: 'low', label: '低（low）' },
  { value: 'medium', label: '中（medium）' },
  { value: 'high', label: '高（high）' },
  { value: 'xhigh', label: '很高（xhigh）' },
  { value: 'max', label: '最高（max）' },
];

const BUILTIN_RULE_IDS = new Set(['websearch', 'webfetch', 'classifier']);

function isBuiltinRule(rule) {
  return BUILTIN_RULE_IDS.has(String(rule?.id || '').toLowerCase());
}

export function RoutingPage() {
  const { config, saveConfig, openModal, addToast } = useApp();
  const rules = config?.featureRules || [];

  const handleToggleRule = async (ruleId, enabled) => {
    try {
      const nextConfig = clone(config);
      const targetRule = nextConfig.featureRules.find((r) => r.id === ruleId);
      if (targetRule) {
        targetRule.enabled = enabled;
        await saveConfig(nextConfig);
        addToast(`规则「${targetRule.name || targetRule.id}」已${enabled ? '启用' : '停用'}`, 'success');
      }
    } catch (err) {
      addToast(`操作失败: ${err.message}`, 'error');
    }
  };

  const handleDeleteRule = async (rule) => {
    if (isBuiltinRule(rule)) {
      addToast('内建分流规则只能调整启停和目标，不能删除', 'warning');
      return;
    }
    if (!confirm(`确定删除分流规则「${rule.name || rule.id}」吗？`)) return;
    try {
      const nextConfig = clone(config);
      nextConfig.featureRules = nextConfig.featureRules.filter((r) => r.id !== rule.id);
      await saveConfig(nextConfig);
      addToast('分流规则已删除', 'success');
    } catch (err) {
      addToast(`删除失败: ${err.message}`, 'error');
    }
  };

  const openRuleEditor = (rule = null) => {
    const isNew = !rule;
    const isBuiltin = isBuiltinRule(rule);
    let name = rule?.name || '';
    let requestKind = rule?.match?.requestKind || 'websearch';
    let toolTypePrefix = rule?.match?.toolTypePrefix || '';
    let modelEquals = rule?.match?.modelEquals || '';
    let systemContains = rule?.match?.systemContains || '';
    let messagesContain = rule?.match?.messagesContain || '';

    let endpointID = rule?.target?.endpointID || '';
    let targetModel = rule?.target?.model || '';
    let protocol = rule?.target?.protocol || '';
    let effort = rule?.target?.effort || '';
    let enabled = rule ? rule.enabled : true;

    const endpoints = config?.endpoints || [];

    openModal({
      title: isNew ? '新建智能分流规则' : `编辑分流规则 · ${rule.name || rule.id}`,
      maxWidth: '680px',
      content: (
        <div className="panel-stack">
          <div className="form-group">
            <label className="form-label">规则名称 *</label>
            <input
              type="text"
              className="form-input"
              defaultValue={name}
              placeholder="如：WebSearch 搜索智能劫持"
              onChange={(e) => { name = e.target.value; }}
              disabled={isBuiltin}
            />
            {isBuiltin && <span className="form-hint">内建规则的名称和匹配条件由代理协议固定，只能调整启停与分流目标。</span>}
          </div>

          {/* Match Conditions Fieldset */}
          <div
            style={{
              padding: '14px',
              borderRadius: 'var(--radius-md)',
              background: 'var(--bg-surface-glass)',
              border: '1px solid var(--border-subtle)',
              display: 'flex',
              flexDirection: 'column',
              gap: '12px',
            }}
          >
            <strong style={{ fontSize: '0.88rem', color: 'var(--primary)' }}>触发匹配特征条件</strong>

            <div className="grid-2col">
              <div className="form-group">
                <label className="form-label">请求用途 (Request Kind)</label>
                <select
                  className="form-select"
                  defaultValue={requestKind}
                  onChange={(e) => { requestKind = e.target.value; }}
                  disabled={isBuiltin}
                >
                  <option value="websearch">WebSearch (搜索)</option>
                  <option value="webfetch">WebFetch (网页抓取)</option>
                  <option value="classifier">安全分类器 (Classifier)</option>
                </select>
              </div>

              <div className="form-group">
                <label className="form-label">工具调用前缀 (Tool Type Prefix)</label>
                <input
                  type="text"
                  className="form-input"
                  defaultValue={toolTypePrefix}
                  placeholder="如：web_search 或 web_fetch"
                  onChange={(e) => { toolTypePrefix = e.target.value; }}
                  disabled={isBuiltin}
                />
              </div>
            </div>

            <div className="grid-2col">
              <div className="form-group">
                <label className="form-label">客户端模型精确匹配 (可选)</label>
                <input
                  type="text"
                  className="form-input"
                  defaultValue={modelEquals}
                  placeholder="如: claude-3-5-sonnet-20241022"
                  onChange={(e) => { modelEquals = e.target.value; }}
                  disabled={isBuiltin}
                />
              </div>

              <div className="form-group">
                <label className="form-label">System Prompt 包含文本 (可选)</label>
                <input
                  type="text"
                  className="form-input"
                  defaultValue={systemContains}
                  placeholder="匹配系统提示词中的关键字"
                  onChange={(e) => { systemContains = e.target.value; }}
                  disabled={isBuiltin}
                />
              </div>
            </div>

            <div className="form-group">
              <label className="form-label">Messages 包含文本 (可选)</label>
              <input
                type="text"
                className="form-input"
                defaultValue={messagesContain}
                placeholder="匹配消息内容中的关键字"
                onChange={(e) => { messagesContain = e.target.value; }}
                disabled={isBuiltin}
              />
            </div>

          </div>

          {/* Target Routing Destination */}
          <div
            style={{
              padding: '14px',
              borderRadius: 'var(--radius-md)',
              background: 'var(--bg-surface-glass)',
              border: '1px solid var(--border-subtle)',
              display: 'flex',
              flexDirection: 'column',
              gap: '12px',
            }}
          >
            <strong style={{ fontSize: '0.88rem', color: 'var(--status-good)' }}>分流重定向目标</strong>

            <div className="form-group">
              <label className="form-label">固定入口通道（可选）</label>
              <select
                className="form-select"
                defaultValue={endpointID}
                onChange={(e) => { endpointID = e.target.value; }}
              >
                <option value="">按入口优先级自动轮转</option>
                {endpoints.map((e) => (
                  <option key={e.id} value={e.id}>{e.name}</option>
                ))}
              </select>
            </div>

            <div className="grid-2col">
              <div className="form-group">
                <label className="form-label">目标承接模型 *</label>
                <input
                  type="text"
                  className="form-input"
                  defaultValue={targetModel}
                  placeholder="如：gpt-4o-mini 或 deepseek-chat"
                  onChange={(e) => { targetModel = e.target.value; }}
                />
              </div>

              <div className="form-group">
                <label className="form-label">目标协议（可选）</label>
                <select
                  className="form-select"
                  defaultValue={protocol}
                  onChange={(e) => { protocol = e.target.value; }}
                >
                  <option value="">自动：优先原生，必要时桥接</option>
                  <option value="anthropic">Anthropic Messages</option>
                  <option value="openai">OpenAI Chat Completions</option>
                  <option value="openai-responses">OpenAI Responses</option>
                </select>
              </div>
            </div>

            <div className="form-group">
              <label className="form-label" htmlFor="route-effort-override">Effort 覆盖</label>
              <select
                id="route-effort-override"
                className="form-select"
                defaultValue={effort}
                aria-describedby="route-effort-hint"
                onChange={(e) => { effort = e.target.value; }}
              >
                {effortOptions.map((option) => (
                  <option key={option.value || 'inherit'} value={option.value}>{option.label}</option>
                ))}
              </select>
              <span id="route-effort-hint" className="form-hint">
                仅在命中这条 Claude Code 路由时覆盖；跟随原请求不会改写客户端 effort。
              </span>
            </div>
          </div>
        </div>
      ),
      actions: [
        { label: '取消', kind: 'ghost' },
        {
          label: '保存规则',
          kind: 'primary',
          onClick: async () => {
            if (!name.trim() || !targetModel.trim()) {
              addToast('请完整填写规则名称与目标承接模型', 'warning');
              return true;
            }

            const matchObj = {
              ...(requestKind ? { requestKind } : {}),
              ...(toolTypePrefix.trim() ? { toolTypePrefix: toolTypePrefix.trim() } : {}),
              ...(modelEquals.trim() ? { modelEquals: modelEquals.trim() } : {}),
              ...(systemContains.trim() ? { systemContains: systemContains.trim() } : {}),
              ...(messagesContain.trim() ? { messagesContain: messagesContain.trim() } : {}),
            };

            const targetObj = {
              model: targetModel.trim(),
              ...(endpointID ? { endpointID } : {}),
              ...(protocol ? { protocol } : {}),
              ...(effort ? { effort } : {}),
            };

            const nextConfig = clone(config);
            nextConfig.featureRules = nextConfig.featureRules || [];

            if (isNew) {
              const newId = `rule-${Date.now().toString(36)}`;
              nextConfig.featureRules.push({
                id: newId,
                name: name.trim(),
                enabled: true,
                match: matchObj,
                target: targetObj,
              });
            } else {
              const target = nextConfig.featureRules.find((r) => r.id === rule.id);
              if (target) {
                if (!isBuiltin) {
                  target.name = name.trim();
                  target.match = matchObj;
                }
                target.target = targetObj;
              }
            }

            await saveConfig(nextConfig);
            return false;
          },
        },
      ],
    });
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
