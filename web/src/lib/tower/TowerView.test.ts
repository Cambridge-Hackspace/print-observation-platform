// @vitest-environment jsdom
// The tower's home screen (#49), rendered from a given state.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, within } from '@testing-library/svelte';
import TowerView from './TowerView.svelte';
import type { ScreenState } from '../screen';
import type { TowerPrinter } from '../generated/TowerPrinter';
import type { PrinterState } from '../generated/PrinterState';

afterEach(cleanup);

const state = (s: Partial<PrinterState> & Pick<PrinterState, 'status'>): PrinterState => ({
  progress: null,
  layer: null,
  seconds_left: null,
  job: null,
  error: null,
  held: false,
  ...s,
});
const printer = (name: string, s: PrinterState, more: Partial<TowerPrinter> = {}): TowerPrinter => ({
  id: `id-${name}`,
  name,
  state: s,
  watched: true,
  materials: [],
  training: null,
  ...more,
});
const screen = (printers: TowerPrinter[], public_url: string | null = null): ScreenState => ({
  status: { version: '0.1.0', schema_version: 6, public_url },
  printers,
  told: [],
});
const NOW = new Date(2026, 10, 16, 12, 11);

const TOWER = [
  printer('Voron 2.4', state({ status: 'idle' }), { training: 'advanced' }),
  printer('Mk4 Left', state({ status: 'printing', progress: 0.81, seconds_left: 2280, job: { name: 'gear_housing.gcode', owner: 'Dev Patel' } })),
  printer('Ender 3', state({ status: 'needs_hand', progress: 0.37, error: { words: 'The filament ran out', raw: 'runout', known: true, key: null } })),
  printer('Mini+', state({ status: 'done', job: { name: 'keycap_set.gcode', owner: 'Jo' } })),
];

describe('the tower', () => {
  it('shows one card per printer, most needed first, as the server sent them', () => {
    const { getAllByTestId } = render(TowerView, { screen: screen(TOWER), connection: 'live', now: NOW });
    const cards = getAllByTestId('printer');
    expect(cards.map((c) => within(c).getByRole('heading').textContent)).toEqual([
      'Ender 3',
      'Mk4 Left',
      'Mini+',
      'Voron 2.4',
    ]);
    expect(cards.map((c) => c.dataset.need)).toEqual(['needs_hand', 'printing', 'done', 'free']);
  });

  it('says each card in words, with the owner by first name', () => {
    const { getAllByTestId } = render(TowerView, { screen: screen(TOWER), connection: 'live', now: NOW });
    const mk4 = getAllByTestId('printer')[1]!;
    expect(mk4.textContent).toContain('Dev’s gear_housing');
    expect(mk4.textContent).toContain('81% · 38 min left');
    expect(mk4.textContent).toContain('done about 12:49 PM');
    expect(within(mk4).getByRole('progressbar').getAttribute('aria-valuenow')).toBe('81');
    const ender = getAllByTestId('printer')[0]!;
    expect(ender.textContent).toContain('Stopped at 37% · The filament ran out');
  });

  it('opens a printer that needs a hand when it is tapped, and only that one', async () => {
    const onopen = vi.fn();
    const { getAllByTestId } = render(TowerView, { screen: screen(TOWER), connection: 'live', now: NOW, onopen });
    const cards = getAllByTestId('printer');
    expect(cards.map((c) => within(c).queryAllByRole('button').length)).toEqual([1, 0, 0, 0]);
    for (const c of cards.slice(1)) await fireEvent.click(c);
    expect(onopen).not.toHaveBeenCalled();
    await fireEvent.click(within(cards[0]!).getByRole('button', { name: 'See what Ender 3 needs' }));
    expect(onopen).toHaveBeenCalledWith('id-Ender 3');
  });

  it('counts each state in the header', () => {
    const { getByRole } = render(TowerView, { screen: screen(TOWER), connection: 'live', now: NOW });
    const tally = getByRole('list', { name: 'Printers by state' });
    expect([...tally.querySelectorAll('li')].map((li) => li.textContent?.trim())).toEqual([
      '1 needs a hand',
      '1 printing',
      '1 done',
      '1 free',
    ]);
  });

  it('keeps the tap prompt, and shows the phone QR code only when POP knows its address', () => {
    const without = render(TowerView, { screen: screen(TOWER), connection: 'live', now: NOW });
    expect(without.getByText('Tap your card to start printing')).toBeTruthy();
    expect(without.queryByRole('img', { name: /QR code/ })).toBeNull();
    cleanup();
    const withUrl = render(TowerView, {
      screen: screen(TOWER, 'https://pop.example.org'),
      connection: 'live',
      now: NOW,
    });
    expect(withUrl.getByRole('img', { name: 'A QR code for https://pop.example.org' })).toBeTruthy();
  });

  it('says when what it shows may be out of date', () => {
    const live = render(TowerView, { screen: screen(TOWER), connection: 'live', now: NOW });
    expect(live.queryByRole('status')).toBeNull();
    cleanup();
    const away = render(TowerView, { screen: screen(TOWER), connection: 'reconnecting', now: NOW });
    expect(away.getByRole('status').textContent).toContain('may be out of date');
  });

  it('says how to get printers when it has none', () => {
    const { getByText } = render(TowerView, { screen: screen([]), connection: 'live', now: NOW });
    expect(getByText(/an admin adds them from BAM!/)).toBeTruthy();
  });
});
