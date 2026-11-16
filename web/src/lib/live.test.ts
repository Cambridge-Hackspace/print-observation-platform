import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  follow,
  parse,
  resumeUrl,
  FIRST_RETRY_MS,
  MAX_RETRY_MS,
  type Connection,
  type Source,
} from './live';
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
  message(data: string, lastEventId = '') {
    this.onmessage?.({ data, lastEventId } as MessageEvent);
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

const STATUS = '{"type":"status","data":{"version":"0.1.0","schema_version":1,"public_url":null}}';

describe('parse', () => {
  it('reads a snapshot', () => {
    expect(
      parse('{"type":"snapshot","data":{"status":{"version":"0.1.0","schema_version":1,"public_url":null},"printers":[]}}'),
    ).toEqual({
      type: 'snapshot',
      data: { status: { version: '0.1.0', schema_version: 1, public_url: null }, printers: [] },
    });
  });
  it('reads the printers, as the server sends them', () => {
    // A frame as pop-server serializes it (tests/run.rs sees the same shape).
    const voron = {
      id: '642f57e2',
      name: 'Voron 2.4',
      state: {
        status: 'needs_hand',
        progress: 0.37,
        layer: { current: 41, total: null },
        seconds_left: 2280,
        job: { name: 'phone_stand.gcode', owner: 'Sam Lee' },
        error: { words: 'The filament ran out', raw: 'filament runout', known: true, key: 'klipper-runout' },
      },
      watched: true,
      materials: ['PLA', 'PETG'],
      training: 'advanced',
    };
    expect(parse(JSON.stringify({ type: 'printers', data: [voron] }))).toEqual({ type: 'printers', data: [voron] });
  });
  it('reads where phones reach POP, when it is set', () => {
    const raw = '{"type":"status","data":{"version":"0.1.0","schema_version":1,"public_url":"https://pop.example.org"}}';
    expect(parse(raw)).toEqual({
      type: 'status',
      data: { version: '0.1.0', schema_version: 1, public_url: 'https://pop.example.org' },
    });
  });
  it('reads a status event', () => {
    expect(parse(STATUS)).toEqual({ type: 'status', data: { version: '0.1.0', schema_version: 1, public_url: null } });
  });
  it('refuses what is not a ServerEvent', () => {
    for (const raw of [
      'not json',
      'null',
      '[]',
      '{"type":"status"}',
      '{"type":"status","data":{"version":1,"schema_version":1}}',
      '{"type":"status","data":{"version":"0.1.0","schema_version":"1"}}',
      '{"type":"status","data":{"version":"0.1.0","schema_version":1}}',
      '{"type":"status","data":{"version":"0.1.0","schema_version":1,"public_url":7}}',
      '{"type":"printer","data":{}}',
      '{"type":"snapshot","data":{}}',
      '{"type":"snapshot","data":{"status":{"version":"0.1.0"}}}',
      '{"type":"snapshot","data":{"status":{"version":"0.1.0","schema_version":1,"public_url":null}}}',
      '{"type":"printers","data":{}}',
      '{"type":"printers","data":[{"id":"a"}]}',
      '{"type":"printers","data":[{"id":"a","name":"A","watched":true,"materials":[1],"training":null,"state":{"status":"idle","progress":null,"layer":null,"seconds_left":null,"job":null,"error":null}}]}',
      '{"type":"printers","data":[{"id":"a","name":"A","watched":"yes","materials":[],"training":null,"state":{"status":"idle","progress":null,"layer":null,"seconds_left":null,"job":null,"error":null}}]}',
      '{"type":"printers","data":[{"id":"a","name":"A","watched":true,"materials":[],"training":null,"state":{"status":"asleep","progress":null,"layer":null,"seconds_left":null,"job":null,"error":null}}]}',
      '{"type":"printers","data":[{"id":"a","name":"A","watched":true,"materials":[],"training":null,"state":{"status":"idle","progress":"half","layer":null,"seconds_left":null,"job":null,"error":null}}]}',
      '{"type":"printers","data":[{"id":"a","name":"A","watched":true,"materials":[],"training":null,"state":{"status":"idle","progress":null,"layer":{"total":3},"seconds_left":null,"job":null,"error":null}}]}',
      '{"type":"printers","data":[{"id":"a","name":"A","watched":true,"materials":[],"training":null,"state":{"status":"idle","progress":null,"layer":null,"seconds_left":null,"job":{"owner":null},"error":null}}]}',
      '{"type":"printers","data":[{"id":"a","name":"A","watched":true,"materials":[],"training":null,"state":{"status":"idle","progress":null,"layer":null,"seconds_left":null,"job":null,"error":{"words":"x","raw":"x"}}}]}',
      '{"type":"printers","data":[{"id":"a","name":"A","watched":true,"materials":[],"training":null,"state":{"status":"idle","progress":null,"layer":null,"job":null,"error":null}}]}',
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
    expect(t.events).toEqual([{ type: 'status', data: { version: '0.1.0', schema_version: 1, public_url: null } }]);
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

describe('resuming', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('builds the address with the last id, escaped', () => {
    expect(resumeUrl('/api/events', '')).toBe('/api/events');
    expect(resumeUrl('/api/events', 'a1b.42')).toBe('/api/events?last_event_id=a1b.42');
    expect(resumeUrl('/api/events?x=1', 'e&v.1')).toBe('/api/events?x=1&last_event_id=e%26v.1');
  });

  it('reopens a connection it opens itself from the last id it saw', () => {
    const t = setup();
    t.last().open();
    t.last().message(STATUS, 'ep.7');
    t.last().message(STATUS, 'ep.8');
    t.last().refuse();
    vi.advanceTimersByTime(FIRST_RETRY_MS);
    expect(t.last().url).toBe('/api/events?last_event_id=ep.8');
  });

  it('reopens plainly when it has seen nothing yet', () => {
    const t = setup();
    t.last().refuse();
    vi.advanceTimersByTime(FIRST_RETRY_MS);
    expect(t.last().url).toBe('/api/events');
  });
});
