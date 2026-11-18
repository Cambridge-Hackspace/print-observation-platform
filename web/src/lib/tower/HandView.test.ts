// @vitest-environment jsdom
// "A printer needs a hand" (#55), rendered from a given printer, with the
// server's guide answers stood in for.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, waitFor } from '@testing-library/svelte';
import HandView from './HandView.svelte';
import type { PrinterState } from '../generated/PrinterState';
import type { PublicGuide } from '../generated/PublicGuide';
import type { TowerPrinter } from '../generated/TowerPrinter';

const ender = (s: Partial<PrinterState> = {}): TowerPrinter => ({
  id: 'id-ender',
  name: 'Ender 3',
  state: {
    status: 'needs_hand',
    progress: 0.37,
    layer: { current: 112, total: 300 },
    seconds_left: null,
    job: { name: 'phone_stand.gcode', owner: 'Sam Lee' },
    error: { words: 'The filament ran out at layer 112.', raw: 'runout', known: true, key: 'klipper-runout' },
    held: true,
    ...s,
  },
  watched: true,
  materials: [],
  training: null, camera: false,
});

const GUIDE: PublicGuide = {
  slug: 'reload-filament',
  title: 'Reload filament',
  summary: 'When it runs out.',
  start: 'load',
  license: null,
  steps: [
    { id: 'load', title: 'Load a new spool', body: 'From the shelf.', image: null, then: { kind: 'next', step: 'purge' } },
    { id: 'purge', title: 'Push filament through until it runs clean', body: '', image: null, then: { kind: 'done' } },
  ],
};

/** The server: `links` by error key, and the guides by slug. */
function serve(links: Record<string, string>, guides: PublicGuide[] = [GUIDE]) {
  const asked: string[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      asked.push(url);
      if (url === '/api/public/error-guides') return new Response(JSON.stringify(links));
      const g = guides.find((x) => url === `/api/public/guides/${x.slug}`);
      return g ? new Response(JSON.stringify(g)) : new Response('', { status: 404 });
    }),
  );
  return asked;
}

const steps = (el: HTMLElement) => [...el.querySelectorAll('[data-testid=steps] li .title')].map((t) => t.textContent);

beforeEach(() => vi.useRealTimers());
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('a printer that needs a hand', () => {
  it('says the cause and whose print, with the guide’s steps and then Resume at the printer', async () => {
    serve({ 'klipper-runout': 'reload-filament' });
    const { getByTestId, getByRole } = render(HandView, { printer: ender(), told: null, onback: () => {} });
    expect(getByRole('heading').textContent).toContain('Ender 3 needs a hand');
    expect(getByTestId('cause').textContent).toBe('The filament ran out at layer 112.');
    expect(getByTestId('hand').textContent).toContain('Sam’s print is paused, not lost.');
    await waitFor(() =>
      expect(steps(getByTestId('steps'))).toEqual([
        'Load a new spool',
        'Push filament through until it runs clean',
        'Press Resume on the printer’s screen',
      ]),
    );
    const link = getByTestId('guide') as HTMLAnchorElement;
    expect(link.getAttribute('href')).toBe('/kiosk/guides/reload-filament');
    expect(link.textContent?.trim()).toBe('Show me how');
  });

  it('never offers to resume a print that stopped', async () => {
    serve({});
    const { getByTestId } = render(HandView, { printer: ender({ held: false }), told: null, onback: () => {} });
    expect(steps(getByTestId('steps'))).toEqual(['Clear the error on the printer’s screen']);
    expect(getByTestId('hand').textContent).toContain('Sam’s print stopped and can’t be resumed.');
    expect(getByTestId('hand').textContent).not.toContain('Resume');
    expect(getByTestId('hand').textContent).not.toContain('not lost');
  });

  it('says there is no guide when none is linked, and links none', async () => {
    const asked = serve({ 'something-else': 'reload-filament' });
    const { getByTestId, queryByTestId } = render(HandView, { printer: ender(), told: null, onback: () => {} });
    await waitFor(() => expect(asked).toContain('/api/public/error-guides'));
    await new Promise((r) => setTimeout(r, 20));
    expect(queryByTestId('guide')).toBeNull();
    expect(getByTestId('hand').textContent).toContain('No guide for this one yet');
    expect(asked).toEqual(['/api/public/error-guides']);
  });

  it('shows what the printer said when POP does not know it', () => {
    serve({});
    const error = { words: 'The printer reported: E42', raw: 'E42', known: false, key: null };
    const { getByTestId } = render(HandView, { printer: ender({ error }), told: null, onback: () => {} });
    expect(getByTestId('cause').textContent).toBe('The printer reported: E42');
    expect(getByTestId('hand').textContent).toContain('this is what the printer said');
  });

  it('shows what the group chat saw, and says so when it saw nothing', () => {
    serve({});
    const at = new Date(2026, 10, 16, 11, 58).getTime() / 1000;
    const told = { printer: 'id-ender', text: 'Ender 3 needs a hand: the filament ran out at layer 112. Sam’s print is paused.', at };
    const shown = render(HandView, { printer: ender(), told, onback: () => {} });
    expect(shown.getByTestId('told').textContent).toContain(told.text);
    expect(shown.getByTestId('told').textContent).toContain('11:58 AM');
    expect(shown.queryByTestId('not-told')).toBeNull();
    cleanup();
    const none = render(HandView, { printer: ender(), told: null, onback: () => {} });
    expect(none.queryByTestId('told')).toBeNull();
    expect(none.getByTestId('not-told').textContent).toBe('POP hasn’t posted this one to the group chat.');
  });

  it('says when the printer no longer needs a hand, with no steps', () => {
    serve({});
    const { getByRole, queryByTestId } = render(HandView, {
      printer: ender({ status: 'printing', error: null }),
      told: null,
      onback: () => {},
    });
    expect(getByRole('heading').textContent).toContain('Ender 3 is printing again');
    expect(queryByTestId('steps')).toBeNull();
  });

  it('goes back when asked, and when left alone', async () => {
    serve({});
    vi.useFakeTimers();
    const onback = vi.fn();
    const { getByRole } = render(HandView, { printer: ender(), told: null, onback, idleMs: 1000 });
    await fireEvent.click(getByRole('button', { name: 'Back to the printers' }));
    expect(onback).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(900);
    expect(onback).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(200);
    expect(onback).toHaveBeenCalledTimes(2);
  });

  it('is not sent back while someone is using it', async () => {
    serve({});
    vi.useFakeTimers();
    const onback = vi.fn();
    render(HandView, { printer: ender(), told: null, onback, idleMs: 1000 });
    vi.advanceTimersByTime(800);
    window.dispatchEvent(new Event('pointerdown'));
    vi.advanceTimersByTime(800);
    expect(onback).not.toHaveBeenCalled();
    vi.advanceTimersByTime(300);
    expect(onback).toHaveBeenCalledTimes(1);
  });
});
