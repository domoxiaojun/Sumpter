import { useRef } from 'react';
import { GitBranch, Pencil, Plus, SlidersHorizontal, Trash2 } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import { DataTable } from '../components/DataTable.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { clone } from '../utils/helpers.js';
import { sourceFormatLabel } from '../utils/protocols.js';
import { FeatureRuleEditor } from '../components/FeatureRuleEditor.jsx';
import { createRouteCatalogLoader, isBuiltinRule } from '../utils/featureRoutes.js';
import { api } from '../services/api.js';
import { RequestPolicyEditor } from '../components/RequestPolicyEditor.jsx';
import { Badge } from '../components/ui/badge.jsx';
import { Button } from '../components/ui/button.jsx';
import { Card, CardDescription, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { PageHeader } from '../components/ui/page-header.jsx';

const catalogLoaderFactory = (fetchModels) => createRouteCatalogLoader(fetchModels);

export function RoutingPage() {
  const { config, configDoc, saveConfig, openModal, addToast } = useAppControls();
  const rules = config?.featureRules || [];
  const catalogLoaderRef = useRef(null);
  if (!catalogLoaderRef.current) {
    catalogLoaderRef.current = catalogLoaderFactory((endpointID, options) => api.fetchProviderModels(endpointID, options));
  }
  const catalogLoader = catalogLoaderRef.current;

  const handleToggleRule = async (ruleId, enabled) => {
    try {
      const nextConfig = clone(config);
      const targetRule = nextConfig.featureRules.find((r) => r.id === ruleId);
      if (targetRule) {
        targetRule.enabled = enabled;
        await saveConfig(nextConfig, {}, configDoc);
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
      await saveConfig(nextConfig, {}, configDoc);
      addToast('分流规则已删除', 'success');
    } catch (err) {
      addToast(`删除失败: ${err.message}`, 'error');
    }
  };

  const openRuleEditor = (rule = null) => {
    openModal({
      title: rule ? `编辑分流规则 · ${rule.name || rule.id}` : '新建智能分流规则',
      maxWidth: '680px',
      content: <FeatureRuleEditor rule={rule} catalogLoader={catalogLoader} />,
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
      width: '88px',
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
      minWidth: '210px',
      render: (row) => (
        <div className="space-y-1">
          <div className="flex flex-wrap items-center gap-1.5 text-xs font-semibold text-foreground">
            <span>{row.name || row.id}</span>
            {isBuiltinRule(row) && (
              <Badge variant="default" className="px-1.5 py-0 text-[10px]">
                内建
              </Badge>
            )}
          </div>
          <div className="text-[11px] text-muted-foreground">{formatMatchSummary(row.match)}</div>
        </div>
      ),
    },
    {
      title: '分流 Provider 与模型',
      minWidth: '280px',
      render: (row) => (
        <span className="font-mono text-xs text-foreground/90">
          {formatTargetSummary(row.target)}
        </span>
      ),
    },
    {
      title: '操作',
      type: 'action',
      align: 'right',
      width: '132px',
      render: (row) => (
        <div className="flex justify-end gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="xs"
            onClick={() => openRuleEditor(row)}
          >
            <Pencil className="size-3" />
            <span>{isBuiltinRule(row) ? '编辑目标' : '编辑'}</span>
          </Button>
          {!isBuiltinRule(row) && (
            <Button
              type="button"
              variant="destructive"
              size="icon-sm"
              onClick={() => handleDeleteRule(row)}
              aria-label={`删除分流规则「${row.name || row.id}」`}
              title={`删除分流规则「${row.name || row.id}」`}
            >
              <Trash2 className="size-3.5" />
            </Button>
          )}
        </div>
      ),
    },
  ];

  return (
    <div className="space-y-6">
      <PageHeader
        icon={GitBranch}
        title={<span>Claude Code 路由</span>}
        description="Claude Code 内部子请求分流与 effort 覆盖。"
        actions={(
          <Button type="button" size="sm" onClick={() => openRuleEditor(null)}>
            <Plus className="size-3.5" />
            <span>添加分流规则</span>
          </Button>
        )}
      />

      <Card aria-label="全客户端请求策略">
        <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-3 space-y-0">
          <div className="space-y-1">
            <CardTitle className="flex items-center gap-2 text-base font-semibold">
              <SlidersHorizontal className="size-4 text-primary" aria-hidden="true" />
              <span>全客户端请求策略</span>
            </CardTitle>
            <CardDescription className="text-xs">
              设置全局并发上限、排队等待、重试预算、Responses 流保护和入口兼容声明。
            </CardDescription>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => openModal({ title: '请求策略', maxWidth: '760px', content: <RequestPolicyEditor /> })}
          >
            <SlidersHorizontal className="size-3.5" />
            <span>配置请求策略</span>
          </Button>
        </CardHeader>
      </Card>

      <Card className="gap-0 overflow-hidden py-0">
        <CardHeader className="border-b border-border/75 py-4">
          <CardTitle className="text-base font-semibold">
            Claude Code 分流规则 ({rules.length})
          </CardTitle>
          <CardDescription className="text-xs">
            命中特征条件的请求会自动绕过主对话模型并路由至指定的目标通道。
          </CardDescription>
        </CardHeader>
        <div>
          <div className="routing-desktop-table hidden md:block">
            <DataTable
              columns={columns}
              data={rules}
              keyField="id"
              tableMinWidth="740px"
              emptyText="暂无配置分流规则"
              ariaLabel="Claude Code 分流规则"
            />
          </div>

          <div className="routing-mobile-list space-y-3 p-4 md:hidden" role="list" aria-label="Claude Code 分流规则">
            {rules.length === 0 ? (
              <div className="py-10 text-center text-xs text-muted-foreground">暂无配置分流规则</div>
            ) : rules.map((rule) => (
              <article key={rule.id} className="space-y-3 rounded-2xl border border-border/80 bg-card p-4 shadow-2xs" role="listitem">
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0 space-y-0.5">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <strong className="text-xs font-semibold text-foreground">{rule.name || rule.id}</strong>
                      {isBuiltinRule(rule) && (
                        <Badge variant="default" className="px-1.5 py-0 text-[10px]">
                          内建
                        </Badge>
                      )}
                    </div>
                    <small className="block font-mono text-[11px] text-muted-foreground">{rule.id}</small>
                  </div>
                  <QuickToggle
                    checked={rule.enabled}
                    onChange={(value) => handleToggleRule(rule.id, value)}
                    title={rule.enabled ? '点击停用' : '点击启用'}
                    ariaLabel={`${rule.name || rule.id}：${rule.enabled ? '停用分流规则' : '启用分流规则'}`}
                  />
                </div>

                <dl className="grid gap-2 rounded-xl bg-muted/40 p-3 text-xs">
                  <div>
                    <dt className="text-[11px] text-muted-foreground">匹配条件</dt>
                    <dd className="mt-0.5 text-foreground">{formatMatchSummary(rule.match)}</dd>
                  </div>
                  <div>
                    <dt className="text-[11px] text-muted-foreground">目标 Provider 与模型</dt>
                    <dd className="mt-0.5 font-mono text-xs text-foreground">{formatTargetSummary(rule.target)}</dd>
                  </div>
                </dl>

                <div className="flex flex-wrap gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="xs"
                    onClick={() => openRuleEditor(rule)}
                  >
                    <Pencil className="size-3" />
                    <span>{isBuiltinRule(rule) ? '编辑目标' : '编辑规则'}</span>
                  </Button>
                  {!isBuiltinRule(rule) && (
                    <Button
                      type="button"
                      variant="destructive"
                      size="xs"
                      onClick={() => handleDeleteRule(rule)}
                      aria-label={`删除分流规则「${rule.name || rule.id}」`}
                      title={`删除分流规则「${rule.name || rule.id}」`}
                    >
                      <Trash2 className="size-3" />
                      <span>删除</span>
                    </Button>
                  )}
                </div>
              </article>
            ))}
          </div>
        </div>
      </Card>
    </div>
  );
}
