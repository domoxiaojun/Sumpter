import { useState } from 'react';
import { copyWithToast } from '../utils/clipboard.js';
import { useApp } from '../context/AppContext.jsx';
import { formatTimestamp } from '../utils/helpers.js';
import { attributionCommand, latestCollectorEvidence } from '../utils/clientAttribution.js';

export function UnifiedAttributionPanel() {
  const { runtime } = useApp();
  const [client, setClient] = useState('claude');
  const [shell, setShell] = useState('bash');
  const [proxyURL, setProxyURL] = useState(() => window.location.origin);
  const [feedback, setFeedback] = useState('');
  const evidence = latestCollectorEvidence(runtime?.recentEvents, client);
  function commandRow(label, action) {
    let command, error;
    try { command = attributionCommand(action, client, shell, proxyURL); } catch (e) { error = e.message; }
    return <div key={action} className="attribution-command">
      <div><strong>{label}</strong>{error ? <p role="alert">{error}</p> : <pre><code>{command}</code></pre>}</div>
      <button type="button" className="btn btn-secondary" disabled={!command} aria-label={`复制${label}命令`} onClick={async () => {
        const ok = await copyWithToast(command);
        setFeedback(ok ? '命令已复制，请在客户端主机执行。' : '复制失败，请选中命令复制。');
      }}>复制命令</button>
    </div>;
  }
  return <section className="glass-panel panel-padded-stack attribution-panel">
    <h2>客户端项目归因</h2>
    <p>在<strong>运行 Claude / Grok / Gemini / Codex / pi 的电脑</strong>启用。首次安装需要 Node.js 18+ 和 bash/zsh；浏览器无法检查该电脑的安装状态。</p>
    <div className="attribution-options">
      <label>客户端 <select className="form-select" value={client} onChange={e => setClient(e.target.value)}>
        <option value="claude">Claude Code</option><option value="grok">Grok Build</option><option value="gemini">Gemini CLI</option><option value="codex">Codex CLI / TUI</option><option value="pi">pi</option><option value="all">全部客户端</option>
      </select></label>
      <label>终端 Shell <select className="form-select" value={shell} onChange={e => setShell(e.target.value)}>
        <option value="bash">bash</option><option value="zsh">zsh</option>
      </select></label>
    </div>
    <label>绑定的 Sumpter 地址 <input className="form-input" value={proxyURL} onChange={e => setProxyURL(e.target.value)} placeholder="http://127.0.0.1:57878" /></label>
    <p>请核对客户端实际连接地址，远程电脑不能使用服务器的 127.0.0.1。地址不匹配或无法确认时跳过补充采集；pi 以 provider 的显式标记为准。</p>
    <div className="attribution-code" aria-label="下载安装命令">
      <pre><code>{"curl --proto '=https' --tlsv1.2 -fLo setup-client-attribution.sh https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh"}</code></pre>
    </div>
    {commandRow('首次安装并启用 / 重新启用', 'enable')}
    {commandRow('关闭归因', 'disable')}
    {commandRow('检查配置与最近启动', 'status')}
    {feedback && <p role="status">{feedback}</p>}
    <p>首次启用后新开终端并启动客户端。关闭后请重启已运行的 Claude/Grok/Gemini/Codex；新版 pi 在下一次请求读取开关。关闭仅停止脚本补充，不删除历史或原生客户端信息。</p>
    <div role="status">
      <strong>服务端请求证据</strong>
      <p>{evidence ? `最近收到 ${evidence.clientKind} · ${formatTimestamp(evidence.timestamp, { date: true })} · 脚本 ${evidence.clientDeclared.collectorVersion} · 来源 ${evidence.clientDeclared.sourceId}` : '未在已加载请求中观察到所选客户端的来源与脚本版本。'}</p>
      <p>这是此服务收到的请求，可能来自其他电脑。请与客户端 status 输出的来源 ID、版本和时间核对；没有记录不代表客户端绕过代理。</p>
    </div>
    <details>
      <summary>高级操作与使用条件</summary>
      {commandRow('更新脚本，保留开关状态', 'install')}
      <p>恢复备份可能恢复旧版归因，想停止采集请使用“关闭归因”。先运行 status 查看 restoreTarget；legacy 表示将恢复旧版配置。</p>
      {commandRow('恢复安装前配置', 'restore')}
      {commandRow('移除终端注册', 'uninstall')}
      <p>关闭保留新版包装器的连接准备；移除终端注册后 Gemini 不再由包装器准备连接，请自行保留客户端连接配置。安装前自动备份，损坏备份不阻止关闭。</p>
      <p>Gemini 需设置 SUMPTER_GEMINI_BASE_URL 和 SUMPTER_AUTH_TOKEN；关闭归因保留 URL 与认证准备。pi 需在对应 provider 添加 x-sumpter-client: pi，包装器自动加载私有配套扩展。密钥不写入归因配置。</p>
      <p>Codex 支持已有自定义 provider 和 -C / --cd，复杂项目/CLI 配置无法确认时跳过补充。桌面 App 不加载终端包装器。Grok 需确认 GROK_CONFIG 的两个请求端点均指向绑定地址。</p>
      <p>工作区路径发送到 Sumpter 后先生成项目身份、再脱敏显示；Git remote 先去掉凭据，私有头不发往上游。启动记录只代表包装器准备结果，不等同于已收到请求。</p>
      <p>需使用当前运行服务的配套资源时，可从该服务 /__sumpter/setup-client-attribution.sh 获取 setup，并设置 SUMPTER_BASE_URL；启用认证时还需设置 SUMPTER_AUTH_TOKEN。setup 自动下载同版本的 launcher 与 pi 扩展。</p>
    </details>
  </section>;
}
