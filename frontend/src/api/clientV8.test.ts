// v8 additions to the REAL client module (presets, export/import, histogram window) against a stubbed fetch.
// Contract: see the "v8" block in src/test/helpers.tsx.
import { vi } from 'vitest';
import * as fx from '../test/fixtures';

type Client = typeof import('./client');
let client: Client;
const fetchMock = vi.fn();

function reply(status: number, body?: unknown) {
  fetchMock.mockResolvedValueOnce(
    new Response(body === undefined ? null : JSON.stringify(body), {
      status,
      headers: body === undefined ? {} : { 'content-type': 'application/json' },
    }),
  );
}

function lastCall(): { url: string; init: RequestInit } {
  const [url, init] = fetchMock.mock.calls[fetchMock.mock.calls.length - 1];
  return { url: String(url), init: (init ?? {}) as RequestInit };
}

const contentType = (init: RequestInit) => new Headers(init.headers).get('content-type');

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

describe('api client v8: presets', () => {
  it('getPresets GETs /api/presets', async () => {
    reply(200, [fx.preset()]);
    await expect(client.getPresets()).resolves.toEqual([fx.preset()]);
    expect(lastCall().url).toBe('/api/presets');
    expect((lastCall().init.method ?? 'GET').toUpperCase()).toBe('GET');
  });

  it('getPreset GETs /api/presets/{id}', async () => {
    reply(200, fx.preset({ id: 3 }));
    await expect(client.getPreset(3)).resolves.toMatchObject({ id: 3 });
    expect(lastCall().url).toBe('/api/presets/3');
  });

  it('createPreset POSTs the JSON body', async () => {
    const input = { name: 'wan', description: 'd', config: fx.config3() };
    reply(201, { id: 5, ...input });
    await expect(client.createPreset(input)).resolves.toMatchObject({ id: 5 });
    const { url, init } = lastCall();
    expect(url).toBe('/api/presets');
    expect(init.method?.toUpperCase()).toBe('POST');
    expect(contentType(init)).toMatch(/^application\/json/);
    expect(JSON.parse(String(init.body))).toEqual(input);
  });

  it('updatePreset PUTs /api/presets/{id}', async () => {
    const input = { name: 'wan2', config: fx.config() };
    reply(200, { id: 7, ...input });
    await client.updatePreset(7, input);
    const { url, init } = lastCall();
    expect(url).toBe('/api/presets/7');
    expect(init.method?.toUpperCase()).toBe('PUT');
    expect(contentType(init)).toMatch(/^application\/json/);
    expect(JSON.parse(String(init.body))).toEqual(input);
  });

  it('deletePreset DELETEs, resolves on 204 and sends a JSON content type', async () => {
    reply(204);
    await expect(client.deletePreset(4)).resolves.toBeUndefined();
    const { url, init } = lastCall();
    expect(url).toBe('/api/presets/4');
    expect(init.method?.toUpperCase()).toBe('DELETE');
    expect(contentType(init)).toMatch(/^application\/json/);
  });

  it('createPreset rejects with ApiError(409) carrying the message', async () => {
    reply(409, { message: 'preset name "wan" already exists' });
    const err = await client.createPreset({ name: 'wan', config: fx.config() }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(client.ApiError);
    expect((err as InstanceType<Client['ApiError']>).status).toBe(409);
    expect((err as Error).message).toBe('preset name "wan" already exists');
  });
});

describe('api client v8: export / import', () => {
  it('exportSetup GETs /api/export and returns the document', async () => {
    reply(200, fx.exportDoc());
    await expect(client.exportSetup()).resolves.toEqual(fx.exportDoc());
    expect(lastCall().url).toBe('/api/export');
    expect((lastCall().init.method ?? 'GET').toUpperCase()).toBe('GET');
  });

  it('importSetup POSTs the document untouched to /api/import?mode=replace', async () => {
    const doc = fx.exportDoc();
    reply(200, { links: 1, presets: 1 });
    await expect(client.importSetup(doc)).resolves.toEqual({ links: 1, presets: 1 });
    const { url, init } = lastCall();
    expect(url).toBe('/api/import?mode=replace');
    expect(init.method?.toUpperCase()).toBe('POST');
    expect(contentType(init)).toMatch(/^application\/json/);
    expect(JSON.parse(String(init.body))).toEqual(doc);
  });

  it.each([400, 409, 413])('importSetup maps status %i to ApiError with the message', async (status) => {
    reply(status, { message: `import failed ${status}` });
    const err = await client.importSetup(fx.exportDoc()).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(client.ApiError);
    expect((err as InstanceType<Client['ApiError']>).status).toBe(status);
    expect((err as Error).message).toBe(`import failed ${status}`);
  });
});

describe('api client v8: histogram window', () => {
  it('without a window the URL has no query string (unchanged)', async () => {
    reply(200, fx.histogram(2));
    await client.getDelayHistogram(2);
    expect(lastCall().url).toBe('/api/stats/2/delay_histogram');
  });

  it.each([10, 60, 300])('window %i s is sent as ?window_s=%i', async (n) => {
    reply(200, fx.histogram(1, { window_s: n }));
    await expect(client.getDelayHistogram(1, n)).resolves.toMatchObject({ window_s: n });
    expect(lastCall().url).toBe(`/api/stats/1/delay_histogram?window_s=${n}`);
  });
});
