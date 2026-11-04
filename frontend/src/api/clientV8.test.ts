// v8 additions to the REAL client module (histogram window) against a stubbed fetch; v22 removed the presets and export/import
// routes together with their client functions (see client.test.ts). Contract: see the "v8" block in src/test/helpers.tsx.
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
