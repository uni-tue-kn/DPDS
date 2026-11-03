// Tests the REAL client module (setup.ts mocks it for everything else) against a stubbed fetch.
import { vi } from 'vitest';
import * as fx from '../test/fixtures';

type Client = typeof import('./client');
let client: Client;
const fetchMock = vi.fn();

function reply(status: number, body?: unknown, raw?: string) {
  fetchMock.mockResolvedValueOnce(
    new Response(raw ?? (body === undefined ? null : JSON.stringify(body)), {
      status,
      headers: raw === undefined && body === undefined ? {} : { 'content-type': 'application/json' },
    }),
  );
}

function lastCall(): { url: string; init: RequestInit } {
  const [url, init] = fetchMock.mock.calls[fetchMock.mock.calls.length - 1];
  return { url: String(url), init: (init ?? {}) as RequestInit };
}

function header(init: RequestInit, name: string): string | null {
  return new Headers(init.headers).get(name);
}

beforeAll(async () => {
  client = await vi.importActual<Client>('./client');
});
beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe('api client (real module)', () => {
  it.each([
    ['getOnline', () => client.getOnline(), '/api/online'],
    ['getInterfaces', () => client.getInterfaces(), '/api/interfaces'],
    ['getHostHints', () => client.getHostHints(), '/api/host/hints'],
    ['getLinks', () => client.getLinks(), '/api/links'],
    ['getLink', () => client.getLink(3), '/api/links/3'],
    ['getSettings', () => client.getSettings(), '/api/settings'],
    ['getEmulation', () => client.getEmulation(), '/api/emulation'],
    ['getStats', () => client.getStats(), '/api/stats'],
    ['getDelayHistogram', () => client.getDelayHistogram(2), '/api/stats/2/delay_histogram'],
  ])('%s GETs %s and returns the parsed body', async (_name, call, url) => {
    reply(200, { ok: 1 });
    await expect(call()).resolves.toEqual({ ok: 1 });
    expect(lastCall().url).toBe(url);
    expect((lastCall().init.method ?? 'GET').toUpperCase()).toBe('GET');
  });

  it('createLink POSTs the JSON body with a JSON content type', async () => {
    const input = { name: 'a-b', ingress: '0000:65:00.0', egress: '0000:65:00.1', config: fx.config(), worker: null };
    reply(201, { id: 5, ...input });
    await expect(client.createLink(input)).resolves.toMatchObject({ id: 5 });
    const { url, init } = lastCall();
    expect(url).toBe('/api/links');
    expect(init.method?.toUpperCase()).toBe('POST');
    expect(header(init, 'content-type')).toMatch(/^application\/json/);
    expect(JSON.parse(String(init.body))).toEqual(input);
  });

  it('updateLink PUTs to /api/links/{id}', async () => {
    const input = { name: 'x', ingress: 'net_null0', egress: 'net_null0', config: fx.config(), worker: 3 };
    reply(200, { id: 7, ...input });
    await client.updateLink(7, input);
    const { url, init } = lastCall();
    expect(url).toBe('/api/links/7');
    expect(init.method?.toUpperCase()).toBe('PUT');
    expect(JSON.parse(String(init.body))).toEqual(input);
    expect(header(init, 'content-type')).toMatch(/^application\/json/);
  });

  it('updateSettings PUTs /api/settings', async () => {
    const s = fx.settings({ main_lcore: 1 });
    reply(200, s);
    await expect(client.updateSettings(s)).resolves.toEqual(s);
    const { url, init } = lastCall();
    expect(url).toBe('/api/settings');
    expect(init.method?.toUpperCase()).toBe('PUT');
    expect(JSON.parse(String(init.body))).toEqual(s);
  });

  it('deleteLink DELETEs and resolves on 204 without a body', async () => {
    reply(204);
    await expect(client.deleteLink(4)).resolves.toBeUndefined();
    const { url, init } = lastCall();
    expect(url).toBe('/api/links/4');
    expect(init.method?.toUpperCase()).toBe('DELETE');
    // the controller demands a JSON content type on every mutating request
    expect(header(init, 'content-type')).toMatch(/^application\/json/);
  });

  it.each([
    ['startEmulation', () => client.startEmulation(), '/api/emulation/start'],
    ['stopEmulation', () => client.stopEmulation(), '/api/emulation/stop'],
  ])('%s POSTs %s (202) with a JSON content type even without a body', async (_n, call, url) => {
    reply(202, fx.emulation('starting'));
    await expect(call()).resolves.toMatchObject({ state: 'starting' });
    const { url: u, init } = lastCall();
    expect(u).toBe(url);
    expect(init.method?.toUpperCase()).toBe('POST');
    expect(header(init, 'content-type')).toMatch(/^application\/json/);
  });

  it('rejects with ApiError carrying status and the message of the error body', async () => {
    reply(400, { message: 'delayer.std_ns must be >= 0' });
    const err = await client.createLink(fx.link()).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(client.ApiError);
    expect(err).toBeInstanceOf(Error);
    expect((err as InstanceType<Client['ApiError']>).status).toBe(400);
    expect((err as Error).message).toBe('delayer.std_ns must be >= 0');
  });

  it('falls back to "HTTP <status>" for non-JSON error bodies', async () => {
    reply(502, undefined, '<html>Bad gateway</html>');
    const err = await client.getStats().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(client.ApiError);
    expect((err as Error).message).toContain('502');
  });

  it.each([404, 409, 413, 415, 500])('maps status %i to ApiError', async (status) => {
    reply(status, { message: `m${status}` });
    const err = await client.getLink(1).catch((e: unknown) => e);
    expect((err as InstanceType<Client['ApiError']>).status).toBe(status);
    expect((err as Error).message).toBe(`m${status}`);
  });

  it('propagates network failures as errors', async () => {
    fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'));
    await expect(client.getOnline()).rejects.toThrow(/fetch/i);
  });
});
