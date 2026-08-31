import React from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';

const repoURL = 'https://github.com/domoxiaojun/sumpter';

export function AboutPage() {
  const { status } = useApp();
  const version = status?.version || '—';

  return (
    <div className="help-about-page about-page">
      <div className="page-header">
        <div className="page-title-group">
          <h1 className="page-title"><Icon name="info" size={24} style={{ color: 'var(--primary)' }} /><span>关于</span></h1>
          <p className="page-subtitle">版本、许可证、作者与项目链接。</p>
        </div>
      </div>

      <section className="glass-panel about-hero">
        <div className="about-mark" aria-hidden="true"><Icon name="server" size={28} /></div>
        <div>
          <h2>Sumpter</h2>
          <p>本地多上游 AI 协议代理</p>
          <p className="about-description">支持 Claude Code / Codex 的协议适配、智能路由、故障转移与运行统计。</p>
        </div>
      </section>

      <section className="glass-panel about-panel">
        <div className="about-row"><span>作者 / Maintainer</span><strong>Domo Mido</strong></div>
        <div className="about-row"><span>Daemon 版本</span><strong className="mono-cell">{version}</strong></div>
        <div className="about-row"><span>许可证</span><strong>MIT License</strong></div>
        <div className="about-row"><span>管理界面</span><strong>Linux Web Admin</strong></div>
      </section>

      <section className="glass-panel about-panel">
        <div className="panel-title"><Icon name="link" size={18} style={{ color: 'var(--primary)' }} /><span>项目链接</span></div>
        <div className="about-links-list">
          <a className="about-link-card" href={repoURL} target="_blank" rel="noreferrer" aria-label="打开 Sumpter GitHub 仓库">
            <span className="about-link-card-icon" aria-hidden="true"><Icon name="link" size={17} /></span>
            <span><strong>Sumpter GitHub 仓库</strong><small>源代码、Issue 与版本发布</small></span>
            <Icon name="chevron" size={15} aria-hidden="true" />
          </a>
          <a className="about-link-card" href={`${repoURL}/issues`} target="_blank" rel="noreferrer" aria-label="打开 Sumpter GitHub Issue 反馈页面">
            <span className="about-link-card-icon" aria-hidden="true"><Icon name="help" size={17} /></span>
            <span><strong>反馈问题或提出建议</strong><small>在 GitHub Issue 中提交反馈</small></span>
            <Icon name="chevron" size={15} aria-hidden="true" />
          </a>
        </div>
        <p className="about-disclaimer">本项目与 Anthropic、OpenAI 及其产品无隶属关系。</p>
      </section>
    </div>
  );
}
