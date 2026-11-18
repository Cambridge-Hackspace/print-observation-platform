// @vitest-environment jsdom
// Watching a print (#61): progress, time left and finish time, the camera
// for members, and deliberately no way to pause or stop it.
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import type { TowerPrinter } from '../generated/TowerPrinter';
import Watch from './Watch.svelte';

const NOW = new Date('2026-11-18T14:00:00');

function printing(camera: boolean): TowerPrinter {
  return {
    id: 'p1',
    name: 'Voron 2.4',
    state: {
      status: 'printing',
      progress: 0.45,
      layer: { current: 40, total: 120 },
      seconds_left: 30 * 60,
      job: { name: 'benchy.gcode', owner: 'Sam Lee' },
      error: null,
      held: false,
    },
    watched: true,
    materials: [],
    training: null,
    camera,
  };
}

describe('watching a print', () => {
  afterEach(cleanup);

  it('shows its progress, time left and when it finishes', () => {
    const r = render(Watch, { printer: printing(false), now: NOW, signedIn: false });
    expect(r.getByRole('heading', { name: 'Voron 2.4' })).toBeTruthy();
    expect(r.getByTestId('watch-left').textContent).toContain('30 min left');
    expect(r.getByTestId('watch-right').textContent).toBe('done about 2:30 PM');
    expect(r.getByRole('progressbar')).toBeTruthy();
    // Whose print and what, marked so a feedback screenshot never shows it.
    const job = r.getByText(/benchy/);
    expect(job.getAttribute('data-sensitive')).toBe('file');
  });

  it('has no way to pause, stop or change the print: watching is remote', () => {
    for (const signedIn of [false, true]) {
      const r = render(Watch, { printer: printing(true), now: NOW, signedIn });
      const controls = r
        .queryAllByRole('button')
        .map((b) => b.textContent?.trim().toLowerCase() ?? '')
        .filter((t) => /pause|stop|resume|cancel|abort|start/.test(t));
      expect(controls).toEqual([]);
      cleanup();
    }
  });

  it('shows the camera to members, through POP, marked as a camera', () => {
    const r = render(Watch, { printer: printing(true), now: NOW, signedIn: true });
    // jsdom plays no video, so the camera is watched as MJPEG.
    const img = r.getByAltText('The printer’s camera, live');
    expect(img.getAttribute('src')).toBe('/api/printers/p1/camera/mjpeg');
    expect(img.getAttribute('data-sensitive')).toBe('camera');
  });

  it('asks someone not signed in to sign in for the camera', () => {
    const r = render(Watch, { printer: printing(true), now: NOW, signedIn: false });
    expect(r.getByTestId('camera').getAttribute('data-showing')).toBe('sign_in');
    expect(r.queryByAltText('The printer’s camera, live')).toBeNull();
    expect(r.getByRole('button', { name: 'Sign in' })).toBeTruthy();
  });

  it('shows no camera for a printer that has none', () => {
    const r = render(Watch, { printer: printing(false), now: NOW, signedIn: true });
    expect(r.queryByTestId('camera')).toBeNull();
  });

  it('says so when the tower has no such printer', () => {
    const r = render(Watch, { printer: null, now: NOW, signedIn: true });
    expect(r.getByRole('heading', { name: 'Not on the tower' })).toBeTruthy();
  });
});
