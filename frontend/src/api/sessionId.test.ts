// Spec 48 F1: a per-tab session id (sessionStorage key `dpds-session-id`, crypto.randomUUID() when available, else
// `Math.random().toString(36).slice(2) + Date.now().toString(36)`), sent as `X-Session-Id` on EVERY API request.
// Tests the REAL client module (setup.ts mocks it for everything else) against a stubbed fetch.
import { vi } from 'vitest';

type Client = typeof import('./client');
const KEY = 'dpds-session-id';
const fetchMock = vi.fn();

/** A fresh module instance, as after a page reload (the in-memory state is gone, sessionStorage stays). */
async function loadClient(): Promise<Client> {
  vi.resetModules();
  return vi.importActual<Client>('./client');
}

function ok(body: unknown = {}) {
  fetchMock.mockImplementation(async () => new Response(JSON.stringify(body), { status: 200 }));
}

function sentIds(): (string | null)[] {
  return fetchMock.mock.calls.map((_, i) => sentHeaders(i).get('x-session-id'));
}

function sentHeaders(i: number): Headers {
  const init = (fetchMock.mock.calls[i][1] ?? {}) as RequestInit;
  return new Headers(init.headers);
}

beforeEach(() => {
  fetchMock.mockReset();
  ok();
  vi.stubGlobal('fetch', fetchMock);
  sessionStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('session id (spec 48 F1)', () => {
  it('sends a non-empty X-Session-Id and keeps it in sessionStorage under dpds-session-id', async () => {
    const client = await loadClient();
    await client.getOnline();
    const [id] = sentIds();
    expect(id).toBeTruthy();
    expect(sessionStorage.getItem(KEY)).toBe(id);
  });

  it('is the same id on every request of one page', async () => {
    const client = await loadClient();
    await client.getOnline();
    await client.getEmulation();
    await client.stopEmulation();
    const ids = sentIds();
    expect(ids).toHaveLength(3);
    expect(new Set(ids).size).toBe(1);
    expect(ids[0]).toBeTruthy();
  });

  it('is stable across a reload of the same tab (sessionStorage survives)', async () => {
    const first = await loadClient();
    await first.getOnline();
    const second = await loadClient();
    await second.getOnline();
    const [a, b] = sentIds();
    expect(a).toBeTruthy();
    expect(b).toBe(a);
  });

  it('uses an id that is already in sessionStorage', async () => {
    sessionStorage.setItem(KEY, 'preset-id');
    const client = await loadClient();
    await client.getOnline();
    expect(sentIds()).toEqual(['preset-id']);
  });

  it('differs between tabs (empty sessionStorage), with the real randomUUID', async () => {
    const a = await loadClient();
    await a.getOnline();
    sessionStorage.clear();
    const b = await loadClient();
    await b.getOnline();
    const [x, y] = sentIds();
    expect(x).toBeTruthy();
    expect(y).toBeTruthy();
    expect(x).not.toBe(y);
  });

  it('uses crypto.randomUUID() when it exists', async () => {
    vi.stubGlobal('crypto', { randomUUID: () => 'uuid-from-crypto' });
    const client = await loadClient();
    await client.getOnline();
    expect(sentIds()).toEqual(['uuid-from-crypto']);
    expect(sessionStorage.getItem(KEY)).toBe('uuid-from-crypto');
  });

  it('falls back to Math.random().toString(36).slice(2) + Date.now().toString(36) without randomUUID', async () => {
    vi.stubGlobal('crypto', {});
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    vi.spyOn(Date, 'now').mockReturnValue(1_700_000_000_000);
    const expected = (0.5).toString(36).slice(2) + (1_700_000_000_000).toString(36);
    const client = await loadClient();
    await client.getOnline();
    expect(sentIds()).toEqual([expected]);
    expect(sessionStorage.getItem(KEY)).toBe(expected);
  });

  it('is sent on GET and on POST (next to Content-Type) and on a body-less POST', async () => {
    sessionStorage.setItem(KEY, 'tab-1');
    const client = await loadClient();
    await client.getInterfaces();
    await client.startEmulation({ links: [] });
    await client.stopEmulation();
    expect(sentIds()).toEqual(['tab-1', 'tab-1', 'tab-1']);
    expect(sentHeaders(0).get('content-type')).toBeNull();
    expect(sentHeaders(1).get('content-type')).toBe('application/json');
    expect(sentHeaders(2).get('content-type')).toBe('application/json');
  });

  it('is sent on every request function', async () => {
    sessionStorage.setItem(KEY, 'tab-2');
    const client = await loadClient();
    await client.getOnline();
    await client.getInterfaces();
    await client.getHostHints();
    await client.getHostHintsFor({ links: [] });
    await client.getEmulation();
    await client.getStats();
    await client.getDelayHistogram(1);
    await client.getStatsHistory();
    expect(sentIds()).toEqual(Array(8).fill('tab-2'));
  });

  it('is also sent when the request fails (HTTP 500)', async () => {
    sessionStorage.setItem(KEY, 'tab-3');
    fetchMock.mockImplementation(async () => new Response('{"message":"x"}', { status: 500 }));
    const client = await loadClient();
    await expect(client.getOnline()).rejects.toThrow();
    expect(sentIds()).toEqual(['tab-3']);
  });

  it('keeps one id in memory when sessionStorage throws, and still sends it', async () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('denied');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('denied');
    });
    const client = await loadClient();
    await client.getOnline();
    await client.getEmulation();
    const [a, b] = sentIds();
    expect(a).toBeTruthy();
    expect(b).toBe(a);
  });
});
