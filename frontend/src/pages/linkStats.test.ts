// The numbers behind the dashboard: drops by cause, the sum of links (null counts of an SR-IOV VF stay out), the ended run's totals.
import * as fx from '../test/fixtures';
import { dropsOfCounters, dropsOfTotals, totalDrops } from './LossDonut';
import { addStats, NO_STATS, statsOfLive, statsOfTotals } from './linkStats';

const NULL_PORT_COUNTS = { nic_rx_port_buffer_discards: null, nic_rx_phy_discards: null, nic_tx_phy_discards: null };

describe('drops', () => {
  it('the total is the sum of all causes; unmeasurable ones count as 0', () => {
    expect(totalDrops(dropsOfCounters(fx.counters()))).toBe(10);
    expect(totalDrops(dropsOfTotals(fx.totals()))).toBe(100);
    expect(totalDrops(dropsOfCounters(fx.counters({ ...NULL_PORT_COUNTS, nic_rx_missed: 5 })))).toBe(15);
    expect(totalDrops(dropsOfCounters(fx.counters({ nic_tx_phy_discards: 9, nic_rx_port_buffer_discards: 1 })))).toBe(
      20,
    );
  });
});

describe('link statistics', () => {
  it('live statistics carry the rates, the forwarded and out-of-order frames and the frame sizes', () => {
    const s = statsOfLive(fx.dirStats(0));
    expect(s).toMatchObject({
      txBps: 8.25e9,
      rxBps: 9.5e9,
      txPps: 1200000,
      rxPps: 1234567,
      forwarded: 990,
      outOfOrder: 7,
    });
    expect(s.frameSizes).toEqual(fx.counters().frame_sizes);
  });

  it('an ended run takes its counts from the totals and has no rates', () => {
    const s = statsOfTotals(fx.totals());
    expect(s).toMatchObject({ txBps: 0, rxBps: 0, txPps: 0, rxPps: 0, forwarded: 9900, outOfOrder: 9 });
    expect(totalDrops(s.drops)).toBe(100);
  });

  it("links add up field by field; a count missing on one link is the other link's, missing on both stays missing", () => {
    const a = statsOfLive(fx.dirStats(0, { counters: fx.counters({ ...NULL_PORT_COUNTS, loss_drops: 1 }) }));
    const b = statsOfLive(fx.dirStats(1, { counters: fx.counters({ ...NULL_PORT_COUNTS, nic_rx_phy_discards: 4 }) }));
    const sum = addStats(a, b);
    expect(sum.drops.loss).toBe(5);
    expect(sum.drops.rxPhy).toBe(4);
    expect(sum.drops.portBuffer).toBeNull();
    expect(sum.txBps).toBe(2 * 8.25e9);
    expect(sum.forwarded).toBe(1980);
    expect(sum.frameSizes).toEqual(fx.counters().frame_sizes.map((n) => 2 * n));
  });

  it('NO_STATS is neutral for the sum', () => {
    const s = statsOfTotals(fx.totals());
    expect(addStats(NO_STATS, s)).toEqual(s);
  });
});
