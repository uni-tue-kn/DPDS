import { fireEvent, waitFor, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import { card, findCard } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api } from '../test/mockApi';

// Spec 35 F4 (user 2026-11-05) removes the histogram extras of spec 8 from the dashboard: the window select, the log-scale
// toggle, the expected-distribution overlay, the percentile list and the under/overflow text. Removed from this file, with the
// reason: 'window selector' (the histogram is always requested for the whole run, `getDelayHistogram(dir)` with no window),
// 'percentiles' (the list p50..p99.9 with clamp markers; the dashed annotation lines stay and are pinned in
// dashboardP4tg.test.tsx), 'log scale' (the y axis is linear), 'expected-distribution overlay' (one dataset, the bars) and
// 'chart registration' (LineController was needed for the bar + line overlay; the line charts register their own).
// The mean delay and jitter now sit in the chart's title line (dashboardP4tg.test.tsx).
// What stays here: the one-argument requests, that the chart follows the reply, and that an older controller's reply (without
// the optional extras) still renders.

interface ChartData {
  labels: string[];
  datasets: { type?: string; label?: string; data: number[] }[];
}

const bar = (d: number) => within(card(d)).getByTestId('chart-bar');
const chartData = (d: number): ChartData => JSON.parse(bar(d).getAttribute('data-chart') ?? 'null');

async function ready(d = 0) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
  renderApp('/');
  await findCard(1);
  for (const link of [0, 1]) fireEvent.click(within(card(link)).getByRole('radio', { name: 'Delay histogram' }));
  await within(card(d)).findByTestId('chart-bar');
}

describe('Dashboard histogram: no window, no log scale, no overlay, no percentile list (spec 35 F4)', () => {
  it('has none of the removed controls', async () => {
    await ready();
    expect(within(card(0)).queryByLabelText('Histogram window')).not.toBeInTheDocument();
    expect(within(card(0)).queryByLabelText('Log scale')).not.toBeInTheDocument();
    expect(within(card(0)).queryByLabelText('Show expected distribution')).not.toBeInTheDocument();
    expect(within(card(0)).queryByTestId('percentiles')).not.toBeInTheDocument();
    for (const text of ['Since start', '300 s', 'Underflow', 'Overflow', 'p99.9']) {
      expect(within(card(0)).queryByText(new RegExp(text)), text).not.toBeInTheDocument();
    }
  });

  it('the requests are always the one-argument form, the first and every later poll', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Delay histogram' }));
    await tick(0);
    await tick(5000);
    expect(api.getDelayHistogram.mock.calls.filter(([d]) => d === 0).length).toBeGreaterThanOrEqual(3);
    for (const call of api.getDelayHistogram.mock.calls) expect(call).toEqual([call[0]]);
  });

  it('the chart shows exactly the bars of the reply, one dataset, no expected line', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) => fx.histogram(dir));
    await ready();
    const data = chartData(0);
    expect(data.datasets).toHaveLength(1);
    expect(data.datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    expect(data.datasets.some((s) => s.type === 'line')).toBe(false);
    expect(data.labels).toHaveLength(5);
  });

  it('each link shows the bins of its own reply', async () => {
    // A link whose tab is not shown does not render its histogram: read each link's chart while its tab shows.
    await ready(0);
    expect(chartData(0).datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    card(1);
    await within(card(1)).findByTestId('chart-bar');
    expect(chartData(1).datasets[0].data).toEqual([1, 10, 100, 40, 3]);
  });

  it('the chart follows the next reply', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Delay histogram' }));
    await tick(0);
    expect(chartData(0).datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    api.getDelayHistogram.mockImplementation(async (dir) => fx.histogram(dir, { bins: [9, 9, 9, 9, 9] }));
    await tick(2500);
    expect(chartData(0).datasets[0].data).toEqual([9, 9, 9, 9, 9]);
  });

  it('a controller without the optional extras still renders the bars (no percentiles, no expected curve, no crash)', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) => {
      const h: Partial<HistogramView> = fx.histogram(dir);
      for (const k of [
        'percentiles',
        'count',
        'expected',
        'expected_underflow',
        'expected_overflow',
        'expected_model',
        'window_s',
        'span_ms',
      ] as const)
        delete h[k];
      return h as HistogramView;
    });
    await ready();
    expect(chartData(0).datasets).toHaveLength(1);
    expect(chartData(0).datasets[0].data).toEqual([1, 10, 100, 40, 2]);
  });

  it('a reply with an expected curve does not add it to the chart (the overlay is gone)', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { expected: [2, 12.5, 90.25, 45, 5.5], expected_model: 'exact' }),
    );
    await ready();
    await waitFor(() => expect(chartData(0).datasets).toHaveLength(1));
  });
});
