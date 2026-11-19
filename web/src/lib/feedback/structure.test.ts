// No screen renders a member's details outside `use:sensitive` (#96), so a
// new screen cannot forget to, and a feedback screenshot never shows them.
//
// The rules are written here, not taken from the code they check: what
// counts as a member's details is listed below, by how templates reach it.
// What this does not prove: a member's name reached some other way, say
// through a variable named for something else, is not seen. redact.ts's
// backstop, and the person looking at the picture before sending it, are
// what stand behind that.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { describe, expect, it } from 'vitest';

/** An expression that carries a member's details. */
const SENSITIVE: RegExp[] = [
  /\.(username|email|card|owner|job)\b/,
  // An API key, shown once when it is made (#13).
  /\.secret\b/,
  /\b(me|a|user|account|member|owner|who)\.name\b/,
  // A group notice's wording names the print's owner.
  /\btold\.text\b/,
  // People's confirmations name the account changed.
  /^said$/,
];

interface Finding {
  file: string;
  line: number;
  expr: string;
}

/** Each text expression in `template` that carries a member's details and
 *  is not inside an element marked `use:sensitive`. */
export function unmarked(template: string, file = 'template'): Finding[] {
  // Only the markup: scripts and styles are not rendered.
  const blank = (m: string) => m.replace(/[^\n]/g, ' ');
  const markup = template
    .replace(/<script[\s\S]*?<\/script>/g, blank)
    .replace(/<style[\s\S]*?<\/style>/g, blank);
  const found: Finding[] = [];
  const re = /\{([^{}#/:@][^{}]*)\}/g;
  for (let m = re.exec(markup); m; m = re.exec(markup)) {
    const expr = m[1]!.trim();
    if (!SENSITIVE.some((s) => s.test(expr))) continue;
    const before = markup.slice(0, m.index);
    // Inside a tag (an attribute's value): not rendered as text.
    if (before.lastIndexOf('<') > before.lastIndexOf('>')) continue;
    // The element it sits in: the last opening tag before it not closed.
    const stack: string[] = [];
    const tags = /<(\/?)([A-Za-z][\w.-]*)([^>]*?)(\/?)>/g;
    for (let t = tags.exec(before); t; t = tags.exec(before)) {
      if (t[1]) stack.pop();
      else if (!t[4]) stack.push(t[0]);
    }
    const parent = stack[stack.length - 1] ?? '';
    if (/\buse:sensitive\b/.test(parent)) continue;
    found.push({ file, line: before.split('\n').length, expr });
  }
  return found;
}

function svelteFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((n) => {
    const p = join(dir, n);
    if (statSync(p).isDirectory()) return svelteFiles(p);
    return n.endsWith('.svelte') ? [p] : [];
  });
}

describe('member details on screen', () => {
  it('are always inside use:sensitive', () => {
    const root = join(__dirname, '..', '..');
    const files = svelteFiles(root);
    expect(files.length).toBeGreaterThan(10);
    const found = files.flatMap((f) => unmarked(readFileSync(f, 'utf8'), relative(root, f)));
    expect(found, 'mark each with use:sensitive').toEqual([]);
  });

  // The check, fed what a screen that forgot would look like.
  it('fails a screen that renders a name unmarked', () => {
    expect(unmarked('<p>Hello, {me.name}</p>')).toEqual([{ file: 'template', line: 1, expr: 'me.name' }]);
    expect(unmarked('<td>{a.username}</td>').length).toBe(1);
    expect(unmarked('<span class="job">{w.job}</span>').length).toBe(1);
    expect(unmarked('<p>{told.text}</p>').length).toBe(1);
    // Marked on an outer element only: still the inner one shows it.
    expect(unmarked('<div use:sensitive={"name"}><p>{me.name}</p></div>').length).toBe(1);
  });

  it('passes what is marked, and what is not a member', () => {
    expect(unmarked(`<span use:sensitive={'name'}>{me.name}</span>`)).toEqual([]);
    expect(unmarked('<h2>{p.name}</h2><b>{printer.name}</b>')).toEqual([]);
    // A printer's name in an attribute is not a member's in text.
    expect(unmarked('<button aria-label="See {p.name}">x</button>')).toEqual([]);
    expect(unmarked('<script>let x = me.name;</script><p>hi</p>')).toEqual([]);
  });
});
