import { ArrowUpRight, HelpCircle, Info, Link2 } from 'lucide-react';
import { Badge } from '../components/ui/badge.jsx';
import { Card, CardContent, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { PageHeader } from '../components/ui/page-header.jsx';
import { useAppControls } from '../context/AppContext.jsx';
import donkeyHead from '../assets/donkey-logo.png';

const repoURL = 'https://github.com/domoxiaojun/sumpter';

export function AboutPage() {
  const { status } = useAppControls();
  const version = status?.version || '—';

  return (
    <div className="space-y-6">
      <PageHeader
        icon={Info}
        title={<span>关于</span>}
        description="版本、许可证、作者与项目链接。"
      />

      <Card className="relative overflow-hidden">
        <span
          aria-hidden="true"
          className="pointer-events-none absolute -top-16 -right-16 size-56 rounded-full bg-primary/10 blur-3xl"
        />
        <CardContent className="relative z-10 flex flex-col items-start gap-5 sm:flex-row sm:items-center">
          <div
            className="flex size-20 shrink-0 items-center justify-center overflow-hidden rounded-2xl border border-border/85 bg-background/90 p-2 shadow-sm"
            aria-hidden="true"
          >
            <img src={donkeyHead} alt="" className="size-full object-contain" />
          </div>
          <div className="space-y-1.5">
            <div className="flex flex-wrap items-center gap-2.5">
              <h2 className="text-xl font-semibold tracking-tight text-foreground">Sumpter</h2>
              <Badge variant="default" className="font-mono">v{version}</Badge>
            </div>
            <p className="text-xs font-semibold text-primary">本地多上游 AI 协议代理 · DONKEY TECH</p>
            <p className="max-w-xl text-xs leading-relaxed text-muted-foreground">
              支持 Claude Code / Codex 的协议适配、智能路由、故障转移与运行统计。
            </p>
          </div>
        </CardContent>
      </Card>

      <div className="stagger-cards grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader className="border-b border-border/75">
            <div className="flex items-center gap-2">
              <Info className="size-4 text-primary" />
              <CardTitle>版本与维护信息</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="divide-y divide-border/65">
            <div className="flex items-center justify-between py-3 text-xs first:pt-0 last:pb-0">
              <span className="text-muted-foreground">作者 / Maintainer</span>
              <strong className="font-medium text-foreground">Domo Mido</strong>
            </div>
            <div className="flex items-center justify-between py-3 text-xs">
              <span className="text-muted-foreground">Daemon 版本</span>
              <strong className="font-mono font-semibold text-foreground">{version}</strong>
            </div>
            <div className="flex items-center justify-between py-3 text-xs">
              <span className="text-muted-foreground">许可证</span>
              <strong className="font-medium text-foreground">MIT License</strong>
            </div>
            <div className="flex items-center justify-between py-3 text-xs last:pb-0">
              <span className="text-muted-foreground">管理界面</span>
              <strong className="font-medium text-foreground">Linux Web Admin</strong>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="border-b border-border/75">
            <div className="flex items-center gap-2">
              <Link2 className="size-4 text-primary" />
              <CardTitle>项目链接</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="space-y-3.5">
            <div className="space-y-2.5">
              <a
                className="surface-card-interactive group/link flex items-center gap-3.5 rounded-2xl border border-border/80 bg-muted/25 px-4 py-3 text-xs hover:border-border hover:bg-card"
                href={repoURL}
                target="_blank"
                rel="noreferrer"
                aria-label="打开 Sumpter GitHub 仓库"
              >
                <span className="flex size-9 shrink-0 items-center justify-center rounded-xl border border-border/75 bg-card text-primary shadow-2xs" aria-hidden="true">
                  <Link2 className="size-4" />
                </span>
                <span className="min-w-0 flex-1 space-y-0.5">
                  <strong className="block font-semibold text-foreground">Sumpter GitHub 仓库</strong>
                  <small className="block text-[11px] text-muted-foreground">源代码、Issue 与版本发布</small>
                </span>
                <ArrowUpRight className="size-4 shrink-0 text-muted-foreground transition-transform duration-150 group-hover/link:translate-x-0.5 group-hover/link:-translate-y-0.5 group-hover/link:text-primary" aria-hidden="true" />
              </a>

              <a
                className="surface-card-interactive group/link flex items-center gap-3.5 rounded-2xl border border-border/80 bg-muted/25 px-4 py-3 text-xs hover:border-border hover:bg-card"
                href={`${repoURL}/issues`}
                target="_blank"
                rel="noreferrer"
                aria-label="打开 Sumpter GitHub Issue 反馈页面"
              >
                <span className="flex size-9 shrink-0 items-center justify-center rounded-xl border border-border/75 bg-card text-primary shadow-2xs" aria-hidden="true">
                  <HelpCircle className="size-4" />
                </span>
                <span className="min-w-0 flex-1 space-y-0.5">
                  <strong className="block font-semibold text-foreground">反馈问题或提出建议</strong>
                  <small className="block text-[11px] text-muted-foreground">在 GitHub Issue 中提交反馈</small>
                </span>
                <ArrowUpRight className="size-4 shrink-0 text-muted-foreground transition-transform duration-150 group-hover/link:translate-x-0.5 group-hover/link:-translate-y-0.5 group-hover/link:text-primary" aria-hidden="true" />
              </a>
            </div>
            <p className="text-[11px] text-muted-foreground">本项目与 Anthropic、OpenAI 及其产品无隶属关系。</p>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
