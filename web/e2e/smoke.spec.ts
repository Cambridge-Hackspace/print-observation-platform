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

  // The phone app (#60) says it under You.
  await page.goto('/#you');
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

test('the phone app passes an accessibility check at phone width, on each section', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  for (const at of ['/', '/#you']) {
    await page.goto(at);
    await expect(page.getByTestId('phone')).toBeVisible();
    if (at === '/') await expect(page.getByText(/an admin adds them from BAM!/)).toBeVisible();
    else await expect(page.getByTestId('connection')).toHaveText('Live');
    const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
    expect(results.violations, `${at}: ${JSON.stringify(results.violations, null, 1)}`).toEqual([]);
  }
});

test('the phone app is installable: its manifest and icons are served as a browser needs them', async ({ page, request }) => {
  const res = await request.get('/manifest.webmanifest');
  expect(res.status()).toBe(200);
  expect(res.headers()['content-type']).toContain('application/manifest+json');
  const manifest = await res.json();
  for (const icon of manifest.icons as { src: string; type: string }[]) {
    const i = await request.get(icon.src);
    expect(i.status(), icon.src).toBe(200);
    expect(i.headers()['content-type'], icon.src).toContain(icon.type);
  }
  // And the page the browser installs from links it.
  await page.goto('/');
  await expect(page.locator('link[rel="manifest"]')).toHaveAttribute('href', '/manifest.webmanifest');
  const touch = await request.get('/icon-180.png');
  expect(touch.status()).toBe(200);
});

test('the public guides open signed out, and pass an accessibility check', async ({ page }) => {
  const res = await page.goto('/guides');
  expect(res?.status()).toBe(200);
  // A new POP has none published: it says so, rather than nothing.
  await expect(page.getByText('No guides are published yet.')).toBeVisible();
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(results.violations, JSON.stringify(results.violations, null, 1)).toEqual([]);
  // A guide that is not published says so.
  await page.goto('/guides/no-such-guide');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('No such guide');
});
