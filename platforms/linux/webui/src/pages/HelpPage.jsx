import React from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';
import { getOnboardingState, ONBOARDING_STATES } from '../utils/onboarding.js';

const repoURL = 'https://github.com/domoxiaojun/sumpter';
const docsURL = `${repoURL}/blob/main/USAGE.md`;
const issueURL = `${repoURL}/issues`;

function HelpStep({ number, title, children }) {
  return (
    <div className="help-step">
      <span className="help-step-number" aria-hidden="true">{number}</span>
      <div>
        <h3>{title}</h3>
        <p>{children}</p>
      </div>
    </div>
  );
}

function HelpQuestion({ question, children }) {
  return (
    <div className="help-question">
      <h3>{question}</h3>
      <p>{children}</p>
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
    <div className="help-about-page">
      <div className="page-header">
        <div className="page-title-group">
          <h1 className="page-title"><Icon name="help" size={24} style={{ color: 'var(--primary)' }} /><span>帮助</span></h1>
          <p className="page-subtitle">快速开始、客户端接入、常见问题与安全边界。</p>
        </div>
      </div>

      <section className="glass-panel help-panel help-onboarding" data-onboarding-state={onboardingState}>
        <div className="panel-title"><Icon name="route" size={18} style={{ color: 'var(--primary)' }} /><span>当前开箱状态</span><span className="help-state-badge">{onboarding.label}</span></div>
        <p className="help-onboarding-detail">{onboarding.detail}</p>
        <div className="help-onboarding-action">
          {onboarding.href ? (
            <a className="btn btn-primary" href={onboarding.href} target="_blank" rel="noreferrer"><Icon name="book" size={16} />{onboarding.action}</a>
          ) : (
            <button type="button" className="btn btn-primary" onClick={() => navigate(onboarding.route)}><Icon name="route" size={16} />{onboarding.action}</button>
          )}
        </div>
      </section>

      <section className="glass-panel help-panel help-attribution-entry">
        <div className="panel-title"><Icon name="shield" size={18} style={{ color: 'var(--status-good)' }} /><span>客户端项目归因</span></div>
        <p>归因安装、状态检查和还原统一在“安全”页完成。请在启动 Claude Code、Grok、Gemini、Codex 或 pi 的那台主机执行，浏览器和 daemon 主机不会代替客户端修改配置。</p>
        <div className="help-links">
          <button type="button" className="btn btn-primary" onClick={() => navigate('security')}><Icon name="shield" size={16} />打开安全页配置</button>
          <a className="btn btn-secondary" href={`${docsURL}#linuxmacos-客户端归因脚本统一安装`} target="_blank" rel="noreferrer"><Icon name="book" size={16} />查看完整安装说明</a>
        </div>
      </section>

      <div className="help-grid">
        <section className="glass-panel help-panel">
          <div className="panel-title"><Icon name="sparkles" size={18} style={{ color: 'var(--primary)' }} /><span>快速开始</span></div>
          <div className="help-step-list">
            <HelpStep number="1" title="准备上游服务入口">在“入口库”添加地址和密钥，再到“模型组”选择模型并绑定入口；启用组和入口后保存。</HelpStep>
            <HelpStep number="2" title="确认代理监听">在“运行”页确认 daemon 正在运行。当前监听地址为 <code>{listener}</code>。</HelpStep>
            <HelpStep number="3" title="连接客户端">Claude Code 使用 <code>ANTHROPIC_BASE_URL</code>；Codex 或其它 OpenAI 客户端使用带 <code>/v1</code> 的 API Base（默认 <code>http://127.0.0.1:57878/v1</code>）。完整协议矩阵见仓库的 USAGE.md。</HelpStep>
          </div>
        </section>

        <section className="glass-panel help-panel">
          <div className="panel-title"><Icon name="server" size={18} style={{ color: 'var(--accent-cyan)' }} /><span>配置与接入</span></div>
          <div className="help-fact-list">
            <div><span>配置文件</span><code>$XDG_CONFIG_HOME/sumpter/config.json</code><small>未设置时使用 ~/.config/sumpter/config.json</small></div>
            <div><span>代理端口</span><code>http://{listener}</code></div>
            <div><span>配置版本</span><strong>schema v{schemaVersion}</strong></div>
          </div>
          <p className="help-warning"><Icon name="warning" size={16} />不要把 config.json、API key、入站 Token 或诊断原文提交到 Git，也不要粘贴到公开 Issue。</p>
        </section>
      </div>

      <section className="glass-panel help-panel">
        <div className="panel-title"><Icon name="search" size={18} style={{ color: 'var(--status-warning)' }} /><span>常见问题</span></div>
        <div className="help-faq-grid">
          <HelpQuestion question="Claude Code 连不上？">确认 Base URL 指向当前监听地址；若启用了入站认证，客户端 Token 必须与配置完全一致。</HelpQuestion>
          <HelpQuestion question="请求返回模型未找到？">检查启用模型组是否声明客户端模型名，组内是否绑定了启用入口；旧配置也可检查入口 mappings。</HelpQuestion>
          <HelpQuestion question="Realtime、Files 或 Videos 失败？">这些能力由代理直接 relay 给上游：先检查 Provider 的 baseURL、API key、模型 mapping，以及上游是否开放对应 HTTP/WebSocket 能力。代理不会在本地重建协议。</HelpQuestion>
          <HelpQuestion question="仍然无法判断故障在哪？">先看“运行”和“诊断”页，再带上脱敏后的请求 ID、时间和错误阶段提交 Issue；不要上传 raw 捕获。</HelpQuestion>
        </div>
      </section>

      <section className="glass-panel help-panel">
        <div className="panel-title"><Icon name="terminal" size={18} /><span>pi 客户端与项目归因</span></div>
        <p>在 pi 所在主机编辑 <code>~/.pi/agent/models.json</code>，为 Sumpter provider 配置 API、Base URL、入站 Token 与已启用的客户端模型名，并添加 <code>{'"headers": { "X-Sumpter-Client": "pi" }'}</code>。</p>
        <p>OpenAI Responses / Chat 使用 <code>/v1</code>；Anthropic 使用根地址；Gemini 使用 <code>/v1beta</code>。Token 可通过 <code>"apiKey": "$SUMPTER_API_KEY"</code> 引用环境变量。</p>
        <p>归因包装器的安装、状态检查和还原请前往“安全”页；操作后新开终端并重新启动 pi，<code>/reload</code> 不会加载 shell 配置。运行中的 listener 仍提供 <code>/__sumpter/pi-project-attribution.ts</code>。</p>
        <p>扩展随当前项目和会话更新归因。发送请求后，在“运行”查看客户端 pi、项目和会话，再按 pi 筛选统计；暂无数据不能说明扩展未安装。</p>
        <div className="help-links">
          <button type="button" className="btn btn-secondary" onClick={() => navigate('security')}><Icon name="shield" size={16} />前往安全页安装归因</button>
          <a className="btn btn-secondary" href={`${docsURL}#pi-客户端`} target="_blank" rel="noreferrer">完整 pi 配置说明</a>
        </div>
      </section>

      <section className="glass-panel help-panel help-links-panel">
        <div>
          <div className="panel-title"><Icon name="link" size={18} style={{ color: 'var(--primary)' }} /><span>文档与反馈</span></div>
          <p>仓库根目录的 USAGE.md 是开箱、配置和 Claude Code / Codex 接入的完整指南。</p>
        </div>
        <div className="help-links">
          <a className="btn btn-secondary" href={repoURL} target="_blank" rel="noreferrer"><Icon name="activity" size={16} />GitHub 仓库</a>
          <a className="btn btn-secondary" href={docsURL} target="_blank" rel="noreferrer"><Icon name="book" size={16} />USAGE.md</a>
          <a className="btn btn-secondary" href={issueURL} target="_blank" rel="noreferrer"><Icon name="warning" size={16} />提交 Issue</a>
        </div>
      </section>
    </div>
  );
}
