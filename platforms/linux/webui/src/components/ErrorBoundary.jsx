import { Component } from 'react';
import { RefreshCw, TriangleAlert } from 'lucide-react';
import { Button } from './ui/button.jsx';
import { cn } from '../lib/utils.js';
import { errorSummary, isChunkLoadError } from '../utils/errorBoundary.js';

// 没有边界时，任何渲染异常或分包加载失败都会让 React 卸载整棵树（整页空白）。
// scope="page" 只替换页面区域并随 resetKey（路由）复位；scope="app" 兜住外壳自身的异常。
export class ErrorBoundary extends Component {
  state = { error: null, resetKey: this.props.resetKey };

  static getDerivedStateFromError(error) {
    return { error };
  }

  static getDerivedStateFromProps(props, state) {
    return props.resetKey === state.resetKey ? null : { error: null, resetKey: props.resetKey };
  }

  componentDidCatch(error, info) {
    console.error('Sumpter WebUI 渲染失败', error, info?.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    const chunk = isChunkLoadError(error);
    const fullScreen = this.props.scope === 'app';
    return (
      <div
        role="alert"
        data-slot="error-boundary"
        data-error-kind={chunk ? 'chunk' : 'render'}
        className={cn(
          'flex flex-col items-center justify-center gap-3 px-4 text-center',
          fullScreen ? 'min-h-dvh bg-background text-foreground' : 'min-h-[40vh]',
        )}
      >
        <TriangleAlert className="size-6 text-destructive" aria-hidden="true" />
        <h2 className="text-base font-semibold">{chunk ? '控制台已更新，需要刷新页面' : '页面显示出错'}</h2>
        <p className="max-w-md text-sm text-muted-foreground">
          {chunk
            ? '当前标签页加载的是旧版本界面，所需资源已被新版本替换。刷新后即可继续，已保存的配置不受影响。'
            : '界面渲染时发生异常，代理服务本身不受影响。可以重试，或刷新页面后再操作。'}
        </p>
        {!chunk && errorSummary(error) && (
          <pre className="max-w-full overflow-x-auto rounded-md border border-border bg-muted/50 px-3 py-2 text-left font-mono text-xs whitespace-pre-wrap text-muted-foreground">
            {errorSummary(error)}
          </pre>
        )}
        <div className="flex flex-wrap justify-center gap-2">
          {!chunk && (
            <Button variant="outline" size="sm" onClick={() => this.setState({ error: null })}>
              重试
            </Button>
          )}
          <Button size="sm" onClick={() => window.location.reload()}>
            <RefreshCw aria-hidden="true" />
            刷新页面
          </Button>
        </div>
      </div>
    );
  }
}
