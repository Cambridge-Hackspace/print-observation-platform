// What a person draws on a feedback screenshot (#96), as plain data, with
// undo and redo. The annotation layer (Annotate.svelte) draws these with
// Konva; keeping them apart keeps the drawing's rules testable without a
// canvas.

export type Tool = 'pen' | 'arrow' | 'box' | 'text' | 'blackout';

export type Mark =
  | { tool: 'pen'; points: number[] }
  | { tool: 'arrow'; from: [number, number]; to: [number, number] }
  | { tool: 'box' | 'blackout'; from: [number, number]; to: [number, number] }
  | { tool: 'text'; at: [number, number]; text: string };

/** The marks so far, with what undo took, for redo. */
export interface Marks {
  done: Mark[];
  undone: Mark[];
}

export const none = (): Marks => ({ done: [], undone: [] });

/** A new mark: what was undone can no longer be redone. */
export function add(m: Marks, mark: Mark): Marks {
  if (!worthKeeping(mark)) return m;
  return { done: [...m.done, mark], undone: [] };
}

export function undo(m: Marks): Marks {
  const last = m.done[m.done.length - 1];
  return last ? { done: m.done.slice(0, -1), undone: [last, ...m.undone] } : m;
}

export function redo(m: Marks): Marks {
  const [next, ...rest] = m.undone;
  return next ? { done: [...m.done, next], undone: rest } : m;
}

/** A tap, or text left empty, is not a mark. */
export function worthKeeping(mark: Mark): boolean {
  switch (mark.tool) {
    case 'pen':
      return mark.points.length >= 4;
    case 'text':
      return mark.text.trim().length > 0;
    default: {
      const [x1, y1] = mark.from;
      const [x2, y2] = mark.to;
      return Math.abs(x2 - x1) + Math.abs(y2 - y1) >= 6;
    }
  }
}

/** A box's corner and size, however it was dragged. */
export function rect(from: [number, number], to: [number, number]) {
  return {
    x: Math.min(from[0], to[0]),
    y: Math.min(from[1], to[1]),
    width: Math.abs(to[0] - from[0]),
    height: Math.abs(to[1] - from[1]),
  };
}
