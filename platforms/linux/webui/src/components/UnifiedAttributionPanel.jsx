import React, { useState } from 'react';
import { copyWithToast } from '../utils/clipboard.js';

export function UnifiedAttributionPanel() {
  const [client, setClient] = useState('claude');
  const [feedback, setFeedback] = useState('');
  const commands = [
    ['检查状态', `bash setup-client-attribution.sh status ${client}`],
    ['安装配置', `bash setup-client-attribution.sh install ${client}`],
    ['还原配置', `bash setup-client-attribution.sh restore ${client}`],
  ];
  return <section className="glass-panel panel-padded-stack">
    <h2>Claude / Grok / Gemini 项目归因</h2>
    <p>在运行客户端的主机配置，需要 Node.js 18+ 与 bash/zsh。三个客户端共用项目、工作区、用户和 Git remote 归因；中文目录使用 URI 编码。</p>
    <label>客户端 <select className="form-select" value={client} onChange={(e) => setClient(e.target.value)}>
      <option value="claude">Claude Code</option><option value="grok">Grok Build</option><option value="gemini">Gemini CLI</option><option value="pi">pi</option><option value="all">全部客户端</option>
    </select></label>
    <p>自动化脚本支持一键安装、还原和状态检查，操作后自动显示结果。不带参数运行可进入交互菜单。请以普通用户在客户端主机执行；浏览器无法检查该主机的终端配置。</p>
    <p>从 Linux 安装包 scripts/setup-client-attribution.sh 获取，或设置以下环境变量后下载：</p>
    <pre><code>{'export SUMPTER_BASE_URL="http://你的代理地址:57878"\nexport SUMPTER_AUTH_TOKEN="你的代理入站 Token"\ncurl -fsS -H "Authorization: Bearer $SUMPTER_AUTH_TOKEN" "${SUMPTER_BASE_URL%/}/__sumpter/setup-client-attribution.sh" -o setup-client-attribution.sh'}</code></pre>
    <p>SUMPTER_BASE_URL 是可访问的代理根地址，请勿使用管理 API 地址。</p>
    {commands.map(([label, command]) => <div key={label} className="panel-toolbar">
      <div><strong>{label}</strong><pre><code>{command}</code></pre></div>
      <button type="button" className="btn btn-secondary" onClick={async () => {
        const ok = await copyWithToast(command);
        setFeedback(ok ? '命令已复制，请在客户端主机执行。' : '复制失败，请选中命令复制。');
      }}>复制命令</button>
    </div>)}
    {feedback && <p role="status">{feedback}</p>}
    <p>脚本自动获取配套安装器。安装前备份 rc 并替换所选客户端的旧标记块；安装、还原后新开终端。还原只处理该归因块，不覆盖后续 shell 改动。状态检查不等同于已观察到请求归因。</p>
    <p>请先将客户端连接到 Sumpter；Gemini 需设置 SUMPTER_GEMINI_BASE_URL 和 SUMPTER_AUTH_TOKEN。密钥不会写入安装块。工作区路径会发送到 Sumpter，Git remote 先删除凭据；专用 header 不发往上游。</p>
  </section>;
}
