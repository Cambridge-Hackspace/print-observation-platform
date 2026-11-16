import { defineConfig } from '@playwright/test';

// End-to-end tests against a real `pop` binary with the app built in, on a
// fresh database (tools/web/e2e.sh starts it and sets POP_URL).
export default defineConfig({
  testDir: 'e2e',
  workers: 1,
  fullyParallel: false,
  retries: 0,
  timeout: 30_000,
  reporter: [['list']],
  use: {
    baseURL: process.env.POP_URL ?? 'http://127.0.0.1:8080',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  outputDir: 'test-results',
});
