import { fireEvent, screen, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistorySample, HistoryView } from '../api/types';
import {
  CAUSES,
  card,
  delayBar,
  expectState,
  hasTab,
  queryRadio,
  summaryTab,
  tab,
  tabBadge,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedStore, tick } from '../test/helpers';
import type { HistoryServer } from '../test/mockApi';
import { api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// The dashboard charts are built from the controller's series, `GET /api/stats/history[?since_ms=<t>]`, not from a client-side
// buffer of stats polls:
//   - one full fetch (no argument) on mount, then once per poll an incremental one with since_ms = the newest t_ms received so
//     far (over all links); while nothing has been received yet, the fetches stay full;
//   - a changed `run_id` is a new run: the data held is dropped and the history is fetched in full again, because the new run's
//     t_ms start at 0 again and an incremental answer would be empty. `started_at` is display data and decides nothing;
//   - the card of link d (its `dir`) charts samples[String(d)]: rate = rx_gbps / tx_gbps as they are, drops = the nine cumulative
//     counts; x labels = secondsToTime(floor(t_ms / 1000)), e.g. t_ms 192000 -> "0h 3m 12s".

interface Dataset {
  label?: string;
  data: number[];
}
interface ChartData {
  labels: string[];
  datasets: Dataset[];
}

const dataOf = (el: HTMLElement): ChartData => JSON.parse(el.getAttribute('data-chart') ?? 'null');
const chartOf = (d: number) => dataOf(within(card(d)).getByTestId('chart-line'));
const dataset = (c: ChartData, re: RegExp) =>
  must(
    c.datasets.find((s) => re.test(s.label ?? '')),
    `${re} dataset`,
  );
const rxOf = (d: number) => dataset(chartOf(d), /RX/).data;
const labelsOf = (d: number) => chartOf(d).labels;
const sinceArgs = () => api.getStatsHistory.mock.calls.map(([since]) => since);

const S1 = 1762000000;

function server(samples: Record<string, HistorySample[]> = {}, startedAt: number | null = S1): HistoryServer {
  const s: HistoryServer = { startedAt, samples };
  serveHistory(s);
  return s;
}

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

describe('Dashboard (spec 29): the history is loaded once, then incrementally', () => {
  it('the first request, on mount, is a full one (no since_ms)', async () => {
    server({ '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) });
    await open();
    expect(api.getStatsHistory).toHaveBeenCalled();
    expect(sinceArgs()[0]).toBeUndefined();
  });

  it('it is requested on mount even while nothing runs', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([]));
    server();
    await open();
    expect(api.getStatsHistory.mock.calls.length).toBeGreaterThanOrEqual(1);
  });

  it('later requests carry since_ms = the newest t_ms received, over all links', async () => {
    const s = server({ '0': fx.historySamples(0, 6), '1': fx.historySamples(0, 5) }); // newest: 5 (link 0)
    await open();
    await tick(2500);
    const later = sinceArgs().slice(1);
    expect(later.length).toBeGreaterThanOrEqual(2);
    expect(later.every((a) => a === 5000)).toBe(true);
    // the run goes on: the next increment moves since_ms on
    s.samples['0'].push(fx.historySample(6040), fx.historySample(7080));
    s.samples['1'].push(fx.historySample(6040));
    await tick(1000);
    await tick(1000);
    expect(sinceArgs().at(-1)).toBe(7080);
  });

  it('there is exactly one full request over a long time', async () => {
    server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) });
    await open();
    await tick(10000);
    expect(sinceArgs().filter((a) => a === undefined)).toHaveLength(1);
    expect(sinceArgs().length).toBeGreaterThanOrEqual(8);
  });

  it('while no sample has arrived yet every request stays a full one', async () => {
    server({});
    await open();
    await tick(3500);
    expect(api.getStatsHistory.mock.calls.length).toBeGreaterThanOrEqual(3);
    expect(sinceArgs().every((a) => a === undefined)).toBe(true);
  });

  it('is polled about once per second, like the stats', async () => {
    server({ '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) });
    await open();
    const n = api.getStatsHistory.mock.calls.length;
    await tick(5000);
    const polls = api.getStatsHistory.mock.calls.length - n;
    expect(polls).toBeGreaterThanOrEqual(3);
    expect(polls).toBeLessThanOrEqual(8);
  });

  it('a request that never settles is not followed by a second one', async () => {
    const calls: number[] = [];
    api.getStatsHistory.mockImplementation(() => {
      calls.push(calls.length);
      return new Promise<HistoryView>(() => undefined);
    });
    await open();
    await tick(10000);
    expect(calls).toHaveLength(1);
    expect(api.getStats.mock.calls.length).toBeGreaterThanOrEqual(3); // the other polls go on
  });

  it('stops after unmount', async () => {
    server({ '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) });
    const view = await open();
    view.unmount();
    const n = api.getStatsHistory.mock.calls.length;
    await tick(20000);
    expect(api.getStatsHistory.mock.calls.length).toBe(n);
  });
});

describe('Dashboard (spec 29): the charts show the whole run with elapsed-time labels', () => {
  it('x labels are the elapsed time of each sample: t_ms 190000, 191000, 192700 -> 0h 3m 10s, 0h 3m 11s, 0h 3m 12s', async () => {
    server({
      '0': [fx.historySample(190000), fx.historySample(191000), fx.historySample(192700)],
      '1': [fx.historySample(190000)],
    });
    await open();
    expect(labelsOf(0)).toEqual(['0h 3m 10s', '0h 3m 11s', '0h 3m 12s']);
    expect(labelsOf(1)).toEqual(['0h 3m 10s']);
  });

  it('hours count up: t_ms 3725000 is 1h 2m 5s, a first sample at 0 is 0h 0m 0s', async () => {
    server({ '0': [fx.historySample(0), fx.historySample(3725000)], '1': [] });
    await open();
    expect(labelsOf(0)).toEqual(['0h 0m 0s', '1h 2m 5s']);
  });

  it('the x axis is titled as a time', async () => {
    server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) });
    await open();
    const o = JSON.parse(within(card(0)).getByTestId('chart-line').getAttribute('data-options') ?? 'null');
    expect(o.scales.x.title.text).toMatch(/time/i);
  });

  // The rate chart plots L1 like P4TG (tx_rate_l1 / rx_rate_l1): L1 Gbit/s = gbps + mpps * 0.16 (20 B * 8 bit per frame), per sample.
  it('the rate chart plots the L1 rates (gbps + mpps * 0.16), per link', async () => {
    server({
      '0': [
        fx.historySample(1000, { rx_gbps: 1.5, tx_gbps: 1.25, rx_mpps: 10, tx_mpps: 5 }),
        fx.historySample(2000, { rx_gbps: 2.5, tx_gbps: 2.25, rx_mpps: 0, tx_mpps: 2.5 }),
      ],
      '1': [fx.historySample(1000, { rx_gbps: 7.5, tx_gbps: 6.25, rx_mpps: 25, tx_mpps: 12.5 })],
    });
    await open();
    const closeTo = (actual: number[], expected: number[]) => {
      expect(actual).toHaveLength(expected.length);
      expected.forEach((v, i) => {
        expect(actual[i]).toBeCloseTo(v, 9);
      });
    };
    closeTo(dataset(chartOf(0), /RX/).data, [3.1, 2.5]);
    closeTo(dataset(chartOf(0), /TX/).data, [2.05, 2.65]);
    closeTo(dataset(chartOf(1), /RX/).data, [11.5]);
    closeTo(dataset(chartOf(1), /TX/).data, [8.25]);
  });

  it('with the default Mpps of the fixture the rate chart is above the L2 gbps by mpps * 0.16', async () => {
    server({ '0': [fx.historySample(1000)], '1': [] });
    await open();
    expect(dataset(chartOf(0), /RX/).data[0]).toBeCloseTo(9.5 + 1.234567 * 0.16, 9);
    expect(dataset(chartOf(0), /TX/).data[0]).toBeCloseTo(8.25 + 1.2 * 0.16, 9);
  });

  it('a link is charted from the samples under its own index, not by position', async () => {
    // the cards are links 0 and 1; samples of link 1 come first in the object
    server({
      '1': [fx.historySample(1000, { ...fx.NO_PPS, rx_gbps: 7.5 })],
      '0': [fx.historySample(1000, { ...fx.NO_PPS, rx_gbps: 1.5 })],
    });
    await open();
    expect(rxOf(0)).toEqual([1.5]);
    expect(rxOf(1)).toEqual([7.5]);
  });

  it('the loss chart plots the nine cumulative counts (spec 47), one line per cause in the donut order', async () => {
    server({
      '0': [
        fx.historySample(1000, {
          loss_drops: 1,
          limiter_drops: 2,
          buffer_full_drops: 3,
          nic_tx_drops: 4,
          nic_rx_missed: 5,
          nic_rx_nombuf: 6,
          nic_rx_port_buffer_discards: 7,
          nic_rx_phy_discards: 8,
          nic_tx_phy_discards: 9,
        }),
        fx.historySample(2000, {
          loss_drops: 10,
          limiter_drops: 20,
          buffer_full_drops: 30,
          nic_tx_drops: 40,
          nic_rx_missed: 50,
          nic_rx_nombuf: 60,
          nic_rx_port_buffer_discards: 70,
          nic_rx_phy_discards: 80,
          nic_tx_phy_discards: 90,
        }),
      ],
      '1': [],
    });
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Packet loss' }));
    await tick(0);
    const c = chartOf(0);
    expect(c.datasets.map((d) => d.label)).toEqual([...CAUSES]);
    expect(c.datasets).toHaveLength(9);
    CAUSES.forEach((label, i) => {
      expect(c.datasets[i].data, label).toEqual([i + 1, (i + 1) * 10]);
    });
    expect(c.labels).toEqual(['0h 0m 1s', '0h 0m 2s']);
  });

  // A time chart shows at most 100 points, selected over the whole run (first and last kept); the cap of the page's history (86400)
  // is in dashboardSpec29b.test.tsx.
  it('there is no 120-sample cap: 300 samples are charted as 100 points spanning the whole run, oldest first', async () => {
    server({ '0': fx.historySamples(0, 300), '1': fx.historySamples(0, 300) });
    await open();
    expect(labelsOf(0)).toHaveLength(100);
    expect(rxOf(0)).toHaveLength(100);
    expect(labelsOf(0)[0]).toBe('0h 0m 0s');
    expect(labelsOf(0).at(-1)).toBe('0h 4m 59s');
  });

  it('nor does it grow past 120 as the run goes on: 119 samples, then 3 more', async () => {
    const s = server({ '0': fx.historySamples(0, 119), '1': fx.historySamples(0, 119) });
    await open();
    s.samples['0'].push(...fx.historySamples(119, 3));
    s.samples['1'].push(...fx.historySamples(119, 3));
    await tick(2000);
    expect(rxOf(0)).toHaveLength(100); // the 122 samples are charted as 100 points, the newest one the last
    expect(labelsOf(0).at(-1)).toBe('0h 2m 1s');
  });

  it('a run of hours is shown whole, as 100 points (spec 50 G): 10800 samples (3 h at 1 s)', async () => {
    server({ '0': fx.historySamples(0, 10800), '1': [] });
    await open();
    expect(rxOf(0)).toHaveLength(100);
    expect(labelsOf(0)[0]).toBe('0h 0m 0s');
    expect(labelsOf(0).at(-1)).toBe('2h 59m 59s');
  });

  it('a link without samples yet has an empty chart (and the card is there)', async () => {
    server({ '0': fx.historySamples(0, 2) });
    await open();
    expect(rxOf(1)).toEqual([]);
    expect(labelsOf(1)).toEqual([]);
    expect(rxOf(0)).toHaveLength(2);
  });

  it('the stats polls add no points of their own: without a series the chart stays empty', async () => {
    server({});
    await open();
    await tick(5000);
    expect(rxOf(0)).toEqual([]);
    expect(labelsOf(0)).toEqual([]);
  });
});

describe('Dashboard (spec 29): increments are appended', () => {
  it('new samples are added after the old ones: nothing lost, nothing twice', async () => {
    const s = server({ '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) });
    await open();
    s.samples['0'].push(
      fx.historySample(3000, { ...fx.NO_PPS, rx_gbps: 3.5 }),
      fx.historySample(4000, { ...fx.NO_PPS, rx_gbps: 4.5 }),
    );
    s.samples['1'].push(fx.historySample(3000, { ...fx.NO_PPS, rx_gbps: 13.5 }));
    await tick(1000);
    await tick(1000);
    // the first three are the fixture's defaults, plotted as L1: 9.5 + 1.234567 x 0.16 = 9.69753072 Gbit/s
    const r6 = (v: number[]) => v.map((x) => Math.round(x * 1e6) / 1e6);
    expect(r6(rxOf(0))).toEqual([9.697531, 9.697531, 9.697531, 3.5, 4.5]);
    expect(r6(rxOf(1))).toEqual([9.697531, 9.697531, 9.697531, 13.5]);
    expect(labelsOf(0)).toEqual(['0h 0m 0s', '0h 0m 1s', '0h 0m 2s', '0h 0m 3s', '0h 0m 4s']);
    // polls with nothing new change nothing
    await tick(3000);
    expect(rxOf(0)).toHaveLength(5);
    expect(rxOf(1)).toHaveLength(4);
  });

  it('a link that gets its first samples later is charted from then on', async () => {
    const s = server({ '0': fx.historySamples(0, 2) });
    await open();
    expect(rxOf(1)).toEqual([]);
    s.samples['1'] = [fx.historySample(2000, { ...fx.NO_PPS, rx_gbps: 5.5 })];
    await tick(1000);
    await tick(1000);
    expect(rxOf(1)).toEqual([5.5]);
  });
});

describe('Dashboard (spec 29): leaving the dashboard and coming back, and reload', () => {
  it('unmounting and mounting again shows the same data, including what happened meanwhile, without a gap', async () => {
    const s = server({
      '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    const first = await open();
    const before = rxOf(0).length;
    first.unmount();
    // the run goes on while the dashboard is not on screen
    s.samples['0'].push(...fx.historySamples(5, 4, { ...fx.NO_PPS, rx_gbps: 1.75 }));
    s.samples['1'].push(...fx.historySamples(5, 4, { ...fx.NO_PPS, rx_gbps: 2.75 }));
    const calls = api.getStatsHistory.mock.calls.length;
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(sinceArgs()[calls]).toBeUndefined(); // the new mount loads everything again
    expect(before).toBe(5);
    expect(rxOf(0)).toEqual([...Array(5).fill(1.5), ...Array(4).fill(1.75)]);
    expect(rxOf(1)).toEqual([...Array(5).fill(2.5), ...Array(4).fill(2.75)]);
    expect(labelsOf(0)).toHaveLength(9);
    expect(labelsOf(0).at(-1)).toBe('0h 0m 8s');
  });

  it('navigating to another page and back does the same', async () => {
    const s = server({ '0': fx.historySamples(0, 4), '1': fx.historySamples(0, 4) });
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(screen.getByRole('link', { name: 'Settings' }));
    await tick(0);
    expect(hasTab(0)).toBe(false);
    s.samples['0'].push(...fx.historySamples(4, 3));
    s.samples['1'].push(...fx.historySamples(4, 3));
    const calls = api.getStatsHistory.mock.calls.length;
    fireEvent.click(screen.getByRole('link', { name: 'Dashboard' }));
    await tick(0);
    await tick(0);
    expect(sinceArgs()[calls]).toBeUndefined();
    expect(rxOf(0)).toHaveLength(7);
    expect(rxOf(1)).toHaveLength(7);
    expect(labelsOf(0).at(-1)).toBe('0h 0m 6s');
  });

  it('after the remount the polls are incremental again from the data just loaded', async () => {
    server({ '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) });
    const first = await open();
    first.unmount();
    const calls = api.getStatsHistory.mock.calls.length;
    renderApp('/');
    await tick(0);
    await tick(0);
    await tick(2500);
    const after = sinceArgs().slice(calls);
    expect(after[0]).toBeUndefined();
    expect(after.slice(1).length).toBeGreaterThanOrEqual(2);
    expect(after.slice(1).every((a) => a === 4000)).toBe(true);
  });

  it('the data is shown from the server alone: no client-side buffer survives an unmount', async () => {
    // the server has 2 samples only; whatever a previous mount held (here: 6 samples) must not come back
    const s = server({ '0': fx.historySamples(0, 6), '1': fx.historySamples(0, 6) });
    const first = await open();
    expect(rxOf(0)).toHaveLength(6);
    first.unmount();
    s.samples = { '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) };
    serveHistory(s);
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(rxOf(0)).toHaveLength(2);
  });
});

describe('Dashboard (spec 29): a new run replaces the old data', () => {
  it('a changed run_id discards the data held and loads the new run in full', async () => {
    const s = server({
      '0': fx.historySamples(0, 6, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 6, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    await open();
    expect(rxOf(0)).toHaveLength(6);
    // the next run: its clock starts at 0 again, so an incremental request (since_ms = 5000) finds nothing newer
    s.runId = 2;
    s.startedAt = S1 + 900;
    s.samples = {
      '0': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 4.5 }),
      '1': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 5.5 }),
    };
    const calls = api.getStatsHistory.mock.calls.length;
    await tick(1000);
    await tick(1000);
    await tick(1000);
    expect(sinceArgs().slice(calls)).toContain(undefined); // fetched in full again
    expect(rxOf(0)).toEqual([4.5, 4.5]);
    expect(rxOf(1)).toEqual([5.5, 5.5]);
    expect(labelsOf(0)).toEqual(['0h 0m 0s', '0h 0m 1s']);
  });

  it('after the reload the polls are incremental again, from the new run', async () => {
    const s = server({ '0': fx.historySamples(0, 6), '1': fx.historySamples(0, 6) });
    await open();
    s.runId = 2;
    s.startedAt = S1 + 900;
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

  it('samples of the old run are never mixed with the new run, even when the new run is already longer', async () => {
    const s = server({ '0': fx.historySamples(0, 3, { ...fx.NO_PPS, rx_gbps: 1.5 }), '1': [] });
    await open();
    s.runId = 2;
    s.startedAt = S1 + 900;
    s.samples = { '0': fx.historySamples(0, 8, { ...fx.NO_PPS, rx_gbps: 4.5 }), '1': [] };
    await tick(3000);
    expect(rxOf(0)).toEqual(Array(8).fill(4.5));
  });

  it('the first run after an idle controller (run_id null, then 1) is loaded without waiting for a restart', async () => {
    const s = server({}, null);
    await open();
    expect(rxOf(0)).toEqual([]);
    s.runId = 1;
    s.startedAt = S1;
    s.samples = { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) };
    await tick(2000);
    expect(rxOf(0)).toHaveLength(3);
    expect(rxOf(1)).toHaveLength(3);
  });
});

describe('Dashboard (spec 29): a failing history request', () => {
  it('keeps the charts as they are, asks again with the same since_ms, and recovers', async () => {
    const s = server({ '0': fx.historySamples(0, 4), '1': fx.historySamples(0, 4) });
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    await open();
    const ok = api.getStatsHistory.getMockImplementation();
    api.getStatsHistory.mockRejectedValue(new Error('Failed to fetch'));
    await tick(3000);
    expect(rxOf(0)).toHaveLength(4); // the card and its chart are still there
    expect(api.getStatsHistory.mock.calls.length).toBeGreaterThanOrEqual(3);
    s.samples['0'].push(fx.historySample(4000, { ...fx.NO_PPS, rx_gbps: 6.5 }));
    api.getStatsHistory.mockImplementation(must(ok, 'the mock implementation'));
    await tick(2000);
    expect(rxOf(0)).toHaveLength(5);
    expect(rxOf(0).at(-1)).toBe(6.5);
    expect(sinceArgs().filter((a) => a === undefined)).toHaveLength(1); // one full request only: the failure did not reset
    errors.mockRestore();
  });

  it('does not stop the other polls or hide the cards', async () => {
    api.getStatsHistory.mockRejectedValue(new Error('boom'));
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    await open();
    await tick(4000);
    expect(hasTab(0)).toBe(true);
    expect(api.getStats.mock.calls.length).toBeGreaterThanOrEqual(4);
    expect(api.getEmulation.mock.calls.length).toBeGreaterThanOrEqual(4);
    errors.mockRestore();
  });
});

// ---- the final charts stay after a stop -----------------------------------------------------------
//
// After a stop the dashboard keeps showing the stopped run's tabs with their final rate and loss charts (from the history, which
// the controller keeps until the next start), until the next start:
//   - the tabs are there while the polled state is `stopped`, `stopping` or `failed` and the history of the run is not empty: one tab
//     per link index in the history, in the same session after a stop and after a reload/remount alike. The live stat list is NOT
//     pinned; the titles are (they come from the history's `links`);
//   - the charts are the history's, Rate and Loss, both selectable; the tab keeps the enabled "Delay histogram" choice, the
//     controller serves the run's last histogram after the stop (dashboardSpec50.test.tsx);
//   - a start keeps the tabs under the loading overlay while `starting`; the next run's tabs show only its own samples once it runs.
// A controller that never ran (no samples, started_at null) has no tabs.

describe('Dashboard (spec 29, round 2): the final charts of a stopped run stay until the next start', () => {
  const stoppedStats = (elapsed_ms = 305_400) => fx.stats([], { stale: true, age_ms: undefined, elapsed_ms });
  // The marker of an ended run's tab is the tab itself, without any badge.
  const marker = (d: number) => (hasTab(d) && tabBadge(d) === '' ? tab(d) : null);
  const radioOf = (d: number, name: string) => queryRadio(d, name);

  /** A running emulation with two links and a 5 s series, opened and settled; `stop()` turns it into the stopped payload. */
  async function runThenStop() {
    const s = server({
      '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    await open();
    expect(tabBadge(0)).toBe(''); // a running link is not marked
    expect(tabBadge(1)).toBe('');
    const stop = async () => {
      api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
      api.getStats.mockResolvedValue(stoppedStats());
      await tick(2500);
    };
    return { s, stop };
  }

  it('after the stop both cards are still there, unmarked (spec 54 L1), with their whole final series', async () => {
    const { stop } = await runThenStop();
    await stop();
    expectState('stopped');
    for (const d of [0, 1]) expect(marker(d), `card ${d}`).toBeInTheDocument();
    expect(rxOf(0)).toEqual(Array(5).fill(1.5));
    expect(rxOf(1)).toEqual(Array(5).fill(2.5));
    expect(labelsOf(0)).toEqual(['0h 0m 0s', '0h 0m 1s', '0h 0m 2s', '0h 0m 3s', '0h 0m 4s']);
  });

  it('they stay for as long as the run is stopped, and the charts do not change', async () => {
    const { stop } = await runThenStop();
    await stop();
    await tick(30_000);
    expect(marker(0)).toBeInTheDocument();
    expect(rxOf(0)).toHaveLength(5);
    expect(rxOf(1)).toHaveLength(5);
  });

  it('the Loss chart of a stopped link works too, and Rate comes back', async () => {
    const { s, stop } = await runThenStop();
    s.samples['0'].push(
      fx.historySample(5000, { loss_drops: 2, nic_tx_drops: 0, nic_rx_missed: 3 }),
      fx.historySample(6000, { loss_drops: 4, nic_tx_drops: 0, nic_rx_missed: 3 }),
    );
    await stop();
    fireEvent.click(must(radioOf(0, 'Packet loss'), 'Drops radio'));
    await tick(0);
    expect(dataset(chartOf(0), /^Loss model$/).data.slice(-2)).toEqual([2, 4]);
    expect(dataset(chartOf(0), /^NIC RX missed/).data.slice(-2)).toEqual([3, 3]);
    fireEvent.click(must(radioOf(0, 'Traffic rates'), 'Rate radio'));
    await tick(0);
    expect(rxOf(0)).toHaveLength(7);
  });

  it('the "Delay histogram" choice stays on a stopped card (spec 50 A3), enabled', async () => {
    const { stop } = await runThenStop();
    await stop();
    for (const d of [0, 1]) expect(radioOf(d, 'Delay histogram'), `card ${d}`).toBeEnabled();
  });

  it('a card that showed the histogram keeps showing it at the stop, and keeps fetching it', async () => {
    const { stop } = await runThenStop();
    fireEvent.click(must(radioOf(0, 'Delay histogram'), 'histogram radio'));
    await tick(1000);
    expect(delayBar(card(0))).toBeInTheDocument();
    await stop();
    expect(delayBar(card(0))).toBeInTheDocument();
    expect(within(card(0)).queryByTestId('chart-line')).not.toBeInTheDocument();
    expect(radioOf(0, 'Delay histogram')).toBeChecked();
    const n = api.getDelayHistogram.mock.calls.length;
    await tick(5000);
    expect(api.getDelayHistogram.mock.calls.length).toBeGreaterThan(n);
  });

  // The real controller answers /api/stats of a stopping emulation with no directions at once (the poller only reads a running dpds).
  it('while the run is stopping (the stats already empty) the cards stay, unmarked (spec 54 L1)', async () => {
    await runThenStop();
    api.getEmulation.mockResolvedValue(fx.emulation('stopping'));
    api.getStats.mockResolvedValue(stoppedStats());
    await tick(2500);
    for (const d of [0, 1]) {
      expect(tabBadge(d), `tab ${d}`).toBe('');
      expect(marker(d), `tab ${d}`).toBeInTheDocument();
    }
    expect(rxOf(0)).toEqual(Array(5).fill(1.5));
  });

  // A new start keeps the tablist and the Summary tab mounted; the tabs are the stored links and the charts carry no samples.
  it('once the state is "starting" the tablist and the Summary tab stay, the charts carry no samples (spec 60)', async () => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
    const { s, stop } = await runThenStop();
    await stop();
    expect(hasTab(0)).toBe(true);
    const list = screen.getByRole('tablist');
    const summary = summaryTab();
    // the controller started again: its series is empty so far (a new started_at), the first polls carry no directions yet
    s.runId = 2;
    s.startedAt = S1 + 900;
    s.samples = {};
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await tick(2500);
    expect(screen.getByRole('tablist')).toBe(list);
    expect(summaryTab()).toBe(summary);
    expect(hasTab(0)).toBe(true);
    expect(hasTab(1)).toBe(true);
    expect(rxOf(0)).toEqual([]);
    expect(rxOf(1)).toEqual([]);
  });

  // The old run is not shown, also while the history still holds it.
  it('also when the history still holds the old run at that moment, the charts carry no samples (spec 60)', async () => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
    const { stop } = await runThenStop();
    await stop();
    const list = screen.getByRole('tablist');
    // state "starting", the server's series is still the old run's
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await tick(1000);
    expect(screen.getByRole('tablist')).toBe(list);
    expect(hasTab(0)).toBe(true);
    expect(hasTab(1)).toBe(true);
    expect(rxOf(0)).toEqual([]);
    expect(rxOf(1)).toEqual([]);
  });

  it('pressing Start keeps them when the controller has accepted the request', async () => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
    const { stop } = await runThenStop();
    await stop();
    expect(hasTab(0)).toBe(true);
    api.startEmulation.mockResolvedValue(fx.emulation('starting'));
    fireEvent.click(screen.getByRole('button', { name: /^start$/i }));
    await tick(0);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(hasTab(0)).toBe(true);
  });

  it('the next run is charted from its own samples only, as live cards', async () => {
    const { s, stop } = await runThenStop();
    await stop();
    s.runId = 2;
    s.startedAt = S1 + 900;
    s.samples = {
      '0': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 4.5 }),
      '1': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 5.5 }),
    };
    api.getEmulation.mockResolvedValue(fx.emulation('running', { started_at: S1 + 900 }));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)], { elapsed_ms: 1500 }));
    await tick(3000);
    expect(tabBadge(0)).toBe('');
    expect(tabBadge(1)).toBe('');
    expect(rxOf(0)).toEqual([4.5, 4.5]);
    expect(rxOf(1)).toEqual([5.5, 5.5]);
  });

  it('after a reload (a fresh mount) of a stopped controller the final charts are shown the same way', async () => {
    server({
      '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(stoppedStats());
    await open();
    await tick(1000);
    for (const d of [0, 1]) expect(marker(d), `card ${d}`).toBeInTheDocument();
    expect(rxOf(0)).toEqual(Array(5).fill(1.5));
    expect(rxOf(1)).toEqual(Array(5).fill(2.5));
    expect(radioOf(0, 'Delay histogram')).toBeEnabled();
  });

  it('a one-link run leaves one card', async () => {
    server({ '0': fx.historySamples(0, 3) });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(stoppedStats());
    await open();
    await tick(1000);
    expect(marker(0)).toBeInTheDocument();
    expect(hasTab(1)).toBe(false);
  });

  it('a controller that never ran has no cards', async () => {
    server({}, null);
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await open();
    await tick(2000);
    expect(hasTab(0)).toBe(false);
    expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  });

  it('a stopped run with an empty series (it stopped before the first sample) has no cards either', async () => {
    server({ '0': [], '1': [] });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(stoppedStats(400));
    await open();
    await tick(2000);
    expect(hasTab(0)).toBe(false);
  });
});
