import { defineConfig } from '@playwright/test';

// The suite loads the built app: `npm run build` must precede `npm run test:browser`.
export default defineConfig({
  testDir: 'web/tests/browser',
  reporter: [['list'], ['html', { open: 'never' }]],
  use: { baseURL: 'http://127.0.0.1:4173' },
  webServer: {
    command: 'npm run preview',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: !process.env.CI,
  },
});
