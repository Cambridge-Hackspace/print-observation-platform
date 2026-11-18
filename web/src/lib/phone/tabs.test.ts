import { describe, expect, it } from 'vitest';
import { followUrl, tabFrom, watchHash, watching } from './tabs';

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

  it('makes the address a kiosk QR code holds to follow a print', () => {
    expect(followUrl('https://pop.example.org', 'p1')).toBe('https://pop.example.org/#watch/p1');
    expect(followUrl('https://pop.example.org/', 'a b')).toBe('https://pop.example.org/#watch/a%20b');
    // A POP served under a path keeps it.
    expect(followUrl('https://example.org/pop/', 'p1')).toBe('https://example.org/pop/#watch/p1');
  });
});
