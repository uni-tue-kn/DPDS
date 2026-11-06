// The REAL client module (setup.ts mocks it everywhere else) against a stubbed fetch: the session id on every request and the
// statistics query strings.
import { vi } from 'vitest';
import * as fx from '../test/fixtures';

type Client = typeof import('./client');
const KEY = 'dpds-session-id';
const fetchMock = vi.fn();

/** A fresh module instance, as after a page reload (the in-memory id is gone, sessionStorage stays). */
async function loadClient(): Promise<Client> {
  vi.resetModules();
  return vi.importActual<Client>('./client');
}

const sentHeaders = (i: number) => new Headers(((fetchMock.mock.calls[i]?.[1] ?? {}) as RequestInit).headers);
const sentIds = () => fetchMock.mock.calls.map((_, i) => sentHeaders(i).get('x-session-id'));
const urls = () => fetchMock.mock.calls.map((c) => String(c[0]));

beforeEach(() => {
  fetchMock.mockReset();
  fetchMock.mockImplementation(async () => new Response('{}', { status: 200 }));
  vi.stubGlobal('fetch', fetchMock);
  sessionStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('session id', () => {
  it('is one id per tab, kept in sessionStorage, sent on every kind of request and also when the request fails', async () => {
    const client = await loadClient();
    await client.getOnline();
    await client.startEmulation({ links: [] });
    await client.stopEmulation();
    fetchMock.mockImplementation(async () => new Response('{"message":"x"}', { status: 500 }));
    await expect(client.getEmulation()).rejects.toThrow();
    const [id] = sentIds();
    expect(id).toBeTruthy();
    expect(sentIds()).toEqual([id, id, id, id]);
    expect(sessionStorage.getItem(KEY)).toBe(id);
    // every non-GET request says it carries JSON, also the body-less stop
    expect(sentHeaders(0).get('content-type')).toBeNull();
    expect(sentHeaders(1).get('content-type')).toBe('application/json');
    expect(sentHeaders(2).get('content-type')).toBe('application/json');
  });

  it('survives a reload of the tab and uses an id that is already stored; another tab gets another id', async () => {
    sessionStorage.setItem(KEY, 'preset-id');
    await (await loadClient()).getOnline();
    await (await loadClient()).getOnline();
    sessionStorage.clear();
    await (await loadClient()).getOnline();
    const [a, b, c] = sentIds();
    expect([a, b]).toEqual(['preset-id', 'preset-id']);
    expect(c).toBeTruthy();
    expect(c).not.toBe('preset-id');
  });

  it('falls back to a random id without crypto.randomUUID, and keeps one in memory when sessionStorage throws', async () => {
    vi.stubGlobal('crypto', {});
    const client = await loadClient();
    await client.getOnline();
    expect(sentIds()[0]).toBeTruthy();
    expect(sessionStorage.getItem(KEY)).toBe(sentIds()[0]);

    sessionStorage.clear();
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('denied');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('denied');
    });
    const offline = await loadClient();
    await offline.getOnline();
    await offline.getEmulation();
    const [, x, y] = sentIds();
    expect(x).toBeTruthy();
    expect(y).toBe(x);
  });
});

describe('statistics requests', () => {
  it('the delay histogram window is a query only when given', async () => {
    const client = await loadClient();
    fetchMock.mockResolvedValueOnce(new Response(JSON.stringify(fx.histogram(2)), { status: 200 }));
    await client.getDelayHistogram(2);
    await client.getDelayHistogram(1, 60);
    expect(urls()).toEqual(['/api/stats/2/delay_histogram', '/api/stats/1/delay_histogram?window_s=60']);
  });

  it('the history request asks for samples newer than `sinceMs`, where 0 is a real time', async () => {
    const client = await loadClient();
    await client.getStatsHistory();
    await client.getStatsHistory(0);
    await client.getStatsHistory(12500);
    expect(urls()).toEqual([
      '/api/stats/history',
      '/api/stats/history?since_ms=0',
      '/api/stats/history?since_ms=12500',
    ]);
  });
});
