import { defineConfig } from 'vite';
export default defineConfig({
  build: { target: 'es2022', outDir: 'dist', assetsInlineLimit: 0 },
  worker: { format: 'es' },
  // The proxy targets BIND_ADDR's default in crates/server/src/config.rs (checked by scripts/mirrors.test.mjs).
  server: { host: '127.0.0.1', proxy: { '/api': 'http://127.0.0.1:3000' } },
  // The coder.cs.odu.edu proxy that forwards this pod's ports reaches the
  // preview with a port--workspace--user subdomain Host, which vite's host
  // check otherwise rejects; localhost stays allowed. preview.proxy inherits
  // server.proxy above, so the API proxy survives here unchanged.
  preview: { allowedHosts: ['.coder.cs.odu.edu', 'coder.cs.odu.edu', 'localhost'] },
});
