export function configSaveMessage(code, reason) {
  const guidance = {
    generation_conflict: '配置已在其他位置更新，请重新加载最新配置后再编辑保存。',
    invalid_config: '配置内容无效，请修正后重试。',
    config_write_failed: '配置写入失败，请检查配置文件权限和磁盘空间后重试。',
    listener_rebind_failed: '监听配置未生效，请检查地址和端口占用后重试。',
    reload_failed: '配置未能应用到引擎，请检查服务状态后重新加载配置。',
    unauthorized: '管理登录已失效，请重新登录后再保存。',
    csrf_required: '管理会话校验失败，请刷新页面或重新登录后再保存。',
  }[code] || '配置保存失败，请检查服务状态后重试。';
  return reason ? `${guidance} 详情：${reason}` : guidance;
}
