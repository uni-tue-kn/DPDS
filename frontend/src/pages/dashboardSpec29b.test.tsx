import { screen, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistorySample } from '../api/types';
import { CHART_RADIOS, card, hasTab, radioNames, tab, tabBadge, tabs, tabTitle } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedStore, tick } from '../test/helpers';
import type { HistoryServer } from '../test/mockApi';
import { ApiError, api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// A run is identified by `run_id` of `GET /api/stats/history`:
//   - A changed `run_id` (also null -> n and n -> null) drops the data and fetches in full again; a changed `started_at` with the same
//     `run_id` changes nothing.
//   - History tabs (the final charts, no live stat list) show for `stopping`, `stopped` and `failed` whenever the history has samples.
//     They carry no state badge. Their titles come from the history's `links` (`name: ingress → egress`, ports labelled with the
//     netdev as everywhere), never from the stats of an earlier poll or the plan of the emulation; `Link <n>` only for an index
//     `links` has no entry for.
//   - While the state is `starting`, live tabs chart nothing: the history still held is the old run's.
//   - At most 86400 samples per link are kept, the oldest are dropped first.
//   - A failing history request shows a warning (`data-testid="history-warning"`, visible text naming the history and the error
//     message, separate from `stats-warning`) until the next success; the charts keep their data.

// A link's chart area is a tab (src/test/dashboardUi.tsx); the title is the tab's.
const dataOf = (el: HTMLElement) => JSON.parse(el.getAttribute('data-chart') ?? 'null') as ChartData;
interface ChartData {
  labels: string[];
  datasets: { label?: string; data: number[] }[];
}
const chartOf = (d: number) => dataOf(within(card(d)).getByTestId('chart-line'));
const rxOf = (d: number) =>
  must(
    chartOf(d).datasets.find((s) => /RX/.test(s.label ?? '')),
    'RX dataset',
  ).data;
const labelsOf = (d: number) => chartOf(d).labels;
const sinceArgs = () => api.getStatsHistory.mock.calls.map(([since]) => since);

const S1 = 1762000000;

function server(samples: Record<string, HistorySample[]> = {}, startedAt: number | null = S1): HistoryServer {
  const s: HistoryServer = { startedAt, samples };
  serveHistory(s);
  return s;
}

const STOPPED_STATS = fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 });
const quiet = () => vi.spyOn(console, 'error').mockImplementation(() => undefined);

async function open() {
  vi.useFakeTimers();
  const view = renderApp('/');
  await tick(0);
  await tick(0);
  return view;
}

beforeEach(() => {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
});

// ---- run identity is run_id --------------------------------------------------------------------------------------

describe('Dashboard (spec 29b item 4): a run is its run_id, not its started_at', () => {
  it('a changed started_at with the same run_id is not a new run: the data stays and the polls stay incremental', async () => {
    const s = server({
      '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    await open();
    // the controller reports another start time for the same run (display data only)
    s.startedAt = S1 + 900;
    s.samples['0'].push(fx.historySample(5000, { ...fx.NO_PPS, rx_gbps: 3.5 }));
    const calls = api.getStatsHistory.mock.calls.length;
    await tick(3000);
    expect(sinceArgs().slice(calls)).not.toContain(undefined); // no full request: nothing was dropped
    expect(rxOf(0)).toEqual([...Array(5).fill(1.5), 3.5]);
    expect(rxOf(1)).toEqual(Array(5).fill(2.5));
  });

  it('a changed run_id with the same started_at is a new run: the data is dropped and the new run loaded in full', async () => {
    const s = server({
      '0': fx.historySamples(0, 6, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 6, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    await open();
    expect(rxOf(0)).toHaveLength(6);
    // two runs within the same second: the start time cannot tell them apart, the id does
    s.runId = 2;
    s.samples = {
      '0': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 4.5 }),
      '1': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 5.5 }),
    };
    const calls = api.getStatsHistory.mock.calls.length;
    await tick(3000);
    expect(sinceArgs().slice(calls)).toContain(undefined);
    expect(rxOf(0)).toEqual([4.5, 4.5]);
    expect(rxOf(1)).toEqual([5.5, 5.5]);
  });

  it('a new run whose answer to the incremental request is empty is still loaded: the id changed', async () => {
    const s = server({ '0': fx.historySamples(0, 6, { ...fx.NO_PPS, rx_gbps: 1.5 }), '1': [] });
    await open();
    s.runId = 2;
    s.samples = { '0': [fx.historySample(0, { ...fx.NO_PPS, rx_gbps: 7.5 })], '1': [] }; // nothing newer than since_ms = 5000
    await tick(3000);
    expect(rxOf(0)).toEqual([7.5]);
  });

  it('afterwards the polls are incremental again from the new run, and its increments are appended', async () => {
    const s = server({ '0': fx.historySamples(0, 6), '1': fx.historySamples(0, 6) });
    await open();
    s.runId = 2;
    s.samples = { '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) };
    await tick(3000);
    const calls = api.getStatsHistory.mock.calls.length;
    await tick(3000);
    const later = sinceArgs().slice(calls);
    expect(later.length).toBeGreaterThanOrEqual(2);
    expect(later.every((a) => a === 1000)).toBe(true);
    s.samples['0'].push(fx.historySample(2000, { ...fx.NO_PPS, rx_gbps: 8.5 }));
    await tick(2000);
    expect(rxOf(0).at(-1)).toBe(8.5);
    expect(rxOf(0)).toHaveLength(3);
  });

  it('a controller that restarted (run_id null again, nothing ran) drops the old data and its cards', async () => {
    const s = server({ '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) });
    await open();
    expect(rxOf(0)).toHaveLength(5);
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(STOPPED_STATS);
    await tick(2500);
    expect(hasTab(0)).toBe(true); // the stopped run's charts
    s.runId = null;
    s.startedAt = null;
    s.samples = {};
    s.links = [];
    await tick(3000);
    expect(tabs()).toHaveLength(0);
  });

  it('with an unchanged run_id the data is never refetched in full, however long the dashboard stays open', async () => {
    server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) });
    await open();
    await tick(10000);
    expect(sinceArgs().filter((a) => a === undefined)).toHaveLength(1);
  });
});

// ---- cards for stopping, stopped and failed; titles from the history's links ---------------------------------------

describe('Dashboard (spec 29b item 5): history cards for stopping, stopped and failed, titled from the history', () => {
  const settle = (state: 'stopping' | 'stopped' | 'failed') => {
    api.getEmulation.mockResolvedValue(
      fx.emulation(state, { directions: [], error: state === 'failed' ? 'dpds exited (status 1)' : null }),
    );
    api.getStats.mockResolvedValue(STOPPED_STATS);
  };
  const BADGE = { stopping: 'Stopping', stopped: 'Stopped', failed: 'Failed' } as const;

  it.each(['stopping', 'stopped', 'failed'] as const)(
    '"%s": a card per link with the final charts and no state badge (spec 54 L1)',
    async (state) => {
      server({
        '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
        '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
      });
      settle(state);
      await open();
      await tick(2500);
      for (const d of [0, 1]) {
        expect(tabBadge(d), `${state}: tab ${d}`).toBe('');
      }
      expect(rxOf(0)).toEqual(Array(5).fill(1.5));
      expect(rxOf(1)).toEqual(Array(5).fill(2.5));
      // no stored config says delay_stats is off, so the histogram stays
      expect(radioNames(0)).toEqual(CHART_RADIOS);
    },
  );

  it.each(['stopping', 'stopped', 'failed'] as const)(
    '"%s": a link whose stored config has delay_stats off has the final charts only, the other one keeps its histogram (spec 50 A3)',
    async (state) => {
      seedStore({
        links: [
          fx.link({ id: 1, name: 'a-b' }),
          fx.link({
            id: 2,
            name: 'b-a',
            ingress: '0000:65:00.1',
            egress: '0000:65:00.0',
            config: { ...fx.config(), delay_stats: false },
          }),
        ],
      });
      server({
        '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
        '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
      });
      settle(state);
      api.getDelayHistogram.mockImplementation(async (dir) => {
        if (dir === 1) throw new ApiError(404, 'delay statistics are off for direction 1');
        return fx.histogram(dir);
      });
      await open();
      await tick(2500);
      expect(radioNames(0)).toEqual(CHART_RADIOS);
      expect(radioNames(1)).toEqual(['Traffic rates', 'Packet loss']);
    },
  );

  it.each(['stopping', 'stopped', 'failed'] as const)('"%s" with an empty series: no cards', async (state) => {
    server({ '0': [], '1': [] });
    settle(state);
    await open();
    await tick(2500);
    expect(tabs()).toHaveLength(0);
  });

  it.each(['stopping', 'stopped', 'failed'] as const)('"%s", the controller never ran: no cards', async (state) => {
    server({}, null);
    settle(state);
    await open();
    await tick(2500);
    expect(tabs()).toHaveLength(0);
    expect(screen.queryByText(BADGE[state], { selector: '.badge' })).not.toBeInTheDocument();
  });

  it('a run that goes from running through stopping to stopped keeps its cards the whole way, never badged (spec 54 L1)', async () => {
    server({ '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) });
    await open();
    expect(tabBadge(0)).toBe(''); // live: no state badge
    settle('stopping');
    await tick(2500);
    expect(tabBadge(0)).toBe('');
    settle('stopped');
    await tick(2500);
    expect(tabBadge(0)).toBe('');
    expect(rxOf(0)).toHaveLength(5);
  });

  it('a failed run has no badge either, and the charts stay', async () => {
    server({ '0': fx.historySamples(0, 4), '1': fx.historySamples(0, 4) });
    settle('stopping');
    await open();
    await tick(2500);
    settle('failed');
    await tick(2500);
    expect(tabBadge(1)).toBe('');
    expect(rxOf(1)).toHaveLength(4);
  });

  it('titles are "name: ingress → egress" from the history links, with netdev names (the plan and the stats are not used)', async () => {
    // the emulation still carries a plan of other names, as an inconsistent payload could
    api.getEmulation.mockResolvedValue(fx.emulation('stopped')); // directions: plan(), names a-b and b-a
    api.getStats.mockResolvedValue(STOPPED_STATS);
    const s = server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) });
    s.links = [
      { name: 'uplink', ingress: '0000:65:00.0', egress: '0000:65:00.1' },
      { name: 'loop', ingress: 'net_null0', egress: 'net_null0' },
    ];
    await open();
    await tick(2500);
    expect(tabTitle(0)).toBe('uplink: enp101s0f0 → enp101s0f1');
    expect(tabTitle(1)).toBe('loop: net_null0 → net_null0');
    expect(tab(0)).not.toHaveTextContent('a-b');
    expect(tab(0)).not.toHaveTextContent('0000:65:00'); // no PCI address next to a netdev
  });

  it('a freshly opened page of a stopped controller titles its cards from the links, not "Link 0"', async () => {
    server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(STOPPED_STATS);
    await open();
    await tick(2500);
    expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1');
    expect(tabTitle(1)).toBe('b-a: enp101s0f1 → enp101s0f0');
    expect(screen.queryByText(/^Link \d+$/)).not.toBeInTheDocument();
  });

  it('the title never comes from the stats of an earlier poll: another run is shown under its own link names', async () => {
    const s = server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) });
    s.links = [
      { name: 'alpha', ingress: 'net_null0', egress: 'net_null1' },
      { name: 'beta', ingress: 'net_null1', egress: 'net_null0' },
    ];
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, { link: 'alpha', rx: 'net_null0', tx: 'net_null1' }),
        fx.dirStats(1, { link: 'beta', rx: 'net_null1', tx: 'net_null0' }),
      ]),
    );
    await open();
    expect(tabTitle(0)).toBe('alpha: net_null0 → net_null1'); // live, from the stats
    // meanwhile another run, with other links, ran and ended (started from another browser): this page only sees it stopped
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(STOPPED_STATS);
    s.runId = 2;
    s.startedAt = S1 + 900;
    s.links = [
      { name: 'gamma', ingress: 'net_null2', egress: 'net_null3' },
      { name: 'delta', ingress: 'net_null3', egress: 'net_null2' },
    ];
    s.samples = { '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) };
    await tick(3000);
    expect(tabTitle(0)).toBe('gamma: net_null2 → net_null3');
    expect(tabTitle(1)).toBe('delta: net_null3 → net_null2');
    expect(document.body).not.toHaveTextContent(/alpha|beta/);
  });

  it('"Link <n>" only for an index the links have no entry for', async () => {
    const s = server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3), '2': fx.historySamples(0, 3) });
    s.links = [{ name: 'only', ingress: 'net_null0', egress: 'net_null1' }];
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(STOPPED_STATS);
    await open();
    await tick(2500);
    expect(tabTitle(0)).toBe('only: net_null0 → net_null1');
    expect(tabTitle(1)).toBe('Link 1');
    expect(tabTitle(2)).toBe('Link 2');
    expect(screen.queryByText('Link 0')).not.toBeInTheDocument();
  });

  it('the links of the history come in link-index order: entry i names the card of index i, whatever the key order of the samples', async () => {
    const s = server({ '1': fx.historySamples(0, 2), '0': fx.historySamples(0, 2) });
    s.links = [
      { name: 'first', ingress: 'net_null0', egress: 'net_null1' },
      { name: 'second', ingress: 'net_null1', egress: 'net_null0' },
    ];
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(STOPPED_STATS);
    await open();
    await tick(2500);
    expect(tabTitle(0)).toBe('first: net_null0 → net_null1');
    expect(tabTitle(1)).toBe('second: net_null1 → net_null0');
  });

  it('a stopped run keeps its title when the dashboard is reopened after the run was replaced', async () => {
    // the links are refetched with each full request, so the titles follow a new run at once
    const s = server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(STOPPED_STATS);
    const first = await open();
    await tick(1500);
    first.unmount();
    s.runId = 2;
    s.links = [
      { name: 'again0', ingress: 'net_null0', egress: 'net_null1' },
      { name: 'again1', ingress: 'net_null1', egress: 'net_null0' },
    ];
    renderApp('/');
    await tick(0);
    await tick(1500);
    expect(tabTitle(0)).toBe('again0: net_null0 → net_null1');
  });
});

// ---- no old-run points on the cards of a starting run -----------------------------------------------------------

describe('Dashboard (spec 29b item 6): while the emulation is starting, live cards get no history', () => {
  it('live cards of a starting emulation chart nothing although the history still holds the old run', async () => {
    server({
      '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    await open();
    await tick(2500);
    for (const d of [0, 1]) {
      expect(radioNames(d)).toContain('Delay histogram'); // a live card (an ended one has no histogram choice)
      expect(rxOf(d), `card ${d}`).toEqual([]);
      expect(labelsOf(d), `card ${d}`).toEqual([]);
    }
  });

  it('once the run is running and the controller serves the new run, its own samples are charted, none of the old ones', async () => {
    const s = server({
      '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    await open();
    await tick(2500);
    s.runId = 2;
    s.startedAt = S1 + 900;
    s.samples = {
      '0': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 4.5 }),
      '1': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 5.5 }),
    };
    api.getEmulation.mockResolvedValue(fx.emulation('running', { started_at: S1 + 900 }));
    await tick(3000);
    expect(rxOf(0)).toEqual([4.5, 4.5]);
    expect(rxOf(1)).toEqual([5.5, 5.5]);
  });

  it('a running emulation charts the history as before (control)', async () => {
    server({
      '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    await open();
    expect(rxOf(0)).toEqual(Array(5).fill(1.5));
    expect(rxOf(1)).toEqual(Array(5).fill(2.5));
  });

  it('the history is still polled while starting (the run being started is loaded as soon as it exists)', async () => {
    server({ '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) });
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    await open();
    const n = api.getStatsHistory.mock.calls.length;
    await tick(4000);
    expect(api.getStatsHistory.mock.calls.length - n).toBeGreaterThanOrEqual(3);
  });
});

// ---- at most 86400 samples per link ---------------------------------------------------------------------------

describe('Dashboard (spec 29b item 7): the client keeps at most 86400 samples per link, dropping the oldest', () => {
  /** `count` samples a second apart from t = `from` seconds, `rx_gbps` = the sample's own number of seconds. */
  const numbered = (from: number, count: number): HistorySample[] =>
    Array.from({ length: count }, (_, i) => fx.historySample((from + i) * 1000, { ...fx.NO_PPS, rx_gbps: from + i }));

  // A time chart plots at most 100 points, the first and the last of the history the page holds. So the cap of 86400
  // samples is seen at the chart's FIRST point (the oldest sample the page still holds) and its last one (the newest).
  it('a series at the cap that grows by 3 keeps 86400 samples: the 3 oldest are gone', async () => {
    const s = server({ '0': numbered(0, 86400), '1': fx.historySamples(0, 10) });
    await open();
    expect(rxOf(0)).toHaveLength(100);
    expect(rxOf(0)[0]).toBe(0);
    s.samples['0'].push(...numbered(86400, 3));
    await tick(2000);
    const rx = rxOf(0);
    expect(rx).toHaveLength(100);
    expect(rx[0]).toBe(3);
    expect(rx.at(-1)).toBe(86402);
    expect(labelsOf(0)).toHaveLength(100);
    expect(labelsOf(0)[0]).toBe('0h 0m 3s');
    expect(labelsOf(0).at(-1)).toBe('24h 0m 2s');
  });

  it('the cap is per link: a short link is untouched', async () => {
    const s = server({ '0': numbered(0, 86400), '1': fx.historySamples(0, 10) });
    await open();
    s.samples['0'].push(...numbered(86400, 1));
    s.samples['1'].push(fx.historySample(86_400_000));
    await tick(2000);
    expect(rxOf(0)).toHaveLength(100);
    expect(rxOf(0)[0]).toBe(1); // the one oldest sample is gone (seen at the first point)
    expect(rxOf(0).at(-1)).toBe(86400);
    expect(rxOf(1)).toHaveLength(11);
  });

  it('crossing the cap over several increments drops exactly the excess, oldest first', async () => {
    const s = server({ '0': numbered(0, 86398), '1': [] });
    await open();
    s.samples['0'].push(...numbered(86398, 2)); // exactly at the cap
    await tick(2000);
    expect(rxOf(0)).toHaveLength(100);
    expect(rxOf(0)[0]).toBe(0);
    expect(rxOf(0).at(-1)).toBe(86399);
    s.samples['0'].push(...numbered(86400, 3));
    await tick(2000);
    expect(rxOf(0)).toHaveLength(100);
    expect(rxOf(0)[0]).toBe(3);
    expect(rxOf(0).at(-1)).toBe(86402);
  });

  it('below the cap nothing is dropped', async () => {
    const s = server({ '0': numbered(0, 86390), '1': [] });
    await open();
    s.samples['0'].push(...numbered(86390, 5));
    await tick(2000);
    expect(rxOf(0)).toHaveLength(100);
    expect(rxOf(0)[0]).toBe(0); // nothing dropped: the oldest sample is still the first point
    expect(rxOf(0).at(-1)).toBe(86394);
  });

  it('polling stays incremental from the newest sample after the cap was reached', async () => {
    const s = server({ '0': numbered(0, 86400), '1': [] });
    await open();
    s.samples['0'].push(...numbered(86400, 2));
    await tick(2000);
    await tick(2000);
    expect(sinceArgs().at(-1)).toBe(86401 * 1000);
    expect(sinceArgs().filter((a) => a === undefined)).toHaveLength(1);
  });
});

// ---- a failing history request is visible ---------------------------------------------------------------------

describe('Dashboard (spec 29b item 8): a failing history request shows a warning until the next success', () => {
  const warning = () => screen.queryByTestId('history-warning');

  it('the first request fails: the warning names the history and the error, the rest of the page works', async () => {
    const errors = quiet();
    api.getStatsHistory.mockRejectedValue(new ApiError(500, 'history read failed'));
    await open();
    const w = must(warning(), 'history warning');
    expect(w).toHaveTextContent(/history/i);
    expect(w).toHaveTextContent('history read failed');
    expect(hasTab(0)).toBe(true);
    errors.mockRestore();
  });

  it('a failure after data keeps the charts, and the warning stays for as long as the requests fail', async () => {
    const errors = quiet();
    const s = server({ '0': fx.historySamples(0, 4), '1': fx.historySamples(0, 4) });
    await open();
    expect(warning()).not.toBeInTheDocument();
    const ok = api.getStatsHistory.getMockImplementation();
    api.getStatsHistory.mockRejectedValue(new ApiError(502, 'controller unreachable'));
    await tick(1500);
    expect(must(warning(), 'history warning')).toHaveTextContent('controller unreachable');
    await tick(5000);
    expect(warning()).toBeInTheDocument(); // still failing: still visible
    expect(rxOf(0)).toHaveLength(4); // the charts keep their data
    expect(rxOf(1)).toHaveLength(4);
    // the next success removes it and the series goes on
    s.samples['0'].push(fx.historySample(4000, { ...fx.NO_PPS, rx_gbps: 6.5 }));
    api.getStatsHistory.mockImplementation(must(ok, 'the mock implementation'));
    await tick(2500);
    expect(warning()).not.toBeInTheDocument();
    expect(rxOf(0)).toHaveLength(5);
    expect(rxOf(0).at(-1)).toBe(6.5);
    errors.mockRestore();
  });

  it('one failed request between good ones shows the warning only until the next one succeeds', async () => {
    const errors = quiet();
    server({ '0': fx.historySamples(0, 4), '1': fx.historySamples(0, 4) });
    await open();
    const ok = must(api.getStatsHistory.getMockImplementation(), 'the mock implementation');
    api.getStatsHistory.mockRejectedValueOnce(new Error('blip'));
    await tick(1000);
    expect(warning()).toBeInTheDocument();
    await tick(2000);
    expect(warning()).not.toBeInTheDocument();
    expect(api.getStatsHistory.getMockImplementation()).toBe(ok);
    errors.mockRestore();
  });

  it('there is no warning while the requests succeed', async () => {
    server({ '0': fx.historySamples(0, 4), '1': fx.historySamples(0, 4) });
    await open();
    await tick(5000);
    expect(warning()).not.toBeInTheDocument();
  });

  it('it is not the stats warning: a failing history leaves stats-warning out, a failing stats poll leaves history-warning out', async () => {
    const errors = quiet();
    api.getStatsHistory.mockRejectedValue(new Error('history down'));
    await open();
    expect(warning()).toBeInTheDocument();
    expect(screen.queryByTestId('stats-warning')).not.toBeInTheDocument();
    errors.mockRestore();
  });

  it('a failing stats poll does not show the history warning', async () => {
    const errors = quiet();
    server({ '0': fx.historySamples(0, 4), '1': fx.historySamples(0, 4) });
    api.getStats.mockRejectedValue(new Error('stats down'));
    await open();
    expect(screen.getByTestId('stats-warning')).toHaveTextContent('stats down');
    expect(warning()).not.toBeInTheDocument();
    errors.mockRestore();
  });

  it('the warning is also shown for a stopped run whose final charts are on screen', async () => {
    const errors = quiet();
    server({ '0': fx.historySamples(0, 4), '1': fx.historySamples(0, 4) });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(STOPPED_STATS);
    await open();
    await tick(1500);
    api.getStatsHistory.mockRejectedValue(new Error('gone'));
    await tick(2500);
    expect(warning()).toBeInTheDocument();
    expect(rxOf(0)).toHaveLength(4);
    errors.mockRestore();
  });

  it('a request that never settles shows no warning (it is slow, not failed)', async () => {
    api.getStatsHistory.mockImplementation(() => new Promise(() => undefined));
    await open();
    await tick(10000);
    expect(warning()).not.toBeInTheDocument();
  });

  it('after leaving the dashboard and coming back there is no warning left from before', async () => {
    const errors = quiet();
    api.getStatsHistory.mockRejectedValue(new Error('down'));
    const first = await open();
    expect(warning()).toBeInTheDocument();
    first.unmount();
    server({ '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) });
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(warning()).not.toBeInTheDocument();
    errors.mockRestore();
  });
});
