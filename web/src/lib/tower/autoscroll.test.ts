import { describe, expect, it } from 'vitest';
import { AutoScroll, type Options } from './autoscroll';

const O: Options = { idleMs: 1000, speed: 100, restMs: 500, reducedMotion: false };

/** A list 1000 px tall in a 400 px window. */
const list = (scrollHeight = 1000) => ({ scrollTop: 0, scrollHeight, clientHeight: 400 });

/** Tick every 100 ms from `from` to `to`. */
function run(a: AutoScroll, from: number, to: number) {
  for (let t = from + 100; t <= to; t += 100) a.tick(t, 100);
}

describe('AutoScroll', () => {
  it('waits until nobody has touched it for a while, then scrolls', () => {
    const el = list();
    const a = new AutoScroll(el, 0, O);
    run(a, 0, 900);
    expect(el.scrollTop).toBe(0);
    run(a, 900, 2000);
    // Ticks at 1.0 s to 2.0 s, 11 of them, 10 px each.
    expect(el.scrollTop).toBeCloseTo(110, 5);
  });

  it('stops when touched, and waits to be idle again', () => {
    const el = list();
    const a = new AutoScroll(el, 0, O);
    run(a, 0, 2000);
    const at = el.scrollTop;
    a.touch(2000);
    run(a, 2000, 2900);
    expect(el.scrollTop).toBe(at);
    run(a, 2900, 3500);
    expect(el.scrollTop).toBeGreaterThan(at);
  });

  it('rests at the bottom, goes back to the top, rests, and carries on', () => {
    const el = list();
    const a = new AutoScroll(el, 0, O);
    run(a, 0, 1000 + 6000); // 600 px to the bottom takes 6 s
    expect(el.scrollTop).toBe(600);
    run(a, 7000, 7400);
    expect(el.scrollTop).toBe(600); // resting at the bottom
    run(a, 7400, 7600);
    expect(el.scrollTop).toBe(0); // back to the top
    run(a, 7600, 7900);
    expect(el.scrollTop).toBe(0); // resting at the top, until 8.0 s
    run(a, 7900, 9000);
    expect(el.scrollTop).toBeGreaterThan(0);
  });

  it('does nothing when every printer fits', () => {
    const el = list(400);
    const a = new AutoScroll(el, 0, O);
    run(a, 0, 10_000);
    expect(el.scrollTop).toBe(0);
    expect(a.active(10_000)).toBe(false);
  });

  it('moves a screenful at a time when motion is reduced', () => {
    const el = list();
    const a = new AutoScroll(el, 0, { ...O, reducedMotion: true });
    a.tick(1000, 100);
    expect(el.scrollTop).toBe(400);
    a.tick(1200, 100);
    expect(el.scrollTop).toBe(400); // resting
    a.tick(1500, 100);
    expect(el.scrollTop).toBe(600); // to the bottom, no further
  });
});
