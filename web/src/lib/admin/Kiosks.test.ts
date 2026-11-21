// @vitest-environment jsdom
// The desk's kiosks (#12): make this screen one, see each one's last word,
// revoke one.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import type { Kiosk } from '../generated/Kiosk';
import Kiosks from './Kiosks.svelte';
import { ago } from './kiosks';

type Call = { method: string; url: string; body: unknown };

function pop(start: Kiosk[]) {
  let kiosks = start;
  const calls: Call[] = [];
  const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit = {}) => {
      const method = init.method ?? 'GET';
      const body = typeof init.body === 'string' ? JSON.parse(init.body) : null;
      calls.push({ method, url, body });
      if (url === '/api/admin/kiosks' && method === 'GET') return json(kiosks);
      if (url === '/api/admin/kiosks' && method === 'POST') {
        kiosks = [...kiosks, { id: kiosks.length + 1, name: body.name, enrolled_at: 1, revoked: false, last_seen_at: null, last_address: null, card_resource: null }];
        return json({ id: kiosks.length }, 201);
      }
      const r = /^\/api\/admin\/kiosks\/(\d+)\/card-reader$/.exec(url);
      if (r && method === 'PUT') {
        if (kiosks.some((k) => k.card_resource === body.resource && k.id !== Number(r[1]))) {
          return json({ error: 'another kiosk already has that card reader: take it off that one first' }, 409);
        }
        kiosks = kiosks.map((k) => (k.id === Number(r[1]) ? { ...k, card_resource: body.resource || null } : k));
        return new Response(null, { status: 204 });
      }
      const m = /^\/api\/admin\/kiosks\/(\d+)$/.exec(url);
      if (m && method === 'DELETE') {
        kiosks = kiosks.map((k) => (k.id === Number(m[1]) ? { ...k, revoked: true } : k));
        return new Response(null, { status: 204 });
      }
      return json({ error: 'no such route' }, 404);
    }),
  );
  return calls;
}

const SEEN = Math.round(Date.now() / 1000) - 20;

describe('the desk’s kiosks', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('says plainly that nothing is the space until a kiosk checks in', async () => {
    pop([]);
    const r = render(Kiosks);
    await waitFor(() => expect(r.getByText(/No kiosk yet/).textContent).toContain('nothing counts as the space'));
  });

  it('makes this screen a kiosk, by name', async () => {
    const calls = pop([]);
    const r = render(Kiosks);
    await waitFor(() => r.getByText(/No kiosk yet/));
    await fireEvent.input(r.getByLabelText('Make this screen a kiosk'), { target: { value: '  Tower screen ' } });
    await fireEvent.submit(r.getByRole('button', { name: 'Make this screen a kiosk' }).closest('form')!);
    await waitFor(() => expect(r.getAllByTestId('kiosk')).toHaveLength(1));
    expect(calls.find((c) => c.method === 'POST')).toMatchObject({ url: '/api/admin/kiosks', body: { name: 'Tower screen' } });
    expect(r.getByRole('status').textContent).toContain('This screen is now the kiosk “Tower screen”');
    expect(r.getByTestId('kiosk').textContent).toContain('Never checked in');
  });

  it('shows where and when each kiosk last checked in, and revokes one', async () => {
    const calls = pop([
      { id: 1, name: 'The tower', enrolled_at: 1, revoked: false, last_seen_at: SEEN, last_address: '192.168.1.5', card_resource: null },
      { id: 2, name: 'Old screen', enrolled_at: 1, revoked: true, last_seen_at: null, last_address: null, card_resource: null },
    ]);
    const r = render(Kiosks);
    await waitFor(() => expect(r.getAllByTestId('kiosk')).toHaveLength(2));
    const [tower, old] = r.getAllByTestId('kiosk');
    expect(tower!.textContent).toMatch(/Checked in \d+ seconds ago from 192\.168\.1\.5/);
    expect(old!.textContent).toContain('Revoked');
    expect(r.queryByRole('button', { name: 'Revoke the kiosk Old screen' })).toBeNull();
    await fireEvent.click(r.getByRole('button', { name: 'Revoke the kiosk The tower' }));
    await waitFor(() => expect(r.getAllByTestId('kiosk')[0]!.textContent).toContain('Revoked'));
    expect(calls.filter((c) => c.method === 'DELETE').map((c) => c.url)).toEqual(['/api/admin/kiosks/1']);
  });

  it('sets, shows and clears a kiosk’s card reader, and says when another has it (#19)', async () => {
    const calls = pop([
      { id: 1, name: 'The tower', enrolled_at: 1, revoked: false, last_seen_at: SEEN, last_address: '192.168.1.5', card_resource: null },
      { id: 2, name: 'Spare', enrolled_at: 1, revoked: false, last_seen_at: null, last_address: null, card_resource: 'tower-2' },
      { id: 3, name: 'Old screen', enrolled_at: 1, revoked: true, last_seen_at: null, last_address: null, card_resource: null },
    ]);
    const r = render(Kiosks);
    await waitFor(() => expect(r.getAllByTestId('kiosk')).toHaveLength(3));
    const fields = () => r.getAllByTestId('kiosk-reader') as HTMLInputElement[];
    // A revoked kiosk has no reader to set.
    expect(fields().map((f) => f.value)).toEqual(['', 'tower-2']);

    await fireEvent.input(fields()[0]!, { target: { value: ' tower-1 ' } });
    await fireEvent.click(r.getByRole('button', { name: 'Save the card reader of The tower' }));
    await waitFor(() => expect(r.getByRole('status').textContent).toBe('A card tap at tower-1 now signs in at The tower.'));
    expect(calls.find((c) => c.method === 'PUT')).toMatchObject({ url: '/api/admin/kiosks/1/card-reader', body: { resource: 'tower-1' } });
    await waitFor(() => expect(fields()[0]!.value).toBe('tower-1'));

    // Another kiosk's reader: POP's words, and nothing changed.
    await fireEvent.input(fields()[0]!, { target: { value: 'tower-2' } });
    await fireEvent.click(r.getByRole('button', { name: 'Save the card reader of The tower' }));
    await waitFor(() => expect(r.getByRole('alert').textContent).toContain('another kiosk already has that card reader'));
    await waitFor(() => expect(fields()[0]!.value).toBe('tower-1'));

    // Emptied, it is cleared.
    await fireEvent.input(fields()[1]!, { target: { value: '' } });
    await fireEvent.click(r.getByRole('button', { name: 'Save the card reader of Spare' }));
    await waitFor(() => expect(r.getByRole('status').textContent).toBe('Spare has no card reader now.'));
    expect(calls.filter((c) => c.method === 'PUT').at(-1)).toMatchObject({ url: '/api/admin/kiosks/2/card-reader', body: { resource: '' } });
  });

  it('says how long ago, in words', () => {
    const now = 1_000_000_000;
    expect(ago(now / 1000 - 1, now)).toBe('1 second ago');
    expect(ago(now / 1000 - 59, now)).toBe('59 seconds ago');
    expect(ago(now / 1000 - 60, now)).toBe('1 minute ago');
    expect(ago(now / 1000 - 7200, now)).toBe('2 hours ago');
    expect(ago(now / 1000 - 3 * 86400, now)).toBe('3 days ago');
    expect(ago(now / 1000 + 5, now)).toBe('0 seconds ago');
  });
});
