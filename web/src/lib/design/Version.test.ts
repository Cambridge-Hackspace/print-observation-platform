// @vitest-environment jsdom
// The version at the foot of every screen (#189).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, waitFor } from '@testing-library/svelte';
import Version from './Version.svelte';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('the version line', () => {
  it('shows the version the running POP reports', async () => {
    const fetch = vi.fn(
      async (_url: string) =>
        new Response(JSON.stringify({ version: '9.8.7', schema_version: 3, public_url: null }), { status: 200 }),
    );
    vi.stubGlobal('fetch', fetch);
    const { container } = render(Version);
    await waitFor(() => expect(container.textContent).toContain('POP v9.8.7'));
    expect(fetch.mock.calls[0]![0]).toBe('/api/status');
  });

  it('shows nothing, not a guess, when POP cannot be asked', async () => {
    const fetch = vi.fn(async () => new Response('{"error":"no"}', { status: 503 }));
    vi.stubGlobal('fetch', fetch);
    const { container } = render(Version);
    // The precondition first: it asked, and the answer came back.
    await waitFor(() => expect(fetch).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 50));
    expect(container.textContent?.trim()).toBe('');
  });
});
