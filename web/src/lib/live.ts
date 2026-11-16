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
  const { version, schema_version } = v as Record<string, unknown>;
  return typeof version === 'string' && typeof schema_version === 'number'
    ? { version, schema_version }
    : null;
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
  if (type === 'snapshot' && typeof data === 'object' && data !== null) {
    const s = status((data as Record<string, unknown>).status);
    return s ? { type, data: { status: s } } : null;
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
