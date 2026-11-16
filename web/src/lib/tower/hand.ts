// The "a printer needs a hand" screen (#55): what went wrong in plain words,
// numbered steps to fix it, and what the group chat was told. Pure, so every
// rule here is tested without a browser.
//
// POP never resumes or clears a printer: control stays at the printer, and
// the last step always says so.

import type { GroupNotice } from '../generated/GroupNotice';
import type { TowerPrinter } from '../generated/TowerPrinter';
import type { Walkable } from '../guides/walk';
import { firstName } from './need';

/** One numbered step. */
export interface FixStep {
  title: string;
  /** More words, or empty. */
  body: string;
}

/** The most steps shown before "Show me how" takes over. */
export const MOST_STEPS = 5;

/** The step that always ends the list: what only the printer can do. */
export function lastStep(held: boolean): FixStep {
  return held
    ? { title: 'Press Resume on the printer’s screen', body: 'POP never resumes a printer. Control stays at the printer.' }
    : {
        title: 'Clear the error on the printer’s screen',
        body: 'This print can’t be resumed. POP never controls a printer: that stays at the printer.',
      };
}

/**
 * The steps to fix it: the guide's, from its start for as long as one leads
 * straight to the next, then the printer's own last step. `more` when the
 * guide goes on past what is shown, to a question or beyond the most shown.
 */
export function fixSteps(guide: Walkable | null, held: boolean): { steps: FixStep[]; more: boolean } {
  const steps: FixStep[] = [];
  let more = false;
  if (guide) {
    const seen = new Set<string>();
    let id: string | null = guide.start;
    while (id !== null) {
      const s = guide.steps.find((x) => x.id === id);
      if (!s || seen.has(id)) break;
      if (s.then.kind === 'question') {
        more = true;
        break;
      }
      if (steps.length === MOST_STEPS) {
        more = true;
        break;
      }
      seen.add(id);
      steps.push({ title: s.title, body: s.body });
      id = s.then.kind === 'next' ? s.then.step : null;
    }
  }
  return { steps: [...steps, lastStep(held)], more };
}

/** What went wrong, as the headline says it. */
export function cause(p: TowerPrinter): string {
  return p.state.error?.words ?? `${p.name} stopped and needs someone to look at it.`;
}

/** Whose print, and whether it is safe: "Sam’s print is paused, not lost." */
export function whose(p: TowerPrinter): string {
  const who = firstName(p.state.job?.owner ?? null);
  const print = who ? `${who}’s print` : 'The print';
  if (!p.state.job) return '';
  return p.state.held ? `${print} is paused, not lost.` : `${print} stopped and can’t be resumed.`;
}

/** What the group chat was told about this printer, if anything. */
export const toldAbout = (told: readonly GroupNotice[], id: string): GroupNotice | null =>
  told.find((n) => n.printer === id) ?? null;
