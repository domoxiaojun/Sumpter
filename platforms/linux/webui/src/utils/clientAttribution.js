const clients = ['claude', 'grok', 'gemini', 'codex', 'pi', 'all'];

// Only infer a direct HTTP connection whose browser address agrees with the
// daemon's actual Admin listener. Public reverse-proxy URLs need user input.
export function defaultAttributionProxyURL(pageURL, proxyListener, adminListener) {
  try {
    const url = new URL(pageURL);
    const proxyPort = Number(proxyListener?.port);
    if (url.protocol !== 'http:' || Number(url.port || 80) !== Number(adminListener?.port)
      || !Number.isInteger(proxyPort) || proxyPort < 1 || proxyPort > 65535
      || !['/', '/index.html', '/admin', '/admin/'].includes(url.pathname)) return '';
    const host = url.hostname.replace(/^\[|\]$/gu, '').toLowerCase();
    const matches = (value) => {
      if (typeof value !== 'string') return false;
      const bound = value.replace(/^\[|\]$/gu, '').trim().toLowerCase();
      if (bound === '' || bound === '0.0.0.0') return !host.includes(':');
      if (bound === '::') return host.includes(':');
      return bound === host || (['localhost', '127.0.0.1'].includes(bound) && ['localhost', '127.0.0.1'].includes(host));
    };
    if (!matches(adminListener?.host) || !matches(proxyListener?.host)) return '';
    url.port = String(proxyPort);
    return url.origin;
  } catch { return ''; }
}

export function attributionCommand(action, client, shell, proxyURL) {
  if (!['enable', 'disable', 'status', 'install', 'restore', 'uninstall'].includes(action)
    || !clients.includes(client) || !['bash', 'zsh'].includes(shell)) throw new Error('归因操作参数无效');
  let command = `bash setup-client-attribution.sh ${action} ${client} --shell ${shell}`;
  if (['enable', 'install'].includes(action)) {
    const value = proxyURL.trim();
    let url;
    try { url = new URL(value); } catch { throw new Error('请输入有效的 Sumpter HTTP(S) 地址'); }
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash
      || /[\u0000-\u0020\u007f]/u.test(value)) throw new Error('地址不能包含凭据、空白、query 或 fragment');
    command += ` --proxy-url '${value.replaceAll("'", "'\\''")}'`;
  }
  return command;
}

export function latestCollectorEvidence(events, client, sourceId) {
  const kinds = { claude: 'claude_code', grok: 'grok_build', gemini: 'gemini_cli', codex: 'codex', pi: 'pi' };
  return (events || []).filter(event => event.kind === 'client'
    && (client === 'all' || event.clientKind === kinds[client])
    && event.clientDeclared?.sourceId && event.clientDeclared?.collectorVersion
    && (!sourceId || event.clientDeclared.sourceId === sourceId))
    .reduce((latest, event) => !latest || event.timestamp > latest.timestamp ? event : latest, null);
}
