// The kiosk session's countdown and its keyboard (#12), pure.
import { describe, expect, it } from 'vitest';
import { apply, rows } from './keys';
import { PING_MS, clockOf, secondsLeft, shouldPing } from './session';

describe('the countdown', () => {
  it('counts from when POP last heard from the screen, and stops at zero', () => {
    expect(secondsLeft(0, 0, 90)).toBe(90);
    expect(secondsLeft(0, 45_000, 90)).toBe(45);
    expect(secondsLeft(0, 44_100, 90)).toBe(46);
    expect(secondsLeft(0, 90_000, 90)).toBe(0);
    expect(secondsLeft(0, 200_000, 90)).toBe(0);
  });

  it('reads like a clock', () => {
    expect(clockOf(45)).toBe('0:45');
    expect(clockOf(90)).toBe('1:30');
    expect(clockOf(5)).toBe('0:05');
    expect(clockOf(-3)).toBe('0:00');
  });

  it('passes a touch on to POP at most every ten seconds', () => {
    expect(PING_MS).toBe(10_000);
    expect(shouldPing(0, 9_999)).toBe(false);
    expect(shouldPing(0, 10_000)).toBe(true);
  });
});

describe('the on-screen keyboard', () => {
  it('types letters, capitals with shift, symbols, spaces, and takes one back', () => {
    const letters = rows(false, false).flat().filter((k) => k.kind === 'char').map((k) => (k.kind === 'char' ? k.char : ''));
    expect(letters.join('')).toBe('1234567890qwertyuiopasdfghjklzxcvbnm');
    const caps = rows(true, false).flat().filter((k) => k.kind === 'char').map((k) => (k.kind === 'char' ? k.char : ''));
    expect(caps).toContain('Q');
    const syms = rows(false, true).flat().filter((k) => k.kind === 'char').map((k) => (k.kind === 'char' ? k.char : ''));
    expect(syms).toEqual(expect.arrayContaining(['!', '@', "'", '"', '\\', '`']));
    let t = '';
    for (const k of [{ kind: 'char', char: 'a' }, { kind: 'space' }, { kind: 'char', char: 'B' }, { kind: 'back' }, { kind: 'back' }] as const) {
      t = apply(t, k);
    }
    expect(t).toBe('a');
    expect(apply('', { kind: 'back' })).toBe('');
    expect(apply('é', { kind: 'back' })).toBe('');
  });

  it('always offers shift or the symbols, backspace and space', () => {
    for (const [shift, symbols] of [[false, false], [true, false], [false, true]] as const) {
      const kinds = rows(shift, symbols).flat().map((k) => k.kind);
      expect(kinds).toContain('back');
      expect(kinds).toContain('space');
      expect(kinds).toContain('symbols');
    }
  });
});
