import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

// The guides on a phone (#72), in a real browser at phone width: what jsdom
// cannot see is layout, so this is where "it fits a phone" is proven. An
// admin (made by tools/web/e2e.sh) publishes a guide whose flowchart is far
// wider than a phone, with four boxes in its last row.

const PHONE = { width: 390, height: 844 };
const SLUG = 'e2e-wide-flowchart';

const GUIDE = {
  slug: SLUG,
  title: 'A wide flowchart',
  status: 'published',
  summary: 'Two questions deep, four ends.',
  start: 'q1',
  steps: [
    { id: 'q1', title: 'Is the first layer lifting?', body: 'Look at the corners.', image: null, then: { kind: 'question', yes: 'q2', no: 'q3' } },
    { id: 'q2', title: 'Is the bed clean?', body: '', image: null, then: { kind: 'question', yes: 'level', no: 'clean' } },
    { id: 'q3', title: 'Is the nozzle dragging?', body: '', image: null, then: { kind: 'question', yes: 'z', no: 'fine' } },
    { id: 'level', title: 'Level the bed.', body: '', image: null, then: { kind: 'done' } },
    { id: 'clean', title: 'Clean the bed with isopropyl alcohol.', body: '', image: null, then: { kind: 'done' } },
    { id: 'z', title: 'Raise the Z offset a little.', body: '', image: null, then: { kind: 'done' } },
    { id: 'fine', title: 'It is fine: print on.', body: '', image: null, then: { kind: 'done' } },
  ],
};

test.beforeAll(async ({ request }) => {
  const password = process.env.POP_E2E_ADMIN_PASSWORD;
  expect(password, 'tools/web/e2e.sh makes the admin these tests sign in as').toBeTruthy();
  const signIn = await request.post('/api/session', { data: { username: 'e2e', password } });
  expect(signIn.status()).toBe(200);
  const made = await request.post('/api/admin/guides', { data: GUIDE });
  expect([200, 201, 409], await made.text()).toContain(made.status());
});

/** The page itself never scrolls sideways at phone width. */
async function fitsTheScreen(page: Page) {
  const [scroll, width] = await page.evaluate(() => [document.documentElement.scrollWidth, window.innerWidth]);
  expect(scroll, 'the page is wider than the phone').toBeLessThanOrEqual(width);
}

async function axe(page: Page) {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(results.violations, JSON.stringify(results.violations, null, 1)).toEqual([]);
}

test('the guides list fits a phone and leads back to the printers', async ({ page }) => {
  await page.setViewportSize(PHONE);
  await page.goto('/guides');
  await expect(page.getByRole('link', { name: 'A wide flowchart' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Printers' })).toHaveAttribute('href', '/');
  await fitsTheScreen(page);
  await axe(page);
});

test('a guide walks on a phone, and its wide flowchart scrolls rather than shrinking', async ({ page }) => {
  await page.setViewportSize(PHONE);
  await page.goto(`/guides/${SLUG}`);
  await expect(page.getByTestId('step-title')).toHaveText('Is the first layer lifting?');
  await page.getByRole('button', { name: 'Yes' }).click();
  await expect(page.getByTestId('step-title')).toHaveText('Is the bed clean?');
  await fitsTheScreen(page);

  await page.getByText('The whole guide').click();
  const scroller = page.getByTestId('map-scroll');
  await expect(scroller).toBeVisible();
  const [inner, outer] = await scroller.evaluate((el) => [el.scrollWidth, el.clientWidth]);
  expect(inner, 'the flowchart keeps its own size').toBeGreaterThan(outer);
  // Its words are drawn at their own size: a box is as wide as layout.ts says.
  const box = await page.getByTestId('guide-map').locator('[data-step="level"] rect').first().boundingBox();
  expect(box?.width).toBeGreaterThanOrEqual(160);
  await fitsTheScreen(page);
  await axe(page);
});
