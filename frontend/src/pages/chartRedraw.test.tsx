import { fireEvent, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { HistogramView } from '../api/types';
import { propsOf } from '../test/chartProps';
import { card, delayBar, lossDonut, summary } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';

// Spec 40: the rate, loss and delay-histogram charts redraw like P4TG's: Chart.js's default animation (unless the user
// prefers reduced motion), x ticks that skip themselves without a cap, and Chart.js's default point radius. The loss
// donuts keep their options. Identity of options/data across a poll without a change is pinned in dashboardMemo.test.tsx;
// here only the reduced-motion variant of it.

type Opts = {
  animation?: unknown;
  elements?: { point?: { radius?: unknown } };
  scales?: { x?: { ticks?: { autoSkip?: unknown; maxTicksLimit?: unknown } } };
};

const optionsOf = (el: HTMLElement): Opts => propsOf(el).options as Opts;
// Spec 51: a panel holds several Bar / Doughnut stubs (delay and IAT, loss and frame sizes), so they are picked by their image box.
const chart = (scope: HTMLElement, id: 'chart-line' | 'chart-bar' | 'chart-doughnut') =>
  id === 'chart-bar' ? delayBar(scope) : id === 'chart-doughnut' ? lossDonut(scope) : within(scope).getByTestId(id);

function reduceMotion(reduce: boolean): void {
  const base = window.matchMedia;
  window.matchMedia = vi.fn((query: string) => {
    const mql = base(query);
    return query.includes('prefers-reduced-motion') ? { ...mql, matches: reduce && query.includes('reduce') } : mql;
  }) as unknown as typeof window.matchMedia;
}

beforeEach(() => {
  vi.useFakeTimers();
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { counters: fx.counters({ loss_drops: 4 }) })]));
  api.getDelayHistogram.mockResolvedValue(fx.histogram(0) as HistogramView);
  serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 5) } });
});

async function open() {
  renderApp('/');
  await tick(0);
  await tick(0);
}

/** The rate, loss and histogram chart of link 0, in this order. */
async function linkCharts(): Promise<Record<'rate' | 'loss' | 'histogram', HTMLElement>> {
  const rate = chart(card(0), 'chart-line');
  fireEvent.click(within(card(0)).getByRole('radio', { name: 'Packet loss' }));
  const loss = chart(card(0), 'chart-line');
  fireEvent.click(within(card(0)).getByRole('radio', { name: 'Delay histogram' }));
  await tick(0);
  await tick(0);
  const histogram = chart(card(0), 'chart-bar');
  return { rate, loss, histogram };
}

describe('spec 40: default motion', () => {
  it.each(['rate', 'loss', 'histogram'] as const)(
    'the %s chart is animated by default (no animation: false)',
    async (view) => {
      await open();
      const o = optionsOf((await linkCharts())[view]);
      expect(o.animation).not.toBe(false);
    },
  );

  it.each(['rate', 'loss', 'histogram'] as const)(
    'the %s chart: x ticks autoSkip, without a maxTicksLimit',
    async (view) => {
      await open();
      const ticks = optionsOf((await linkCharts())[view]).scales?.x?.ticks;
      expect(ticks?.autoSkip).toBe(true);
      expect(ticks).not.toHaveProperty('maxTicksLimit');
    },
  );

  it.each(['rate', 'loss'] as const)('the %s chart: points keep the Chart.js default radius (not 0)', async (view) => {
    await open();
    const o = optionsOf((await linkCharts())[view]);
    expect(o.elements?.point?.radius).not.toBe(0);
  });

  it('the Summary charts are animated and skip ticks too', async () => {
    await open();
    const o = optionsOf(chart(summary(), 'chart-line'));
    expect(o.animation).not.toBe(false);
    expect(o.scales?.x?.ticks?.autoSkip).toBe(true);
    expect(o.scales?.x?.ticks).not.toHaveProperty('maxTicksLimit');
    expect(o.elements?.point?.radius).not.toBe(0);
  });
});

describe('spec 40: prefers-reduced-motion: reduce', () => {
  it.each(['rate', 'loss', 'histogram'] as const)('the %s chart has animation: false', async (view) => {
    reduceMotion(true);
    await open();
    expect(optionsOf((await linkCharts())[view]).animation).toBe(false);
  });

  it('the Summary chart has animation: false', async () => {
    reduceMotion(true);
    await open();
    expect(optionsOf(chart(summary(), 'chart-line')).animation).toBe(false);
  });

  it('the other spec 40 options hold under reduced motion too', async () => {
    reduceMotion(true);
    await open();
    const o = optionsOf((await linkCharts()).rate);
    expect(o.scales?.x?.ticks?.autoSkip).toBe(true);
    expect(o.scales?.x?.ticks).not.toHaveProperty('maxTicksLimit');
    expect(o.elements?.point?.radius).not.toBe(0);
  });

  it('a poll without new data keeps the options and data objects of the rate chart', async () => {
    reduceMotion(true);
    await open();
    const before = propsOf(chart(card(0), 'chart-line'));
    await tick(0);
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { counters: fx.counters({ loss_drops: 4 }) })]));
    await tick(1000);
    const after = propsOf(chart(card(0), 'chart-line'));
    expect(after.options).toBe(before.options);
    expect(after.data).toBe(before.data);
  });
});

describe('spec 40: the loss donut keeps its options', () => {
  it.each([false, true])('animation: false, no tick or point options (reduced motion %s)', async (reduce) => {
    reduceMotion(reduce);
    await open();
    const o = propsOf(chart(card(0), 'chart-doughnut')).options as Record<string, unknown>;
    expect(o.animation).toBe(false);
    expect(o.responsive).toBe(true);
    expect(o.maintainAspectRatio).toBe(false);
    expect(o).not.toHaveProperty('scales');
    expect(o.plugins).toMatchObject({ legend: { display: false } });
  });
});
