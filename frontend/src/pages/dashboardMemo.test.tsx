import { fireEvent, screen, within } from '@testing-library/react';
import React from 'react';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import * as fx from '../test/fixtures';
import { api, serveHistory } from '../test/mockApi';
import { renderApp, tick } from '../test/helpers';
import { card, delayBar, lossDonut } from '../test/dashboardUi';
import { must } from '../test/must';

// Spec 35 (F7): the loss donut is memoised too: its data depend on the six counts only, its options on the theme only (the
// Chart.js tooltip callback is part of the options, so a counter change must not rebuild them). The stub below records the
// Doughnut like the other charts. The link's chart area is a tab now (`card(d)` of src/test/dashboardUi.tsx), "Drops" is "Loss".
//
// Spec 28b item 2: rendering cost. "Chart data/options are memoized on their inputs (history, histogram, theme, selected
// view); DirectionCard and Histogram do not recompute charts on unrelated re-renders; the elapsed pill ticks in its own
// component, so the 1 Hz tick re-renders only that pill." The last clause is superseded: the pill has no tick any more.
//
// What can be observed, and is pinned here: this file replaces the chart stubs of test/setup.ts by ones that record the
// `data` and `options` objects (by reference) and count their renders.
//   - an unrelated re-render (theme toggle: only the options may change; a new sample: only the data may change) hands the
//     chart the same object for the input that did not change;
//   - a stats poll that changes only `elapsed_ms` (the pill; see the last describe, spec 29 round 2) leaves data and options
//     as they were.
// NOT observable here, left to the review: which `useMemo`/`memo` calls the implementation uses, and that the dependency
// lists are right (a memo that never recomputes fails the tests with changed inputs in dashboardHistory.test.tsx and
// dashboardP4tg.test.tsx; one that always recomputes fails the identity tests below).

interface Seen {
  data: unknown;
  options: unknown;
  /** The DOM element of that render (set at commit), to tell the charts of one kind apart. */
  el: Element | null;
}
const seen = vi.hoisted(() => ({ line: [] as Seen[], bar: [] as Seen[], doughnut: [] as Seen[] }));

vi.mock('react-chartjs-2', () => {
  const stub = (kind: 'line' | 'bar' | 'doughnut') => (props: { data?: unknown; options?: unknown }) => {
    const entry: Seen = { data: props.data, options: props.options, el: null };
    seen[kind].push(entry);
    return React.createElement('div', {
      ref: (el: Element | null) => {
        entry.el = el;
      },
      'data-testid': `chart-${kind}`,
      'data-chart': JSON.stringify(props.data ?? null),
      'data-options': JSON.stringify(props.options ?? null),
    });
  };
  return { Bar: stub('bar'), Line: stub('line'), Doughnut: stub('doughnut') };
});

const last = (kind: 'line') => must(seen[kind].at(-1), `a rendered ${kind} chart`);
// Spec 51: the link view has two doughnuts (loss, frame sizes) and two bar charts (delay, IAT): the chart under test is the
// newest render of the stub whose element is the one in its image box.
const lastAt = (kind: 'bar' | 'doughnut', el: Element) =>
  must(seen[kind].filter((s) => s.el === el).at(-1), `a rendered ${kind} chart`);
const lastDelay = () => lastAt('bar', delayBar(card(0)));
const lastLoss = () => lastAt('doughnut', lossDonut(card(0)));
const toggleTheme = () => fireEvent.click(screen.getByRole('button', { name: 'Toggle dark mode' }));

beforeEach(() => {
  seen.line.length = 0;
  seen.bar.length = 0;
  seen.doughnut.length = 0;
  vi.useFakeTimers();
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  // one link only: every chart on the page is then the one under test (the stubs record all charts alike)
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { counters: fx.counters({ loss_drops: 4 }) })]));
  serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 5) } });
});

async function open() {
  renderApp('/');
  await tick(0);
  await tick(0);
}

describe('Dashboard (spec 28b item 2): charts keep their data and options objects while their inputs are unchanged', () => {
  it('the Rate chart: a theme toggle renews the options and keeps the data object', async () => {
    await open();
    const before = last('line');
    toggleTheme();
    await tick(0);
    const after = last('line');
    expect(after.options).not.toBe(before.options); // the colours changed, so the options really are new
    expect(after.data).toBe(before.data);
  });

  it('the Loss chart: the same', async () => {
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Packet loss' }));
    await tick(0);
    const before = last('line');
    toggleTheme();
    await tick(0);
    expect(last('line').options).not.toBe(before.options);
    expect(last('line').data).toBe(before.data);
  });

  it('the Rate chart: a new sample renews the data and keeps the options object', async () => {
    const samples = { '0': fx.historySamples(0, 5) };
    serveHistory({ startedAt: 1762000000, samples });
    await open();
    const before = last('line');
    samples['0'].push(fx.historySample(5000));
    await tick(1000);
    await tick(1000);
    const after = last('line');
    expect(JSON.parse(JSON.stringify(after.data)).labels).toHaveLength(6); // the data really did change
    expect(after.data).not.toBe(before.data);
    expect(after.options).toBe(before.options);
  });

  it('the Loss chart: a new sample keeps the options object', async () => {
    const samples = { '0': fx.historySamples(0, 5) };
    serveHistory({ startedAt: 1762000000, samples });
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Packet loss' }));
    await tick(0);
    const before = last('line');
    samples['0'].push(fx.historySample(5000));
    await tick(1000);
    await tick(1000);
    expect(last('line').data).not.toBe(before.data);
    expect(last('line').options).toBe(before.options);
  });

  it('the delay histogram: a theme toggle renews the options and keeps the data object (same reply, same bins)', async () => {
    const h = fx.histogram(0);
    api.getDelayHistogram.mockResolvedValue(h as HistogramView);
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Delay histogram' }));
    await tick(0);
    await tick(0);
    const before = lastDelay();
    toggleTheme();
    await tick(0);
    expect(lastDelay().options).not.toBe(before.options);
    expect(lastDelay().data).toBe(before.data);
  });

  it('the delay histogram: a poll with the same reply object changes neither data nor options', async () => {
    const h = fx.histogram(0);
    api.getDelayHistogram.mockResolvedValue(h as HistogramView);
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Delay histogram' }));
    await tick(0);
    await tick(0);
    const before = lastDelay();
    await tick(1000); // a poll fetches the same object again
    const after = lastDelay();
    expect(after.data).toBe(before.data);
    expect(after.options).toBe(before.options);
  });
});

describe('Dashboard (spec 35 F7): the loss donut keeps its data and options objects while their inputs are unchanged', () => {
  const withCounts = (counters: Parameters<typeof fx.counters>[0]) =>
    fx.stats([fx.dirStats(0, { counters: fx.counters(counters) })]);

  it('a theme toggle renews the options and keeps the data object', async () => {
    await open();
    const before = lastLoss();
    toggleTheme();
    await tick(0);
    expect(lastLoss().options).not.toBe(before.options);
    expect(lastLoss().data).toBe(before.data);
  });

  it('a poll with fresh stats objects but the same counters changes neither data nor options', async () => {
    await open();
    const before = lastLoss();
    api.getStats.mockResolvedValue(withCounts({ loss_drops: 4 })); // new objects, same counts
    await tick(2500);
    expect(lastLoss().data).toBe(before.data);
    expect(lastLoss().options).toBe(before.options);
  });

  it('a changed counter renews the data and keeps the options object', async () => {
    await open();
    const before = lastLoss();
    api.getStats.mockResolvedValue(withCounts({ loss_drops: 9 }));
    await tick(2500);
    const after = lastLoss();
    expect(after.data).not.toBe(before.data);
    expect(JSON.parse(JSON.stringify(after.data)).datasets[0].data[0]).toBe(9);
    expect(after.options).toBe(before.options);
  });

  it('a new rate sample (a chart history poll) does not touch the donut', async () => {
    const samples = { '0': fx.historySamples(0, 5) };
    serveHistory({ startedAt: 1762000000, samples });
    await open();
    const before = lastLoss();
    samples['0'].push(fx.historySample(5000));
    await tick(1000);
    await tick(1000);
    expect(lastLoss().data).toBe(before.data);
    expect(lastLoss().options).toBe(before.options);
  });

  it('switching the chart radio does not rebuild the donut data or options', async () => {
    await open();
    const before = lastLoss();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Packet loss' }));
    await tick(0);
    expect(lastLoss().data).toBe(before.data);
    expect(lastLoss().options).toBe(before.options);
  });
});

// Spec 29, second round: the elapsed pill no longer ticks by itself (it shows the `elapsed_ms` of the stats poll, with no
// browser clock). Was: "the elapsed pill ticks alone", three tests that held every poll back and checked that the 1 Hz tick
// rendered no chart. A poll is now the only thing that changes the pill, so the pinned property is: a stats poll that changes
// only `elapsed_ms` leaves the charts' `data` and `options` objects as they were.
describe('Dashboard (spec 28b item 2 / spec 29): a stats poll that changes only elapsed_ms leaves the charts alone', () => {
  /** Every other request answers once and never again; getStats answers every poll with fresh objects and a later elapsed_ms. */
  function onlyStatsKeepAnswering() {
    const once = <T,>(value: T) => {
      let n = 0;
      return () => (n++ === 0 ? Promise.resolve(value) : new Promise<T>(() => undefined));
    };
    let elapsed = 192_000;
    api.getEmulation.mockImplementation(once(fx.emulation('running')));
    api.getStatsHistory.mockImplementation(once(fx.history({ '0': fx.historySamples(0, 5) })));
    api.getDelayHistogram.mockImplementation(once(fx.histogram(0)));
    api.getStats.mockImplementation(async () => {
      elapsed += 1000;
      return fx.stats([fx.dirStats(0)], { elapsed_ms: elapsed });
    });
  }

  it('Rate chart: the pill moves on, data and options are the same objects', async () => {
    onlyStatsKeepAnswering();
    await open();
    expect(screen.getByTestId('pill-elapsed')).toHaveTextContent('0h 3m 13s');
    const before = last('line');
    await tick(1000);
    expect(screen.getByTestId('pill-elapsed')).toHaveTextContent('0h 3m 14s');
    expect(last('line').data).toBe(before.data);
    expect(last('line').options).toBe(before.options);
  });

  it('Loss chart: the same', async () => {
    onlyStatsKeepAnswering();
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Packet loss' }));
    await tick(0);
    const before = last('line');
    await tick(1000);
    await tick(1000);
    expect(screen.getByTestId('pill-elapsed')).toHaveTextContent('0h 3m 15s');
    expect(last('line').data).toBe(before.data);
    expect(last('line').options).toBe(before.options);
  });

  it('delay histogram: the same', async () => {
    onlyStatsKeepAnswering();
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Delay histogram' }));
    await tick(0);
    await tick(0);
    expect(delayBar(card(0))).toBeInTheDocument();
    const before = lastDelay();
    await tick(2000);
    expect(screen.getByTestId('pill-elapsed')).toHaveTextContent('0h 3m 15s');
    expect(lastDelay().data).toBe(before.data);
    expect(lastDelay().options).toBe(before.options);
  });

  it('loss donut: the same (spec 35)', async () => {
    onlyStatsKeepAnswering();
    await open();
    expect(lossDonut(card(0))).toBeInTheDocument();
    const before = lastLoss();
    await tick(2000);
    expect(screen.getByTestId('pill-elapsed')).toHaveTextContent('0h 3m 15s');
    expect(lastLoss().data).toBe(before.data);
    expect(lastLoss().options).toBe(before.options);
  });
});
