import { expect, test } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

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

test('the tower opens at its own address and goes live', async ({ page }) => {
  const res = await page.goto('/kiosk');
  expect(res?.status()).toBe(200);
  await expect(page.getByTestId('tower')).toHaveAttribute('data-connection', 'live');
  // No BAM! here, so no printers: it says how to get some, and keeps the
  // tap prompt.
  await expect(page.getByText(/an admin adds them from BAM!/)).toBeVisible();
  await expect(page.getByText('Tap your card to start printing')).toBeVisible();
});

test('the tower passes an accessibility check', async ({ page }) => {
  await page.goto('/kiosk');
  await expect(page.getByTestId('tower')).toHaveAttribute('data-connection', 'live');
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(results.violations, JSON.stringify(results.violations, null, 1)).toEqual([]);
});

test('an unknown API path is an error, not the app', async ({ request }) => {
  const res = await request.get('/api/nope');
  expect(res.status()).toBe(404);
  expect(await res.json()).toHaveProperty('error');
});

test('the server reports its health, naming the database', async ({ request }) => {
  const res = await request.get('/healthz');
  expect(res.status()).toBe(200);
  const health = await res.json();
  expect(health.ok).toBe(true);
  expect(health.checks.database).toEqual({ ok: true });
});

test('a screen of the app passes an accessibility check', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('connection')).toHaveText('Live');
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(results.violations, JSON.stringify(results.violations, null, 1)).toEqual([]);
});
