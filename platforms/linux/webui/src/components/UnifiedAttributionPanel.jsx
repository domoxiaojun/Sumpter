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
    <h2>Claude / Grok / Gemini / pi 项目归因</h2>
    <p>在<strong>启动 Claude / Grok / Gemini / pi 的那台电脑</strong>配置，不要装到只跑本 daemon 的 Linux。需要 Node.js 18+；脚本通过 bash 执行，pi 扩展不修改终端配置。</p>
    <label>客户端 <select className="form-select" value={client} onChange={(e) => setClient(e.target.value)}>
      <option value="claude">Claude Code</option><option value="grok">Grok Build</option><option value="gemini">Gemini CLI</option><option value="pi">pi</option><option value="all">全部客户端</option>
    </select></label>
    <p>自动化脚本支持一键安装、还原和状态检查，操作后自动显示结果。不带参数运行可进入交互菜单。请以普通用户在客户端主机执行；浏览器无法检查该主机的终端配置。</p>
    <p>客户端主机从 GitHub 拉脚本。可先临时跑（不改 rc），再决定是否安装：</p>
    <pre><code>{'curl --proto "=https" --tlsv1.2 -fLo client-attribution.mjs \\\n  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/client-attribution.mjs\ncurl --proto "=https" --tlsv1.2 -fLo setup-client-attribution.sh \\\n  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh'}</code></pre>
    <p>临时运行把代理地址换成这台 daemon 的可达地址，例如 <code>node client-attribution.mjs run claude --</code>。pi 没有临时 run。无法访问 GitHub 且代理已运行时，可设 <code>SUMPTER_BASE_URL</code> 从 <code>/__sumpter/</code> 下载。</p>
    {commands.map(([label, command]) => <div key={label} className="panel-toolbar">
      <div><strong>{label}</strong><pre><code>{command}</code></pre></div>
      <button type="button" className="btn btn-secondary" onClick={async () => {
        const ok = await copyWithToast(command);
        setFeedback(ok ? '命令已复制，请在客户端主机执行。' : '复制失败，请选中命令复制。');
      }}>复制命令</button>
    </div>)}
    {feedback && <p role="status">{feedback}</p>}
    <p>脚本自动获取配套安装器与 pi 扩展。安装前备份 rc 或已有扩展；重复安装保留首次备份。还原仅恢复所选客户端的归因块或 pi 扩展，pi 原先未安装时会移除扩展。pi 操作后执行 /reload，其他客户端新开终端。状态检查不等同于已观察到请求归因。</p>
    <p>pi 的 Sumpter provider 需要设置 X-Sumpter-Client: pi；安装器不修改 provider 配置。</p>
    <p>请先将客户端连接到 Sumpter；Gemini 需设置 SUMPTER_GEMINI_BASE_URL 和 SUMPTER_AUTH_TOKEN。密钥不会写入安装块。工作区路径会发送到 Sumpter，Git remote 先删除凭据；专用 header 不发往上游。</p>
  </section>;
}
