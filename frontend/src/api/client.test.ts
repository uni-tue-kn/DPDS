// Tests the REAL client module (setup.ts mocks it for everything else) against a stubbed fetch.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
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
    ['getEmulation', () => client.getEmulation(), '/api/emulation'],
    ['getStats', () => client.getStats(), '/api/stats'],
    ['getDelayHistogram', () => client.getDelayHistogram(2), '/api/stats/2/delay_histogram'],
    ['getIatHistogram', () => client.getIatHistogram(2), '/api/stats/2/iat_histogram'],
    ['getStatsHistory', () => client.getStatsHistory(), '/api/stats/history'],
    ['getStatsHistory with since_ms', () => client.getStatsHistory(12500), '/api/stats/history?since_ms=12500'],
    // 0 is a real time here (samples newer than 0 s), unlike the histogram window where 0 means absent
    ['getStatsHistory since 0', () => client.getStatsHistory(0), '/api/stats/history?since_ms=0'],
    [
      'getStatsHistory since a whole number',
      () => client.getStatsHistory(192000),
      '/api/stats/history?since_ms=192000',
    ],
  ])('%s GETs %s and returns the parsed body', async (_name, call, url) => {
    reply(200, { ok: 1 });
    await expect(call()).resolves.toEqual({ ok: 1 });
    expect(lastCall().url).toBe(url);
    expect((lastCall().init.method ?? 'GET').toUpperCase()).toBe('GET');
  });

  // whole configuration in the request body
  it('startEmulation POSTs the start request (`{links}` without ids, no settings, no worker) as JSON to /api/emulation/start, untouched', async () => {
    const body = {
      links: [
        { ingress: '0000:65:00.0', egress: '0000:65:00.1', config: fx.config() },
        { ingress: '0000:65:00.1', egress: '0000:65:00.0', config: fx.config2() },
      ],
    };
    reply(202, fx.emulation('starting'));
    await expect(client.startEmulation(body)).resolves.toMatchObject({ state: 'starting' });
    const { url, init } = lastCall();
    expect(url).toBe('/api/emulation/start');
    expect(init.method?.toUpperCase()).toBe('POST');
    expect(header(init, 'content-type')).toMatch(/^application\/json/);
    expect(JSON.parse(String(init.body))).toEqual(body);
  });

  it('getHostHintsFor POSTs the start request as JSON to /api/host/hints and returns the hints', async () => {
    const body = {
      links: [{ ingress: '0000:65:00.0', egress: '0000:65:00.1', config: fx.config() }],
    };
    reply(200, fx.hints());
    await expect(client.getHostHintsFor(body)).resolves.toEqual(fx.hints());
    const { url, init } = lastCall();
    expect(url).toBe('/api/host/hints');
    expect(init.method?.toUpperCase()).toBe('POST');
    expect(header(init, 'content-type')).toMatch(/^application\/json/);
    expect(JSON.parse(String(init.body))).toEqual(body);
  });

  // `getHostHintsFor(req, { recheck: true })` makes the controller read the NIC firmware again; the body is unchanged.
  describe('getHostHintsFor recheck (spec 34)', () => {
    const body = () => ({
      links: [{ ingress: '0000:65:00.0', egress: '0000:65:00.1', config: fx.config() }],
    });

    it('{ recheck: true } POSTs /api/host/hints?recheck=true with the same JSON body and returns the hints', async () => {
      reply(200, fx.hints());
      await expect(client.getHostHintsFor(body(), { recheck: true })).resolves.toEqual(fx.hints());
      const { url, init } = lastCall();
      expect(url).toBe('/api/host/hints?recheck=true');
      expect(init.method?.toUpperCase()).toBe('POST');
      expect(header(init, 'content-type')).toMatch(/^application\/json/);
      expect(JSON.parse(String(init.body))).toEqual(body());
    });

    it.each([
      ['no second argument', () => client.getHostHintsFor(body())],
      ['an empty options object', () => client.getHostHintsFor(body(), {})],
      ['{ recheck: false }', () => client.getHostHintsFor(body(), { recheck: false })],
      ['{ recheck: undefined }', () => client.getHostHintsFor(body(), { recheck: undefined })],
    ])('%s sends no query at all (the first load never asks for a recheck)', async (_name, call) => {
      reply(200, []);
      await call();
      expect(lastCall().url).toBe('/api/host/hints');
    });

    // the GET answers the controller's stored result; `{ recheck: true }` computes it again.
    it('getHostHints({ recheck: true }) is a body-less GET of /api/host/hints?recheck=true', async () => {
      reply(200, fx.hints());
      await expect(client.getHostHints({ recheck: true })).resolves.toEqual(fx.hints());
      const { url, init } = lastCall();
      expect(url).toBe('/api/host/hints?recheck=true');
      expect((init.method ?? 'GET').toUpperCase()).toBe('GET');
      expect(init.body).toBeUndefined();
    });

    it.each([
      ['no argument', () => client.getHostHints()],
      ['an empty options object', () => client.getHostHints({})],
      ['{ recheck: false }', () => client.getHostHints({ recheck: false })],
      ['{ recheck: undefined }', () => client.getHostHints({ recheck: undefined })],
    ])('getHostHints with %s sends no query at all', async (_name, call) => {
      reply(200, []);
      await call();
      expect(lastCall().url).toBe('/api/host/hints');
    });

    it('a recheck that fails rejects with the ApiError of the controller (400 naming recheck)', async () => {
      reply(400, { message: 'query parameter recheck must be true or false' });
      const err = await client.getHostHintsFor(body(), { recheck: true }).catch((e: unknown) => e);
      expect(err).toBeInstanceOf(client.ApiError);
      expect((err as InstanceType<Client['ApiError']>).status).toBe(400);
      expect((err as Error).message).toContain('recheck');
    });
  });

  it('getHostHints stays a body-less GET of /api/host/hints', async () => {
    reply(200, []);
    await client.getHostHints();
    expect(lastCall().url).toBe('/api/host/hints');
    expect((lastCall().init.method ?? 'GET').toUpperCase()).toBe('GET');
    expect(lastCall().init.body).toBeUndefined();
  });

  it('the removed REST surface is gone: no function for links, presets, settings, export or import', () => {
    for (const gone of [
      'getLinks',
      'getLink',
      'createLink',
      'updateLink',
      'deleteLink',
      'getSettings',
      'updateSettings',
      'getPresets',
      'getPreset',
      'createPreset',
      'updatePreset',
      'deletePreset',
      'exportSetup',
      'importSetup',
      'applyHostAction', // DPDS never changes the host, hints only suggest commands
    ]) {
      expect(client, gone).not.toHaveProperty(gone);
    }
    expect(Object.keys(client).sort()).toEqual([
      'ApiError',
      'getDelayHistogram',
      'getEmulation',
      'getHostHints',
      'getHostHintsFor',
      'getIatHistogram',
      'getInterfaces',
      'getOnline',
      'getStats',
      'getStatsHistory',
      'startEmulation',
      'stopEmulation',
    ]);
  });

  it('no request of the client source targets a removed route (a source check on top of the export list)', () => {
    const src = readFileSync(join(process.cwd(), 'src/api/client.ts'), 'utf8');
    const removed = /request(?:<[^(]*>)?\(\s*['"][A-Z]+['"]\s*,\s*[`'"]\/(links|presets|settings|export|import)\b/;
    expect(src).not.toMatch(removed);
  });

  it('stopEmulation POSTs /api/emulation/stop (202) with a JSON content type even without a body', async () => {
    reply(202, fx.emulation('stopping'));
    await expect(client.stopEmulation()).resolves.toMatchObject({ state: 'stopping' });
    const { url, init } = lastCall();
    expect(url).toBe('/api/emulation/stop');
    expect(init.method?.toUpperCase()).toBe('POST');
    expect(header(init, 'content-type')).toMatch(/^application\/json/);
    expect(init.body).toBeUndefined();
  });

  it('rejects with ApiError carrying status and the message of the error body', async () => {
    reply(400, { message: 'delayer.std_ns must be >= 0' });
    const err = await client.startEmulation({ links: [] }).catch((e: unknown) => e);
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
    const err = await client.getEmulation().catch((e: unknown) => e);
    expect((err as InstanceType<Client['ApiError']>).status).toBe(status);
    expect((err as Error).message).toBe(`m${status}`);
  });

  it('propagates network failures as errors', async () => {
    fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'));
    await expect(client.getOnline()).rejects.toThrow(/fetch/i);
  });
});
