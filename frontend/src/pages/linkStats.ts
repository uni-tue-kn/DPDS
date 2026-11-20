import type { Counters, DirectionStats, HistoryTotals } from '../api/types';
import { addNullable } from '../common/format';
import { type DropCounts, dropsOfCounters, dropsOfTotals } from './LossDonut';

/** Everything the chart area of a link (or of all links added up) shows besides the time series. */
export interface LinkStats {
  drops: DropCounts;
  /** The live rates; 0 once the run has ended. */
  txBps: number;
  txPps: number;
  rxBps: number;
  rxPps: number;
  /** The frames received per size bucket. */
  frameSizes: number[];
  /** The frames the link forwarded. */
  forwarded: number;
  outOfOrder: number;
}

export const NO_DROPS: DropCounts = {
  loss: 0,
  limiter: 0,
  bufferFull: 0,
  nicTx: 0,
  missed: 0,
  nombuf: 0,
  portBuffer: 0,
  rxPhy: 0,
  txPhy: 0,
};

/** The frame size buckets of the donut. */
export const SIZE_BUCKETS = 8;

export const NO_STATS: LinkStats = {
  drops: NO_DROPS,
  txBps: 0,
  txPps: 0,
  rxBps: 0,
  rxPps: 0,
  frameSizes: new Array<number>(SIZE_BUCKETS).fill(0),
  forwarded: 0,
  outOfOrder: 0,
};

const addDrops = (a: DropCounts, b: DropCounts): DropCounts => ({
  loss: a.loss + b.loss,
  limiter: a.limiter + b.limiter,
  bufferFull: a.bufferFull + b.bufferFull,
  nicTx: a.nicTx + b.nicTx,
  missed: a.missed + b.missed,
  nombuf: a.nombuf + b.nombuf,
  portBuffer: addNullable(a.portBuffer, b.portBuffer),
  rxPhy: addNullable(a.rxPhy, b.rxPhy),
  txPhy: addNullable(a.txPhy, b.txPhy),
});

/** The sum of several links. */
export const addStats = (a: LinkStats, b: LinkStats): LinkStats => ({
  drops: addDrops(a.drops, b.drops),
  txBps: a.txBps + b.txBps,
  txPps: a.txPps + b.txPps,
  rxBps: a.rxBps + b.rxBps,
  rxPps: a.rxPps + b.rxPps,
  frameSizes: a.frameSizes.map((v, i) => v + (b.frameSizes[i] ?? 0)),
  forwarded: a.forwarded + b.forwarded,
  outOfOrder: a.outOfOrder + b.outOfOrder,
});

/** From the live statistics of `GET /api/stats`. */
export const statsOfLive = (d: DirectionStats): LinkStats => {
  const c: Counters = d.counters;
  return {
    drops: dropsOfCounters(c),
    txBps: d.tx_bps,
    txPps: d.tx_pps,
    rxBps: d.rx_bps,
    rxPps: d.rx_pps,
    frameSizes: c.frame_sizes,
    forwarded: c.tx_sent,
    outOfOrder: c.out_of_order,
  };
};

/** From the run's final counters of an ended run: its rates are 0 and its totals stay. */
export const statsOfTotals = (t: HistoryTotals): LinkStats => ({
  ...NO_STATS,
  drops: dropsOfTotals(t),
  frameSizes: t.frame_sizes,
  forwarded: t.tx_sent,
  outOfOrder: t.out_of_order,
});
