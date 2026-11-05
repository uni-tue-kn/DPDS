import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { Counters, DirectionStats } from '../api/types';
import {
  bodyCells,
  card,
  DELAY_BAR_LABEL,
  findCard,
  findTabs,
  findDelayBar,
  LOSS_DONUT_LABEL,
  lossTable,
  radio,
  rateTable,
  summary,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedLinks, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 51 T2 (user 2026-11-05: "a table below the rate diagram and above the donuts ... TX L1, RX L1, TX L2, RX L2 / Lost Frames,
// Frame Loss Ratio, Out of Order. Same setting/UI/table as P4TG"). Source of truth: P4TG StatView.tsx ~550-680. Contract:
//   rate col   (spec 53 B: both tables sit in ONE `.row.mb-3`, rates left, loss right, each in a `.col-12.col-md-6`; they stack below md)
//              `.col-12.col-md-6` > `table.table.table-striped.table-bordered.table-hover.table-sm.mt-3.mb-3`,
//              `thead.table-dark`, `th.col-3` x4: TX L1, RX L1, TX L2, RX L2; one body row of formatBits(...).
//              L2 = the link's tx_bps / rx_bps; L1 = L2 + 20 B x 8 x pps of the same side (tx_pps for TX, rx_pps for RX).
//   loss col   `.col-12.col-md-6` > the same table style, `thead.table-dark`: Lost Frames | Frame Loss Ratio |
//              Out of Order. Lost = formatFrameCount(lost) with a tooltip of the exact count, Frame Loss Ratio =
//              lost > 0 ? (lost * 100 / (lost + forwarded)).toFixed(2) + " %" : "0.00 %", Out of Order = formatFrameCount(out_of_order)
//              with a tooltip of the exact count. lost = the nine causes of the link (the Status's), forwarded = counters.tx_sent.
//   Summary    the same two tables with the links' sums (rates, pps, lost, forwarded, out of order).
//   after the stop the rates read 0 and the loss table keeps the run's totals (the history's `totals`).
// The forwarded frames of an ended run are `tx_sent` of the history's totals (fx.totals(): 9900).
// Before any run the two tables are not shown at all (pinned at the end).

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

/** The live counter of each cause, in the order of the nine causes. */
const CAUSE_KEYS = [
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

/** Nine distinct counts summing to 100. */
const NINE: Partial<Counters> = {
  loss_drops: 36,
  limiter_drops: 24,
  buffer_full_drops: 16,
  tx_drops: 8,
  nic_rx_missed: 6,
  nic_rx_nombuf: 4,
  nic_rx_port_buffer_discards: 3,
  nic_rx_phy_discards: 2,
  nic_tx_phy_discards: 1,
};

/** A direction with all drops 0 unless `over` says otherwise; `tx_pkts` differs from `tx_sent` so the wrong one shows. */
const dir = (n: number, over: Partial<Counters> = {}, rest: Partial<DirectionStats> = {}) =>
  fx.dirStats(n, { counters: fx.counters({ ...ZERO, rx_pkts: 100000, tx_pkts: 800, tx_sent: 900, ...over }), ...rest });

// Link 0: TX 4 Gbit/s at 500 kpps, RX 3.5 Gbit/s at 400 kpps. Link 1: TX 1 Gbit/s at 100 kpps, RX 2 Gbit/s at 200 kpps.
const RATES0 = { tx_bps: 4e9, tx_pps: 5e5, rx_bps: 3.5e9, rx_pps: 4e5 };
const RATES1 = { tx_bps: 1e9, tx_pps: 1e5, rx_bps: 2e9, rx_pps: 2e5 };

function serveLive(...dirs: DirectionStats[]) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats(dirs));
}
async function openLive(...dirs: DirectionStats[]) {
  serveLive(...dirs);
  renderApp('/');
  await findCard(dirs.length - 1);
}

const classes = (e: Element) => Array.from(e.classList).sort();
const TABLE_CLASSES = ['mb-3', 'mt-3', 'table', 'table-bordered', 'table-hover', 'table-sm', 'table-striped'];

// ---- the markup ---------------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 51 T2): the markup of the two tables, as P4TG has it', () => {
  it('rate table: in a `.col-12.col-md-6` of the one `.row.mb-3`, a striped bordered hover small table with mt-3 mb-3', async () => {
    await openLive(dir(0, {}, RATES0), dir(1, {}, RATES1));
    const t = rateTable(card(0));
    expect(classes(t)).toEqual(TABLE_CLASSES);
    const col = must(t.parentElement, 'the column');
    expect(col).toHaveClass('col-12', 'col-md-6');
    expect(col).not.toHaveClass('col-md-4');
    const row = must(col.parentElement, 'the row');
    expect(classes(row)).toEqual(['mb-3', 'row']);
  });

  it('rate table: `thead.table-dark` with the headers TX L1, RX L1, TX L2, RX L2, each `th.col-3`', async () => {
    await openLive(dir(0, {}, RATES0), dir(1, {}, RATES1));
    const t = rateTable(card(0));
    const head = must(t.querySelector('thead'), 'thead');
    expect(classes(head)).toEqual(['table-dark']);
    const ths = Array.from(head.querySelectorAll('th'));
    expect(ths.map((th) => th.textContent?.trim())).toEqual(['TX L1', 'RX L1', 'TX L2', 'RX L2']);
    for (const th of ths) expect(classes(th)).toEqual(['col-3']);
    expect(t.querySelectorAll('tbody > tr')).toHaveLength(1);
    expect(t.querySelectorAll('tbody > tr > td')).toHaveLength(4);
  });

  it('loss table: in a `.col-12.col-md-6` of the same `.row.mb-3`, the same table style, `thead.table-dark`, three headers', async () => {
    await openLive(dir(0, NINE), dir(1));
    const t = lossTable(card(0));
    expect(classes(t)).toEqual(TABLE_CLASSES);
    const col = must(t.parentElement, 'the column');
    expect(col).toHaveClass('col-12', 'col-md-6');
    expect(col).not.toHaveClass('col-md-4');
    expect(classes(must(col.parentElement, 'the row'))).toEqual(['mb-3', 'row']);
    const head = must(t.querySelector('thead'), 'thead');
    expect(classes(head)).toEqual(['table-dark']);
    expect(Array.from(head.querySelectorAll('th')).map((th) => th.textContent?.trim())).toEqual([
      'Lost Frames',
      'Frame Loss Ratio',
      'Out of Order',
    ]);
    expect(t.querySelectorAll('tbody > tr')).toHaveLength(1);
    expect(t.querySelectorAll('tbody > tr > td')).toHaveLength(3);
  });

  it('one row holds both tables, rates left and loss right (two columns, in this order); the row is after the chart and its radios and before the donuts', async () => {
    await openLive(dir(0, NINE, RATES0), dir(1));
    const panel = card(0);
    const rateCol = must(rateTable(panel).parentElement, 'the rate column');
    const lossCol = must(lossTable(panel).parentElement, 'the loss column');
    const row = must(rateCol.closest('.row'), 'the row');
    expect(must(lossCol.closest('.row'))).toBe(row);
    expect(Array.from(row.children)).toEqual([rateCol, lossCol]);
    expect(row.querySelectorAll('table')).toHaveLength(2);
    const follows = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    const radios = within(panel).getByRole('radiogroup', { name: 'Chart' });
    const donut = within(panel).getByRole('img', { name: LOSS_DONUT_LABEL });
    expect(follows(radios, row)).toBe(true);
    expect(follows(row, donut)).toBe(true);
  });

  it('the Summary has the same one row with both tables side by side', async () => {
    await openLive(dir(0, NINE, RATES0), dir(1));
    const rateCol = must(rateTable(summary()).parentElement, 'the rate column');
    const lossCol = must(lossTable(summary()).parentElement, 'the loss column');
    expect(rateCol).toHaveClass('col-12', 'col-md-6');
    expect(lossCol).toHaveClass('col-12', 'col-md-6');
    const row = must(rateCol.closest('.row'), 'the row');
    expect(Array.from(row.children)).toEqual([rateCol, lossCol]);
  });

  it('in the Delay histogram view they come after the delay chart', async () => {
    await openLive(dir(0, NINE, RATES0), dir(1));
    fireEvent.click(radio(0, 'Delay histogram'));
    const bar = await findDelayBar(card(0));
    const box = must(bar.closest('[role="img"]'));
    expect(box).toHaveAttribute('aria-label', expect.stringMatching(DELAY_BAR_LABEL));
    expect(box.compareDocumentPosition(rateTable(card(0))) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('exist once in a link tab and once in the Summary (also with the radio on Packet loss)', async () => {
    await openLive(dir(0, NINE, RATES0), dir(1));
    fireEvent.click(radio(0, 'Packet loss'));
    expect(rateTable(card(0))).toBeInTheDocument();
    expect(lossTable(card(0))).toBeInTheDocument();
    expect(rateTable(summary())).toBeInTheDocument();
    expect(lossTable(summary())).toBeInTheDocument();
  });
});

// ---- the rate table -----------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 51 T2): the rate table cells, L1 = L2 + 20 B x 8 x pps', () => {
  it('link 0: TX L1 4.08 Gbit/s, RX L1 3.56 Gbit/s, TX L2 4 Gbit/s, RX L2 3.5 Gbit/s', async () => {
    await openLive(dir(0, {}, RATES0), dir(1, {}, RATES1));
    // 4e9 + 160 x 5e5 = 4.08e9; 3.5e9 + 160 x 4e5 = 3.564e9
    expect(bodyCells(rateTable(card(0)))).toEqual(['4.08 Gbit/s', '3.56 Gbit/s', '4 Gbit/s', '3.5 Gbit/s']);
  });

  it('link 1: TX L1 1.02 Gbit/s, RX L1 2.03 Gbit/s, TX L2 1 Gbit/s, RX L2 2 Gbit/s', async () => {
    await openLive(dir(0, {}, RATES0), dir(1, {}, RATES1));
    expect(bodyCells(rateTable(card(1)))).toEqual(['1.02 Gbit/s', '2.03 Gbit/s', '1 Gbit/s', '2 Gbit/s']);
  });

  it('reads the overhead from the pps of its own side: 8 kbit/s at 10 pps is 9.6 Kbit/s at L1 (+1600 bit/s)', async () => {
    await openLive(dir(0, {}, { tx_bps: 8e3, tx_pps: 10, rx_bps: 8e3, rx_pps: 0 }), dir(1));
    expect(bodyCells(rateTable(card(0)))).toEqual(['9.6 Kbit/s', '8 Kbit/s', '8 Kbit/s', '8 Kbit/s']);
  });

  it('reads 0 Bit/s in all four cells for a link at rest', async () => {
    await openLive(dir(0, {}, { tx_bps: 0, tx_pps: 0, rx_bps: 0, rx_pps: 0 }), dir(1));
    expect(bodyCells(rateTable(card(0)))).toEqual(['0 Bit/s', '0 Bit/s', '0 Bit/s', '0 Bit/s']);
  });

  it('the Summary sums L2 and the pps of the links: L1 = 5.1 / 5.6 Gbit/s, L2 = 5 / 5.5 Gbit/s', async () => {
    await openLive(dir(0, {}, RATES0), dir(1, {}, RATES1));
    // TX L2 4e9 + 1e9 = 5e9, pps 6e5: 5e9 + 160 x 6e5 = 5.096e9; RX L2 5.5e9, pps 6e5: 5.596e9
    expect(bodyCells(rateTable(summary()))).toEqual(['5.1 Gbit/s', '5.6 Gbit/s', '5 Gbit/s', '5.5 Gbit/s']);
  });

  it('follows the polls', async () => {
    vi.useFakeTimers();
    serveLive(dir(0, {}, RATES0), dir(1, {}, RATES1));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(bodyCells(rateTable(card(0)))[2]).toBe('4 Gbit/s');
    serveLive(dir(0, {}, { ...RATES0, tx_bps: 2e9, tx_pps: 1e5 }), dir(1, {}, RATES1));
    await tick(2000);
    // 2e9 + 160 x 1e5 = 2.016e9
    expect(bodyCells(rateTable(card(0)))[0]).toBe('2.02 Gbit/s');
    expect(bodyCells(rateTable(card(0)))[2]).toBe('2 Gbit/s');
  });
});

// ---- the loss table -----------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 51 T2): the loss table cells', () => {
  it('link 0: 100 lost of 900 forwarded reads 100.00, 10.00 %, and the out of order count 1.23 M', async () => {
    await openLive(dir(0, { ...NINE, out_of_order: 1234567 }), dir(1));
    // 100 x 100 / (100 + 900) = 10.00; tx_pkts (800) and rx_pkts (100000) are not the forwarded frames
    expect(bodyCells(lossTable(card(0)))).toEqual(['100.00', '10.00 %', '1.23 M']);
  });

  it('link 1: 1 lost of 2 forwarded reads 1.00, 33.33 %, and 0 out of order reads 0', async () => {
    await openLive(dir(0, NINE), dir(1, { loss_drops: 1, tx_sent: 2, out_of_order: 0 }));
    expect(bodyCells(lossTable(card(1)))).toEqual(['1.00', '33.33 %', '0']);
  });

  it('nothing lost reads 0, "0.00 %" (also with nothing forwarded: no NaN) and 0', async () => {
    await openLive(dir(0, { tx_sent: 0, out_of_order: 0 }), dir(1, { tx_sent: 500, out_of_order: 0 }));
    expect(bodyCells(lossTable(card(0)))).toEqual(['0', '0.00 %', '0']);
    expect(bodyCells(lossTable(card(1)))).toEqual(['0', '0.00 %', '0']);
  });

  it.each(CAUSE_KEYS)('counts %s as a lost frame (5 lost of 95 forwarded: 5.00 %)', async (key) => {
    await openLive(dir(0, { [key]: 5, tx_sent: 95, out_of_order: 3 }), dir(1));
    expect(bodyCells(lossTable(card(0)))).toEqual(['5.00', '5.00 %', '3.00']);
  });

  it('does not count the rejects or the NIC diagnostics as lost', async () => {
    await openLive(
      dir(0, {
        dpds_rejects: 50,
        nic_rx_crc_errors: 60,
        nic_rx_symbol_errors: 70,
        nic_rx_length_errors: 80,
        tx_sent: 100,
      }),
      dir(1),
    );
    expect(bodyCells(lossTable(card(0)))).toEqual(['0', '0.00 %', '7.00']);
  });

  it('the Summary sums lost, forwarded and out of order: 101 lost of 902 forwarded is 10.07 %', async () => {
    await openLive(
      dir(0, { ...NINE, tx_sent: 900, out_of_order: 1234567 }),
      dir(1, { loss_drops: 1, tx_sent: 2, out_of_order: 3 }),
    );
    // 101 x 100 / (101 + 902) = 10.0697 (not the mean of 10.00 and 33.33); out of order 1234570
    expect(bodyCells(lossTable(summary()))).toEqual(['101.00', '10.07 %', '1.23 M']);
  });

  it('follows the polls', async () => {
    vi.useFakeTimers();
    serveLive(dir(0, { loss_drops: 1, tx_sent: 3 }), dir(1));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(bodyCells(lossTable(card(0)))).toEqual(['1.00', '25.00 %', '7.00']); // 100 / 4; out_of_order = the fixture's 7
    serveLive(dir(0, { loss_drops: 2, tx_sent: 6, out_of_order: 8 }), dir(1));
    await tick(2000);
    expect(bodyCells(lossTable(card(0)))).toEqual(['2.00', '25.00 %', '8.00']);
  });
});

describe('Dashboard (spec 51 T2): the tooltips carry the exact counts', () => {
  const cell = (panel: HTMLElement, i: number) => must(within(lossTable(panel)).getAllByRole('cell')[i]);
  const trigger = (panel: HTMLElement, i: number) =>
    must(cell(panel, i).querySelector('span'), `the span of cell ${i}`);

  it('Lost Frames: hovering shows the exact lost count, e.g. 1234567 for 1.23 M', async () => {
    await openLive(dir(0, { loss_drops: 1234567, tx_sent: 1 }), dir(1));
    const user = userEvent.setup();
    expect(cell(card(0), 0)).toHaveTextContent(/^1\.23 M$/);
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    await user.hover(trigger(card(0), 0));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('1234567');
    await user.unhover(trigger(card(0), 0));
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });

  it('Lost Frames: the nine causes added up, not formatted: 100', async () => {
    await openLive(dir(0, NINE), dir(1));
    const user = userEvent.setup();
    await user.hover(trigger(card(0), 0));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('100');
  });

  it('Out of Order: hovering shows the exact count, e.g. 7654321 for 7.65 M', async () => {
    await openLive(dir(0, { out_of_order: 7654321 }), dir(1));
    const user = userEvent.setup();
    expect(cell(card(0), 2)).toHaveTextContent(/^7\.65 M$/);
    await user.hover(trigger(card(0), 2));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('7654321');
    await user.unhover(trigger(card(0), 2));
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });

  it('Out of Order of 0 has the tooltip "0"', async () => {
    await openLive(dir(0, { out_of_order: 0 }), dir(1));
    const user = userEvent.setup();
    await user.hover(trigger(card(0), 2));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('0');
  });

  it('the Summary has them too, with the sums: 101 and 1234570', async () => {
    await openLive(dir(0, { ...NINE, out_of_order: 1234567 }), dir(1, { loss_drops: 1, out_of_order: 3 }));
    const user = userEvent.setup();
    await user.hover(trigger(summary(), 0));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('101');
    await user.unhover(trigger(summary(), 0));
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
    await user.hover(trigger(summary(), 2));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('1234570');
  });

  it('the Frame Loss Ratio cell has no tooltip trigger', async () => {
    await openLive(dir(0, NINE), dir(1));
    expect(cell(card(0), 1).querySelector('span')).toBeNull();
  });
});

// ---- after the stop -----------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 51 T2): after the stop', () => {
  const S1 = 1762000000;
  const totals = (over: Partial<ReturnType<typeof fx.totals>> = {}) =>
    fx.totals({
      loss_drops: 36,
      limiter_drops: 24,
      buffer_full_drops: 16,
      nic_tx_drops: 8,
      nic_rx_missed: 6,
      nic_rx_nombuf: 4,
      nic_rx_port_buffer_discards: 3,
      nic_rx_phy_discards: 2,
      nic_tx_phy_discards: 1,
      rx_pkts: 10000,
      out_of_order: 1234567,
      ...over,
    });
  function ended(state: 'stopped' | 'failed') {
    serveHistory({
      startedAt: S1,
      samples: { '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) },
      totals: {
        '0': totals(),
        '1': totals({
          loss_drops: 1,
          nic_tx_drops: 0,
          limiter_drops: 0,
          buffer_full_drops: 0,
          nic_rx_missed: 0,
          nic_rx_nombuf: 0,
          nic_rx_port_buffer_discards: 0,
          nic_rx_phy_discards: 0,
          nic_tx_phy_discards: 0,
          out_of_order: 3,
        }),
      },
    });
    api.getEmulation.mockResolvedValue(fx.emulation(state, { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 5000 }));
  }

  it.each(['stopped', 'failed'] as const)(
    'the rates read 0 Bit/s in all four cells (%s), like the pills',
    async (state) => {
      ended(state);
      renderApp('/');
      await findCard(1);
      for (const panel of [card(0), card(1), summary()]) {
        expect(bodyCells(rateTable(panel))).toEqual(['0 Bit/s', '0 Bit/s', '0 Bit/s', '0 Bit/s']);
      }
    },
  );

  it.each(['stopped', 'failed'] as const)(
    'the loss table keeps the run’s totals: 100 lost, 1.23 M out of order (%s)',
    async (state) => {
      ended(state);
      renderApp('/');
      await findCard(1);
      await waitFor(() => expect(bodyCells(lossTable(card(0)))[0]).toBe('100.00'));
      const row = bodyCells(lossTable(card(0)));
      expect(row[2]).toBe('1.23 M');
      expect(row[1]).toBe('1.00 %'); // 100 x 100 / (100 + 9900): tx_sent of the totals is the forwarded count
      expect(bodyCells(lossTable(card(1)))[1]).toBe('0.01 %'); // 1 x 100 / (1 + 9900) = 0.0101
      expect(bodyCells(lossTable(card(1)))[0]).toBe('1.00');
      expect(bodyCells(lossTable(card(1)))[2]).toBe('3.00');
    },
  );

  it('the Summary keeps the sums of the totals: 101 lost, 1234570 out of order', async () => {
    ended('stopped');
    renderApp('/');
    await findCard(1);
    await waitFor(() => expect(bodyCells(lossTable(summary()))[0]).toBe('101.00'));
    expect(bodyCells(lossTable(summary()))[2]).toBe('1.23 M');
    const user = userEvent.setup();
    const out = must(within(lossTable(summary())).getAllByRole('cell')[2].querySelector('span'));
    await user.hover(out);
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('1234570');
  });

  it('the tooltips of an ended run carry the totals’ exact counts', async () => {
    ended('stopped');
    renderApp('/');
    await findCard(1);
    await waitFor(() => expect(bodyCells(lossTable(card(0)))[0]).toBe('100.00'));
    const user = userEvent.setup();
    const cells = within(lossTable(card(0))).getAllByRole('cell');
    await user.hover(must(cells[0].querySelector('span')));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('100');
    await user.unhover(must(cells[0].querySelector('span')));
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
    await user.hover(must(cells[2].querySelector('span')));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('1234567');
  });

  it('a run that ends under the open page: the rates fall to 0, the loss table keeps the totals', async () => {
    vi.useFakeTimers();
    ended('stopped'); // the history of the run; the live stats below take over until the stop
    serveLive(dir(0, { ...NINE, tx_sent: 900, out_of_order: 1234567 }, RATES0), dir(1, {}, RATES1)); // the stats are live again
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(bodyCells(rateTable(card(0)))).toEqual(['4.08 Gbit/s', '3.56 Gbit/s', '4 Gbit/s', '3.5 Gbit/s']);
    expect(bodyCells(lossTable(card(0)))).toEqual(['100.00', '10.00 %', '1.23 M']);
    ended('stopped');
    await tick(3000);
    expect(bodyCells(rateTable(card(0)))).toEqual(['0 Bit/s', '0 Bit/s', '0 Bit/s', '0 Bit/s']);
    expect(bodyCells(lossTable(card(0)))[0]).toBe('100.00');
    expect(bodyCells(lossTable(card(0)))[2]).toBe('1.23 M');
  });
});

describe('Dashboard (spec 51 T2): before any run', () => {
  it('shows neither the rate table nor the loss table, in a link tab or in the Summary', async () => {
    seedLinks([fx.link(), fx.linkRev()]);
    renderApp('/');
    await findTabs(2);
    for (const panel of [summary(), card(0), card(1)]) {
      expect(within(panel).queryAllByRole('table')).toHaveLength(0);
      expect(within(panel).queryByText('TX L1')).not.toBeInTheDocument();
      expect(within(panel).queryByText('Lost Frames')).not.toBeInTheDocument();
    }
  });
});
