// Sending feedback (#96): "sent" only once the issue exists.
import { describe, expect, it } from 'vitest';
import type { Answer } from '../api';
import type { NewFeedback } from '../generated/NewFeedback';
import { send } from './send';

const report: NewFeedback = {
  kind: 'bug',
  title: 't',
  description: '',
  device: 'kiosk',
  screenshot: '',
  context: {},
};

/** A POP whose report goes through `states`, one per question. */
function pop(states: ('pending' | 'sent' | 'failed')[], post: Answer<unknown> = { ok: true, status: 202, data: { id: 'r1', state: 'pending', issue_url: null } }) {
  const asked: string[] = [];
  const call = async <T,>(method: string, url: string): Promise<Answer<T>> => {
    asked.push(`${method} ${url}`);
    if (method === 'POST') return post as Answer<T>;
    const state = states.shift() ?? 'pending';
    return {
      ok: true,
      status: 200,
      data: { id: 'r1', state, issue_url: state === 'sent' ? 'https://github.com/acme/pop/issues/7' : null } as T,
    };
  };
  return { call, asked };
}

const now = { every: 10, within: 50, sleep: async () => {} };

describe('sending feedback', () => {
  it('is sent once the issue exists, and not before', async () => {
    const p = pop(['pending', 'pending', 'sent']);
    expect(await send(report, p.call, now)).toEqual({ state: 'sent', url: 'https://github.com/acme/pop/issues/7' });
    expect(p.asked).toEqual(['POST /api/feedback', 'GET /api/feedback/r1', 'GET /api/feedback/r1', 'GET /api/feedback/r1']);
  });

  it('is kept, not sent, while GitHub cannot take it', async () => {
    expect(await send(report, pop([]).call, now)).toEqual({ state: 'kept' });
  });

  it('says when GitHub refused it', async () => {
    expect(await send(report, pop(['failed']).call, now)).toEqual({ state: 'failed' });
  });

  it('says why POP would not take it', async () => {
    const p = pop([], { ok: false, status: 429, error: 'at most 10 reports an hour; try again later' });
    expect(await send(report, p.call, now)).toEqual({ state: 'refused', error: 'at most 10 reports an hour; try again later' });
  });
});
