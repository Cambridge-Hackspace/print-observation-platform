// @vitest-environment jsdom
// The phone app's shell (#60), against stand-ins for POP's API and stream.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import Phone from './Phone.svelte';
import type { TowerPrinter } from '../generated/TowerPrinter';
import type { User } from '../generated/User';

const SAM: User = { id: 2, username: 'sam', name: 'Sam Lee', role: 'member' };
const ASH: User = { id: 1, username: 'ash', name: 'Ash', role: 'admin' };

const printer = (name: string, s: Partial<TowerPrinter['state']>): TowerPrinter => ({
  id: `id-${name}`,
  name,
  state: { status: 'idle', progress: null, layer: null, seconds_left: null, job: null, error: null, held: false, ...s },
  watched: true,
  materials: [],
  training: null,
});

function pop(start: { me: User | null; printers?: TowerPrinter[] }) {
  let me = start.me;
  const calls: { method: string; url: string }[] = [];
  const snapshot = JSON.stringify({
    type: 'snapshot',
    data: { status: { version: '0.1.0', schema_version: 10, public_url: null }, printers: start.printers ?? [], told: [] },
  });
  vi.stubGlobal(
    'EventSource',
    class {
      readyState = 0;
      onopen: ((e: Event) => void) | null = null;
      onmessage: ((e: MessageEvent) => void) | null = null;
      onerror = null;
      constructor() {
        setTimeout(() => {
          this.onopen?.(new Event('open'));
          this.onmessage?.(new MessageEvent('message', { data: snapshot, lastEventId: 'ep.1' }));
        }, 0);
      }
      close() {}
    },
  );
  const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit = {}) => {
      const method = init.method ?? 'GET';
      calls.push({ method, url });
      if (url === '/api/me') return me ? json(me) : json({ error: 'sign in first' }, 401);
      if (url === '/api/session' && method === 'POST') {
        const b = JSON.parse(String(init.body)) as { username: string; password: string };
        if (b.username === 'sam' && b.password === 'right') {
          me = SAM;
          return json(SAM);
        }
        return json({ error: 'wrong username or password' }, 401);
      }
      if (url === '/api/session' && method === 'DELETE') {
        me = null;
        return new Response(null, { status: 204 });
      }
      return json({ error: 'no such route' }, 404);
    }),
  );
  return calls;
}

beforeEach(() => history.replaceState(null, '', '/'));
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('the phone app', () => {
  it('shows the printers live, most needed first, to someone not signed in', async () => {
    pop({
      me: null,
      printers: [
        printer('Voron 2.4', { status: 'idle' }),
        printer('Mk4', { status: 'printing', progress: 0.5, seconds_left: 600, job: { name: 'gear.gcode', owner: 'Dev Patel' } }),
        printer('Ender 3', { status: 'needs_hand', progress: 0.4 }),
      ],
    });
    const r = render(Phone);
    await waitFor(() => expect(within(r.getByTestId('phone-printers')).getAllByRole('listitem')).toHaveLength(3));
    const items = within(r.getByTestId('phone-printers')).getAllByRole('listitem');
    expect(items.map((i) => i.dataset.need)).toEqual(['needs_hand', 'printing', 'free']);
    expect(items[1]!.textContent).toContain('Dev’s gear');
    expect(within(items[1]!).getByRole('progressbar')).toBeTruthy();
    expect(r.getByRole('button', { name: 'Sign in' })).toBeTruthy();
  });

  it('signs in, says who you are, and signs out', async () => {
    const calls = pop({ me: null });
    const r = render(Phone);
    await fireEvent.click(await waitFor(() => r.getByRole('button', { name: 'Sign in' })));
    await fireEvent.input(r.getByLabelText('Username'), { target: { value: 'sam' } });
    await fireEvent.input(r.getByLabelText('Password'), { target: { value: 'right' } });
    await fireEvent.submit(r.getByTestId('signin'));
    await waitFor(() => expect(r.getByRole('link', { name: 'Sam Lee' })).toBeTruthy());
    history.replaceState(null, '', '/#you');
    window.dispatchEvent(new HashChangeEvent('hashchange'));
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('You'));
    expect(r.getByTestId('phone').textContent).toContain('Signed in as Sam Lee (sam).');
    expect(r.queryByRole('link', { name: 'Open the admin desk' })).toBeNull();
    await fireEvent.click(r.getByRole('button', { name: 'Sign out' }));
    await waitFor(() => expect(r.getByTestId('phone').textContent).toContain('Sign in to send prints'));
    expect(calls.some((c) => c.method === 'DELETE' && c.url === '/api/session')).toBe(true);
  });

  it('lets someone who would rather not sign in go back', async () => {
    pop({ me: null });
    const r = render(Phone);
    await fireEvent.click(await waitFor(() => r.getByRole('button', { name: 'Sign in' })));
    await fireEvent.click(r.getByRole('button', { name: 'Not now' }));
    expect(r.getByTestId('phone')).toBeTruthy();
  });

  it('opens the section the address names, marks it in the navigation, and offers an admin the desk', async () => {
    history.replaceState(null, '', '/#you');
    pop({ me: ASH });
    const r = render(Phone);
    await waitFor(() => expect(r.getByRole('link', { name: 'Open the admin desk' }).getAttribute('href')).toBe('/admin'));
    const nav = r.getByRole('navigation', { name: 'Sections' });
    expect(within(nav).getByRole('link', { name: 'You' }).getAttribute('aria-current')).toBe('page');
    expect(within(nav).getByRole('link', { name: 'Printers' }).getAttribute('aria-current')).toBeNull();
    expect(within(nav).getByRole('link', { name: 'Guides' }).getAttribute('href')).toBe('/guides');
    await waitFor(() => expect(r.getByTestId('connection').textContent).toBe('Live'));
    expect(r.getByTestId('version').textContent).toBe('0.1.0');
    expect(r.getByTestId('schema').textContent).toBe('10');
  });
});
