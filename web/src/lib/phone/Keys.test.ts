// @vitest-environment jsdom
// Your slicer keys, in the phone app's You tab (#13).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import Keys from './Keys.svelte';
import type { ApiKey } from '../generated/ApiKey';

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const SECRET = 'pop_AbcdefGhijkLmnopQrstuVwxyzAbcdefGhijkLmno';

type Call = { method: string; url: string; body?: unknown };

/** A POP holding `keys`, recording what is asked of it. */
function pop(keys: ApiKey[], refuse?: string) {
  const calls: Call[] = [];
  let next = 100;
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET';
      const body = init?.body ? JSON.parse(init.body as string) : undefined;
      calls.push({ method, url, body });
      const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
      if (url === '/api/keys' && method === 'GET') return json(keys);
      if (url === '/api/keys' && method === 'POST') {
        if (refuse) return json({ error: refuse }, 409);
        const key: ApiKey = { id: next++, name: body.name, prefix: SECRET.slice(0, 10), created_at: 1, last_used_at: null, revoked_at: null };
        keys = [key, ...keys];
        return json({ key, secret: SECRET }, 201);
      }
      const m = /^\/api\/keys\/(\d+)$/.exec(url);
      if (m && method === 'DELETE') {
        keys = keys.map((k) => (k.id === Number(m[1]) ? { ...k, revoked_at: Math.floor(Date.now() / 1000) } : k));
        return new Response(null, { status: 204 });
      }
      return json({ error: 'no such route' }, 404);
    }),
  );
  return calls;
}

const now = Math.floor(Date.now() / 1000);
const orca: ApiKey = { id: 1, name: 'Orca at home', prefix: 'pop_Xyz123', created_at: now - 9000, last_used_at: now - 120, revoked_at: null };
const old: ApiKey = { id: 2, name: 'Old Cura', prefix: 'pop_Old456', created_at: now - 99000, last_used_at: null, revoked_at: now - 3600 };

describe('your slicer keys', () => {
  it('lists live keys by name and prefix, and revoked ones apart', async () => {
    pop([orca, old]);
    const r = render(Keys, { address: 'https://pop.example.org' });
    await waitFor(() => expect(r.getAllByTestId('key')).toHaveLength(1));
    const k = r.getByTestId('key').textContent!;
    expect(k).toContain('Orca at home');
    expect(k).toContain('pop_Xyz123…');
    expect(k).toContain('used 2 minutes ago');
    expect(r.getByText('Revoked keys (1)')).toBeTruthy();
  });

  it('shows a new key once, with where it goes, and never again', async () => {
    const calls = pop([]);
    const r = render(Keys, { address: 'https://pop.example.org' });
    await waitFor(() => expect(r.getByText('You have no keys yet.')).toBeTruthy());
    await fireEvent.input(r.getByLabelText('What the key is for'), { target: { value: '  PrusaSlicer  ' } });
    await fireEvent.submit(r.getByLabelText('What the key is for').closest('form')!);
    await waitFor(() => expect(r.getByTestId('secret').textContent).toBe(SECRET));
    expect(calls.find((c) => c.method === 'POST')).toMatchObject({ url: '/api/keys', body: { name: 'PrusaSlicer' } });
    const made = r.getByTestId('made').textContent!;
    expect(made).toContain('won’t show it again');
    expect(made).toContain('OctoPrint');
    expect(made).toContain('https://pop.example.org');
    // Hidden from a feedback screenshot.
    expect(r.getByTestId('secret').getAttribute('data-sensitive')).toBe('secret');
    await fireEvent.click(r.getByRole('button', { name: 'Done' }));
    await waitFor(() => expect(r.queryByTestId('made')).toBeNull());
    expect(r.container.textContent).not.toContain(SECRET);
    // The list reloads after the key is made: listed once that is back.
    await waitFor(() => expect(r.getByTestId('key').textContent).toContain('PrusaSlicer'));
  });

  it('asks before revoking, then revokes', async () => {
    const calls = pop([orca]);
    const r = render(Keys, { address: 'https://pop.example.org' });
    await waitFor(() => expect(r.getAllByTestId('key')).toHaveLength(1));
    await fireEvent.click(r.getByRole('button', { name: 'Revoke the key Orca at home' }));
    // Not yet: a second, plain choice.
    expect(calls.some((c) => c.method === 'DELETE')).toBe(false);
    await fireEvent.click(r.getByRole('button', { name: 'Keep it' }));
    expect(calls.some((c) => c.method === 'DELETE')).toBe(false);
    await fireEvent.click(r.getByRole('button', { name: 'Revoke the key Orca at home' }));
    await fireEvent.click(r.getByRole('button', { name: 'Revoke it' }));
    await waitFor(() => expect(r.queryAllByTestId('key')).toHaveLength(0));
    expect(calls.filter((c) => c.method === 'DELETE').map((c) => c.url)).toEqual(['/api/keys/1']);
    expect(r.getByText('Revoked keys (1)')).toBeTruthy();
  });

  it('says why POP refused a new key', async () => {
    pop([], 'you have 20 keys already: revoke one you no longer use first');
    const r = render(Keys, { address: 'https://pop.example.org' });
    await waitFor(() => expect(r.getByText('You have no keys yet.')).toBeTruthy());
    await fireEvent.input(r.getByLabelText('What the key is for'), { target: { value: 'One more' } });
    await fireEvent.submit(r.getByLabelText('What the key is for').closest('form')!);
    await waitFor(() => expect(r.getByRole('alert').textContent).toContain('20 keys already'));
    expect(r.queryByTestId('made')).toBeNull();
  });
});
