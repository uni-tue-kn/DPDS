// Mock of the `../api/client` module (installed globally in setup.ts via vi.mock).
// Tests import `api` from here to program and inspect the calls, and `ApiError` to build rejections.
import { vi } from 'vitest';
import type { Mock } from 'vitest';
import type {
  EmulationView, Hint, HistogramView, Interface, Link, LinkInput, OnlineView, Settings, StatsView,
} from '../api/types';
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
  getHostHints: vi.fn() as Mock<() => Promise<Hint[]>>,
  getLinks: vi.fn() as Mock<() => Promise<Link[]>>,
  getLink: vi.fn() as Mock<(id: number) => Promise<Link>>,
  createLink: vi.fn() as Mock<(input: LinkInput) => Promise<Link>>,
  updateLink: vi.fn() as Mock<(id: number, input: LinkInput) => Promise<Link>>,
  deleteLink: vi.fn() as Mock<(id: number) => Promise<void>>,
  getSettings: vi.fn() as Mock<() => Promise<Settings>>,
  updateSettings: vi.fn() as Mock<(s: Settings) => Promise<Settings>>,
  getEmulation: vi.fn() as Mock<() => Promise<EmulationView>>,
  startEmulation: vi.fn() as Mock<() => Promise<EmulationView>>,
  stopEmulation: vi.fn() as Mock<() => Promise<EmulationView>>,
  getStats: vi.fn() as Mock<() => Promise<StatsView>>,
  getDelayHistogram: vi.fn() as Mock<(dir: number) => Promise<HistogramView>>,
};

/** The module object the `../api/client` import resolves to in tests. */
export function clientMock() {
  return { ...api, ApiError };
}

/** Reset every mock to a benign default (stopped emulation, one link, no stats). */
export function resetApi(): void {
  for (const fn of Object.values(api)) fn.mockReset();
  api.getOnline.mockResolvedValue(fx.online());
  api.getInterfaces.mockResolvedValue(fx.interfaces());
  api.getHostHints.mockResolvedValue(fx.hints());
  api.getLinks.mockResolvedValue([fx.link()]);
  api.getLink.mockImplementation(async () => fx.link());
  api.createLink.mockImplementation(async (input) => ({ id: 99, ...input }));
  api.updateLink.mockImplementation(async (id, input) => ({ id, ...input }));
  api.deleteLink.mockResolvedValue(undefined);
  api.getSettings.mockResolvedValue(fx.settings());
  api.updateSettings.mockImplementation(async (s) => s);
  api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
  api.startEmulation.mockResolvedValue(fx.emulation('starting'));
  api.stopEmulation.mockResolvedValue(fx.emulation('stopping'));
  api.getStats.mockResolvedValue(fx.stats([]));
  api.getDelayHistogram.mockImplementation(async (dir) => fx.histogram(dir));
}
