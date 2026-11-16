// Managing local users (#90): the words and choices on the People section.
// Pure, so every rule here is tested without a browser.

import type { Account } from '../generated/Account';
import type { AdminPrinter } from '../generated/AdminPrinter';

/**
 * The training an admin can tick: every training a printer in BAM! needs,
 * and whatever the accounts already hold, each once whatever its case, sorted.
 */
export function trainingChoices(printers: readonly AdminPrinter[], accounts: readonly Account[]): string[] {
  const out: string[] = [];
  const add = (t: string | null | undefined) => {
    const v = t?.trim();
    if (v && !out.some((o) => o.toLowerCase() === v.toLowerCase())) out.push(v);
  };
  printers.forEach((p) => add(p.training));
  accounts.forEach((a) => a.training.forEach(add));
  return out.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
}

/** Whether `held` includes `t`, whatever its case. */
export const holds = (held: readonly string[], t: string) => held.some((h) => h.toLowerCase() === t.toLowerCase());

/** `held` with `t` ticked on or off. */
export function tick(held: readonly string[], t: string, on: boolean): string[] {
  const rest = held.filter((h) => h.toLowerCase() !== t.toLowerCase());
  return on ? [...rest, t] : rest;
}

/** "Admin · advanced, basic", or "Member · no training". */
export function summary(a: Account): string {
  const role = a.role === 'admin' ? 'Admin' : 'Member';
  return `${role} · ${a.training.length ? a.training.join(', ') : 'no training'}`;
}
