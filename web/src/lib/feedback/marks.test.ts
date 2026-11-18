// Drawing on a feedback screenshot (#96): undo, redo, and what counts as a mark.
import { describe, expect, it } from 'vitest';
import { add, none, rect, redo, undo, type Mark } from './marks';

const box: Mark = { tool: 'box', from: [10, 10], to: [60, 40] };
const black: Mark = { tool: 'blackout', from: [100, 100], to: [20, 30] };

describe('marks', () => {
  it('undo takes the last mark back, and redo puts it back', () => {
    let m = add(add(none(), box), black);
    m = undo(m);
    expect(m.done).toEqual([box]);
    m = undo(m);
    expect(m.done).toEqual([]);
    m = undo(m);
    expect(m.done).toEqual([]);
    m = redo(m);
    expect(m.done).toEqual([box]);
    m = redo(redo(m));
    expect(m.done).toEqual([box, black]);
  });

  it('a new mark after undo ends what redo could bring back', () => {
    let m = undo(add(add(none(), box), black));
    m = add(m, { tool: 'arrow', from: [0, 0], to: [50, 50] });
    expect(redo(m).done.map((x) => x.tool)).toEqual(['box', 'arrow']);
  });

  it('a tap, or empty text, is not a mark', () => {
    expect(add(none(), { tool: 'box', from: [5, 5], to: [7, 6] }).done).toEqual([]);
    expect(add(none(), { tool: 'pen', points: [1, 1] }).done).toEqual([]);
    expect(add(none(), { tool: 'text', at: [1, 1], text: '  ' }).done).toEqual([]);
    expect(add(none(), { tool: 'text', at: [1, 1], text: 'here' }).done.length).toBe(1);
  });

  it('a box dragged up and left is the same box', () => {
    expect(rect([100, 100], [20, 30])).toEqual({ x: 20, y: 30, width: 80, height: 70 });
  });
});
