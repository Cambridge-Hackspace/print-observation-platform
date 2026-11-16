// The printers table's words (#86).
import { describe, expect, it } from 'vitest';
import { connectsBy, fieldsSay, now, offered, training } from './desk';
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
