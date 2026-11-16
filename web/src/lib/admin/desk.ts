// The admin desk's printers table (#86): every printer in BAM!'s category, how
// it connects, its training and camera, and what it is doing now. Pure, so
// every rule here is tested without a browser.

import type { AdminPrinter } from '../generated/AdminPrinter';
import type { TowerPrinter } from '../generated/TowerPrinter';
import { card, firstName, need, type Need } from '../tower/need';

/** How a printer connects, as people say it. */
const CONNECTS: Record<string, string> = {
  moonraker: 'Klipper',
  bambu: 'Bambu',
  octoprint: 'OctoPrint',
  prusalink: 'PrusaLink',
  usb_serial: 'USB',
};

/** "Klipper"; what BAM! said, when POP does not know it; a dash for nothing. */
export function connectsBy(p: AdminPrinter): string {
  if (p.connection === null) return '—';
  return CONNECTS[p.connection] ?? p.connection;
}

/** "Advanced" from "advanced"; "None" when it needs none. */
export function training(p: AdminPrinter): string {
  const t = p.training?.trim();
  return t ? t[0]!.toUpperCase() + t.slice(1) : 'None';
}

/** What a row's "Now" says, and the state it is drawn in. */
export interface Now {
  text: string;
  /** A tower state, or where it stands when the tower does not run it. */
  tone: Need | 'waiting' | 'off';
}

/**
 * "Printing · Alex", "Needs a hand", "Free", from the tower's live state when
 * the tower runs it; otherwise where it stands with the tower.
 */
export function now(p: AdminPrinter, live: TowerPrinter | undefined): Now {
  if (p.tower === 'added' && live) {
    const who = firstName(live.state.job?.owner ?? null);
    const state = card(live, new Date(0)).state;
    return { text: who && live.state.job ? `${state} · ${who}` : state, tone: need(live) };
  }
  switch (p.tower) {
    case 'offered':
      return { text: p.ready ? 'New: not on the tower yet' : 'New: its BAM! fields need fixing', tone: 'waiting' };
    case 'added':
      // Added, but the tower cannot run it until BAM! has it right.
      return { text: 'Added: its BAM! fields need fixing', tone: 'waiting' };
    case 'declined':
      return { text: 'Not for the tower', tone: 'off' };
    case 'retired':
      return { text: 'Archived in BAM!', tone: 'off' };
  }
}

/** The printers waiting for an admin to say, for the "New in BAM!" banners. */
export const offered = (all: readonly AdminPrinter[]) => all.filter((p) => p.tower === 'offered');

/** "Its fields say Klipper, advanced training, camera fitted." */
export function fieldsSay(p: AdminPrinter): string {
  const parts = [
    connectsBy(p) === '—' ? 'no connection' : connectsBy(p),
    p.training?.trim() ? `${p.training.trim()} training` : 'no training needed',
    p.camera ? 'camera fitted' : 'no camera',
  ];
  return `Its fields say ${parts.join(', ')}.`;
}
