import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';
export default defineConfig({
  root: fileURLToPath(new URL('.', import.meta.url)),
  build: { target: 'es2022', outDir: 'dist', assetsInlineLimit: 0 },
  worker: { format: 'es' },
  server: { host: '127.0.0.1', proxy: { '/api': 'http://127.0.0.1:3000' } },
});
