// The kiosk's on-screen keyboard (#12): the kiosk has no keyboard, and
// signing in by password needs one. Pure, so its layout and editing are
// tested without a screen.

export type Key = { kind: 'char'; char: string } | { kind: 'shift' } | { kind: 'symbols' } | { kind: 'back' } | { kind: 'space' };

const LETTERS = ['1234567890', 'qwertyuiop', 'asdfghjkl', 'zxcvbnm'];
const SYMBOLS = ['1234567890', '!@#$%^&*()', `-_=+.,'"?/`, ':;<>[]{}~\\|`'];

/** The rows to draw: letters (upper-case with shift) or symbols. */
export function rows(shift: boolean, symbols: boolean): Key[][] {
  const chars = (symbols ? SYMBOLS : LETTERS).map((r) =>
    [...r].map((c): Key => ({ kind: 'char', char: shift && !symbols ? c.toUpperCase() : c })),
  );
  // Shift with the letters; with the symbols there is none to shift.
  if (!symbols) chars[3]!.unshift({ kind: 'shift' });
  chars[3]!.push({ kind: 'back' });
  // The toggle between letters and symbols, and the space bar.
  return [...chars, [{ kind: 'symbols' }, { kind: 'space' }]];
}

/** `text` after `key`. */
export function apply(text: string, key: Key): string {
  switch (key.kind) {
    case 'char':
      return text + key.char;
    case 'space':
      return text + ' ';
    case 'back':
      return [...text].slice(0, -1).join('');
    default:
      return text;
  }
}
