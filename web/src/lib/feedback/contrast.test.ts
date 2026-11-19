// Text on the feedback screens can be read (#192): the drawing tools' bar and
// the form sat on cream, and the design system's plain buttons draw light text
// with no fill of their own, so Pen, Undo, Cancel and Close were cream on
// cream. Read from the source, the colors resolved through tokens.css, and
// held to WCAG AA for normal text, 4.5:1.
//
// What it does not prove: anything a browser adds (its own form controls,
// focus rings), or colors set other than by these rules.
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const src = (p: string) => readFileSync(resolve(__dirname, p), 'utf8');

const tokens = new Map(
  [...src('../design/tokens.css').matchAll(/(--pop-[a-z0-9-]+):\s*(#[0-9a-f]{6})\b/gi)].map((m) => [m[1]!, m[2]!]),
);

/** One property of one rule in a component's <style>, as a hex color. */
function color(file: string, selector: string, property: string): string {
  const style = src(file).split('<style>')[1]!;
  const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const rule = new RegExp(`(?:^|\\n)\\s*${esc}\\s*\\{([^}]*)\\}`).exec(style);
  if (!rule) throw new Error(`${file} has no rule ${selector}`);
  const decl = new RegExp(`(?:^|[;\\s])${property}:\\s*([^;]+);`).exec(rule[1]!);
  if (!decl) throw new Error(`${file} ${selector} sets no ${property}`);
  const v = decl[1]!.trim();
  // A token, with or without a fallback: the token is defined, so it wins.
  const name = /^var\((--pop-[a-z0-9-]+)(?:,[^)]*)?\)$/.exec(v)?.[1];
  const hex = name ? tokens.get(name) : v;
  if (!hex || !/^#[0-9a-f]{6}$/i.test(hex)) throw new Error(`${file} ${selector} ${property}: ${v} is not a token or a hex color`);
  return hex;
}

/** WCAG 2's contrast ratio of two colors. */
function contrast(a: string, b: string): number {
  const lum = (hex: string) => {
    const [r, g, bl] = [1, 3, 5].map((i) => {
      const c = parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r! + 0.7152 * g! + 0.0722 * bl!;
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

const BUTTON = '../design/Button.svelte';
const plain = () => color(BUTTON, '.btn', 'color');
const sheet = () => color('Feedback.svelte', '.sheet', 'background');
const bar = () => color('Annotate.svelte', '.tools', 'background');

const pairs: [string, () => string, () => string][] = [
  ['a plain button (Pen, Undo, Cancel) on the tools bar', plain, bar],
  ['a plain button (Cancel, Close) on the form', plain, sheet],
  ['the form’s own text', () => color('Feedback.svelte', '.sheet', 'color'), sheet],
  ['a problem on the form', () => color('Feedback.svelte', '.problem', 'color'), sheet],
  ['a link on the form (See the issue)', () => color('Feedback.svelte', '.sheet a', 'color'), sheet],
  ['a primary button’s text on its fill', () => color(BUTTON, '.primary', 'color'), () => color(BUTTON, '.primary', 'background')],
];

describe('text on the feedback screens can be read', () => {
  it.each(pairs)('%s', (_, fg, bg) => {
    expect(contrast(fg(), bg())).toBeGreaterThanOrEqual(4.5);
  });

  it('measures what it says: cream on cream fails, ink on cream passes', () => {
    // The oracle itself, on the defect it exists for.
    expect(contrast(tokens.get('--pop-text')!, tokens.get('--pop-cream')!)).toBeLessThan(1.5);
    expect(contrast(tokens.get('--pop-ink')!, tokens.get('--pop-cream')!)).toBeGreaterThan(4.5);
  });
});
