import { describe, expect, it } from 'vitest';
import { tabFrom, watchHash, watching } from './tabs';

describe('the phone app’s sections', () => {
  it('are named by the hash, and anything else is the printers', () => {
    expect(tabFrom('#you')).toBe('you');
    expect(tabFrom('#printers')).toBe('printers');
    expect(tabFrom('')).toBe('printers');
    expect(tabFrom('#admin')).toBe('printers');
  });

  it('watch a printer by its id, and back again', () => {
    expect(watching('#watch/p1')).toBe('p1');
    expect(watching(watchHash('a b/c'))).toBe('a b/c');
    expect(watching('#printers')).toBeNull();
    expect(watching('#watch/')).toBeNull();
    expect(watching('#watch/%E0%A4%A')).toBeNull();
    // Watching is not a section of its own: the printers' list is behind it.
    expect(tabFrom(watchHash('p1'))).toBe('printers');
  });
});
