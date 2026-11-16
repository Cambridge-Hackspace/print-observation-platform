import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { follow, FIRST_RETRY_MS, type Source } from './live';
import { empty, reduce, type ScreenState } from './screen';
import type { ServerEvent } from './generated/ServerEvent';
import type { TowerPrinter } from './generated/TowerPrinter';

const status = (n: number): ServerEvent => ({ type: 'status', data: { version: '0.1.0', schema_version: n, public_url: null } });
const printer = (name: string, status: 'idle' | 'printing'): TowerPrinter => ({
  id: name,
  name,
  watched: true,
  materials: ['PLA'],
  training: null,
  state: { status, progress: null, layer: null, seconds_left: null, job: null, error: null },
});
const snapshot = (n: number, printers: TowerPrinter[] = []): ServerEvent => ({
  type: 'snapshot',
  data: { status: { version: '0.1.0', schema_version: n, public_url: null }, printers },
});

describe('reduce', () => {
  it('replaces everything on a snapshot, and the status on a status event', () => {
    let s = reduce(empty(), snapshot(1));
    expect(s.status?.schema_version).toBe(1);
    s = reduce(s, status(2));
    expect(s.status?.schema_version).toBe(2);
    s = reduce(s, snapshot(9));
    expect(s.status?.schema_version).toBe(9);
  });

  it('replaces the printers on a printers event, and on a snapshot', () => {
    let s = reduce(empty(), snapshot(1, [printer('Voron', 'idle')]));
    expect(s.printers.map((p) => p.state.status)).toEqual(['idle']);
    s = reduce(s, { type: 'printers', data: [printer('Voron', 'printing'), printer('X1', 'idle')] });
    expect(s.printers.map((p) => p.name)).toEqual(['Voron', 'X1']);
    expect(s.status?.schema_version).toBe(1);
    s = reduce(s, snapshot(2));
    expect(s.printers).toEqual([]);
  });

  it('never changes the state it is given', () => {
    const before = reduce(empty(), snapshot(1));
    const copy = structuredClone(before);
    reduce(before, status(2));
    expect(before).toEqual(copy);
  });
});

/** A fake connection the test feeds. */
class Fake implements Source {
  readyState = 0;
  onopen: ((ev: Event) => unknown) | null = null;
  onmessage: ((ev: MessageEvent) => unknown) | null = null;
  onerror: ((ev: Event) => unknown) | null = null;
  constructor(readonly url: string) {}
  send(e: ServerEvent, id: string) {
    this.readyState = 1;
    this.onmessage?.({ data: JSON.stringify(e), lastEventId: id } as MessageEvent);
  }
  refuse() {
    this.readyState = 2;
    this.onerror?.(new Event('error'));
  }
  close() {
    this.readyState = 2;
  }
}

/** A screen following the stream through `follow`, as the app does. */
function screen() {
  let state: ScreenState = empty();
  const conns: Fake[] = [];
  follow('/api/events', { event: (e) => (state = reduce(state, e)), connection: () => {} }, (u) => {
    const c = new Fake(u);
    conns.push(c);
    return c;
  });
  return { state: () => state, conn: () => conns[conns.length - 1]!, conns };
}

describe('a screen that drops and resumes', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  // The issue's "done when", from the screen's side: the stream breaks,
  // the screen reconnects from the last id it saw, the server (playing its
  // part here) sends what was missed, and the screen ends where one that
  // never dropped does.
  it('ends in the same state as one that never dropped', () => {
    const events = [snapshot(0), ...Array.from({ length: 20 }, (_, i) => status(i + 1))];
    const ids = events.map((_, i) => `ep.${i}`);

    const steady = screen();
    events.forEach((e, i) => steady.conn().send(e, ids[i]!));

    const flaky = screen();
    events.slice(0, 8).forEach((e, i) => flaky.conn().send(e, ids[i]!));
    flaky.conn().refuse();
    vi.advanceTimersByTime(FIRST_RETRY_MS);
    const resumed = flaky.conn();
    expect(resumed.url).toBe('/api/events?last_event_id=ep.7');
    // The server replays from the id it was given.
    const from = Number(new URL(resumed.url, 'http://x').searchParams.get('last_event_id')!.split('.')[1]) + 1;
    events.slice(from).forEach((e, i) => resumed.send(e, ids[from + i]!));

    expect(flaky.state()).toEqual(steady.state());
  });

  it('ends there too when it is sent a snapshot instead', () => {
    const steady = screen();
    steady.conn().send(snapshot(0), 'ep.0');
    steady.conn().send(status(5), 'ep.1');
    const flaky = screen();
    flaky.conn().send(snapshot(0), 'ep.0');
    flaky.conn().refuse();
    vi.advanceTimersByTime(FIRST_RETRY_MS);
    // Too late to replay: the server sends what is true now.
    flaky.conn().send(snapshot(5), 'ep.1');
    expect(flaky.state()).toEqual(steady.state());
  });
});
