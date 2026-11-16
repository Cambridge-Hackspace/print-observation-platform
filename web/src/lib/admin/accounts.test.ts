// Managing local users (#90): the choices and words on the People section.
import { describe, expect, it } from 'vitest';
import { holds, summary, tick, trainingChoices } from './accounts';
import type { Account } from '../generated/Account';
import type { AdminPrinter } from '../generated/AdminPrinter';

const printer = (training: string | null): AdminPrinter => ({
  bam_id: `id-${training}`,
  name: 'P',
  ready: true,
  connection: 'moonraker',
  problems: [],
  tower: 'added',
  categories: [],
  training,
  camera: false,
});
const account = (training: string[], more: Partial<Account> = {}): Account => ({
  id: 1,
  username: 'grace',
  name: 'Grace',
  role: 'member',
  active: true,
  training,
  ...more,
});

describe('training choices', () => {
  it('are every printer’s training and every account’s, once each whatever the case, sorted', () => {
    expect(trainingChoices([printer('advanced'), printer(null), printer(' basic '), printer('Advanced')], [account(['resin', 'BASIC'])])).toEqual([
      'advanced',
      'basic',
      'resin',
    ]);
  });
  it('tick on and off without regard to case', () => {
    expect(holds(['Advanced'], 'advanced')).toBe(true);
    expect(holds(['basic'], 'advanced')).toBe(false);
    expect(tick(['Advanced', 'basic'], 'advanced', false)).toEqual(['basic']);
    expect(tick(['basic'], 'advanced', true)).toEqual(['basic', 'advanced']);
    expect(tick(['Advanced'], 'advanced', true)).toEqual(['advanced']);
  });
  it('summarize an account', () => {
    expect(summary(account(['advanced', 'basic'], { role: 'admin' }))).toBe('Admin · advanced, basic');
    expect(summary(account([]))).toBe('Member · no training');
  });
});
