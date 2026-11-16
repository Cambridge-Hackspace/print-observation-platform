// The tower scrolls its printers on its own when nobody has touched it for a
// while (#49), so every card is seen from across the room. A touch stops it.
// Driven by ticks rather than timers of its own, so a test can drive it.

/** The parts of a scrolling element this uses. */
export interface Scroller {
  scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
}

export interface Options {
  /** How long nobody must have touched it. */
  idleMs: number;
  /** How fast it scrolls, in pixels a second. */
  speed: number;
  /** How long it rests at the top and at the bottom. */
  restMs: number;
  /** Move a screenful at a time, after each rest, rather than glide. */
  reducedMotion: boolean;
}

export const DEFAULTS: Options = { idleMs: 30_000, speed: 24, restMs: 5_000, reducedMotion: false };

export class AutoScroll {
  private touched: number;
  /** Resting (at the top, the bottom, or between screenfuls) until then. */
  private restUntil = 0;
  /** Already rested at the bottom: next, back to the top. */
  private restedAtBottom = false;
  constructor(
    private readonly el: Scroller,
    now: number,
    private readonly o: Options = DEFAULTS,
  ) {
    this.touched = now;
  }

  /** Someone touched the screen: stop, and wait to be idle again. */
  touch(now: number): void {
    this.touched = now;
    this.restUntil = 0;
    this.restedAtBottom = false;
  }

  /** Whether it scrolls on its own now: idle long enough, with more to show. */
  active(now: number): boolean {
    return now - this.touched >= this.o.idleMs && this.el.scrollHeight > this.el.clientHeight;
  }

  /** Move on by `dt` milliseconds, ending at `now`. */
  tick(now: number, dt: number): void {
    if (!this.active(now) || now < this.restUntil) return;
    const bottom = this.el.scrollHeight - this.el.clientHeight;
    if (this.el.scrollTop >= bottom - 0.5) {
      if (!this.restedAtBottom) {
        this.restedAtBottom = true;
      } else {
        this.el.scrollTop = 0;
        this.restedAtBottom = false;
      }
      this.restUntil = now + this.o.restMs;
      return;
    }
    if (this.o.reducedMotion) {
      this.el.scrollTop = Math.min(bottom, this.el.scrollTop + this.el.clientHeight);
      this.restUntil = now + this.o.restMs;
      return;
    }
    this.el.scrollTop = Math.min(bottom, this.el.scrollTop + (this.o.speed * dt) / 1000);
  }
}
