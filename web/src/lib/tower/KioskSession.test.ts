// @vitest-environment jsdom
// Signing in at the kiosk, and the session that ends left alone (#12).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import type { SessionInfo } from '../generated/SessionInfo';
import KioskSession from './KioskSession.svelte';
import KioskSignIn from './KioskSignIn.svelte';

type Call = { method: string; url: string; body: unknown };

function pop(opts: { password?: string; session?: number } = {}) {
  const calls: Call[] = [];
  const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
  let sessionStatus = opts.session ?? 200;
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit = {}) => {
      const method = init.method ?? 'GET';
      const body = typeof init.body === 'string' ? JSON.parse(init.body) : null;
      calls.push({ method, url, body });
      if (url === '/api/session' && method === 'POST') {
        return body.password === (opts.password ?? 'pw')
          ? json({ id: 2, username: 'grace', name: 'Grace', role: 'member' })
          : json({ error: 'wrong username or password' }, 401);
      }
      if (url === '/api/session' && method === 'GET') {
        return sessionStatus === 200 ? json(SESSION) : json({ error: 'sign in first' }, sessionStatus);
      }
      if (url === '/api/session' && method === 'DELETE') return new Response(null, { status: 204 });
      return json({ error: 'no such route' }, 404);
    }),
  );
  return { calls, expire: () => (sessionStatus = 401) };
}

const SESSION: SessionInfo = { user: { id: 2, username: 'grace', name: 'Grace', role: 'member' }, idle_secs: 90 };

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('signing in at the kiosk', () => {
  type Screen = { getByRole: (role: 'button', o: { name: string; exact: boolean }) => HTMLElement };
  const type = async (r: Screen, text: string) => {
    for (const c of text) await fireEvent.click(r.getByRole('button', { name: c, exact: true }));
  };

  it('types with the on-screen keyboard, hides the password, and signs in', async () => {
    const { calls } = pop();
    const onsignedin = vi.fn();
    const r = render(KioskSignIn, { onsignedin });
    await type(r, 'grace');
    expect(r.getByTestId('username').textContent).toBe('grace');
    await fireEvent.click(r.getByRole('button', { name: /Password/ }));
    await type(r, 'pw');
    expect(r.getByTestId('password').textContent).toBe('••');
    await fireEvent.click(r.getByRole('button', { name: 'Sign in' }));
    await waitFor(() => expect(onsignedin).toHaveBeenCalledWith(SESSION));
    expect(calls.find((c) => c.method === 'POST')).toMatchObject({ url: '/api/session', body: { username: 'grace', password: 'pw' } });
  });

  it('gives one capital per shift, then small letters', async () => {
    pop();
    const r = render(KioskSignIn);
    await fireEvent.click(r.getByRole('button', { name: 'Shift' }));
    await type(r, 'Gr');
    expect(r.getByTestId('username').textContent).toBe('Gr');
  });

  it('says why a sign-in failed, and clears the password', async () => {
    pop();
    const onsignedin = vi.fn();
    const r = render(KioskSignIn, { onsignedin });
    await type(r, 'grace');
    await fireEvent.click(r.getByRole('button', { name: /Password/ }));
    await type(r, 'no');
    await fireEvent.click(r.getByRole('button', { name: 'Sign in' }));
    await waitFor(() => expect(r.getByRole('alert').textContent).toBe('wrong username or password'));
    expect(r.getByTestId('password').textContent?.trim()).toBe('');
    expect(onsignedin).not.toHaveBeenCalled();
  });
});

describe('the signed-in bar', () => {
  it('greets, counts down from POP’s time, and signs out when left alone', async () => {
    vi.useFakeTimers();
    const { calls } = pop();
    const onsignedout = vi.fn();
    const r = render(KioskSession, { session: SESSION, onsignedout });
    expect(r.getByText('Grace')).toBeTruthy();
    expect(r.getByTestId('countdown').textContent).toContain('Signs you out in 1:30');
    await vi.advanceTimersByTimeAsync(45_000);
    expect(r.getByTestId('countdown').textContent).toContain('Signs you out in 0:45');
    await vi.advanceTimersByTimeAsync(45_000);
    await waitFor(() => expect(onsignedout).toHaveBeenCalledTimes(1));
    expect(calls.filter((c) => c.method === 'DELETE').map((c) => c.url)).toEqual(['/api/session']);
  });

  it('passes a touch on to POP, and only once POP has it does the countdown start again', async () => {
    vi.useFakeTimers();
    const { calls } = pop();
    const r = render(KioskSession, { session: SESSION });
    await vi.advanceTimersByTimeAsync(5_000);
    await fireEvent.pointerDown(window);
    // Under ten seconds since the last word to POP: not passed on.
    expect(calls.filter((c) => c.method === 'GET')).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(40_000);
    expect(r.getByTestId('countdown').textContent).toContain('0:45');
    await fireEvent.pointerDown(window);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(calls.filter((c) => c.method === 'GET')).toHaveLength(1);
    expect(r.getByTestId('countdown').textContent).toMatch(/Signs you out in 1:(29|30)/);
  });

  it('does not start the countdown again for a touch POP never heard', async () => {
    vi.useFakeTimers();
    pop({ session: 503 });
    const r = render(KioskSession, { session: SESSION });
    await vi.advanceTimersByTimeAsync(45_000);
    await fireEvent.pointerDown(window);
    await vi.advanceTimersByTimeAsync(1_000);
    // POP did not answer: the screen shows POP's time, not hope.
    expect(r.getByTestId('countdown').textContent).toContain('0:44');
  });

  it('signs out when POP says the session is over', async () => {
    vi.useFakeTimers();
    const { expire } = pop();
    const onsignedout = vi.fn();
    render(KioskSession, { session: SESSION, onsignedout });
    expire();
    await vi.advanceTimersByTimeAsync(11_000);
    await fireEvent.pointerDown(window);
    await vi.advanceTimersByTimeAsync(10);
    expect(onsignedout).toHaveBeenCalled();
  });

  it('tags out on request', async () => {
    const { calls } = pop();
    const onsignedout = vi.fn();
    const r = render(KioskSession, { session: SESSION, onsignedout });
    await fireEvent.click(r.getByRole('button', { name: 'Tag out' }));
    await waitFor(() => expect(onsignedout).toHaveBeenCalledTimes(1));
    expect(calls).toEqual([{ method: 'DELETE', url: '/api/session', body: {} }]);
  });

  it('shows no countdown for a session that is not a kiosk one', () => {
    pop();
    const r = render(KioskSession, { session: { ...SESSION, idle_secs: null } });
    expect(r.queryByTestId('countdown')).toBeNull();
    expect(r.getByRole('button', { name: 'Tag out' })).toBeTruthy();
  });
});
