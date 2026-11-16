// The tower's home screen (#49): which printers come first, and the words on
// each card. Pure, so every rule here is tested without a browser.

import type { TowerPrinter } from '../generated/TowerPrinter';

/** What a printer needs from people, most first. */
export type Need = 'needs_hand' | 'printing' | 'done' | 'free' | 'offline';

export const NEEDS: readonly Need[] = ['needs_hand', 'printing', 'done', 'free', 'offline'];

export function need(p: TowerPrinter): Need {
  switch (p.state.status) {
    case 'needs_hand':
      return 'needs_hand';
    case 'printing':
    case 'paused':
      return 'printing';
    case 'done':
      return 'done';
    case 'idle':
      return 'free';
    case 'offline':
      return 'offline';
  }
}

const byName = (a: TowerPrinter, b: TowerPrinter) => a.name.localeCompare(b.name);

/** Needs a hand, then printing (soonest done first), then done, then free, then offline. */
export function byNeed(printers: readonly TowerPrinter[]): TowerPrinter[] {
  const left = (p: TowerPrinter) => p.state.seconds_left ?? Number.POSITIVE_INFINITY;
  return [...printers].sort((a, b) => {
    const g = NEEDS.indexOf(need(a)) - NEEDS.indexOf(need(b));
    if (g !== 0) return g;
    if (need(a) === 'printing' && left(a) !== left(b)) return left(a) - left(b);
    return byName(a, b);
  });
}

/** How many printers are in each state, for the header. */
export function counts(printers: readonly TowerPrinter[]): Record<Need, number> {
  const c: Record<Need, number> = { needs_hand: 0, printing: 0, done: 0, free: 0, offline: 0 };
  for (const p of printers) c[need(p)] += 1;
  return c;
}

const COUNT_WORDS: Record<Need, string> = {
  needs_hand: 'needs a hand',
  printing: 'printing',
  done: 'done',
  free: 'free',
  offline: 'offline',
};

/** "1 needs a hand", "2 printing": each state that has a printer, most needed first. */
export function countLabels(printers: readonly TowerPrinter[]): { need: Need; text: string }[] {
  const c = counts(printers);
  return NEEDS.filter((n) => c[n] > 0).map((n) => ({ need: n, text: `${c[n]} ${COUNT_WORDS[n]}` }));
}

/** "Sam" from "Sam Lee"; null when nobody is known. */
export function firstName(owner: string | null): string | null {
  const first = owner?.trim().split(/\s+/)[0];
  return first ? first : null;
}

/** A print file's name as people say it: "gear_housing" from "gear_housing.gcode". */
export function jobName(file: string): string {
  return file.replace(/\.(gcode|bgcode|gco|g|3mf)$/i, '');
}

/** "38 min", "2 h 41 min", "less than a minute". */
export function duration(seconds: number): string {
  const minutes = Math.round(seconds / 60);
  if (minutes < 1) return 'less than a minute';
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

/** "12:49 PM": the space's clock, 12-hour. */
export function clock(d: Date): string {
  const h = d.getHours();
  const m = String(d.getMinutes()).padStart(2, '0');
  return `${h % 12 === 0 ? 12 : h % 12}:${m} ${h < 12 ? 'AM' : 'PM'}`;
}

/** "37%". */
export const percent = (fraction: number) => `${Math.round(Math.min(1, Math.max(0, fraction)) * 100)}%`;

/** The words on one printer's card. */
export interface Card {
  /** The state, beside the printer's name: "Needs a hand". */
  state: string;
  /** Whose print, and what: "Sam's phone_stand"; or what a free printer takes. */
  job: string;
  /** Under the bar, on the left: "81% · 38 min left". */
  left: string;
  /** And on the right: "done about 12:49 PM". */
  right: string;
}

export function card(p: TowerPrinter, now: Date): Card {
  const s = p.state;
  const whose = (() => {
    if (!s.job) return '';
    const who = firstName(s.job.owner);
    return who ? `${who}’s ${jobName(s.job.name)}` : jobName(s.job.name);
  })();
  const takes = p.materials.length ? `Takes ${list(p.materials)}` : '';
  switch (s.status) {
    case 'needs_hand': {
      const at = s.progress === null ? 'Stopped' : `Stopped at ${percent(s.progress)}`;
      return { state: 'Needs a hand', job: whose, left: s.error ? `${at} · ${s.error.words}` : at, right: '' };
    }
    case 'printing': {
      const parts = [s.progress === null ? '' : percent(s.progress)];
      if (s.seconds_left !== null) parts.push(`${duration(s.seconds_left)} left`);
      const right =
        s.seconds_left === null ? '' : `done about ${clock(new Date(now.getTime() + s.seconds_left * 1000))}`;
      return { state: 'Printing', job: whose, left: parts.filter(Boolean).join(' · '), right };
    }
    case 'paused':
      return {
        state: 'Paused',
        job: whose,
        left: s.progress === null ? 'Paused' : `Paused at ${percent(s.progress)}`,
        right: '',
      };
    case 'done':
      return { state: 'Done', job: whose, left: 'Finished', right: 'still on the bed' };
    case 'idle':
      return p.training
        ? { state: `Free · ${p.training}`, job: takes, left: `Needs ${p.training} training`, right: '' }
        : { state: 'Free', job: takes, left: 'Ready when you are', right: 'tap your card' };
    case 'offline':
      return {
        state: 'Offline',
        job: '',
        left: p.watched ? 'POP cannot reach this printer' : 'POP cannot watch this printer yet',
        right: '',
      };
  }
}

/** "PLA", "PLA and PETG", "PLA, PETG and TPU". */
export function list(items: readonly string[]): string {
  if (items.length <= 1) return items.join('');
  return `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`;
}
