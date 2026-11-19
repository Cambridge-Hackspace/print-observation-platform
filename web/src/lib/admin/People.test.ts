// @vitest-environment jsdom
// Managing local users (#90) on the admin desk, against a stand-in for POP.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/svelte';
import People from './People.svelte';
import type { Account } from '../generated/Account';
import type { AdminPrinter } from '../generated/AdminPrinter';

const ME = { id: 1, username: 'ash', name: 'Ash', role: 'admin' as const };
const VORON: AdminPrinter = {
  bam_id: 'v',
  name: 'Voron 2.4',
  ready: true,
  connection: 'moonraker',
  problems: [],
  tower: 'added',
  categories: [],
  training: 'advanced',
  camera: false,
  access_code: false,
  camera_login_fits: false,
  camera_login: false,
  printer_login_fits: false,
  printer_login: false,
};

interface Call {
  method: string;
  url: string;
  body: Record<string, unknown> | null;
}

function pop(start: Account[], refuse: Record<string, string> = {}) {
  let accounts = start.map((a) => ({ ...a }));
  const calls: Call[] = [];
  const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit = {}) => {
      const method = init.method ?? 'GET';
      const body = typeof init.body === 'string' ? (JSON.parse(init.body) as Record<string, unknown>) : null;
      calls.push({ method, url, body });
      if (url === '/api/admin/users' && method === 'GET') return json(accounts);
      if (url === '/api/admin/users' && method === 'POST') {
        const a: Account = {
          id: accounts.length + 10,
          username: String(body!.username),
          name: String(body!.name),
          role: body!.role as 'member',
          active: true,
          training: [],
        };
        accounts = [...accounts, a];
        return json(a, 201);
      }
      const m = /^\/api\/admin\/users\/(\d+)\/(\w+)$/.exec(url);
      if (m && method === 'PUT') {
        if (refuse[m[2]!]) return json({ error: refuse[m[2]!] }, 409);
        const id = Number(m[1]);
        accounts = accounts.map((a) => (a.id === id && m[2] !== 'password' ? { ...a, ...body } : a));
        return m[2] === 'password' ? new Response(null, { status: 204 }) : json(accounts.find((a) => a.id === id));
      }
      return json({ error: 'no such route' }, 404);
    }),
  );
  return calls;
}

const grace: Account = { id: 2, username: 'grace', name: 'Grace', role: 'member', active: true, training: [] };
const ash: Account = { id: 1, username: 'ash', name: 'Ash', role: 'admin', active: true, training: ['basic'] };
const puts = (calls: Call[]) => calls.filter((c) => c.method === 'PUT').map((c) => [c.url, c.body]);

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('people', () => {
  it('lists every account with its role, training and whether it is active', async () => {
    pop([ash, { ...grace, active: false }]);
    const r = render(People, { printers: [VORON], me: ME });
    await waitFor(() => expect(r.getAllByTestId('person')).toHaveLength(2));
    const [first, second] = r.getAllByTestId('person');
    expect(first!.textContent).toContain('ash · Admin · basic');
    expect(second!.textContent).toContain('grace · Member · no training · deactivated');
    expect(within(second!).getByRole('button', { name: 'Reactivate' })).toBeTruthy();
  });

  it('will not let an admin deactivate themselves from here', async () => {
    pop([ash, grace]);
    const r = render(People, { printers: [], me: ME });
    await waitFor(() => r.getAllByTestId('person'));
    const mine = within(r.getAllByTestId('person')[0]!).getByRole('button', { name: 'Deactivate' }) as HTMLButtonElement;
    expect(mine.disabled).toBe(true);
    const hers = within(r.getAllByTestId('person')[1]!).getByRole('button', { name: 'Deactivate' }) as HTMLButtonElement;
    expect(hers.disabled).toBe(false);
  });

  it('sends only what changed: a new name and training ticked from the printers’, then a password', async () => {
    const calls = pop([grace]);
    const r = render(People, { printers: [VORON], me: ME });
    await waitFor(() => r.getByTestId('person'));
    await fireEvent.click(r.getByRole('button', { name: 'Edit' }));
    const form = r.getByTestId('edit-person');
    await fireEvent.input(within(form).getByLabelText('Name'), { target: { value: 'Grace Hopper' } });
    await fireEvent.click(within(form).getByLabelText('advanced'));
    await fireEvent.input(within(form).getByLabelText(/^Another/), { target: { value: 'resin' } });
    await fireEvent.input(within(form).getByLabelText(/^New password/), { target: { value: 'a new long password' } });
    await fireEvent.submit(form);
    await waitFor(() => expect(r.getByRole('status').textContent).toBe('Saved Grace Hopper.'));
    expect(puts(calls)).toEqual([
      ['/api/admin/users/2/name', { name: 'Grace Hopper' }],
      ['/api/admin/users/2/training', { training: ['advanced', 'resin'] }],
      ['/api/admin/users/2/password', { password: 'a new long password' }],
    ]);
    expect(r.getByTestId('person').textContent).toContain('Member · advanced, resin');
  });

  it('sends nothing when nothing changed', async () => {
    const calls = pop([grace]);
    const r = render(People, { printers: [VORON], me: ME });
    await waitFor(() => r.getByTestId('person'));
    await fireEvent.click(r.getByRole('button', { name: 'Edit' }));
    await fireEvent.submit(r.getByTestId('edit-person'));
    await waitFor(() => r.getByRole('status'));
    expect(puts(calls)).toEqual([]);
  });

  it('shows POP’s words when it refuses, and stops there', async () => {
    const calls = pop([ash], { role: 'role: this is the only admin; make someone else an admin first' });
    const r = render(People, { printers: [], me: { ...ME, id: 99 } });
    await waitFor(() => r.getByTestId('person'));
    await fireEvent.click(r.getByRole('button', { name: 'Edit' }));
    const form = r.getByTestId('edit-person');
    await fireEvent.change(within(form).getByLabelText('Role'), { target: { value: 'member' } });
    await fireEvent.input(within(form).getByLabelText(/^New password/), { target: { value: 'a new long password' } });
    await fireEvent.submit(form);
    await waitFor(() => expect(r.getByRole('alert').textContent).toContain('only admin'));
    // Time for anything it would have sent next.
    await new Promise((done) => setTimeout(done, 50));
    expect(puts(calls).map((c) => c[0])).toEqual(['/api/admin/users/1/role']);
    expect(r.getByTestId('edit-person')).toBeTruthy();
    expect(r.queryByRole('status')).toBeNull();
  });

  it('deactivates and reactivates', async () => {
    const calls = pop([grace]);
    const r = render(People, { printers: [], me: ME });
    await waitFor(() => r.getByTestId('person'));
    await fireEvent.click(r.getByRole('button', { name: 'Deactivate' }));
    await waitFor(() => expect(r.getByRole('status').textContent).toBe('Grace is signed out and can’t sign in.'));
    await fireEvent.click(r.getByRole('button', { name: 'Reactivate' }));
    await waitFor(() => expect(r.getByRole('status').textContent).toBe('Grace can sign in again.'));
    expect(puts(calls)).toEqual([
      ['/api/admin/users/2/active', { active: false }],
      ['/api/admin/users/2/active', { active: true }],
    ]);
  });

  it('adds a person', async () => {
    const calls = pop([ash]);
    const r = render(People, { printers: [], me: ME });
    await waitFor(() => r.getByTestId('person'));
    await fireEvent.click(r.getByRole('button', { name: 'Add a person' }));
    const form = r.getByTestId('add-person');
    await fireEvent.input(within(form).getByLabelText('Name'), { target: { value: 'Sam Lee' } });
    await fireEvent.input(within(form).getByLabelText(/^Username/), { target: { value: 'sam' } });
    await fireEvent.input(within(form).getByLabelText('Password'), { target: { value: 'a long password' } });
    await fireEvent.submit(form);
    await waitFor(() => expect(r.getByRole('status').textContent).toBe('Added Sam Lee. They sign in as sam.'));
    expect(r.getAllByTestId('person')).toHaveLength(2);
    expect(calls.find((c) => c.method === 'POST')!.body).toEqual({
      username: 'sam',
      name: 'Sam Lee',
      password: 'a long password',
      role: 'member',
    });
  });
});
