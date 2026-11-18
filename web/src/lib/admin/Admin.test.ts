// @vitest-environment jsdom
// The admin desk (#86), against a stand-in for POP's API: signing in, who
// may see it, the printers table, and adding or declining a new printer.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import Admin from './Admin.svelte';
import type { AdminPrinter } from '../generated/AdminPrinter';
import type { User } from '../generated/User';
import type { TowerPrinter } from '../generated/TowerPrinter';

const ASH: User = { id: 1, username: 'ash', name: 'Ash', role: 'admin' };
const SAM: User = { id: 2, username: 'sam', name: 'Sam', role: 'member' };

const printer = (name: string, more: Partial<AdminPrinter> = {}): AdminPrinter => ({
  bam_id: `id-${name}`,
  name,
  ready: true,
  connection: 'moonraker',
  problems: [],
  tower: 'added',
  categories: ['Printers'],
  training: null,
  camera: false,
  ...more,
});

interface Call {
  method: string;
  url: string;
  type: string | null;
  body: unknown;
}

/** POP, as far as the desk sees it. `me` is who is signed in, if anyone. */
function pop(start: { me: User | null; printers: AdminPrinter[]; refuse?: string; live?: TowerPrinter[] }) {
  let me = start.me;
  let printers = start.printers;
  const calls: Call[] = [];
  const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
  // The live stream: a snapshot of the tower's printers, once it opens.
  const snapshot = JSON.stringify({
    type: 'snapshot',
    data: { status: { version: '0.1.0', schema_version: 1, public_url: null }, printers: start.live ?? [], told: [] },
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
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit = {}) => {
      const method = init.method ?? 'GET';
      const type = new Headers(init.headers).get('Content-Type');
      const body = typeof init.body === 'string' ? JSON.parse(init.body) : null;
      calls.push({ method, url, type, body });
      if (url === '/api/me') return me ? json(me) : json({ error: 'sign in first' }, 401);
      if (url === '/api/session' && method === 'POST') {
        const b = body as { username: string; password: string };
        if (b.username === 'ash' && b.password === 'right') {
          me = ASH;
          return json(ASH);
        }
        return json({ error: 'wrong username or password' }, 401);
      }
      if (url === '/api/session' && method === 'DELETE') {
        me = null;
        return new Response(null, { status: 204 });
      }
      if (me?.role !== 'admin') return json({ error: 'admins only' }, 403);
      if (url === '/api/admin/printers') return json(printers);
      if (url === '/api/admin/users' && method === 'GET') return json([]);
      const act = /^\/api\/admin\/printers\/([^/]+)\/(add|decline)$/.exec(url);
      if (act && method === 'POST') {
        if (start.refuse) return json({ error: start.refuse }, 403);
        const id = decodeURIComponent(act[1]!);
        printers = printers.map((p) => (p.bam_id === id ? { ...p, tower: act[2] === 'add' ? 'added' : 'declined' } : p));
        return json({});
      }
      return json({ error: 'no such route' }, 404);
    }),
  );
  return calls;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('the admin desk', () => {
  it('asks someone not signed in to sign in, and shows the desk once they have', async () => {
    const calls = pop({ me: null, printers: [printer('Voron 2.4')] });
    const r = render(Admin);
    await waitFor(() => r.getByTestId('signin'));
    expect(r.queryByTestId('desk')).toBeNull();
    await fireEvent.input(r.getByLabelText('Username'), { target: { value: 'ash' } });
    await fireEvent.input(r.getByLabelText('Password'), { target: { value: 'wrong' } });
    await fireEvent.submit(r.getByTestId('signin'));
    await waitFor(() => expect(r.getByRole('alert').textContent).toContain('don’t match'));
    expect(r.queryByTestId('desk')).toBeNull();
    await fireEvent.input(r.getByLabelText('Password'), { target: { value: 'right' } });
    await fireEvent.submit(r.getByTestId('signin'));
    await waitFor(() => r.getByTestId('desk'));
    expect(r.getByTestId('desk').textContent).toContain('Signed in as Ash');
    const signIn = calls.find((c) => c.url === '/api/session' && c.method === 'POST')!;
    expect(signIn.type).toBe('application/json');
  });

  it('tells a member it is for admins, and never asks for the printers', async () => {
    const calls = pop({ me: SAM, printers: [printer('Voron 2.4')] });
    const r = render(Admin);
    await waitFor(() => expect(r.container.textContent).toContain('This is for printer admins'));
    expect(r.queryByTestId('desk')).toBeNull();
    expect(calls.map((c) => c.url)).not.toContain('/api/admin/printers');
  });

  it('lists every printer with its category, connection, training, camera and where it stands', async () => {
    pop({
      me: ASH,
      printers: [
        printer('Voron 2.4', { training: 'advanced', camera: true, categories: ['Printers', 'Enclosed'] }),
        printer('MK4', { connection: 'prusalink', tower: 'declined' }),
      ],
    });
    const r = render(Admin);
    await waitFor(() => expect(r.getAllByTestId('row')).toHaveLength(2));
    const cells = (i: number) => [...r.getAllByTestId('row')[i]!.querySelectorAll('td')].map((td) => td.textContent?.trim());
    expect(cells(0)).toEqual(['Voron 2.4', 'Printers, Enclosed', 'Klipper', 'Advanced', 'Yes', 'Added: its BAM! fields need fixing']);
    expect(cells(1)).toEqual(['MK4', 'Printers', 'PrusaLink', 'None', 'No', 'Not for the tower']);
  });

  it('says what each printer on the tower is doing now, from the live stream', async () => {
    const voron: TowerPrinter = {
      id: 'id-Voron 2.4',
      name: 'Voron 2.4',
      state: {
        status: 'printing',
        progress: 0.5,
        layer: null,
        seconds_left: 600,
        job: { name: 'gear.gcode', owner: 'Alex Kim' },
        error: null,
        held: false,
      },
      watched: true,
      materials: [],
      training: null, camera: false,
    };
    pop({ me: ASH, printers: [printer('Voron 2.4')], live: [voron] });
    const r = render(Admin);
    await waitFor(() => expect(r.getByTestId('row').textContent).toContain('Printing · Alex'));
  });

  it('lists what needs an admin now, and says when nothing does', async () => {
    const stuck: TowerPrinter = {
      id: 'id-Ender 3',
      name: 'Ender 3',
      state: { status: 'needs_hand', progress: 0.4, layer: null, seconds_left: null, job: null, error: null, held: false },
      watched: true,
      materials: [],
      training: null, camera: false,
    };
    pop({ me: ASH, printers: [printer('Ender 3')], live: [stuck] });
    const r = render(Admin);
    await waitFor(() => expect(r.getAllByTestId('attention').map((a) => a.dataset.kind)).toEqual(['needs_hand']));
    expect(r.getByTestId('attention').textContent).toContain('Ender 3 needs a hand');
    expect(r.queryByTestId('all-clear')).toBeNull();
    cleanup();
    pop({ me: ASH, printers: [printer('Voron 2.4')], live: [] });
    const calm = render(Admin);
    await waitFor(() => calm.getByTestId('all-clear'));
    expect(calm.queryByTestId('attention')).toBeNull();
  });

  it('adds a new printer to the tower, as JSON, and shows it added', async () => {
    const calls = pop({ me: ASH, printers: [printer('Core One', { tower: 'offered', camera: true })] });
    const r = render(Admin);
    const banner = await waitFor(() => r.getByTestId('new'));
    expect(banner.textContent).toContain('New in BAM!: Core One');
    expect(banner.textContent).toContain('Its fields say Klipper, no training needed, camera fitted.');
    await fireEvent.click(within(banner).getByRole('button', { name: 'Add to the tower' }));
    await waitFor(() => expect(r.queryByTestId('new')).toBeNull());
    const add = calls.find((c) => c.method === 'POST' && c.url === '/api/admin/printers/id-Core%20One/add')!;
    expect(add.type).toBe('application/json');
    expect(calls.some((c) => c.url.endsWith('/decline'))).toBe(false);
  });

  it('declines one that is not for the tower', async () => {
    const calls = pop({ me: ASH, printers: [printer('Resin', { tower: 'offered' })] });
    const r = render(Admin);
    const banner = await waitFor(() => r.getByTestId('new'));
    await fireEvent.click(within(banner).getByRole('button', { name: 'Not this one' }));
    await waitFor(() => expect(r.getByTestId('row').textContent).toContain('Not for the tower'));
    expect(calls.some((c) => c.url === '/api/admin/printers/id-Resin/decline')).toBe(true);
    expect(calls.some((c) => c.url.endsWith('/add'))).toBe(false);
  });

  it('cannot add one whose BAM! fields are wrong, and says what is wrong', async () => {
    pop({
      me: ASH,
      printers: [printer('Ender 3', { tower: 'offered', ready: false, connection: null, problems: [{ field: 'Connection (connection)', problem: 'missing' }] })],
    });
    const r = render(Admin);
    const banner = await waitFor(() => r.getByTestId('new'));
    expect(banner.textContent).toContain('Connection (connection): missing');
    expect((within(banner).getByRole('button', { name: 'Add to the tower' }) as HTMLButtonElement).disabled).toBe(true);
  });

  it('shows POP’s own words when it refuses, as it does from outside the space', async () => {
    pop({ me: ASH, printers: [printer('Core One', { tower: 'offered' })], refuse: 'only from inside the space' });
    const r = render(Admin);
    const banner = await waitFor(() => r.getByTestId('new'));
    await fireEvent.click(within(banner).getByRole('button', { name: 'Add to the tower' }));
    await waitFor(() => expect(r.getByRole('alert').textContent).toBe('only from inside the space'));
    expect(r.getByTestId('new')).toBeTruthy();
  });

  it('signs out', async () => {
    const calls = pop({ me: ASH, printers: [] });
    const r = render(Admin);
    await waitFor(() => r.getByTestId('desk'));
    await fireEvent.click(r.getByRole('button', { name: 'Sign out' }));
    await waitFor(() => r.getByTestId('signin'));
    expect(calls.some((c) => c.url === '/api/session' && c.method === 'DELETE')).toBe(true);
  });
});
