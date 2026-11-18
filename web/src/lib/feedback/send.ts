// Sending a feedback report (#96), and waiting to say it was sent. POP
// keeps a report and files it on GitHub; the screen says "sent" only once the
// issue exists, with a link to it. A report GitHub cannot take yet is kept by
// POP and sent when it can: the screen says so, rather than sent.

import type { Answer } from '../api';
import type { FeedbackStatus } from '../generated/FeedbackStatus';
import type { NewFeedback } from '../generated/NewFeedback';

export type Outcome =
  | { state: 'sent'; url: string }
  /** POP has it, and will send it when GitHub can be reached. */
  | { state: 'kept' }
  /** GitHub refused POP's App: an admin needs to look at POP's log. */
  | { state: 'failed' }
  /** POP would not take it, in its own words. */
  | { state: 'refused'; error: string };

type Call = <T>(method: 'GET' | 'POST', url: string, body?: unknown) => Promise<Answer<T>>;

export interface Waiting {
  /** How often to ask whether it has gone. */
  every: number;
  /** How long to wait before saying it is kept instead. */
  within: number;
  sleep?: (ms: number) => Promise<void>;
}

const pause = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export async function send(report: NewFeedback, call: Call, wait: Waiting = { every: 1000, within: 15000 }): Promise<Outcome> {
  const sleep = wait.sleep ?? pause;
  const kept = await call<FeedbackStatus>('POST', '/api/feedback', report);
  if (!kept.ok) return { state: 'refused', error: kept.error };
  const id = kept.data.id;
  for (let waited = 0; waited < wait.within; waited += wait.every) {
    await sleep(wait.every);
    const s = await call<FeedbackStatus>('GET', `/api/feedback/${encodeURIComponent(id)}`);
    if (!s.ok) continue;
    if (s.data.state === 'sent' && s.data.issue_url) return { state: 'sent', url: s.data.issue_url };
    if (s.data.state === 'failed') return { state: 'failed' };
  }
  return { state: 'kept' };
}
