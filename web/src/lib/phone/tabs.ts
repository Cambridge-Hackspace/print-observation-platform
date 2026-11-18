// The phone app's sections (#60), as the address's hash names them, so the
// back button and a link both land on the right one. Watching a print (#61)
// is `#watch/<printer's BAM! id>`.

export type Tab = 'printers' | 'you';

export const TABS: readonly Tab[] = ['printers', 'you'];

/** The section `hash` names; the printers for anything else. */
export function tabFrom(hash: string): Tab {
  const t = hash.replace(/^#/, '');
  return (TABS as readonly string[]).includes(t) ? (t as Tab) : 'printers';
}

/** The printer `hash` watches, if it watches one. */
export function watching(hash: string): string | null {
  const m = /^#watch\/(.+)$/.exec(hash);
  if (!m) return null;
  try {
    return decodeURIComponent(m[1]!);
  } catch {
    return null;
  }
}

/** The hash that watches `printer`. */
export function watchHash(printer: string): string {
  return `#watch/${encodeURIComponent(printer)}`;
}
