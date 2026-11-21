// A card tap at this kiosk's reader (#19). The kiosk asks POP for its tap,
// and POP answers when one comes, or after a while with none; then it asks
// again. The answer says only what to show: the code or ticket CS:S gave
// never reaches this screen. A screen that is not an enrolled kiosk, or a
// POP with no card sign-in, is told so on its first ask, and stops asking.

import { call } from '../api';
import type { KioskTap } from '../generated/KioskTap';
import type { TapView } from '../generated/TapView';

/** How long a refusal stays up, unless someone touches OK first. */
export const REFUSAL_MS = 12_000;

/** After a failed ask, how long before the next, so a POP restarting is not hammered. */
export const RETRY_MS = 5_000;

/** Watch for taps, telling `ontap` each; the returned function stops it. */
export function watchTaps(ontap: (t: TapView) => void, retry = RETRY_MS): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const ask = async () => {
    if (stopped) return;
    const a = await call<KioskTap>('GET', '/api/kiosk/tap');
    if (stopped) return;
    // Not a kiosk, or no card sign-in here: nothing will ever come.
    if (!a.ok && (a.status === 401 || a.status === 404)) return;
    if (!a.ok) {
      timer = setTimeout(ask, retry);
      return;
    }
    if (a.data.tap) ontap(a.data.tap);
    void ask();
  };
  void ask();
  return () => {
    stopped = true;
    if (timer !== undefined) clearTimeout(timer);
  };
}
