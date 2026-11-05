import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { Counters, DirectionStats } from '../api/types';
import { propsOf } from '../test/chartProps';
import { card, CAUSE_HELP, CAUSES, findCard, lossDonut, queryLossDonut, radio, tab } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 47 F1: nine causes (the three new ones are rx_port_buffer_discards, rx_phy_discards, tx_phy_discards). Below, "the six" /
// "six" are the spec 35 wording of the first six; every list is now the nine of CAUSES.
// Spec 35 F5 (user 2026-11-05): below the chart of the selected link a donut with the distribution of the losses, a React
// legend with an explanation tooltip per cause, and on hover "<label>: <count> packets (<share> % of drops)". Contract (the
// header of src/test/dashboardUi.tsx has the test ids):
//   - the Doughnut (stub: `chart-doughnut`) has one dataset with nine values in the order of CAUSES, labels = CAUSES; live from
//     the link's `counters` (loss_drops, limiter_drops, buffer_full_drops, tx_drops, nic_rx_missed, nic_rx_nombuf,
//     nic_rx_port_buffer_discards, nic_rx_phy_discards, nic_tx_phy_discards), ended from
//     the history's `totals` (loss_drops, limiter_drops, buffer_full_drops, nic_tx_drops, nic_rx_missed, nic_rx_nombuf,
//     nic_rx_port_buffer_discards, nic_rx_phy_discards, nic_tx_phy_discards);
//   - the Chart.js legend is off (`plugins.legend.display === false`); the legend is `donut-legend` with one listitem per cause
//     (all nine, also those at 0), each with a `legend-swatch` whose inline background colour is the segment's colour and a
//     button "About <label>" whose hover/focus tooltip is exactly CAUSE_HELP[label];
//   - the hover text is `options.plugins.tooltip.callbacks.label(context)` (Chart.js tooltip item: `label`, `parsed`, `raw`,
//     `dataIndex`, `dataset.data`); CHOICE: the count has thousands separators ("1,234,567 packets"), the share two decimals
//     ("33.33 % of drops"), always "packets";
//   - all nine counts 0 (user 2026-11-05: the donut is always visible): the chart is still there, as ONE neutral grey full ring
//     (a single placeholder segment: one dataset, one positive value, a grey colour), the legend of nine causes is unchanged,
//     "No drops" stays as text, and hovering the ring says "No drops" (never "NaN", never a division by zero);
//   - CHOICE: the donut is shown under every radio choice (Rate, Delay histogram, Loss).

const ZERO: Partial<Counters> = {
  loss_drops: 0,
  limiter_drops: 0,
  buffer_full_drops: 0,
  tx_drops: 0,
  nic_rx_missed: 0,
  nic_rx_nombuf: 0,
  nic_rx_port_buffer_discards: 0,
  nic_rx_phy_discards: 0,
  nic_tx_phy_discards: 0,
  dpds_rejects: 0,
};

/** The live counter of each cause, in the order of CAUSES. */
const COUNTER_KEYS = [
  'loss_drops',
  'limiter_drops',
  'buffer_full_drops',
  'tx_drops',
  'nic_rx_missed',
  'nic_rx_nombuf',
  'nic_rx_port_buffer_discards',
  'nic_rx_phy_discards',
  'nic_tx_phy_discards',
] as const;
const zeros = (n = CAUSES.length) => Array<number>(n).fill(0);

/** 100 drops out of 10000 received packets: a cause's share of the drops is its count in percent. */
const HUNDRED: Partial<Counters> = {
  rx_pkts: 10000,
  loss_drops: 40,
  limiter_drops: 30,
  buffer_full_drops: 20,
  tx_drops: 5,
  nic_rx_missed: 4,
  nic_rx_nombuf: 1,
  nic_rx_port_buffer_discards: 0,
  nic_rx_phy_discards: 0,
  nic_tx_phy_discards: 0,
  dpds_rejects: 90,
};

/** Nine distinct counts, one per cause, summing to 100: a swapped, merged or dropped cause shows. */
const NINE: Partial<Counters> = {
  rx_pkts: 10000,
  loss_drops: 36,
  limiter_drops: 24,
  buffer_full_drops: 16,
  tx_drops: 8,
  nic_rx_missed: 6,
  nic_rx_nombuf: 4,
  nic_rx_port_buffer_discards: 3,
  nic_rx_phy_discards: 2,
  nic_tx_phy_discards: 1,
  dpds_rejects: 90,
};
const NINE_VALUES = [36, 24, 16, 8, 6, 4, 3, 2, 1];

const live = (dir: number, counters: Partial<Counters>, over: Partial<DirectionStats> = {}) =>
  fx.dirStats(dir, { counters: fx.counters(counters), ...over });

function serveLive(...dirs: DirectionStats[]) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats(dirs));
}

const donut = (d: number) => lossDonut(card(d));
const queryDonut = (d: number) => queryLossDonut(card(d));
const legend = (d: number) => within(card(d)).getByTestId('donut-legend');
interface DonutData {
  labels: string[];
  datasets: { data: number[]; backgroundColor: string[] }[];
}
const dataOf = (d: number): DonutData => JSON.parse(donut(d).getAttribute('data-chart') ?? 'null');
const valuesOf = (d: number) => dataOf(d).datasets[0].data;

async function openLive(...dirs: DirectionStats[]) {
  serveLive(...dirs);
  renderApp('/');
  await findCard(dirs.length - 1);
  card(0);
}

/** Calls the chart's tooltip label callback for segment `i` like Chart.js would, and returns the text. */
function hoverText(d: number, i: number): string {
  const props = propsOf(donut(d));
  const data = props.data as DonutData;
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

/** hue in degrees and saturation 0..1 of #rgb, #rrggbb or rgb()/rgba(). */
function hsv(css: string): { h: number; s: number; v: number } {
  const c = css.trim().toLowerCase().replace(/\s/g, '');
  let ch: number[];
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/.exec(c);
  if (hex) {
    const h = hex[1].length === 3 ? [...hex[1]].map((x) => x + x).join('') : hex[1];
    ch = [0, 2, 4].map((i) => Number.parseInt(h.slice(i, i + 2), 16));
  } else {
    const m = /^rgba?\(([^)]*)\)$/.exec(c);
    if (!m) throw new Error(`cannot parse colour ${css}`);
    ch = m[1].split(',').slice(0, 3).map(Number);
  }
  const [r, g, b] = ch.map((x) => x / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  let h = 0;
  if (d > 0) {
    if (max === r) h = 60 * (((g - b) / d + 6) % 6);
    else if (max === g) h = 60 * ((b - r) / d + 2);
    else h = 60 * ((r - g) / d + 4);
  }
  return { h, s: max === 0 ? 0 : d / max, v: max };
}
const rgbOf = (css: string): string => {
  const probe = document.createElement('span');
  probe.style.backgroundColor = css;
  return probe.style.backgroundColor;
};

// ---- the data --------------------------------------------------------------------------------------------------------------

describe('Loss donut (spec 35 F5): what it plots', () => {
  it('live: one dataset, nine segments in the order of the causes, labelled with the cause names, from the counters', async () => {
    await openLive(live(0, HUNDRED), live(1, ZERO));
    const data = dataOf(0);
    expect(data.labels).toEqual([...CAUSES]);
    expect(data.datasets).toHaveLength(1);
    expect(data.datasets[0].data).toEqual([40, 30, 20, 5, 4, 1, 0, 0, 0]);
    expect(data.datasets[0].backgroundColor).toHaveLength(9);
  });

  it('live: nine distinct counts land on nine segments, each cause on its own (spec 47 F1)', async () => {
    await openLive(live(0, NINE));
    expect(valuesOf(0)).toEqual(NINE_VALUES);
    expect(CAUSES).toHaveLength(9);
    expect(dataOf(0).labels).toEqual([...CAUSES]);
    expect(dataOf(0).datasets[0].backgroundColor).toHaveLength(9);
  });

  it.each(CAUSES.map((c, i) => [c, i] as const))(
    'live: "%s" is segment %i and nothing else moves with its counter',
    async (_cause, i) => {
      const key = COUNTER_KEYS[i];
      await openLive(live(0, { ...ZERO, [key]: 7 }));
      const want = zeros();
      want[i] = 7;
      expect(valuesOf(0)).toEqual(want);
    },
  );

  it('live: the counts are the cumulative counters, not the per-poll percentages', async () => {
    await openLive(
      live(0, { ...ZERO, loss_drops: 11, limiter_drops: 22 }, { loss_pct: 99, limiter_pct: 0, buffer_full_pct: 50 }),
    );
    expect(valuesOf(0)).toEqual([11, 22, 0, 0, 0, 0, 0, 0, 0]);
  });

  it('live: each link has its own donut, shown with its own tab', async () => {
    await openLive(live(0, HUNDRED), live(1, { ...ZERO, buffer_full_drops: 9, nic_rx_nombuf: 3 }));
    expect(valuesOf(0)).toEqual([40, 30, 20, 5, 4, 1, 0, 0, 0]);
    expect(valuesOf(1)).toEqual([0, 0, 9, 0, 0, 3, 0, 0, 0]);
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
  });

  it('live: it follows the next poll', async () => {
    vi.useFakeTimers();
    serveLive(live(0, { ...ZERO, loss_drops: 5 }));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(valuesOf(0)).toEqual([5, 0, 0, 0, 0, 0, 0, 0, 0]);
    api.getStats.mockResolvedValue(
      fx.stats([live(0, { ...ZERO, loss_drops: 5, nic_rx_missed: 8, nic_tx_phy_discards: 2 })]),
    );
    await tick(2500);
    expect(valuesOf(0)).toEqual([5, 0, 0, 0, 8, 0, 0, 0, 2]);
  });

  it.each(['stopped', 'stopping', 'failed'] as const)(
    '"%s": the donut is the history\'s totals of the link, with NIC TX from nic_tx_drops',
    async (state) => {
      vi.useFakeTimers();
      serveHistory({
        startedAt: 1762000000,
        samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) },
        totals: {
          '0': fx.totals(),
          '1': fx.totals({
            loss_drops: 1,
            limiter_drops: 0,
            buffer_full_drops: 0,
            nic_tx_drops: 2,
            nic_rx_missed: 0,
            nic_rx_nombuf: 6,
            nic_rx_port_buffer_discards: 7,
            nic_rx_phy_discards: 8,
            nic_tx_phy_discards: 9,
          }),
        },
      });
      api.getEmulation.mockResolvedValue(fx.emulation(state, { directions: [] }));
      api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
      renderApp('/');
      await tick(0);
      await tick(1000);
      expect(valuesOf(0)).toEqual([40, 30, 20, 5, 4, 1, 0, 0, 0]);
      expect(valuesOf(1)).toEqual([1, 0, 0, 2, 0, 6, 7, 8, 9]);
      expect(dataOf(0).labels).toEqual([...CAUSES]);
    },
  );

  it('after a stop it follows the totals the controller serves (the newest answer wins)', async () => {
    vi.useFakeTimers();
    const s = {
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(0, 3) },
      totals: { '0': fx.totals() },
    };
    serveHistory(s);
    api.getEmulation.mockResolvedValue(fx.emulation('stopping', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    renderApp('/');
    await tick(0);
    await tick(1000);
    expect(valuesOf(0)).toEqual([40, 30, 20, 5, 4, 1, 0, 0, 0]);
    s.totals = { '0': fx.totals({ loss_drops: 41, nic_rx_nombuf: 9, nic_rx_phy_discards: 5 }) };
    await tick(2500);
    expect(valuesOf(0)).toEqual([41, 30, 20, 5, 4, 9, 0, 5, 0]);
  });

  it('the donut changes with the run: live counters while running, the totals once it has ended', async () => {
    vi.useFakeTimers();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3) }, totals: { '0': fx.totals() } });
    serveLive(live(0, { ...ZERO, loss_drops: 3 }));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(valuesOf(0)).toEqual([3, 0, 0, 0, 0, 0, 0, 0, 0]);
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    await tick(2500);
    expect(valuesOf(0)).toEqual([40, 30, 20, 5, 4, 1, 0, 0, 0]);
  });

  it.each(['Traffic rates', 'Delay histogram', 'Packet loss'])(
    'the donut is under the chart whichever radio is chosen (%s)',
    async (choice) => {
      await openLive(live(0, HUNDRED));
      fireEvent.click(radio(0, choice));
      expect(valuesOf(0)).toEqual([40, 30, 20, 5, 4, 1, 0, 0, 0]);
      const group = within(card(0)).getByRole('radiogroup', { name: 'Chart' });
      expect(group.compareDocumentPosition(donut(0)) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    },
  );
});

// ---- no drops ------------------------------------------------------------------------------------------------------------------

/** The totals of an ended run with no drop at all. */
const NO_TOTALS = {
  loss_drops: 0,
  limiter_drops: 0,
  buffer_full_drops: 0,
  nic_tx_drops: 0,
  nic_rx_missed: 0,
  nic_rx_nombuf: 0,
  nic_rx_port_buffer_discards: 0,
  nic_rx_phy_discards: 0,
  nic_tx_phy_discards: 0,
};

async function openEnded(totals: Partial<typeof NO_TOTALS>) {
  vi.useFakeTimers();
  serveHistory({
    startedAt: 1762000000,
    samples: { '0': fx.historySamples(0, 3) },
    totals: { '0': fx.totals(totals) },
  });
  api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
  api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
  renderApp('/');
  await tick(0);
  await tick(1000);
}

/** The placeholder ring: one positive value in a grey that is not one of the cause colours. */
function expectPlaceholderRing(d: number) {
  const data = dataOf(d);
  expect(data.datasets).toHaveLength(1);
  expect(data.datasets[0].data).toHaveLength(1);
  expect(data.datasets[0].data[0]).toBeGreaterThan(0);
  expect(data.datasets[0].backgroundColor).toHaveLength(1);
  expect(hsv(data.datasets[0].backgroundColor[0]).s).toBeLessThan(0.25);
}

describe('Loss donut (spec 35 F5): no drops, the donut is still there', () => {
  it('live, all nine counts 0: the donut is one grey full ring, and "No drops" is shown', async () => {
    await openLive(live(0, ZERO));
    expect(queryDonut(0)).toBeInTheDocument();
    expectPlaceholderRing(0);
    expect(within(card(0)).getByText('No drops')).toBeInTheDocument();
  });

  it('live, all nine counts 0: the legend still lists the nine causes with their "About" buttons', async () => {
    await openLive(live(0, ZERO));
    const items = within(legend(0)).getAllByRole('listitem');
    expect(items).toHaveLength(9);
    CAUSES.forEach((cause, i) => {
      expect(items[i], cause).toHaveTextContent(cause);
    });
    expect(within(legend(0)).getAllByRole('button')).toHaveLength(9);
  });

  it('the legend swatches keep the cause colours, not the grey of the placeholder', async () => {
    await openLive(live(0, ZERO));
    const swatches = within(legend(0)).getAllByTestId('legend-swatch');
    expect(new Set(swatches.map((s) => rgbOf(s.style.backgroundColor))).size).toBe(9);
  });

  it('the Chart.js legend is off and the tooltips are on, also for the placeholder', async () => {
    await openLive(live(0, ZERO));
    const options = JSON.parse(donut(0).getAttribute('data-options') ?? 'null');
    expect(options.plugins.legend.display).toBe(false);
    expect(options.plugins.tooltip?.enabled).not.toBe(false);
  });

  it('hovering the ring says "No drops": no NaN, no percentage, no count', async () => {
    await openLive(live(0, ZERO));
    const text = hoverText(0, 0);
    expect(text).toContain('No drops');
    expect(text).not.toMatch(/NaN|Infinity|%|packets/);
  });

  it('ended with totals of 0: the donut is there as well, grey, with "No drops" and the nine-cause legend', async () => {
    await openEnded(NO_TOTALS);
    expect(queryDonut(0)).toBeInTheDocument();
    expectPlaceholderRing(0);
    expect(within(card(0)).getByText('No drops')).toBeInTheDocument();
    expect(within(legend(0)).getAllByRole('listitem')).toHaveLength(9);
    expect(hoverText(0, 0)).not.toMatch(/NaN|Infinity|%/);
  });

  it('with drops there is no placeholder and no "No drops"; even a single packet gives the real segments', async () => {
    await openLive(live(0, { ...ZERO, nic_rx_nombuf: 1 }));
    expect(valuesOf(0)).toEqual([0, 0, 0, 0, 0, 1, 0, 0, 0]);
    expect(dataOf(0).labels).toEqual([...CAUSES]);
    expect(within(card(0)).queryByText('No drops')).not.toBeInTheDocument();
  });

  it('it is per link: one link with drops, the other without; both have a donut', async () => {
    await openLive(live(0, HUNDRED), live(1, ZERO));
    expect(valuesOf(0)).toEqual([40, 30, 20, 5, 4, 1, 0, 0, 0]);
    expect(within(card(0)).queryByText('No drops')).not.toBeInTheDocument();
    expectPlaceholderRing(1);
    expect(within(card(1)).getByText('No drops')).toBeInTheDocument();
  });

  it('it switches with the polls: placeholder, then the real segments, then the placeholder again', async () => {
    vi.useFakeTimers();
    serveLive(live(0, ZERO));
    renderApp('/');
    await tick(0);
    await tick(0);
    expectPlaceholderRing(0);
    expect(within(card(0)).getByText('No drops')).toBeInTheDocument();
    api.getStats.mockResolvedValue(fx.stats([live(0, { ...ZERO, loss_drops: 2 })]));
    await tick(2500);
    expect(valuesOf(0)).toEqual([2, 0, 0, 0, 0, 0, 0, 0, 0]);
    expect(within(card(0)).queryByText('No drops')).not.toBeInTheDocument();
    api.getStats.mockResolvedValue(fx.stats([live(0, ZERO)]));
    await tick(2500);
    expectPlaceholderRing(0);
    expect(within(card(0)).getByText('No drops')).toBeInTheDocument();
  });

  it('CHOICE: the ring and "No drops" are shown below every chart choice', async () => {
    await openLive(live(0, ZERO));
    for (const choice of ['Packet loss', 'Delay histogram', 'Traffic rates']) {
      fireEvent.click(radio(0, choice));
      expect(within(card(0)).getByText('No drops'), choice).toBeInTheDocument();
      expect(queryDonut(0), choice).toBeInTheDocument();
    }
  });
});

// ---- the legend ------------------------------------------------------------------------------------------------------------------

describe('Loss donut (spec 35 F5): the React legend', () => {
  it('Chart.js draws no legend of its own', async () => {
    await openLive(live(0, HUNDRED));
    const options = JSON.parse(donut(0).getAttribute('data-options') ?? 'null');
    expect(options.plugins.legend.display).toBe(false);
  });

  it('one item per cause with the exact labels, in the order of the segments, also for causes at 0', async () => {
    await openLive(live(0, { ...ZERO, buffer_full_drops: 4 }));
    const items = within(legend(0)).getAllByRole('listitem');
    expect(items).toHaveLength(9);
    CAUSES.forEach((cause, i) => {
      expect(items[i], cause).toHaveTextContent(cause);
      expect(within(items[i]).getByText(cause)).toBeInTheDocument();
    });
  });

  it('each item has a colour swatch in the colour of its segment', async () => {
    await openLive(live(0, HUNDRED));
    const colours = dataOf(0).datasets[0].backgroundColor;
    const items = within(legend(0)).getAllByRole('listitem');
    items.forEach((item, i) => {
      const swatch = within(item).getByTestId('legend-swatch');
      expect(rgbOf(swatch.style.backgroundColor), CAUSES[i]).toBe(rgbOf(colours[i]));
    });
  });

  it('each item has a button "About <label>"', async () => {
    await openLive(live(0, HUNDRED));
    const items = within(legend(0)).getAllByRole('listitem');
    items.forEach((item, i) => {
      const button = within(item).getByRole('button', { name: `About ${CAUSES[i]}` });
      expect(button).toHaveAttribute('type', 'button');
    });
    expect(within(legend(0)).getAllByRole('button')).toHaveLength(9);
  });

  it.each(CAUSES)('hovering "About %s" shows exactly its explanation', async (cause) => {
    const user = userEvent.setup();
    await openLive(live(0, HUNDRED));
    const button = within(legend(0)).getByRole('button', { name: `About ${cause}` });
    await user.hover(button);
    const tip = await screen.findByRole('tooltip');
    expect(tip.textContent?.trim()).toBe(CAUSE_HELP[cause]);
    await user.unhover(button);
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });

  it('keyboard focus shows the explanation too', async () => {
    await openLive(live(0, HUNDRED));
    const button = within(legend(0)).getByRole('button', { name: `About ${CAUSES[4]}` });
    fireEvent.focus(button);
    const tip = await screen.findByRole('tooltip');
    expect(tip.textContent?.trim()).toBe(CAUSE_HELP[CAUSES[4]]);
  });

  it('the legend of an ended run is the same', async () => {
    vi.useFakeTimers();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3) }, totals: { '0': fx.totals() } });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    renderApp('/');
    await tick(0);
    await tick(1000);
    expect(within(legend(0)).getAllByRole('listitem')).toHaveLength(9);
    expect(within(legend(0)).getAllByRole('button')).toHaveLength(9);
  });
});

// ---- the hover text ----------------------------------------------------------------------------------------------------------------

describe('Loss donut (spec 35 F5): the hover text', () => {
  it('reads "<label>: <count> packets (<share> % of drops)" with the share of all drops, two decimals', async () => {
    await openLive(live(0, HUNDRED));
    expect(hoverText(0, 0)).toBe('Loss model: 40 packets (40.00 % of drops)');
    expect(hoverText(0, 1)).toBe('Rate limiter: 30 packets (30.00 % of drops)');
    expect(hoverText(0, 2)).toBe('Buffer full: 20 packets (20.00 % of drops)');
    expect(hoverText(0, 3)).toBe('NIC TX ring full: 5 packets (5.00 % of drops)');
    expect(hoverText(0, 4)).toBe('NIC RX missed: 4 packets (4.00 % of drops)');
    expect(hoverText(0, 5)).toBe('RX buffers exhausted: 1 packets (1.00 % of drops)');
  });

  it('spec 47: the hover text of the three new causes, with their share of all nine', async () => {
    await openLive(live(0, NINE));
    expect(hoverText(0, 6)).toBe('NIC RX port buffer full: 3 packets (3.00 % of drops)');
    expect(hoverText(0, 7)).toBe('NIC RX port discards: 2 packets (2.00 % of drops)');
    expect(hoverText(0, 8)).toBe('NIC TX port discards: 1 packets (1.00 % of drops)');
  });

  it('the share is of the drops, not of the received packets', async () => {
    // 3 of 4 drops are the loss model; 4 drops of 1,000,000 received would make it 0.0003 %
    await openLive(live(0, { ...ZERO, rx_pkts: 1_000_000, loss_drops: 3, buffer_full_drops: 1 }));
    expect(hoverText(0, 0)).toBe('Loss model: 3 packets (75.00 % of drops)');
    expect(hoverText(0, 2)).toBe('Buffer full: 1 packets (25.00 % of drops)');
  });

  it('thirds round to two decimals: 33.33 and 66.67', async () => {
    await openLive(live(0, { ...ZERO, loss_drops: 1, limiter_drops: 2 }));
    expect(hoverText(0, 0)).toBe('Loss model: 1 packets (33.33 % of drops)');
    expect(hoverText(0, 1)).toBe('Rate limiter: 2 packets (66.67 % of drops)');
  });

  it('CHOICE: a large count has thousands separators', async () => {
    await openLive(live(0, { ...ZERO, loss_drops: 1_234_567, limiter_drops: 1_234_567 }));
    expect(hoverText(0, 0)).toBe('Loss model: 1,234,567 packets (50.00 % of drops)');
  });

  it('a segment at 0 reads "0 packets (0.00 % of drops)"', async () => {
    await openLive(live(0, { ...ZERO, loss_drops: 5 }));
    expect(hoverText(0, 3)).toBe('NIC TX ring full: 0 packets (0.00 % of drops)');
  });

  it('the same text for an ended run, from the totals', async () => {
    vi.useFakeTimers();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3) }, totals: { '0': fx.totals() } });
    api.getEmulation.mockResolvedValue(fx.emulation('failed', { directions: [], error: 'x' }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    renderApp('/');
    await tick(0);
    await tick(1000);
    expect(hoverText(0, 3)).toBe('NIC TX ring full: 5 packets (5.00 % of drops)');
  });

  it('Chart.js tooltips are not switched off for the donut', async () => {
    await openLive(live(0, HUNDRED));
    const options = JSON.parse(donut(0).getAttribute('data-options') ?? 'null');
    expect(options.plugins.tooltip?.enabled).not.toBe(false);
  });
});

// ---- colours -----------------------------------------------------------------------------------------------------------------------

describe('Loss donut (spec 35 F5): neutral for the loss model, warm for the overload causes', () => {
  it('the loss model is grey or blue', async () => {
    await openLive(live(0, HUNDRED));
    const { h, s } = hsv(dataOf(0).datasets[0].backgroundColor[0]);
    expect(s < 0.25 || (h >= 180 && h <= 270), `hue ${h}, saturation ${s}`).toBe(true);
  });

  it.each([1, 2, 3, 4, 5, 6, 7, 8])('segment %i (an overload cause) is amber, orange or red', async (i) => {
    await openLive(live(0, HUNDRED));
    const { h, s } = hsv(dataOf(0).datasets[0].backgroundColor[i]);
    expect(h <= 50 || h >= 340, `${CAUSES[i]}: hue ${h}`).toBe(true);
    expect(s, `${CAUSES[i]}: saturation`).toBeGreaterThanOrEqual(0.5);
  });

  it('the nine colours are all different', async () => {
    await openLive(live(0, HUNDRED));
    const colours = dataOf(0).datasets[0].backgroundColor.map(rgbOf);
    expect(new Set(colours).size).toBe(9);
  });
});

// ---- spec 50 B: the labels carry no counter names, the hover help does ----------------------------------------------------------------
//
// User 2026-11-05: "the labels should not contain in brackets the counter names. They are only shown when hovering above the donut.
// Also the donut label should not include them." All nine lines stay in the Packet loss chart, zero lines too. The expectations
// below are literal (not built from CAUSES) so that a stale helper cannot hide a stale label.

const TRIMMED = [
  'Loss model',
  'Rate limiter',
  'Buffer full',
  'NIC TX ring full',
  'NIC RX missed',
  'RX buffers exhausted',
  'NIC RX port buffer full',
  'NIC RX port discards',
  'NIC TX port discards',
];

/** The counter names each cause's hover help must mention (none for the first three). */
const COUNTERS_IN_HELP: [string, string[]][] = [
  ['NIC TX ring full', ['tx_drops']],
  ['NIC RX missed', ['rx_missed_errors']],
  ['RX buffers exhausted', ['rx_mbuf_allocation_errors']],
  ['NIC RX port buffer full', ['rx_prio*_buf_discard_packets', 'rx_prio*_cong_discard_packets']],
  ['NIC RX port discards', ['rx_phy_discard_packets']],
  ['NIC TX port discards', ['tx_phy_discard_packets']],
];

describe('Loss donut (spec 50 B1): trimmed labels', () => {
  it('the donut data labels are the nine trimmed names', async () => {
    await openLive(live(0, NINE));
    expect(dataOf(0).labels).toEqual(TRIMMED);
  });

  it('the legend lists the trimmed names, with no bracket and no counter name', async () => {
    await openLive(live(0, NINE));
    const items = within(legend(0)).getAllByRole('listitem');
    expect(items.map((i) => (i.textContent ?? '').trim())).toEqual(TRIMMED);
    for (const i of items) expect(i.textContent).not.toMatch(/[()_*]/);
  });

  it('the "About" buttons are named after the trimmed labels', async () => {
    await openLive(live(0, NINE));
    for (const name of TRIMMED) {
      expect(within(legend(0)).getByRole('button', { name: `About ${name}` })).toBeInTheDocument();
    }
  });

  it('the hover text of the donut has the trimmed label, never a counter name, for each of the nine causes', async () => {
    await openLive(live(0, NINE));
    TRIMMED.forEach((name, i) => {
      const text = hoverText(0, i);
      expect(text.startsWith(`${name}: ${NINE_VALUES[i]} packets (`), text).toBe(true);
      expect(text).not.toMatch(/_/);
    });
  });

  it('an ended run has the same trimmed labels in the donut and the legend', async () => {
    vi.useFakeTimers();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3) }, totals: { '0': fx.totals() } });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    renderApp('/');
    await tick(0);
    await tick(1000);
    expect(dataOf(0).labels).toEqual(TRIMMED);
    const items = within(legend(0)).getAllByRole('listitem');
    expect(items.map((i) => (i.textContent ?? '').trim())).toEqual(TRIMMED);
  });

  it('the Packet loss chart has nine datasets with the trimmed labels, the ones at 0 too', async () => {
    await openLive(live(0, { ...ZERO, loss_drops: 3 }));
    fireEvent.click(radio(0, 'Packet loss'));
    const chart = JSON.parse(within(card(0)).getByTestId('chart-line').getAttribute('data-chart') ?? 'null') as {
      datasets: { label: string }[];
    };
    expect(chart.datasets.map((d) => d.label)).toEqual(TRIMMED);
  });
});

describe('Loss donut (spec 50 B2): the counter names are in the legend hover help', () => {
  it.each(COUNTERS_IN_HELP)('"About %s" names its counter(s) in brackets', async (cause, counters) => {
    const user = userEvent.setup();
    await openLive(live(0, HUNDRED));
    const button = within(legend(0)).getByRole('button', { name: `About ${cause}` });
    await user.hover(button);
    const tip = await screen.findByRole('tooltip');
    const text = tip.textContent ?? '';
    for (const c of counters) expect(text, c).toContain(c);
    expect(text).toContain(`(${counters.join(', ')})`);
    await user.unhover(button);
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });

  it.each([
    ['Loss model', 'Packets dropped on purpose by the configured loss model (random or Gilbert-Elliott).'],
    ['Rate limiter', "Packets the limiter refused: the Spacer's backlog or the token bucket's budget was exceeded."],
    ['Buffer full', 'Packets that arrived while the delay buffer already held its maximum number of packets.'],
  ])('"About %s" keeps its text: it has no counter', async (cause, help) => {
    const user = userEvent.setup();
    await openLive(live(0, HUNDRED));
    const button = within(legend(0)).getByRole('button', { name: `About ${cause}` });
    await user.hover(button);
    const tip = await screen.findByRole('tooltip');
    expect(tip.textContent?.trim()).toBe(help);
    await user.unhover(button);
  });
});
