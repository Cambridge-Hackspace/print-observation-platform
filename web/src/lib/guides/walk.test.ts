import { describe, expect, it } from 'vitest';
import { back, begin, choose, current, lastSaid, progress } from './walk';
import { STRINGING as G } from './fixture';

describe('walking a guide', () => {
  it('goes where each answer leads, and remembers the way', () => {
    let w = begin(G);
    expect(current(G, w).id).toBe('hairs');
    w = choose(G, w, 'yes');
    expect(current(G, w).id).toBe('dry-box');
    w = choose(G, w, 'no');
    expect(current(G, w).id).toBe('cooler');
    w = choose(G, w, 'next');
    expect(current(G, w).id).toBe('test-tower');
    expect(w.trail).toEqual(['hairs', 'dry-box', 'cooler', 'test-tower']);
    expect(w.answers).toEqual(['yes', 'no', 'next']);
  });

  it('ignores an answer the step does not take', () => {
    const w = begin(G);
    expect(choose(G, w, 'next')).toBe(w);
    const end = choose(G, w, 'no');
    expect(current(G, end).id).toBe('blobs');
    expect(choose(G, end, 'yes')).toBe(end);
  });

  it('goes back a step at a time, and no further than the start', () => {
    let w = choose(G, choose(G, begin(G), 'yes'), 'yes');
    w = back(w);
    expect(current(G, w).id).toBe('dry-box');
    w = back(back(w));
    expect(current(G, w).id).toBe('hairs');
    expect(w.answers).toEqual([]);
  });

  it('says how far, and about how far in all', () => {
    let w = begin(G);
    // hairs, then blobs: the shortest way to the end is two.
    expect(progress(G, w)).toEqual({ at: 1, about: 2 });
    w = choose(G, w, 'yes');
    // The round 6 canvas: "Step 2 of about 4".
    expect(progress(G, w)).toEqual({ at: 2, about: 4 });
    w = choose(G, choose(G, w, 'yes'), 'next');
    expect(progress(G, w)).toEqual({ at: 4, about: 4 });
  });

  it('remembers what was said at the last question', () => {
    let w = begin(G);
    expect(lastSaid(G, w)).toBeNull();
    w = choose(G, choose(G, w, 'yes'), 'no');
    expect(lastSaid(G, w)).toEqual({ question: 'Has this spool been out of the dry box for more than a day?', answer: 'no' });
    w = choose(G, w, 'next');
    expect(lastSaid(G, w)?.answer).toBe('no');
  });
});
