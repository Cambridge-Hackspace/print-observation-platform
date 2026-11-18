// A session at the kiosk (#12): how long it has left, as POP counts it. The
// countdown runs from the last time POP heard from this screen, not from the
// last touch: a touch is passed on to POP (at most every PING_MS), and only
// once POP has it does the countdown start again, so the screen never shows
// time POP no longer gives.

/** At most this often does a touch reach POP. */
export const PING_MS = 10_000;

/** Seconds left, never below zero. */
export function secondsLeft(heardAtMs: number, nowMs: number, idleSecs: number): number {
  return Math.max(0, Math.ceil(idleSecs - (nowMs - heardAtMs) / 1000));
}

/** "0:45", "1:30". */
export function clockOf(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** Whether a touch now should reach POP. */
export function shouldPing(lastPingMs: number, nowMs: number): boolean {
  return nowMs - lastPingMs >= PING_MS;
}
