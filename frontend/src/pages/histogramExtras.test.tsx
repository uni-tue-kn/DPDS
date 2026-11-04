import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import * as fx from '../test/fixtures';
import { api } from '../test/mockApi';
import { field, renderApp, tick } from '../test/helpers';
import { must } from '../test/must';

// Contract: "Dashboard histogram" in the v8 block of src/test/helpers.tsx. The Bar stub exposes the chart data as
// `data-chart` and the chart options as `data-options` (setup.ts).
interface ChartData {
  labels: string[];
  datasets: { type?: string; label?: string; data: number[] }[];
}
interface ChartOptions {
  scales?: { y?: { type?: string } };
  plugins?: { legend?: { display?: boolean } };
}

function twoDirections() {
  return fx.stats([fx.dirStats(0), fx.dirStats(1)]);
}

const card = (d: 0 | 1) => screen.getByTestId(`direction-card-${d}`);
const bar = (d: 0 | 1) => within(card(d)).getByTestId('chart-bar');
const chartData = (d: 0 | 1): ChartData => JSON.parse(bar(d).getAttribute('data-chart') ?? 'null');
const chartOptions = (d: 0 | 1): ChartOptions => JSON.parse(bar(d).getAttribute('data-options') ?? 'null');
const windowSelect = (d: 0 | 1) => within(card(d)).getByLabelText('Histogram window') as HTMLSelectElement;
const logToggle = (d: 0 | 1) => within(card(d)).getByLabelText('Log scale') as HTMLInputElement;
const overlayToggle = (d: 0 | 1) => within(card(d)).getByLabelText('Show expected distribution') as HTMLInputElement;
const percentiles = (d: 0 | 1) => within(card(d)).getByTestId('percentiles');

// Spec 28: the histogram is one choice of each card's radio selector (Rate is the default), so every test first picks
// "Delay histogram" in both cards. Was: the histogram was always on the page below the rate and drops charts.
function chooseHistogram(d: 0 | 1) {
  fireEvent.click(within(card(d)).getByRole('radio', { name: 'Delay histogram' }));
}

async function ready(d: 0 | 1 = 0) {
  renderApp('/');
  await screen.findByTestId(`direction-card-${d}`);
  await screen.findByTestId('direction-card-0');
  await screen.findByTestId('direction-card-1');
  chooseHistogram(0);
  chooseHistogram(1);
  await within(card(d)).findByTestId('chart-bar');
}

beforeEach(() => {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(twoDirections());
});

describe('Dashboard histogram: window selector', () => {
  it('offers Since start / 10 s / 60 s / 300 s with the window_s values, default since start', async () => {
    await ready();
    const opts = within(windowSelect(0)).getAllByRole('option');
    expect(opts.map((o) => o.textContent)).toEqual(['Since start', '10 s', '60 s', '300 s']);
    expect(opts.map((o) => (o as HTMLOptionElement).value)).toEqual(['0', '10', '60', '300']);
    expect(windowSelect(0)).toHaveValue('0');
  });

  it('every card has its own selector', async () => {
    await ready(1);
    expect(windowSelect(0)).toBeInTheDocument();
    expect(windowSelect(1)).toBeInTheDocument();
  });

  it('the initial fetches use the old one-argument form (since start)', async () => {
    await ready();
    expect(api.getDelayHistogram).toHaveBeenCalledWith(0);
    expect(api.getDelayHistogram.mock.calls.every(([, w]) => w === undefined || w === 0)).toBe(true);
  });

  it.each([10, 60, 300])('choosing %i s refetches that direction with window_s %i', async (n) => {
    const user = userEvent.setup();
    await ready();
    await user.selectOptions(windowSelect(0), String(n));
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalledWith(0, n));
    expect(windowSelect(0)).toHaveValue(String(n));
  });

  it('only the changed direction uses the window', async () => {
    const user = userEvent.setup();
    await ready(1);
    await user.selectOptions(windowSelect(0), '60');
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalledWith(0, 60));
    expect(api.getDelayHistogram.mock.calls.some(([d, w]) => d === 1 && w === 60)).toBe(false);
    expect(windowSelect(1)).toHaveValue('0');
  });

  it('choosing "Since start" again goes back to the one-argument form', async () => {
    const user = userEvent.setup();
    await ready();
    await user.selectOptions(windowSelect(0), '10');
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalledWith(0, 10));
    api.getDelayHistogram.mockClear();
    await user.selectOptions(windowSelect(0), '0');
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalled());
    const last = api.getDelayHistogram.mock.calls[api.getDelayHistogram.mock.calls.length - 1];
    expect(last[0]).toBe(0);
    expect(last[1] ?? 0).toBe(0);
  });

  it('later polls keep the chosen window and the selector keeps its value', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    renderApp('/');
    await tick(0);
    await tick(0);
    chooseHistogram(0);
    await tick(0);
    fireEvent.change(windowSelect(0), { target: { value: '60' } });
    await tick(0);
    api.getDelayHistogram.mockClear();
    await tick(5000);
    const calls0 = api.getDelayHistogram.mock.calls.filter(([d]) => d === 0);
    expect(calls0.length).toBeGreaterThanOrEqual(3);
    expect(calls0.every(([, w]) => w === 60)).toBe(true);
    expect(windowSelect(0)).toHaveValue('60');
    expect(within(card(0)).getByTestId('chart-bar')).toBeInTheDocument();
  });

  it('shows the data of the window (chart updates with the reply)', async () => {
    api.getDelayHistogram.mockImplementation(async (dir, w) =>
      fx.histogram(dir, w === 60 ? { bins: [9, 9, 9, 9, 9], window_s: 60, span_ms: 60000 } : {}),
    );
    const user = userEvent.setup();
    await ready();
    expect(chartData(0).datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    await user.selectOptions(windowSelect(0), '60');
    await waitFor(() => expect(chartData(0).datasets[0].data).toEqual([9, 9, 9, 9, 9]));
  });
});

describe('Dashboard histogram: percentiles', () => {
  it('shows p50, p90, p99 and p99.9 as durations under the chart', async () => {
    await ready();
    const p = percentiles(0);
    expect(field(p, 'p50')).toHaveTextContent('1.05 ms');
    expect(field(p, 'p90')).toHaveTextContent('1.15 ms');
    expect(field(p, 'p99')).toHaveTextContent('1.25 ms');
    expect(field(p, 'p99.9')).toHaveTextContent('1.29 ms');
  });

  it('unclamped values carry no clamp marker', async () => {
    await ready();
    for (const label of ['p50', 'p90', 'p99', 'p99.9']) {
      const dd = field(percentiles(0), label);
      expect(dd).not.toHaveAttribute('data-clamped');
      expect(dd.textContent).not.toMatch(/[≤≥]/);
    }
  });

  it("shows each direction's own percentiles", async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { percentiles: { p50: dir === 0 ? 1050000 : 2500000, p90: 1, p99: 2, p999: 3 } }),
    );
    await ready(1);
    await within(card(1)).findByTestId('percentiles');
    expect(field(percentiles(0), 'p50')).toHaveTextContent('1.05 ms');
    expect(field(percentiles(1), 'p50')).toHaveTextContent('2.50 ms');
  });

  it('marks a percentile that fell into the overflow with data-clamped=overflow and a ≥', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, {
        percentiles: { ...must(fx.histogram(dir).percentiles), p999: 1400000, p999_clamped: 'overflow' },
      }),
    );
    await ready();
    const dd = field(percentiles(0), 'p99.9');
    expect(dd).toHaveAttribute('data-clamped', 'overflow');
    expect(dd).toHaveTextContent('≥');
    expect(dd).toHaveTextContent('1.40 ms');
    expect(field(percentiles(0), 'p99')).not.toHaveAttribute('data-clamped');
  });

  it('marks a percentile that fell into the underflow with data-clamped=underflow and a ≤', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, {
        percentiles: { ...must(fx.histogram(dir).percentiles), p50: 900000, p50_clamped: 'underflow' },
      }),
    );
    await ready();
    const dd = field(percentiles(0), 'p50');
    expect(dd).toHaveAttribute('data-clamped', 'underflow');
    expect(dd).toHaveTextContent('≤');
    expect(dd).toHaveTextContent('900.00 µs');
  });

  it('prints n/a for percentiles without samples (null)', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, {
        bins: [0, 0, 0, 0, 0],
        underflow: 0,
        overflow: 0,
        count: 0,
        percentiles: { p50: null, p90: null, p99: null, p999: null },
      }),
    );
    await ready();
    for (const label of ['p50', 'p90', 'p99', 'p99.9']) expect(field(percentiles(0), label)).toHaveTextContent('n/a');
  });

  it('a controller without the extras still renders the bars (no percentiles, no overlay, no crash)', async () => {
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
    expect(within(card(0)).queryByTestId('percentiles')).not.toBeInTheDocument();
    expect(chartData(0).datasets).toHaveLength(1);
    expect(chartData(0).datasets[0].data).toEqual([1, 10, 100, 40, 2]);
  });
});

describe('Dashboard histogram: log scale', () => {
  it('is off by default (linear y axis)', async () => {
    await ready();
    expect(logToggle(0)).not.toBeChecked();
    expect(chartOptions(0).scales?.y?.type ?? 'linear').toBe('linear');
  });

  it('turns the y axis logarithmic and back', async () => {
    const user = userEvent.setup();
    await ready();
    await user.click(logToggle(0));
    expect(logToggle(0)).toBeChecked();
    expect(chartOptions(0).scales?.y?.type).toBe('logarithmic');
    await user.click(logToggle(0));
    expect(chartOptions(0).scales?.y?.type ?? 'linear').toBe('linear');
  });

  it('is per card and survives the next poll', async () => {
    const user = userEvent.setup();
    await ready(1);
    await within(card(1)).findByTestId('chart-bar');
    await user.click(logToggle(0));
    expect(chartOptions(1).scales?.y?.type ?? 'linear').toBe('linear');
    const before = api.getDelayHistogram.mock.calls.length;
    await waitFor(() => expect(api.getDelayHistogram.mock.calls.length).toBeGreaterThan(before), { timeout: 3000 });
    expect(logToggle(0)).toBeChecked();
    expect(chartOptions(0).scales?.y?.type).toBe('logarithmic');
  });

  it('keeps the bar data unchanged', async () => {
    const user = userEvent.setup();
    await ready();
    await user.click(logToggle(0));
    expect(chartData(0).datasets[0].data).toEqual([1, 10, 100, 40, 2]);
  });
});

describe('Dashboard histogram: expected-distribution overlay', () => {
  it('adds the expected counts as a line dataset after the bars', async () => {
    await ready();
    const data = chartData(0);
    expect(data.datasets).toHaveLength(2);
    expect(data.datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    expect(data.datasets[1].type).toBe('line');
    expect(data.datasets[1].data).toEqual([2, 12.5, 90.25, 45, 5.5]);
    expect(data.datasets[1].data).toHaveLength(data.labels.length);
  });

  it('names the exact model in the legend entry and shows the legend', async () => {
    await ready();
    const label = chartData(0).datasets[1].label ?? '';
    expect(label).toMatch(/expected/i);
    expect(label).toMatch(/exact/i);
    expect(label).not.toMatch(/normal approximation/i);
    expect(label).not.toMatch(/excludes/i);
    expect(chartOptions(0).plugins?.legend?.display).toBe(true);
  });

  it('names the normal approximation for correlated delays', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { expected_model: 'normal_approximation' }),
    );
    await ready();
    const label = chartData(0).datasets[1].label ?? '';
    expect(label).toMatch(/normal approximation/i);
    expect(label).not.toMatch(/\bexact\b/i);
  });

  it('adds the limiter note to the legend entry when the reply has one', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { expected_note: 'excludes limiter queueing delay' }),
    );
    await ready();
    const label = chartData(0).datasets[1].label ?? '';
    expect(label).toContain('excludes limiter queueing delay');
    expect(label).toMatch(/exact/i);
  });

  it('the toggle is on by default and removes the overlay (and the legend) when switched off', async () => {
    const user = userEvent.setup();
    await ready();
    expect(overlayToggle(0)).toBeChecked();
    await user.click(overlayToggle(0));
    expect(overlayToggle(0)).not.toBeChecked();
    expect(chartData(0).datasets).toHaveLength(1);
    expect(chartOptions(0).plugins?.legend?.display).toBeFalsy();
    await user.click(overlayToggle(0));
    expect(chartData(0).datasets).toHaveLength(2);
  });

  it('the toggle is per card', async () => {
    const user = userEvent.setup();
    await ready(1);
    await within(card(1)).findByTestId('chart-bar');
    await user.click(overlayToggle(0));
    expect(chartData(0).datasets).toHaveLength(1);
    expect(chartData(1).datasets).toHaveLength(2);
  });

  it('no overlay without an `expected` array in the reply', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) => {
      const h: Partial<HistogramView> = fx.histogram(dir);
      delete h.expected;
      delete h.expected_model;
      return h as HistogramView;
    });
    await ready();
    expect(chartData(0).datasets).toHaveLength(1);
  });

  it('log scale and overlay work together', async () => {
    const user = userEvent.setup();
    await ready();
    await user.click(logToggle(0));
    expect(chartOptions(0).scales?.y?.type).toBe('logarithmic');
    expect(chartData(0).datasets).toHaveLength(2);
  });

  it('the window selector keeps the overlay (expected is recomputed by the controller for the window)', async () => {
    api.getDelayHistogram.mockImplementation(async (dir, w) =>
      fx.histogram(dir, w === 10 ? { expected: [1, 1, 1, 1, 1], window_s: 10 } : {}),
    );
    const user = userEvent.setup();
    await ready();
    await user.selectOptions(windowSelect(0), '10');
    await waitFor(() => expect(chartData(0).datasets[1].data).toEqual([1, 1, 1, 1, 1]));
  });
});

describe('chart registration (mixed bar + line chart)', () => {
  it('components/charts.ts registers LineController, else a line dataset on a Bar chart throws in the browser', () => {
    const src = readFileSync(resolve(__dirname, '../components/charts.ts'), 'utf8');
    expect(src).toMatch(/\bLineController\b/);
  });
});
