import { ChevronRight, HelpCircle, Info, Link2 } from 'lucide-react';
import { Card, CardContent, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { useAppControls } from '../context/AppContext.jsx';
import donkeyHead from '../assets/donkey-logo.png';

const repoURL = 'https://github.com/domoxiaojun/sumpter';

export function AboutPage() {
  const { status } = useAppControls();
  const version = status?.version || '—';

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-1">
        <h1 className="flex items-center gap-2.5 text-xl font-semibold tracking-tight text-foreground">
          <Info className="size-5 text-primary" />
          <span>关于</span>
        </h1>
        <p className="text-xs text-muted-foreground">版本、许可证、作者与项目链接。</p>
      </div>

      <Card>
        <CardContent className="flex flex-col items-start gap-4 pt-1 sm:flex-row sm:items-center">
          <div
            className="flex size-16 shrink-0 items-center justify-center overflow-hidden rounded-2xl border border-border/80 bg-muted/40 p-1.5"
            aria-hidden="true"
          >
            <img src={donkeyHead} alt="" className="size-full object-contain" />
          </div>
          <div className="space-y-1">
            <h2 className="text-lg font-semibold tracking-tight text-foreground">Sumpter</h2>
            <p className="text-xs font-medium text-primary">本地多上游 AI 协议代理</p>
            <p className="text-xs leading-relaxed text-muted-foreground">
              支持 Claude Code / Codex 的协议适配、智能路由、故障转移与运行统计。
            </p>
          </div>
        </CardContent>
      </Card>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader className="pb-2">
            <div className="flex items-center gap-2">
              <Info className="size-4 text-primary" />
              <CardTitle>版本与维护信息</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="divide-y divide-border/60">
            <div className="flex items-center justify-between py-2.5 text-xs">
              <span className="text-muted-foreground">作者 / Maintainer</span>
              <strong className="font-medium text-foreground">Domo Mido</strong>
            </div>
            <div className="flex items-center justify-between py-2.5 text-xs">
              <span className="text-muted-foreground">Daemon 版本</span>
              <strong className="font-mono font-semibold text-foreground">{version}</strong>
            </div>
            <div className="flex items-center justify-between py-2.5 text-xs">
              <span className="text-muted-foreground">许可证</span>
              <strong className="font-medium text-foreground">MIT License</strong>
            </div>
            <div className="flex items-center justify-between py-2.5 text-xs">
              <span className="text-muted-foreground">管理界面</span>
              <strong className="font-medium text-foreground">Linux Web Admin</strong>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <div className="flex items-center gap-2">
              <Link2 className="size-4 text-primary" />
              <CardTitle>项目链接</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="space-y-2">
              <a
                className="flex items-center gap-3 rounded-lg border border-border/80 bg-muted/20 px-3.5 py-2.5 text-xs transition-colors hover:border-border hover:bg-muted/45"
                href={repoURL}
                target="_blank"
                rel="noreferrer"
                aria-label="打开 Sumpter GitHub 仓库"
              >
                <span className="flex size-8 shrink-0 items-center justify-center rounded-md border border-border/70 bg-background/60 text-primary" aria-hidden="true">
                  <Link2 className="size-4" />
                </span>
                <span className="min-w-0 flex-1">
                  <strong className="block font-semibold text-foreground">Sumpter GitHub 仓库</strong>
                  <small className="block text-[11px] text-muted-foreground">源代码、Issue 与版本发布</small>
                </span>
                <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
              </a>

              <a
                className="flex items-center gap-3 rounded-lg border border-border/80 bg-muted/20 px-3.5 py-2.5 text-xs transition-colors hover:border-border hover:bg-muted/45"
                href={`${repoURL}/issues`}
                target="_blank"
                rel="noreferrer"
                aria-label="打开 Sumpter GitHub Issue 反馈页面"
              >
                <span className="flex size-8 shrink-0 items-center justify-center rounded-md border border-border/70 bg-background/60 text-primary" aria-hidden="true">
                  <HelpCircle className="size-4" />
                </span>
                <span className="min-w-0 flex-1">
                  <strong className="block font-semibold text-foreground">反馈问题或提出建议</strong>
                  <small className="block text-[11px] text-muted-foreground">在 GitHub Issue 中提交反馈</small>
                </span>
                <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
              </a>
            </div>
            <p className="text-[11px] text-muted-foreground">本项目与 Anthropic、OpenAI 及其产品无隶属关系。</p>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
