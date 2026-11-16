import { expect, test } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

// The design system's gallery (#48), served by the real binary: it renders
// in Lexend, bundled rather than fetched, and passes an accessibility check,
// color contrast on the dark palette included.

test('the design system renders in Lexend, with nothing fetched from outside', async ({ page }) => {
  const outside: string[] = [];
  page.on('request', (r) => {
    if (!r.url().startsWith('http://127.0.0.1')) outside.push(r.url());
  });
  await page.goto('/design');
  await expect(page.getByRole('heading', { name: 'Design system' })).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
  expect(await page.evaluate(() => document.fonts.check('600 32px Lexend'))).toBe(true);
  expect(outside).toEqual([]);
});

test('the design system passes an accessibility check', async ({ page }) => {
  await page.goto('/design');
  await expect(page.getByRole('progressbar', { name: 'Benchy on the X1' })).toBeVisible();
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(results.violations, JSON.stringify(results.violations, null, 1)).toEqual([]);
});

test('every printer is named and its state said, not only drawn', async ({ page }) => {
  await page.goto('/design');
  for (const name of ['X1, Printing', 'MK4, Printing', 'P1S, Ready', 'Ender, Needs a hand']) {
    await expect(page.getByRole('img', { name })).toBeVisible();
  }
});
