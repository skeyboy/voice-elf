import { defineConfig } from 'vite';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
  plugins: [
    {
      name: 'cross-origin-isolation',
      configureServer(server) {
        server.middlewares.use((request, response, next) => {
          response.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
          response.setHeader('Cross-Origin-Embedder-Policy', 'require-corp');
          response.setHeader('Permissions-Policy', 'microphone=(self), display-capture=(self)');
          setVadCacheHeaders(request.url, response);
          next();
        });
      },
      configurePreviewServer(server) {
        server.middlewares.use((request, response, next) => {
          response.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
          response.setHeader('Cross-Origin-Embedder-Policy', 'require-corp');
          response.setHeader('Permissions-Policy', 'microphone=(self), display-capture=(self)');
          setVadCacheHeaders(request.url, response);
          next();
        });
      },
    },
    sveltekit(),
  ],
  server: {
    host: '0.0.0.0',
    port: 5173,
    strictPort: false,
    proxy: {
      '/voice_elf.v1.ApiService': 'http://127.0.0.1:3001',
      '/api/admin': 'http://127.0.0.1:3002',
      '/api/setup': 'http://127.0.0.1:3002',
      '/api/runtime/dependencies': 'http://127.0.0.1:3002',
      '/api': 'http://127.0.0.1:3001',
      '/media': 'http://127.0.0.1:3001',
      '/ws': {
        target: 'ws://127.0.0.1:3001',
        ws: true,
      },
    },
  },
  preview: {
    host: '0.0.0.0',
  },
  build: {
    target: 'es2022',
  },
});

function setVadCacheHeaders(url: string | undefined, response: import('node:http').ServerResponse) {
  const path = url?.split('?', 1)[0];
  if (path === '/wasm/manifest.json') {
    response.setHeader('Cache-Control', 'no-cache');
  } else if (/^\/wasm\/voice_elf_web_vad\.[a-f0-9]{16}\.wasm$/.test(path ?? '')) {
    response.setHeader('Cache-Control', 'public, max-age=31536000, immutable');
  }
}
