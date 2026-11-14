import {
  Activity,
  AlertTriangle,
  BookOpen,
  HelpCircle,
  Link2,
  Route,
  Search,
  Server,
  ShieldCheck,
  Sparkles,
  Terminal,
} from 'lucide-react';
import { Badge } from '../components/ui/badge.jsx';
import { Button } from '../components/ui/button.jsx';
import { Card, CardContent, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { useApp } from '../context/AppContext.jsx';
import { getOnboardingState, ONBOARDING_STATES } from '../utils/onboarding.js';

const repoURL = 'https://github.com/domoxiaojun/sumpter';
const docsURL = `${repoURL}/blob/main/USAGE.md`;
const issueURL = `${repoURL}/issues`;

function HelpStep({ number, title, children }) {
  return (
    <div className="flex gap-3">
      <span
        className="flex size-6 shrink-0 items-center justify-center rounded-full border border-primary/35 bg-primary/10 font-mono text-xs font-semibold text-primary"
        aria-hidden="true"
      >
        {number}
      </span>
      <div className="space-y-1">
        <h3 className="text-sm font-semibold text-foreground">{title}</h3>
        <p className="text-xs leading-relaxed text-muted-foreground">{children}</p>
      </div>
    </div>
  );
}

function HelpQuestion({ question, children }) {
  return (
    <div className="space-y-1 rounded-lg border border-border/75 bg-muted/20 p-3.5">
      <h3 className="text-xs font-semibold text-foreground">{question}</h3>
      <p className="text-xs leading-relaxed text-muted-foreground">{children}</p>
    </div>
  );
}

const onboardingCopy = {
  [ONBOARDING_STATES.NOT_STARTED]: {
    label: '未启动',
    detail: '代理还没有进入运行态。先启动 daemon，再继续配置入口。',
    action: '前往运行并启动',
    route: 'run',
  },
  [ONBOARDING_STATES.NOT_CONFIGURED]: {
    label: '未配置',
    detail: '入口库尚未配置连接。先添加一个入口并保存。',
    action: '前往入口库添加连接',
    route: 'providers',
  },
  [ONBOARDING_STATES.NO_MAPPING]: {
    label: '无可用模型',
    detail: '入口已配置，但没有启用的模型与入口绑定。请在模型组中选择模型并绑定入口。',
    action: '前往模型组配置模型',
    route: 'model-groups',
  },
  [ONBOARDING_STATES.CLIENT_NOT_CONNECTED]: {
    label: '客户端未接入',
    detail: '还没有观察到客户端请求。把 Claude Code 或 Codex 的 Base URL 指向当前代理。',
    action: '查看客户端接入说明',
    href: docsURL,
  },
  [ONBOARDING_STATES.FIRST_FAILURE]: {
    label: '首次失败',
    detail: '已收到客户端请求，但还没有成功请求。先查看请求链中的失败阶段和上游响应。',
    action: '前往运行查看请求链',
    route: 'run',
  },
  [ONBOARDING_STATES.FIRST_SUCCESS]: {
    label: '首次成功',
    detail: '已完成至少一次客户端成功请求。可以继续查看请求链和 failover 结果。',
    action: '查看成功请求链',
    route: 'run',
  },
};

export function HelpPage() {
  const { status, config, runtime, navigate } = useApp();
  const listener = status?.listener
    ? `${status.listener.host}:${status.listener.port}`
    : '127.0.0.1:57878';
  const schemaVersion = config?.schemaVersion ?? 7;
  const onboardingState = getOnboardingState({ status, config, runtime });
  const onboarding = onboardingCopy[onboardingState];

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-1">
        <h1 className="flex items-center gap-2.5 text-xl font-semibold tracking-tight text-foreground">
          <HelpCircle className="size-5 text-primary" />
          <span>帮助</span>
        </h1>
        <p className="text-xs text-muted-foreground">快速开始、客户端接入、常见问题与安全边界。</p>
      </div>

      <Card data-onboarding-state={onboardingState}>
        <CardHeader className="pb-2">
          <div className="flex flex-wrap items-center gap-2">
            <Route className="size-4 text-primary" />
            <CardTitle>当前开箱状态</CardTitle>
            <Badge variant="default">{onboarding.label}</Badge>
          </div>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-xs leading-relaxed text-muted-foreground">{onboarding.detail}</p>
          <div>
            {onboarding.href ? (
              <Button asChild size="sm">
                <a href={onboarding.href} target="_blank" rel="noreferrer">
                  <BookOpen className="size-3.5" />
                  <span>{onboarding.action}</span>
                </a>
              </Button>
            ) : (
              <Button type="button" size="sm" onClick={() => navigate(onboarding.route)}>
                <Route className="size-3.5" />
                <span>{onboarding.action}</span>
              </Button>
            )}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="pb-2">
          <div className="flex items-center gap-2">
            <ShieldCheck className="size-4 text-status-good" />
            <CardTitle>客户端项目归因</CardTitle>
          </div>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-xs leading-relaxed text-muted-foreground">
            客户端原生标记自动识别。缺少产品标记时，在“安全 → 客户端识别”为独立入站 Token 绑定客户端；项目和会话仍以请求证据为准。
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" size="sm" onClick={() => navigate('security')}>
              <ShieldCheck className="size-3.5" />
              <span>打开安全页配置</span>
            </Button>
            <Button asChild variant="secondary" size="sm">
              <a href={`${docsURL}#5-让统计识别客户端项目和会话`} target="_blank" rel="noreferrer">
                <BookOpen className="size-3.5" />
                <span>查看客户端识别说明</span>
              </a>
            </Button>
          </div>
        </CardContent>
      </Card>

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader className="pb-2">
            <div className="flex items-center gap-2">
              <Sparkles className="size-4 text-primary" />
              <CardTitle>快速开始</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="space-y-4">
            <HelpStep number="1" title="准备上游服务入口">
              在 <strong className="font-semibold text-foreground">入口库</strong> 添加地址、密钥和客户端模型映射，再到 <strong className="font-semibold text-foreground">模型组</strong> 选择已添加的模型并绑定入口；启用组和入口后保存。
            </HelpStep>
            <HelpStep number="2" title="确认代理监听">
              在 <strong className="font-semibold text-foreground">运行</strong> 页确认 daemon 正在运行。容器或进程内监听地址为 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">{listener}</code>。远程客户端应使用服务器可达地址；Docker 使用 Compose 发布的宿主机端口，不能把 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">0.0.0.0</code> 当作客户端地址。
            </HelpStep>
            <HelpStep number="3" title="连接客户端">
              Claude Code 使用 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">ANTHROPIC_BASE_URL</code>；Codex 或其它 OpenAI 客户端使用带 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">/v1</code> 的 API Base（默认 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">http://127.0.0.1:57878/v1</code>）。完整协议矩阵见仓库的 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">USAGE.md</code>。
            </HelpStep>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <div className="flex items-center gap-2">
              <Server className="size-4 text-accent-cyan" />
              <CardTitle>配置与接入</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="divide-y divide-border/60 rounded-lg border border-border/75 bg-muted/20">
              <div className="space-y-1 p-3">
                <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
                  <span className="text-muted-foreground">配置文件</span>
                  <code className="font-mono text-[11px] text-foreground">$XDG_CONFIG_HOME/sumpter/config.json</code>
                </div>
                <p className="text-[11px] text-muted-foreground">
                  普通用户未设 XDG 时用 ~/.config/sumpter/config.json；system 安装用 /var/lib/sumpter/config.json；Docker 用挂载的 /config/config.json。
                </p>
              </div>
              <div className="flex flex-wrap items-center justify-between gap-2 p-3 text-xs">
                <span className="text-muted-foreground">代理端口</span>
                <code className="font-mono text-[11px] text-foreground">http://{listener}</code>
              </div>
              <div className="flex flex-wrap items-center justify-between gap-2 p-3 text-xs">
                <span className="text-muted-foreground">配置版本</span>
                <strong className="font-mono text-xs font-semibold text-foreground">schema v{schemaVersion}</strong>
              </div>
            </div>
            <p className="flex items-start gap-2 rounded-lg border border-status-warning/35 bg-status-warning/10 p-3 text-xs text-status-warning">
              <AlertTriangle className="mt-0.5 size-4 shrink-0" />
              <span>不要把 config.json、API key、入站 Token 或诊断原文提交到 Git，也不要粘贴到公开 Issue。</span>
            </p>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader className="pb-2">
          <div className="flex items-center gap-2">
            <Search className="size-4 text-status-warning" />
            <CardTitle>常见问题</CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          <div className="grid gap-3 md:grid-cols-2">
            <HelpQuestion question="Claude Code 连不上？">
              确认 Base URL 指向当前监听地址；若启用了入站认证，客户端 Token 必须与配置完全一致。
            </HelpQuestion>
            <HelpQuestion question="请求返回模型未找到？">
              先检查入口的模型映射是否包含客户端模型；配置了模型组时，还要启用组和入口绑定，并确保模型在两者范围内。模型目录仅供发现，不自动授权路由。
            </HelpQuestion>
            <HelpQuestion question="Realtime、Files 或 Videos 失败？">
              这些能力由代理直接 relay 给上游：Realtime/Live 需在入口启用原样透传能力；Files/Videos 仍检查对应 mapping。再确认 baseURL、API key 与上游 HTTP/WebSocket 能力。代理不会在本地重建协议。
            </HelpQuestion>
            <HelpQuestion question="仍然无法判断故障在哪？">
              先看 <strong className="font-semibold text-foreground">运行</strong> 和 <strong className="font-semibold text-foreground">诊断</strong> 页，再带上脱敏后的请求 ID、时间和错误阶段提交 Issue；不要上传 raw 捕获。
            </HelpQuestion>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="pb-2">
          <div className="flex items-center gap-2">
            <Terminal className="size-4 text-primary" />
            <CardTitle>pi 客户端与项目归因</CardTitle>
          </div>
        </CardHeader>
        <CardContent className="space-y-3">
          <p className="text-xs leading-relaxed text-muted-foreground">
            在 pi 所在主机编辑 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">~/.pi/agent/models.json</code>，为 Sumpter provider 配置 API、Base URL、入站 Token 与已启用的客户端模型名，并添加 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">{'"headers": { "X-Sumpter-Client": "pi" }'}</code>。
          </p>
          <p className="text-xs leading-relaxed text-muted-foreground">
            OpenAI Responses / Chat 使用 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">/v1</code>；Anthropic 使用根地址；Gemini 使用 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">/v1beta</code>。Token 可通过 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">"apiKey": "$SUMPTER_API_KEY"</code> 引用环境变量。
          </p>
          <p className="text-xs leading-relaxed text-muted-foreground">
            客户端身份可通过安全页的 Token 绑定补充。项目仅在原生请求提供工作区时识别；未提供时保持未知。
          </p>
          <div className="flex flex-wrap items-center gap-2 pt-1">
            <Button type="button" variant="secondary" size="sm" onClick={() => navigate('security')}>
              <ShieldCheck className="size-3.5" />
              <span>前往安全页绑定客户端</span>
            </Button>
            <Button asChild variant="secondary" size="sm">
              <a href={`${docsURL}#pi-客户端`} target="_blank" rel="noreferrer">完整 pi 配置说明</a>
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardContent className="flex flex-wrap items-center justify-between gap-4 pt-1">
          <div className="space-y-1">
            <div className="flex items-center gap-2 text-sm font-semibold text-foreground">
              <Link2 className="size-4 text-primary" />
              <span>文档与反馈</span>
            </div>
            <p className="text-xs text-muted-foreground">仓库根目录的 USAGE.md 是开箱、配置和 Claude Code / Codex 接入的完整指南。</p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button asChild variant="secondary" size="sm">
              <a href={repoURL} target="_blank" rel="noreferrer">
                <Activity className="size-3.5" />
                <span>GitHub 仓库</span>
              </a>
            </Button>
            <Button asChild variant="secondary" size="sm">
              <a href={docsURL} target="_blank" rel="noreferrer">
                <BookOpen className="size-3.5" />
                <span>USAGE.md</span>
              </a>
            </Button>
            <Button asChild variant="secondary" size="sm">
              <a href={issueURL} target="_blank" rel="noreferrer">
                <AlertTriangle className="size-3.5" />
                <span>提交 Issue</span>
              </a>
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
