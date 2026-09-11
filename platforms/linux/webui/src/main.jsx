import React from 'react';
import { createRoot } from 'react-dom/client';
import { AppProvider } from './context/AppContext.jsx';
import { AppContent } from './App.jsx';

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
