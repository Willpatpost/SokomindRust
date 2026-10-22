import { defineConfig } from '@playwright/test';

// The suite loads the built app: `npm run build` must precede `npm run test:browser`.
export default defineConfig({
  testDir: 'web/tests/browser',
  // A stray test.only would otherwise pass CI after running a single spec.
  forbidOnly: !!process.env.CI,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: { baseURL: 'http://127.0.0.1:4173' },
  webServer: [
    {
      command: 'npm run preview',
      url: 'http://127.0.0.1:4173',
      reuseExistingServer: !process.env.CI,
    },
    // The native-engine spec's API: the server the preview's /api proxy
    // targets, started and stopped with the suite, its health endpoint the
    // readiness probe. The command builds first, which a cold cache pays
    // inside the generous timeout.
    {
      command: 'npm run server',
      url: 'http://127.0.0.1:3000/api/health',
      reuseExistingServer: !process.env.CI,
      timeout: 240_000,
    },
  ],
});
