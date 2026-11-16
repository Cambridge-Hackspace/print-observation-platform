import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { follow, parse, FIRST_RETRY_MS, MAX_RETRY_MS, type Connection, type Source } from './live';
import type { ServerEvent } from './generated/ServerEvent';

/** Stands in for an EventSource; the test drives it. */
class FakeSource implements Source {
  readyState = 0;
  onopen: ((ev: Event) => unknown) | null = null;
  onmessage: ((ev: MessageEvent) => unknown) | null = null;
  onerror: ((ev: Event) => unknown) | null = null;
  closed = false;
  constructor(readonly url: string) {}
  open() {
    this.readyState = 1;
    this.onopen?.(new Event('open'));
  }
  message(data: string) {
    this.onmessage?.({ data } as MessageEvent);
  }
  /** The browser lost the connection and will retry it itself. */
  drop() {
    this.readyState = 0;
    this.onerror?.(new Event('error'));
  }
  /** The browser gave up, as on an HTTP error from a proxy. */
  refuse() {
    this.readyState = 2;
    this.onerror?.(new Event('error'));
  }
  close() {
    this.closed = true;
    this.readyState = 2;
  }
}

function setup() {
  const sources: FakeSource[] = [];
  const events: ServerEvent[] = [];
  const states: Connection[] = [];
  const invalid: string[] = [];
  const stop = follow(
    '/api/events',
    { event: (e) => events.push(e), connection: (c) => states.push(c), invalid: (r) => invalid.push(r) },
    (url) => {
      const s = new FakeSource(url);
      sources.push(s);
      return s;
    },
  );
  const last = () => sources[sources.length - 1]!;
  return { sources, events, states, invalid, stop, last };
}

const STATUS = '{"type":"status","data":{"version":"0.1.0","schema_version":1}}';

describe('parse', () => {
  it('reads a status event', () => {
    expect(parse(STATUS)).toEqual({ type: 'status', data: { version: '0.1.0', schema_version: 1 } });
  });
  it('refuses what is not a ServerEvent', () => {
    for (const raw of [
      'not json',
      'null',
      '[]',
      '{"type":"status"}',
      '{"type":"status","data":{"version":1,"schema_version":1}}',
      '{"type":"status","data":{"version":"0.1.0","schema_version":"1"}}',
      '{"type":"printer","data":{}}',
    ]) {
      expect(parse(raw), raw).toBeNull();
    }
  });
});

describe('follow', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('connects, then says live once the stream opens', () => {
    const t = setup();
    expect(t.sources.map((s) => s.url)).toEqual(['/api/events']);
    expect(t.states).toEqual(['connecting']);
    t.last().open();
    expect(t.states).toEqual(['connecting', 'live']);
  });

  it('hands on each event, and drops a malformed message without acting on it', () => {
    const t = setup();
    t.last().open();
    t.last().message(STATUS);
    t.last().message('{"type":"status","data":{}}');
    expect(t.events).toEqual([{ type: 'status', data: { version: '0.1.0', schema_version: 1 } }]);
    expect(t.invalid).toEqual(['{"type":"status","data":{}}']);
    expect(t.states.at(-1)).toBe('live');
  });

  it('leaves a dropped connection to the browser to retry', () => {
    const t = setup();
    t.last().open();
    t.last().drop();
    expect(t.states.at(-1)).toBe('reconnecting');
    vi.advanceTimersByTime(MAX_RETRY_MS * 2);
    expect(t.sources).toHaveLength(1);
    t.last().open();
    expect(t.states.at(-1)).toBe('live');
  });

  it('opens a new connection when the browser gives up, waiting longer each time, up to a limit', () => {
    const t = setup();
    const waits: number[] = [];
    for (let i = 0; i < 8; i++) {
      t.last().refuse();
      expect(t.last().closed).toBe(true);
      const before = t.sources.length;
      let waited = 0;
      while (t.sources.length === before) {
        vi.advanceTimersByTime(100);
        waited += 100;
        expect(waited, 'no new connection within twice the limit').toBeLessThanOrEqual(MAX_RETRY_MS * 2);
      }
      waits.push(waited);
    }
    expect(waits.slice(0, 3)).toEqual([FIRST_RETRY_MS, FIRST_RETRY_MS * 2, FIRST_RETRY_MS * 4]);
    expect(Math.max(...waits)).toBe(MAX_RETRY_MS);
  });

  it('starts waiting from the beginning again after a connection opens', () => {
    const t = setup();
    t.last().refuse();
    vi.advanceTimersByTime(FIRST_RETRY_MS);
    t.last().refuse();
    vi.advanceTimersByTime(FIRST_RETRY_MS * 2);
    t.last().open();
    t.last().refuse();
    const before = t.sources.length;
    vi.advanceTimersByTime(FIRST_RETRY_MS);
    expect(t.sources.length).toBe(before + 1);
  });

  it('stops for good when told to, even with a retry pending', () => {
    const t = setup();
    t.last().refuse();
    t.stop();
    vi.advanceTimersByTime(MAX_RETRY_MS * 4);
    expect(t.sources).toHaveLength(1);
    const t2 = setup();
    t2.last().open();
    t2.stop();
    expect(t2.last().closed).toBe(true);
  });
});
