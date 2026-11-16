import { describe, expect, it } from 'vitest';
import { BOX_H, BOX_W, layout } from './layout';
import { STRINGING } from './fixture';
import type { Walkable } from './walk';

const overlaps = (a: { x: number; y: number }, b: { x: number; y: number }) =>
  a.x < b.x + BOX_W && b.x < a.x + BOX_W && a.y < b.y + BOX_H && b.y < a.y + BOX_H;

describe('the flowchart', () => {
  it('puts each step in a row by how far it is from the start, none overlapping', () => {
    const c = layout(STRINGING);
    const row = Object.fromEntries(c.boxes.map((b) => [b.id, b.row]));
    expect(row).toEqual({ hairs: 0, 'dry-box': 1, blobs: 1, 'dry-it': 2, cooler: 2, 'test-tower': 3 });
    for (const a of c.boxes) for (const b of c.boxes) if (a !== b) expect(overlaps(a, b), `${a.id} ${b.id}`).toBe(false);
    for (const b of c.boxes) {
      expect(b.x).toBeGreaterThanOrEqual(0);
      expect(b.x + BOX_W).toBeLessThanOrEqual(c.width);
      expect(b.y + BOX_H).toBeLessThanOrEqual(c.height);
    }
  });

  it('draws an arrow for every answer, labelled', () => {
    const c = layout(STRINGING);
    expect(c.arrows.map((a) => `${a.from}>${a.to}:${a.label}`).sort()).toEqual(
      ['hairs>dry-box:Yes', 'hairs>blobs:No', 'dry-box>dry-it:Yes', 'dry-box>cooler:No', 'dry-it>test-tower:', 'cooler>test-tower:'].sort(),
    );
    expect(c.arrows.every((a) => !a.back)).toBe(true);
    expect(c.boxes.filter((b) => b.done).map((b) => b.id).sort()).toEqual(['blobs', 'test-tower']);
  });

  it('draws an arrow to a step in the same row down the side, not through the row', () => {
    // "Dry it" then "Print cooler": both two answers from the start.
    const sideways: Walkable = {
      ...STRINGING,
      steps: STRINGING.steps.map((s) => (s.id === 'dry-it' ? { ...s, then: { kind: 'next' as const, step: 'cooler' } } : s)),
    };
    const c = layout(sideways);
    const a = c.arrows.find((x) => x.from === 'dry-it' && x.to === 'cooler')!;
    expect(a.back).toBe(true);
  });

  it('draws a loop back up the side, dashed', () => {
    const looping: Walkable = {
      ...STRINGING,
      steps: STRINGING.steps.map((s) =>
        s.id === 'test-tower' ? { ...s, then: { kind: 'question' as const, yes: 'hairs', no: 'blobs' } } : s,
      ),
    };
    const c = layout(looping);
    const loop = c.arrows.find((a) => a.from === 'test-tower' && a.to === 'hairs')!;
    expect(loop.back).toBe(true);
    const lane = Math.max(...c.boxes.map((b) => b.x + BOX_W));
    // Down the side, right of every box, and inside the drawing.
    const xs = [...loop.d.matchAll(/H(-?[\d.]+)/g)].map((m) => Number(m[1]));
    expect(Math.max(...xs)).toBeGreaterThan(lane);
    expect(Math.max(...xs)).toBeLessThanOrEqual(c.width);
  });
});
