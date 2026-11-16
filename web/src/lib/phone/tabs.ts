// The phone app's sections (#60), as the address's hash names them, so the
// back button and a link both land on the right one.

export type Tab = 'printers' | 'you';

export const TABS: readonly Tab[] = ['printers', 'you'];

/** The section `hash` names; the printers for anything else. */
export function tabFrom(hash: string): Tab {
  const t = hash.replace(/^#/, '');
  return (TABS as readonly string[]).includes(t) ? (t as Tab) : 'printers';
}
