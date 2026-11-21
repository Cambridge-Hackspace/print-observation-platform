// The kiosk watches for card taps (#19), and stops when it never will get one.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { watchTaps } from './taps';

/** fetch answering each ask in turn from `answers` (then holding the last open forever). */
function fake(answers: { status: number; body?: unknown }[]) {
  const urls: string[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      urls.push(url);
      const a = answers.shift();
      if (!a) return new Promise<Response>(() => {});
      return new Response(a.body === undefined ? null : JSON.stringify(a.body), {
        status: a.status,
        headers: { 'Content-Type': 'application/json' },
      });
    }),
  );
  return urls;
}

const flush = () => vi.advanceTimersByTimeAsync(0);

describe('watching for card taps', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('tells each tap, and asks again at once, with or without one', async () => {
    vi.useFakeTimers();
    const urls = fake([
      { status: 200, body: { tap: null } },
      { status: 200, body: { tap: { kind: 'sign_in' } } },
      { status: 200, body: { tap: { kind: 'refused', reason: 'Unknown card' } } },
    ]);
    const seen: unknown[] = [];
    const stop = watchTaps((t) => seen.push(t));
    await flush();
    expect(seen).toEqual([{ kind: 'sign_in' }, { kind: 'refused', reason: 'Unknown card' }]);
    expect(urls).toEqual(['/api/kiosk/tap', '/api/kiosk/tap', '/api/kiosk/tap', '/api/kiosk/tap']);
    stop();
  });

  it.each([401, 404])('stops asking when POP answers %i: never a kiosk with a tap', async (status) => {
    vi.useFakeTimers();
    const urls = fake([{ status, body: { error: 'no' } }]);
    watchTaps(() => {});
    await vi.advanceTimersByTimeAsync(60_000);
    expect(urls).toHaveLength(1);
  });

  it('waits a while after anything else before asking again', async () => {
    vi.useFakeTimers();
    const urls = fake([{ status: 503, body: { error: 'restarting' } }]);
    const stop = watchTaps(() => {}, 5_000);
    await flush();
    expect(urls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(4_999);
    expect(urls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(urls).toHaveLength(2);
    stop();
  });

  it('tells nothing once stopped, even of an answer already on its way', async () => {
    vi.useFakeTimers();
    let answer: (r: Response) => void = () => {};
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>((r) => (answer = r))));
    const seen: unknown[] = [];
    const stop = watchTaps((t) => seen.push(t));
    await flush();
    stop();
    answer(new Response(JSON.stringify({ tap: { kind: 'sign_in' } }), { status: 200 }));
    await flush();
    expect(seen).toEqual([]);
  });
});
