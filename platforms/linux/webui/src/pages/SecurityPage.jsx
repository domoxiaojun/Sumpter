import { useEffect, useRef, useState } from 'react';
import { Activity, Check, ChevronRight, KeyRound, RefreshCw, ShieldCheck, Wrench } from 'lucide-react';
import { ClientIdentityPanel } from '../components/ClientIdentityPanel.jsx';
import { InboundTokensPanel } from '../components/InboundTokensPanel.jsx';
import { QuickToggle } from '../components/QuickToggle.jsx';
import { Button } from '../components/ui/button.jsx';
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { Input } from '../components/ui/input.jsx';
import { Label } from '../components/ui/label.jsx';
import { useAppControls } from '../context/AppContext.jsx';
import { usePendingActions } from '../hooks/usePendingActions.js';
import { useUnsavedChanges } from '../hooks/useUnsavedChanges.js';
import { cn } from '../lib/utils.js';
import { api } from '../services/api.js';
import { clone, isLoopback, parseList } from '../utils/helpers.js';

export function SecurityPage() {
  const { config, configDoc, secretStatus, saveConfig, addToast, auth } = useAppControls();
  const listenerSource = useRef(configDoc);
  const { pending, run } = usePendingActions();

  // Listener draft state
  const [host, setHost] = useState(config?.listener?.host || '127.0.0.1');
  const [port, setPort] = useState(config?.listener?.port || 57878);
  const [cidrs, setCidrs] = useState((config?.listener?.allowedCIDRs || []).join(', '));
  const [trustedProxies, setTrustedProxies] = useState((config?.listener?.trustedProxyCIDRs || []).join(', '));
  const [tlsEnabled, setTlsEnabled] = useState(Boolean(config?.listener?.tls?.enabled));
  const [httpsPort, setHttpsPort] = useState(config?.listener?.tls?.port ?? 57880);
  const [certificatePath, setCertificatePath] = useState(config?.listener?.tls?.certificatePath || '');
  const [privateKeyPath, setPrivateKeyPath] = useState(config?.listener?.tls?.privateKeyPath || '');

  // Admin password change state
  const [currentPass, setCurrentPass] = useState('');
  const [newPass, setNewPass] = useState('');
  const [confirmPass, setConfirmPass] = useState('');
  const [adminUser, setAdminUser] = useState(auth?.username || 'admin');
  const [autostartLoading, setAutostartLoading] = useState(false);
  const [savedAdminUser, setSavedAdminUser] = useState(auth?.username || 'admin');

  const listenerValues = (value) => [
    value?.host || '127.0.0.1',
    String(value?.port || 57878),
    (value?.allowedCIDRs || []).join(', '),
    (value?.trustedProxyCIDRs || []).join(', '),
    Boolean(value?.tls?.enabled),
    value?.tls?.certificatePath || '',
    value?.tls?.privateKeyPath || '',
    String(value?.tls?.port ?? 57880),
  ];
  const [listenerBaseline, setListenerBaseline] = useState(() => JSON.stringify(listenerValues(config?.listener)));
  const listenerDirty = JSON.stringify([
    host,
    String(port),
    cidrs,
    trustedProxies,
    tlsEnabled,
    certificatePath,
    privateKeyPath,
    String(httpsPort),
  ]) !== listenerBaseline;

  useUnsavedChanges(
    listenerDirty || Boolean(currentPass || newPass || confirmPass) || adminUser !== savedAdminUser,
    Boolean(pending.config || pending.admin || (pending.autostart && !autostartLoading)),
  );

  useEffect(() => {
    if (listenerDirty) return;
    listenerSource.current = configDoc;
    const values = listenerValues(config?.listener);
    setHost(values[0]);
    setPort(values[1]);
    setCidrs(values[2]);
    setTrustedProxies(values[3]);
    setTlsEnabled(values[4]);
    setCertificatePath(values[5]);
    setPrivateKeyPath(values[6]);
    setHttpsPort(values[7]);
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
    if (
      tlsEnabled
      && [certificatePath, privateKeyPath].some(
        (path) => !path.trim().startsWith('/') || /[\u0000-\u001f\u007f]/u.test(path) || new TextEncoder().encode(path.trim()).length > 4096,
      )
    ) {
      return addToast('HTTPS 证书链和私钥必须填写 daemon 所在主机的有效绝对路径', 'warning');
    }
    const tlsPortNum = Number(httpsPort);
    if (tlsEnabled && (!Number.isInteger(tlsPortNum) || tlsPortNum < 1 || tlsPortNum > 65535 || tlsPortNum === portNum)) {
      return addToast('HTTPS 端口必须为 1-65535 的整数，且不能与 HTTP 端口相同', 'warning');
    }
    if (!isLoopback(host) && !secretStatus?.inboundAuthToken?.configured && !(config?.listener?.authTokens || []).length) {
      const ok = confirm('当前未启用入站 Token。保存后代理将对外监听，局域网/公网设备可能直接访问。确认继续？');
      if (!ok) return;
    }

    try {
      const nextConfig = clone(listenerSource.current.config);
      nextConfig.listener.host = host.trim();
      nextConfig.listener.port = portNum;
      nextConfig.listener.allowedCIDRs = parseList(cidrs);
      nextConfig.listener.trustedProxyCIDRs = parseList(trustedProxies);
      if (tlsEnabled || certificatePath.trim() || privateKeyPath.trim()) {
        nextConfig.listener.tls = {
          enabled: tlsEnabled,
          port: Number.isInteger(tlsPortNum) ? tlsPortNum : 57880,
          certificatePath: certificatePath.trim(),
          privateKeyPath: privateKeyPath.trim(),
        };
      } else {
        delete nextConfig.listener.tls;
      }
      listenerSource.current = await saveConfig(nextConfig, {}, listenerSource.current);
      const values = listenerValues(listenerSource.current.config.listener);
      setHost(values[0]);
      setPort(values[1]);
      setCidrs(values[2]);
      setTrustedProxies(values[3]);
      setTlsEnabled(values[4]);
      setCertificatePath(values[5]);
      setPrivateKeyPath(values[6]);
      setHttpsPort(values[7]);
      setListenerBaseline(JSON.stringify(values));
    } catch (err) {
      addToast(`保存监听失败: ${err.message}`, 'error');
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
    <div className="space-y-6">
      <div className="flex flex-col gap-1">
        <h1 className="flex items-center gap-2.5 text-xl font-semibold tracking-tight text-foreground">
          <ShieldCheck className="size-5 text-status-good" />
          <span>安全</span>
        </h1>
        <p className="text-xs text-muted-foreground">管理客户端访问凭据、监听与控制台登录。</p>
      </div>

      <InboundTokensPanel />
      <ClientIdentityPanel />

      <details className="group rounded-xl border border-border bg-card text-card-foreground shadow-sm">
        <summary className="flex cursor-pointer list-none items-center justify-between gap-2 px-5 py-4 text-sm font-semibold text-foreground select-none hover:bg-muted/25">
          <span className="flex items-center gap-2">
            <ChevronRight className="size-4 text-muted-foreground transition-transform group-open:rotate-90" />
            <span>控制台登录凭据</span>
          </span>
          <span className="text-xs font-normal text-muted-foreground">修改管理员用户名或密码</span>
        </summary>
        <div className="space-y-4 border-t border-border/60 p-5">
          <div className="flex items-center gap-2 text-sm font-semibold text-foreground">
            <KeyRound className="size-4 text-primary" />
            <span>Admin 控制台登录凭据</span>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="security-admin-user">管理员用户名</Label>
            <Input
              disabled={Boolean(pending.admin)}
              id="security-admin-user"
              autoComplete="username"
              type="text"
              value={adminUser}
              onChange={(e) => setAdminUser(e.target.value)}
            />
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-1.5">
              <Label htmlFor="security-current-password">当前密码</Label>
              <Input
                disabled={Boolean(pending.admin)}
                id="security-current-password"
                autoComplete="current-password"
                type="password"
                placeholder="输入当前旧密码"
                value={currentPass}
                onChange={(e) => setCurrentPass(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="security-new-password">新密码 (至少12位)</Label>
              <Input
                disabled={Boolean(pending.admin)}
                id="security-new-password"
                autoComplete="new-password"
                type="password"
                placeholder="输入新密码"
                value={newPass}
                onChange={(e) => setNewPass(e.target.value)}
              />
            </div>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="security-confirm-password">确认新密码</Label>
            <Input
              disabled={Boolean(pending.admin)}
              id="security-confirm-password"
              autoComplete="new-password"
              type="password"
              placeholder="再次输入新密码"
              value={confirmPass}
              onChange={(e) => setConfirmPass(e.target.value)}
            />
          </div>

          <div>
            <Button
              type="button"
              size="sm"
              onClick={handleChangeAdmin}
              disabled={Boolean(pending.admin)}
              aria-busy={Boolean(pending.admin)}
            >
              <Check className="size-3.5" />
              <span>{pending.admin ? '更新中…' : '更新登录凭据'}</span>
            </Button>
          </div>
        </div>
      </details>

      {/* Proxy Listener Configuration */}
      <Card>
        <CardHeader className="border-b border-border/60 pb-4">
          <div className="flex items-center gap-2">
            <Activity className="size-4 text-primary" />
            <CardTitle>代理监听网络与 IP 白名单 (CIDR)</CardTitle>
          </div>
          <CardDescription>配置代理监听地址、端口、HTTPS/WSS 证书链与可信代理网段。</CardDescription>
        </CardHeader>

        <CardContent className="space-y-4">
          <div className="grid gap-4 md:grid-cols-3">
            <div className="space-y-1.5">
              <Label htmlFor="security-listener-host">监听地址 (Host) *</Label>
              <Input
                disabled={Boolean(pending.config)}
                id="security-listener-host"
                type="text"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="127.0.0.1 或 0.0.0.0"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="security-listener-port">监听端口 (Port) *</Label>
              <Input
                disabled={Boolean(pending.config)}
                id="security-listener-port"
                type="number"
                value={port}
                onChange={(e) => setPort(e.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="security-allowed-cidrs">允许的 IP 网段 (CIDR)</Label>
              <Input
                disabled={Boolean(pending.config)}
                id="security-allowed-cidrs"
                type="text"
                value={cidrs}
                onChange={(e) => setCidrs(e.target.value)}
                placeholder="如: 192.168.1.0/24, 10.0.0.0/8"
              />
            </div>
          </div>

          <div className="space-y-3 rounded-lg border border-border/70 bg-muted/20 p-3.5">
            <QuickToggle
              label="启用 HTTPS / WSS"
              checked={tlsEnabled}
              disabled={Boolean(pending.config)}
              onChange={setTlsEnabled}
            />
            {tlsEnabled && (
              <div className="grid gap-3 pt-1 md:grid-cols-3">
                <div className="space-y-1.5">
                  <Label htmlFor="security-https-port">HTTPS 端口</Label>
                  <Input
                    id="security-https-port"
                    type="number"
                    value={httpsPort}
                    disabled={Boolean(pending.config)}
                    onChange={(event) => setHttpsPort(event.target.value)}
                    placeholder="57880"
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="security-tls-cert">证书链 PEM</Label>
                  <Input
                    id="security-tls-cert"
                    value={certificatePath}
                    disabled={Boolean(pending.config)}
                    onChange={(event) => setCertificatePath(event.target.value)}
                    placeholder="证书链的绝对路径"
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="security-tls-key">私钥 PEM</Label>
                  <Input
                    id="security-tls-key"
                    value={privateKeyPath}
                    disabled={Boolean(pending.config)}
                    onChange={(event) => setPrivateKeyPath(event.target.value)}
                    placeholder="未加密私钥的绝对路径"
                  />
                </div>
              </div>
            )}
            <p className="text-xs leading-relaxed text-muted-foreground">
              HTTPS 使用独立端口，HTTP 继续可用。允许自签名，不额外检查证书信任、域名或有效期；TLS 库仍需可加载的证书及匹配私钥。文件路径属于 daemon 所在主机；客户端的证书校验由客户端决定。保存监听修改会重启监听，Admin 管理端口不受影响。
            </p>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="security-trusted-proxies">可信代理 IP / CIDR</Label>
            <Input
              disabled={Boolean(pending.config)}
              id="security-trusted-proxies"
              type="text"
              value={trustedProxies}
              onChange={(e) => setTrustedProxies(e.target.value)}
              placeholder="如: 172.18.0.5, 10.0.0.0/8"
            />
            <p className="text-xs leading-relaxed text-muted-foreground">
              事件的请求源 IP 按 <code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">X-Real-IP</code>、<code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">X-Forwarded-For</code>、连接对端的顺序取第一个合法值，不需要先登记代理。
              这里填多层代理中的已知代理，<code className="rounded bg-muted px-1 py-0.5 font-mono text-[11px]">X-Forwarded-For</code> 链会跳过它们；不影响入站认证与上面的 CIDR 白名单。
            </p>
          </div>

          <div>
            <Button
              type="button"
              size="sm"
              onClick={handleSaveListener}
              disabled={Boolean(pending.config) || !listenerDirty}
              aria-busy={Boolean(pending.config)}
            >
              <Check className="size-3.5" />
              <span>{pending.config ? '处理中…' : '保存监听配置'}</span>
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* systemd Autostart Panel */}
      <Card>
        <CardHeader className="border-b border-border/60 pb-4">
          <div className="flex items-center gap-2">
            <Wrench className="size-4 text-primary" />
            <CardTitle>systemd 自启动管理</CardTitle>
          </div>
          <CardAction>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={fetchAutostart}
              disabled={Boolean(pending.autostart)}
            >
              <RefreshCw className={cn('size-3.5', autostartLoading && 'animate-spin')} />
              <span>{pending.autostart ? '处理中…' : '刷新状态'}</span>
            </Button>
          </CardAction>
        </CardHeader>

        <CardContent className="space-y-3">
          {autostartError && (
            <p role="alert" className="rounded-lg border border-status-critical/35 bg-status-critical/10 px-3 py-2 text-xs text-status-critical">
              {autostartError}；请点击刷新状态重试。
            </p>
          )}
          <div
            className="flex flex-wrap items-center justify-between gap-4 rounded-lg border border-border/70 bg-muted/20 px-4 py-3"
            aria-busy={Boolean(pending.autostart)}
          >
            <div className="space-y-0.5">
              <strong className="block text-sm font-medium text-foreground">
                {autostart?.scope === 'system' ? 'systemd 系统级服务自启动' : 'systemd user 用户级服务开机自启动'}
              </strong>
              <span className="block font-mono text-xs text-muted-foreground">
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
        </CardContent>
      </Card>
    </div>
  );
}
