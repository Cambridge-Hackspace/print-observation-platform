// The live-update stream from the server (`GET /api/events`; ADR 0001, #6).
//
// Browsers reconnect an EventSource on their own after a dropped connection,
// sending the last event id they saw, and the server replays what they
// missed (or a snapshot, if it is too late for that). But a browser gives up
// for good when the server answers with an error instead, as a proxy does
// while POP restarts. A kiosk must not give up, so when that happens this
// opens a new connection itself, waiting longer each time, up to a limit,
// and says the last id it saw in the address, which a new EventSource cannot
// send as a header.

import type { ServerEvent } from './generated/ServerEvent';
import type { Status } from './generated/Status';
import type { TowerPrinter } from './generated/TowerPrinter';
import type { PrinterState } from './generated/PrinterState';
import type { PrinterStatus } from './generated/PrinterStatus';

export type Connection = 'connecting' | 'live' | 'reconnecting';

export interface Handlers {
  event(e: ServerEvent): void;
  connection(c: Connection): void;
  /** A message that was not a ServerEvent. It is dropped, not acted on. */
  invalid?(raw: string): void;
}

/** The parts of EventSource this uses, so tests can stand in for it. */
export interface Source {
  readonly readyState: number;
  onopen: ((ev: Event) => unknown) | null;
  onmessage: ((ev: MessageEvent) => unknown) | null;
  onerror: ((ev: Event) => unknown) | null;
  close(): void;
}

/** EventSource.CLOSED: the browser has given up on this connection. */
const CLOSED = 2;
export const FIRST_RETRY_MS = 1_000;
export const MAX_RETRY_MS = 30_000;

function status(v: unknown): Status | null {
  if (typeof v !== 'object' || v === null) return null;
  const { version, schema_version, public_url } = v as Record<string, unknown>;
  return typeof version === 'string' &&
    typeof schema_version === 'number' &&
    (public_url === null || typeof public_url === 'string')
    ? { version, schema_version, public_url }
    : null;
}

const STATUSES: readonly PrinterStatus[] = ['offline', 'idle', 'printing', 'paused', 'needs_hand', 'done'];

const obj = (v: unknown): Record<string, unknown> | null =>
  typeof v === 'object' && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
const num = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const str = (v: unknown): v is string => typeof v === 'string';
/** `v` read by `read`, or null when `v` is null; undefined when neither. */
function nullable<T>(v: unknown, read: (v: unknown) => T | null): T | null | undefined {
  if (v === null) return null;
  const r = read(v);
  return r === null ? undefined : r;
}

function printerState(v: unknown): PrinterState | null {
  const o = obj(v);
  if (!o || !STATUSES.includes(o.status as PrinterStatus)) return null;
  const progress = nullable(o.progress, (p) => (num(p) ? p : null));
  const seconds_left = nullable(o.seconds_left, (p) => (num(p) ? p : null));
  const layer = nullable(o.layer, (l) => {
    const x = obj(l);
    if (!x || !num(x.current)) return null;
    const total = nullable(x.total, (t) => (num(t) ? t : null));
    return total === undefined ? null : { current: x.current, total };
  });
  const job = nullable(o.job, (j) => {
    const x = obj(j);
    if (!x || !str(x.name)) return null;
    const owner = nullable(x.owner, (w) => (str(w) ? w : null));
    return owner === undefined ? null : { name: x.name, owner };
  });
  const error = nullable(o.error, (e) => {
    const x = obj(e);
    return x && str(x.words) && str(x.raw) && typeof x.known === 'boolean'
      ? { words: x.words, raw: x.raw, known: x.known }
      : null;
  });
  if ([progress, seconds_left, layer, job, error].includes(undefined)) return null;
  return {
    status: o.status as PrinterStatus,
    progress: progress!,
    seconds_left: seconds_left!,
    layer: layer!,
    job: job!,
    error: error!,
  } as PrinterState;
}

function printer(v: unknown): TowerPrinter | null {
  const o = obj(v);
  if (!o || !str(o.id) || !str(o.name) || typeof o.watched !== 'boolean') return null;
  if (!Array.isArray(o.materials) || !o.materials.every(str)) return null;
  const training = nullable(o.training, (t) => (str(t) ? t : null));
  const state = printerState(o.state);
  if (training === undefined || !state) return null;
  return { id: o.id, name: o.name, state, watched: o.watched, materials: [...o.materials], training };
}

/** Every printer, or null if any is not one. */
function printers(v: unknown): TowerPrinter[] | null {
  if (!Array.isArray(v)) return null;
  const out = v.map(printer);
  return out.every((p) => p !== null) ? (out as TowerPrinter[]) : null;
}

/** Parses one message, or returns null if it is not a ServerEvent. */
export function parse(raw: string): ServerEvent | null {
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof v !== 'object' || v === null) return null;
  const { type, data } = v as { type?: unknown; data?: unknown };
  if (type === 'status') {
    const s = status(data);
    return s ? { type, data: s } : null;
  }
  if (type === 'printers') {
    const ps = printers(data);
    return ps ? { type, data: ps } : null;
  }
  if (type === 'snapshot' && typeof data === 'object' && data !== null) {
    const d = data as Record<string, unknown>;
    const s = status(d.status);
    const ps = printers(d.printers);
    return s && ps ? { type, data: { status: s, printers: ps } } : null;
  }
  return null;
}

/** The address to reconnect to, carrying the last event id seen, if any. */
export function resumeUrl(url: string, lastId: string): string {
  if (!lastId) return url;
  const sep = url.includes('?') ? '&' : '?';
  return `${url}${sep}last_event_id=${encodeURIComponent(lastId)}`;
}

/**
 * Follows the stream at `url` until the returned function is called.
 * `open` makes the connection: `new EventSource(url)` unless a test says otherwise.
 */
export function follow(
  url: string,
  h: Handlers,
  open: (url: string) => Source = (u) => new EventSource(u),
): () => void {
  let source: Source | null = null;
  let retry: ReturnType<typeof setTimeout> | null = null;
  let delay = FIRST_RETRY_MS;
  let stopped = false;
  let lastId = '';

  const connect = () => {
    retry = null;
    const s = open(resumeUrl(url, lastId));
    source = s;
    s.onopen = () => {
      delay = FIRST_RETRY_MS;
      h.connection('live');
    };
    s.onmessage = (m: MessageEvent) => {
      if (m.lastEventId) lastId = m.lastEventId;
      const e = parse(String(m.data));
      if (e) h.event(e);
      else h.invalid?.(String(m.data));
    };
    s.onerror = () => {
      if (stopped) return;
      h.connection('reconnecting');
      if (s.readyState !== CLOSED) return; // the browser is retrying itself
      s.close();
      retry = setTimeout(connect, delay);
      delay = Math.min(delay * 2, MAX_RETRY_MS);
    };
  };

  h.connection('connecting');
  connect();
  return () => {
    stopped = true;
    if (retry !== null) clearTimeout(retry);
    source?.close();
  };
}
