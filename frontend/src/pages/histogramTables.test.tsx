import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import { formatCount, formatDuration } from '../common/format';
import { card, findTabs, radio, summary } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api } from '../test/mockApi';
import { must } from '../test/must';

// Spec 36 S2 (user 2026-11-05: the histogram's numbers in rows above the diagram, "displayed like the table in P4TG"). Contract:
// the header of src/test/dashboardUi.tsx. In a link's "Delay histogram" view, above the bar chart, one `.row` with three
// `.col-12.col-md-4` columns, each a `table.table-striped.table-bordered.table-hover.table-sm` with a `thead.table-dark`, one header
// row (whose hover tooltip is pinned below) and exactly one body row:
//   1. "Histogram config":   Bin Range | #Bins | Bin Width  ->  [lo, lo + bins x bin_ns] | bins.length | bin_ns
//   2. "Values calculated from histogram data":  Delay (overlined) | σ(Delay) | Outlier | Total #Packets
//        ->  delay_mean_ns | delay_std_ns | underflow + overflow | count (else Σbins + underflow + overflow)
//   3. "Percentiles":       p50 | p90 | p99 | p99.9  ->  each percentile or "-"
// No Dir column (user 2026-11-05: "Remove the direction in the histogram table").
// Durations by formatDuration, counts by formatCount (expectations are built with the same functions, one literal spot check below).
// The chart title line "Mean ... · Jitter (σ) ..." is gone; the percentile annotation lines stay (dashboardP4tg.test.tsx).
// CHOICE: "Delay" is overlined by an inline `text-decoration: overline` on the header cell or an element inside it.
// CHOICE: "-" (spec: "means not available") also stands for a missing mean or σ, not formatDuration's "n/a".
// CHOICE: no tables while the histogram is unavailable (the error text shows instead).

const tablesOf = (d: number) => within(card(d)).getAllByRole('table');
const headers = (t: HTMLElement) =>
  within(t)
    .getAllByRole('columnheader')
    .map((h) => (h.textContent ?? '').trim());
const bodyRows = (t: HTMLElement) => within(t).getAllByRole('row').slice(1);
const cells = (t: HTMLElement) =>
  Array.from(must(bodyRows(t)[0], 'the body row').children).map((c) =>
    (c.textContent ?? '').replace(/\s+/g, ' ').trim(),
  );
const [CONFIG, VALUES, PERCENTILES] = [0, 1, 2];

async function ready(d = 0, dirs = [fx.dirStats(0), fx.dirStats(1)]) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats(dirs));
  renderApp('/');
  await findTabs(2);
  fireEvent.click(radio(d, 'Delay histogram'));
  await within(card(d)).findAllByRole('table');
}

describe('Histogram tables (spec 36 S2): layout', () => {
  it('three tables, in the order config, values, percentiles, with the headers of the spec', async () => {
    await ready();
    const tables = tablesOf(0);
    expect(tables).toHaveLength(3);
    expect(headers(must(tables[CONFIG]))).toEqual(['Bin Range', '#Bins', 'Bin Width']);
    expect(headers(must(tables[VALUES]))).toEqual(['Delay', 'σ(Delay)', 'Outlier', 'Total #Packets']);
    expect(headers(must(tables[PERCENTILES]))).toEqual(['p50', 'p90', 'p99', 'p99.9']);
  });

  it('each table has the header row and exactly one body row', async () => {
    await ready();
    for (const t of tablesOf(0)) expect(within(t).getAllByRole('row')).toHaveLength(2);
  });

  it('they sit in one row of three columns xs=12 md=4, above the bar chart', async () => {
    await ready();
    const tables = tablesOf(0);
    const cols = tables.map((t) => must(t.closest('.col-md-4'), 'a .col-md-4 around the table'));
    for (const c of cols) expect(c).toHaveClass('col-12', 'col-md-4');
    const row = must(cols[0].parentElement, 'the row');
    expect(row).toHaveClass('row');
    expect(cols.map((c) => c.parentElement)).toEqual([row, row, row]);
    expect(Array.from(row.children).filter((c) => c.classList.contains('col-md-4'))).toEqual(cols);
    const chart = within(card(0)).getByTestId('chart-bar');
    expect(row.compareDocumentPosition(chart) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(row.contains(chart)).toBe(false);
  });

  it('are P4TG-styled: striped, bordered, hover, small, mt-3 mb-3, with a dark thead', async () => {
    await ready();
    for (const t of tablesOf(0)) {
      expect(t).toHaveClass('table', 'table-striped', 'table-bordered', 'table-hover', 'table-sm', 'mt-3', 'mb-3');
      expect(must(t.querySelector('thead'))).toHaveClass('table-dark');
    }
  });

  it('"Delay" is overlined (the mean, like the overlined RTT of P4TG), the other headers are not', async () => {
    await ready();
    const th = must(within(must(tablesOf(0)[VALUES])).getAllByRole('columnheader')[0]);
    const styled = [th, ...Array.from(th.querySelectorAll('*'))].map((e) => e.getAttribute('style') ?? '');
    expect(styled.some((s) => /overline/.test(s))).toBe(true);
    const others = within(must(tablesOf(0)[VALUES]))
      .getAllByRole('columnheader')
      .filter((_, i) => i !== 0);
    for (const h of others) {
      expect(
        [h, ...Array.from(h.querySelectorAll('*'))].some((e) => /overline/.test(e.getAttribute('style') ?? '')),
      ).toBe(false);
    }
  });

  it('are only in the "Delay histogram" view of a link: not under Rate or Loss, not in the Summary', async () => {
    await ready();
    fireEvent.click(radio(0, 'Rate'));
    expect(within(card(0)).queryAllByRole('table')).toHaveLength(0);
    fireEvent.click(radio(0, 'Loss'));
    expect(within(card(0)).queryAllByRole('table')).toHaveLength(0);
    fireEvent.click(radio(0, 'Delay histogram'));
    expect(await within(card(0)).findAllByRole('table')).toHaveLength(3);
    expect(within(summary()).queryAllByRole('table')).toHaveLength(0);
  });

  it('are not shown when the histogram is unavailable', async () => {
    api.getDelayHistogram.mockRejectedValue(new Error('no telemetry'));
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    renderApp('/');
    await findTabs(2);
    fireEvent.click(radio(0, 'Delay histogram'));
    expect(await within(card(0)).findByTestId('histogram-error')).toBeInTheDocument();
    expect(within(card(0)).queryAllByRole('table')).toHaveLength(0);
  });
});

describe('Histogram tables (spec 36 S2): header tooltips', () => {
  it.each([
    [CONFIG, 'Histogram config'],
    [VALUES, 'Values calculated from histogram data'],
    [PERCENTILES, 'Percentiles'],
  ])('hovering the header of table %i shows "%s"', async (i, text) => {
    await ready();
    const user = userEvent.setup();
    const th = must(within(must(tablesOf(0)[i])).getAllByRole('columnheader')[0]);
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    await user.hover(th);
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe(text);
    await user.unhover(th);
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });
});

describe('Histogram tables (spec 36 S2): the cells', () => {
  it('config: [lo, lo + bins x bin_ns], the number of bins, the bin width', async () => {
    await ready(); // lo 900000 ns, 5 bins of 100000 ns
    expect(cells(must(tablesOf(0)[CONFIG]))).toEqual([
      `[${formatDuration(900000)}, ${formatDuration(1400000)}]`,
      formatCount(5),
      formatDuration(100000),
    ]);
  });

  it('config: literal check of the units ("[900.00 µs, 1.40 ms]", "5", "100.00 µs")', async () => {
    await ready();
    const [range, bins, width] = cells(must(tablesOf(0)[CONFIG]));
    expect(range).toMatch(/^\[900\.00 [µμ]s, 1\.40 ms\]$/);
    expect(bins).toBe('5');
    expect(width).toMatch(/^100\.00 [µμ]s$/);
  });

  it('config follows another lo, bin width and bin count (the upper end is lo + bins.length x bin_ns, not edges_ns)', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { lo_ns: 100, bin_ns: 250, bins: [1, 2, 3, 4], edges_ns: [100, 350, 600, 850, 1100] }),
    );
    await ready();
    expect(cells(must(tablesOf(0)[CONFIG]))).toEqual([
      `[${formatDuration(100)}, ${formatDuration(1100)}]`,
      '4',
      formatDuration(250),
    ]);
    expect(cells(must(tablesOf(0)[CONFIG]))[0]).toMatch(/^\[100\.00 ns, 1\.10 [µμ]s\]$/);
  });

  it('values: mean, σ from the link stats, outliers = underflow + overflow, count', async () => {
    await ready(); // delay_mean_ns 1000000, delay_std_ns 50000, underflow 3, overflow 7, count 163
    expect(cells(must(tablesOf(0)[VALUES]))).toEqual([
      formatDuration(1000000),
      formatDuration(50000),
      formatCount(10),
      formatCount(163),
    ]);
    expect(cells(must(tablesOf(0)[VALUES]))[0]).toBe('1.00 ms');
    expect(cells(must(tablesOf(0)[VALUES]))[1]).toMatch(/^50\.00 [µμ]s$/);
  });

  it('values: Total #Packets is Σbins + underflow + overflow when the reply has no count, with thousands separators', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) => {
      const h: Partial<HistogramView> = fx.histogram(dir, {
        bins: [1000, 2000, 3000, 4000, 5000],
        underflow: 1000,
        overflow: 234,
      });
      delete h.count;
      return h as HistogramView;
    });
    await ready();
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row[2]).toBe('1,234');
    expect(row[3]).toBe('16,234'); // 15,000 + 1,000 + 234
  });

  it('values: the count of the reply wins over the sum of the bins', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) => fx.histogram(dir, { count: 12345 }));
    await ready();
    expect(cells(must(tablesOf(0)[VALUES]))[3]).toBe('12,345');
  });

  it('values: no outliers and no packets read 0', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { bins: [0, 0, 0, 0, 0], underflow: 0, overflow: 0, count: 0 }),
    );
    await ready();
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row[2]).toBe('0');
    expect(row[3]).toBe('0');
  });

  it('values: a missing mean or σ reads "-"', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { delay_mean_ns: null, delay_std_ns: null }), fx.dirStats(1)]),
    );
    renderApp('/');
    await findTabs(2);
    fireEvent.click(radio(0, 'Delay histogram'));
    await within(card(0)).findAllByRole('table');
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row.slice(0, 2)).toEqual(['-', '-']);
  });

  it('percentiles: p50, p90, p99, p99.9', async () => {
    await ready();
    expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual([
      formatDuration(1050000),
      formatDuration(1150000),
      formatDuration(1250000),
      formatDuration(1290000),
    ]);
    expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual(['1.05 ms', '1.15 ms', '1.25 ms', '1.29 ms']);
  });

  it('percentiles: null reads "-" per percentile, the others keep their value', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { percentiles: { p50: 1050000, p90: null, p99: 1250000, p999: null } }),
    );
    await ready();
    expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual(['1.05 ms', '-', '1.25 ms', '-']);
  });

  it('percentiles: a reply without percentiles reads "-" four times', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) => {
      const h = fx.histogram(dir);
      delete h.percentiles;
      return h;
    });
    await ready();
    expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual(['-', '-', '-', '-']);
  });

  it('each link shows its own numbers, and no table has a direction', async () => {
    await ready(0, [fx.dirStats(0), fx.dirStats(1, { delay_mean_ns: 2_500_000_000, delay_std_ns: 800 })]);
    expect(within(card(0)).queryByText(/→/)).not.toBeInTheDocument();
    fireEvent.click(radio(1, 'Delay histogram'));
    await within(card(1)).findAllByRole('table');
    expect(cells(must(tablesOf(1)[VALUES]))).toEqual(['2.50 s', '800.00 ns', '10', '164']); // count 163 + dir
    for (const t of tablesOf(1)) expect(within(t).queryByText(/→/)).not.toBeInTheDocument();
  });

  it('follow the next poll of the histogram and of the stats', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'Delay histogram'));
    await tick(0);
    expect(cells(must(tablesOf(0)[VALUES]))[3]).toBe('163');
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, {
        count: 999,
        bins: [9, 9, 9, 9, 9],
        percentiles: { p50: 2_000_000, p90: null, p99: null, p999: null },
      }),
    );
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { delay_mean_ns: 3_000_000, delay_std_ns: 120_000 }), fx.dirStats(1)]),
    );
    await tick(2500);
    const values = cells(must(tablesOf(0)[VALUES]));
    expect(values[0]).toBe('3.00 ms');
    expect(values[1]).toMatch(/^120\.00 [µμ]s$/);
    expect(values[3]).toBe('999');
    expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual(['2.00 ms', '-', '-', '-']);
  });
});

describe('Histogram tables (spec 36 S2): the chart keeps its bars and loses its title line', () => {
  it('the bar chart is still there with the bars of the reply and its percentile annotation lines', async () => {
    await ready();
    const chart = within(card(0)).getByTestId('chart-bar');
    const data = JSON.parse(chart.getAttribute('data-chart') ?? 'null');
    expect(data.datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    const options = JSON.parse(chart.getAttribute('data-options') ?? 'null');
    expect(Object.keys(options.plugins?.annotation?.annotations ?? {})).toHaveLength(4);
  });

  it('has no "Mean ... · Jitter (σ) ..." title: not in the options, not in the page', async () => {
    await ready();
    const chart = within(card(0)).getByTestId('chart-bar');
    const options = JSON.parse(chart.getAttribute('data-options') ?? 'null');
    expect(options.plugins?.title?.display).not.toBe(true);
    expect(JSON.stringify(options.plugins?.title ?? {})).not.toMatch(/Mean|Jitter/);
    expect(within(card(0)).queryByText(/Jitter/)).not.toBeInTheDocument();
    expect(within(card(0)).queryByText(/^Mean /)).not.toBeInTheDocument();
  });
});
