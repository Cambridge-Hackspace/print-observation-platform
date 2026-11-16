// Walking a guide (#56, #72, #94): one step or yes/no question at a time,
// remembering the path taken so far. Pure, so every viewer shares it.

import type { GuideStep } from '../generated/GuideStep';

/** What a viewer needs of a guide to walk it. */
export interface Walkable {
  start: string;
  steps: GuideStep[];
}

export type Answer = 'yes' | 'no' | 'next';

/** Where someone is: the steps they have been at, and how they left each. */
export interface Walk {
  trail: string[];
  answers: Answer[];
}

export const begin = (g: Walkable): Walk => ({ trail: [g.start], answers: [] });

export function step(g: Walkable, id: string): GuideStep {
  const s = g.steps.find((x) => x.id === id);
  if (!s) throw new Error(`the guide has no step ${id}`);
  return s;
}

export const current = (g: Walkable, w: Walk): GuideStep => step(g, w.trail[w.trail.length - 1]!);

/** The step `answer` leads to from `s`, if it leads anywhere. */
export function leadsTo(s: GuideStep, answer: Answer): string | null {
  const t = s.then;
  if (t.kind === 'next') return answer === 'next' ? t.step : null;
  if (t.kind === 'question') return answer === 'yes' ? t.yes : answer === 'no' ? t.no : null;
  return null;
}

/** The answers the current step takes. */
export function answers(s: GuideStep): Answer[] {
  switch (s.then.kind) {
    case 'question':
      return ['yes', 'no'];
    case 'next':
      return ['next'];
    case 'done':
      return [];
  }
}

/** On from the current step. An answer it does not take changes nothing. */
export function choose(g: Walkable, w: Walk, answer: Answer): Walk {
  const to = leadsTo(current(g, w), answer);
  return to === null ? w : { trail: [...w.trail, to], answers: [...w.answers, answer] };
}

/** Back one step; at the start, nowhere. */
export function back(w: Walk): Walk {
  return w.trail.length <= 1 ? w : { trail: w.trail.slice(0, -1), answers: w.answers.slice(0, -1) };
}

/** The fewest steps from `id` to one that says what to do, `id` and that one included. */
export function toTheEnd(g: Walkable, id: string): number {
  const seen = new Set<string>([id]);
  let layer = [id];
  for (let n = 1; layer.length > 0; n++) {
    const next: string[] = [];
    for (const x of layer) {
      const s = step(g, x);
      if (s.then.kind === 'done') return n;
      for (const a of answers(s)) {
        const to = leadsTo(s, a);
        if (to !== null && !seen.has(to)) {
          seen.add(to);
          next.push(to);
        }
      }
    }
    layer = next;
  }
  // A guide POP kept always has a way to the end (#71).
  return 1;
}

/** "Step 2 of about 4": how far, and about how far in all. */
export function progress(g: Walkable, w: Walk): { at: number; about: number } {
  const at = w.trail.length;
  return { at, about: at - 1 + toTheEnd(g, w.trail[at - 1]!) };
}

/** What was said at the last question, to show above the next step. */
export function lastSaid(g: Walkable, w: Walk): { question: string; answer: 'yes' | 'no' } | null {
  for (let i = w.answers.length - 1; i >= 0; i--) {
    const a = w.answers[i]!;
    if (a === 'yes' || a === 'no') return { question: step(g, w.trail[i]!).title, answer: a };
  }
  return null;
}
