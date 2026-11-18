// Watching a camera through POP (#61, #67): the rules, apart from the
// browser. CameraView.svelte plays what these decide.
//
// POP serves a camera two ways. MSE, over a WebSocket, for cameras that send
// H.264: the screen says which codecs it can play, and POP answers with the
// stream's type and then its video, or with why not. MJPEG, a plain address an
// <img> shows, for cameras that send JPEG pictures; POP says "unsupported" on
// the MSE socket for those, and the screen falls back to it.

/** What a screen offers to play, as go2rtc's own player offers them. */
export const CANDIDATES: readonly { codec: string; kind: 'video' | 'audio' }[] = [
  { codec: 'avc1.640029', kind: 'video' },
  { codec: 'avc1.64002A', kind: 'video' },
  { codec: 'avc1.640033', kind: 'video' },
  { codec: 'hvc1.1.6.L153.B0', kind: 'video' },
  { codec: 'mp4a.40.2', kind: 'audio' },
  { codec: 'mp4a.40.5', kind: 'audio' },
  { codec: 'flac', kind: 'audio' },
  { codec: 'opus', kind: 'audio' },
];

/** The codecs this browser can play, as POP takes them: `a,b,c`. Empty when
 *  it can play no video at all. */
export function codecs(canPlay: (mime: string) => boolean): string {
  const ok = CANDIDATES.filter((c) => canPlay(`${c.kind}/mp4; codecs="${c.codec}"`));
  return ok.some((c) => c.kind === 'video') ? ok.map((c) => c.codec).join(',') : '';
}

/** What POP said on the MSE socket. */
export type Said =
  | { kind: 'playing'; mime: string }
  /** The camera sends JPEG pictures: watch it as MJPEG instead. */
  | { kind: 'mjpeg' }
  | { kind: 'no_picture' }
  | { kind: 'unreadable' };

export function said(text: string): Said {
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return { kind: 'unreadable' };
  }
  const o = v as { type?: unknown; value?: unknown };
  if (o?.type === 'mse' && typeof o.value === 'string' && o.value.length > 0) return { kind: 'playing', mime: o.value };
  if (o?.type === 'error' && o.value === 'unsupported') return { kind: 'mjpeg' };
  if (o?.type === 'error') return { kind: 'no_picture' };
  return { kind: 'unreadable' };
}

/** Where a printer's camera is, for this page: the MSE socket and the MJPEG
 *  address. */
export function addresses(printer: string, page: { protocol: string; host: string }) {
  const id = encodeURIComponent(printer);
  const ws = page.protocol === 'https:' ? 'wss:' : 'ws:';
  return {
    mse: `${ws}//${page.host}/api/printers/${id}/camera/mse`,
    mjpeg: `/api/printers/${id}/camera/mjpeg`,
  };
}

/** How far behind live a player may fall before it jumps forward, in seconds. */
export const MAX_BEHIND = 3;

/** Where to jump to, if `buffered_end` is too far ahead of `now`. */
export function catchUp(now: number, bufferedEnd: number): number | null {
  return bufferedEnd - now > MAX_BEHIND ? bufferedEnd - 0.5 : null;
}
