import { useEffect, useState } from 'react';
import { Moon, Sun } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import donkeyHead from '../assets/donkey-logo.png';
import { Button } from '../components/ui/button.jsx';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { Input } from '../components/ui/input.jsx';
import { Label } from '../components/ui/label.jsx';

export function LoginPage() {
  const { login, theme, toggleTheme } = useAppControls();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [retryAt, setRetryAt] = useState(0);
  const [now, setNow] = useState(Date.now());
  const waitSeconds = Math.max(0, Math.ceil((retryAt - now) / 1000));

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

  return (
    <main className="relative flex min-h-dvh items-center justify-center bg-background p-4">
      <div className="pointer-events-none fixed inset-0 bg-[radial-gradient(ellipse_at_top,oklch(0.705_0.185_45/0.09),transparent_55%)]" aria-hidden="true" />
      <Button
        type="button"
        variant="outline"
        size="icon-sm"
        className="fixed right-4 top-4 z-10"
        onClick={toggleTheme}
        title={theme === 'dark' ? '切换为亮色主题' : '切换为暗色主题'}
        aria-label="切换主题"
      >
        {theme === 'dark' ? <Sun className="size-4" /> : <Moon className="size-4" />}
      </Button>

      <Card className="relative z-10 w-full max-w-sm shadow-lg" aria-labelledby="login-title">
        <CardHeader className="space-y-3 pb-2">
          <div className="flex items-center gap-2.5">
            <span className="flex size-9 items-center justify-center overflow-hidden rounded-xl border bg-muted/40 p-1.5">
              <img src={donkeyHead} alt="" className="size-full object-contain" />
            </span>
            <div className="flex flex-col">
              <span className="text-sm font-semibold tracking-tight text-foreground">Sumpter</span>
              <span className="font-mono text-[10px] tracking-wider text-muted-foreground">DONKEY TECH</span>
            </div>
          </div>
          <div className="space-y-1">
            <CardTitle id="login-title" className="text-xl font-semibold tracking-tight">
              登录管理台
            </CardTitle>
            <CardDescription className="text-xs">
              使用 Admin 用户名和密码管理 Linux 代理。
            </CardDescription>
          </div>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="admin-username" className="text-xs font-medium">
                用户名
              </Label>
              <Input
                id="admin-username"
                value={username}
                autoComplete="username"
                autoCapitalize="none"
                spellCheck="false"
                onChange={(event) => setUsername(event.target.value)}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="admin-password" className="text-xs font-medium">
                密码
              </Label>
              <Input
                id="admin-password"
                type="password"
                value={password}
                autoComplete="current-password"
                onChange={(event) => setPassword(event.target.value)}
              />
            </div>
            {error ? (
              <p
                role="alert"
                className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive"
              >
                {error}
              </p>
            ) : null}
            <Button
              type="submit"
              className="w-full"
              disabled={busy || waitSeconds > 0}
              aria-busy={busy || undefined}
            >
              {waitSeconds > 0 ? `请等待 ${waitSeconds} 秒` : busy ? '登录中…' : '登录'}
            </Button>
          </form>
        </CardContent>
      </Card>
    </main>
  );
}
