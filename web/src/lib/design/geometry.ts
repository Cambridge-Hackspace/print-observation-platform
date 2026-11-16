// The round 6 kit's drawings as geometry (#48), ported from
// docs/design/round-6/kit.py and held to it by design.test.ts's goldens.

/**
 * A number as Python's `f'{v:.1f}'` writes it. toFixed already rounds the
 * number exactly as stored (1.05, stored a hair above, becomes "1.1"); the two
 * differ only on an exact tie, which toFixed breaks upward (0.25 to "0.3") and
 * Python to even ("0.2"). A tie is judged on the stored number's exact decimal
 * expansion, never after scaling it, which can round a near-tie into one.
 */
export function fmt1(v: number): string {
  const exact = Math.abs(v).toFixed(20);
  const [int, frac] = exact.split('.') as [string, string];
  if (frac[1] === '5' && /^0+$/.test(frac.slice(2))) {
    const tenths = Number(int) * 10 + Number(frac[0]);
    const even = tenths % 2 === 0 ? tenths : tenths + 1;
    const s = (even / 10).toFixed(1);
    return v < 0 ? `-${s}` : s;
  }
  return v.toFixed(1);
}

/** A pine: its outline's path. */
export function pinePath(x: number, b: number, h: number): string {
  const w = h * 0.56;
  const pts: [number, number][] = [
    [0, -h],
    [0.3, -0.66],
    [0.17, -0.66],
    [0.42, -0.38],
    [0.27, -0.38],
    [0.5, -0.12],
    [0.07, -0.12],
    [0.07, 0],
  ];
  const right = pts.map(([dx, dy]): [number, number] =>
    dy > -1 ? [x + dx * w, b + dy * h] : [x, b - h],
  );
  const left = pts
    .slice(1)
    .reverse()
    .map(([dx, dy]): [number, number] => [x - dx * w, b + dy * h]);
  return 'M' + [...right, ...left].map(([px, py]) => `${fmt1(px)} ${fmt1(py)}`).join(' L') + 'Z';
}

/** A fern: a stem and pairs of fronds, shorter toward the top. */
export function fernPath(x: number, b: number, h: number): string {
  const segs = [`M${x} ${b}V${b - h}`];
  for (let i = 0; i < Math.floor(h / 11); i++) {
    const y = b - 8 - i * 11;
    const L = Math.max(5, 15 - i * 2);
    segs.push(`M${x} ${y}l${-L} ${fmt1(-L * 0.55)}M${x} ${y}l${L} ${fmt1(-L * 0.55)}`);
  }
  return segs.join('');
}

export interface Shroom {
  stem: { x: string; y: string; width: string; height: string };
  cap: string;
  spots: { x: string; y: string; size: string }[];
}

/** A mushroom of size `s` standing on `b`: its stem, cap and spots. */
export function shroom(x: number, b: number, s: number): Shroom {
  const q = s * 0.22;
  return {
    stem: { x: fmt1(x - s * 0.28), y: fmt1(b - s), width: fmt1(s * 0.56), height: fmt1(s) },
    cap: `M${fmt1(x - s)} ${fmt1(b - s * 0.9)}a${fmt1(s)} ${fmt1(s * 0.85)} 0 0 1 ${fmt1(2 * s)} 0z`,
    spots: (
      [
        [-0.5, 0.3],
        [0.1, 0.62],
        [0.5, 0.28],
      ] as const
    ).map(([dx, dy]) => ({
      x: fmt1(x + dx * s - q / 2),
      y: fmt1(b - s * 0.9 - dy * s - q / 2),
      size: fmt1(q),
    })),
  };
}

/** The radius of a spool's filament ring: its thickness is what is left. */
export function spoolRadius(frac: number): string {
  return fmt1(12 + 15 * clamp01(frac));
}

export function clamp01(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0;
}
