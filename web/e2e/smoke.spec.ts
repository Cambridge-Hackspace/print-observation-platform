import { expect, test } from '@playwright/test';

// The whole wiring, end to end: the binary serves the app it was built with,
// the app opens the live stream, and what it shows is what the API reports.
// The server's own tests prove the routing rules; this proves they are wired
// to a real build, which no cheaper test can.

test('the app goes live and shows what the server reports', async ({ page, request }) => {
  const api = await request.get('/api/status');
  expect(api.ok()).toBe(true);
  const status = await api.json();

  await page.goto('/');
  await expect(page.getByTestId('connection')).toHaveText('Live');
  await expect(page.getByTestId('version')).toHaveText(status.version);
  await expect(page.getByTestId('schema')).toHaveText(String(status.schema_version));
});

test('a screen of the app opens at its own address', async ({ page }) => {
  const res = await page.goto('/kiosk');
  expect(res?.status()).toBe(200);
  await expect(page.getByTestId('connection')).toHaveText('Live');
});

test('an unknown API path is an error, not the app', async ({ request }) => {
  const res = await request.get('/api/nope');
  expect(res.status()).toBe(404);
  expect(await res.json()).toHaveProperty('error');
});
