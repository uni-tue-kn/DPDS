import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { Counters, DirectionStats, HistorySample } from '../api/types';
import { formatElapsed } from '../common/format';
import { propsOf } from '../test/chartProps';
import {
  activePanel,
  allTabs,
  CAUSE_HELP,
  CAUSES,
  card,
  findTabs,
  hasSummary,
  panelOf,
  summary,
  summaryRadio,
  summaryTab,
  tab,
  tabBadge,
  tabs,
  tabTitle,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 36 S1 (user 2026-11-05: "one summary tab which displays the aggregates of all links"). Contract: header of
// src/test/dashboardUi.tsx. The tests below pin, per the spec:
//   - the Summary tab is first, titled "Summary", selected by default; the link tabs keep titles and badges;
//   - radios Traffic rates / Delay histogram / Packet loss, "Delay histogram" disabled with the tooltip
//     "Delay histograms are per link; select a link tab.", and nothing in the Summary polls getDelayHistogram;
//   - Rate: RX and TX in Gbit/s summed per t_ms over the times at which every link has a sample;
//   - Loss (spec 43 F1, was "Model loss %" / "Overload drops %" weighted by rx_mpps): one line per cause of the donut, in its order, each
//     the plain sum over the links of the cumulative count at the sample time;
//   - donut: counters (live) or totals (ended) summed per cause; legend, hover and "No drops" as for a link;
//   - shown whenever a link tab is shown (also after a stop), absent without link tabs; the selection persists across polls.
// CHOICE (spec silent): the spec says RX and TX are summed "in both Gbit/s and Mpps, like the per-link chart", but the per-link chart
// plots Gbit/s only; the Mpps sums are not observable in a chart's data and are pinned only through the weights of the loss view.
// CHOICE: when the selected link tab disappears, the first tab (the Summary) is selected (the old "first tab" rule).

const SUMMARY_TEXT = 'Delay histograms are per link; select a link tab.';

const sample = (t: number, over: Partial<HistorySample> = {}) => fx.historySample(t, over);

interface LineData {
  labels: string[];
  datasets: { label: string; data: number[]; borderColor?: string }[];
}
const lineOf = (panel: HTMLElement): LineData =>
  JSON.parse(within(panel).getByTestId('chart-line').getAttribute('data-chart') ?? 'null');
const dataset = (panel: HTMLElement, label: string) =>
  must(
    lineOf(panel).datasets.find((s) => s.label === label),
    `dataset ${label}`,
  ).data;

function serveRunning(dirs: DirectionStats[] = [fx.dirStats(0), fx.dirStats(1)]) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats(dirs));
}

async function open(samples: Record<string, HistorySample[]>, dirs?: DirectionStats[]) {
  serveRunning(dirs);
  serveHistory({ startedAt: 1762000000, samples });
  renderApp('/');
  await findTabs(dirs?.length ?? 2);
}

/** Like lossDonut.test.tsx: calls the tooltip label callback of the donut stub for segment `i`. */
function hoverText(panel: HTMLElement, i: number): string {
  const props = propsOf(within(panel).getByTestId('chart-doughnut'));
  const data = props.data as { labels: string[]; datasets: { data: number[] }[] };
  const options = props.options as {
    plugins?: { tooltip?: { callbacks?: { label?: (c: unknown) => string | string[] } } };
  };
  const label = must(options.plugins?.tooltip?.callbacks?.label, 'plugins.tooltip.callbacks.label');
  const value = data.datasets[0].data[i];
  const out = label({
    label: data.labels[i],
    parsed: value,
    raw: value,
    formattedValue: String(value),
    dataIndex: i,
    datasetIndex: 0,
    dataset: data.datasets[0],
    chart: { data },
  });
  return Array.isArray(out) ? out.join(' ') : out;
}
const donutValues = (panel: HTMLElement) =>
  JSON.parse(within(panel).getByTestId('chart-doughnut').getAttribute('data-chart') ?? 'null').datasets[0]
    .data as number[];

const counters = (over: Partial<Counters>) =>
  fx.counters({
    loss_drops: 0,
    limiter_drops: 0,
    buffer_full_drops: 0,
    tx_drops: 0,
    nic_rx_imissed: 0,
    nic_rx_nombuf: 0,
    ...over,
  });

// ---- position, default, visibility --------------------------------------------------------------------------------------------

describe('Summary tab (spec 36 S1): position, default and visibility', () => {
  it('is the first tab, titled "Summary" with no badge, before the link tabs which keep their titles', async () => {
    await open({ '0': [sample(1000)], '1': [sample(1000)] });
    const all = allTabs();
    expect(all).toHaveLength(3);
    expect(all[0]).toBe(summaryTab());
    expect(all[0]).toHaveTextContent(/^Summary$/);
    await waitFor(() => expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1'));
    expect(tabTitle(1)).toBe('b-a: enp101s0f1 → enp101s0f0');
    expect(all[1]).toBe(tab(0));
    expect(all[2]).toBe(tab(1));
    expect(tabBadge(0)).toBe('');
    expect(screen.getAllByRole('tablist')).toHaveLength(1);
  });

  it('is selected by default, no link tab is, and its panel is the one that shows', async () => {
    await open({ '0': [sample(1000)], '1': [sample(1000)] });
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    expect(tab(0)).toHaveAttribute('aria-selected', 'false');
    expect(tab(1)).toHaveAttribute('aria-selected', 'false');
    expect(activePanel()).toBe(panelOf(summaryTab()));
  });

  it('a single link still gets a Summary tab, after the same default', async () => {
    await open({ '0': [sample(1000)] }, [fx.dirStats(0)]);
    expect(allTabs()).toHaveLength(2);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
  });

  it('with no stored link and no run there is only the Summary tab (spec 39): no link tab, one tablist, one tabpanel', async () => {
    vi.useFakeTimers();
    api.getStats.mockResolvedValue(fx.stats([]));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(hasSummary()).toBe(true);
    expect(allTabs()).toHaveLength(1);
    expect(tabs()).toHaveLength(0);
    expect(screen.getAllByRole('tablist')).toHaveLength(1);
    expect(screen.getAllByRole('tabpanel', { hidden: true })).toHaveLength(1);
  });

  it('after a stop the Summary tab stays, first, with the link tabs badged "Stopped" and the Summary tab not', async () => {
    vi.useFakeTimers();
    serveRunning();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    renderApp('/');
    await tick(0);
    await tick(0);
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    await tick(2500);
    expect(allTabs()[0]).toBe(summaryTab());
    expect(summaryTab()).toHaveTextContent(/^Summary$/);
    expect(tabs()).toHaveLength(2);
    expect(tabBadge(0)).toBe('Stopped');
    expect(tabBadge(1)).toBe('Stopped');
    expect(summaryTab().querySelector('.badge')).toBeNull();
  });

  it('the selection persists across polls: a chosen link tab stays, and so does the Summary when chosen again', async () => {
    vi.useFakeTimers();
    serveRunning();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    await tick(3000);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true'); // not reset to anything by the polls
    fireEvent.click(tab(1)); // fireEvent: userEvent waits on timers that are faked here
    await tick(3000);
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    fireEvent.click(summaryTab());
    await tick(3000);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    expect(tab(1)).toHaveAttribute('aria-selected', 'false');
  });

  it('the Summary tab stays selected when the run ends', async () => {
    vi.useFakeTimers();
    serveRunning();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    renderApp('/');
    await tick(0);
    await tick(0);
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    await tick(2500);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    expect(within(activePanel()).getByTestId('chart-line')).toBeInTheDocument();
  });
});

// ---- radios -------------------------------------------------------------------------------------------------------------------

describe('Summary tab (spec 36 S1): the radios', () => {
  const names = () =>
    within(summary())
      .getAllByRole('radio')
      .map((r) => (r as HTMLInputElement).labels?.[0]?.textContent?.trim() ?? '');

  it('are Rate, Delay histogram, Loss in this order, in the radiogroup "Chart", Rate checked', async () => {
    await open({ '0': [sample(1000)], '1': [sample(1000)] });
    expect(names()).toEqual(['Traffic rates', 'Delay histogram', 'Packet loss']);
    expect(within(summary()).getByRole('radiogroup', { name: 'Chart' })).toBeInTheDocument();
    expect(summaryRadio('Traffic rates')).toBeChecked();
    expect(summaryRadio('Packet loss')).not.toBeChecked();
  });

  it('"Delay histogram" is disabled even though every link has delay statistics; Rate and Loss are enabled', async () => {
    await open({ '0': [sample(1000)], '1': [sample(1000)] });
    expect(summaryRadio('Delay histogram')).toBeDisabled();
    expect(summaryRadio('Traffic rates')).toBeEnabled();
    expect(summaryRadio('Packet loss')).toBeEnabled();
  });

  it('"Delay histogram" is there and disabled also when no link has delay statistics', async () => {
    await open({ '0': [sample(1000)] }, [fx.dirStats(0, { delay_stats: false })]);
    expect(names()).toEqual(['Traffic rates', 'Delay histogram', 'Packet loss']);
    expect(summaryRadio('Delay histogram')).toBeDisabled();
  });

  it('hovering "Delay histogram" shows the tooltip "Delay histograms are per link; select a link tab."', async () => {
    await open({ '0': [sample(1000)], '1': [sample(1000)] });
    const user = userEvent.setup();
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    await user.hover(within(summary()).getByText('Delay histogram'));
    expect(await screen.findByRole('tooltip')).toHaveTextContent(SUMMARY_TEXT);
    expect(screen.getByRole('tooltip').textContent?.trim()).toBe(SUMMARY_TEXT);
    await user.unhover(within(summary()).getByText('Delay histogram'));
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });

  it('clicking the disabled "Delay histogram" changes nothing and requests no histogram', async () => {
    await open({ '0': [sample(1000)], '1': [sample(1000)] });
    fireEvent.click(summaryRadio('Delay histogram'));
    expect(summaryRadio('Traffic rates')).toBeChecked();
    expect(within(summary()).queryByTestId('chart-bar')).not.toBeInTheDocument();
    expect(api.getDelayHistogram).not.toHaveBeenCalled();
  });

  it('Loss switches the chart to the loss series and Rate back; the choice survives polls', async () => {
    vi.useFakeTimers();
    serveRunning();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(lineOf(summary()).datasets.map((s) => s.label)).toEqual(['RX', 'TX']);
    fireEvent.click(summaryRadio('Packet loss'));
    expect(summaryRadio('Packet loss')).toBeChecked();
    expect(lineOf(summary()).datasets.map((s) => s.label)).toEqual([...CAUSES]);
    expect(must(within(summary()).getAllByRole('img')[0]).getAttribute('aria-label')).toMatch(/loss/i);
    await tick(3000);
    expect(summaryRadio('Packet loss')).toBeChecked();
    fireEvent.click(summaryRadio('Traffic rates'));
    expect(lineOf(summary()).datasets.map((s) => s.label)).toEqual(['RX', 'TX']);
    expect(must(within(summary()).getAllByRole('img')[0]).getAttribute('aria-label')).toMatch(/rate/i);
  });

  it('the order inside the panel is chart, radiogroup, donut', async () => {
    await open({ '0': [sample(1000)], '1': [sample(1000)] });
    const panel = summary();
    const chart = within(panel).getByTestId('chart-line');
    const group = within(panel).getByRole('radiogroup', { name: 'Chart' });
    const donut = within(panel).getByTestId('chart-doughnut');
    expect(chart.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(group.compareDocumentPosition(donut) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe('Summary tab (spec 36 S1): no delay histogram polling', () => {
  it('while the Summary shows no getDelayHistogram call is made, not by the first poll nor by later ones', async () => {
    vi.useFakeTimers();
    serveRunning();
    renderApp('/');
    await tick(0);
    await tick(0);
    await tick(6000);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    expect(api.getDelayHistogram).not.toHaveBeenCalled();
  });

  it('back from a link tab with its histogram open, the polling stops again', async () => {
    vi.useFakeTimers();
    serveRunning();
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Delay histogram' }));
    await tick(2500);
    expect(api.getDelayHistogram.mock.calls.length).toBeGreaterThanOrEqual(2);
    fireEvent.click(summaryTab());
    await tick(0);
    const before = api.getDelayHistogram.mock.calls.length;
    await tick(5000);
    expect(api.getDelayHistogram.mock.calls.length).toBe(before);
    expect(within(summary()).queryByTestId('chart-bar')).not.toBeInTheDocument();
  });
});

// ---- rate view ----------------------------------------------------------------------------------------------------------------

describe('Summary tab (spec 36 S1): the rate chart', () => {
  it('sums RX and TX (Gbit/s) of all links per sample time, labelled like the per-link chart', async () => {
    await open({
      '0': [sample(1000, { rx_gbps: 1.5, tx_gbps: 1.25 }), sample(2000, { rx_gbps: 2.5, tx_gbps: 0.25 })],
      '1': [sample(1000, { rx_gbps: 7.5, tx_gbps: 0.5 }), sample(2000, { rx_gbps: 4, tx_gbps: 2 })],
    });
    await waitFor(() => expect(dataset(summary(), 'RX')).toHaveLength(2));
    expect(dataset(summary(), 'RX')).toEqual([9, 6.5]);
    expect(dataset(summary(), 'TX')).toEqual([1.75, 2.25]);
    expect(lineOf(summary()).labels).toEqual([formatElapsed(1000), formatElapsed(2000)]);
    expect(lineOf(card(0)).labels).toEqual(lineOf(summary()).labels); // same time axis as a link's chart
  });

  it('covers only the times at which every link has a sample', async () => {
    await open({
      '0': [sample(1000, { rx_gbps: 1 }), sample(2000, { rx_gbps: 2 }), sample(3000, { rx_gbps: 3 })],
      '1': [sample(2000, { rx_gbps: 10 }), sample(3000, { rx_gbps: 20 }), sample(4000, { rx_gbps: 30 })],
    });
    await waitFor(() => expect(dataset(summary(), 'RX')).toHaveLength(2));
    expect(dataset(summary(), 'RX')).toEqual([12, 23]);
    expect(lineOf(summary()).labels).toEqual([formatElapsed(2000), formatElapsed(3000)]);
  });

  it('is empty while one link has no sample at all', async () => {
    await open({ '0': [sample(1000), sample(2000)], '1': [] });
    await waitFor(() => expect(lineOf(card(0)).labels).toHaveLength(2)); // the history has arrived
    expect(lineOf(summary()).labels).toEqual([]);
    expect(dataset(summary(), 'RX')).toEqual([]);
  });

  it('with three links sums all three', async () => {
    const dirs = [fx.dirStats(0), fx.dirStats(1), fx.dirStats(2, { link: 'c', rx: 'net_null0', tx: 'net_null1' })];
    await open(
      { '0': [sample(1000, { rx_gbps: 1 })], '1': [sample(1000, { rx_gbps: 2 })], '2': [sample(1000, { rx_gbps: 4 })] },
      dirs,
    );
    await waitFor(() => expect(dataset(summary(), 'RX')).toEqual([7]));
  });

  it('follows the next poll of the history', async () => {
    vi.useFakeTimers();
    const server = {
      startedAt: 1762000000,
      samples: { '0': [sample(1000, { rx_gbps: 1 })], '1': [sample(1000, { rx_gbps: 2 })] } as Record<
        string,
        HistorySample[]
      >,
    };
    serveRunning();
    serveHistory(server);
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(dataset(summary(), 'RX')).toEqual([3]);
    server.samples = {
      '0': [sample(1000, { rx_gbps: 1 }), sample(2000, { rx_gbps: 5 })],
      '1': [sample(1000, { rx_gbps: 2 }), sample(2000, { rx_gbps: 6 })],
    };
    serveHistory(server);
    await tick(2500);
    expect(dataset(summary(), 'RX')).toEqual([3, 11]);
  });

  it('after a stop it shows the history of the ended run, summed', async () => {
    vi.useFakeTimers();
    serveHistory({
      startedAt: 1762000000,
      samples: {
        '0': [sample(1000, { rx_gbps: 1.5 }), sample(2000, { rx_gbps: 2.5 })],
        '1': [sample(1000, { rx_gbps: 7.5 }), sample(2000, { rx_gbps: 0.5 })],
      },
    });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    renderApp('/');
    await tick(0);
    await tick(1000);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    expect(dataset(summary(), 'RX')).toEqual([9, 3]);
  });

  it('while the emulation is starting it plots no sample, like the link charts', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    serveHistory({ startedAt: 1762000000, samples: { '0': [sample(1000)], '1': [sample(1000)] } });
    renderApp('/');
    await findTabs(2);
    await waitFor(() => expect(api.getStatsHistory).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 50)); // let the history reply land
    expect(lineOf(card(0)).labels).toEqual([]); // the premise: the link chart is empty while starting
    expect(lineOf(summary()).labels).toEqual([]);
  });
});

// ---- loss view ----------------------------------------------------------------------------------------------------------------

describe('Summary tab (spec 36 S1, spec 43 F1): the loss chart', () => {
  const counts = (a: number, b = 0, c = 0, d = 0, e = 0, f = 0): Partial<HistorySample> => ({
    loss_drops: a,
    limiter_drops: b,
    buffer_full_drops: c,
    nic_tx_drops: d,
    nic_rx_imissed: e,
    nic_rx_nombuf: f,
  });
  const show = async (history: Record<string, HistorySample[]>) => {
    await open(history);
    fireEvent.click(summaryRadio('Packet loss'));
  };

  it('has six lines in the order, with the labels, and the colours of the donut', async () => {
    await show({ '0': [sample(1000)], '1': [sample(1000)] });
    const chart = lineOf(summary());
    expect(chart.datasets.map((s) => s.label)).toEqual([...CAUSES]);
    const donut = JSON.parse(within(summary()).getByTestId('chart-doughnut').getAttribute('data-chart') ?? 'null');
    expect(chart.datasets.map((s) => s.borderColor)).toEqual(donut.datasets[0].backgroundColor);
  });

  it('sums the links counts per cause, plainly (no weighting by rx_mpps)', async () => {
    await show({
      '0': [sample(1000, { rx_mpps: 3, ...counts(4, 1, 2, 3, 4, 5) })],
      '1': [sample(1000, { rx_mpps: 1, ...counts(8, 10, 20, 30, 40, 50) })],
    });
    await waitFor(() => expect(dataset(summary(), 'Loss model')).toHaveLength(1));
    const at = (label: string) => dataset(summary(), label)[0];
    expect(CAUSES.map(at)).toEqual([12, 11, 22, 33, 44, 55]);
  });

  it('the y axis is titled "Dropped packets" and starts at zero', async () => {
    await show({ '0': [sample(1000)], '1': [sample(1000)] });
    const options = JSON.parse(within(summary()).getByTestId('chart-line').getAttribute('data-options') ?? 'null');
    expect(options.scales.y.title.text).toBe('Dropped packets');
    expect(options.scales.y.beginAtZero === true || options.scales.y.min === 0).toBe(true);
  });

  it('is 0 (not NaN) for a link that dropped nothing and a rx_mpps of 0', async () => {
    await show({
      '0': [sample(1000, { rx_mpps: 0, ...counts(0) })],
      '1': [sample(1000, { rx_mpps: 0, ...counts(0) })],
    });
    await waitFor(() => expect(dataset(summary(), 'Loss model')).toHaveLength(1));
    for (const label of CAUSES) expect(dataset(summary(), label)).toEqual([0]);
  });

  it('is computed per sample time and over the common times only', async () => {
    await show({
      '0': [sample(1000, counts(10)), sample(2000, counts(20)), sample(3000, counts(99))],
      '1': [sample(1000, counts(2)), sample(2000, counts(5))],
    });
    await waitFor(() => expect(dataset(summary(), 'Loss model')).toHaveLength(2));
    expect(dataset(summary(), 'Loss model')).toEqual([12, 25]);
    expect(lineOf(summary()).labels).toEqual([formatElapsed(1000), formatElapsed(2000)]);
  });
});

// ---- donut --------------------------------------------------------------------------------------------------------------------

describe('Summary tab (spec 36 S1): the loss donut', () => {
  const dirs = (a: Partial<Counters>, b: Partial<Counters>) => [
    fx.dirStats(0, { counters: counters(a) }),
    fx.dirStats(1, { counters: counters(b) }),
  ];

  it('live: the counters of all links summed per cause, in the order of the causes', async () => {
    serveRunning(
      dirs(
        {
          rx_pkts: 10000,
          loss_drops: 40,
          limiter_drops: 30,
          buffer_full_drops: 20,
          tx_drops: 5,
          nic_rx_imissed: 4,
          nic_rx_nombuf: 1,
        },
        { rx_pkts: 500, loss_drops: 2, buffer_full_drops: 9, nic_rx_nombuf: 3 },
      ),
    );
    renderApp('/');
    await findTabs(2);
    const data = JSON.parse(within(summary()).getByTestId('chart-doughnut').getAttribute('data-chart') ?? 'null');
    expect(data.labels).toEqual([...CAUSES]);
    expect(data.datasets).toHaveLength(1);
    expect(data.datasets[0].data).toEqual([42, 30, 29, 5, 4, 4]);
  });

  it('live: the link donuts stay the links', async () => {
    serveRunning(dirs({ loss_drops: 40 }, { loss_drops: 2 }));
    renderApp('/');
    await findTabs(2);
    expect(donutValues(card(0))).toEqual([40, 0, 0, 0, 0, 0]);
    expect(donutValues(card(1))).toEqual([2, 0, 0, 0, 0, 0]);
    expect(donutValues(summary())).toEqual([42, 0, 0, 0, 0, 0]);
  });

  it('live: the legend lists the six causes with the explanation tooltips', async () => {
    serveRunning(dirs({ loss_drops: 40 }, { loss_drops: 2 }));
    renderApp('/');
    await findTabs(2);
    const legend = within(summary()).getByTestId('donut-legend');
    const items = within(legend).getAllByRole('listitem');
    expect(items).toHaveLength(6);
    CAUSES.forEach((cause, i) => {
      expect(items[i], cause).toHaveTextContent(cause);
    });
    const user = userEvent.setup();
    await user.hover(within(legend).getByRole('button', { name: 'About Rate limiter' }));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe(CAUSE_HELP['Rate limiter']);
  });

  it('live: hovering a segment says "<cause>: <count> packets (<share> % of drops)" of the summed counts', async () => {
    serveRunning(dirs({ loss_drops: 30 }, { loss_drops: 10, limiter_drops: 60 }));
    renderApp('/');
    await findTabs(2);
    const text = hoverText(summary(), 0);
    expect(text).toContain('Loss model');
    expect(text).toContain('40 packets');
    expect(text).toContain('40.00 %'); // 40 of 100 summed drops
  });

  it('live, no drop on any link: one grey ring, "No drops", and the six-cause legend', async () => {
    serveRunning(dirs({}, {}));
    renderApp('/');
    await findTabs(2);
    const panel = summary();
    const data = JSON.parse(within(panel).getByTestId('chart-doughnut').getAttribute('data-chart') ?? 'null');
    expect(data.datasets).toHaveLength(1);
    expect(data.datasets[0].data).toHaveLength(1);
    expect(data.datasets[0].data[0]).toBeGreaterThan(0);
    expect(within(panel).getByText('No drops')).toBeInTheDocument();
    expect(within(within(panel).getByTestId('donut-legend')).getAllByRole('listitem')).toHaveLength(6);
    expect(hoverText(panel, 0)).toContain('No drops');
    expect(hoverText(panel, 0)).not.toMatch(/NaN|Infinity/);
  });

  it('with drops on one link only there is no "No drops" in the Summary', async () => {
    serveRunning(dirs({}, { loss_drops: 1 }));
    renderApp('/');
    await findTabs(2);
    expect(within(summary()).queryByText('No drops')).not.toBeInTheDocument();
    expect(donutValues(summary())).toEqual([1, 0, 0, 0, 0, 0]);
  });

  it('ended: the totals of all links summed per cause (loss, limiter, buffer, NIC TX, imissed, nombuf)', async () => {
    vi.useFakeTimers();
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) },
      totals: {
        '0': fx.totals(), // 40, 30, 20, 5, 4, 1
        '1': fx.totals({
          loss_drops: 2,
          limiter_drops: 0,
          buffer_full_drops: 1,
          nic_tx_drops: 0,
          nic_rx_imissed: 0,
          nic_rx_nombuf: 3,
        }),
      },
    });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    renderApp('/');
    await tick(0);
    await tick(1000);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    expect(donutValues(summary())).toEqual([42, 30, 21, 5, 4, 4]);
  });

  it('ended with totals of 0 on every link: the grey ring and "No drops"', async () => {
    vi.useFakeTimers();
    const zero = fx.totals({
      loss_drops: 0,
      limiter_drops: 0,
      buffer_full_drops: 0,
      nic_tx_drops: 0,
      nic_rx_imissed: 0,
      nic_rx_nombuf: 0,
    });
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) },
      totals: { '0': zero, '1': zero },
    });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    renderApp('/');
    await tick(0);
    await tick(1000);
    expect(within(summary()).getByText('No drops')).toBeInTheDocument();
    expect(donutValues(summary())).toHaveLength(1);
  });
});
