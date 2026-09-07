export function piAttributionState(rows = []) {
  const piRows = rows.filter((row) => (row.clientKinds || []).includes('pi')
    && Number(row.requests ?? row.attempts ?? 0) > 0);
  if (piRows.some((row) => row.name === 'unidentified_project'
    || (row.source ?? row.projectSource) === 'missing_workspace_metadata')) return 'unattributed';
  if (piRows.some((row) => ['workspace_local', 'client_declared'].includes(row.source ?? row.projectSource))) return 'observed';
  return 'unknown';
}

export const PI_ATTRIBUTION_COPY = {
  observed: '已观察到 pi 项目归因。当前项目与会话信息来自客户端声明。',
  unattributed: '存在未归因的 pi 请求。请在 pi 所在主机加载扩展，并为 Sumpter provider 设置 X-Sumpter-Client: pi。',
  unknown: '当前视图尚无可判定的 pi 项目数据，不能据此判断扩展是否已安装。',
};
