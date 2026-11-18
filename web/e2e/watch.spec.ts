import { expect, test, request as newRequest } from '@playwright/test';

// Watching a print from a phone (#61), in a real browser at phone width,
// against the POP with printers (crates/pop-fakes): open a printing printer
// from the list, see it live, and find no way to pause or stop it.
//
// Not covered here: the camera. The fake printers have none, and the fake
// go2rtc's video is not a picture a browser can play; the camera's rules are
// in src/lib/phone/player.test.ts and Watch.test.ts, and the server's in
// crates/pop-server/tests/cameras.rs.

const FLEET = process.env.POP_FLEET_URL;
const FAKES = process.env.POP_FAKES_URL;
test.use({ baseURL: FLEET, viewport: { width: 390, height: 844 }, hasTouch: true });

let printers: Record<string, string> = {};

test.beforeAll(async ({ request }) => {
  expect(FLEET, 'tools/web/e2e.sh sets POP_FLEET_URL').toBeTruthy();
  const fakes = await newRequest.newContext({ baseURL: FAKES });
  printers = await (await fakes.get('/printers')).json();
  const signedIn = await request.post('/api/session', {
    data: { username: 'e2e', password: process.env.POP_E2E_ADMIN_PASSWORD },
  });
  expect(signedIn.status()).toBe(200);
  for (const id of Object.values(printers)) {
    const added = await request.post(`/api/admin/printers/${id}/add`, { data: {} });
    expect([204, 409]).toContain(added.status());
  }
  await request.delete('/api/session', { data: {} });
  const set = await fakes.post(`/printers/${encodeURIComponent('Voron 2.4')}`, {
    data: { print_stats: { state: 'printing', filename: 'gear_housing.gcode' }, display_status: { progress: 0.4 } },
  });
  expect(set.status()).toBe(204);
});

test('a printing printer is watched live from the phone, with no way to stop it', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('link', { name: 'Voron 2.4', exact: true }).tap();
  const watch = page.getByTestId('watch');
  await expect(watch).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`#watch/${printers['Voron 2.4']}$`));
  await expect(watch.getByRole('heading', { name: 'Voron 2.4' })).toBeVisible();
  await expect(watch.getByRole('progressbar')).toHaveAttribute('aria-valuenow', /^40/);
  await expect(watch).toContainText('gear_housing');
  // Watching is remote; control is local (#7).
  for (const name of [/pause/i, /stop/i, /resume/i, /cancel/i]) {
    await expect(watch.getByRole('button', { name })).toHaveCount(0);
  }
  // Live: it follows the printer.
  const fakes = await newRequest.newContext({ baseURL: FAKES });
  await fakes.post(`/printers/${encodeURIComponent('Voron 2.4')}`, { data: { display_status: { progress: 0.7 } } });
  await expect(watch.getByRole('progressbar')).toHaveAttribute('aria-valuenow', /^70/);
  // And back to the list.
  await watch.getByRole('link', { name: /Printers/ }).tap();
  await expect(page.getByTestId('phone-printers')).toBeVisible();
});
