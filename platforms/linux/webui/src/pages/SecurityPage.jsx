import { StatusBadge } from '../components/StatusBadge.jsx';
import { Icon } from '../utils/icons.jsx';
import { UnifiedAttributionPanel } from '../components/UnifiedAttributionPanel.jsx';
import React, { useState, useEffect } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { api } from '../services/api.js';
import {
  clone, isLoopback, parseList,
} from '../utils/helpers.js';

export function SecurityPage() {
  const { config, secretStatus, saveConfig, addToast, auth } = useApp();

  // Listener draft state
  const [host, setHost] = useState(config?.listener?.host || '127.0.0.1');
  const [port, setPort] = useState(config?.listener?.port || 57878);
  const [cidrs, setCidrs] = useState((config?.listener?.allowedCIDRs || []).join(', '));
  const [inboundToken, setInboundToken] = useState('');

  // Admin password change state
  const [currentPass, setCurrentPass] = useState('');
  const [newPass, setNewPass] = useState('');
  const [confirmPass, setConfirmPass] = useState('');
  const [adminUser, setAdminUser] = useState(auth?.username || 'admin');

  // Autostart state
  const [autostart, setAutostart] = useState(null);
  const [autostartLoading, setAutostartLoading] = useState(false);

  const fetchAutostart = async () => {
    try {
      setAutostartLoading(true);
      const data = await api.getAutostart();
      setAutostart(data);
    } catch {
      // ignore
    } finally {
      setAutostartLoading(false);
    }
  };

  useEffect(() => {
    fetchAutostart();
  }, []);

  const handleToggleAutostart = async (enabled) => {
    try {
      await api.setAutostart(enabled);
      setAutostart((prev) => ({ ...prev, enabled }));
      addToast(enabled ? '已启用 systemd 自启动' : '已停用 systemd 自启动', 'success');
    } catch (err) {
      addToast(`设置自启动失败: ${err.message}`, 'error');
    }
  };

  // Save Listener
  const handleSaveListener = async () => {
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
      const nextConfig = clone(config);
      nextConfig.listener.host = host.trim();
      nextConfig.listener.port = portNum;
      nextConfig.listener.allowedCIDRs = parseList(cidrs);
      await saveConfig(nextConfig);
    } catch (err) {
      addToast(`保存监听失败: ${err.message}`, 'error');
    }
  };

  // Set / Clear Token
  const handleSetToken = async () => {
    if (!inboundToken.trim()) return addToast('请输入有效的入站 Token', 'warning');
    try {
      await saveConfig(clone(config), { inboundAuthToken: inboundToken.trim() });
      setInboundToken('');
      addToast('入站 Token 已更新', 'success');
    } catch (err) {
      addToast(`设置失败: ${err.message}`, 'error');
    }
  };

  const handleClearToken = async () => {
    if (!confirm('清除入站 Token 后，客户端无需认证即可访问代理监听。确认继续？')) return;
    try {
      await saveConfig(clone(config), { inboundAuthToken: '' });
      addToast('入站 Token 已清除', 'success');
    } catch (err) {
      addToast(`清除失败: ${err.message}`, 'error');
    }
  };

  // Change Admin Credentials
  const handleChangeAdmin = async () => {
    if (!adminUser.trim()) return addToast('用户名不能为空', 'warning');
    if (newPass.length < 12) return addToast('新密码至少需要 12 个字符', 'warning');
    if (newPass !== confirmPass) return addToast('两次输入的新密码不一致', 'warning');
    if (!currentPass) return addToast('请输入当前密码以确认本人操作', 'warning');

    try {
      await api.changeCredentials(currentPass, adminUser.trim(), newPass);
      setCurrentPass('');
      setNewPass('');
      setConfirmPass('');
      addToast('Admin 登录凭据已成功更新', 'success');
    } catch (err) {
      addToast(`更新凭据失败: ${err.message}`, 'error');
    }
  };

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
            <label className="form-label">管理员用户名</label>
            <input
              type="text"
              className="form-input"
              value={adminUser}
              onChange={(e) => setAdminUser(e.target.value)}
            />
          </div>

          <div className="grid-2col">
            <div className="form-group">
              <label className="form-label">当前密码</label>
              <input
                type="password"
                className="form-input"
                placeholder="输入当前旧密码"
                value={currentPass}
                onChange={(e) => setCurrentPass(e.target.value)}
              />
            </div>
            <div className="form-group">
              <label className="form-label">新密码 (至少12位)</label>
              <input
                type="password"
                className="form-input"
                placeholder="输入新密码"
                value={newPass}
                onChange={(e) => setNewPass(e.target.value)}
              />
            </div>
          </div>

          <div className="form-group">
            <label className="form-label">确认新密码</label>
            <input
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
          >
            <Icon name="check" size={16} />
            <span>更新登录凭据</span>
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
            <label className="form-label">设置新 Token</label>
            <input
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
              disabled={!inboundToken.trim()}
            >
              <Icon name="key" size={15} />
              <span>设置 Token</span>
            </button>
            <button
              type="button"
              className="btn btn-danger"
              onClick={handleClearToken}
              disabled={!secretStatus?.inboundAuthToken?.configured}
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
            <label className="form-label">监听地址 (Host) *</label>
            <input
              type="text"
              className="form-input"
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="127.0.0.1 或 0.0.0.0"
            />
          </div>
          <div className="form-group">
            <label className="form-label">监听端口 (Port) *</label>
            <input
              type="number"
              className="form-input"
              value={port}
              onChange={(e) => setPort(e.target.value)}
            />
          </div>
          <div className="form-group">
            <label className="form-label">允许的 IP 网段 (CIDR)</label>
            <input
              type="text"
              className="form-input"
              value={cidrs}
              onChange={(e) => setCidrs(e.target.value)}
              placeholder="如: 192.168.1.0/24, 10.0.0.0/8"
            />
          </div>
        </div>

        <button
          type="button"
          className="btn btn-primary panel-action-primary"
          onClick={handleSaveListener}
        >
          <Icon name="check" size={16} />
          <span>保存监听配置</span>
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
            disabled={autostartLoading}
          >
            <Icon name="refresh" size={14} className={autostartLoading ? 'animate-spin' : ''} />
            <span>刷新状态</span>
          </button>
        </div>

        <div className="status-setting-row">
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
            disabled={!autostart?.available || autostart?.controllable === false}
            title={autostart?.controllable === false ? '系统级服务由 root 管理' : '切换开机自启动'}
            ariaLabel="切换 systemd 开机自启动"
          />
        </div>
      </div>

      <UnifiedAttributionPanel />
    </div>
  );
}
