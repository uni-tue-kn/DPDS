// Mock of the `../api/client` module (installed globally in setup.ts via vi.mock).
// Tests import `api` from here to program and inspect the calls, and `ApiError` to build rejections.
// The surface is exactly the real client's: a page that calls a function the client lacks fails with "is not a function".
import { vi } from 'vitest';
import type { Mock } from 'vitest';
import type {
  EmulationState,
  EmulationView,
  Hint,
  HistogramView,
  HistorySample,
  HistoryView,
  Interface,
  OnlineView,
  StartRequest,
  StatsView,
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
  /** GET /api/host/hints: host-only hints.  `{ recheck: true }` (`?recheck=true`) computes them again; the first load passes no argument. */
  getHostHints: vi.fn() as Mock<(opts?: { recheck?: boolean }) => Promise<Hint[]>>,
  /**
   * POST /api/host/hints with a start request: the hints that depend on the links.  `{ recheck: true }` as the second
   * argument asks the controller to read the NIC firmware again (`?recheck=true`); the first load passes no second argument.
   */
  getHostHintsFor: vi.fn() as Mock<(req: StartRequest, opts?: { recheck?: boolean }) => Promise<Hint[]>>,
  getEmulation: vi.fn() as Mock<() => Promise<EmulationView>>,
  startEmulation: vi.fn() as Mock<(req: StartRequest) => Promise<EmulationView>>,
  stopEmulation: vi.fn() as Mock<() => Promise<EmulationView>>,
  getStats: vi.fn() as Mock<() => Promise<StatsView>>,
  getDelayHistogram: vi.fn() as Mock<(dir: number, windowS?: number) => Promise<HistogramView>>,
  /** GET /api/stats/{dir}/iat_histogram, the departure-gap histogram (the delay histogram's shape, no `expected*`). */
  getIatHistogram: vi.fn() as Mock<(dir: number) => Promise<HistogramView>>,
  /** GET /api/stats/history, `sinceMs` absent = everything (0 is a real value: samples newer than 0 ms). */
  getStatsHistory: vi.fn() as Mock<(sinceMs?: number) => Promise<HistoryView>>,
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
  api.getIatHistogram.mockImplementation(async (dir) => fx.iatHistogram(dir));
  api.getStatsHistory.mockResolvedValue(fx.history({}, { started_at: null, run_id: null, links: [], totals: {} })); // nothing ran
}

/**
 * The controller's series as a test controls it. `getStatsHistory(sinceMs)` answers like `GET /api/stats/history`:
 * `run_id`, `started_at`, `links` and, per link index, the samples with `t_ms > sinceMs` (all without `sinceMs`); a known link
 * keeps its key with `[]` when nothing is newer. Change `server` between polls to simulate the run going on, a new run, and so on.
 *
 * A run is identified by `runId` (`run_id`), not by `startedAt`: a test that simulates a new run sets a new `runId`
 * (and, as the controller does, a new `startedAt` and `samples`). Without `runId` the run is 1 (null while `startedAt` is
 * null: nothing ran); without `links` they are `fx.historyLinks()` (empty while nothing ran).
 */
export interface HistoryServer {
  startedAt: number | null;
  samples: Record<string, HistorySample[]>;
  runId?: number | null;
  links?: HistoryView['links'];
  /** The run's final counters per link index; absent = `{}` (the answer of a controller that never ran). */
  totals?: HistoryView['totals'];
}

export function serveHistory(server: HistoryServer): void {
  api.getStatsHistory.mockImplementation(async (sinceMs) => {
    const ran = server.startedAt !== null;
    return {
      run_id: server.runId === undefined ? (ran ? 1 : null) : server.runId,
      started_at: server.startedAt,
      links: server.links ?? (ran ? fx.historyLinks() : []),
      totals: server.totals ?? {},
      samples: Object.fromEntries(
        Object.entries(server.samples).map(([link, list]) => [
          link,
          list.filter((s) => sinceMs === undefined || s.t_ms > sinceMs).map((s) => ({ ...s })),
        ]),
      ),
    };
  });
}

/**
 * The controller's start/stop state machine as the real one has it (not a mock that answers `stopped` at once).
 * `stop` answers 202 with `stopping` and the state stays `stopping` until the test sets `c.state` to `stopped` (or
 * `failed`), the way `dpds` finishes its drain; `start` is a 409 `ApiError` unless the state is `stopped` or `failed`
 * and otherwise answers `starting`. `getEmulation` answers the current state and remembers what it answered last, so a test
 * can tell what the client had seen when it sent a start.
 */
export interface FakeController {
  state: EmulationState;
  /** The state the latest `getEmulation` answer carried. */
  lastPolled: EmulationState | undefined;
  /** One entry per `startEmulation` call: the controller's state and the last polled state at that moment. */
  starts: { state: EmulationState; lastPolled: EmulationState | undefined }[];
}

export function fakeController(initial: EmulationState = 'running'): FakeController {
  const c: FakeController = { state: initial, lastPolled: undefined, starts: [] };
  api.getEmulation.mockImplementation(async () => {
    c.lastPolled = c.state;
    return fx.emulation(c.state);
  });
  api.stopEmulation.mockImplementation(async () => {
    c.state = 'stopping';
    return fx.emulation('stopping');
  });
  api.startEmulation.mockImplementation(async () => {
    c.starts.push({ state: c.state, lastPolled: c.lastPolled });
    if (c.state !== 'stopped' && c.state !== 'failed') {
      throw new ApiError(409, `cannot start while the emulation is ${c.state}`);
    }
    c.state = 'starting';
    return fx.emulation('starting');
  });
  return c;
}
