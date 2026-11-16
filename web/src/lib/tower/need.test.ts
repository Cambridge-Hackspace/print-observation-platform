import { describe, expect, it } from 'vitest';
import type { TowerPrinter } from '../generated/TowerPrinter';
import type { PrinterState } from '../generated/PrinterState';
import { byNeed, card, clock, countLabels, duration, firstName, jobName, list } from './need';

const state = (s: Partial<PrinterState> & Pick<PrinterState, 'status'>): PrinterState => ({
  progress: null,
  layer: null,
  seconds_left: null,
  job: null,
  error: null,
  ...s,
});
const printer = (name: string, s: PrinterState, more: Partial<TowerPrinter> = {}): TowerPrinter => ({
  id: name,
  name,
  state: s,
  watched: true,
  materials: [],
  training: null,
  ...more,
});

// 12:11 PM, local time, as the round 6 canvas shows it.
const NOW = new Date(2026, 10, 16, 12, 11, 0);

describe('byNeed', () => {
  it('puts a printer that needs a hand first, then printing soonest done, done, free and offline', () => {
    const tower = [
      printer('Voron 2.4', state({ status: 'idle' })),
      printer('Offline One', state({ status: 'offline' })),
      printer('Mini+', state({ status: 'done' })),
      printer('X1 Carbon', state({ status: 'printing', seconds_left: 9660 })),
      printer('Mk4 Left', state({ status: 'printing', seconds_left: 2280 })),
      printer('Ender 3', state({ status: 'needs_hand' })),
      printer('Mk4 Right', state({ status: 'idle' })),
      printer('Prusa Paused', state({ status: 'paused', seconds_left: 5000 })),
    ];
    expect(byNeed(tower).map((p) => p.name)).toEqual([
      'Ender 3',
      'Mk4 Left',
      'Prusa Paused',
      'X1 Carbon',
      'Mini+',
      'Mk4 Right',
      'Voron 2.4',
      'Offline One',
    ]);
  });

  it('puts a print with no estimate after those with one, and ties by name', () => {
    const tower = [
      printer('B', state({ status: 'printing' })),
      printer('A', state({ status: 'printing' })),
      printer('C', state({ status: 'printing', seconds_left: 60 })),
    ];
    expect(byNeed(tower).map((p) => p.name)).toEqual(['C', 'A', 'B']);
  });

  it('does not reorder what it is given', () => {
    const tower = [printer('B', state({ status: 'idle' })), printer('A', state({ status: 'needs_hand' }))];
    byNeed(tower);
    expect(tower.map((p) => p.name)).toEqual(['B', 'A']);
  });
});

describe('countLabels', () => {
  it('counts each state that has a printer, most needed first', () => {
    const tower = [
      printer('a', state({ status: 'needs_hand' })),
      printer('b', state({ status: 'printing' })),
      printer('c', state({ status: 'paused' })),
      printer('d', state({ status: 'done' })),
      printer('e', state({ status: 'idle' })),
      printer('f', state({ status: 'idle' })),
    ];
    expect(countLabels(tower).map((c) => c.text)).toEqual(['1 needs a hand', '2 printing', '1 done', '2 free']);
    expect(countLabels([])).toEqual([]);
  });
});

describe('the words', () => {
  it('says a print, as the canvas does', () => {
    const p = printer(
      'Mk4 Left',
      state({
        status: 'printing',
        progress: 0.81,
        seconds_left: 38 * 60,
        job: { name: 'gear_housing.gcode', owner: 'Dev Patel' },
      }),
    );
    expect(card(p, NOW)).toEqual({
      state: 'Printing',
      job: 'Dev’s gear_housing',
      left: '81% · 38 min left',
      right: 'done about 12:49 PM',
    });
  });

  it('says what stopped a printer that needs a hand', () => {
    const p = printer(
      'Ender 3',
      state({
        status: 'needs_hand',
        progress: 0.37,
        job: { name: 'phone_stand.3mf', owner: 'Sam' },
        error: { words: 'The filament ran out', raw: 'runout', known: true, key: null },
      }),
    );
    expect(card(p, NOW)).toMatchObject({
      state: 'Needs a hand',
      job: 'Sam’s phone_stand',
      left: 'Stopped at 37% · The filament ran out',
    });
  });

  it('says a free printer takes what it takes, and what training it needs', () => {
    const free = printer('Mk4 Right', state({ status: 'idle' }), { materials: ['PLA', 'PETG'] });
    expect(card(free, NOW)).toEqual({
      state: 'Free',
      job: 'Takes PLA and PETG',
      left: 'Ready when you are',
      right: 'tap your card',
    });
    const advanced = printer('Voron 2.4', state({ status: 'idle' }), { training: 'advanced' });
    expect(card(advanced, NOW)).toMatchObject({ state: 'Free · advanced', left: 'Needs advanced training' });
  });

  it('says whether POP cannot reach an offline printer, or cannot watch it at all', () => {
    expect(card(printer('A', state({ status: 'offline' })), NOW).left).toBe('POP cannot reach this printer');
    expect(card(printer('B', state({ status: 'offline' }), { watched: false }), NOW).left).toBe(
      'POP cannot watch this printer yet',
    );
  });

  it('says a print started at the printer itself without an owner', () => {
    const p = printer('X', state({ status: 'done', job: { name: 'benchy.gcode', owner: null } }));
    expect(card(p, NOW)).toEqual({ state: 'Done', job: 'benchy', left: 'Finished', right: 'still on the bed' });
  });

  it('gives times on a 12-hour clock', () => {
    expect(clock(new Date(2026, 0, 1, 0, 5))).toBe('12:05 AM');
    expect(clock(new Date(2026, 0, 1, 11, 59))).toBe('11:59 AM');
    expect(clock(new Date(2026, 0, 1, 12, 0))).toBe('12:00 PM');
    expect(clock(new Date(2026, 0, 1, 14, 52))).toBe('2:52 PM');
  });

  it('gives durations in hours and minutes', () => {
    expect(duration(20)).toBe('less than a minute');
    expect(duration(38 * 60)).toBe('38 min');
    expect(duration(2 * 3600)).toBe('2 h');
    expect(duration(2 * 3600 + 41 * 60)).toBe('2 h 41 min');
  });

  it('names people and files as people say them', () => {
    expect(firstName('  Sam   Lee ')).toBe('Sam');
    expect(firstName('')).toBeNull();
    expect(firstName(null)).toBeNull();
    expect(jobName('keycap_set.bgcode')).toBe('keycap_set');
    expect(jobName('notes.txt')).toBe('notes.txt');
    expect(list(['PLA'])).toBe('PLA');
    expect(list(['PLA', 'PETG', 'TPU'])).toBe('PLA, PETG and TPU');
  });
});
