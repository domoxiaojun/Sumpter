import { StatusBadge } from '../components/StatusBadge.jsx';
import { Icon } from '../utils/icons.jsx';
import { UnifiedAttributionPanel } from '../components/UnifiedAttributionPanel.jsx';
import { useState, useEffect, useRef } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { api } from '../services/api.js';
import { usePendingActions } from '../hooks/usePendingActions.js';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import {
  clone, isLoopback, parseList,
} from '../utils/helpers.js';

export function SecurityPage() {
  const { config, configDoc, secretStatus, saveConfig, addToast, auth } = useAppControls();
  const listenerSource = useRef(configDoc);
  const { pending, run } = usePendingActions();

  // Listener draft state
  const [host, setHost] = useState(config?.listener?.host || '127.0.0.1');
  const [port, setPort] = useState(config?.listener?.port || 57878);
  const [cidrs, setCidrs] = useState((config?.listener?.allowedCIDRs || []).join(', '));
  const [trustedProxies, setTrustedProxies] = useState((config?.listener?.trustedProxyCIDRs || []).join(', '));
  const [inboundToken, setInboundToken] = useState('');

  // Admin password change state
  const [currentPass, setCurrentPass] = useState('');
  const [newPass, setNewPass] = useState('');
  const [confirmPass, setConfirmPass] = useState('');
  const [adminUser, setAdminUser] = useState(auth?.username || 'admin');
  const [autostartLoading, setAutostartLoading] = useState(false);
  const [savedAdminUser, setSavedAdminUser] = useState(auth?.username || 'admin');
  const listenerValues = value => [value?.host || '127.0.0.1', String(value?.port || 57878), (value?.allowedCIDRs || []).join(', '), (value?.trustedProxyCIDRs || []).join(', ')];
  const [listenerBaseline, setListenerBaseline] = useState(() => JSON.stringify(listenerValues(config?.listener)));
  const listenerDirty = JSON.stringify([host, String(port), cidrs, trustedProxies]) !== listenerBaseline;
  useUnsavedChanges(listenerDirty || Boolean(inboundToken || currentPass || newPass || confirmPass) || adminUser !== savedAdminUser,
    Boolean(pending.config || pending.admin || (pending.autostart && !autostartLoading)));
  useEffect(() => {
    if (listenerDirty) return;
    listenerSource.current = configDoc;
    const values = listenerValues(config?.listener);
    setHost(values[0]); setPort(values[1]); setCidrs(values[2]); setTrustedProxies(values[3]);
    setListenerBaseline(JSON.stringify(values));
  }, [configDoc, listenerDirty]);

  // Autostart state
  const [autostart, setAutostart] = useState(null);
  const [autostartError, setAutostartError] = useState('');

  const fetchAutostart = () => run('autostart', async () => {
    try {
      setAutostartLoading(true);
      setAutostartError('');
      const data = await api.getAutostart();
      setAutostart(data);
    } catch (error) {
      setAutostartError(`读取自启动状态失败：${error.message}`);
    } finally {
      setAutostartLoading(false);
    }
  });

  useEffect(() => {
    fetchAutostart();
  }, []);

  const handleToggleAutostart = (enabled) => run('autostart', async () => {
    setAutostartError('');
    try {
      await api.setAutostart(enabled);
      setAutostart((prev) => ({ ...prev, enabled }));
      addToast(enabled ? '已启用 systemd 自启动' : '已停用 systemd 自启动', 'success');
    } catch (err) {
      setAutostartError(`设置自启动失败：${err.message}`);
      addToast(`设置自启动失败: ${err.message}`, 'error');
    }
  });

  // Save Listener
  const handleSaveListener = () => run('config', async () => {
    if (!host.trim()) {
      return addToast('监听地址不能为空', 'warning');
    }
    const portNum = Number(port);
    if (!Number.isInteger(portNum) || portNum < 1 || portNum > 65535) {
      return addToast('监听端口必须为 1-65535 的整数', 'warning');
    }

    if (!isLoopback(host) && !secretStatus?.inboundAuthToken?.configured) {
      const ok = confirm('当前未启用入站 Token。保存后代理将对外监听，局域网/公网设备可能直接访问。确认继续？');
      if (!ok) return;
    }

    try {
      const nextConfig = clone(listenerSource.current.config);
      nextConfig.listener.host = host.trim();
      nextConfig.listener.port = portNum;
      nextConfig.listener.allowedCIDRs = parseList(cidrs);
      nextConfig.listener.trustedProxyCIDRs = parseList(trustedProxies);
      listenerSource.current = await saveConfig(nextConfig, {}, listenerSource.current);
      const values = listenerValues(listenerSource.current.config.listener);
      setHost(values[0]); setPort(values[1]); setCidrs(values[2]); setTrustedProxies(values[3]);
      setListenerBaseline(JSON.stringify(values));
    } catch (err) {
      addToast(`保存监听失败: ${err.message}`, 'error');
    }
  });

  // Set / Clear Token
  const handleSetToken = () => run('config', async () => {
    if (!inboundToken.trim()) return addToast('请输入有效的入站 Token', 'warning');
    try {
      await saveConfig((latest) => latest, { inboundAuthToken: inboundToken.trim() });
      setInboundToken('');
      addToast('入站 Token 已更新', 'success');
    } catch (err) {
      addToast(`设置失败: ${err.message}`, 'error');
    }
  });

  const handleClearToken = () => run('config', async () => {
    if (!confirm('清除入站 Token 后，客户端无需认证即可访问代理监听。确认继续？')) return;
    try {
      await saveConfig((latest) => latest, { inboundAuthToken: '' });
      addToast('入站 Token 已清除', 'success');
    } catch (err) {
      addToast(`清除失败: ${err.message}`, 'error');
    }
  });

  // Change Admin Credentials
  const handleChangeAdmin = () => run('admin', async () => {
    if (!adminUser.trim()) return addToast('用户名不能为空', 'warning');
    if (newPass.length < 12) return addToast('新密码至少需要 12 个字符', 'warning');
    if (newPass !== confirmPass) return addToast('两次输入的新密码不一致', 'warning');
    if (!currentPass) return addToast('请输入当前密码以确认本人操作', 'warning');

    try {
      await api.changeCredentials(currentPass, adminUser.trim(), newPass);
      setAdminUser(adminUser.trim());
      setSavedAdminUser(adminUser.trim());
      setCurrentPass('');
      setNewPass('');
      setConfirmPass('');
      addToast('Admin 登录凭据已成功更新', 'success');
    } catch (err) {
      addToast(`更新凭据失败: ${err.message}`, 'error');
    }
  });

  return (
    <div className="page-stack">
      <div className="page-header">
        <div className="page-title-group">
          <h1 className="page-title">
            <Icon name="shield" size={24} style={{ color: 'var(--status-good)' }} />
            <span>安全</span>
          </h1>
          <p className="page-subtitle">监听、入站认证、入站方言和登录项。</p>
        </div>
      </div>

      <div className="grid-2col">
        {/* Admin Credentials Panel */}
        <div className="glass-panel panel-padded-stack">
          <div className="panel-title">
            <Icon name="key" size={18} style={{ color: 'var(--primary)' }} />
            <span>Admin 控制台登录凭据</span>
          </div>

          <div className="form-group">
            <label className="form-label" htmlFor="security-admin-user">管理员用户名</label>
            <input disabled={Boolean(pending.admin)} id="security-admin-user" autoComplete="username"
              type="text"
              className="form-input"
              value={adminUser}
              onChange={(e) => setAdminUser(e.target.value)}
            />
          </div>

          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label" htmlFor="security-current-password">当前密码</label>
              <input disabled={Boolean(pending.admin)} id="security-current-password" autoComplete="current-password"
                type="password"
                className="form-input"
                placeholder="输入当前旧密码"
                value={currentPass}
                onChange={(e) => setCurrentPass(e.target.value)}
              />
            </div>
            <div className="form-group">
              <label className="form-label" htmlFor="security-new-password">新密码 (至少12位)</label>
              <input disabled={Boolean(pending.admin)} id="security-new-password" autoComplete="new-password"
                type="password"
                className="form-input"
                placeholder="输入新密码"
                value={newPass}
                onChange={(e) => setNewPass(e.target.value)}
              />
            </div>
          </div>

          <div className="form-group">
            <label className="form-label" htmlFor="security-confirm-password">确认新密码</label>
            <input disabled={Boolean(pending.admin)} id="security-confirm-password" autoComplete="new-password"
              type="password"
              className="form-input"
              placeholder="再次输入新密码"
              value={confirmPass}
              onChange={(e) => setConfirmPass(e.target.value)}
            />
          </div>

          <button
            type="button"
            className="btn btn-primary panel-action-primary"
            onClick={handleChangeAdmin}
            disabled={Boolean(pending.admin)}
            aria-busy={Boolean(pending.admin)}
          >
            <Icon name="check" size={16} />
            <span>{pending.admin ? '更新中…' : '更新登录凭据'}</span>
          </button>
        </div>

        {/* Inbound Authentication Token */}
        <div className="glass-panel panel-padded-stack">
          <div className="panel-toolbar">
            <div className="panel-title">
              <Icon name="shield" size={18} style={{ color: 'var(--accent-cyan)' }} />
              <span>入站客户端 API Token</span>
            </div>
            <StatusBadge
              text={secretStatus?.inboundAuthToken?.configured ? `已启用 (尾号 ${secretStatus.inboundAuthToken.last4 || '****'})` : '未启用'}
              kind={secretStatus?.inboundAuthToken?.configured ? 'good' : 'warning'}
            />
          </div>

          <p className="panel-description">
            客户端（如 Claude Code / Codex）需在 Header 携带 <code>Authorization: Bearer &lt;Token&gt;</code> 访问代理。
          </p>

          <div className="form-group">
            <label className="form-label" htmlFor="security-inbound-token">设置新 Token</label>
            <input disabled={Boolean(pending.config)} id="security-inbound-token" autoComplete="off"
              type="password"
              className="form-input"
              placeholder="输入新的入站 Token"
              value={inboundToken}
              onChange={(e) => setInboundToken(e.target.value)}
            />
          </div>

          <div className="panel-actions">
            <button
              type="button"
              className="btn btn-primary"
              onClick={handleSetToken}
              disabled={pending.config || !inboundToken.trim()}
            >
              <Icon name="key" size={15} />
              <span>{pending.config ? '处理中…' : '设置 Token'}</span>
            </button>
            <button
              type="button"
              className="btn btn-danger"
              onClick={handleClearToken}
              disabled={pending.config || !secretStatus?.inboundAuthToken?.configured}
            >
              <Icon name="trash" size={15} />
              <span>清除 Token</span>
            </button>
          </div>
        </div>
      </div>

      {/* Proxy Listener Configuration */}
      <div className="glass-panel panel-padded-stack">
        <div className="panel-title">
          <Icon name="activity" size={18} style={{ color: 'var(--primary)' }} />
          <span>代理监听网络与 IP 白名单 (CIDR)</span>
        </div>

        <div className="grid-3col">
          <div className="form-group">
            <label className="form-label" htmlFor="security-listener-host">监听地址 (Host) *</label>
            <input disabled={Boolean(pending.config)} id="security-listener-host"
              type="text"
              className="form-input"
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="127.0.0.1 或 0.0.0.0"
            />
          </div>
          <div className="form-group">
            <label className="form-label" htmlFor="security-listener-port">监听端口 (Port) *</label>
            <input disabled={Boolean(pending.config)} id="security-listener-port"
              type="number"
              className="form-input"
              value={port}
              onChange={(e) => setPort(e.target.value)}
            />
          </div>
          <div className="form-group">
            <label className="form-label" htmlFor="security-allowed-cidrs">允许的 IP 网段 (CIDR)</label>
            <input disabled={Boolean(pending.config)} id="security-allowed-cidrs"
              type="text"
              className="form-input"
              value={cidrs}
              onChange={(e) => setCidrs(e.target.value)}
              placeholder="如: 192.168.1.0/24, 10.0.0.0/8"
            />
          </div>
        </div>

        <div className="form-group">
          <label className="form-label" htmlFor="security-trusted-proxies">可信代理 IP / CIDR</label>
          <input disabled={Boolean(pending.config)} id="security-trusted-proxies"
            type="text"
            className="form-input"
            value={trustedProxies}
            onChange={(e) => setTrustedProxies(e.target.value)}
            placeholder="如: 172.18.0.5, 10.0.0.0/8"
          />
          <p className="form-hint">
            事件的请求源 IP 按 <code>X-Real-IP</code>、<code>X-Forwarded-For</code>、连接对端的顺序取第一个合法值，不需要先登记代理。
            这里填多层代理中的已知代理，<code>X-Forwarded-For</code> 链会跳过它们；不影响入站认证与上面的 CIDR 白名单。
          </p>
        </div>

        <button
          type="button"
          className="btn btn-primary panel-action-primary"
          onClick={handleSaveListener}
          disabled={Boolean(pending.config) || !listenerDirty}
          aria-busy={Boolean(pending.config)}
        >
          <Icon name="check" size={16} />
          <span>{pending.config ? '处理中…' : '保存监听配置'}</span>
        </button>
      </div>

      {/* systemd Autostart Panel */}
      <div className="glass-panel panel-padded-stack">
        <div className="panel-toolbar">
          <div className="panel-title">
            <Icon name="wrench" size={18} style={{ color: 'var(--primary)' }} />
            <span>systemd 自启动管理</span>
          </div>
          <button
            type="button"
            className="btn btn-ghost"
            onClick={fetchAutostart}
            disabled={Boolean(pending.autostart)}
          >
            <Icon name="refresh" size={14} className={autostartLoading ? 'animate-spin' : ''} />
            <span>{pending.autostart ? '处理中…' : '刷新状态'}</span>
          </button>
        </div>

        {autostartError && <p role="alert" className="form-hint">{autostartError}；请点击刷新状态重试。</p>}
        <div className="status-setting-row" aria-busy={Boolean(pending.autostart)}>
          <div>
            <strong>
              {autostart?.scope === 'system' ? 'systemd 系统级服务自启动' : 'systemd user 用户级服务开机自启动'}
            </strong>
            <span>
              {autostart?.reason || autostart?.unit || 'sumpter.service'}
            </span>
          </div>
          <QuickToggle
            checked={Boolean(autostart?.enabled)}
            onChange={handleToggleAutostart}
            disabled={pending.autostart || Boolean(autostartError) || !autostart?.available || autostart?.controllable === false}
            title={autostart?.controllable === false ? '系统级服务由 root 管理' : '切换开机自启动'}
            ariaLabel="切换 systemd 开机自启动"
          />
        </div>
      </div>

      <UnifiedAttributionPanel />
    </div>
  );
}
