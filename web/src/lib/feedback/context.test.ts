// @vitest-environment jsdom
// What a screen sends with a report (#96): its last errors and failed
// requests, with no token, cookie, header or query string in them.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { context, deviceOf, forget, KEEP, scrubbed, watch, withoutQuery } from './context';

describe('a report’s context', () => {
  beforeEach(() => forget());

  it('keeps failed requests without their query strings or headers', async () => {
    const answers = [new Response('{}', { status: 502 }), new Response('{}', { status: 200 })];
    vi.stubGlobal('fetch', vi.fn(async () => answers.shift()!));
    watch(window);
    await window.fetch('/api/admin/printers?access_token=planted-q-1', {
      headers: { Authorization: 'Bearer planted-h-2', Cookie: 'pop_session=planted-c-3' },
    });
    await window.fetch('/api/status');
    const c = context(window);
    expect(c.failed_requests).toEqual([{ method: 'GET', url: '/api/admin/printers', status: 502 }]);
    const all = JSON.stringify(c);
    for (const planted of ['planted-q-1', 'planted-h-2', 'planted-c-3', 'Authorization', 'Cookie']) {
      expect(all).not.toContain(planted);
    }
  });

  it('keeps console errors, scrubbed, and only the last few', () => {
    watch(window);
    const quiet = vi.spyOn(console, 'error');
    for (let i = 0; i < KEEP + 5; i++) console.error(`oops ${i}`);
    console.error('GET https://pop.example.org/api/x?last_event_id=planted-u-4 failed for ada@example.org');
    const c = context(window);
    expect(c.console_errors.length).toBe(KEEP);
    const last = c.console_errors[KEEP - 1]!;
    expect(last).toBe('GET https://pop.example.org/api/x failed for [email]');
    expect(c.console_errors[0]).toBe('oops 6');
    quiet.mockRestore();
  });

  it('says which device by where it is', () => {
    expect(deviceOf('/kiosk')).toBe('kiosk');
    expect(deviceOf('/kiosk/guides/x')).toBe('kiosk');
    expect(deviceOf('/admin')).toBe('desk');
    expect(deviceOf('/')).toBe('phone');
    expect(deviceOf('/guides')).toBe('phone');
  });

  it('takes query strings and hashes out of addresses and messages', () => {
    expect(withoutQuery('/a/b?c=d#e')).toBe('/a/b');
    expect(scrubbed('see /x?y=1 and https://h/p#frag, then stop?')).toBe('see /x and https://h/p, then stop?');
  });
});
