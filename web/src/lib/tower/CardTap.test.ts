// @vitest-environment jsdom
// A card tap at the kiosk's reader (#18): the member welcomed by name and
// signed in at once.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import type { SessionInfo } from '../generated/SessionInfo';
import type { TapView } from '../generated/TapView';
import CardTap from './CardTap.svelte';

type Call = { method: string; url: string; body: unknown };

const SESSION: SessionInfo = { user: { id: 7, username: 'alex', name: 'Alex Doe', role: 'member' }, idle_secs: 90 };

/** POP, answering the kiosk's takes with `answers` in turn, then with a yes. */
function pop(answers: { status: number; error?: string }[] = []) {
  const calls: Call[] = [];
  const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit = {}) => {
      const method = init.method ?? 'GET';
      const body = typeof init.body === 'string' ? JSON.parse(init.body) : null;
      calls.push({ method, url, body });
      if (url === '/api/kiosk/tap' && method === 'POST') {
        const a = answers.shift() ?? { status: 200 };
        return a.status === 200 ? json(SESSION.user) : json({ error: a.error }, a.status);
      }
      if (url === '/api/kiosk/tap' && method === 'DELETE') return new Response(null, { status: 204 });
      if (url === '/api/session') return json(SESSION);
      return json({ error: 'no such route' }, 404);
    }),
  );
  return calls;
}

function show(tap: TapView) {
  const onsignedin = vi.fn();
  const ondone = vi.fn();
  const r = render(CardTap, { tap, onsignedin, ondone });
  return { r, onsignedin, ondone };
}

describe('a card tap at the kiosk', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('welcomes the member by name and signs them in at once', async () => {
    const calls = pop();
    const { r, onsignedin } = show({ kind: 'sign_in', name: 'Alex' });
    expect(r.getByTestId('tap-welcome').textContent).toBe('Welcome, Alex!');
    await waitFor(() => expect(onsignedin).toHaveBeenCalledWith(SESSION));
    expect(calls.map((c) => [c.method, c.url, c.body])).toEqual([
      ['POST', '/api/kiosk/tap', {}],
      ['GET', '/api/session', null],
    ]);
  });

  it('welcomes a member CS:S has no name for without one', () => {
    pop();
    const { r } = show({ kind: 'sign_in', name: '' });
    expect(r.getByTestId('tap-welcome').textContent).toBe('Welcome!');
  });

  it('says so, and offers only OK, when the sign-in cannot be finished', async () => {
    pop([{ status: 404, error: 'there is no card tap to sign in with: tap again' }]);
    const { r, onsignedin, ondone } = show({ kind: 'sign_in', name: 'Alex' });
    await waitFor(() => expect(r.getByRole('alert').textContent).toContain('tap again'));
    expect(onsignedin).not.toHaveBeenCalled();
    expect(r.queryByTestId('tap-welcome')).toBeNull();
    await fireEvent.click(r.getByRole('button', { name: 'OK' }));
    expect(ondone).toHaveBeenCalled();
  });
});
