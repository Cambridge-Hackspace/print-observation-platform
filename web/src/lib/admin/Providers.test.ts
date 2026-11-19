// @vitest-environment jsdom
// "Sign in with CS:S" (#14), and what a sign-in that came back without one
// says.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, waitFor } from '@testing-library/svelte';
import Providers from './Providers.svelte';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  history.replaceState(null, '', '/');
});

function pop(providers: unknown[]) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) =>
      url === '/api/sign-in/providers'
        ? new Response(JSON.stringify(providers), { status: 200 })
        : new Response('{"error":"no"}', { status: 404 }),
    ),
  );
}

describe('signing in through a provider', () => {
  it('links to each configured provider, coming back to this screen', async () => {
    pop([{ id: 'css', label: 'CS:S' }]);
    const r = render(Providers, { then: '/admin' });
    const link = await waitFor(() => r.getByRole('link', { name: 'Sign in with CS:S' }));
    expect(link.getAttribute('href')).toBe('/api/sign-in/css?then=%2Fadmin');
    expect(r.getByText('or with your POP username and password')).toBeTruthy();
  });

  it('offers nothing when no provider is configured', async () => {
    pop([]);
    const r = render(Providers, { then: '/' });
    await new Promise((done) => setTimeout(done, 20));
    expect(r.queryByTestId('providers')).toBeNull();
  });

  it('says plainly why a sign-in came back without one', () => {
    pop([]);
    for (const [said, words] of [
      ['failed', 'Signing in didn’t work'],
      ['cancelled', 'Signing in was cancelled.'],
      ['deactivated', 'That account is switched off.'],
      ['expired', 'took too long'],
      ['unreachable', 'couldn’t reach the sign-in service'],
      ['something-new', 'Signing in didn’t work'],
    ]) {
      history.replaceState(null, '', `/admin?sign_in=${said}`);
      const r = render(Providers, { then: '/admin' });
      expect(r.getByTestId('sign-in-problem').textContent).toContain(words);
      cleanup();
    }
  });
});
