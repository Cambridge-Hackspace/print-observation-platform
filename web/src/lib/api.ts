// Calling POP's API from a screen. Every write is sent as JSON, even with
// nothing to say, since the server refuses any other write (its CSRF
// defence). An error carries the server's own words.

export type Answer<T> = { ok: true; status: number; data: T } | { ok: false; status: number; error: string };

export async function call<T>(method: 'GET' | 'POST' | 'PUT' | 'DELETE', url: string, body?: unknown): Promise<Answer<T>> {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: method === 'GET' ? {} : { 'Content-Type': 'application/json' },
      body: method === 'GET' ? undefined : JSON.stringify(body ?? {}),
    });
  } catch {
    return { ok: false, status: 0, error: 'POP didn’t answer. Check the connection and try again.' };
  }
  const text = await res.text();
  let parsed: unknown = null;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = null;
  }
  if (res.ok) return { ok: true, status: res.status, data: parsed as T };
  const said = (parsed as { error?: unknown } | null)?.error;
  return { ok: false, status: res.status, error: typeof said === 'string' ? said : `POP answered ${res.status}.` };
}
