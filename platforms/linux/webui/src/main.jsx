import React from 'react';
import { createRoot } from 'react-dom/client';
import { AppProvider } from './context/AppContext.jsx';
import { AppContent } from './App.jsx';

// 自托管字体(SIL OFL):Inter 负责拉丁/数字,Noto Sans SC 负责中文,JetBrains Mono 负责等宽。
// 三者都按 unicode-range 分片,浏览器只拉取页面实际用到的片段。
import '@fontsource-variable/inter/index.css';
import '@fontsource-variable/noto-sans-sc/index.css';
import '@fontsource-variable/jetbrains-mono/index.css';
import './styles/variables.css';
import './styles/base.css';
import './styles/components.css';
import './styles/workspace.css';
import './styles/animations.css';

const container = document.getElementById('root');
const root = createRoot(container);

root.render(
  <React.StrictMode>
    <AppProvider>
      <AppContent />
    </AppProvider>
  </React.StrictMode>
);
