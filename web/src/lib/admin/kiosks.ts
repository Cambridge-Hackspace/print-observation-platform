// The desk's kiosk list (#12), pure so it is tested without a browser.

/** "12 seconds ago", "4 minutes ago", "3 hours ago", "2 days ago". */
export function ago(unixSecs: number, nowMs: number): string {
  const s = Math.max(0, Math.round(nowMs / 1000 - unixSecs));
  const say = (n: number, unit: string) => `${n} ${unit}${n === 1 ? '' : 's'} ago`;
  if (s < 60) return say(s, 'second');
  if (s < 3600) return say(Math.floor(s / 60), 'minute');
  if (s < 86400) return say(Math.floor(s / 3600), 'hour');
  return say(Math.floor(s / 86400), 'day');
}
