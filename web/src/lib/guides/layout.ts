// The whole guide as a flowchart (#56, #94): each step a box, in rows by how
// far it is from the start, with an arrow for each answer. An arrow back up
// (a loop: "still stringing? start again") is drawn dashed, down the side.

import type { Walkable } from './walk';
import { answers, leadsTo } from './walk';

export const BOX_W = 168;
export const BOX_H = 44;
const GAP_X = 20;
const GAP_Y = 46;
const PAD = 12;
/** Room on the right for arrows that loop back. */
const LOOP_LANE = 28;

export interface Box {
  id: string;
  x: number;
  y: number;
  row: number;
  /** It ends the guide: it says what to do. */
  done: boolean;
}

export interface Arrow {
  from: string;
  to: string;
  /** "Yes", "No", or none for a plain next step. */
  label: string;
  /** Loops back up, to a step no further from the start. */
  back: boolean;
  /** An SVG path from the box to the box. */
  d: string;
}

export interface Chart {
  width: number;
  height: number;
  boxes: Box[];
  arrows: Arrow[];
}

export function layout(g: Walkable): Chart {
  // Rows by the fewest answers from the start; in each row, in the order
  // first reached.
  const row = new Map<string, number>([[g.start, 0]]);
  const order: string[] = [g.start];
  for (let i = 0; i < order.length; i++) {
    const id = order[i]!;
    const s = g.steps.find((x) => x.id === id)!;
    for (const a of answers(s)) {
      const to = leadsTo(s, a);
      if (to !== null && !row.has(to)) {
        row.set(to, row.get(id)! + 1);
        order.push(to);
      }
    }
  }
  const rows: string[][] = [];
  for (const id of order) (rows[row.get(id)!] ??= []).push(id);
  const widest = Math.max(...rows.map((r) => r.length));
  const inner = widest * BOX_W + (widest - 1) * GAP_X;
  const boxes = new Map<string, Box>();
  rows.forEach((ids, r) => {
    const w = ids.length * BOX_W + (ids.length - 1) * GAP_X;
    const left = PAD + (inner - w) / 2;
    ids.forEach((id, i) => {
      const s = g.steps.find((x) => x.id === id)!;
      boxes.set(id, {
        id,
        x: left + i * (BOX_W + GAP_X),
        y: PAD + r * (BOX_H + GAP_Y),
        row: r,
        done: s.then.kind === 'done',
      });
    });
  });
  const width = PAD * 2 + inner + LOOP_LANE;
  const arrows: Arrow[] = [];
  let lane = 0;
  for (const id of order) {
    const s = g.steps.find((x) => x.id === id)!;
    const from = boxes.get(id)!;
    for (const a of answers(s)) {
      const to = leadsTo(s, a)!;
      const target = boxes.get(to)!;
      const label = a === 'yes' ? 'Yes' : a === 'no' ? 'No' : '';
      const back = target.row <= from.row;
      let d: string;
      if (back) {
        // Out of the box's right side, down the lane, and in at the target's right.
        const x = PAD + inner + 8 + (lane++ % 3) * 6;
        const y1 = from.y + BOX_H / 2;
        const y2 = target.y + BOX_H / 2;
        d = `M${from.x + BOX_W} ${y1}H${x}V${y2}H${target.x + BOX_W}`;
      } else {
        const x1 = from.x + BOX_W / 2;
        const y1 = from.y + BOX_H;
        const x2 = target.x + BOX_W / 2;
        const y2 = target.y;
        const mid = y1 + GAP_Y / 2;
        d = x1 === x2 ? `M${x1} ${y1}V${y2}` : `M${x1} ${y1}V${mid}H${x2}V${y2}`;
      }
      arrows.push({ from: id, to, label, back, d });
    }
  }
  const height = PAD * 2 + rows.length * BOX_H + (rows.length - 1) * GAP_Y;
  return { width, height, boxes: [...boxes.values()], arrows };
}
