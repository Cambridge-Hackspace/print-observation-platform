// The printers table's words (#86).
import { describe, expect, it } from 'vitest';
import { connectsBy, fieldsSay, midSentence, needsYou, now, offered, training } from './desk';
import type { AdminPrinter } from '../generated/AdminPrinter';
import type { PrinterState } from '../generated/PrinterState';
import type { TowerPrinter } from '../generated/TowerPrinter';

const admin = (more: Partial<AdminPrinter> = {}): AdminPrinter => ({
  bam_id: 'id-voron',
  name: 'Voron 2.4',
  ready: true,
  connection: 'moonraker',
  problems: [],
  tower: 'added',
  categories: ['Printers'],
  training: 'advanced',
  camera: true,
  ...more,
});
const live = (s: Partial<PrinterState>): TowerPrinter => ({
  id: 'id-voron',
  name: 'Voron 2.4',
  state: { status: 'idle', progress: null, layer: null, seconds_left: null, job: null, error: null, held: false, ...s },
  watched: true,
  materials: [],
  training: 'advanced',
});

describe('the printers table', () => {
  it('names each way of connecting as people say it, and keeps one POP does not know', () => {
    expect(
      (['moonraker', 'bambu', 'octoprint', 'prusalink', 'usb_serial', 'telepathy'] as const).map((c) =>
        connectsBy(admin({ connection: c })),
      ),
    ).toEqual(['Klipper', 'Bambu', 'OctoPrint', 'PrusaLink', 'USB', 'telepathy']);
    expect(connectsBy(admin({ connection: null }))).toBe('—');
  });

  it('capitalizes the training, and says when there is none', () => {
    expect(training(admin())).toBe('Advanced');
    expect(training(admin({ training: null }))).toBe('None');
    expect(training(admin({ training: '  ' }))).toBe('None');
  });

  it('says what a printer on the tower is doing now, with whose print', () => {
    expect(now(admin(), live({ status: 'printing', job: { name: 'gear.gcode', owner: 'Alex Kim' } }))).toEqual({
      text: 'Printing · Alex',
      tone: 'printing',
    });
    expect(now(admin(), live({ status: 'needs_hand' }))).toEqual({ text: 'Needs a hand', tone: 'needs_hand' });
    expect(now(admin(), live({ status: 'idle' }))).toEqual({ text: 'Free · advanced', tone: 'free' });
    expect(now(admin(), live({ status: 'offline' })).tone).toBe('offline');
  });

  it('says where a printer the tower does not run stands', () => {
    expect(now(admin({ tower: 'offered' }), undefined)).toEqual({ text: 'New: not on the tower yet', tone: 'waiting' });
    expect(now(admin({ tower: 'offered', ready: false }), undefined).text).toBe('New: its BAM! fields need fixing');
    expect(now(admin({ tower: 'declined' }), undefined)).toEqual({ text: 'Not for the tower', tone: 'off' });
    expect(now(admin({ tower: 'added' }), undefined)).toEqual({
      text: 'Added: its BAM! fields need fixing',
      tone: 'waiting',
    });
  });

  it('ignores the live state of a printer the tower does not run', () => {
    expect(now(admin({ tower: 'declined' }), live({ status: 'printing' })).text).toBe('Not for the tower');
  });

  it('offers only what waits for an admin', () => {
    const all = [admin(), admin({ bam_id: 'n', tower: 'offered' }), admin({ bam_id: 'd', tower: 'declined' })];
    expect(offered(all).map((p) => p.bam_id)).toEqual(['n']);
  });

  it('says what its BAM! fields say', () => {
    expect(fieldsSay(admin())).toBe('Its fields say Klipper, advanced training, camera fitted.');
    expect(fieldsSay(admin({ connection: null, training: null, camera: false }))).toBe(
      'Its fields say no connection, no training needed, no camera.',
    );
  });
});

describe('what needs you', () => {
  const stuck = (id: string, name: string, s: Partial<PrinterState> = {}): TowerPrinter => ({
    ...live({ status: 'needs_hand', ...s }),
    id,
    name,
  });
  const at = new Date(2026, 10, 16, 11, 58).getTime() / 1000;

  it('lists a printer that needs a hand, what stopped it, and whether the group chat was told', () => {
    const ender = stuck('e', 'Ender 3', {
      error: { words: 'The filament ran out at layer 112.', raw: 'runout', known: true, key: null },
      job: { name: 'stand.gcode', owner: 'Sam Lee' },
    });
    const told = [{ printer: 'e', text: 'Ender 3 needs a hand.', at }];
    expect(needsYou([], [ender], told)).toEqual([
      {
        key: 'e:needs_hand',
        kind: 'needs_hand',
        title: 'Ender 3: the filament ran out at layer 112',
        detail: 'Posted to the group chat at 11:58 AM · Sam’s print',
      },
    ]);
    expect(needsYou([], [stuck('x', 'X1')], [])[0]).toMatchObject({
      title: 'X1 needs a hand',
      detail: 'Not posted to the group chat',
    });
  });

  it('puts the cause mid-sentence, keeping an acronym’s capitals', () => {
    expect(midSentence('The filament ran out.')).toBe('the filament ran out');
    expect(midSentence('PLA jammed.')).toBe('PLA jammed');
    expect(midSentence('The printer reported: E42')).toBe('the printer reported: E42');
  });

  it('lists printers that need a hand by name', () => {
    expect(needsYou([], [stuck('z', 'Voron'), stuck('a', 'Ender 3')], []).map((i) => i.key)).toEqual([
      'a:needs_hand',
      'z:needs_hand',
    ]);
  });

  it('does not take another printer’s notice for this one', () => {
    const told = [{ printer: 'other', text: 'Other needs a hand.', at }];
    expect(needsYou([], [stuck('e', 'Ender 3')], told)[0]!.detail).toBe('Not posted to the group chat');
  });

  it('leaves out printers that are fine', () => {
    expect(needsYou([admin()], [live({ status: 'printing' }), live({ status: 'offline' })], [])).toEqual([]);
  });

  it('lists a printer added to the tower that cannot run, with what to fix, after the ones that need a hand', () => {
    const broken = admin({
      bam_id: 'b',
      name: 'MK4',
      ready: false,
      problems: [{ field: 'Connection (connection)', problem: 'missing' }],
    });
    const items = needsYou(
      [broken, admin({ bam_id: 'n', ready: false, tower: 'offered' }), admin({ bam_id: 'd', ready: false, tower: 'declined' })],
      [stuck('e', 'Ender 3')],
      [],
    );
    expect(items.map((i) => i.key)).toEqual(['e:needs_hand', 'b:fields']);
    expect(items[1]).toMatchObject({ title: 'MK4 can’t run: fix it in BAM!', detail: 'Connection (connection): missing' });
  });
});
