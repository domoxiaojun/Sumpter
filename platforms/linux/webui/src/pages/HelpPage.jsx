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
    detail: '当前没有可用的 Provider 入口。先添加一个入口并保存。',
    action: '前往 Provider 添加入口',
    route: 'providers',
  },
  [ONBOARDING_STATES.NO_MAPPING]: {
    label: '无 mapping',
    detail: '入口存在，但没有启用入口声明客户端模型 mapping。',
    action: '前往 Provider 添加 mapping',
    route: 'providers',
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
  const schemaVersion = config?.schemaVersion ?? 6;
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

      <div className="help-grid">
        <section className="glass-panel help-panel">
          <div className="panel-title"><Icon name="sparkles" size={18} style={{ color: 'var(--primary)' }} /><span>快速开始</span></div>
          <div className="help-step-list">
            <HelpStep number="1" title="准备 Provider 入口">在“Provider”页添加至少一个已启用入口，填写上游地址和密钥，并配置客户端模型映射。</HelpStep>
            <HelpStep number="2" title="确认代理监听">在“运行”页确认 daemon 正在运行。当前监听地址为 <code>{listener}</code>。</HelpStep>
            <HelpStep number="3" title="连接客户端">Claude Code 使用 <code>ANTHROPIC_BASE_URL</code>；Codex 或其它 OpenAI 客户端使用 API Base。完整协议矩阵见仓库的 USAGE.md。</HelpStep>
          </div>
        </section>

        <section className="glass-panel help-panel">
          <div className="panel-title"><Icon name="server" size={18} style={{ color: 'var(--accent-cyan)' }} /><span>配置与接入</span></div>
          <div className="help-fact-list">
            <div><span>配置文件</span><code>$XDG_CONFIG_HOME/kekulv/config.json</code><small>未设置时使用 ~/.config/kekulv/config.json</small></div>
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
          <HelpQuestion question="请求返回模型未找到？">检查 Provider 入口的 mappings 是否覆盖客户端发送的模型名；代理不会拿未声明的原名盲试上游。</HelpQuestion>
          <HelpQuestion question="为什么某些能力不支持？">当前代理面向一次性 HTTP 协议适配；Realtime、WebSocket、Files、Videos 等不同生命周期能力不属于普通透传。</HelpQuestion>
          <HelpQuestion question="仍然无法判断故障在哪？">先看“运行”和“诊断”页，再带上脱敏后的请求 ID、时间和错误阶段提交 Issue；不要上传 raw 捕获。</HelpQuestion>
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
