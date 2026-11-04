import { fireEvent, screen, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistorySample, HistoryView } from '../api/types';
import * as fx from '../test/fixtures';
import { api, serveHistory } from '../test/mockApi';
import type { HistoryServer } from '../test/mockApi';
import { renderApp, seedStore, tick } from '../test/helpers';
import { must } from '../test/must';

// Spec 29 (user 2026-11-04, "Server-side series"): the dashboard charts are built from the controller's series,
// `GET /api/stats/history[?since_ms=<t>]`, not from a client-side buffer of stats polls. Contract (frontend side):
//   - `api.getStatsHistory(sinceMs?)`, `HistoryView {started_at, samples: {"<link index>": HistorySample[]}}`, see
//     src/test/mockApi.ts (`serveHistory`) and the header of crates/dpds-controller/tests/integration/chart_history.rs.
//   - one full fetch (no argument) on mount, then once per poll an incremental one with since_ms = the newest t_ms received so
//     far (over all links); while nothing has been received yet, the fetches stay full (CHOICE).
//   - a changed `started_at` is a new run (CHOICE of the tester): the data held is dropped and the history is fetched in full
//     again, because the new run's t_ms start at 0 again and an incremental answer would be empty.
//   - the card of link d (its `dir`) charts samples[String(d)]: rate = rx_gbps / tx_gbps as they are, drops = loss_pct and
//     limiter + buffer_full + nic_tx + nic_rx; x labels = formatElapsed(t_ms / 1000), e.g. t_ms 192000 -> "0h 3m 12s".
//   - no cap: the 120-sample client buffer is gone, a run of any length is shown whole.
// Not pinned: whether the dashboard shows the final charts of a stopped run (cards exist only while the stats have
// directions, as before).

interface Dataset {
  label?: string;
  data: number[];
}
interface ChartData {
  labels: string[];
  datasets: Dataset[];
}

const card = (d: number) => screen.getByTestId(`direction-card-${d}`);
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

  it('the rate chart plots rx_gbps and tx_gbps as they are, per link', async () => {
    server({
      '0': [
        fx.historySample(1000, { rx_gbps: 1.5, tx_gbps: 1.25 }),
        fx.historySample(2000, { rx_gbps: 2.5, tx_gbps: 2.25 }),
      ],
      '1': [fx.historySample(1000, { rx_gbps: 7.5, tx_gbps: 6.25 })],
    });
    await open();
    expect(dataset(chartOf(0), /RX/).data).toEqual([1.5, 2.5]);
    expect(dataset(chartOf(0), /TX/).data).toEqual([1.25, 2.25]);
    expect(dataset(chartOf(1), /RX/).data).toEqual([7.5]);
    expect(dataset(chartOf(1), /TX/).data).toEqual([6.25]);
  });

  it('a link is charted from the samples under its own index, not by position', async () => {
    // the cards are links 0 and 1; samples of link 1 come first in the object
    server({
      '1': [fx.historySample(1000, { rx_gbps: 7.5 })],
      '0': [fx.historySample(1000, { rx_gbps: 1.5 })],
    });
    await open();
    expect(rxOf(0)).toEqual([1.5]);
    expect(rxOf(1)).toEqual([7.5]);
  });

  it('the drops chart plots loss_pct and the sum of the four overload causes', async () => {
    server({
      '0': [
        fx.historySample(1000, {
          loss_pct: 0.5,
          limiter_pct: 1,
          buffer_full_pct: 2,
          nic_tx_pct: 0.25,
          nic_rx_pct: 0.125,
        }),
        fx.historySample(2000, { loss_pct: 0, limiter_pct: 0, buffer_full_pct: 0, nic_tx_pct: 0, nic_rx_pct: 4 }),
      ],
      '1': [],
    });
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Drops' }));
    await tick(0);
    const c = chartOf(0);
    expect(dataset(c, /model loss/i).data).toEqual([0.5, 0]);
    expect(dataset(c, /overload/i).data).toEqual([3.375, 4]);
    expect(c.labels).toEqual(['0h 0m 1s', '0h 0m 2s']);
  });

  it('there is no 120-sample cap: 300 samples are all in the chart, oldest first', async () => {
    server({ '0': fx.historySamples(0, 300), '1': fx.historySamples(0, 300) });
    await open();
    expect(labelsOf(0)).toHaveLength(300);
    expect(rxOf(0)).toHaveLength(300);
    expect(labelsOf(0)[0]).toBe('0h 0m 0s');
    expect(labelsOf(0).at(-1)).toBe('0h 4m 59s');
  });

  it('nor does it grow past 120 as the run goes on: 119 samples, then 3 more', async () => {
    const s = server({ '0': fx.historySamples(0, 119), '1': fx.historySamples(0, 119) });
    await open();
    s.samples['0'].push(...fx.historySamples(119, 3));
    s.samples['1'].push(...fx.historySamples(119, 3));
    await tick(2000);
    expect(rxOf(0)).toHaveLength(122);
    expect(labelsOf(0).at(-1)).toBe('0h 2m 1s');
  });

  it('a run of hours is shown whole: 10800 samples (3 h at 1 s)', async () => {
    server({ '0': fx.historySamples(0, 10800), '1': [] });
    await open();
    expect(rxOf(0)).toHaveLength(10800);
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
    s.samples['0'].push(fx.historySample(3000, { rx_gbps: 3.5 }), fx.historySample(4000, { rx_gbps: 4.5 }));
    s.samples['1'].push(fx.historySample(3000, { rx_gbps: 13.5 }));
    await tick(1000);
    await tick(1000);
    expect(rxOf(0)).toEqual([9.5, 9.5, 9.5, 3.5, 4.5]);
    expect(rxOf(1)).toEqual([9.5, 9.5, 9.5, 13.5]);
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
    s.samples['1'] = [fx.historySample(2000, { rx_gbps: 5.5 })];
    await tick(1000);
    await tick(1000);
    expect(rxOf(1)).toEqual([5.5]);
  });
});

describe('Dashboard (spec 29): leaving the dashboard and coming back, and reload', () => {
  it('unmounting and mounting again shows the same data, including what happened meanwhile, without a gap', async () => {
    const s = server({
      '0': fx.historySamples(0, 5, { rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { rx_gbps: 2.5 }),
    });
    const first = await open();
    const before = rxOf(0).length;
    first.unmount();
    // the run goes on while the dashboard is not on screen
    s.samples['0'].push(...fx.historySamples(5, 4, { rx_gbps: 1.75 }));
    s.samples['1'].push(...fx.historySamples(5, 4, { rx_gbps: 2.75 }));
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
    fireEvent.click(screen.getByRole('link', { name: 'Links' }));
    await tick(0);
    expect(screen.queryByTestId('direction-card-0')).not.toBeInTheDocument();
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
  it('a changed started_at discards the data held and loads the new run in full', async () => {
    const s = server({
      '0': fx.historySamples(0, 6, { rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 6, { rx_gbps: 2.5 }),
    });
    await open();
    expect(rxOf(0)).toHaveLength(6);
    // the next run: its clock starts at 0 again, so an incremental request (since_ms = 5000) finds nothing newer
    s.startedAt = S1 + 900;
    s.samples = { '0': fx.historySamples(0, 2, { rx_gbps: 4.5 }), '1': fx.historySamples(0, 2, { rx_gbps: 5.5 }) };
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
    s.startedAt = S1 + 900;
    s.samples = { '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) };
    await tick(3000);
    const calls = api.getStatsHistory.mock.calls.length;
    await tick(3000);
    const later = sinceArgs().slice(calls);
    expect(later.length).toBeGreaterThanOrEqual(2);
    expect(later.every((a) => a === 1000)).toBe(true);
    s.samples['0'].push(fx.historySample(2000, { rx_gbps: 8.5 }));
    await tick(2000);
    expect(rxOf(0).at(-1)).toBe(8.5);
    expect(rxOf(0)).toHaveLength(3);
  });

  it('samples of the old run are never mixed with the new run, even when the new run is already longer', async () => {
    const s = server({ '0': fx.historySamples(0, 3, { rx_gbps: 1.5 }), '1': [] });
    await open();
    s.startedAt = S1 + 900;
    s.samples = { '0': fx.historySamples(0, 8, { rx_gbps: 4.5 }), '1': [] };
    await tick(3000);
    expect(rxOf(0)).toEqual(Array(8).fill(4.5));
  });

  it('the first run after an idle controller (started_at null, then a time) is loaded without waiting for a restart', async () => {
    const s = server({}, null);
    await open();
    expect(rxOf(0)).toEqual([]);
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
    s.samples['0'].push(fx.historySample(4000, { rx_gbps: 6.5 }));
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
    expect(screen.getByTestId('direction-card-0')).toBeInTheDocument();
    expect(api.getStats.mock.calls.length).toBeGreaterThanOrEqual(4);
    expect(api.getEmulation.mock.calls.length).toBeGreaterThanOrEqual(4);
    errors.mockRestore();
  });
});

// ---- spec 29, second round: the final charts stay after a stop -----------------------------------------------------------
//
// User decision (2026-11-04): after a stop the dashboard keeps showing the stopped run's link cards with their final rate and
// drops charts (from the history, which the controller keeps until the next start), marked as stopped, until the next start.
// Contract the tests pin (the tester's choices, not user-confirmed):
//   - the marker is the visible text "Stopped" inside each card (the card of a running link has none);
//   - cards are there while the polled state is `stopped` and the history of the run is not empty: one card per link index in
//     the history (testid `direction-card-<index>`), in the same session after a stop and after a reload/remount alike. Their
//     titles and the live stat list are NOT pinned (a card may show the last values it knew, or none);
//   - the charts are the history's, Rate and Drops, both selectable; there is no "Delay histogram" choice on a stopped card
//     (the controller's histogram needs live telemetry), nothing is fetched for it, and a card that showed the histogram falls
//     back to Rate;
//   - a start clears them: as soon as the state is `starting` (or the Start request has been answered) no stopped card is
//     left, and the next run's cards show only its own samples.
// A controller that never ran (no samples, started_at null) has no cards.

describe('Dashboard (spec 29, round 2): the final charts of a stopped run stay until the next start', () => {
  const stoppedStats = (elapsed_ms = 305_400) => fx.stats([], { stale: true, age_ms: undefined, elapsed_ms });
  const marker = (d: number) => within(card(d)).queryByText('Stopped');
  const radioOf = (d: number, name: string) => within(card(d)).queryByRole('radio', { name });

  /** A running emulation with two links and a 5 s series, opened and settled; `stop()` turns it into the stopped payload. */
  async function runThenStop() {
    const s = server({
      '0': fx.historySamples(0, 5, { rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { rx_gbps: 2.5 }),
    });
    await open();
    expect(marker(0)).not.toBeInTheDocument(); // a running link is not marked
    expect(marker(1)).not.toBeInTheDocument();
    const stop = async () => {
      api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
      api.getStats.mockResolvedValue(stoppedStats());
      await tick(2500);
    };
    return { s, stop };
  }

  it('after the stop both cards are still there, marked "Stopped", with their whole final series', async () => {
    const { stop } = await runThenStop();
    await stop();
    expect(screen.getByTestId('state-badge')).toHaveAttribute('data-state', 'stopped');
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

  it('the Drops chart of a stopped card works too, and Rate comes back', async () => {
    const { s, stop } = await runThenStop();
    s.samples['0'].push(
      fx.historySample(5000, { loss_pct: 2, limiter_pct: 0, buffer_full_pct: 0, nic_tx_pct: 0, nic_rx_pct: 3 }),
      fx.historySample(6000, { loss_pct: 4, limiter_pct: 0, buffer_full_pct: 0, nic_tx_pct: 0, nic_rx_pct: 0 }),
    );
    await stop();
    fireEvent.click(must(radioOf(0, 'Drops'), 'Drops radio'));
    await tick(0);
    expect(dataset(chartOf(0), /model loss/i).data.slice(-2)).toEqual([2, 4]);
    expect(dataset(chartOf(0), /overload/i).data.slice(-2)).toEqual([3, 0]);
    fireEvent.click(must(radioOf(0, 'Rate'), 'Rate radio'));
    await tick(0);
    expect(rxOf(0)).toHaveLength(7);
  });

  it('there is no "Delay histogram" choice on a stopped card, and none is fetched', async () => {
    const { stop } = await runThenStop();
    await stop();
    expect(radioOf(0, 'Delay histogram')).not.toBeInTheDocument();
    expect(radioOf(1, 'Delay histogram')).not.toBeInTheDocument();
    const n = api.getDelayHistogram.mock.calls.length;
    await tick(5000);
    expect(api.getDelayHistogram.mock.calls.length).toBe(n);
  });

  it('a card that showed the histogram falls back to the Rate chart at the stop, and stops fetching', async () => {
    const { stop } = await runThenStop();
    fireEvent.click(must(radioOf(0, 'Delay histogram'), 'histogram radio'));
    await tick(1000);
    expect(within(card(0)).getByTestId('chart-bar')).toBeInTheDocument();
    await stop();
    expect(within(card(0)).queryByTestId('chart-bar')).not.toBeInTheDocument();
    expect(within(card(0)).getByTestId('chart-line')).toBeInTheDocument();
    expect(radioOf(0, 'Rate')).toBeChecked();
    const n = api.getDelayHistogram.mock.calls.length;
    await tick(5000);
    expect(api.getDelayHistogram.mock.calls.length).toBe(n);
  });

  it('while the run is stopping the cards are still the live ones, not marked', async () => {
    await runThenStop();
    api.getEmulation.mockResolvedValue(fx.emulation('stopping'));
    await tick(2500);
    expect(marker(0)).not.toBeInTheDocument();
    expect(marker(1)).not.toBeInTheDocument();
    expect(screen.getByTestId('direction-card-0')).toBeInTheDocument();
  });

  it('a new start clears them: once the state is "starting" no card is left', async () => {
    const { s, stop } = await runThenStop();
    await stop();
    expect(screen.getByTestId('direction-card-0')).toBeInTheDocument();
    // the controller started again: its series is empty so far (a new started_at), the first polls carry no directions yet
    s.startedAt = S1 + 900;
    s.samples = {};
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await tick(2500);
    expect(screen.queryByTestId('direction-card-0')).not.toBeInTheDocument();
    expect(screen.queryByTestId('direction-card-1')).not.toBeInTheDocument();
  });

  it('even when the history still holds the old run at that moment', async () => {
    const { stop } = await runThenStop();
    await stop();
    // state "starting", the server's series is still the old run's
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await tick(1000);
    expect(screen.queryByTestId('direction-card-0')).not.toBeInTheDocument();
    expect(screen.queryByTestId('direction-card-1')).not.toBeInTheDocument();
  });

  it('pressing Start clears them as soon as the controller has accepted the request', async () => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
    const { stop } = await runThenStop();
    await stop();
    expect(screen.getByTestId('direction-card-0')).toBeInTheDocument();
    api.startEmulation.mockResolvedValue(fx.emulation('starting'));
    fireEvent.click(screen.getByRole('button', { name: /^start$/i }));
    await tick(0);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('direction-card-0')).not.toBeInTheDocument();
  });

  it('the next run is charted from its own samples only, as live cards', async () => {
    const { s, stop } = await runThenStop();
    await stop();
    s.startedAt = S1 + 900;
    s.samples = { '0': fx.historySamples(0, 2, { rx_gbps: 4.5 }), '1': fx.historySamples(0, 2, { rx_gbps: 5.5 }) };
    api.getEmulation.mockResolvedValue(fx.emulation('running', { started_at: S1 + 900 }));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)], { elapsed_ms: 1500 }));
    await tick(3000);
    expect(marker(0)).not.toBeInTheDocument();
    expect(marker(1)).not.toBeInTheDocument();
    expect(rxOf(0)).toEqual([4.5, 4.5]);
    expect(rxOf(1)).toEqual([5.5, 5.5]);
  });

  it('after a reload (a fresh mount) of a stopped controller the final charts are shown the same way', async () => {
    server({ '0': fx.historySamples(0, 5, { rx_gbps: 1.5 }), '1': fx.historySamples(0, 5, { rx_gbps: 2.5 }) });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(stoppedStats());
    await open();
    await tick(1000);
    for (const d of [0, 1]) expect(marker(d), `card ${d}`).toBeInTheDocument();
    expect(rxOf(0)).toEqual(Array(5).fill(1.5));
    expect(rxOf(1)).toEqual(Array(5).fill(2.5));
    expect(radioOf(0, 'Delay histogram')).not.toBeInTheDocument();
  });

  it('a one-link run leaves one card', async () => {
    server({ '0': fx.historySamples(0, 3) });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(stoppedStats());
    await open();
    await tick(1000);
    expect(marker(0)).toBeInTheDocument();
    expect(screen.queryByTestId('direction-card-1')).not.toBeInTheDocument();
  });

  it('a controller that never ran has no cards', async () => {
    server({}, null);
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await open();
    await tick(2000);
    expect(screen.queryByTestId('direction-card-0')).not.toBeInTheDocument();
    expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  });

  it('a stopped run with an empty series (it stopped before the first sample) has no cards either', async () => {
    server({ '0': [], '1': [] });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(stoppedStats(400));
    await open();
    await tick(2000);
    expect(screen.queryByTestId('direction-card-0')).not.toBeInTheDocument();
  });
});
