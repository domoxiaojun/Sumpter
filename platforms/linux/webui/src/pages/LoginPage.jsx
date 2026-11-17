import { useEffect, useState } from 'react';
import { MeshGradient } from '@paper-design/shaders-react';
import { ArrowRight, Lock, Moon, Sun } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import donkeyHead from '../assets/donkey-logo.png';
import { Button } from '../components/ui/button.jsx';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { Input } from '../components/ui/input.jsx';
import { Label } from '../components/ui/label.jsx';

function supportsWebGL2() {
  if (typeof document === 'undefined') return false;
  try {
    return Boolean(document.createElement('canvas').getContext('webgl2'));
  } catch {
    return false;
  }
}

export function LoginPage() {
  const { login, theme, toggleTheme } = useAppControls();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [retryAt, setRetryAt] = useState(0);
  const [now, setNow] = useState(Date.now());
  const [webglReady] = useState(supportsWebGL2);
  const [reducedMotion, setReducedMotion] = useState(
    () => typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches,
  );
  const waitSeconds = Math.max(0, Math.ceil((retryAt - now) / 1000));

  useEffect(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const onChange = () => setReducedMotion(mq.matches);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);

  useEffect(() => {
    if (!retryAt) return undefined;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [retryAt]);

  const submit = async (event) => {
    event.preventDefault();
    if (busy || waitSeconds > 0) return;
    if (!username.trim() || !password) return setError('请输入用户名和密码');
    setBusy(true);
    setError('');
    try {
      await login(username.trim(), password);
      setPassword('');
    } catch (loginError) {
      setError(loginError.message || '登录失败，请检查凭据和 Admin 地址');
      if (loginError.retryAfterSeconds > 0) {
        const timestamp = Date.now();
        setNow(timestamp);
        setRetryAt(timestamp + loginError.retryAfterSeconds * 1000);
      }
    } finally {
      setBusy(false);
    }
  };

  const shaderColors = theme === 'dark'
    ? ['#c26227', '#2b3554', '#182032', '#d97c38']
    : ['#f5b482', '#cbd5e1', '#f8fafc', '#ea8f53'];

  return (
    <main className="ambient-canvas-glow relative flex min-h-dvh items-center justify-center bg-background p-4 sm:p-6">
      {webglReady && !reducedMotion && (
        <span className="login-ambient-shader" aria-hidden="true">
          <MeshGradient
            className="telemetry-live-mesh"
            colors={shaderColors}
            speed={0.05}
            distortion={0.38}
            swirl={0.1}
            grainMixer={0.01}
            grainOverlay={0}
            minPixelRatio={1}
            maxPixelCount={1200000}
          />
        </span>
      )}

      <Button
        type="button"
        variant="outline"
        size="icon"
        className="fixed right-5 top-5 z-10 rounded-xl bg-card/80 backdrop-blur-md"
        onClick={toggleTheme}
        title={theme === 'dark' ? '切换为亮色主题' : '切换为暗色主题'}
        aria-label="切换主题"
      >
        {theme === 'dark' ? <Sun className="size-4" /> : <Moon className="size-4" />}
      </Button>

      <Card
        className="page-stage-enter surface-popover relative z-10 w-full max-w-[400px] border-border/80 bg-card/92 py-6 backdrop-blur-xl"
        aria-labelledby="login-title"
      >
        <CardHeader className="space-y-4 px-6 pb-1">
          <div className="flex items-center justify-between gap-3">
            <div className="flex items-center gap-3">
              <span className="flex size-11 items-center justify-center overflow-hidden rounded-2xl border border-border/85 bg-background/80 p-1.5 shadow-2xs">
                <img src={donkeyHead} alt="" className="size-full object-contain" />
              </span>
              <div className="flex flex-col gap-0.5">
                <span className="text-base font-semibold leading-tight tracking-tight text-foreground">
                  Sumpter
                </span>
                <span className="font-mono text-[10px] font-medium tracking-[0.14em] text-muted-foreground">
                  DONKEY TECH · ADMIN
                </span>
              </div>
            </div>
            <span className="inline-flex size-8 items-center justify-center rounded-xl border border-primary/20 bg-primary/10 text-primary" aria-hidden="true">
              <Lock className="size-3.5" />
            </span>
          </div>
          <div className="space-y-1 pt-1">
            <CardTitle id="login-title" className="text-xl font-semibold tracking-tight">
              登录管理台
            </CardTitle>
            <CardDescription className="text-xs">
              使用 Admin 用户名和密码管理 Linux 代理控制平面。
            </CardDescription>
          </div>
        </CardHeader>
        <CardContent className="px-6 pt-1">
          <form onSubmit={submit} className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="admin-username">用户名</Label>
              <Input
                id="admin-username"
                className="h-10"
                placeholder="输入管理员账号"
                value={username}
                autoComplete="username"
                autoCapitalize="none"
                spellCheck="false"
                onChange={(event) => setUsername(event.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="admin-password">密码</Label>
              <Input
                id="admin-password"
                className="h-10"
                type="password"
                placeholder="输入管理员密码"
                value={password}
                autoComplete="current-password"
                onChange={(event) => setPassword(event.target.value)}
              />
            </div>
            {error ? (
              <p
                role="alert"
                className="rounded-xl border border-destructive/30 bg-destructive/10 px-3.5 py-2.5 text-xs leading-relaxed text-destructive"
              >
                {error}
              </p>
            ) : null}
            <Button
              type="submit"
              size="lg"
              className="mt-1 w-full gap-2 text-sm font-semibold"
              disabled={busy || waitSeconds > 0}
              aria-busy={busy || undefined}
            >
              <span>{waitSeconds > 0 ? `请等待 ${waitSeconds} 秒` : busy ? '登录中…' : '登录控制台'}</span>
              {!busy && waitSeconds <= 0 && <ArrowRight className="size-4" />}
            </Button>
          </form>
        </CardContent>
      </Card>
    </main>
  );
}
