import React, { useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';
import donkeyHead from '../assets/donkey-head.svg';

export function LoginPage() {
  const { login } = useApp();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const submit = async (event) => {
    event.preventDefault();
    if (!username.trim() || !password) return setError('请输入用户名和密码');
    setBusy(true); setError('');
    try { await login(username.trim(), password); setPassword(''); }
    catch (loginError) { setError(loginError.message || '登录失败，请检查凭据和 Admin 地址'); }
    finally { setBusy(false); }
  };

  return (
    <main className="login-shell">
      <section className="login-card" aria-labelledby="login-title">
        <div className="login-brand"><span className="brand-mark"><img src={donkeyHead} alt="" /></span><span>Sumpter</span></div>
        <h1 id="login-title">登录管理台</h1>
        <p className="page-subtitle">使用 Admin 用户名和密码管理 Linux 代理。</p>
        <form onSubmit={submit} className="login-form">
          <div className="form-group">
            <label className="form-label" htmlFor="admin-username">用户名</label>
            <input id="admin-username" className="form-input" value={username} autoComplete="username" autoCapitalize="none" spellCheck="false" onChange={(event) => setUsername(event.target.value)} />
          </div>
          <div className="form-group">
            <label className="form-label" htmlFor="admin-password">密码</label>
            <input id="admin-password" className="form-input" type="password" value={password} autoComplete="current-password" onChange={(event) => setPassword(event.target.value)} />
          </div>
          {error ? <p className="form-error" role="alert">{error}</p> : null}
          <button type="submit" className="btn btn-primary login-submit" disabled={busy} aria-busy={busy || undefined}>
            {busy ? '登录中…' : '登录'}
          </button>
        </form>
      </section>
    </main>
  );
}
