import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { fileURLToPath } from 'node:url';
import net from 'node:net';

function probeLocalAdminPort(port = 57879, host = '127.0.0.1', timeoutMs = 120) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ port, host });
    const done = (ok) => {
      socket.destroy();
      resolve(ok);
    };
    socket.setTimeout(timeoutMs);
    socket.once('connect', () => done(true));
    socket.once('error', () => done(false));
    socket.once('timeout', () => done(false));
  });
}

function autoMockWhenBackendOffline() {
  return {
    name: 'sumpter-auto-mock-fallback',
    apply: 'serve',
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        const url = req.url || '/';
        if ((url === '/' || url.startsWith('/?')) && !url.includes('mock=')) {
          const backendOnline = await probeLocalAdminPort(57879, '127.0.0.1');
          if (!backendOnline) {
            const separator = url.includes('?') ? '&' : '?';
            res.statusCode = 302;
            res.setHeader('Location', `${url}${separator}mock=1`);
            res.end();
            return;
          }
        }
        next();
      });
    },
  };
}

export default defineConfig({
  base: './',
  plugins: [react(), tailwindcss(), autoMockWhenBackendOffline()],
  server: {
    port: 57890,
    host: '127.0.0.1',
    proxy: {
      '/admin/api': {
        target: 'http://127.0.0.1:57879',
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: fileURLToPath(new URL('../web', import.meta.url)),
    emptyOutDir: true,
    target: 'es2022',
    sourcemap: false,
  },
});
