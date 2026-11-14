import { useState } from 'react';
import { RotateCcw, Trash2 } from 'lucide-react';
import { cn } from '../lib/utils.js';
import { endpointGroupModels, modelMatches } from '../utils/modelGroups.js';
import { Badge } from './ui/badge.jsx';
import { Button } from './ui/button.jsx';
import { Input } from './ui/input.jsx';

const SELECT_CLS = 'h-8 rounded-md border border-input bg-background px-2 text-xs text-foreground shadow-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50';

// Each model owns its selection and optional overrides in one row.
export function ModelGroupBindingEditor({ binding, models, endpoint, edit, followsLibrary = false, remove }) {
  const [query, setQuery] = useState('');
  const available = endpointGroupModels(endpoint, models);
  const all = binding.models == null;
  const selected = (name) => all || binding.models.some((pattern) => modelMatches(pattern, name));
  const inherited = (name) => !all && selected(name) && !binding.models.includes(name);
  const unavailable = (binding.models || []).filter((name) => !available.includes(name));
  const rows = available;
  const visible = rows.filter((name) => name.toLowerCase().includes(query.trim().toLowerCase()));

  const prune = (value) => {
    if (value.models != null) {
      value.overrides = (value.overrides || []).filter((item) => value.models.some((pattern) => modelMatches(pattern, item.model)));
    }
  };

  const override = (model, field, value) => edit((draft) => {
    draft.overrides ||= [];
    let item = draft.overrides.find((o) => o.model === model);
    if (!item) {
      item = { model };
      draft.overrides.push(item);
    }
    if (value === '') delete item[field];
    else item[field] = field === 'priority' ? Number(value) : value;
    draft.overrides = draft.overrides.filter((o) => o.priority != null || o.upstreamModel != null);
  });

  const summary = all
    ? `全部可用模型 · 可选 ${available.length}`
    : `已选 ${available.filter(selected).length} · 可选 ${available.length}`;

  return (
    <div className="space-y-4 border-t bg-muted/10 p-4">
      <div className="flex flex-wrap items-center gap-4">
        {/* 组内启用开关在折叠头部,不折叠也能切;这里只留展开后才需要的设置。 */}
        <label className="flex items-center gap-2 text-xs">
          <span className="font-medium text-muted-foreground">默认优先级</span>
          {followsLibrary ? (
            <Badge variant="outline" className="font-normal text-[11px]" title="默认组的顺序与优先级保存时自动跟随入口库">
              跟随入口库
            </Badge>
          ) : (
            <Input
              className="h-8 w-20 font-mono text-xs tabular-nums"
              type="number"
              min="0"
              step="1"
              aria-label="入口优先级"
              title="数字越小越优先"
              value={binding.priority}
              onChange={(e) => edit((value) => { value.priority = Number(e.target.value); })}
            />
          )}
        </label>
        <label className="flex items-center gap-2 text-xs">
          <span className="font-medium text-muted-foreground">承接范围</span>
          <select
            className={SELECT_CLS}
            value={all ? 'all' : 'selected'}
            onChange={(e) => edit((value) => {
              value.models = e.target.value === 'all' ? null : available;
              prune(value);
            })}
          >
            <option value="selected">指定模型</option>
            <option value="all">全部可用模型</option>
          </select>
        </label>
      </div>

      {endpoint?.enabled === false && (
        <p className="rounded-lg border border-status-warning/35 bg-status-warning/10 px-3 py-2 text-xs text-status-warning">
          入口库已停用，此处设置保留，启用入口后才会参与调度。
        </p>
      )}
      {all ? (
        <p className="text-[11px] text-muted-foreground">
          仅承接本入口已添加的组内模型；新增可用模型会自动纳入，移除入口模型后立即停止承接新请求。
        </p>
      ) : (
        <p className="text-[11px] text-muted-foreground">
          指定模型是手动白名单；入口新增模型不会自动加入承接范围。
        </p>
      )}

      <section className="space-y-3 rounded-xl border bg-card p-3.5" aria-label="模型与覆盖">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <div className="flex items-center gap-2 text-xs">
              <strong className="font-semibold text-foreground">模型与覆盖</strong>
              <span className="text-muted-foreground tabular-nums">{summary}</span>
            </div>
            <p className="text-[11px] text-muted-foreground">覆盖留空时继承入口设置。</p>
          </div>
          <div className="flex items-center gap-1.5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs"
              onClick={() => edit((value) => { value.models = available; prune(value); })}
            >
              全选可用
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs"
              onClick={() => edit((value) => { value.models = []; value.overrides = []; })}
            >
              清空
            </Button>
          </div>
        </div>

        {(rows.length > 8 || query) && (
          <Input
            className="h-8 text-xs"
            aria-label="搜索入口模型"
            placeholder="搜索模型"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        )}

        <div className="divide-y rounded-lg border">
          <div className="hidden grid-cols-[minmax(0,1fr)_minmax(0,180px)_110px_36px] gap-2 bg-muted/40 px-3 py-1.5 text-[11px] font-medium text-muted-foreground md:grid" aria-hidden="true">
            <span>模型</span>
            <span>上游模型名称</span>
            <span>优先级</span>
            <span />
          </div>
          {visible.map((name) => {
            const checked = selected(name);
            const covered = inherited(name);
            const item = binding.overrides?.find((o) => o.model === name);
            return (
              <div
                key={name}
                className="grid gap-2 px-3 py-2 text-xs md:grid-cols-[minmax(0,1fr)_minmax(0,180px)_110px_36px] md:items-center"
              >
                <label className="flex cursor-pointer items-center gap-2 min-w-0">
                  <input
                    type="checkbox"
                    className="size-3.5 accent-primary"
                    checked={checked}
                    disabled={all || covered}
                    onChange={(e) => edit((value) => {
                      value.models = e.target.checked
                        ? [...new Set([...value.models, name])]
                        : value.models.filter((model) => model !== name);
                      prune(value);
                    })}
                  />
                  <span className="flex min-w-0 flex-wrap items-center gap-1.5">
                    <strong className="truncate font-mono text-xs font-medium text-foreground">{name}</strong>
                    {covered ? (
                      <small className="text-[10px] text-muted-foreground">由已选通配符承接</small>
                    ) : !available.includes(name) ? (
                      <small className="text-[10px] text-status-warning">未在此入口添加</small>
                    ) : item ? (
                      <small className="rounded bg-primary/10 px-1 py-0 text-[10px] text-primary">已设置覆盖</small>
                    ) : null}
                  </span>
                </label>

                {name.includes('*') ? (
                  <p className="text-[11px] text-muted-foreground md:col-span-3">
                    通配符沿用入口映射；具体模型可设置单独覆盖。
                  </p>
                ) : !checked ? (
                  <p className="text-[11px] text-muted-foreground md:col-span-3">
                    选中后可设置上游名称与优先级
                  </p>
                ) : (
                  <>
                    <Input
                      className="h-7 font-mono text-xs"
                      aria-label={`${name} 上游模型名称`}
                      placeholder="继承入口映射"
                      value={item?.upstreamModel ?? ''}
                      onChange={(e) => override(name, 'upstreamModel', e.target.value)}
                    />
                    <Input
                      className="h-7 font-mono text-xs tabular-nums"
                      type="number"
                      min="0"
                      step="1"
                      aria-label={`${name} 优先级`}
                      placeholder={`继承 ${binding.priority}`}
                      value={item?.priority ?? ''}
                      onChange={(e) => override(name, 'priority', e.target.value)}
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      className="size-7"
                      aria-label={`${name} 恢复继承`}
                      title="恢复继承"
                      disabled={!item}
                      onClick={() => edit((value) => {
                        value.overrides = value.overrides.filter((o) => o.model !== name);
                      })}
                    >
                      <RotateCcw className="size-3.5" />
                    </Button>
                  </>
                )}
              </div>
            );
          })}
          {!visible.length && (
            <p className="px-3 py-6 text-center text-xs text-muted-foreground">
              {rows.length ? '没有匹配的模型。' : '暂无可选模型，请先在入口库添加模型并纳入当前组。'}
            </p>
          )}
        </div>
      </section>

      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          {!!unavailable.length && (
            <>
              <span className={cn('text-xs text-status-warning')}>
                {unavailable.length} 项历史选择不在当前可选范围，已从列表隐藏。
              </span>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-7 px-2 text-xs"
                onClick={() => edit((value) => {
                  value.models = available.filter(selected);
                  prune(value);
                })}
              >
                清理历史选择
              </Button>
            </>
          )}
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-7 gap-1 px-2 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
          onClick={remove}
        >
          <Trash2 className="size-3.5" />
          <span>移出组</span>
        </Button>
      </div>
    </div>
  );
}
