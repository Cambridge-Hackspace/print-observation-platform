import { expect, test } from '@playwright/test';

// Feedback from the kiosk (#96), end to end: at 1280x800 with touch only, an
// admin taps the button, marks the screenshot, says what is wrong and sends
// it; the fake GitHub (crates/pop-fakes) gets the picture on its branch and
// an issue that shows it, labelled. Against the POP with printers, whose
// [feedback] tools/web/e2e.sh points at the fake.
//
// Not covered here, and where it is: redaction (src/lib/feedback/redact.test.ts,
// on the clone), scrubbing and retrying while GitHub is down
// (crates/pop-server/tests/feedback.rs).

const FLEET = process.env.POP_FLEET_URL;
const FAKES = process.env.POP_FAKES_URL;
test.use({ baseURL: FLEET, viewport: { width: 1280, height: 800 }, hasTouch: true });

test('an admin files feedback from the kiosk with touch alone', async ({ page, request }) => {
  expect(FLEET, 'tools/web/e2e.sh sets POP_FLEET_URL').toBeTruthy();
  const before = (await (await request.get(`${FAKES}/github/issues`)).json()) as unknown[];
  // Signed in as an admin, who holds feedback.file by default.
  const signedIn = await page.request.post('/api/session', {
    data: { username: 'e2e', password: process.env.POP_E2E_ADMIN_PASSWORD },
  });
  expect(signedIn.status()).toBe(200);
  await page.goto('/kiosk');
  await expect(page.getByTestId('tower')).toBeVisible();

  await page.getByTestId('feedback-open').tap();
  await expect(page.getByTestId('feedback-annotate')).toBeVisible();
  // A note on the picture, placed by a tap.
  await page.getByTestId('tool-text').tap();
  const canvas = page.locator('[data-testid="feedback-annotate"] canvas').first();
  const box = (await canvas.boundingBox())!;
  await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
  await page.getByLabel('Text to add').fill('this one');
  await page.getByRole('button', { name: 'Add' }).tap();
  await page.getByTestId('annotate-done').tap();

  await expect(page.getByTestId('feedback-form')).toBeVisible();
  await expect(page.getByText('public', { exact: false }).first()).toBeVisible();
  await page.getByTestId('feedback-title').fill('The Voron card reads offline');
  await page.getByTestId('feedback-description').fill('Filed from the kiosk by the e2e battery.');
  await page.getByTestId('feedback-send').tap();

  const outcome = page.getByTestId('feedback-outcome');
  await expect(outcome).toContainText('Sent', { timeout: 20_000 });
  await expect(outcome.getByRole('link', { name: 'See the issue' })).toHaveAttribute('href', /github\.com\/acme\/pop\/issues\/\d+/);

  const issues = (await (await request.get(`${FAKES}/github/issues`)).json()) as {
    title: string;
    body: string;
    labels: string[];
  }[];
  expect(issues.length).toBe(before.length + 1);
  const issue = issues[issues.length - 1]!;
  expect(issue.title).toBe('The Voron card reads offline');
  expect(issue.labels).toEqual(['feedback', 'bug']);
  expect(issue.body).toContain('Reported by E2E Admin from the kiosk.');
  const picture = /\]\((https:\/\/raw\.githubusercontent\.com\/acme\/pop\/feedback-attachments\/(feedback\/[0-9a-f]+\.png))\)/.exec(issue.body);
  expect(picture, issue.body).not.toBeNull();
  const files = (await (await request.get(`${FAKES}/github/files`)).json()) as string[];
  expect(files).toContain(picture![2]);
});
