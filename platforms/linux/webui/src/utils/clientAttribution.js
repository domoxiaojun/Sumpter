const clients = ['claude', 'grok', 'gemini', 'codex', 'pi', 'all'];
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
