// The kiosk's side of being the kiosk (#12): every half minute it checks in
// with POP using its enrolled credential (an HttpOnly cookie this code cannot
// read, and does not need to). POP believes where the check-in came from, so
// a network is the space's while this screen is on it. A screen that is not
// an enrolled kiosk is told so on its first check-in, and stops asking.

import { call } from '../api';

/** How often the kiosk checks in: a quarter of POP's two minutes. */
export const EVERY_MS = 30_000;

/** Start checking in; the returned function stops it. */
export function startCheckins(every = EVERY_MS): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const once = async () => {
    const a = await call('POST', '/api/kiosk/checkin');
    // Not an enrolled kiosk (or revoked): nothing to vouch for, so stop.
    if (stopped || (!a.ok && a.status === 401)) return;
    timer = setTimeout(once, every);
  };
  void once();
  return () => {
    stopped = true;
    if (timer !== undefined) clearTimeout(timer);
  };
}
