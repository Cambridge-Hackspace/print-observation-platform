// The kiosk checks in every half minute, and stops when it is not a kiosk (#12).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { startCheckins } from './checkin';

function fake(status: number) {
  const calls: { url: string; method: string; type: string | null }[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: RequestInit = {}) => {
      calls.push({ url, method: init.method ?? 'GET', type: new Headers(init.headers).get('Content-Type') });
      return new Response(null, { status });
    }),
  );
  return calls;
}

const flush = () => vi.advanceTimersByTimeAsync(0);

describe('the kiosk’s check-in', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('checks in at once and every half minute, as JSON, until stopped', async () => {
    vi.useFakeTimers();
    const calls = fake(204);
    const stop = startCheckins();
    await flush();
    expect(calls).toEqual([{ url: '/api/kiosk/checkin', method: 'POST', type: 'application/json' }]);
    await vi.advanceTimersByTimeAsync(29_999);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(calls).toHaveLength(2);
    stop();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(calls).toHaveLength(2);
  });

  it('stops asking when this screen is not an enrolled kiosk', async () => {
    vi.useFakeTimers();
    const calls = fake(401);
    startCheckins();
    await flush();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(calls).toHaveLength(1);
  });

  it('keeps asking through a passing failure', async () => {
    vi.useFakeTimers();
    const calls = fake(503);
    const stop = startCheckins();
    await flush();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(calls).toHaveLength(3);
    stop();
  });
});
