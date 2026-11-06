import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { Counters, DirectionStats, HistorySample } from '../api/types';
import { formatFrameCount } from '../common/format';
import { propsOf } from '../test/chartProps';
import {
  bodyCells,
  CAUSES,
  card,
  findCard,
  lossDonut,
  lossTable,
  pillText,
  summary,
  summaryRadio,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// A port that is an SR-IOV virtual function has no physical-port counters: the controller sends JSON null for
// nic_rx_port_buffer_discards, nic_rx_phy_discards, nic_tx_phy_discards (and the diagnostics crc/symbol/length, which are shown
// nowhere) in the live counters, in the history samples and in the totals. The dashboard then
//   - shows "n/a" instead of a number, with the title or tooltip NA_TIP, in the legend row of the cause in the loss donut;
//   - never plots null as 0: the Packet loss chart's dataset holds JSON null at that sample;
//   - adds nothing for a null count in any total (Status pill, Lost Frames, donut shares); in the Summary a cause is the sum of
//     the links that have a number, and null only when every link has null.
// The legend row is the DOM place of a cause (the counts themselves are in the chart); `n/a` is its own element, the text of
// the hover on the donut's segment reads "<label>: n/a".

const NA = 'n/a';
const NA_TIP = 'Not measurable on an SR-IOV virtual function; see the physical function on the hypervisor';
const PORT_BUFFER = 6; // positions in CAUSES
const RX_PHY = 7;
const TX_PHY = 8;

const VF: Partial<Counters> = {
  nic_rx_port_buffer_discards: null,
  nic_rx_phy_discards: null,
  nic_tx_phy_discards: null,
  nic_rx_crc_errors: null,
  nic_rx_symbol_errors: null,
  nic_rx_length_errors: null,
};

const live = (dir: number, counters: Partial<Counters>): DirectionStats =>
  fx.dirStats(dir, { counters: fx.counters({ loss_drops: 4, limiter_drops: 0, buffer_full_drops: 0, ...counters }) });

interface Donut {
  datasets: { data: (number | null)[] }[];
}
const valuesOf = (el: HTMLElement) => (JSON.parse(el.getAttribute('data-chart') ?? 'null') as Donut).datasets[0].data;
const legendRow = (scope: HTMLElement, cause: number) =>
  must(within(within(scope).getByTestId('donut-legend')).getAllByRole('listitem')[cause], `legend row ${cause}`);

/** The row shows "n/a" and its explanation as a title or as a tooltip on hover. */
async function expectNa(row: HTMLElement) {
  const na = within(row).getByText(NA);
  const titled = [
    na,
    ...Array.from(na.querySelectorAll('[title]')),
    ...(na.closest('[title]') ? [na.closest('[title]')] : []),
  ];
  if (titled.some((e) => e?.getAttribute('title') === NA_TIP)) return;
  await userEvent.setup().hover(na);
  expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe(NA_TIP);
}
function expectNoNa(row: HTMLElement) {
  expect(within(row).queryByText(NA)).not.toBeInTheDocument();
}

function serveLive(...dirs: DirectionStats[]) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats(dirs));
}

describe('SR-IOV virtual function: the loss donut', () => {
  it('a null cause reads "n/a" with the tooltip; the others show no "n/a" and their shares ignore the null ones', async () => {
    serveLive(live(0, { ...VF, nic_rx_missed: 6, nic_rx_nombuf: 2 }), live(1, {}));
    renderApp('/');
    await findCard(1);
    const scope = card(0);
    const values = valuesOf(lossDonut(scope));
    for (const i of [PORT_BUFFER, RX_PHY, TX_PHY]) {
      expect(values[i] === null || values[i] === 0, `${CAUSES[i]} is not plotted as a number`).toBe(true);
      await expectNa(legendRow(scope, i));
    }
    expect(values.slice(0, 6)).toEqual([4, 0, 0, 0, 6, 2]);
    for (let i = 0; i < PORT_BUFFER; i++) expectNoNa(legendRow(scope, i));
    // hover: "n/a" for a null segment, a share of the 12 known drops for the others
    const props = propsOf(lossDonut(scope));
    const label = must(
      (props.options as { plugins?: { tooltip?: { callbacks?: { label?: (c: unknown) => string } } } }).plugins?.tooltip
        ?.callbacks?.label,
      'tooltip label callback',
    );
    const data = (props.data as { datasets: { data: unknown[] }[] }).datasets[0];
    const hover = (i: number) =>
      label({
        label: CAUSES[i],
        parsed: data.data[i],
        raw: data.data[i],
        dataIndex: i,
        dataset: data,
        datasetIndex: 0,
      });
    expect(hover(PORT_BUFFER)).toContain(NA);
    expect(hover(PORT_BUFFER)).not.toMatch(/NaN|packets/);
    expect(hover(0)).toContain('33.33 % of drops');
    // the other link has numbers: no "n/a" at all
    const other = card(1);
    for (let i = 0; i < CAUSES.length; i++) expectNoNa(legendRow(other, i));
  });

  it('an ended run reads the history totals: null shows "n/a" there as well', async () => {
    vi.useFakeTimers();
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(0, 3) },
      totals: { '0': fx.totals({ ...VF, nic_rx_missed: 4 }) },
    });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
    renderApp('/');
    await tick(0);
    await tick(1000);
    for (const i of [PORT_BUFFER, RX_PHY, TX_PHY]) await expectNa(legendRow(card(0), i));
    expect(valuesOf(lossDonut(card(0))).slice(0, 6)).toEqual([40, 30, 20, 5, 4, 1]);
  });

  it('the Summary adds the links that have a number; it is "n/a" only when every link is null', async () => {
    serveLive(
      live(0, { ...VF, nic_rx_port_buffer_discards: null, nic_tx_phy_discards: 3 }),
      live(1, { nic_rx_port_buffer_discards: 5, nic_rx_phy_discards: null, nic_tx_phy_discards: 4 }),
    );
    renderApp('/');
    await findCard(1);
    const sum = summary();
    const values = valuesOf(lossDonut(sum));
    expect(values[PORT_BUFFER]).toBe(5);
    expect(values[TX_PHY]).toBe(7);
    expectNoNa(legendRow(sum, PORT_BUFFER));
    expectNoNa(legendRow(sum, TX_PHY));
    await expectNa(legendRow(sum, RX_PHY)); // null on both links
  });
});

describe('SR-IOV virtual function: totals', () => {
  it('null counts add nothing to the Status pill and to the Lost Frames cell', async () => {
    serveLive(live(0, { ...VF, nic_rx_missed: 2 }), live(1, { nic_rx_phy_discards: 3 }));
    renderApp('/');
    await findCard(1);
    // 4 + 2 on link 0 (null ones count 0), 4 + 3 on link 1
    await waitFor(() => expect(pillText('pill-status')).toBe('Status: Error. Lost 13.00 frames.'));
    expect(pillText('pill-status')).not.toMatch(/NaN/);
    expect(bodyCells(lossTable(card(0)))[0].trim()).toBe(formatFrameCount(6).trim());
    expect(bodyCells(lossTable(summary()))[0].trim()).toBe(formatFrameCount(13).trim());
  });
});

describe('SR-IOV virtual function: the Packet loss chart', () => {
  const sample = (t: number, over: Partial<HistorySample>) => fx.historySample(t, over);
  const nulls: Partial<HistorySample> = {
    nic_rx_port_buffer_discards: null,
    nic_rx_phy_discards: null,
    nic_tx_phy_discards: null,
  };
  const lineOf = (scope: HTMLElement) =>
    JSON.parse(within(scope).getByTestId('chart-line').getAttribute('data-chart') ?? 'null') as {
      datasets: { label: string; data: (number | null)[] }[];
    };

  async function open() {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    serveHistory({
      startedAt: 1762000000,
      samples: {
        '0': [sample(1000, { ...nulls, nic_rx_missed: 1 }), sample(2000, { ...nulls, nic_rx_missed: 2 })],
        '1': [
          sample(1000, { nic_rx_port_buffer_discards: 5, nic_rx_phy_discards: null, nic_tx_phy_discards: 1 }),
          sample(2000, { nic_rx_port_buffer_discards: 6, nic_rx_phy_discards: null, nic_tx_phy_discards: 2 }),
        ],
      },
    });
    renderApp('/');
    await tick(0);
    await tick(0);
  }

  it('plots null as a gap (JSON null), not as 0, and keeps the valid counters', async () => {
    await open();
    fireEvent.click(within(card(0)).getByRole('radio', { name: 'Packet loss' }));
    await tick(0);
    const c = lineOf(card(0));
    for (const i of [PORT_BUFFER, RX_PHY, TX_PHY]) expect(c.datasets[i].data, CAUSES[i]).toEqual([null, null]);
    expect(c.datasets[4].data).toEqual([1, 2]); // nic_rx_missed stays a number
  });

  it('the Summary line is the sum of the links with a number, null where all are null', async () => {
    await open();
    fireEvent.click(summaryRadio('Packet loss'));
    await tick(0);
    const c = lineOf(summary());
    expect(c.datasets[PORT_BUFFER].data).toEqual([5, 6]);
    expect(c.datasets[RX_PHY].data).toEqual([null, null]);
    expect(c.datasets[TX_PHY].data).toEqual([1, 2]);
  });
});

describe('SR-IOV virtual function: types', () => {
  const types = readFileSync(resolve(__dirname, '../api/types.ts'), 'utf8');
  const members = (name: string) => {
    const start = types.indexOf(`export interface ${name} {`);
    expect(start, `interface ${name}`).toBeGreaterThanOrEqual(0);
    return types.slice(start, types.indexOf('\n}', start)).replace(/\/\*[\s\S]*?\*\//g, '');
  };

  it.each([
    [
      'Counters',
      [
        'nic_rx_port_buffer_discards',
        'nic_rx_phy_discards',
        'nic_tx_phy_discards',
        'nic_rx_crc_errors',
        'nic_rx_symbol_errors',
        'nic_rx_length_errors',
      ],
    ],
    ['HistorySample', ['nic_rx_port_buffer_discards', 'nic_rx_phy_discards', 'nic_tx_phy_discards']],
    ['HistoryTotals', ['nic_rx_port_buffer_discards', 'nic_rx_phy_discards', 'nic_tx_phy_discards']],
  ])('%s: the physical-port counters are number | null, the valid ones plain numbers', (name, nullable) => {
    const body = members(name);
    for (const k of nullable) expect(body, k).toMatch(new RegExp(`\\b${k}:\\s*number \\| null\\s*;`));
    for (const k of ['nic_rx_missed', 'nic_rx_nombuf']) expect(body, k).toMatch(new RegExp(`\\b${k}:\\s*number\\s*;`));
  });
});
