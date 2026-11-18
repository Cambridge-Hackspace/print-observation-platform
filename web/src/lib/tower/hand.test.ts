// The "needs a hand" screen's words and steps (#55).
import { describe, expect, it } from 'vitest';
import { cause, fixSteps, lastStep, MOST_STEPS, toldAbout, whose } from './hand';
import type { GuideStep } from '../generated/GuideStep';
import type { PrinterState } from '../generated/PrinterState';
import type { TowerPrinter } from '../generated/TowerPrinter';

const state = (s: Partial<PrinterState>): PrinterState => ({
  status: 'needs_hand',
  progress: null,
  layer: null,
  seconds_left: null,
  job: null,
  error: null,
  held: false,
  ...s,
});
const printer = (s: Partial<PrinterState>): TowerPrinter => ({
  id: 'id-ender',
  name: 'Ender 3',
  state: state(s),
  watched: true,
  materials: [],
  training: null, camera: false,
});
const step = (id: string, then: GuideStep['then']): GuideStep => ({ id, title: `Do ${id}`, body: `About ${id}`, image: null, then });
const titles = (r: { steps: { title: string }[] }) => r.steps.map((s) => s.title);
const RESUME = 'Press Resume on the printer’s screen';
const CLEAR = 'Clear the error on the printer’s screen';

describe('the steps', () => {
  it('end at the printer: resume a held print, clear one that stopped', () => {
    expect(lastStep(true).title).toBe(RESUME);
    expect(lastStep(true).body).toContain('POP never resumes a printer');
    expect(lastStep(false).title).toBe(CLEAR);
    expect(lastStep(false).body).toContain('can’t be resumed');
  });

  it('are only the printer’s own with no guide', () => {
    expect(fixSteps(null, true)).toEqual({ steps: [lastStep(true)], more: false });
  });

  it('follow a straight guide to its end, then the printer’s step', () => {
    const g = { start: 'a', steps: [step('b', { kind: 'done' }), step('a', { kind: 'next', step: 'b' })] };
    const r = fixSteps(g, true);
    expect(titles(r)).toEqual(['Do a', 'Do b', RESUME]);
    expect(r.steps[0]!.body).toBe('About a');
    expect(r.more).toBe(false);
  });

  it('stop before a question, and say the guide goes on', () => {
    const g = {
      start: 'a',
      steps: [step('a', { kind: 'next', step: 'q' }), step('q', { kind: 'question', yes: 'a', no: 'a' })],
    };
    const r = fixSteps(g, false);
    expect(titles(r)).toEqual(['Do a', CLEAR]);
    expect(r.more).toBe(true);
  });

  it('show at most a handful, and say the guide goes on', () => {
    const ids = Array.from({ length: MOST_STEPS + 2 }, (_, i) => `s${i}`);
    const g = {
      start: 's0',
      steps: ids.map((id, i) => step(id, i + 1 < ids.length ? { kind: 'next', step: ids[i + 1]! } : { kind: 'done' })),
    };
    const r = fixSteps(g, true);
    expect(r.steps).toHaveLength(MOST_STEPS + 1);
    expect(r.more).toBe(true);
    const exact = { start: 's0', steps: g.steps.slice(0, MOST_STEPS).map((s, i) => (i === MOST_STEPS - 1 ? { ...s, then: { kind: 'done' as const } } : s)) };
    expect(fixSteps(exact, true).more).toBe(false);
  });

  it('stop at a loop rather than going round it', () => {
    const g = { start: 'a', steps: [step('a', { kind: 'next', step: 'b' }), step('b', { kind: 'next', step: 'a' })] };
    expect(titles(fixSteps(g, true))).toEqual(['Do a', 'Do b', RESUME]);
  });
});

describe('the words', () => {
  it('give the catalog’s cause, or say it stopped', () => {
    const words = 'The filament ran out at layer 112.';
    expect(cause(printer({ error: { words, raw: 'runout', known: true, key: 'k' } }))).toBe(words);
    expect(cause(printer({}))).toBe('Ender 3 stopped and needs someone to look at it.');
  });

  it('say a held print is safe, and never that one that stopped is', () => {
    const job = { name: 'phone_stand.gcode', owner: 'Sam Lee' };
    expect(whose(printer({ job, held: true }))).toBe('Sam’s print is paused, not lost.');
    expect(whose(printer({ job, held: false }))).toBe('Sam’s print stopped and can’t be resumed.');
    expect(whose(printer({ job: { name: 'x', owner: null }, held: true }))).toBe('The print is paused, not lost.');
    expect(whose(printer({ held: false }))).toBe('');
  });

  it('find what the group was told about this printer', () => {
    const told = [
      { printer: 'other', text: 'Other needs a hand.', at: 1 },
      { printer: 'id-ender', text: 'Ender 3 needs a hand.', at: 2 },
    ];
    expect(toldAbout(told, 'id-ender')?.text).toBe('Ender 3 needs a hand.');
    expect(toldAbout(told, 'nobody')).toBeNull();
  });
});
