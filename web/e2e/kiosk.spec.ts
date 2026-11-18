import { expect, test, type APIRequestContext, type Page, request as newRequest } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

// The kiosk (#59), in a real browser, against a POP with printers: its own
// `pop`, pointed at the fake BAM! and fake Moonraker printers that
// tools/web/e2e.sh starts (crates/pop-fakes). The other specs keep the POP
// with no BAM!, so their "no printers yet" stays true.
//
// Not covered here: the inactivity sign-out, because a kiosk has no session
// to sign out of until kiosk sessions exist (#12). The card reader is not
// built yet either.

const FLEET = process.env.POP_FLEET_URL;
const FAKES = process.env.POP_FAKES_URL;
test.use({ baseURL: FLEET });

/** Every fake printer, by name: its BAM! id. */
let printers: Record<string, string> = {};
let fakes: APIRequestContext;

/** Change a fake printer as Klipper would report it. */
async function set(name: string, objects: object) {
  const res = await fakes.post(`/printers/${encodeURIComponent(name)}`, { data: objects });
  expect(res.status(), `the fakes know ${name}`).toBe(204);
}

const STANDBY = {
  webhooks: { state: 'ready', state_message: 'Printer is ready' },
  print_stats: { state: 'standby', filename: '', message: '' },
  display_status: { progress: 0 },
};

function card(page: Page, name: string) {
  return page.getByTestId('printer').filter({ has: page.getByRole('heading', { name, exact: true }) });
}

async function live(page: Page) {
  await expect(page.getByTestId('tower')).toHaveAttribute('data-connection', 'live');
  await expect(page.getByTestId('printer')).toHaveCount(Object.keys(printers).length);
}

test.beforeAll(async ({ request }) => {
  expect(FLEET, 'tools/web/e2e.sh starts the POP with printers and sets POP_FLEET_URL').toBeTruthy();
  expect(FAKES, 'tools/web/e2e.sh starts the fakes and sets POP_FAKES_URL').toBeTruthy();
  fakes = await newRequest.newContext({ baseURL: FAKES });
  printers = await (await fakes.get('/printers')).json();
  expect(Object.keys(printers).length, 'enough printers that the tower has to scroll').toBeGreaterThanOrEqual(10);
  // Added at the space by an admin, as the desk does. The request comes from
  // this machine, so it is local.
  const password = process.env.POP_E2E_ADMIN_PASSWORD;
  const signedIn = await request.post('/api/session', { data: { username: 'e2e', password } });
  expect(signedIn.status()).toBe(200);
  for (const id of Object.values(printers)) {
    const added = await request.post(`/api/admin/printers/${id}/add`, { data: {} });
    expect([204, 409], `adding ${id}: ${await added.text()}`).toContain(added.status());
  }
});

test.beforeEach(async () => {
  for (const name of Object.keys(printers)) await set(name, STANDBY);
});

test.afterAll(async () => {
  await fakes.dispose();
});

test('the printers added at the space are on the tower, and change as the printers do', async ({ page }) => {
  await page.goto('/kiosk');
  await live(page);
  const voron = card(page, 'Voron 2.4');
  await expect(voron).toHaveAttribute('data-need', 'free');

  await set('Voron 2.4', {
    print_stats: { state: 'printing', filename: 'gear_housing.gcode' },
    display_status: { progress: 0.4 },
  });
  await expect(voron).toHaveAttribute('data-need', 'printing');
  // As people say it: without the .gcode (need.ts).
  await expect(voron).toContainText('gear_housing');
  await expect(voron.getByRole('progressbar')).toHaveAttribute('aria-valuenow', /^40/);

  await set('Voron 2.4', { print_stats: { state: 'complete' }, display_status: { progress: 1 } });
  await expect(voron).toHaveAttribute('data-need', 'done');
});

test('a printer that needs a hand says what to do, and the tower comes back', async ({ page }) => {
  await page.clock.install();
  await page.goto('/kiosk');
  await live(page);
  await set('Bench 01', {
    print_stats: {
      state: 'error',
      filename: 'bracket.gcode',
      message: 'Heater extruder not heating at expected rate',
    },
  });
  const bench = card(page, 'Bench 01');
  await expect(bench).toHaveAttribute('data-need', 'needs_hand');

  // Asked for, and sent back by hand.
  await page.getByRole('button', { name: 'See what Bench 01 needs' }).click();
  const hand = page.getByTestId('hand');
  await expect(hand).toBeVisible();
  await expect(page.getByTestId('cause')).toContainText('extruder heater');
  await page.getByRole('button', { name: 'Back to the printers' }).first().click();
  await expect(hand).toBeHidden();
  await expect(bench).toBeVisible();

  // Left alone, it goes back by itself after two minutes, and not before.
  await page.getByRole('button', { name: 'See what Bench 01 needs' }).click();
  await expect(hand).toBeVisible();
  await page.clock.runFor(110_000);
  await expect(hand).toBeVisible();
  await page.clock.runFor(15_000);
  await expect(hand).toBeHidden();
  await expect(page.getByTestId('printers')).toBeVisible();
});

test('the tower with printers, and what a printer needs, pass an accessibility check', async ({ page }) => {
  await page.goto('/kiosk');
  await live(page);
  await set('Voron 2.4', {
    print_stats: { state: 'printing', filename: 'gear_housing.gcode' },
    display_status: { progress: 0.4 },
  });
  await set('Bench 01', { print_stats: { state: 'error', filename: 'bracket.gcode', message: 'Heater extruder not heating at expected rate' } });
  await expect(card(page, 'Bench 01')).toHaveAttribute('data-need', 'needs_hand');
  const tower = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(tower.violations, JSON.stringify(tower.violations, null, 1)).toEqual([]);

  await page.getByRole('button', { name: 'See what Bench 01 needs' }).click();
  await expect(page.getByTestId('hand')).toBeVisible();
  const hand = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa']).analyze();
  expect(hand.violations, JSON.stringify(hand.violations, null, 1)).toEqual([]);
});

/** The printer list's scroll position and size. */
async function scroll(page: Page) {
  return page.getByTestId('printers').evaluate((el) => ({
    top: el.scrollTop,
    height: el.clientHeight,
    bottom: el.scrollHeight - el.clientHeight,
  }));
}

test('left alone for thirty seconds, the tower scrolls through the printers, slowly', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.clock.install();
  await page.goto('/kiosk');
  await live(page);
  const start = await scroll(page);
  expect(start.bottom, 'more printers than fit, so there is something to scroll').toBeGreaterThan(0);
  expect(start.top).toBe(0);

  await page.clock.runFor(29_000);
  expect((await scroll(page)).top, 'not before thirty seconds').toBe(0);
  await page.clock.runFor(2_000);
  const moving = await scroll(page);
  expect(moving.top, 'moving').toBeGreaterThan(0);
  expect(moving.top, 'slowly, not a screenful at once').toBeLessThan(moving.height / 2);
});

test('with reduced motion, the tower moves a screenful at a time instead', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.clock.install();
  await page.goto('/kiosk');
  await live(page);
  const start = await scroll(page);
  expect(start.bottom).toBeGreaterThan(0);

  await page.clock.runFor(29_000);
  expect((await scroll(page)).top).toBe(0);
  await page.clock.runFor(2_000);
  const jumped = await scroll(page);
  expect(jumped.top, 'one screenful, in one step').toBeCloseTo(Math.min(start.bottom, start.height), 0);
  // And it rests there before the next.
  await page.clock.runFor(3_000);
  expect((await scroll(page)).top).toBeCloseTo(jumped.top, 0);
});
