import { fireEvent, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import { formatCount, formatDuration } from '../common/format';
import { CHART_RADIOS, card, delayBar, hasTab, histTables, queryRadio, radioNames, tab } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedLinks, tick } from '../test/helpers';
import { ApiError, api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 50 A3 (user 2026-11-05: "The histogram diagram is lost when we stop the traffic. It should still be displayed even when
// traffic is stopped."). The controller now answers the run's last histogram after the stop (crates/dpds-controller/tests/
// integration/spec50.rs), with `mean_ns` / `std_ns` of the link's last stats poll. Contract of the frontend side:
//   - in an ended run (stopped or failed) a link whose run had delay statistics keeps an ENABLED "Delay histogram" radio; chosen,
//     it shows the bar chart and the three tables of spec 36, polled as today (the answer simply does not change);
//   - the Values table takes Delay / σ(Delay) from the live stats while there are some and, with no live stats (an ended run), from
//     the response's `mean_ns` / `std_ns` ("-" when they are null);
//   - a histogram view that is open when the run ends stays open (it used to fall back to Rate);
//   - before any run the radio stays disabled, as before (dashboardBeforeRun.test.tsx).
//
// "HAD DELAY STATISTICS", as implemented: the link's STORED config (localStorage) has `delay_stats !== false`, so a link with no
// stored config (or with the field missing) counts as having them; the controller is not asked beforehand. (The spec left the source
// to the implementer: the config, or the controller answering a histogram.) The tests here keep both sources in agreement, so
// either implementation passes: a link with statistics has `delay_stats:
// true` in its stored config AND the controller answers its histogram; a link without has `delay_stats: false` stored AND the
// controller answers 404 for it. Nothing here tells the two sources apart, nor asserts that a link without statistics is (not) probed.

const S1 = 1762000000;

/** Two stored links, the first with delay statistics on, the second as `second` says. */
function storedLinks(secondStats: boolean) {
  seedLinks([
    fx.link({ id: 1, name: 'a-b', config: { ...fx.config(), delay_stats: true } }),
    fx.link({
      id: 2,
      name: 'b-a',
      ingress: '0000:65:00.1',
      egress: '0000:65:00.0',
      config: { ...fx.config(), delay_stats: secondStats },
    }),
  ]);
}

/** The controller after a run: series and totals of two links, no directions, a frozen clock. */
function endedRun(state: 'stopped' | 'failed', over: Partial<HistogramView> = {}) {
  serveHistory({ startedAt: S1, samples: { '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) } });
  api.getEmulation.mockResolvedValue(
    fx.emulation(state, { directions: [], error: state === 'failed' ? 'dpds exited (status 1)' : null }),
  );
  api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
  api.getDelayHistogram.mockImplementation(async (dir) => fx.histogram(dir, over));
}

async function open() {
  vi.useFakeTimers();
  const view = renderApp('/');
  await tick(0);
  await tick(0);
  return view;
}

const tablesOf = (d: number) => histTables(card(d));
const cells = (t: HTMLElement) =>
  Array.from(must(within(t).getAllByRole('row')[1], 'the body row').children).map((c) =>
    (c.textContent ?? '').replace(/\s+/g, ' ').trim(),
  );
const VALUES = 1;

beforeEach(() => {
  storedLinks(true);
});

describe.each(['stopped', 'failed'] as const)('Dashboard (spec 50 A3): the histogram of a %s run', (state) => {
  it('is offered: the radios are Traffic rates, Delay histogram, Packet loss, the histogram one enabled', async () => {
    endedRun(state);
    await open();
    await tick(1000);
    expect(radioNames(0)).toEqual(CHART_RADIOS);
    expect(radioNames(1)).toEqual(CHART_RADIOS);
    for (const d of [0, 1]) expect(must(queryRadio(d, 'Delay histogram'), `radio ${d}`)).toBeEnabled();
  });

  it('shows the bar chart and the three tables when chosen, from the controller', async () => {
    endedRun(state);
    await open();
    fireEvent.click(must(queryRadio(0, 'Delay histogram')));
    await tick(1000);
    expect(delayBar(card(0))).toBeInTheDocument();
    expect(tablesOf(0)).toHaveLength(3);
    expect(api.getDelayHistogram).toHaveBeenCalledWith(0);
    const bars = JSON.parse(delayBar(card(0)).getAttribute('data-chart') ?? 'null');
    expect(bars.datasets[0].data).toEqual(fx.histogram(0).bins);
  });

  it('takes Delay and σ from the response (mean_ns, std_ns) as there are no live stats', async () => {
    endedRun(state, { mean_ns: 1_500_000, std_ns: 75_000 } as Partial<HistogramView>);
    await open();
    fireEvent.click(must(queryRadio(0, 'Delay histogram')));
    await tick(1000);
    const h = fx.histogram(0);
    expect(cells(must(tablesOf(0)[VALUES]))).toEqual([
      formatDuration(1_500_000),
      formatDuration(75_000),
      formatCount(h.underflow + h.overflow),
      formatCount(h.count),
    ]);
  });

  it('reads "-" for Delay and σ when the response has null for them', async () => {
    endedRun(state, { mean_ns: null, std_ns: null } as Partial<HistogramView>);
    await open();
    fireEvent.click(must(queryRadio(0, 'Delay histogram')));
    await tick(1000);
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row.slice(0, 2)).toEqual(['-', '-']);
  });

  it('is polled as before: once per second, the answer simply does not change', async () => {
    endedRun(state);
    await open();
    fireEvent.click(must(queryRadio(1, 'Delay histogram')));
    await tick(1000);
    const n = api.getDelayHistogram.mock.calls.filter(([d]) => d === 1).length;
    await tick(3000);
    expect(api.getDelayHistogram.mock.calls.filter(([d]) => d === 1).length).toBeGreaterThanOrEqual(n + 3);
  });

  it('is not polled while its tab is not shown, like a live one', async () => {
    endedRun(state);
    await open();
    fireEvent.click(must(queryRadio(0, 'Delay histogram')));
    await tick(1000);
    fireEvent.click(tab(1));
    await tick(0);
    const n = api.getDelayHistogram.mock.calls.filter(([d]) => d === 0).length;
    await tick(3000);
    expect(api.getDelayHistogram.mock.calls.filter(([d]) => d === 0).length).toBe(n);
  });
});

describe('Dashboard (spec 50 A3): which links keep a histogram', () => {
  it('a link whose run had no delay statistics has no "Delay histogram" radio, the other one keeps it', async () => {
    storedLinks(false);
    endedRun('stopped');
    api.getDelayHistogram.mockImplementation(async (dir) => {
      if (dir === 1) throw new ApiError(404, 'delay statistics are off for direction 1');
      return fx.histogram(dir);
    });
    await open();
    await tick(1000);
    expect(radioNames(0)).toEqual(CHART_RADIOS);
    expect(radioNames(1)).toEqual(['Traffic rates', 'Packet loss']);
  });

  it('before any run the radio stays disabled and nothing is fetched', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([]));
    await open();
    await tick(1000);
    expect(hasTab(0)).toBe(true);
    for (const d of [0, 1]) expect(must(queryRadio(d, 'Delay histogram'), `radio ${d}`)).toBeDisabled();
    expect(api.getDelayHistogram).not.toHaveBeenCalled();
  });

  it('while running, the live stats still win for Delay and σ over the response', async () => {
    serveHistory({ startedAt: S1, samples: { '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) } });
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, { delay_mean_ns: 1_000_000, delay_std_ns: 50_000 }),
        fx.dirStats(1, { delay_mean_ns: 1_000_000, delay_std_ns: 50_000 }),
      ]),
    );
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { mean_ns: 9_000_000, std_ns: 900_000 } as Partial<HistogramView>),
    );
    await open();
    fireEvent.click(must(queryRadio(0, 'Delay histogram')));
    await tick(1000);
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row.slice(0, 2)).toEqual([formatDuration(1_000_000), formatDuration(50_000)]);
  });
});
