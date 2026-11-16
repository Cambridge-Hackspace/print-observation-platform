import { describe, expect, it } from 'vitest';
import { tabFrom } from './tabs';

describe('the phone app’s sections', () => {
  it('are named by the hash, and anything else is the printers', () => {
    expect(tabFrom('#you')).toBe('you');
    expect(tabFrom('#printers')).toBe('printers');
    expect(tabFrom('')).toBe('printers');
    expect(tabFrom('#admin')).toBe('printers');
  });
});
