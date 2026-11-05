import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import { formatCount, formatDuration } from '../common/format';
import {
  CHART_RADIOS,
  card,
  FRAME_DONUT_LABEL,
  findHistTables,
  findTabs,
  histTables,
  IAT_TABLE_HEADERS,
  iatBar,
  LOSS_DONUT_LABEL,
  lossTable,
  queryDelayBar,
  queryRadio,
  radio,
  radioNames,
  rateTable,
  showIat,
  summary,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedLinks, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 52 (user 2026-11-05: loss donut left, frame donut right, a central line, both centred; the IAT histogram becomes a chart radio
// "like P4TG's chart views", "shown like delay histogram"). Spec: .git/claude-specs/spec52-donut-row-iat-radio.md. Contract of the DOM:
// the header of src/test/dashboardUi.tsx. CHOICEs of the tester where the spec is silent:
//   - the separator class is `donut-divider` (on the left column; the rule is pinned in donutDividerCss.test.ts);
//   - "centred" is pinned as: between each donut's role "img" box and its column (both included) some element has one of the
//     Bootstrap centring classes (`text-center`, `justify-content-center`, `align-items-center`, `mx-auto`);
//   - the IAT tables carry the delay tables' hover texts ("Histogram config", "Values calculated from histogram data", "Percentiles");
//   - "no samples in the bins" reads "-" for the mean and σ (the delay tables read "-" for a missing value);
//   - mean and σ are not pinned to the sample or the population convention: the literal σ fixture has counts so large that both
//     agree to the two decimals shown.

const CENTRING = '.text-center, .justify-content-center, .align-items-center, .mx-auto';

function serveLive(...dirs: ReturnType<typeof fx.dirStats>[]) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats(dirs));
}

async function openLive() {
  serveLive(fx.dirStats(0), fx.dirStats(1));
  renderApp('/');
  await findTabs(2);
}

const follows = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
const iatOptions = () => JSON.parse(iatBar(card(0)).getAttribute('data-options') ?? 'null');
const iatData = () => JSON.parse(iatBar(card(0)).getAttribute('data-chart') ?? 'null');

// ---- R1: the donut row ------------------------------------------------------------------------------------------------------------

/** The two columns of the donut row of `panel`: the one with the loss donut and the one with the frame size donut. */
function donutCols(panel: HTMLElement) {
  const [loss, frame] = [LOSS_DONUT_LABEL, FRAME_DONUT_LABEL].map((name) => {
    const box = within(panel).getByRole('img', { name });
    return { box, col: must(box.closest('.col-md-6'), `the .col-md-6 around "${name}"`) as HTMLElement };
  });
  return { loss, frame };
}

const PANELS = ['a link tab', 'the Summary'] as const;
const panelOf = (which: (typeof PANELS)[number]) => (which === 'a link tab' ? card(0) : summary());

describe('Spec 52 R1: the donut row', () => {
  it.each(PANELS)(
    '%s: exactly two columns col-12 col-md-6, the loss donut left and the frame donut right',
    async (which) => {
      await openLive();
      const { loss, frame } = donutCols(panelOf(which));
      const row = must(loss.col.parentElement, 'the row');
      expect(row).toHaveClass('row');
      expect(frame.col.parentElement).toBe(row);
      expect(loss.col).toHaveClass('col-12', 'col-md-6');
      expect(frame.col).toHaveClass('col-12', 'col-md-6');
      expect(Array.from(row.children)).toEqual([loss.col, frame.col]);
    },
  );

  it('shows the same two columns before any run (placeholders), on the link tabs and the Summary', async () => {
    vi.useFakeTimers();
    seedLinks([fx.link(), fx.linkRev()]);
    renderApp('/');
    await tick(0);
    await tick(0);
    for (const d of [0, 1]) {
      const { loss, frame } = donutCols(card(d));
      expect(Array.from(must(loss.col.parentElement).children)).toEqual([loss.col, frame.col]);
    }
    const { loss, frame } = donutCols(summary());
    expect(Array.from(must(loss.col.parentElement).children)).toEqual([loss.col, frame.col]);
  });

  it.each(PANELS)(
    '%s: the separator class donut-divider is on the left column only, with no inline style',
    async (which) => {
      await openLive();
      const panel = panelOf(which);
      const { loss, frame } = donutCols(panel);
      expect(loss.col).toHaveClass('donut-divider');
      expect(frame.col).not.toHaveClass('donut-divider');
      for (const c of [loss.col, frame.col]) expect(c.getAttribute('style') ?? '').not.toMatch(/border/);
      expect(panel.querySelectorAll('.donut-divider')).toHaveLength(1); // one line, in the middle of the row
    },
  );

  it.each(PANELS)('%s: each donut is centred in its column', async (which) => {
    await openLive();
    const { loss, frame } = donutCols(panelOf(which));
    for (const { box, col } of [loss, frame]) {
      const centring = box.closest(CENTRING);
      expect(centring, 'a centring element around the donut').not.toBeNull();
      expect(col.contains(centring), 'inside the column').toBe(true);
    }
  });

  it('the legend of the loss donut and the frame legend stay in their own columns', async () => {
    await openLive();
    const { loss, frame } = donutCols(card(0));
    expect(within(loss.col).getByTestId('donut-legend')).toBeInTheDocument();
    expect(within(frame.col).queryByTestId('donut-legend')).not.toBeInTheDocument();
    expect(within(frame.col).getByText('0-63')).toBeInTheDocument(); // the frame size legend
    expect(within(loss.col).queryByText('0-63')).not.toBeInTheDocument();
  });
});

// ---- R2: the radios ------------------------------------------------------------------------------------------------------------------

const radiosOf = (panel: HTMLElement) =>
  within(panel)
    .getAllByRole('radio')
    .map((r) => (r as HTMLInputElement).labels?.[0]?.textContent?.trim());

describe('Spec 52 R2: the "IAT histogram" radio', () => {
  it('a live link with delay statistics has the four radios in order, IAT enabled like Delay histogram', async () => {
    await openLive();
    expect(radioNames(0)).toEqual(['Traffic rates', 'Delay histogram', 'IAT histogram', 'Packet loss']);
    expect(radioNames(1)).toEqual(CHART_RADIOS);
    expect(radio(0, 'Delay histogram')).toBeEnabled();
    expect(radio(0, 'IAT histogram')).toBeEnabled();
    expect(radio(0, 'Traffic rates')).toBeChecked();
    expect(radio(0, 'IAT histogram')).not.toBeChecked();
  });

  it('is in the same radiogroup "Chart" as the others', async () => {
    await openLive();
    const group = within(card(0)).getByRole('radiogroup', { name: 'Chart' });
    expect(radiosOf(group)).toEqual(CHART_RADIOS);
  });

  it('is absent, like Delay histogram, for a link without delay statistics', async () => {
    serveLive(fx.dirStats(0, { delay_stats: false }), fx.dirStats(1));
    renderApp('/');
    await findTabs(2);
    expect(radioNames(0)).toEqual(['Traffic rates', 'Packet loss']);
    expect(radioNames(1)).toEqual(CHART_RADIOS);
  });

  it('is there and disabled in the Summary, like Delay histogram', async () => {
    await openLive();
    expect(radiosOf(summary())).toEqual(CHART_RADIOS);
    expect(within(summary()).getByRole('radio', { name: 'Delay histogram' })).toBeDisabled();
    expect(within(summary()).getByRole('radio', { name: 'IAT histogram' })).toBeDisabled();
    expect(within(summary()).getByRole('radio', { name: 'Traffic rates' })).toBeEnabled();
    expect(within(summary()).getByRole('radio', { name: 'Packet loss' })).toBeEnabled();
    expect(within(summary()).queryByText(/IAT histograms are per link/)).not.toBeInTheDocument();
  });

  it('is there and disabled before any run, on the Summary and on every link tab, and nothing is fetched', async () => {
    vi.useFakeTimers();
    seedLinks([fx.link(), fx.linkRev()]);
    renderApp('/');
    await tick(0);
    await tick(0);
    for (const panel of [summary(), card(0), card(1)]) {
      expect(radiosOf(panel)).toEqual(CHART_RADIOS);
      expect(within(panel).getByRole('radio', { name: 'Delay histogram' })).toBeDisabled();
      expect(within(panel).getByRole('radio', { name: 'IAT histogram' })).toBeDisabled();
    }
    await tick(3000);
    expect(api.getIatHistogram).not.toHaveBeenCalled();
  });

  it('choosing it shows the IAT bar chart in a .chart-box, replaces the rate chart and keeps the donuts', async () => {
    await openLive();
    const panel = await showIat(0);
    expect(radio(0, 'IAT histogram')).toBeChecked();
    expect(radio(0, 'Traffic rates')).not.toBeChecked();
    const box = within(panel).getByRole('img', { name: 'IAT histogram of link 0' });
    expect(box).toHaveClass('chart-box');
    expect(within(box).getByTestId('chart-bar')).toBe(iatBar(panel));
    expect(iatData().datasets[0].data).toEqual(fx.iatHistogram(0).bins);
    expect(within(panel).queryByRole('img', { name: /^Rate chart/ })).not.toBeInTheDocument();
    expect(queryDelayBar(panel)).toBeNull();
    expect(within(panel).getByRole('img', { name: LOSS_DONUT_LABEL })).toBeInTheDocument();
    expect(within(panel).getByRole('img', { name: FRAME_DONUT_LABEL })).toBeInTheDocument();
  });

  it('choosing Traffic rates again removes the IAT chart', async () => {
    await openLive();
    await showIat(0);
    fireEvent.click(radio(0, 'Traffic rates'));
    expect(within(card(0)).queryByRole('img', { name: /^IAT/ })).not.toBeInTheDocument();
    expect(within(card(0)).getByRole('img', { name: /^Rate chart/ })).toBeInTheDocument();
  });

  it('is polled only while chosen: no getIatHistogram call in any other view', async () => {
    vi.useFakeTimers();
    serveLive(fx.dirStats(0), fx.dirStats(1));
    renderApp('/');
    await tick(0);
    await tick(0);
    await tick(2000);
    expect(api.getIatHistogram).not.toHaveBeenCalled();
    fireEvent.click(radio(0, 'IAT histogram'));
    await tick(2000);
    expect(api.getIatHistogram.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  describe('after the stop (like "Delay histogram")', () => {
    const S1 = 1762000000;
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
    function ended(state: 'stopped' | 'failed') {
      serveHistory({ startedAt: S1, samples: { '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) } });
      api.getEmulation.mockResolvedValue(fx.emulation(state, { directions: [] }));
      api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    }

    it.each(['stopped', 'failed'] as const)(
      'is enabled whenever Delay histogram is, and absent where it is absent (%s)',
      async (state) => {
        storedLinks(false);
        ended(state);
        vi.useFakeTimers();
        renderApp('/');
        await tick(0);
        await tick(0);
        expect(radioNames(0)).toEqual(CHART_RADIOS);
        expect(must(queryRadio(0, 'Delay histogram'))).toBeEnabled();
        expect(must(queryRadio(0, 'IAT histogram'))).toBeEnabled();
        expect(radioNames(1)).toEqual(['Traffic rates', 'Packet loss']);
        expect(queryRadio(1, 'IAT histogram')).toBeNull();
      },
    );

    it('shows the chart and the three tables from the controller’s copy, and keeps them when the run ends under the open page', async () => {
      storedLinks(true);
      serveLive(fx.dirStats(0), fx.dirStats(1));
      serveHistory({ startedAt: S1, samples: { '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) } });
      vi.useFakeTimers();
      renderApp('/');
      await tick(0);
      await tick(0);
      fireEvent.click(radio(0, 'IAT histogram'));
      await tick(1000);
      expect(iatData().datasets[0].data).toEqual(fx.iatHistogram(0).bins);
      expect(histTables(card(0))).toHaveLength(3);
      ended('stopped');
      await tick(3000);
      expect(screen.queryByTestId('pill-status')).not.toBeInTheDocument(); // the run is over
      expect(radio(0, 'IAT histogram')).toBeChecked();
      expect(iatData().datasets[0].data).toEqual(fx.iatHistogram(0).bins);
      expect(histTables(card(0))).toHaveLength(3);
      const n = api.getIatHistogram.mock.calls.length;
      await tick(2000);
      expect(api.getIatHistogram.mock.calls.length).toBeGreaterThan(n);
    });
  });
});

// ---- R2b: the tables and the chart of the IAT view -----------------------------------------------------------------------------------

const tablesOf = (d: number) => histTables(card(d));
const headers = (t: HTMLElement) =>
  within(t)
    .getAllByRole('columnheader')
    .map((h) => (h.textContent ?? '').trim());
const cells = (t: HTMLElement) =>
  Array.from(must(within(t).getAllByRole('row')[1], 'the body row').children).map((c) =>
    (c.textContent ?? '').replace(/\s+/g, ' ').trim(),
  );
const [CONFIG, VALUES, PERCENTILES] = [0, 1, 2];

async function ready(d = 0) {
  await openLive();
  fireEvent.click(radio(d, 'IAT histogram'));
  await findHistTables(card(d));
}
const serveIat = (over: Partial<HistogramView>) =>
  api.getIatHistogram.mockImplementation(async (dir) => fx.iatHistogram(dir, over));

describe('Spec 52 R2b: the three tables of the IAT view', () => {
  it('are config, values, percentiles, with the exact headers (IAT, σ(IAT), no direction)', async () => {
    await ready();
    const tables = tablesOf(0);
    expect(tables).toHaveLength(3);
    expect(tables.map(headers)).toEqual(IAT_TABLE_HEADERS);
    for (const t of tables) expect(within(t).getAllByRole('row')).toHaveLength(2);
    expect(within(card(0)).queryByText(/→/)).not.toBeInTheDocument();
  });

  it('sit in one row of three col-12 col-md-4 columns above the bar chart, P4TG-styled', async () => {
    await ready();
    const tables = tablesOf(0);
    const cols = tables.map((t) => must(t.closest('.col-md-4'), 'a .col-md-4 around the table'));
    for (const c of cols) expect(c).toHaveClass('col-12', 'col-md-4');
    const row = must(cols[0].parentElement, 'the row');
    expect(row).toHaveClass('row');
    expect(Array.from(row.children)).toEqual(cols);
    expect(follows(row, iatBar(card(0)))).toBe(true);
    expect(row.contains(iatBar(card(0)))).toBe(false);
    for (const t of tables) {
      expect(t).toHaveClass('table', 'table-striped', 'table-bordered', 'table-hover', 'table-sm', 'mt-3', 'mb-3');
      expect(must(t.querySelector('thead'))).toHaveClass('table-dark');
    }
  });

  it('"IAT" is overlined, the other headers are not', async () => {
    await ready();
    const ths = within(must(tablesOf(0)[VALUES])).getAllByRole('columnheader');
    const overlined = (th: HTMLElement) =>
      [th, ...Array.from(th.querySelectorAll('*'))].some((e) => /overline/.test(e.getAttribute('style') ?? ''));
    expect(overlined(must(ths[0]))).toBe(true);
    for (const th of ths.slice(1)) expect(overlined(th)).toBe(false);
  });

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

  it('are only in the IAT view: the delay view has its own tables, the rate and loss views none', async () => {
    await ready();
    fireEvent.click(radio(0, 'Delay histogram'));
    await waitFor(() => expect(histTables(card(0))).toHaveLength(3));
    expect(headers(must(tablesOf(0)[VALUES]))).toEqual(['Delay', 'σ(Delay)', 'Outlier', 'Total #Packets']);
    fireEvent.click(radio(0, 'IAT histogram'));
    await waitFor(() => expect(headers(must(tablesOf(0)[VALUES]))).toEqual(IAT_TABLE_HEADERS[1]));
    fireEvent.click(radio(0, 'Traffic rates'));
    expect(histTables(card(0))).toHaveLength(0);
    fireEvent.click(radio(0, 'Packet loss'));
    expect(histTables(card(0))).toHaveLength(0);
    expect(histTables(summary())).toHaveLength(0);
  });

  it('are not shown, and no chart either, when the IAT histogram is unavailable', async () => {
    api.getIatHistogram.mockRejectedValue(new Error('no telemetry'));
    await openLive();
    fireEvent.click(radio(0, 'IAT histogram'));
    await waitFor(() => expect(api.getIatHistogram.mock.settledResults.length).toBeGreaterThan(0));
    // real timers here: let the rejected fetch's state update land before looking
    await waitFor(() => expect(within(card(0)).queryByRole('img', { name: /^IAT/ })).not.toBeInTheDocument());
    expect(histTables(card(0))).toHaveLength(0);
    expect(within(card(0)).queryByRole('img', { name: /^IAT/ })).not.toBeInTheDocument();
    expect(within(card(0)).getByRole('img', { name: LOSS_DONUT_LABEL })).toBeInTheDocument();
  });
});

describe('Spec 52 R2b: the cells', () => {
  it('config: [lo, lo + bins x bin_ns], the number of bins, the bin width (literal: "[0.00 ns, 10.00 µs]", "5", "2.00 µs")', async () => {
    await ready(); // lo 0, 5 bins of 2000 ns
    const row = cells(must(tablesOf(0)[CONFIG]));
    expect(row).toEqual([`[${formatDuration(0)}, ${formatDuration(10000)}]`, formatCount(5), formatDuration(2000)]);
    expect(row[0]).toMatch(/^\[0\.00 ns, 10\.00 [µμ]s\]$/);
    expect(row[1]).toBe('5');
    expect(row[2]).toMatch(/^2\.00 [µμ]s$/);
  });

  it('config follows another lo, bin width and bin count', async () => {
    serveIat({ lo_ns: 100, bin_ns: 250, bins: [1, 2, 3, 4], edges_ns: [100, 350, 600, 850, 1100] });
    await ready();
    expect(cells(must(tablesOf(0)[CONFIG]))).toEqual([
      `[${formatDuration(100)}, ${formatDuration(1100)}]`,
      '4',
      formatDuration(250),
    ]);
  });

  it('values: mean from the bin centres weighted by the counts, under/overflow excluded (literal: bins 2 and 2 at 1 µs and 5 µs -> "3.00 µs")', async () => {
    serveIat({ bins: [2, 0, 2, 0, 0], underflow: 5, overflow: 7, count: 16 });
    await ready();
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row[0]).toMatch(/^3\.00 [µμ]s$/);
    expect(row[2]).toBe('12'); // outliers: underflow + overflow
    expect(row[3]).toBe('16'); // the count of the reply
  });

  it('values: σ from the bin centres weighted by the counts (literal: 3e6 at 1 µs, 1e6 at 5 µs -> mean "2.00 µs", σ "1.73 µs"; outliers of 1e6 each are left out)', async () => {
    serveIat({ bins: [3_000_000, 0, 1_000_000, 0, 0], underflow: 1_000_000, overflow: 1_000_000, count: 6_000_000 });
    await ready();
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row[0]).toMatch(/^2\.00 [µμ]s$/);
    expect(row[1]).toMatch(/^1\.73 [µμ]s$/);
    expect(row[2]).toBe('2,000,000');
    expect(row[3]).toBe('6,000,000');
  });

  it('values: the default fixture gives mean 343000 / 87 = "3.94 µs", not the link stats (1.00 ms, 50.00 µs) of the Delay table', async () => {
    await ready();
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row[0]).toMatch(/^3\.94 [µμ]s$/);
    expect(row[1]).not.toBe(formatDuration(50000));
    expect(row[1]).toMatch(/^1\.7\d [µμ]s$/); // σ is about 1.71 µs (sample or population)
    expect(row[2]).toBe('4'); // overflow 4 + underflow 0
    expect(row[3]).toBe('91');
  });

  it('values: mean and σ come from the IAT response, so they also show when the link has no live stats (an ended run)', async () => {
    serveIat({ bins: [2, 0, 2, 0, 0], underflow: 0, overflow: 0, count: 4 });
    serveLive(fx.dirStats(0, { delay_mean_ns: null, delay_std_ns: null }), fx.dirStats(1));
    renderApp('/');
    await findTabs(2);
    fireEvent.click(radio(0, 'IAT histogram'));
    await findHistTables(card(0));
    expect(cells(must(tablesOf(0)[VALUES]))[0]).toMatch(/^3\.00 [µμ]s$/);
  });

  it('values: Total #Packets is Σbins + underflow + overflow when the reply has no count, with thousands separators', async () => {
    api.getIatHistogram.mockImplementation(async (dir) => {
      const h: Partial<HistogramView> = fx.iatHistogram(dir, {
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
    expect(row[3]).toBe('16,234');
  });

  it('values: no packet in the bins reads "-" for mean and σ, not NaN', async () => {
    serveIat({ bins: [0, 0, 0, 0, 0], underflow: 0, overflow: 0, count: 0 });
    await ready();
    const row = cells(must(tablesOf(0)[VALUES]));
    expect(row.slice(0, 2)).toEqual(['-', '-']);
    expect(row.join(' ')).not.toMatch(/NaN/);
    expect(row.slice(2)).toEqual(['0', '0']);
  });

  it('percentiles: p50, p90, p99, p99.9 straight from the response (literal "3.00 µs", "5.00 µs", "7.00 µs", "9.00 µs")', async () => {
    await ready();
    expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual([
      formatDuration(3000),
      formatDuration(5000),
      formatDuration(7000),
      formatDuration(9000),
    ]);
    expect(cells(must(tablesOf(0)[PERCENTILES])).join('|')).toMatch(
      /^3\.00 [µμ]s\|5\.00 [µμ]s\|7\.00 [µμ]s\|9\.00 [µμ]s$/,
    );
  });

  it('percentiles: null reads "-" per percentile, a reply without percentiles reads "-" four times', async () => {
    serveIat({ percentiles: { p50: 3000, p90: null, p99: 7000, p999: null } });
    await ready();
    expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual(['3.00 µs', '-', '7.00 µs', '-']);
    api.getIatHistogram.mockImplementation(async (dir) => {
      const h = fx.iatHistogram(dir);
      delete h.percentiles;
      return h;
    });
    await waitFor(() => expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual(['-', '-', '-', '-']), { timeout: 3000 });
  });

  it('each link shows its own numbers', async () => {
    await ready();
    fireEvent.click(radio(1, 'IAT histogram'));
    await findHistTables(card(1));
    expect(cells(must(tablesOf(1)[VALUES]))[3]).toBe('92'); // count 91 + dir
    expect(cells(must(tablesOf(0)[VALUES]))[3]).toBe('91');
  });

  it('follow the next poll', async () => {
    vi.useFakeTimers();
    serveLive(fx.dirStats(0), fx.dirStats(1));
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'IAT histogram'));
    await tick(0);
    expect(cells(must(tablesOf(0)[VALUES]))[3]).toBe('91');
    serveIat({ count: 999, bins: [0, 0, 4, 0, 0], percentiles: { p50: 5000, p90: null, p99: null, p999: null } });
    await tick(2500);
    const values = cells(must(tablesOf(0)[VALUES]));
    expect(values[0]).toMatch(/^5\.00 [µμ]s$/);
    expect(values[3]).toBe('999');
    expect(cells(must(tablesOf(0)[PERCENTILES]))).toEqual(['5.00 µs', '-', '-', '-']);
  });
});

describe('Spec 52 R2b: the chart of the IAT view', () => {
  it('is one bar dataset of the response’s bins, one label per bin, y axis "#Packets", x axis in a time unit', async () => {
    await ready();
    const data = iatData();
    expect(data.datasets).toHaveLength(1);
    expect(data.datasets[0].data).toEqual(fx.iatHistogram(0).bins);
    expect(data.labels).toHaveLength(5);
    expect(data.labels.join(' ')).not.toMatch(/NaN|undefined/);
    const o = iatOptions();
    expect(o.scales?.y?.title?.text).toBe('#Packets');
    expect(o.scales?.x?.title?.text).toMatch(/(^|[^a-zA-Zµ])(ns|µs|us|ms|s)([^a-zA-Zµ]|$)/);
  });

  it('draws the four percentile lines as vertical annotations on the category axis (p50 of 3 µs at bin 1)', async () => {
    await ready();
    const annotations = Object.values(iatOptions().plugins?.annotation?.annotations ?? {}) as {
      type: string;
      xMin: number;
      xMax: number;
      label?: { content?: string };
    }[];
    expect(annotations).toHaveLength(4);
    for (const a of annotations) {
      expect(a.type).toBe('line');
      expect(a.xMax).toBe(a.xMin);
      expect(Number.isFinite(a.xMin)).toBe(true);
    }
    expect(annotations.map((a) => a.label?.content)).toEqual(['p50', 'p90', 'p99', 'p99.9']);
    // bin i is centred on i: (value - lo) / bin_ns - 0.5
    expect(annotations.map((a) => a.xMin)).toEqual([1, 2, 3, 4]);
  });

  it('draws no line for a percentile the response does not have', async () => {
    serveIat({ percentiles: { p50: 3000, p90: null, p99: 7000, p999: null } });
    await ready();
    expect(Object.keys(iatOptions().plugins?.annotation?.annotations ?? {})).toHaveLength(2);
  });

  it('has no expected overlay: no second dataset, no "expected" anywhere in the chart, even when the reply carries expected fields', async () => {
    serveIat({ expected: [1, 2, 3, 4, 5], expected_underflow: 1, expected_overflow: 1, expected_model: 'exact' });
    await ready();
    const data = iatData();
    expect(data.datasets).toHaveLength(1);
    expect(JSON.stringify(data)).not.toMatch(/expected/i);
    expect(JSON.stringify(iatOptions())).not.toMatch(/expected/i);
  });
});

// ---- R3: the order ---------------------------------------------------------------------------------------------------------------------

describe('Spec 52 R3: the rate and loss tables stay between the chart and the donut row', () => {
  it('link, rate view: chart, radios, rate table, loss table, donut row', async () => {
    await openLive();
    const panel = card(0);
    const chart = within(panel).getByRole('img', { name: /^Rate chart/ });
    const radios = within(panel).getByRole('radiogroup', { name: 'Chart' });
    const { loss } = donutCols(panel);
    const row = must(loss.col.parentElement);
    expect(follows(chart, radios)).toBe(true);
    expect(follows(radios, rateTable(panel))).toBe(true);
    expect(follows(rateTable(panel), lossTable(panel))).toBe(true);
    expect(follows(lossTable(panel), row)).toBe(true);
    expect(row.contains(rateTable(panel))).toBe(false);
    expect(row.contains(lossTable(panel))).toBe(false);
  });

  it('link, IAT view: histogram tables, IAT chart, radios, rate table, loss table, donut row', async () => {
    await openLive();
    const panel = await showIat(0);
    await findHistTables(panel);
    const chart = iatBar(panel);
    const radios = within(panel).getByRole('radiogroup', { name: 'Chart' });
    const row = must(donutCols(panel).loss.col.parentElement);
    const hist = tablesOf(0);
    expect(follows(must(hist[2]), chart)).toBe(true);
    expect(follows(chart, radios)).toBe(true);
    expect(follows(radios, rateTable(panel))).toBe(true);
    expect(follows(rateTable(panel), lossTable(panel))).toBe(true);
    expect(follows(lossTable(panel), row)).toBe(true);
  });

  it('Summary: chart, radios, rate table, loss table, donut row', async () => {
    await openLive();
    const panel = summary();
    const chart = within(panel).getByRole('img', { name: /^Rate chart/ });
    const radios = within(panel).getByRole('radiogroup', { name: 'Chart' });
    const row = must(donutCols(panel).loss.col.parentElement);
    expect(follows(chart, radios)).toBe(true);
    expect(follows(radios, rateTable(panel))).toBe(true);
    expect(follows(rateTable(panel), lossTable(panel))).toBe(true);
    expect(follows(lossTable(panel), row)).toBe(true);
  });
});
