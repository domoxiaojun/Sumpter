import React, { useState, useEffect } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { StatusBadge } from '../components/StatusBadge.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { Icon } from '../utils/icons.jsx';
import { api } from '../services/api.js';
import {
  clone, isLoopback, parseList,
  ccAttributionState, CC_ATTRIBUTION_STATE, CC_ATTRIBUTION_GUIDE,
} from '../utils/helpers.js';
import { copyWithToast } from '../utils/clipboard.js';

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
              {autostart?.reason || autostart?.unit || 'kekulv.service'}
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

      <CCAttributionGuidePanel />
    </div>
  );
}

// Claude Code 项目归因引导。
//
// Linux WebUI 可能从另一台机器打开；浏览器无法确认当前操作者就是运行 Claude Code
// 的 Linux 用户，所以主操作只负责一次复制完整命令，不让 daemon 冒充用户修改 shell。
// 统计观察状态与命令执行结果也必须分开，避免把“没有流量”误判成“没有配置”。
function CCAttributionGuidePanel() {
  const { runtimeAnalytics, addToast } = useApp();
  const [advancedExpanded, setAdvancedExpanded] = useState(false);
  const [copying, setCopying] = useState(false);
  const [copyFeedback, setCopyFeedback] = useState('');
  // analytics.projects 的行自带 clientKinds,比全局 facets 精确;两者都给,内核自己挑。
  // 数据没到(没登录完/刚启动)时内核返回 unknown,面板照常渲染引导,不阻塞页面。
  const state = ccAttributionState(
    runtimeAnalytics?.projects,
    runtimeAnalytics?.facets?.clientKinds,
  );
  const badgeKind = state === CC_ATTRIBUTION_STATE.configured
    ? 'good'
    : (state === CC_ATTRIBUTION_STATE.unconfigured ? 'warning' : 'muted');
  const attributionCommand = CC_ATTRIBUTION_GUIDE.steps
    .map((step) => step.command)
    .filter(Boolean)
    .join('\n');

  const handleCopySetup = async () => {
    if (copying) return;
    setCopying(true);
    setCopyFeedback('');
    const result = await copyWithToast(attributionCommand, '配置命令', addToast);
    setCopying(false);
    if (result) setCopyFeedback('命令已复制。请先 SSH/进入运行 Claude Code 的 Linux 主机，再在该主机终端执行。');
  };

  const stateIcon = state === CC_ATTRIBUTION_STATE.configured
    ? 'check'
    : (state === CC_ATTRIBUTION_STATE.unconfigured ? 'warning' : 'activity');

  return (
    <div className="glass-panel cc-guide cc-guide-compact">
      <div className="cc-guide-head">
        <div className="panel-title">
          <Icon name="book" size={18} style={{ color: 'var(--accent-cyan)' }} />
          <span>{CC_ATTRIBUTION_GUIDE.title}</span>
        </div>
        <StatusBadge text={CC_ATTRIBUTION_GUIDE.statusLabels[state]} kind={badgeKind} />
      </div>

      <p className="cc-guide-lead">{CC_ATTRIBUTION_GUIDE.subtitle}</p>
      <div className="cc-guide-local-status" role="status">
        <Icon name={stateIcon} size={17} />
        <div>
          <strong>Linux 配置助手</strong>
          <span>WebUI 只复制命令；浏览器所在设备不等于 Claude Code 主机，不直接修改远程 shell 配置</span>
        </div>
      </div>

      <div className="cc-guide-actions">
        <button
          type="button"
          className="btn btn-primary"
          onClick={handleCopySetup}
          disabled={copying}
        >
          <Icon name={copying ? 'refresh' : 'copy'} size={15} className={copying ? 'animate-spin' : ''} />
          <span>{copying ? '正在复制…' : '一键复制配置命令'}</span>
        </button>
        <button
          type="button"
          className="btn btn-ghost"
          onClick={() => setAdvancedExpanded((expanded) => !expanded)}
          aria-expanded={advancedExpanded}
          aria-controls="cc-attribution-advanced"
        >
          <Icon
            name="chevron"
            size={14}
            style={{ transform: advancedExpanded ? 'rotate(90deg)' : 'none', transition: 'transform 180ms ease-out' }}
          />
          <span>高级说明与手动命令</span>
        </button>
      </div>
      {copyFeedback ? <p className="cc-guide-feedback" role="status">{copyFeedback}</p> : null}

      <div className="cc-guide-callout" role="note">
        <Icon name="warning" size={15} />
        <span>{CC_ATTRIBUTION_GUIDE.whereToRun}</span>
      </div>

      <div className="cc-guide-observed">
        <div className="cc-guide-observed-title">请求验证：{CC_ATTRIBUTION_GUIDE.statusLabels[state]}</div>
        <p className="cc-guide-state">{CC_ATTRIBUTION_GUIDE.statusDetails[state]}</p>
      </div>

      {advancedExpanded ? (
        <div id="cc-attribution-advanced" className="cc-guide-advanced">
          <p className="cc-guide-why">{CC_ATTRIBUTION_GUIDE.why}</p>

          <section className="cc-guide-section">
            <h4 className="cc-guide-heading">远程调用先分清三台机器</h4>
            <div className="cc-guide-machine-grid">
              {CC_ATTRIBUTION_GUIDE.remoteMachines.map((machine) => (
                <article className="cc-guide-machine" key={machine.label}>
                  <strong>{machine.label}</strong>
                  <p>{machine.detail}</p>
                </article>
              ))}
            </div>
          </section>

          <section className="cc-guide-section">
            <h4 className="cc-guide-heading">按调用拓扑执行</h4>
            <div className="cc-guide-remote-scenarios">
              {CC_ATTRIBUTION_GUIDE.remoteScenarios.map((scenario) => (
                <div className="cc-guide-remote-scenario" key={scenario.title}>
                  <strong>{scenario.title}</strong>
                  <span>{scenario.detail}</span>
                </div>
              ))}
            </div>
          </section>

          <section className="cc-guide-section">
            <h4 className="cc-guide-heading">远程执行前检查（在 Claude Code 主机运行）</h4>
            <ul className="cc-guide-checks">
              {CC_ATTRIBUTION_GUIDE.remoteChecks.map((check) => (
                <li key={check.command}>
                  <code>{check.command}</code>
                  <span>{check.note}</span>
                </li>
              ))}
            </ul>
          </section>

          <section className="cc-guide-section">
            <h4 className="cc-guide-heading">三步配完</h4>
            <ol className="cc-guide-steps">
              {CC_ATTRIBUTION_GUIDE.steps.map((step) => (
                <li key={step.title}>
                  <div className="cc-guide-step-title">{step.title}</div>
                  {step.command ? (
                    <div className="cc-guide-command">
                      <code>{step.command}</code>
                      <button
                        type="button"
                        className="btn btn-secondary btn-sm"
                        onClick={() => copyWithToast(step.command, step.title, addToast)}
                      >
                        <Icon name="copy" size={13} />
                        <span>复制</span>
                      </button>
                    </div>
                  ) : null}
                  <p className="cc-guide-note">{step.note}</p>
                </li>
              ))}
            </ol>
          </section>

          <section className="cc-guide-section">
            <h4 className="cc-guide-heading">平台 · shell 差异</h4>
            <div className="cc-guide-table-scroll">
              <table className="cc-guide-table">
                <thead>
                  <tr>
                    <th scope="col">项</th>
                    <th scope="col">macOS</th>
                    <th scope="col">Linux</th>
                  </tr>
                </thead>
                <tbody>
                  {CC_ATTRIBUTION_GUIDE.platformMatrix.map((row) => (
                    <tr key={row.label}>
                      <th scope="row">{row.label}</th>
                      <td>{row.macos}</td>
                      <td>{row.linux}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>

          <section className="cc-guide-section">
            <h4 className="cc-guide-heading">三个陷阱（都是实测踩到的）</h4>
            <ul className="cc-guide-pitfalls">
              {CC_ATTRIBUTION_GUIDE.pitfalls.map((item) => (
                <li key={item.title}>
                  <strong>{item.title}</strong>
                  <span>{item.detail}</span>
                </li>
              ))}
            </ul>
          </section>

          <section className="cc-guide-section">
            <h4 className="cc-guide-heading">出问题就回退</h4>
            <ul className="cc-guide-rollback">
              {CC_ATTRIBUTION_GUIDE.rollback.map((item) => (
                <li key={item.command}>
                  <div className="cc-guide-command">
                    <code>{item.command}</code>
                    <button
                      type="button"
                      className="btn btn-ghost btn-sm"
                      onClick={() => copyWithToast(item.command, '回退命令', addToast)}
                    >
                      <Icon name="copy" size={13} />
                      <span>复制</span>
                    </button>
                  </div>
                  <p className="cc-guide-note">{item.note}</p>
                </li>
              ))}
            </ul>
          </section>

          <p className="cc-guide-privacy">{CC_ATTRIBUTION_GUIDE.privacy}</p>
        </div>
      ) : null}
    </div>
  );
}
