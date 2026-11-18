// What a screen knows, for a feedback report (#96): its route, the device it
// is, its viewport, and its last console errors and failed requests. Gathered
// from the moment the app starts, so a report made after something went
// wrong carries what went wrong.
//
// Scrubbed here before it is shown to the person filing, and again on the
// server: query strings and hashes leave every address, and redact.ts's
// backstop takes tokens, emails and long numbers out of every message.
// Request headers are never kept at all.

import type { Device } from '../generated/Device';
import { backstop } from './redact';

/** How many of each the context keeps: the most recent. */
export const KEEP = 20;

export interface FailedRequest {
  method: string;
  url: string;
  /** The status, or 0 when the request never got an answer. */
  status: number;
}

export interface Context {
  route: string;
  device: Device;
  viewport: { width: number; height: number };
  console_errors: string[];
  failed_requests: FailedRequest[];
}

/** An address without its query string or hash. */
export function withoutQuery(url: string): string {
  return url.replace(/[?#].*$/, '');
}

/** A message with addresses' query strings and anything secret taken out. */
export function scrubbed(message: string): string {
  // A query string has something after its "?": a question's mark does not.
  return backstop(message.replace(/(\S)[?#][^\s"'<>,;)]+/g, '$1'));
}

/** Which device a screen is, by its route. */
export function deviceOf(path: string): Device {
  if (path === '/kiosk' || path.startsWith('/kiosk/')) return 'kiosk';
  if (path === '/admin' || path.startsWith('/admin/')) return 'desk';
  return 'phone';
}

const errors: string[] = [];
const failed: FailedRequest[] = [];
let watching = false;

function keep<T>(list: T[], item: T) {
  list.push(item);
  if (list.length > KEEP) list.splice(0, list.length - KEEP);
}

function said(args: unknown[]): string {
  return args
    .map((a) => (a instanceof Error ? `${a.name}: ${a.message}` : typeof a === 'string' ? a : safeJson(a)))
    .join(' ');
}

function safeJson(v: unknown): string {
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
}

/** Start keeping console errors and failed requests. Once is enough. */
export function watch(win: Window & typeof globalThis = window): void {
  if (watching) return;
  watching = true;
  const error = win.console.error.bind(win.console);
  win.console.error = (...args: unknown[]) => {
    keep(errors, scrubbed(said(args)).slice(0, 500));
    error(...args);
  };
  win.addEventListener('error', (e) => keep(errors, scrubbed(e.message ?? 'error').slice(0, 500)));
  win.addEventListener('unhandledrejection', (e) =>
    keep(errors, scrubbed(said([(e as PromiseRejectionEvent).reason])).slice(0, 500)),
  );
  const fetch = win.fetch.bind(win);
  win.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const method = (init?.method ?? (input instanceof Request ? input.method : 'GET')).toUpperCase();
    const url = withoutQuery(input instanceof Request ? input.url : String(input));
    try {
      const res = await fetch(input, init);
      if (!res.ok) keep(failed, { method, url, status: res.status });
      return res;
    } catch (e) {
      keep(failed, { method, url, status: 0 });
      throw e;
    }
  };
}

/** What the screen knows now. */
export function context(win: Window = window): Context {
  return {
    route: withoutQuery(win.location.pathname),
    device: deviceOf(win.location.pathname),
    viewport: { width: win.innerWidth, height: win.innerHeight },
    console_errors: [...errors],
    failed_requests: failed.map((f) => ({ ...f })),
  };
}

/** For tests: forget everything kept. */
export function forget(): void {
  errors.length = 0;
  failed.length = 0;
}
