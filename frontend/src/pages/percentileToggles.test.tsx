import { fireEvent, waitFor, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import { card, delayBar, findTabs, histTables, iatBar, radio } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api } from '../test/mockApi';
import { must } from '../test/must';

// Spec 56 P1 to P3 (user 2026-11-05: "like clicking on the TX or RX rates"). Spec: .git/claude-specs/spec56-percentile-toggles.md.
// Contract of the DOM, in the delay view and in the IAT view alike:
//   - one `<button aria-pressed>` per percentile line the chart draws, text "p50" | "p90" | "p99" | "p99.9" (DOM order), inside the
//     link's tabpanel, in front of the bar chart (after the tables or as the chart's legend row);
//   - CHOICE: the colour box is a descendant of the button with an INLINE `background-color` (or `border-color`), like Chart.js's
//     legend box; it reads the annotation line's `borderColor` (compared after normalising both through the browser's colour parser);
//   - CHOICE: "struck through" is an inline `text-decoration` with `line-through` on the button or a descendant, or a class
//     containing `line-through` (Bootstrap's `text-decoration-line-through`);
//   - the lines are the annotations of `data-options` (plugins.annotation.annotations), found by the percentile name at the start of
//     their label text, like dashboardP4tg.test.tsx.

type View = {
  name: string;
  radioName: string;
  bar: (scope: HTMLElement) => HTMLElement;
  api: typeof api.getDelayHistogram;
  make: (dir: number, over?: Partial<HistogramView>) => HistogramView;
};
const VIEWS: View[] = [
  { name: 'delay', radioName: 'Delay histogram', bar: delayBar, api: api.getDelayHistogram, make: fx.histogram },
  { name: 'IAT', radioName: 'IAT histogram', bar: iatBar, api: api.getIatHistogram, make: fx.iatHistogram },
];
const NAMES = ['p50', 'p90', 'p99', 'p99.9'];
const OTHER = (v: View) => must(VIEWS.find((x) => x !== v));

type Opts = { plugins?: { annotation?: { annotations?: unknown } } } & Record<string, unknown>;
const optsOf = (el: HTMLElement): Opts => JSON.parse(el.getAttribute('data-options') ?? 'null');
const dataOf = (el: HTMLElement): unknown => el.getAttribute('data-chart');
const annotations = (o: Opts): Record<string, unknown>[] => {
  const a = o.plugins?.annotation?.annotations;
  if (a === undefined || a === null) return [];
  return (Array.isArray(a) ? a : Object.values(a as object)) as Record<string, unknown>[];
};
const lineOf = (o: Opts, name: string) =>
  annotations(o).filter((a) => new RegExp(`"${name.replace('.', '\\.')}(?![.\\d])`).test(JSON.stringify(a)));
const lineNames = (o: Opts) => NAMES.filter((n) => lineOf(o, n).length > 0);
/** The options without the annotations: what hiding a line must not touch. */
const withoutLines = (o: Opts) => ({ ...o, plugins: { ...o.plugins, annotation: undefined } });
const colour = (c: unknown): string => {
  const e = document.createElement('div');
  e.style.color = String(c);
  return e.style.color;
};

const toggles = (d: number) =>
  within(card(d))
    .queryAllByRole('button')
    .filter((b) => b.hasAttribute('aria-pressed') && NAMES.includes((b.textContent ?? '').trim()));
const toggle = (d: number, name: string) =>
  must(
    toggles(d).find((b) => (b.textContent ?? '').trim() === name),
    `a toggle button "${name}" with aria-pressed`,
  );
const pressed = (d: number) =>
  toggles(d).map((b) => `${(b.textContent ?? '').trim()}:${b.getAttribute('aria-pressed')}`);
const boxColour = (b: HTMLElement): string => {
  const box = Array.from(b.querySelectorAll<HTMLElement>('*')).find(
    (e) => e.style.backgroundColor || e.style.borderColor,
  );
  const e = must(box, 'a colour box inside the toggle');
  return colour(e.style.backgroundColor || e.style.borderColor);
};
const struck = (b: HTMLElement) =>
  [b, ...Array.from(b.querySelectorAll('*'))].some(
    (e) => /line-through/.test(e.getAttribute('style') ?? '') || /line-through/.test(e.getAttribute('class') ?? ''),
  );
const tableText = (d: number) => histTables(card(d)).map((t) => t.textContent);

function serve(v: View, over: Partial<HistogramView> = {}) {
  v.api.mockImplementation(async (dir) => v.make(dir, over));
}
async function open(n = 2) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)].slice(0, n)));
  return renderApp('/');
}
/** Shows `v` of link `d` (selects the tab, chooses the radio if needed) and returns its bar chart. */
async function show(v: View, d = 0): Promise<HTMLElement> {
  await waitFor(() => expect(within(card(d)).queryAllByRole('radio').length).toBeGreaterThan(0));
  if (!(radio(d, v.radioName) as HTMLInputElement).checked) fireEvent.click(radio(d, v.radioName));
  await waitFor(() => expect(v.bar(card(d))).toBeInTheDocument());
  return v.bar(card(d));
}

describe.each(VIEWS)('Spec 56 P1: the percentile toggles of the $name view', (v) => {
  it('one toggle per drawn line, in order, all pressed and not struck through', async () => {
    await open();
    const bar = await show(v);
    expect(lineNames(optsOf(bar))).toEqual(NAMES);
    expect(pressed(0)).toEqual(NAMES.map((n) => `${n}:true`));
    for (const b of toggles(0)) expect(struck(b)).toBe(false);
  });

  it('are native buttons in front of the bar chart, outside the tables', async () => {
    await open();
    const bar = await show(v);
    expect(toggles(0)).toHaveLength(4);
    for (const b of toggles(0)) {
      expect(b.tagName).toBe('BUTTON');
      expect(histTables(card(0)).some((t) => t.contains(b))).toBe(false);
      expect(b.compareDocumentPosition(bar) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
  });

  it('only the percentiles that are present get a toggle (p50 and p99 here)', async () => {
    serve(v, {
      percentiles: {
        p50: v.make(0).percentiles?.p50 ?? null,
        p90: null,
        p99: v.make(0).percentiles?.p99 ?? null,
        p999: null,
      },
    });
    await open();
    const bar = await show(v);
    expect(lineNames(optsOf(bar))).toEqual(['p50', 'p99']);
    expect(pressed(0)).toEqual(['p50:true', 'p99:true']);
  });

  it('no toggle at all when no percentile is present', async () => {
    serve(v, { percentiles: { p50: null, p90: null, p99: null, p999: null } });
    await open();
    await show(v);
    expect(toggles(0)).toHaveLength(0);
  });

  it('each toggle has a colour box in the colour of its annotation line', async () => {
    await open();
    const o = optsOf(await show(v));
    for (const n of NAMES) {
      const line = must(lineOf(o, n)[0]);
      expect(boxColour(toggle(0, n)), n).toBe(colour(line.borderColor));
    }
  });

  it('there are no toggles in the rate and loss views', async () => {
    await open();
    await show(v);
    expect(toggles(0)).toHaveLength(4);
    fireEvent.click(radio(0, 'Traffic rates'));
    expect(toggles(0)).toHaveLength(0);
    fireEvent.click(radio(0, 'Packet loss'));
    expect(toggles(0)).toHaveLength(0);
  });

  it('clicking a toggle hides exactly that annotation: aria-pressed false, label struck through', async () => {
    await open();
    const bar = await show(v);
    const before = optsOf(bar);
    fireEvent.click(toggle(0, 'p90'));
    const after = optsOf(v.bar(card(0)));
    expect(lineNames(after)).toEqual(['p50', 'p99', 'p99.9']);
    expect(pressed(0)).toEqual(['p50:true', 'p90:false', 'p99:true', 'p99.9:true']);
    expect(struck(toggle(0, 'p90'))).toBe(true);
    for (const n of ['p50', 'p99', 'p99.9']) {
      expect(lineOf(after, n), n).toEqual(lineOf(before, n));
      expect(struck(toggle(0, n)), n).toBe(false);
    }
  });

  it('clicking again restores the very same line and the pressed look', async () => {
    await open();
    const before = optsOf(await show(v));
    fireEvent.click(toggle(0, 'p99.9'));
    fireEvent.click(toggle(0, 'p99.9'));
    expect(optsOf(v.bar(card(0)))).toEqual(before);
    expect(pressed(0)).toEqual(NAMES.map((n) => `${n}:true`));
    expect(struck(toggle(0, 'p99.9'))).toBe(false);
  });

  it('every line can be hidden; the toggles stay so that it can be shown again', async () => {
    await open();
    await show(v);
    for (const n of NAMES) fireEvent.click(toggle(0, n));
    expect(annotations(optsOf(v.bar(card(0))))).toEqual([]);
    expect(pressed(0)).toEqual(NAMES.map((n) => `${n}:false`));
    fireEvent.click(toggle(0, 'p50'));
    expect(lineNames(optsOf(v.bar(card(0))))).toEqual(['p50']);
  });
});

describe.each(VIEWS)('Spec 56 P3: hiding a line leaves the tables and the bars alone ($name view)', (v) => {
  it('the three tables, the bar data and every other option stay as they are, hidden or shown', async () => {
    await open();
    const bar = await show(v);
    const tables = tableText(0);
    const data = dataOf(bar);
    const options = withoutLines(optsOf(bar));
    expect(tables).toHaveLength(3);
    for (const n of ['p50', 'p99']) fireEvent.click(toggle(0, n));
    expect(tableText(0)).toEqual(tables);
    expect(dataOf(v.bar(card(0)))).toBe(data);
    expect(withoutLines(optsOf(v.bar(card(0))))).toEqual(options);
    expect(card(0).querySelectorAll('table')).toHaveLength(histTables(card(0)).length + 2);
    fireEvent.click(toggle(0, 'p50'));
    expect(tableText(0)).toEqual(tables);
    expect(dataOf(v.bar(card(0)))).toBe(data);
  });

  it('the percentile table still lists a hidden percentile', async () => {
    await open();
    await show(v);
    fireEvent.click(toggle(0, 'p90'));
    const t = must(histTables(card(0))[2]);
    expect(
      within(t)
        .getAllByRole('columnheader')
        .map((h) => h.textContent?.trim()),
    ).toEqual(NAMES);
    const row = must(within(t).getAllByRole('row')[1]);
    expect(Array.from(row.children).map((c) => c.textContent?.trim())).not.toContain('-');
  });
});

describe.each(VIEWS)('Spec 56 P2: where the state lives ($name view)', (v) => {
  it('is kept per link tab: hiding p50 of link 0 leaves link 1 untouched, and back', async () => {
    await open();
    await show(v, 0);
    fireEvent.click(toggle(0, 'p50'));
    const bar1 = await show(v, 1);
    expect(pressed(1)).toEqual(NAMES.map((n) => `${n}:true`));
    expect(lineNames(optsOf(bar1))).toEqual(NAMES);
    fireEvent.click(toggle(1, 'p90'));
    const bar0 = await show(v, 0);
    expect(pressed(0)).toEqual(['p50:false', 'p90:true', 'p99:true', 'p99.9:true']);
    expect(lineNames(optsOf(bar0))).toEqual(['p90', 'p99', 'p99.9']);
    const again = await show(v, 1);
    expect(pressed(1)).toEqual(['p50:true', 'p90:false', 'p99:true', 'p99.9:true']);
    expect(lineNames(optsOf(again))).toEqual(['p50', 'p99', 'p99.9']);
  });

  it('is separate from the other view (delay vs IAT) of the same link, in both directions', async () => {
    const w = OTHER(v);
    await open();
    await show(v);
    fireEvent.click(toggle(0, 'p50'));
    const wBar = await show(w);
    expect(pressed(0)).toEqual(NAMES.map((n) => `${n}:true`));
    expect(lineNames(optsOf(wBar))).toEqual(NAMES);
    fireEvent.click(toggle(0, 'p99'));
    const vBar = await show(v);
    expect(pressed(0)).toEqual(['p50:false', 'p90:true', 'p99:true', 'p99.9:true']);
    expect(lineNames(optsOf(vBar))).toEqual(['p90', 'p99', 'p99.9']);
    await show(w);
    expect(pressed(0)).toEqual(['p50:true', 'p90:true', 'p99:false', 'p99.9:true']);
  });

  it('survives a visit to the rate view', async () => {
    await open();
    await show(v);
    fireEvent.click(toggle(0, 'p99'));
    fireEvent.click(radio(0, 'Traffic rates'));
    const bar = await show(v);
    expect(pressed(0)).toEqual(['p50:true', 'p90:true', 'p99:false', 'p99.9:true']);
    expect(lineNames(optsOf(bar))).toEqual(['p50', 'p90', 'p99.9']);
  });

  it('survives polls: new values arrive, the hidden line stays hidden, the others move', async () => {
    vi.useFakeTimers();
    await open();
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, v.radioName));
    await tick(0);
    fireEvent.click(toggle(0, 'p90'));
    const x = (o: Opts, n: string) => must(lineOf(o, n)[0]).xMin;
    const first = x(optsOf(v.bar(card(0))), 'p50');
    const base = must(v.make(0).percentiles);
    const callsBefore = v.api.mock.calls.length;
    v.api.mockImplementation(async (dir) =>
      v.make(dir, { percentiles: { ...base, p50: must(base.p50) + v.make(0).bin_ns } }),
    );
    await tick(2500);
    expect(v.api.mock.calls.length).toBeGreaterThan(callsBefore);
    const o = optsOf(v.bar(card(0)));
    expect(x(o, 'p50')).not.toBe(first);
    expect(lineNames(o)).toEqual(['p50', 'p99', 'p99.9']);
    expect(pressed(0)).toEqual(['p50:true', 'p90:false', 'p99:true', 'p99.9:true']);
    expect(struck(toggle(0, 'p90'))).toBe(true);
  });

  it('is not kept across a page load: after a remount every toggle is on', async () => {
    const first = await open();
    await show(v);
    fireEvent.click(toggle(0, 'p50'));
    fireEvent.click(toggle(0, 'p99.9'));
    first.unmount();
    await open();
    const bar = await show(v);
    expect(pressed(0)).toEqual(NAMES.map((n) => `${n}:true`));
    expect(lineNames(optsOf(bar))).toEqual(NAMES);
    const other = await show(OTHER(v));
    expect(lineNames(optsOf(other))).toEqual(NAMES);
    await findTabs(2);
  });
});
