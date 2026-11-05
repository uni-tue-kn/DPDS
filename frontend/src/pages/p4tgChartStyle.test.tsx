import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fireEvent, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import { propsOf } from '../test/chartProps';
import { card, delayBar, histTables, iatBar, radio } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { deferred, renderApp, tick } from '../test/helpers';
import { api } from '../test/mockApi';
import { must } from '../test/must';
import { expectBars, percent } from '../test/probabilities';

// Spec 58: the delay and IAT histograms look and switch like P4TG's. Spec: .git/claude-specs/spec58-p4tg-chart-style.md.
// Replaces spec 56's percentileToggles.test.tsx (per-line toggles). Contract of the DOM and of the chart props:
//   - H1: one `<button aria-pressed>` in the link's tabpanel, text "Hide percentiles" / "Show percentiles", inside a row div
//     `d-flex justify-content-end align-items-center gap-2 mb-2` that comes before the first histogram table;
//   - A1..A3: `plugins.annotation.annotations` (record or list, read from `data-options`) holds per shown percentile one
//     `type: 'line'` and one `type: 'label'` annotation; a label belongs to the line at `xMin === label.xValue + 0.15`;
//   - R2: CHOICE: while a view waits for its first data the card holds one element with the class `chart-box` (the 250 px box
//     of index.css) and no chart stub and no histogram table. Tables row height cannot be measured in jsdom: not covered.
//   - R2/S1: the placeholder is also not an `img` box (no role img with the chart's label while nothing is drawn), and index.css has
//     a rule `aspect-ratio: 4` whose selector (one or several classes, no tag) the placeholder fulfils;
//   - S4: the unit symbol of µs is accepted as U+00B5 (DPDS's formatDuration) or U+03BC (P4TG's getTimeUnit);
//   - R3: "no new data/options object" is read from the props the chart stub got in its latest render (test/chartProps.ts).

type Ann = Record<string, unknown> & { type?: string };
type Opts = { plugins?: { annotation?: { annotations?: unknown } } } & Record<string, unknown>;
type Chart = {
  datasets: { data: number[]; backgroundColor?: unknown; borderColor?: unknown; borderWidth?: unknown }[];
};

type View = {
  name: string;
  radioName: string;
  bar: (scope: HTMLElement) => HTMLElement;
  api: typeof api.getDelayHistogram;
  make: (dir: number, over?: Partial<HistogramView>) => HistogramView;
  fill: string;
  palette: string[];
  /** lo, bin and the largest bar of the fixture's histogram, and the line position of p50..p99.9 on the category axis. */
  xs: number[];
};
const VIEWS: View[] = [
  {
    name: 'delay',
    radioName: 'Delay histogram',
    bar: delayBar,
    api: api.getDelayHistogram,
    make: fx.histogram,
    fill: 'rgba(231,76,60,0.6)',
    palette: ['#e74c3c', '#d35400', '#e67e22', '#af601a'],
    xs: [1, 2, 3, 3.4],
  },
  {
    name: 'IAT',
    radioName: 'IAT histogram',
    bar: iatBar,
    api: api.getIatHistogram,
    make: fx.iatHistogram,
    fill: 'rgba(53,162,235,0.6)',
    palette: ['#3c82e7', '#2ecc71', '#16a085', '#1f618d'],
    xs: [1, 2, 3, 4],
  },
];
const NAMES = ['p50', 'p90', 'p99', 'p99.9'];
const OTHER = (v: View) => must(VIEWS.find((x) => x !== v));
const HIDE = 'Hide percentiles';
const SHOW = 'Show percentiles';

const optsOf = (el: HTMLElement): Opts => JSON.parse(el.getAttribute('data-options') ?? 'null');
const chartOf = (el: HTMLElement): Chart => JSON.parse(el.getAttribute('data-chart') ?? 'null');
const annotations = (o: Opts): Ann[] => {
  const a = o.plugins?.annotation?.annotations;
  if (a === undefined || a === null) return [];
  return (Array.isArray(a) ? a : Object.values(a as object)) as Ann[];
};
const lines = (o: Opts) => annotations(o).filter((a) => a.type === 'line');
const labels = (o: Opts) => annotations(o).filter((a) => a.type === 'label');
const contentOf = (a: Ann): string => String((a.content as unknown) ?? '');
const lineOfLabel = (o: Opts, l: Ann) =>
  must(
    lines(o).find((x) => Math.abs((x.xMin as number) - ((l.xValue as number) + 0.15)) < 1e-9),
    `the line of label ${contentOf(l)}`,
  );
const norm = (c: unknown) => String(c).replace(/\s/g, '');
const tableText = (d: number) => histTables(card(d)).map((t) => t.textContent);

const button = (d: number) => within(card(d)).getByRole('button', { name: /percentiles$/ });
const queryButton = (d: number) => within(card(d)).queryByRole('button', { name: /percentiles$/ });
const pressedButtons = (d: number) =>
  within(card(d))
    .queryAllByRole('button')
    .filter((b) => b.hasAttribute('aria-pressed'));
const css = readFileSync(resolve(__dirname, '../assets/index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
/** True iff a rule of index.css with `aspect-ratio: n` has a selector made of classes only that `el` all carries. */
function hasRatioRule(el: Element, n: number): boolean {
  const re = /([^{}]+)\{([^{}]*)\}/g;
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    if (!new RegExp(`(?:^|[;\\s])aspect-ratio:\\s*${n}\\s*(?:/\\s*1\\s*)?(?:;|$)`).test(m[2] ?? '')) continue;
    for (const sel of (m[1] ?? '').split(',').map((x) => x.trim())) {
      if (
        /^(\.[\w-]+)+$/.test(sel) &&
        sel
          .split('.')
          .slice(1)
          .every((c) => el.classList.contains(c))
      )
        return true;
    }
  }
  return false;
}
const chartBoxes = (d: number) => card(d).querySelectorAll('.chart-box');
const stubs = (d: number) => card(d).querySelectorAll('[data-testid="chart-bar"]');

function serve(v: View, over: Partial<HistogramView> = {}) {
  v.api.mockImplementation(async (dir) => v.make(dir, over));
}
async function open(n = 2) {
  vi.useFakeTimers();
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)].slice(0, n)));
  const r = renderApp('/');
  await tick(0);
  await tick(0);
  return r;
}
/** Shows `v` of link `d` (selects the tab, chooses the radio if needed), lets the fetch settle, returns its bar chart. */
async function show(v: View, d = 0): Promise<HTMLElement> {
  card(d);
  if (!(radio(d, v.radioName) as HTMLInputElement).checked) fireEvent.click(radio(d, v.radioName));
  await tick(0);
  return v.bar(card(d));
}

describe.each(VIEWS)('Spec 58 C1: the bars of the $name view', (v) => {
  it('are translucent like P4TG and have no border', async () => {
    await open();
    const ds = must(chartOf(await show(v)).datasets[0]);
    expect(norm(ds.backgroundColor)).toBe(v.fill);
    expect(ds.borderColor).toBeUndefined();
    expect(Number(ds.borderWidth ?? 0)).toBe(0);
  });
});

describe.each(VIEWS)('Spec 58 C2 and A1 to A3: percentile lines and labels of the $name view', (v) => {
  it('two annotations per percentile: four lines and four labels, nothing else', async () => {
    await open();
    const o = optsOf(await show(v));
    expect(annotations(o)).toHaveLength(8);
    expect(lines(o)).toHaveLength(4);
    expect(labels(o).map(contentOf)).toEqual(NAMES);
  });

  it('the line stands at (value - lo) / bin - 0.5, 2 px wide, dashed [6, 6], in the palette colour, without a built-in label', async () => {
    await open();
    const o = optsOf(await show(v));
    const ls = lines(o);
    ls.forEach((l, i) => {
      expect(l.xMin as number, NAMES[i]).toBeCloseTo(must(v.xs[i]), 9);
      expect(l.xMax as number).toBe(l.xMin);
      expect(l.borderWidth).toBe(2);
      expect(l.borderDash).toEqual([6, 6]);
      expect(l.borderColor).toBe(v.palette[i]);
      const lab = l.label as { display?: boolean } | undefined;
      expect(lab === undefined || lab.display !== true, 'no label on the line').toBe(true);
    });
  });

  it('the label sits 0.15 left of its line, at maxCount * 0.95 * (1 - 0.065 i), with the P4TG look', async () => {
    await open();
    const o = optsOf(await show(v));
    expect(labels(o)).toHaveLength(4);
    labels(o).forEach((l, i) => {
      expect(contentOf(l)).toBe(NAMES[i]);
      const line = lineOfLabel(o, l);
      expect(line.borderColor).toBe(v.palette[i]);
      expect(l.xValue as number).toBeCloseTo((line.xMin as number) - 0.15, 9);
      expect(l.yValue as number).toBeCloseTo(Math.max(...percent(v.make(0))) * 0.95 * (1 - 0.065 * i), 9);
      expect(l.backgroundColor).toBe(`${v.palette[i]}80`);
      expect(l.color).toBe('black');
      expect(l.font).toEqual({ size: 18, family: 'sans-serif' });
      expect(l.padding).toBe(4);
      expect(l.borderRadius).toBe(7);
      expect(l.position).toBe('center');
      expect(l.yAdjust).toBe(-10);
      expect(l.rotation === undefined || l.rotation === 0, 'no rotation').toBe(true);
    });
  });

  it('a missing percentile leaves a gap: p50 and p99 keep the colours and label heights of index 0 and 2', async () => {
    const p = must(v.make(0).percentiles);
    serve(v, { percentiles: { ...p, p90: null, p999: null } });
    await open();
    const o = optsOf(await show(v));
    expect(annotations(o)).toHaveLength(4);
    expect(labels(o).map(contentOf)).toEqual(['p50', 'p99']);
    for (const [name, i] of [
      ['p50', 0],
      ['p99', 2],
    ] as const) {
      const l = must(labels(o).find((x) => contentOf(x) === name));
      expect(lineOfLabel(o, l).borderColor, name).toBe(v.palette[i]);
      expect(l.backgroundColor, name).toBe(`${v.palette[i]}80`);
      expect(l.yValue as number, name).toBeCloseTo(Math.max(...percent(v.make(0))) * 0.95 * (1 - 0.065 * i), 9);
    }
  });

  it('no annotation at all without a percentile', async () => {
    serve(v, { percentiles: { p50: null, p90: null, p99: null, p999: null } });
    await open();
    expect(annotations(optsOf(await show(v)))).toEqual([]);
  });
});

describe.each(VIEWS)('Spec 58 H1 and H3: the Hide percentiles button of the $name view', (v) => {
  it('starts as "Hide percentiles": btn btn-sm btn-outline-secondary, aria-pressed false', async () => {
    await open();
    await show(v);
    const b = button(0);
    expect(b.tagName).toBe('BUTTON');
    expect(b).toHaveTextContent(HIDE);
    expect(b).toHaveClass('btn', 'btn-sm', 'btn-outline-secondary');
    expect(b).not.toHaveClass('btn-secondary');
    expect(b).toHaveAttribute('aria-pressed', 'false');
  });

  it('is the only toggle: no per-line buttons of spec 56', async () => {
    await open();
    await show(v);
    expect(pressedButtons(0)).toEqual([button(0)]);
    expect(within(card(0)).queryByRole('button', { name: /^p\d/ })).toBeNull();
  });

  it('sits top right in a row above the histogram tables', async () => {
    await open();
    await show(v);
    const row = must(button(0).parentElement);
    expect(row).toHaveClass('d-flex', 'justify-content-end', 'align-items-center', 'gap-2', 'mb-2');
    const first = must(histTables(card(0))[0]);
    expect(row.compareDocumentPosition(first) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(row.contains(first)).toBe(false);
  });

  it('there is no such button in the rate and loss views', async () => {
    await open();
    await show(v);
    expect(queryButton(0)).not.toBeNull();
    fireEvent.click(radio(0, 'Traffic rates'));
    expect(queryButton(0)).toBeNull();
    fireEvent.click(radio(0, 'Packet loss'));
    expect(queryButton(0)).toBeNull();
  });

  it('clicking hides every line and label: "Show percentiles", btn-secondary, aria-pressed true', async () => {
    await open();
    await show(v);
    fireEvent.click(button(0));
    const b = button(0);
    expect(b).toHaveTextContent(SHOW);
    expect(b).toHaveClass('btn', 'btn-sm', 'btn-secondary');
    expect(b).not.toHaveClass('btn-outline-secondary');
    expect(b).toHaveAttribute('aria-pressed', 'true');
    expect(annotations(optsOf(v.bar(card(0))))).toEqual([]);
  });

  it('clicking again restores the very same annotations and the first look', async () => {
    await open();
    const before = annotations(optsOf(await show(v)));
    fireEvent.click(button(0));
    fireEvent.click(button(0));
    expect(annotations(optsOf(v.bar(card(0))))).toEqual(before);
    expect(button(0)).toHaveTextContent(HIDE);
    expect(button(0)).toHaveAttribute('aria-pressed', 'false');
  });

  it('hiding leaves the tables, the bar data object and every other option alone', async () => {
    await open();
    const bar = await show(v);
    const tables = tableText(0);
    const data = propsOf(bar).data;
    const rest = (o: Opts) => ({ ...o, plugins: { ...o.plugins, annotation: undefined } });
    const options = rest(optsOf(bar));
    expect(tables).toHaveLength(3);
    fireEvent.click(button(0));
    expect(tableText(0)).toEqual(tables);
    expect(propsOf(v.bar(card(0))).data).toBe(data);
    expect(rest(optsOf(v.bar(card(0))))).toEqual(options);
    expect(v.bar(card(0)).getAttribute('data-chart')).toBe(bar.getAttribute('data-chart') ?? '');
  });
});

describe.each(VIEWS)('Spec 58 H2: where the one flag lives (starting from the $name view)', (v) => {
  it('is shared by the delay and the IAT view, in both directions', async () => {
    const w = OTHER(v);
    await open();
    await show(v);
    fireEvent.click(button(0));
    const wBar = await show(w);
    expect(button(0)).toHaveTextContent(SHOW);
    expect(annotations(optsOf(wBar))).toEqual([]);
    fireEvent.click(button(0));
    expect(button(0)).toHaveTextContent(HIDE);
    expect(annotations(optsOf(w.bar(card(0)))).length).toBe(8);
    await show(v);
    expect(button(0)).toHaveTextContent(HIDE);
    expect(annotations(optsOf(v.bar(card(0)))).length).toBe(8);
  });

  it('is kept per link tab: hiding on link 0 leaves link 1 shown, and back', async () => {
    await open();
    await show(v, 0);
    fireEvent.click(button(0));
    const bar1 = await show(v, 1);
    expect(button(1)).toHaveTextContent(HIDE);
    expect(annotations(optsOf(bar1))).toHaveLength(8);
    fireEvent.click(button(1));
    await show(v, 0);
    expect(button(0)).toHaveTextContent(SHOW);
    expect(annotations(optsOf(v.bar(card(0))))).toEqual([]);
    await show(v, 1);
    expect(button(1)).toHaveTextContent(SHOW);
  });

  it('survives a visit to the rate view', async () => {
    await open();
    await show(v);
    fireEvent.click(button(0));
    fireEvent.click(radio(0, 'Traffic rates'));
    const bar = await show(v);
    expect(button(0)).toHaveTextContent(SHOW);
    expect(annotations(optsOf(bar))).toEqual([]);
  });

  it('survives polls with a changed reply: it stays hidden while the bars move', async () => {
    await open();
    await show(v);
    fireEvent.click(button(0));
    const before = chartOf(v.bar(card(0))).datasets[0]?.data;
    const calls = v.api.mock.calls.length;
    v.api.mockImplementation(async (dir) => v.make(dir, { bins: [9, 8, 7, 6, 5] }));
    await tick(2500);
    expect(v.api.mock.calls.length).toBeGreaterThan(calls);
    expectBars(chartOf(v.bar(card(0))).datasets[0]?.data ?? [], v.make(0, { bins: [9, 8, 7, 6, 5] }));
    expect(chartOf(v.bar(card(0))).datasets[0]?.data).not.toEqual(before);
    expect(button(0)).toHaveTextContent(SHOW);
    expect(annotations(optsOf(v.bar(card(0))))).toEqual([]);
  });

  it('is not kept across a page load: after a remount the lines are shown', async () => {
    const first = await open();
    await show(v);
    fireEvent.click(button(0));
    first.unmount();
    await open();
    const bar = await show(v);
    expect(button(0)).toHaveTextContent(HIDE);
    expect(annotations(optsOf(bar))).toHaveLength(8);
    expect(annotations(optsOf(await show(OTHER(v))))).toHaveLength(8);
  });
});

describe.each(VIEWS)('Spec 58 R1 and R2: switching to the $name view', (v) => {
  it('shows the last data in the same render as the click, then refreshes in the background', async () => {
    await open();
    const first = await show(v);
    const shownData = first.getAttribute('data-chart');
    const shownTables = tableText(0);
    fireEvent.click(radio(0, 'Traffic rates'));
    expect(stubs(0)).toHaveLength(0);
    const reply = deferred<HistogramView>();
    v.api.mockImplementation(() => reply.promise);
    fireEvent.click(radio(0, v.radioName));
    // no tick, no await: the same render as the click
    const again = v.bar(card(0));
    expect(again.getAttribute('data-chart')).toBe(shownData);
    expect(optsOf(again).plugins?.annotation).toBeDefined();
    expect(annotations(optsOf(again))).toHaveLength(8);
    expect(tableText(0)).toEqual(shownTables);
    expect(chartBoxes(0)).toHaveLength(1);
    expect(v.api).toHaveBeenCalled();
    const p = must(v.make(0).percentiles);
    reply.resolve(v.make(0, { bins: [4, 3, 2, 1, 0], percentiles: { ...p, p50: must(p.p50) + v.make(0).bin_ns } }));
    await tick(0);
    expect(v.bar(card(0))).toBe(again);
    expectBars(chartOf(again).datasets[0]?.data ?? [], v.make(0, { bins: [4, 3, 2, 1, 0] }));
  });

  it('keeps the hide flag when it shows the cached data', async () => {
    await open();
    await show(v);
    fireEvent.click(button(0));
    fireEvent.click(radio(0, 'Packet loss'));
    v.api.mockImplementation(() => deferred<HistogramView>().promise);
    fireEvent.click(radio(0, v.radioName));
    expect(button(0)).toHaveTextContent(SHOW);
    expect(annotations(optsOf(v.bar(card(0))))).toEqual([]);
  });

  it('the first visit reserves the height: the chart box is there at once, no chart and no tables yet', async () => {
    await open();
    const reply = deferred<HistogramView>();
    v.api.mockImplementation(() => reply.promise);
    card(0);
    fireEvent.click(radio(0, v.radioName));
    expect(stubs(0)).toHaveLength(0);
    expect(histTables(card(0))).toHaveLength(0);
    const boxes = chartBoxes(0);
    expect(boxes, 'one element with class chart-box (250 px) while waiting').toHaveLength(1);
    const placeholder = must(boxes[0]);
    expect(within(card(0)).queryByRole('img', { name: /histogram of link/ })).toBeNull();
    expect(hasRatioRule(placeholder, 4), 'a CSS rule with aspect-ratio: 4 for the placeholder').toBe(true);
    const group = within(card(0)).getByRole('radiogroup');
    expect(placeholder.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    reply.resolve(v.make(0));
    await tick(0);
    expect(stubs(0)).toHaveLength(1);
    expect(histTables(card(0))).toHaveLength(3);
    expect(chartBoxes(0)).toHaveLength(1);
  });

  it('only a first visit waits: the other kind of the same link is still empty at its first visit', async () => {
    const w = OTHER(v);
    await open();
    await show(v);
    fireEvent.click(radio(0, 'Traffic rates'));
    w.api.mockImplementation(() => deferred<HistogramView>().promise);
    fireEvent.click(radio(0, w.radioName));
    expect(stubs(0)).toHaveLength(0);
    expect(histTables(card(0))).toHaveLength(0);
    expect(chartBoxes(0)).toHaveLength(1);
  });

  it('the cache is per link: link 1 is empty at its first visit although link 0 has data', async () => {
    await open();
    await show(v, 0);
    v.api.mockImplementation((dir) => (dir === 1 ? deferred<HistogramView>().promise : Promise.resolve(v.make(dir))));
    card(1);
    fireEvent.click(radio(1, v.radioName));
    expect(stubs(1)).toHaveLength(0);
    expect(histTables(card(1))).toHaveLength(0);
    expect(chartBoxes(1)).toHaveLength(1);
  });

  it('is not kept across a page load: a new page starts empty again', async () => {
    const first = await open();
    await show(v);
    first.unmount();
    await open();
    v.api.mockImplementation(() => deferred<HistogramView>().promise);
    card(0);
    fireEvent.click(radio(0, v.radioName));
    expect(stubs(0)).toHaveLength(0);
    expect(chartBoxes(0)).toHaveLength(1);
  });
});

describe.each(VIEWS)('Spec 58 R3 and R4: polling the $name view', (v) => {
  it('an equal reply creates no new chart data or options object and no new table text', async () => {
    await open();
    const bar = await show(v);
    const { data, options } = propsOf(bar);
    const tables = tableText(0);
    const calls = v.api.mock.calls.length;
    v.api.mockImplementation(async (dir) => v.make(dir)); // a new object with the same content
    await tick(3500);
    expect(v.api.mock.calls.length).toBeGreaterThanOrEqual(calls + 3);
    const now = propsOf(v.bar(card(0)));
    expect(now.data).toBe(data);
    expect(now.options).toBe(options);
    expect(tableText(0)).toEqual(tables);
  });

  it('an equal reply keeps the objects for the hidden state as well', async () => {
    await open();
    await show(v);
    fireEvent.click(button(0));
    const { data, options } = propsOf(v.bar(card(0)));
    v.api.mockImplementation(async (dir) => v.make(dir));
    await tick(2500);
    expect(propsOf(v.bar(card(0))).data).toBe(data);
    expect(propsOf(v.bar(card(0))).options).toBe(options);
  });

  it('a changed bin count updates data in place: same chart element, new data object', async () => {
    await open();
    const bar = await show(v);
    const { data } = propsOf(bar);
    v.api.mockImplementation(async (dir) => v.make(dir, { bins: [2, 4, 8, 16, 32] }));
    await tick(1500);
    expect(v.bar(card(0))).toBe(bar);
    expect(propsOf(bar).data).not.toBe(data);
    expectBars(chartOf(bar).datasets[0]?.data ?? [], v.make(0, { bins: [2, 4, 8, 16, 32] }));
  });

  it('a changed percentile moves its line in place: same chart element, new options object', async () => {
    await open();
    const bar = await show(v);
    const { options } = propsOf(bar);
    const before = Number(must(lines(optsOf(bar))[0]).xMin);
    const p = must(v.make(0).percentiles);
    v.api.mockImplementation(async (dir) =>
      v.make(dir, { percentiles: { ...p, p50: must(p.p50) + v.make(0).bin_ns } }),
    );
    await tick(1500);
    expect(v.bar(card(0))).toBe(bar);
    expect(propsOf(bar).options).not.toBe(options);
    expect(Number(must(lines(optsOf(bar))[0]).xMin)).toBeCloseTo(before + 1, 9);
  });

  it('a changed underflow alone is a change too: the tables follow', async () => {
    await open();
    const bar = await show(v);
    const tables = tableText(0);
    v.api.mockImplementation(async (dir) => v.make(dir, { underflow: v.make(dir).underflow + 123 }));
    await tick(1500);
    expect(v.bar(card(0))).toBe(bar);
    expect(tableText(0)).not.toEqual(tables);
  });

  it('only the selected view polls: leaving it stops its requests', async () => {
    await open();
    await show(v);
    fireEvent.click(radio(0, 'Traffic rates'));
    await tick(0);
    const calls = v.api.mock.calls.length;
    await tick(3500);
    expect(v.api.mock.calls.length).toBe(calls);
  });
});

// ---- the other P4TG style: S1 to S7 ---------------------------------------------------------------------------------------

type Axis = { beginAtZero?: boolean; title?: { text?: string; display?: boolean } };
type FullOpts = {
  responsive?: boolean;
  aspectRatio?: number;
  maintainAspectRatio?: boolean;
  interaction?: unknown;
  scales?: { x?: Axis; y?: Axis };
  plugins?: {
    legend?: { display?: boolean };
    title?: { display?: boolean; text?: string };
    tooltip?: { enabled?: boolean };
  };
};
const full = (el: HTMLElement) => optsOf(el) as FullOpts;
const rateChart = (d: number) => within(card(d)).getByTestId('chart-line');
const lossChart = (d: number) => {
  fireEvent.click(radio(d, 'Packet loss'));
  return within(card(d)).getByTestId('chart-line');
};
const mu = (s: string) => s.replace(/\u03bc/g, '\u00b5');

describe('Spec 58 S1 and S6: size and tooltips of the line charts', () => {
  it.each([
    ['rates', () => rateChart(0)],
    ['loss', () => lossChart(0)],
  ] as const)(
    'the %s chart is responsive with aspectRatio 6, no fixed ratio switch, no tooltips, no interaction',
    async (_n, get) => {
      await open();
      const o = full(get());
      expect(o.responsive).toBe(true);
      expect(o.aspectRatio).toBe(6);
      expect(o.maintainAspectRatio).not.toBe(false);
      expect(o.plugins?.tooltip?.enabled).toBe(false);
      expect(o.interaction).toBeUndefined();
    },
  );
});

describe('Spec 58 S7: the loss chart is filled like P4TG', () => {
  it('every dataset is filled, its background being its line colour at alpha 0.5', async () => {
    await open();
    const sets = chartOf(lossChart(0)).datasets as unknown as {
      fill?: unknown;
      borderColor: string;
      backgroundColor: string;
    }[];
    expect(sets).toHaveLength(9);
    for (const s of sets) {
      expect(s.fill, s.borderColor).toBe(true);
      const e = document.createElement('div');
      e.style.color = s.borderColor;
      const rgb = /rgb\((\d+), (\d+), (\d+)\)/.exec(e.style.color);
      const [, r, g, b] = must(rgb, `an rgb colour for ${s.borderColor}`);
      expect(norm(s.backgroundColor), s.borderColor).toBe(`rgba(${r},${g},${b},0.5)`);
    }
  });
});

describe.each(VIEWS)('Spec 58 S1 to S6: the chart of the $name view', (v) => {
  const name = v.name === 'IAT' ? 'IAT' : 'Delay';

  it('has the aspect ratio 4 and is responsive, without maintainAspectRatio false', async () => {
    await open();
    const o = full(await show(v));
    expect(o.responsive).toBe(true);
    expect(o.aspectRatio).toBe(4);
    expect(o.maintainAspectRatio).not.toBe(false);
  });

  it('has the default bar spacing (S2)', async () => {
    await open();
    const ds = must(chartOf(await show(v)).datasets[0]) as Record<string, unknown>;
    expect(ds.barPercentage).toBeUndefined();
    expect(ds.categoryPercentage).toBeUndefined();
  });

  it('the bars are bin / (bins + underflow + overflow) * 100, the y axis starts at zero and is titled "Probability (%)" (S3)', async () => {
    await open();
    const bar = await show(v);
    const h = v.make(0);
    expectBars(must(chartOf(bar).datasets[0]).data, h);
    const total = h.bins.reduce((a, b) => a + b, 0) + h.underflow + h.overflow;
    expect(must(chartOf(bar).datasets[0]).data[1]).toBeCloseTo(((h.bins[1] ?? 0) / total) * 100, 9);
    const o = full(bar);
    expect(o.scales?.y?.beginAtZero).toBe(true);
    expect(o.scales?.y?.title?.text).toBe('Probability (%)');
  });

  it('a histogram without any count draws zero bars, not NaN', async () => {
    serve(v, { bins: [0, 0, 0, 0, 0], underflow: 0, overflow: 0, count: 0, percentiles: undefined });
    await open();
    const bar = await show(v);
    expect(propsOf(bar).data).toBeDefined();
    const data = (propsOf(bar).data as Chart).datasets[0]?.data;
    expect(data).toEqual([0, 0, 0, 0, 0]);
  });

  it('under and overflow count in the total: more overflow, smaller bars', async () => {
    serve(v, { overflow: v.make(0).overflow + 1000 });
    await open();
    const bar = await show(v);
    const h = v.make(0, { overflow: v.make(0).overflow + 1000 });
    expectBars(must(chartOf(bar).datasets[0]).data, h);
    expect(percent(h)[0]).toBeLessThan(percent(v.make(0))[0] ?? 0);
  });

  it('the x axis is titled "<kind> Range"; the legend shows the dataset "<kind> distribution"; there is no chart title (S4, S5)', async () => {
    await open();
    const bar = await show(v);
    const o = full(bar);
    expect(o.scales?.x?.title?.text).toBe(`${name} Range`);
    expect(o.plugins?.legend?.display).toBe(true);
    expect(must(chartOf(bar).datasets[0] as unknown as { label: string }).label).toBe(`${name} distribution`);
    expect(o.plugins?.title?.display).not.toBe(true);
    expect(JSON.stringify(o.plugins?.title ?? {})).not.toMatch(/IAT|Delay/);
  });

  it('has no tooltips and no index interaction (S6)', async () => {
    await open();
    const o = full(await show(v));
    expect(o.plugins?.tooltip?.enabled).toBe(false);
    expect(o.interaction).toBeUndefined();
  });
});

describe('Spec 58 S4: the labels of the bins are ranges "<start> – <end> <unit>"', () => {
  const labelsOf = async (v: View, over: Partial<HistogramView> = {}) => {
    serve(v, over);
    await open();
    return (propsOf(await show(v)).data as { labels: string[] }).labels.map(mu);
  };
  const [DELAY, IAT] = [must(VIEWS[0]), must(VIEWS[1])];

  it('delay: 900 µs to 1.4 ms in 5 bins; the first bin crosses a unit and names both', async () => {
    expect(await labelsOf(DELAY)).toEqual([
      '900.00 \u00b5s \u2013 1.00 ms',
      '1.00 \u2013 1.10 ms',
      '1.10 \u2013 1.20 ms',
      '1.20 \u2013 1.30 ms',
      '1.30 \u2013 1.40 ms',
    ]);
  });

  it('IAT: 0 to 10 µs in 5 bins, each label with its own unit (no longer one unit for the axis)', async () => {
    expect(await labelsOf(IAT)).toEqual([
      '0.00 ns \u2013 2.00 \u00b5s',
      '2.00 \u2013 4.00 \u00b5s',
      '4.00 \u2013 6.00 \u00b5s',
      '6.00 \u2013 8.00 \u00b5s',
      '8.00 \u2013 10.00 \u00b5s',
    ]);
  });

  it('ns to µs at 1000 ns, like getTimeUnit', async () => {
    expect(
      await labelsOf(DELAY, { lo_ns: 500, bin_ns: 250, bins: [1, 1, 1], edges_ns: [500, 750, 1000, 1250] }),
    ).toEqual(['500.00 \u2013 750.00 ns', '750.00 ns \u2013 1.00 \u00b5s', '1.00 \u2013 1.25 \u00b5s']);
  });

  it('ms to s at 1e9 ns: 500 ms to 2 s', async () => {
    expect(
      await labelsOf(IAT, {
        lo_ns: 5e8,
        bin_ns: 5e8,
        bins: [1, 1, 1],
        edges_ns: [5e8, 1e9, 1.5e9, 2e9],
        percentiles: undefined,
      }),
    ).toEqual(['500.00 ms \u2013 1.00 s', '1.00 \u2013 1.50 s', '1.50 \u2013 2.00 s']);
  });
});
