// @vitest-environment jsdom
// A card tap at the kiosk's reader (#19): taken at once, asked for a code, or
// refused in CS:S's words.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import type { SessionInfo } from '../generated/SessionInfo';
import type { TapView } from '../generated/TapView';
import CardTap from './CardTap.svelte';
import { REFUSAL_MS } from './taps';

type Call = { method: string; url: string; body: unknown };

const SESSION: SessionInfo = { user: { id: 7, username: 'alex', name: 'Alex Doe', role: 'member' }, idle_secs: 90 };

/** POP: a tap that takes `right` as its code (or any body, for a code tap), answering `taps` in turn. */
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
  /** Press each digit of `code` on the pad. */
  const type = async (code: string) => {
    for (const d of code) await fireEvent.click(r.getByRole('button', { name: d }));
  };
  return { r, onsignedin, ondone, type };
}

describe('a card tap at the kiosk', () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('signs in at once when it is a sign-in', async () => {
    const calls = pop();
    const { onsignedin } = show({ kind: 'sign_in' });
    await waitFor(() => expect(onsignedin).toHaveBeenCalledWith(SESSION));
    expect(calls.map((c) => [c.method, c.url, c.body])).toEqual([
      ['POST', '/api/kiosk/tap', {}],
      ['GET', '/api/session', null],
    ]);
  });

  it('says so, and offers only OK, when the sign-in cannot be finished', async () => {
    pop([{ status: 400, error: 'unknown card sign-in' }]);
    const { r, onsignedin, ondone } = show({ kind: 'sign_in' });
    await waitFor(() => expect(r.getByRole('alert').textContent).toBe('unknown card sign-in'));
    expect(onsignedin).not.toHaveBeenCalled();
    await fireEvent.click(r.getByRole('button', { name: 'OK' }));
    expect(ondone).toHaveBeenCalled();
  });

  it('takes the authenticator code on its pad, and sends it at the sixth digit', async () => {
    const calls = pop();
    const { r, onsignedin, type } = show({ kind: 'second_factor' });
    await type('12345');
    expect(calls).toEqual([]);
    expect(r.getByTestId('tap-digits').getAttribute('aria-label')).toBe('5 of 6 digits entered');
    await fireEvent.click(r.getByRole('button', { name: 'Delete a digit' }));
    await type('56');
    await waitFor(() => expect(onsignedin).toHaveBeenCalledWith(SESSION));
    expect(calls[0]).toEqual({ method: 'POST', url: '/api/kiosk/tap', body: { totp: '123456' } });
  });

  it('lets a wrong code be tried again, saying what was wrong', async () => {
    const calls = pop([{ status: 400, error: 'that code is not right' }]);
    const { r, onsignedin, type } = show({ kind: 'second_factor' });
    await type('000000');
    await waitFor(() => expect(r.getByRole('alert').textContent).toBe('that code is not right'));
    expect(r.getByTestId('tap-digits').getAttribute('aria-label')).toBe('0 of 6 digits entered');
    await type('123456');
    await waitFor(() => expect(onsignedin).toHaveBeenCalled());
    expect(calls.filter((c) => c.method === 'POST').map((c) => c.body)).toEqual([{ totp: '000000' }, { totp: '123456' }]);
  });

  it('shows the problem POP says the last code had', () => {
    pop();
    const { r } = show({ kind: 'second_factor', problem: 'that code is not right' });
    expect(r.getByRole('alert').textContent).toBe('that code is not right');
  });

  it('ends the pad when POP no longer holds the tap', async () => {
    pop([{ status: 404, error: 'there is no card tap to sign in with: tap again' }]);
    const { r, type } = show({ kind: 'second_factor' });
    await type('123456');
    await waitFor(() => expect(r.getByRole('alert').textContent).toContain('tap again'));
    expect(r.queryByRole('button', { name: '1' })).toBeNull();
    expect(r.getByRole('button', { name: 'OK' })).toBeTruthy();
  });

  it('sets the tap aside on Cancel', async () => {
    const calls = pop();
    const { r, ondone } = show({ kind: 'second_factor' });
    await fireEvent.click(r.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(ondone).toHaveBeenCalled());
    expect(calls).toEqual([{ method: 'DELETE', url: '/api/kiosk/tap', body: {} }]);
  });

  it('shows a refusal in CS:S’s words, and goes by itself after a while', async () => {
    vi.useFakeTimers();
    const calls = pop();
    const { r, ondone } = show({ kind: 'refused', reason: 'Unknown card' });
    expect(r.getByTestId('tap-reason').textContent).toBe('Unknown card');
    await vi.advanceTimersByTimeAsync(REFUSAL_MS - 1);
    expect(ondone).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(ondone).toHaveBeenCalledTimes(1);
    expect(calls).toEqual([]);
  });

  it('sets a refusal aside on OK', async () => {
    const calls = pop();
    const { r, ondone } = show({ kind: 'refused', reason: 'Unknown card' });
    await fireEvent.click(r.getByRole('button', { name: 'OK' }));
    await waitFor(() => expect(ondone).toHaveBeenCalled());
    expect(calls).toEqual([{ method: 'DELETE', url: '/api/kiosk/tap', body: {} }]);
  });
});
