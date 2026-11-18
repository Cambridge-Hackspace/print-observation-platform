// @vitest-environment jsdom
// A printer's camera, full screen at the kiosk (#58).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/svelte';
import type { TowerPrinter } from '../generated/TowerPrinter';
import KioskCamera from './KioskCamera.svelte';

const voron = (camera: boolean): TowerPrinter => ({
  id: 'p1',
  name: 'Voron 2.4',
  state: { status: 'printing', progress: 0.4, layer: null, seconds_left: null, job: null, error: null, held: false },
  watched: true,
  materials: [],
  training: null,
  camera,
});

describe('the kiosk’s camera view', () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it('shows the printer’s camera through POP, without signing in', () => {
    const r = render(KioskCamera, { printer: voron(true) });
    expect(r.getByRole('heading', { name: 'Voron 2.4' })).toBeTruthy();
    // jsdom plays no video, so it is watched as MJPEG; at the space, no
    // sign-in is asked for.
    expect(r.getByAltText('The printer’s camera, live').getAttribute('src')).toBe('/api/printers/p1/camera/mjpeg');
    expect(r.queryByRole('button', { name: 'Sign in' })).toBeNull();
  });

  it('goes back to the tower when asked, and by itself after a while', async () => {
    vi.useFakeTimers();
    const onback = vi.fn();
    const r = render(KioskCamera, { printer: voron(true), onback, returnAfterMs: 1000 });
    await fireEvent.click(r.getByRole('button', { name: /Back/ }));
    expect(onback).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(999);
    expect(onback).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);
    expect(onback).toHaveBeenCalledTimes(2);
  });

  it('says so when the printer no longer has a camera', () => {
    const r = render(KioskCamera, { printer: voron(false) });
    expect(r.getByText(/no camera on the tower/)).toBeTruthy();
    expect(r.queryByAltText('The printer’s camera, live')).toBeNull();
  });
});
