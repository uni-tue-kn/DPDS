// Mock of the `../api/client` module (installed globally in setup.ts via vi.mock).
// Tests import `api` from here to program and inspect the calls, and `ApiError` to build rejections.
// v22: the surface is exactly the surviving REST client (no links / settings / export / import functions; presets are gone altogether): a page
// that still calls a removed function fails with "is not a function".
import { vi } from 'vitest';
import type { Mock } from 'vitest';
import type { EmulationView, Hint, HistogramView, Interface, OnlineView, StartRequest, StatsView } from '../api/types';
import * as fx from './fixtures';

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

export const api = {
  getOnline: vi.fn() as Mock<() => Promise<OnlineView>>,
  getInterfaces: vi.fn() as Mock<() => Promise<Interface[]>>,
  /** GET /api/host/hints: host-only hints. */
  getHostHints: vi.fn() as Mock<() => Promise<Hint[]>>,
  /** POST /api/host/hints with a start request: the hints that depend on the links. */
  getHostHintsFor: vi.fn() as Mock<(req: StartRequest) => Promise<Hint[]>>,
  getEmulation: vi.fn() as Mock<() => Promise<EmulationView>>,
  startEmulation: vi.fn() as Mock<(req: StartRequest) => Promise<EmulationView>>,
  stopEmulation: vi.fn() as Mock<() => Promise<EmulationView>>,
  getStats: vi.fn() as Mock<() => Promise<StatsView>>,
  getDelayHistogram: vi.fn() as Mock<(dir: number, windowS?: number) => Promise<HistogramView>>,
};

/** The module object the `../api/client` import resolves to in tests. */
export function clientMock() {
  return { ...api, ApiError };
}

/** Reset every mock to a benign default (stopped emulation, no stats). */
export function resetApi(): void {
  for (const fn of Object.values(api)) fn.mockReset();
  api.getOnline.mockResolvedValue(fx.online());
  api.getInterfaces.mockResolvedValue(fx.interfaces());
  api.getHostHints.mockResolvedValue(fx.hints());
  api.getHostHintsFor.mockResolvedValue([]);
  api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
  api.startEmulation.mockResolvedValue(fx.emulation('starting'));
  api.stopEmulation.mockResolvedValue(fx.emulation('stopping'));
  api.getStats.mockResolvedValue(fx.stats([]));
  api.getDelayHistogram.mockImplementation(async (dir, windowS) => fx.histogram(dir, { window_s: windowS ?? 0 }));
}
