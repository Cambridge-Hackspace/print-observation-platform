// @vitest-environment jsdom
// The feedback button (#96): there only for whoever holds feedback.file.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, waitFor } from '@testing-library/svelte';
import Feedback from './Feedback.svelte';

function answering(status: number, body: unknown) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })),
  );
}

describe('the feedback button', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('is there for someone who may file feedback', async () => {
    answering(200, { enabled: true, allowed: true, per_hour: 10 });
    const r = render(Feedback);
    await waitFor(() => expect(r.queryByTestId('feedback-open')).not.toBeNull());
  });

  for (const [why, status, body] of [
    ['for someone who may not', 200, { enabled: true, allowed: false, per_hour: 10 }],
    ['when feedback is not set up', 200, { enabled: false, allowed: false, per_hour: 0 }],
    ['for nobody signed in', 401, { error: 'sign in first' }],
  ] as const) {
    it(`is not there ${why}`, async () => {
      answering(status, body);
      const r = render(Feedback);
      await new Promise((t) => setTimeout(t, 20));
      expect(r.queryByTestId('feedback-open')).toBeNull();
    });
  }
});
