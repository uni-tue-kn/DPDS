import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { Counters, DirectionStats } from '../api/types';
import { formatCount } from '../common/format';
import { propsOf } from '../test/chartProps';
import {
  CHART_RADIOS,
  card,
  delayBar,
  FRAME_DONUT_LABEL,
  findCard,
  findIatBar,
  frameDonut,
  iatBar,
  LOSS_DONUT_LABEL,
  lossDonut,
  queryDelayBar,
  queryFrameDonut,
  queryIatBar,
  radio,
  showIat,
  summary,
  tab,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedLinks, tick } from '../test/helpers';
import { ApiError, api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';
import { expectBars, percent } from '../test/probabilities';

// Spec 51 F1/F2 (user 2026-11-05: "generate and show an IAT histogram and frame size donut right next to the loss donut").
// Spec: .git/claude-specs/spec51-iat-frame-sizes.md. The header of src/test/dashboardUi.tsx has the contract of the DOM (the three
// boxes with role "img" and their aria-labels). SPEC 52 CHANGED THIS FILE: the donut row has two columns (the IAT histogram left it,
// pinned in dashboardSpec52.test.tsx) and the IAT histogram is the chart radio "IAT histogram", polled only while it is chosen and
// the tab is shown; no Summary placeholder. CHOICEs of the tester where the spec is silent:
//   - the frame size donut's box is labelled "Distribution of the received frames by size";
//   - its hover text carries the label, `formatCount(count)` and the share with two decimals and a percent sign (like the loss
//     donut's "<label>: <count> packets (<share> % of drops)", the wording after the count is free);
//   - its placeholder (all 8 counts 0) is one grey ring (one dataset, one positive value, one colour that is none of P4TG's
//     eight), the text "No frames" below it, and the hover text says "No frames" (no NaN, no %);
//   - the IAT box is labelled "IAT histogram of link <dir>"; the chart is a Bar with one dataset whose data are the response's
//     `bins`, one label per bin, the chart title "IAT" (`plugins.title`), the y axis "#Packets", an x axis whose title names a
//     time unit (ns, µs, ms or s); the title line is not pinned any more (spec 52 R2b: the tables carry the numbers);
//   - the IAT is polled by the function `getIatHistogram(dir)` of the API client (the one-argument form, no window), every second
//     like the delay histogram, only while the "IAT histogram" radio is chosen and the link's tab is shown.

/** P4TG's frame size labels (Configuration GUI/src/components/Visuals.tsx, frame_size_data). */
const FRAME_LABELS = ['0-63', '64', '65-127', '128-255', '256-511', '512-1023', '1024-1518', '1519-21519'];
// User 2026-11-05: the frame sizes take the loss donut's soft palette, without its red, instead of P4TG's colours.
const FRAME_COLOURS: [number, number, number][] = [
  [0x6c, 0x8e, 0xbf],
  [0x00, 0xce, 0xc9],
  [0x55, 0xef, 0xc4],
  [0xf6, 0xb7, 0x3c],
  [0xe6, 0x7e, 0x22],
  [0xfd, 0x79, 0xa8],
  [0xa2, 0x9b, 0xfe],
  [0x63, 0x6e, 0x72],
];

/** "rgb(255, 99, 132)", "rgb(255,99,132)" or "#ff6384" as [r, g, b]. */
function rgbOf(colour: string): [number, number, number] {
  const c = colour.trim().toLowerCase();
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/.exec(c);
  if (hex) return [Number.parseInt(hex[1], 16), Number.parseInt(hex[2], 16), Number.parseInt(hex[3], 16)];
  const rgb = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/.exec(c);
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  throw new Error(`unreadable colour ${colour}`);
}

const SIZES0 = [10, 20, 30, 40, 100, 150, 250, 400]; // = fx.counters() default, sum 1000
const SIZES1 = [1, 2, 3, 4, 5, 6, 7, 8];
const ZERO8 = [0, 0, 0, 0, 0, 0, 0, 0];

const live = (dir: number, over: Partial<Counters> = {}, rest: Partial<DirectionStats> = {}) =>
  fx.dirStats(dir, { counters: fx.counters(over), ...rest });

function serveLive(...dirs: DirectionStats[]) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats(dirs));
}

interface Donut {
  labels: string[];
  datasets: { data: number[]; backgroundColor: string[] }[];
}
const donutOf = (el: HTMLElement): Donut => JSON.parse(el.getAttribute('data-chart') ?? 'null');
const framesOf = (scope: HTMLElement) => donutOf(frameDonut(scope)).datasets[0].data;

/** Calls the tooltip label callback of a Doughnut stub for segment `i`, like Chart.js would. */
function hoverText(el: HTMLElement, i: number): string {
  const props = propsOf(el);
  const data = props.data as Donut;
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

async function openLive(...dirs: DirectionStats[]) {
  serveLive(...dirs);
  renderApp('/');
  await findCard(dirs.length - 1);
}

// ---- the row ------------------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 51 F1, spec 52 R1/R2): the donut row of a link', () => {
  it('holds the loss donut and the frame size donut, in this order, in one row of two columns, and no IAT chart', async () => {
    await openLive(live(0), live(1));
    const panel = card(0);
    const boxes = [
      within(panel).getByRole('img', { name: LOSS_DONUT_LABEL }),
      within(panel).getByRole('img', { name: FRAME_DONUT_LABEL }),
    ];
    const cols = boxes.map((b) => must(b.closest('.col-md-6'), 'the column of a chart') as HTMLElement);
    const row = must(cols[0].parentElement, 'the row');
    expect(row).toHaveClass('row');
    for (const [i, col] of cols.entries()) {
      expect(col.parentElement, `column ${i}`).toBe(row);
      expect(col, `column ${i}`).toHaveClass('col-12', 'col-md-6'); // stacks below md
    }
    expect(Array.from(row.children)).toEqual(cols); // DOM order, nothing else in the row
    expect(within(panel).queryByRole('img', { name: /^IAT/ })).not.toBeInTheDocument();
  });

  it('has a Doughnut in the loss box and in the frame box, each in its own box', async () => {
    await openLive(live(0), live(1));
    const panel = card(0);
    expect(lossDonut(panel)).toBeInTheDocument();
    expect(frameDonut(panel)).toBeInTheDocument();
    expect(lossDonut(panel)).not.toBe(frameDonut(panel));
    expect(donutOf(lossDonut(panel)).labels).toHaveLength(9); // the loss donut is the one of spec 35 / 47, unchanged
  });

  it('sits below the radios, the rate table row and the loss table row', async () => {
    await openLive(live(0), live(1));
    const panel = card(0);
    const radios = within(panel).getByRole('radiogroup', { name: 'Chart' });
    const tables = within(panel).getAllByRole('table');
    expect(tables).toHaveLength(2); // TX L1... and Lost Frames... (no histogram tables in the rate view)
    const donuts = within(panel).getByRole('img', { name: LOSS_DONUT_LABEL });
    const follows = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    expect(follows(radios, tables[0])).toBe(true);
    expect(follows(tables[0], tables[1])).toBe(true);
    expect(follows(tables[1], donuts)).toBe(true);
  });

  it.each(CHART_RADIOS)('stays in place under the radio "%s"', async (choice) => {
    await openLive(live(0), live(1));
    fireEvent.click(radio(0, choice));
    const panel = card(0);
    if (choice === 'IAT histogram') await findIatBar(panel);
    expect(lossDonut(panel)).toBeInTheDocument();
    expect(frameDonut(panel)).toBeInTheDocument();
  });

  it('has no IAT radio, no IAT chart, polls none and keeps both donuts for a link without delay statistics', async () => {
    vi.useFakeTimers();
    serveLive(live(0, {}, { delay_stats: false }), live(1));
    renderApp('/');
    await tick(0);
    await tick(0);
    await tick(3000);
    expect(queryIatBar(card(0))).toBeNull();
    expect(within(card(0)).queryByRole('radio', { name: 'IAT histogram' })).not.toBeInTheDocument();
    expect(within(card(0)).queryByRole('img', { name: /^IAT/ })).not.toBeInTheDocument();
    expect(frameDonut(card(0))).toBeInTheDocument();
    expect(lossDonut(card(0))).toBeInTheDocument();
    expect(api.getIatHistogram.mock.calls.filter(([d]) => d === 0)).toEqual([]);
  });
});

describe('Dashboard (spec 51 F1, spec 52 R1/R2): the donut row of the Summary', () => {
  it('shows the loss and the frame size donut in two columns, no IAT chart and no IAT placeholder text', async () => {
    await openLive(live(0), live(1));
    const panel = summary();
    expect(lossDonut(panel)).toBeInTheDocument();
    expect(frameDonut(panel)).toBeInTheDocument();
    expect(within(panel).queryByRole('img', { name: /^IAT/ })).not.toBeInTheDocument();
    expect(queryIatBar(panel)).toBeNull();
    expect(within(panel).queryAllByTestId('chart-bar')).toHaveLength(0);
    expect(within(panel).queryByText(/IAT histograms are per link/)).not.toBeInTheDocument();
    const cols = [LOSS_DONUT_LABEL, FRAME_DONUT_LABEL].map(
      (n) => must(within(panel).getByRole('img', { name: n }).closest('.col-md-6')) as HTMLElement,
    );
    const row = must(cols[0].parentElement);
    expect(row).toHaveClass('row');
    expect(Array.from(row.children)).toEqual(cols);
  });

  it('has the "IAT histogram" radio disabled, and does not poll the IAT of any link while the Summary is shown', async () => {
    vi.useFakeTimers();
    serveLive(live(0), live(1));
    renderApp('/');
    await tick(0);
    await tick(0);
    await tick(3000);
    expect(within(summary()).getByRole('radio', { name: 'IAT histogram' })).toBeDisabled();
    expect(api.getIatHistogram).not.toHaveBeenCalled();
  });
});

describe('Dashboard (spec 51 F1): before any run', () => {
  it('shows the frame size placeholder next to the loss placeholder and polls no IAT', async () => {
    vi.useFakeTimers();
    seedLinks([fx.link(), fx.linkRev()]);
    renderApp('/');
    await tick(0);
    await tick(0);
    await tick(3000);
    for (const d of [0, 1]) {
      const panel = card(d);
      expect(lossDonut(panel)).toBeInTheDocument();
      const ring = donutOf(frameDonut(panel));
      expect(ring.datasets).toHaveLength(1);
      expect(ring.datasets[0].data).toHaveLength(1);
      expect(ring.datasets[0].data[0]).toBeGreaterThan(0);
      expect(queryIatBar(panel)).toBeNull();
      expect(within(panel).getByRole('radio', { name: 'IAT histogram' })).toBeDisabled();
    }
    expect(api.getIatHistogram).not.toHaveBeenCalled();
  });
});

// ---- the frame size donut -------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 51 F1): the frame size donut', () => {
  it("has P4TG's eight labels, in order, and one dataset", async () => {
    await openLive(live(0), live(1));
    const d = donutOf(frameDonut(card(0)));
    expect(d.labels).toEqual(FRAME_LABELS);
    expect(d.datasets).toHaveLength(1);
    expect(d.datasets[0].data).toHaveLength(8);
  });

  it("has P4TG's eight colours (rgb values), in the order of the labels", async () => {
    await openLive(live(0), live(1));
    const colours = donutOf(frameDonut(card(0))).datasets[0].backgroundColor;
    expect(colours).toHaveLength(8);
    expect(colours.map(rgbOf)).toEqual(FRAME_COLOURS);
  });

  it("shows each link's own frame_sizes counters", async () => {
    await openLive(live(0, { frame_sizes: SIZES0 }), live(1, { frame_sizes: SIZES1 }));
    expect(framesOf(card(0))).toEqual(SIZES0);
    expect(framesOf(card(1))).toEqual(SIZES1);
  });

  it('follows the polls (the counters grow)', async () => {
    vi.useFakeTimers();
    serveLive(live(0, { frame_sizes: SIZES0 }), live(1));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(framesOf(card(0))).toEqual(SIZES0);
    const grown = SIZES0.map((v) => v * 2 + 1);
    serveLive(live(0, { frame_sizes: grown }), live(1));
    await tick(2000);
    expect(framesOf(card(0))).toEqual(grown);
  });

  it('shows in the Summary the sum of the links, bucket by bucket', async () => {
    await openLive(live(0, { frame_sizes: SIZES0 }), live(1, { frame_sizes: SIZES1 }));
    expect(framesOf(summary())).toEqual(SIZES0.map((v, i) => v + SIZES1[i]));
  });

  it('shows in the Summary the sum of three links', async () => {
    const third = [100, 0, 0, 0, 0, 0, 0, 1];
    await openLive(
      live(0, { frame_sizes: SIZES0 }),
      live(1, { frame_sizes: SIZES1 }),
      live(2, { frame_sizes: third }, { link: 'c', rx: 'net_null0', tx: 'net_null1' }),
    );
    expect(framesOf(summary())).toEqual(SIZES0.map((v, i) => v + SIZES1[i] + third[i]));
  });

  describe('hover', () => {
    it('says label, count and share of all received frames for every bucket', async () => {
      await openLive(live(0, { frame_sizes: SIZES0 }), live(1));
      const el = frameDonut(card(0));
      for (const [i, count] of SIZES0.entries()) {
        const text = hoverText(el, i);
        expect(text, `bucket ${i}`).toContain(FRAME_LABELS[i]);
        expect(text, `bucket ${i}`).toContain(formatCount(count));
        expect(text, `bucket ${i}`).toContain(`${((count / 1000) * 100).toFixed(2)} %`);
        expect(text).not.toMatch(/NaN|Infinity/);
      }
    });

    it('reads 0.00 % for an empty bucket of a donut that has other frames', async () => {
      await openLive(live(0, { frame_sizes: [0, 0, 0, 0, 0, 0, 0, 5] }), live(1));
      expect(hoverText(frameDonut(card(0)), 0)).toContain('0.00 %');
      expect(hoverText(frameDonut(card(0)), 7)).toContain('100.00 %');
    });

    it('counts with thousands separators', async () => {
      await openLive(live(0, { frame_sizes: [0, 0, 0, 0, 0, 0, 1234567, 1] }), live(1));
      expect(hoverText(frameDonut(card(0)), 6)).toContain('1,234,567');
    });

    it('in the Summary uses the sums', async () => {
      await openLive(live(0, { frame_sizes: SIZES0 }), live(1, { frame_sizes: SIZES1 }));
      const sums = SIZES0.map((v, i) => v + SIZES1[i]); // 11 22 33 44 105 156 257 408, total 1036
      const total = sums.reduce((a, b) => a + b, 0);
      expect(hoverText(frameDonut(summary()), 7)).toContain(`${((408 / total) * 100).toFixed(2)} %`);
      expect(hoverText(frameDonut(summary()), 7)).toContain('408');
    });
  });

  describe('the placeholder (all eight counts 0)', () => {
    it('is one grey ring: one dataset, one positive value, one colour that is none of the eight', async () => {
      await openLive(live(0, { frame_sizes: ZERO8 }), live(1));
      const d = donutOf(frameDonut(card(0)));
      expect(d.datasets).toHaveLength(1);
      expect(d.datasets[0].data).toHaveLength(1);
      expect(d.datasets[0].data[0]).toBeGreaterThan(0);
      expect(d.datasets[0].backgroundColor).toHaveLength(1);
      const [r, g, b] = rgbOf(d.datasets[0].backgroundColor[0]);
      expect(Math.max(r, g, b) - Math.min(r, g, b), 'grey: the channels are close').toBeLessThanOrEqual(40);
      expect(FRAME_COLOURS.map(String)).not.toContain(String([r, g, b]));
    });

    it('says "No frames" as text and on hover, without NaN or a percentage', async () => {
      await openLive(live(0, { frame_sizes: ZERO8 }), live(1));
      expect(within(card(0)).getByText('No frames')).toBeInTheDocument();
      const text = hoverText(frameDonut(card(0)), 0);
      expect(text).toContain('No frames');
      expect(text).not.toMatch(/NaN|Infinity|%/);
    });

    it('does not appear for a donut that has frames, and is replaced as soon as frames arrive', async () => {
      vi.useFakeTimers();
      serveLive(live(0, { frame_sizes: ZERO8 }), live(1));
      renderApp('/');
      await tick(0);
      await tick(0);
      expect(donutOf(frameDonut(card(0))).datasets[0].data).toHaveLength(1);
      serveLive(live(0, { frame_sizes: SIZES0 }), live(1));
      await tick(2000);
      expect(framesOf(card(0))).toEqual(SIZES0);
      expect(within(card(0)).queryByText('No frames')).not.toBeInTheDocument();
    });

    it('is the Summary as well when every link has none; one link with frames is enough to show them', async () => {
      await openLive(live(0, { frame_sizes: ZERO8 }), live(1, { frame_sizes: ZERO8 }));
      expect(donutOf(frameDonut(summary())).datasets[0].data).toHaveLength(1);
      expect(within(summary()).getByText('No frames')).toBeInTheDocument();
    });

    it('has frames in the Summary when only one of the links has some (link 0 on its own is the placeholder)', async () => {
      await openLive(live(0, { frame_sizes: ZERO8 }), live(1, { frame_sizes: SIZES1 }));
      expect(framesOf(summary())).toEqual(SIZES1);
      expect(framesOf(card(0))).toHaveLength(1); // link 0 on its own is the placeholder
    });
  });

  describe('after the stop', () => {
    const S1 = 1762000000;
    function ended(state: 'stopped' | 'failed') {
      serveHistory({
        startedAt: S1,
        samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) },
        totals: {
          '0': fx.totals({ frame_sizes: [5, 4, 3, 2, 1, 0, 0, 9] }),
          '1': fx.totals({ frame_sizes: [1, 1, 1, 1, 1, 1, 1, 1] }),
        },
      });
      api.getEmulation.mockResolvedValue(fx.emulation(state, { directions: [] }));
      api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 3000 }));
    }

    it.each(['stopped', 'failed'] as const)('keeps the run’s totals per link (%s)', async (state) => {
      ended(state);
      renderApp('/');
      await findCard(1);
      await waitFor(() => expect(framesOf(card(0))).toEqual([5, 4, 3, 2, 1, 0, 0, 9]));
      expect(framesOf(card(1))).toEqual([1, 1, 1, 1, 1, 1, 1, 1]);
    });

    it('keeps the Summary’s sums of the totals', async () => {
      ended('stopped');
      renderApp('/');
      await findCard(1);
      await waitFor(() => expect(framesOf(summary())).toEqual([6, 5, 4, 3, 2, 1, 1, 10]));
    });

    it('goes on showing the counts when the run ends under the open page', async () => {
      vi.useFakeTimers();
      serveLive(live(0, { frame_sizes: SIZES0 }), live(1, { frame_sizes: SIZES1 }));
      serveHistory({
        startedAt: S1,
        samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) },
        totals: { '0': fx.totals({ frame_sizes: SIZES0 }), '1': fx.totals({ frame_sizes: SIZES1 }) },
      });
      renderApp('/');
      await tick(0);
      await tick(0);
      expect(framesOf(card(0))).toEqual(SIZES0);
      api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
      api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 3000 }));
      await tick(3000);
      expect(framesOf(card(0))).toEqual(SIZES0);
      expect(framesOf(card(1))).toEqual(SIZES1);
      expect(queryFrameDonut(card(0))).not.toBeNull();
    });
  });
});

// ---- the IAT histogram ----------------------------------------------------------------------------------------------------------

interface Bars {
  labels: string[];
  datasets: { data: number[] }[];
}
interface BarOptions {
  plugins?: { title?: { display?: boolean; text?: string } };
  scales?: { x?: { title?: { text?: string } }; y?: { title?: { text?: string } } };
}
const barsOf = (scope: HTMLElement): Bars => JSON.parse(iatBar(scope).getAttribute('data-chart') ?? 'null');
const optionsOf = (scope: HTMLElement): BarOptions => JSON.parse(iatBar(scope).getAttribute('data-options') ?? 'null');
const iatCalls = (dir: number) => api.getIatHistogram.mock.calls.filter(([d]) => d === dir);

describe('Dashboard (spec 51 F1): the IAT histogram chart', () => {
  it('draws the bins of the controller as bars, one label per bin', async () => {
    await openLive(live(0), live(1));
    await showIat(0);
    const h = fx.iatHistogram(0);
    expect(barsOf(card(0)).datasets).toHaveLength(1);
    expectBars(barsOf(card(0)).datasets[0].data, h);
    expect(barsOf(card(0)).labels).toHaveLength(h.bins.length);
    expect(barsOf(card(0)).labels.join(' ')).not.toMatch(/NaN|undefined/);
  });

  it('is the link’s own: link 1 shows the bins of dir 1', async () => {
    await openLive(live(0), live(1));
    await showIat(1);
    expectBars(barsOf(card(1)).datasets[0].data, fx.iatHistogram(1));
    expect(barsOf(card(1)).datasets[0].data).not.toEqual(percent(fx.iatHistogram(0)));
  });

  it('has the y axis "Probability (%)" and the x axis "IAT Range" (spec 58 S3, S4; was "#Packets" and a time unit)', async () => {
    await openLive(live(0), live(1));
    await showIat(0);
    const o = optionsOf(card(0));
    expect(o.scales?.y?.title?.text).toBe('Probability (%)');
    expect(o.scales?.x?.title?.text).toBe('IAT Range');
  });

  it('is not the delay histogram: each view shows only its own chart, with its own data (spec 52 R2)', async () => {
    await openLive(live(0), live(1));
    fireEvent.click(radio(0, 'Delay histogram'));
    await waitFor(() => expect(within(card(0)).getAllByTestId('chart-bar')).toHaveLength(1));
    expectBars(JSON.parse(delayBar(card(0)).getAttribute('data-chart') ?? 'null').datasets[0].data, fx.histogram(0));
    expect(queryIatBar(card(0))).toBeNull();
    await showIat(0);
    expect(within(card(0)).getAllByTestId('chart-bar')).toHaveLength(1);
    expect(queryDelayBar(card(0))).toBeNull();
    expectBars(barsOf(card(0)).datasets[0].data, fx.iatHistogram(0));
  });

  it('shows what a poll brings (the counts grow)', async () => {
    vi.useFakeTimers();
    serveLive(live(0), live(1));
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'IAT histogram')); // chosen on the shown tab: the first fetch starts
    await tick(0);
    await tick(0);
    expectBars(barsOf(card(0)).datasets[0].data, fx.iatHistogram(0));
    api.getIatHistogram.mockImplementation(async (dir) => fx.iatHistogram(dir, { bins: [9, 8, 7, 6, 5], count: 35 }));
    await tick(2000);
    expectBars(barsOf(card(0)).datasets[0].data, fx.iatHistogram(0, { bins: [9, 8, 7, 6, 5], count: 35 }));
  });

  it('is absent, with no crash and both donuts, when the controller answers 404', async () => {
    api.getIatHistogram.mockRejectedValue(new ApiError(404, 'no delay statistics'));
    await openLive(live(0), live(1));
    fireEvent.click(radio(0, 'IAT histogram'));
    await waitFor(() => expect(api.getIatHistogram).toHaveBeenCalled());
    await waitFor(() => expect(api.getIatHistogram.mock.settledResults.length).toBeGreaterThan(0));
    expect(queryIatBar(card(0))).toBeNull();
    expect(lossDonut(card(0))).toBeInTheDocument();
    expect(frameDonut(card(0))).toBeInTheDocument();
  });
});

describe('Dashboard (spec 51 F2, spec 52 R2): polling the IAT', () => {
  async function open() {
    vi.useFakeTimers();
    serveLive(live(0), live(1));
    renderApp('/');
    await tick(0);
    await tick(0);
  }

  it('polls the visible link once per second with the one-argument form, and no other link', async () => {
    await open();
    fireEvent.click(radio(0, 'IAT histogram'));
    await tick(0);
    const n = iatCalls(0).length;
    expect(n).toBeGreaterThanOrEqual(1);
    await tick(3000);
    expect(iatCalls(0).length).toBeGreaterThanOrEqual(n + 3);
    expect(iatCalls(1)).toEqual([]);
    for (const call of api.getIatHistogram.mock.calls) expect(call).toEqual([call[0]]);
  });

  it('follows the tab: the other link is polled after the switch (its radio chosen) and the first one stops', async () => {
    await open();
    fireEvent.click(radio(0, 'IAT histogram'));
    await tick(2000);
    fireEvent.click(radio(1, 'IAT histogram')); // selects tab 1, so tab 0 is hidden
    await tick(0);
    await tick(1000);
    const first = iatCalls(0).length;
    const second = iatCalls(1).length;
    expect(second).toBeGreaterThanOrEqual(1);
    await tick(3000);
    expect(iatCalls(0).length).toBe(first);
    expect(iatCalls(1).length).toBeGreaterThanOrEqual(second + 3);
  });

  it('polls nothing in the rate view that is the default, nor under the other radios', async () => {
    await open();
    fireEvent.click(tab(0));
    await tick(0);
    expect(within(card(0)).getByRole('radio', { name: 'Traffic rates' })).toBeChecked();
    await tick(3000);
    expect(api.getIatHistogram).not.toHaveBeenCalled();
    for (const name of ['Delay histogram', 'Packet loss']) {
      fireEvent.click(radio(0, name));
      await tick(3000);
    }
    expect(api.getIatHistogram).not.toHaveBeenCalled();
  });

  it('stops polling when another radio is chosen, and resumes when "IAT histogram" is chosen again', async () => {
    await open();
    fireEvent.click(radio(0, 'IAT histogram'));
    await tick(2000);
    fireEvent.click(radio(0, 'Packet loss'));
    await tick(0);
    const n = iatCalls(0).length;
    expect(n).toBeGreaterThanOrEqual(2);
    await tick(3000);
    expect(iatCalls(0).length).toBe(n);
    fireEvent.click(radio(0, 'IAT histogram'));
    await tick(3000);
    expect(iatCalls(0).length).toBeGreaterThanOrEqual(n + 3);
  });

  describe('after the stop (F2: it stays)', () => {
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
      'shows the controller’s copy of a %s run and keeps polling it',
      async (state) => {
        storedLinks(true);
        ended(state);
        vi.useFakeTimers();
        renderApp('/');
        await tick(0);
        await tick(0);
        expect(within(card(0)).getByRole('radio', { name: 'IAT histogram' })).toBeEnabled();
        fireEvent.click(radio(0, 'IAT histogram'));
        await tick(1000);
        expectBars(barsOf(card(0)).datasets[0].data, fx.iatHistogram(0));
        const n = iatCalls(0).length;
        await tick(3000);
        expect(iatCalls(0).length).toBeGreaterThanOrEqual(n + 3);
        expect(iatCalls(1)).toEqual([]);
      },
    );

    it('has no IAT radio and no chart, and still both donuts, for a link whose run had no delay statistics', async () => {
      storedLinks(false);
      ended('stopped');
      api.getIatHistogram.mockImplementation(async (dir) => {
        if (dir === 1) throw new ApiError(404, 'no delay statistics');
        return fx.iatHistogram(dir);
      });
      vi.useFakeTimers();
      renderApp('/');
      await tick(0);
      await tick(0);
      fireEvent.click(tab(1));
      await tick(3000);
      expect(within(card(1)).queryByRole('radio', { name: 'IAT histogram' })).not.toBeInTheDocument();
      expect(queryIatBar(card(1))).toBeNull();
      expect(frameDonut(card(1))).toBeInTheDocument();
      expect(lossDonut(card(1))).toBeInTheDocument();
    });

    it('stays when the run ends under the open page', async () => {
      storedLinks(true);
      serveLive(live(0), live(1));
      serveHistory({ startedAt: S1, samples: { '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) } });
      vi.useFakeTimers();
      renderApp('/');
      await tick(0);
      await tick(0);
      fireEvent.click(radio(0, 'IAT histogram'));
      await tick(1000);
      expectBars(barsOf(card(0)).datasets[0].data, fx.iatHistogram(0));
      ended('stopped');
      await tick(3000);
      expect(screen.queryByTestId('pill-status')).not.toBeInTheDocument(); // the run is over
      expectBars(barsOf(card(0)).datasets[0].data, fx.iatHistogram(0));
      const n = iatCalls(0).length;
      await tick(2000);
      expect(iatCalls(0).length).toBeGreaterThan(n);
    });
  });
});
