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
    // spec 29
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

  // v22: whole configuration in the request body
  it('startEmulation POSTs the start request (links without ids, settings) as JSON to /api/emulation/start, untouched', async () => {
    const body = {
      links: [
        { name: 'a-b', ingress: '0000:65:00.0', egress: '0000:65:00.1', config: fx.config(), worker: null },
        { name: 'b-a', ingress: '0000:65:00.1', egress: '0000:65:00.0', config: fx.config2(), worker: 5 },
      ],
      settings: { main_lcore: null, eal_extra_args: ['--no-huge'], poll_interval_ms: 500 },
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
      links: [{ name: 'a-b', ingress: '0000:65:00.0', egress: '0000:65:00.1', config: fx.config(), worker: null }],
      settings: fx.settings(),
    };
    reply(200, fx.hints());
    await expect(client.getHostHintsFor(body)).resolves.toEqual(fx.hints());
    const { url, init } = lastCall();
    expect(url).toBe('/api/host/hints');
    expect(init.method?.toUpperCase()).toBe('POST');
    expect(header(init, 'content-type')).toMatch(/^application\/json/);
    expect(JSON.parse(String(init.body))).toEqual(body);
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
    ]) {
      expect(client, gone).not.toHaveProperty(gone);
    }
    expect(Object.keys(client).sort()).toEqual([
      'ApiError',
      'getDelayHistogram',
      'getEmulation',
      'getHostHints',
      'getHostHintsFor',
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
    const err = await client.startEmulation({ links: [], settings: fx.settings() }).catch((e: unknown) => e);
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
